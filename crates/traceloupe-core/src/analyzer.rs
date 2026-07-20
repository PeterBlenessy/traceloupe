//! Security Check scan engine: evaluates an [`IndicatorSet`] against the
//! cache DB (Tier A) and the backup manifest's file list, writing findings to
//! the `findings` table under a `scan_runs` row.
//!
//! The engine is deliberately pure over its inputs — cache rows plus an
//! optional iterator of manifest `(domain, relative_path)` entries — so tests
//! seed an in-memory cache and synthetic entries without a real backup.
//! Command-layer wiring (progress events, ManifestIndex) happens in src-tauri.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;

use crate::cache::CacheDb;
use crate::error::Result;
use crate::indicators::{Indicator, IndicatorKind, IndicatorSet};
use crate::sidecar::CancelToken;

/// Analyzer modules in run order. `manifest` only runs when the caller can
/// supply manifest entries (it needs the backup, not just the cache).
pub const MODULES: &[&str] = &[
    "apps",
    "messages",
    "attachments",
    "safari",
    "notes",
    "calendar",
    "contacts",
    "interactions",
    "manifest",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanKind {
    Explicit,
    Passive,
}

impl ScanKind {
    fn as_str(self) -> &'static str {
        match self {
            ScanKind::Explicit => "explicit",
            ScanKind::Passive => "passive",
        }
    }

    /// The Passive Check is apps-only by default (see CONTEXT.md): presence
    /// evidence with near-zero false positives, no content scanning.
    pub fn default_modules(self) -> Vec<&'static str> {
        match self {
            ScanKind::Explicit => MODULES.to_vec(),
            ScanKind::Passive => vec!["apps"],
        }
    }
}

#[derive(Debug)]
pub struct ScanOutcome {
    pub run_id: i64,
    pub findings: usize,
    pub cancelled: bool,
}

/// Indicator lookups shaped for the scan hot paths.
struct Lookup<'a> {
    domains: HashMap<&'a str, &'a Indicator>,
    urls: Vec<&'a Indicator>,
    emails: HashMap<&'a str, &'a Indicator>,
    bundle_ids: HashMap<&'a str, &'a Indicator>,
    file_names: HashMap<String, &'a Indicator>,
    file_paths: Vec<&'a Indicator>,
}

impl<'a> Lookup<'a> {
    fn build(set: &'a IndicatorSet) -> Self {
        let mut l = Lookup {
            domains: HashMap::new(),
            urls: Vec::new(),
            emails: HashMap::new(),
            bundle_ids: HashMap::new(),
            file_names: HashMap::new(),
            file_paths: Vec::new(),
        };
        for i in &set.indicators {
            match i.kind {
                IndicatorKind::Domain => {
                    l.domains.insert(i.value.as_str(), i);
                }
                IndicatorKind::Url => l.urls.push(i),
                IndicatorKind::Email => {
                    l.emails.insert(i.value.as_str(), i);
                }
                IndicatorKind::BundleId => {
                    l.bundle_ids.insert(i.value.as_str(), i);
                }
                IndicatorKind::FileName => {
                    l.file_names.insert(i.value.to_ascii_lowercase(), i);
                }
                IndicatorKind::FilePath => l.file_paths.push(i),
                // ProcessName matching needs Tier B sources (ADDaily,
                // DataUsage); CertSha1/FileHash/Ip have no Tier A surface.
                _ => {}
            }
        }
        l
    }

    /// Exact-or-subdomain match: `sub.evil.example` matches an `evil.example`
    /// indicator, `notevil.example` does not.
    fn match_host(&self, host: &str) -> Option<&'a Indicator> {
        let mut rest = host;
        loop {
            if let Some(i) = self.domains.get(rest) {
                return Some(i);
            }
            match rest.split_once('.') {
                Some((_, tail)) if !tail.is_empty() => rest = tail,
                _ => return None,
            }
        }
    }
}

/// Extract candidate hostnames from free text, conservatively: dot-separated
/// labels, alphabetic TLD of ≥2 chars, no label edge hyphens, bounded by
/// non-hostname characters so substrings of longer hostnames never surface.
fn extract_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let bytes = text.as_bytes();
    let is_host_char = |c: u8| c.is_ascii_alphanumeric() || c == b'.' || c == b'-';
    let mut i = 0;
    while i < bytes.len() {
        if !is_host_char(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_host_char(bytes[i]) {
            i += 1;
        }
        let token = text[start..i].trim_matches(|c| c == '.' || c == '-');
        if token.len() < 4 || !token.contains('.') {
            continue;
        }
        let labels: Vec<&str> = token.split('.').collect();
        let valid = labels
            .iter()
            .all(|l| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-'))
            && labels
                .last()
                .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
        if !valid {
            continue;
        }
        let host = token.to_ascii_lowercase();
        if seen.insert(host.clone()) {
            out.push(host);
        }
    }
    out
}

/// Extract candidate email addresses (lowercased) from free text.
fn extract_emails(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in text.split(|c: char| c.is_whitespace() || "<>,;:()[]\"'".contains(c)) {
        let t = raw.trim_matches(|c: char| c == '.' || c == ',');
        let Some(at) = t.find('@') else { continue };
        if at == 0 || at + 1 >= t.len() || !t[at + 1..].contains('.') {
            continue;
        }
        let email = t.to_ascii_lowercase();
        if seen.insert(email.clone()) {
            out.push(email);
        }
    }
    out
}

fn host_of_url(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    (!host.is_empty() && host.contains('.')).then_some(host)
}

/// One pending findings row.
struct Hit<'a> {
    indicator: &'a Indicator,
    module: &'static str,
    ref_kind: &'static str,
    ref_id: Option<i64>,
    context: String,
    event_time: Option<i64>,
}

struct Sink<'a> {
    hits: Vec<Hit<'a>>,
    dedupe: HashSet<(&'static str, String, &'static str, Option<i64>)>,
}

impl<'a> Sink<'a> {
    fn new() -> Self {
        Sink {
            hits: Vec::new(),
            dedupe: HashSet::new(),
        }
    }

    fn push(&mut self, hit: Hit<'a>) {
        let key = (
            hit.module,
            hit.indicator.value.clone(),
            hit.ref_kind,
            hit.ref_id,
        );
        if self.dedupe.insert(key) {
            self.hits.push(hit);
        }
    }
}

fn snippet(text: &str) -> String {
    let t = text.trim();
    let mut end = t.len().min(160);
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].to_string()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Scan free text against domain/url/email indicators.
fn scan_text<'a>(
    lookup: &Lookup<'a>,
    text: &str,
    module: &'static str,
    ref_kind: &'static str,
    ref_id: Option<i64>,
    event_time: Option<i64>,
    sink: &mut Sink<'a>,
) {
    for host in extract_hosts(text) {
        if let Some(ind) = lookup.match_host(&host) {
            sink.push(Hit {
                indicator: ind,
                module,
                ref_kind,
                ref_id,
                context: snippet(text),
                event_time,
            });
        }
    }
    if text.contains("://") {
        for ind in &lookup.urls {
            if text.contains(ind.value.as_str()) {
                sink.push(Hit {
                    indicator: ind,
                    module,
                    ref_kind,
                    ref_id,
                    context: snippet(text),
                    event_time,
                });
            }
        }
    }
    if text.contains('@') {
        for email in extract_emails(text) {
            if let Some(ind) = lookup.emails.get(email.as_str()) {
                sink.push(Hit {
                    indicator: ind,
                    module,
                    ref_kind,
                    ref_id,
                    context: snippet(text),
                    event_time,
                });
            }
        }
    }
}

/// Run a scan. `manifest_entries` is `(domain, relative_path)` for every file
/// in the backup manifest; pass `None` when only cache modules should run
/// (the `manifest` module is then skipped). `feeds_json` describes the
/// indicator feeds used (stored on the run for the report header).
/// `progress` receives `(module, index, total)` before each module runs.
#[allow(clippy::too_many_arguments)] // a scan genuinely needs all of these inputs
pub fn run_scan(
    db: &CacheDb,
    set: &IndicatorSet,
    kind: ScanKind,
    modules: &[&'static str],
    mut manifest_entries: Option<&mut dyn Iterator<Item = (String, String)>>,
    feeds_json: &str,
    cancel: &CancelToken,
    mut progress: impl FnMut(&str, usize, usize),
) -> Result<ScanOutcome> {
    let conn = db.conn();
    let modules: Vec<&'static str> = modules
        .iter()
        .copied()
        .filter(|m| *m != "manifest" || manifest_entries.is_some())
        .collect();
    conn.execute(
        "INSERT INTO scan_runs (kind, started_at, status, modules_json, feeds_json, indicator_count)
         VALUES (?1, ?2, 'running', ?3, ?4, ?5)",
        params![
            kind.as_str(),
            now_unix(),
            serde_json::to_string(&modules).unwrap_or_else(|_| "[]".into()),
            feeds_json,
            set.len() as i64,
        ],
    )?;
    let run_id = conn.last_insert_rowid();

    let lookup = Lookup::build(set);
    let mut sink = Sink::new();
    let total = modules.len();
    let mut cancelled = false;

    for (idx, module) in modules.iter().enumerate() {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        progress(module, idx, total);
        match *module {
            "apps" => {
                let mut stmt = conn.prepare("SELECT bundle_id FROM installed_apps")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                for bundle in rows.filter_map(|r| r.ok()) {
                    if let Some(ind) = lookup.bundle_ids.get(bundle.to_ascii_lowercase().as_str()) {
                        sink.push(Hit {
                            indicator: ind,
                            module: "apps",
                            ref_kind: "app",
                            ref_id: None,
                            context: bundle.clone(),
                            event_time: None,
                        });
                    }
                }
            }
            "messages" => {
                let mut stmt = conn.prepare(
                    "SELECT id, body, sender, sent_at FROM messages WHERE body IS NOT NULL",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                })?;
                for (id, body, sender, sent_at) in rows.filter_map(|r| r.ok()) {
                    if cancel.is_cancelled() {
                        cancelled = true;
                        break;
                    }
                    scan_text(
                        &lookup,
                        &body,
                        "messages",
                        "message",
                        Some(id),
                        sent_at,
                        &mut sink,
                    );
                    if let Some(sender) = sender {
                        if let Some(ind) = lookup.emails.get(sender.to_ascii_lowercase().as_str()) {
                            sink.push(Hit {
                                indicator: ind,
                                module: "messages",
                                ref_kind: "message",
                                ref_id: Some(id),
                                context: format!("sender: {sender}"),
                                event_time: sent_at,
                            });
                        }
                    }
                }
            }
            "attachments" => {
                let mut stmt = conn.prepare(
                    "SELECT a.id, a.filename, m.sent_at FROM attachments a
                     LEFT JOIN messages m ON m.id = a.message_id
                     WHERE a.filename IS NOT NULL",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                })?;
                for (id, filename, sent_at) in rows.filter_map(|r| r.ok()) {
                    let base = filename.rsplit('/').next().unwrap_or(&filename);
                    if let Some(ind) = lookup.file_names.get(&base.to_ascii_lowercase()) {
                        sink.push(Hit {
                            indicator: ind,
                            module: "attachments",
                            ref_kind: "attachment",
                            ref_id: Some(id),
                            context: filename.clone(),
                            event_time: sent_at,
                        });
                    }
                }
            }
            "safari" => {
                let mut stmt = conn.prepare(
                    "SELECT id, url, visited_at, 'safari_history' FROM safari_history
                     UNION ALL
                     SELECT id, url, date_added, 'safari_bookmark' FROM safari_bookmarks
                     WHERE url IS NOT NULL",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?;
                for (id, url, at, table) in rows.filter_map(|r| r.ok()) {
                    if cancel.is_cancelled() {
                        cancelled = true;
                        break;
                    }
                    let ref_kind: &'static str = if table == "safari_history" {
                        "safari_history"
                    } else {
                        "safari_bookmark"
                    };
                    if let Some(host) = host_of_url(&url) {
                        if let Some(ind) = lookup.match_host(&host.to_ascii_lowercase()) {
                            sink.push(Hit {
                                indicator: ind,
                                module: "safari",
                                ref_kind,
                                ref_id: Some(id),
                                context: snippet(&url),
                                event_time: at,
                            });
                        }
                    }
                    for ind in &lookup.urls {
                        if url.contains(ind.value.as_str()) {
                            sink.push(Hit {
                                indicator: ind,
                                module: "safari",
                                ref_kind,
                                ref_id: Some(id),
                                context: snippet(&url),
                                event_time: at,
                            });
                        }
                    }
                }
            }
            "notes" => {
                let mut stmt = conn.prepare(
                    "SELECT id, coalesce(title,'') || ' ' || coalesce(snippet,'') || ' ' ||
                            coalesce(body_html,''), modified_at FROM notes",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                })?;
                for (id, text, at) in rows.filter_map(|r| r.ok()) {
                    scan_text(&lookup, &text, "notes", "note", Some(id), at, &mut sink);
                }
            }
            "calendar" => {
                let mut stmt = conn.prepare(
                    "SELECT id, coalesce(title,'') || ' ' || coalesce(notes,'') || ' ' ||
                            coalesce(location,'') || ' ' || coalesce(url,''), start_at
                     FROM calendar_events",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                })?;
                for (id, text, at) in rows.filter_map(|r| r.ok()) {
                    scan_text(
                        &lookup,
                        &text,
                        "calendar",
                        "calendar_event",
                        Some(id),
                        at,
                        &mut sink,
                    );
                }
            }
            "contacts" => {
                let mut stmt =
                    conn.prepare("SELECT id, emails_json FROM contacts WHERE emails_json != '[]'")?;
                let rows =
                    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                #[derive(serde::Deserialize)]
                struct LV {
                    value: String,
                }
                for (id, emails_json) in rows.filter_map(|r| r.ok()) {
                    let Ok(entries) = serde_json::from_str::<Vec<LV>>(&emails_json) else {
                        continue;
                    };
                    for e in entries {
                        if let Some(ind) = lookup.emails.get(e.value.to_ascii_lowercase().as_str())
                        {
                            sink.push(Hit {
                                indicator: ind,
                                module: "contacts",
                                ref_kind: "contact",
                                ref_id: Some(id),
                                context: e.value.clone(),
                                event_time: None,
                            });
                        }
                    }
                }
            }
            "interactions" => {
                let mut stmt = conn.prepare(
                    "SELECT id, identifier, last_at FROM interactions WHERE identifier IS NOT NULL",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                })?;
                for (id, identifier, at) in rows.filter_map(|r| r.ok()) {
                    if let Some(ind) = lookup.emails.get(identifier.to_ascii_lowercase().as_str()) {
                        sink.push(Hit {
                            indicator: ind,
                            module: "interactions",
                            ref_kind: "interaction",
                            ref_id: Some(id),
                            context: identifier.clone(),
                            event_time: at,
                        });
                    }
                }
            }
            "manifest" => {
                if let Some(entries) = manifest_entries.as_deref_mut() {
                    for (domain, rel_path) in entries {
                        if cancel.is_cancelled() {
                            cancelled = true;
                            break;
                        }
                        // AppDomain-com.evil.app / AppDomainGroup-group.x /
                        // AppDomainPlugin-… all carry an app identifier.
                        if let Some((prefix, bundle)) = domain.split_once('-') {
                            if prefix.starts_with("AppDomain") {
                                if let Some(ind) =
                                    lookup.bundle_ids.get(bundle.to_ascii_lowercase().as_str())
                                {
                                    sink.push(Hit {
                                        indicator: ind,
                                        module: "manifest",
                                        ref_kind: "manifest_domain",
                                        ref_id: None,
                                        context: domain.clone(),
                                        event_time: None,
                                    });
                                }
                            }
                        }
                        let base = rel_path.rsplit('/').next().unwrap_or(&rel_path);
                        if let Some(ind) = lookup.file_names.get(&base.to_ascii_lowercase()) {
                            sink.push(Hit {
                                indicator: ind,
                                module: "manifest",
                                ref_kind: "manifest_file",
                                ref_id: None,
                                context: format!("{domain}: {rel_path}"),
                                event_time: None,
                            });
                        }
                        for ind in &lookup.file_paths {
                            if rel_path == ind.value
                                || rel_path.ends_with(&format!("/{}", ind.value))
                            {
                                sink.push(Hit {
                                    indicator: ind,
                                    module: "manifest",
                                    ref_kind: "manifest_file",
                                    ref_id: None,
                                    context: format!("{domain}: {rel_path}"),
                                    event_time: None,
                                });
                            }
                        }
                    }
                }
            }
            other => {
                // Unknown module ids are a programming error upstream; skip.
                let _ = other;
            }
        }
        if cancelled {
            break;
        }
    }

    // Write findings in one transaction.
    conn.execute_batch("BEGIN")?;
    let mut insert = conn.prepare(
        "INSERT INTO findings (run_id, severity, kind, module, malware, matched_value,
                               context, ref_kind, ref_id, event_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for h in &sink.hits {
        insert.execute(params![
            run_id,
            serde_json::to_value(h.indicator.severity)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "info".into()),
            serde_json::to_value(h.indicator.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "domain".into()),
            h.module,
            h.indicator.malware,
            h.indicator.value,
            h.context,
            h.ref_kind,
            h.ref_id,
            h.event_time,
        ])?;
    }
    drop(insert);
    conn.execute(
        "UPDATE scan_runs SET status = ?2, finished_at = ?3 WHERE id = ?1",
        params![
            run_id,
            if cancelled { "cancelled" } else { "done" },
            now_unix()
        ],
    )?;
    conn.execute_batch("COMMIT")?;

    Ok(ScanOutcome {
        run_id,
        findings: sink.hits.len(),
        cancelled,
    })
}

// ---------------------------------------------------------------------------
// CSV report export
// ---------------------------------------------------------------------------

/// Escape one CSV field per RFC 4180: wrap in quotes when it contains a comma,
/// quote, CR or LF, doubling any embedded quotes.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn format_epoch(secs: Option<i64>) -> String {
    let Some(secs) = secs else {
        return String::new();
    };
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => dt
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| secs.to_string()),
        Err(_) => secs.to_string(),
    }
}

/// Render a scan run's findings as a CSV report (columns modeled on iMazing's:
/// Severity, Time, Threat, Kind, Module, Matched, Context). A leading comment
/// block records scan metadata + feed attribution (CC-BY requires it). Returns
/// the whole document as a string. `app_version` is stamped into the header.
pub fn export_report_csv(db: &CacheDb, run_id: i64, app_version: &str) -> Result<String> {
    let conn = db.conn();
    let (kind, started, finished, status, feeds_json, indicator_count): (
        String,
        i64,
        Option<i64>,
        String,
        String,
        Option<i64>,
    ) = conn.query_row(
        "SELECT kind, started_at, finished_at, status, feeds_json, indicator_count
         FROM scan_runs WHERE id = ?1",
        [run_id],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        },
    )?;

    let mut out = String::new();
    out.push_str("# TraceLoupe Security Check report\n");
    out.push_str(&format!("# App version: {app_version}\n"));
    out.push_str(&format!("# Scan kind: {kind}\n"));
    out.push_str(&format!("# Started: {}\n", format_epoch(Some(started))));
    out.push_str(&format!("# Finished: {}\n", format_epoch(finished)));
    out.push_str(&format!("# Status: {status}\n"));
    out.push_str(&format!(
        "# Indicators evaluated: {}\n",
        indicator_count.map(|c| c.to_string()).unwrap_or_default()
    ));
    // Feeds used (for attribution).
    if let Ok(feeds) = serde_json::from_str::<Vec<serde_json::Value>>(&feeds_json) {
        for f in feeds {
            if let Some(src) = f.get("source").and_then(|v| v.as_str()) {
                let count = f.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                out.push_str(&format!("# Feed: {src} ({count} indicators)\n"));
            }
        }
    }
    out.push_str(
        "# Indicators are CC-BY (Amnesty International, MVT project, Echap). \
         A match is not proof of compromise.\n",
    );

    out.push_str("Severity,Time,Threat,Kind,Module,Matched,Context\n");
    let mut stmt = conn.prepare(
        "SELECT severity, event_time, malware, kind, module, matched_value, context
         FROM findings WHERE run_id = ?1
         ORDER BY CASE severity WHEN 'critical' THEN 3 WHEN 'warning' THEN 2 ELSE 1 END DESC,
                  module, id",
    )?;
    let rows = stmt.query_map([run_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    for row in rows {
        let (severity, event_time, malware, kind, module, matched, context) = row?;
        let fields = [
            severity,
            format_epoch(event_time),
            malware,
            kind,
            module,
            matched,
            context.unwrap_or_default(),
        ];
        out.push_str(
            &fields
                .iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::{FeedClass, IndicatorSet, LoadedFeed, Severity};

    fn ind(kind: IndicatorKind, value: &str, malware: &str, severity: Severity) -> Indicator {
        Indicator {
            kind,
            value: value.to_string(),
            malware: malware.to_string(),
            sources: vec!["test".into()],
            severity,
        }
    }

    fn test_set() -> IndicatorSet {
        IndicatorSet::from_feeds(vec![LoadedFeed {
            source: "test".into(),
            indicators: vec![
                ind(
                    IndicatorKind::Domain,
                    "evil.example",
                    "TestWare",
                    Severity::Warning,
                ),
                ind(
                    IndicatorKind::Url,
                    "https://short.url/x1",
                    "TestWare",
                    Severity::Warning,
                ),
                ind(
                    IndicatorKind::Email,
                    "operator@evil.example",
                    "TestWare",
                    Severity::Warning,
                ),
                ind(
                    IndicatorKind::BundleId,
                    "com.evil.tracker",
                    "TestWare",
                    Severity::Critical,
                ),
                ind(
                    IndicatorKind::FileName,
                    "implant.plist",
                    "TestWare",
                    Severity::Critical,
                ),
                ind(
                    IndicatorKind::FilePath,
                    "Library/Caches/implant.db",
                    "TestWare",
                    Severity::Critical,
                ),
            ],
            skipped: vec![],
        }])
    }

    fn seeded_db() -> CacheDb {
        let db = CacheDb::open_in_memory().unwrap();
        let c = db.conn();
        c.execute_batch(
            "INSERT INTO installed_apps (bundle_id) VALUES ('Com.Evil.Tracker'), ('com.good.app');
             INSERT INTO threads (id, identifier, service) VALUES (1, 't', 'SMS');
             INSERT INTO messages (id, thread_id, body, sent_at) VALUES
               (1, 1, 'click https://sub.evil.example/payload now', 1700000100),
               (2, 1, 'totally notevil.example.org here', 1700000200),
               (3, 1, 'from operator@evil.example: hi', 1700000300),
               (4, 1, 'go to https://short.url/x1 fast', 1700000400);
             INSERT INTO attachments (id, message_id, filename) VALUES (1, 1, 'implant.plist');
             INSERT INTO safari_history (id, url, visited_at) VALUES
               (1, 'https://evil.example/login', 1700001000),
               (2, 'https://github.com/AssoEchap/stalkerware-indicators', 1700001100);
             INSERT INTO notes (id, title, snippet, body_html) VALUES
               (1, 'todo', 'check evil.example soon', NULL);
             INSERT INTO calendar_events (id, title, notes, start_at) VALUES
               (1, 'sync', 'dial in via evil.example', 1700002000);
             INSERT INTO contacts (id, first_name, emails_json) VALUES
               (1, 'Op', '[{\"label\":\"work\",\"value\":\"Operator@Evil.Example\"}]');
             INSERT INTO interactions (id, identifier) VALUES (1, 'operator@evil.example');",
        )
        .unwrap();
        db
    }

    fn count(db: &CacheDb, module: &str) -> i64 {
        db.conn()
            .query_row(
                "SELECT count(*) FROM findings WHERE module = ?1",
                [module],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn seeded_scan_finds_each_class_once() {
        let db = seeded_db();
        let set = test_set();
        let mut manifest = vec![
            (
                "AppDomain-com.evil.tracker".to_string(),
                "Documents/x".to_string(),
            ),
            (
                "HomeDomain".to_string(),
                "Library/Caches/implant.db".to_string(),
            ),
            (
                "HomeDomain".to_string(),
                "Library/Preferences/ok.plist".to_string(),
            ),
        ]
        .into_iter();
        let cancel = CancelToken::new();
        let mut seen_modules = Vec::new();
        let outcome = run_scan(
            &db,
            &set,
            ScanKind::Explicit,
            MODULES,
            Some(&mut manifest),
            "[]",
            &cancel,
            |m, _, _| seen_modules.push(m.to_string()),
        )
        .unwrap();
        assert!(!outcome.cancelled);
        assert_eq!(seen_modules.len(), MODULES.len());

        // apps: bundle id matched case-insensitively.
        assert_eq!(count(&db, "apps"), 1);
        // messages: subdomain host (msg 1), url indicator (msg 4), and msg 3
        // yields two findings — the email indicator AND the domain indicator
        // (the domain appears inside the address; both are real matches).
        assert_eq!(count(&db, "messages"), 4);
        // attachments: implant.plist.
        assert_eq!(count(&db, "attachments"), 1);
        // safari: host match only — the github URL must NOT match anything.
        assert_eq!(count(&db, "safari"), 1);
        assert_eq!(count(&db, "notes"), 1);
        assert_eq!(count(&db, "calendar"), 1);
        assert_eq!(count(&db, "contacts"), 1);
        assert_eq!(count(&db, "interactions"), 1);
        // manifest: app domain + file path + file name (path row matches both
        // the FileName basename and the FilePath indicator → deduped by value,
        // so 3 total: bundle, implant.db path, implant.plist? No — implant.db
        // basename is not a FileName indicator. bundle + path = 2.
        assert_eq!(count(&db, "manifest"), 2);

        // Severity flows from the indicator.
        let critical: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM findings WHERE severity = 'critical'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(critical, 4); // app + attachment + manifest bundle + manifest path

        // The near-miss host produced nothing.
        let miss: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM findings WHERE matched_value LIKE '%notevil%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(miss, 0);

        // Run bookkeeping.
        let (status, kind): (String, String) = db
            .conn()
            .query_row(
                "SELECT status, kind FROM scan_runs WHERE id = ?1",
                [outcome.run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "done");
        assert_eq!(kind, "explicit");
    }

    #[test]
    fn clean_cache_yields_zero_findings() {
        let db = CacheDb::open_in_memory().unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO installed_apps (bundle_id) VALUES ('com.apple.mobilesafari');
                 INSERT INTO threads (id, identifier, service) VALUES (1, 't', 'SMS');
                 INSERT INTO messages (id, thread_id, body, sent_at)
                   VALUES (1, 1, 'see you at apple.com tomorrow', 1);",
            )
            .unwrap();
        let outcome = run_scan(
            &db,
            &test_set(),
            ScanKind::Explicit,
            MODULES,
            None,
            "[]",
            &CancelToken::new(),
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(outcome.findings, 0);
    }

    #[test]
    fn passive_default_is_apps_only() {
        let db = seeded_db();
        let modules = ScanKind::Passive.default_modules();
        let outcome = run_scan(
            &db,
            &test_set(),
            ScanKind::Passive,
            &modules,
            None,
            "[]",
            &CancelToken::new(),
            |_, _, _| {},
        )
        .unwrap();
        // Only the installed-app match; no content findings.
        assert_eq!(outcome.findings, 1);
        assert_eq!(count(&db, "apps"), 1);
        assert_eq!(count(&db, "messages"), 0);
    }

    #[test]
    fn csv_report_has_header_metadata_and_escapes_fields() {
        let db = seeded_db();
        let mut manifest = std::iter::empty::<(String, String)>();
        let outcome = run_scan(
            &db,
            &test_set(),
            ScanKind::Explicit,
            MODULES,
            Some(&mut manifest),
            r#"[{"source":"echap/ioc","class":"stalkerware","count":2746,"skipped":0}]"#,
            &CancelToken::new(),
            |_, _, _| {},
        )
        .unwrap();

        // Inject a finding whose context needs CSV quoting.
        db.conn()
            .execute(
                "INSERT INTO findings (run_id, severity, kind, module, malware, matched_value, context)
                 VALUES (?1, 'warning', 'domain', 'messages', 'TestWare', 'evil.example',
                         'contains, comma and \"quote\"')",
                [outcome.run_id],
            )
            .unwrap();

        let csv = export_report_csv(&db, outcome.run_id, "9.9.9").unwrap();
        assert!(csv.contains("# TraceLoupe Security Check report"));
        assert!(csv.contains("# App version: 9.9.9"));
        assert!(csv.contains("# Feed: echap/ioc (2746 indicators)"));
        assert!(csv.contains("# Scan kind: explicit"));
        assert!(csv.contains("Severity,Time,Threat,Kind,Module,Matched,Context"));
        // The tricky field is quoted and its quotes doubled.
        assert!(csv.contains("\"contains, comma and \"\"quote\"\"\""));
        // Every finding row is present (header lines start with '#').
        let data_rows = csv
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with("Severity,"))
            .filter(|l| !l.is_empty())
            .count();
        assert!(data_rows >= 1);
    }

    #[test]
    fn cancellation_marks_run_cancelled() {
        let db = seeded_db();
        let cancel = CancelToken::new();
        cancel.cancel();
        let outcome = run_scan(
            &db,
            &test_set(),
            ScanKind::Explicit,
            MODULES,
            None,
            "[]",
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert!(outcome.cancelled);
        let status: String = db
            .conn()
            .query_row(
                "SELECT status FROM scan_runs WHERE id = ?1",
                [outcome.run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "cancelled");
    }

    #[test]
    fn tokenizer_rejects_substrings_and_punycode_survives() {
        let hosts = extract_hosts("visit xn--evil-hra.example and (evil.example), 1.2.3.4 x");
        assert!(hosts.contains(&"xn--evil-hra.example".to_string()));
        assert!(hosts.contains(&"evil.example".to_string()));
        // Numeric TLD (an IPv4) is not a hostname candidate.
        assert!(!hosts.iter().any(|h| h == "1.2.3.4"));
        // Substring inside a longer registered name must not match.
        let l_set = test_set();
        let lookup = Lookup::build(&l_set);
        assert!(lookup.match_host("sub.evil.example").is_some());
        assert!(lookup.match_host("notevil.example").is_none());
        assert!(lookup.match_host("evil.example.attacker.com").is_none());
    }
}
