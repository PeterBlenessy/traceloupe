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
    "process_names",
    "profiles",
    "tcc",
];

/// A process observed running on the device, from a Tier-B artifact
/// (DataUsage.sqlite or OSAnalytics ADDaily). The scan matches `name` against
/// process-name indicators — the artifact class that first exposed Pegasus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedProcess {
    /// The process/executable name as recorded (may be a `UUID/bundle` form in
    /// DataUsage, or a bare daemon name in ADDaily).
    pub name: String,
    /// The associated bundle id, when the source records one (DataUsage).
    pub bundle_id: Option<String>,
    /// Which Tier-B artifact this came from ("DataUsage" | "OSAnalytics").
    pub source: &'static str,
    /// Unix seconds of the most recent activity, if known.
    pub last_seen: Option<i64>,
}

/// An installed configuration profile (from `ProfileTruth.plist` +
/// `PayloadManifest.plist`). A profile can grant broad control over a device —
/// an unexpected or hidden one is a classic stalkerware install vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedProfile {
    pub display_name: String,
    pub organization: Option<String>,
    pub uuid: Option<String>,
    /// Hidden from Settings (present in `PayloadManifest.HiddenProfiles`) — a
    /// profile the user cannot see is a strong monitoring signal.
    pub hidden: bool,
    /// Device-management capabilities detected in the profile's settings
    /// (MDM enrollment, global proxy, always-on VPN, web-content filter).
    pub capabilities: Vec<String>,
    /// Hostnames/URLs referenced by the profile, for indicator matching.
    pub hosts: Vec<String>,
}

/// A permission an app was granted, from `TCC.db` (Transparency, Consent &
/// Control). A stalkerware app holding microphone/camera/location access is
/// strong corroborating evidence beyond mere installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionGrant {
    /// The app's bundle id (`access.client`).
    pub client: String,
    /// Friendly permission name (e.g. "Microphone", "Camera").
    pub service: String,
    /// Whether this permission is surveillance-relevant (mic/camera/location/…).
    pub sensitive: bool,
    /// Unix seconds the grant was last modified.
    pub last_modified: Option<i64>,
}

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
    process_names: HashMap<String, &'a Indicator>,
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
            process_names: HashMap::new(),
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
                IndicatorKind::ProcessName => {
                    // Process names are matched case-sensitively in MVT, but
                    // fold case so an indicator's casing never hides a match.
                    l.process_names.insert(i.value.to_ascii_lowercase(), i);
                }
                // CertSha1/FileHash/Ip have no Tier A/B surface a backup carries.
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

/// A heuristic finding not backed by an indicator match — e.g. a hidden or
/// device-management configuration profile flagged for review. Carries its own
/// severity/attribution rather than borrowing from an [`Indicator`].
struct StructuralFinding {
    severity: &'static str,
    kind: &'static str,
    module: &'static str,
    malware: String,
    matched_value: String,
    context: String,
    ref_kind: &'static str,
    event_time: Option<i64>,
}

struct Sink<'a> {
    hits: Vec<Hit<'a>>,
    structural: Vec<StructuralFinding>,
    dedupe: HashSet<(&'static str, String, &'static str, Option<i64>)>,
}

impl<'a> Sink<'a> {
    fn new() -> Self {
        Sink {
            hits: Vec::new(),
            structural: Vec::new(),
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

    fn push_structural(&mut self, f: StructuralFinding) {
        let key = (f.module, f.matched_value.clone(), f.ref_kind, None);
        if self.dedupe.insert(key) {
            self.structural.push(f);
        }
    }

    fn len(&self) -> usize {
        self.hits.len() + self.structural.len()
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
/// (the `manifest` module is then skipped). `processes` are Tier-B observed
/// processes (empty skips the `process_names` module); `profiles` are installed
/// configuration profiles (empty skips the `profiles` module). `feeds_json`
/// describes the indicator feeds used (stored on the run for the report header).
/// `progress` receives `(module, index, total)` before each module runs.
#[allow(clippy::too_many_arguments)] // a scan genuinely needs all of these inputs
pub fn run_scan(
    db: &CacheDb,
    set: &IndicatorSet,
    kind: ScanKind,
    modules: &[&'static str],
    mut manifest_entries: Option<&mut dyn Iterator<Item = (String, String)>>,
    processes: &[ObservedProcess],
    profiles: &[ObservedProfile],
    grants: &[PermissionGrant],
    feeds_json: &str,
    cancel: &CancelToken,
    mut progress: impl FnMut(&str, usize, usize),
) -> Result<ScanOutcome> {
    let conn = db.conn();
    let modules: Vec<&'static str> = modules
        .iter()
        .copied()
        .filter(|m| *m != "manifest" || manifest_entries.is_some())
        .filter(|m| *m != "process_names" || !processes.is_empty())
        .filter(|m| *m != "tcc" || !grants.is_empty())
        .filter(|m| *m != "profiles" || !profiles.is_empty())
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
            "process_names" => {
                for p in processes {
                    // Match the recorded name and its basename (DataUsage stores
                    // a `UUID/bundle` form; malicious daemon names are bare).
                    let base = p.name.rsplit('/').next().unwrap_or(&p.name);
                    for candidate in [p.name.as_str(), base] {
                        if let Some(ind) = lookup
                            .process_names
                            .get(candidate.to_ascii_lowercase().as_str())
                        {
                            sink.push(Hit {
                                indicator: ind,
                                module: "process_names",
                                ref_kind: "process",
                                ref_id: None,
                                context: format!("{} ({})", p.name, p.source),
                                event_time: p.last_seen,
                            });
                        }
                    }
                    // A DataUsage bundle name is also a bundle-id surface for
                    // processes that never appear in installed_apps.
                    if let Some(bundle) = &p.bundle_id {
                        if let Some(ind) =
                            lookup.bundle_ids.get(bundle.to_ascii_lowercase().as_str())
                        {
                            sink.push(Hit {
                                indicator: ind,
                                module: "process_names",
                                ref_kind: "process",
                                ref_id: None,
                                context: format!("{bundle} ({})", p.source),
                                event_time: p.last_seen,
                            });
                        }
                    }
                }
            }
            "profiles" => {
                for prof in profiles {
                    // Indicator matches on any string the profile carries
                    // (display name, organization, referenced hosts/URLs).
                    let scanned = format!(
                        "{} {} {}",
                        prof.display_name,
                        prof.organization.as_deref().unwrap_or(""),
                        prof.hosts.join(" ")
                    );
                    scan_text(
                        &lookup, &scanned, "profiles", "profile", None, None, &mut sink,
                    );

                    // One structural review finding per profile, escalating on
                    // the strongest signal. Attribution is a category, not a
                    // named threat — a profile is evidence to review, not proof.
                    let org = prof
                        .organization
                        .as_deref()
                        .map(|o| format!(" from {o}"))
                        .unwrap_or_default();
                    let (severity, malware, note): (&str, &str, String) = if prof.hidden {
                        (
                            "warning",
                            "Hidden configuration profile",
                            "hidden from Settings — a profile you cannot see is a monitoring red flag"
                                .to_string(),
                        )
                    } else if !prof.capabilities.is_empty() {
                        (
                            "info",
                            "Device-management profile",
                            format!(
                                "can control/monitor the device ({}); confirm you installed it",
                                prof.capabilities.join(", ")
                            ),
                        )
                    } else {
                        (
                            "info",
                            "Configuration profile",
                            "an installed profile; review it if you didn't add it".to_string(),
                        )
                    };
                    sink.push_structural(StructuralFinding {
                        severity,
                        kind: "profile",
                        module: "profiles",
                        malware: malware.to_string(),
                        matched_value: prof.display_name.clone(),
                        context: format!("{}{org}: {note}", prof.display_name),
                        ref_kind: "profile",
                        event_time: None,
                    });
                }
            }
            "tcc" => {
                // Aggregate grants by client so a matched app produces one
                // finding listing all the sensitive permissions it holds.
                let mut by_client: HashMap<&str, (Vec<&str>, Option<i64>)> = HashMap::new();
                for g in grants {
                    let entry = by_client.entry(g.client.as_str()).or_default();
                    if g.sensitive && !entry.0.contains(&g.service.as_str()) {
                        entry.0.push(g.service.as_str());
                    }
                    entry.1 = entry.1.max(g.last_modified);
                }
                for (client, (mut services, last)) in by_client {
                    // Only a client that matches a stalkerware/watchware
                    // bundle-id indicator is surfaced — an app holding camera
                    // access is normal; a *known monitoring app* holding it is
                    // the signal.
                    if let Some(ind) = lookup.bundle_ids.get(client.to_ascii_lowercase().as_str()) {
                        services.sort_unstable();
                        let perms = if services.is_empty() {
                            "device permissions".to_string()
                        } else {
                            format!("{} access", services.join(", "))
                        };
                        sink.push(Hit {
                            indicator: ind,
                            module: "tcc",
                            ref_kind: "permission",
                            ref_id: None,
                            context: format!("{client} holds {perms}"),
                            event_time: last,
                        });
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
    for f in &sink.structural {
        insert.execute(params![
            run_id,
            f.severity,
            f.kind,
            f.module,
            f.malware,
            f.matched_value,
            f.context,
            f.ref_kind,
            Option::<i64>::None,
            f.event_time,
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
        findings: sink.len(),
        cancelled,
    })
}

// ---------------------------------------------------------------------------
// Tier-B process-activity extraction (DataUsage.sqlite + OSAnalytics ADDaily)
// ---------------------------------------------------------------------------

/// Core Data / CFAbsoluteTime epoch: seconds between 2001-01-01 and the Unix
/// epoch. DataUsage timestamps are seconds since 2001; ADDaily uses real dates.
const MAC_ABSOLUTE_EPOCH: i64 = 978_307_200;

/// Parse observed processes from a `DataUsage.sqlite` (WirelessDomain,
/// `Library/Databases/DataUsage.sqlite`). Reads `ZPROCESS`: process name,
/// bundle name, and last-seen timestamp. Errors only on an unreadable /
/// non-DataUsage database, so the caller can skip it and continue the scan.
pub fn parse_datausage(db_path: &std::path::Path) -> Result<Vec<ObservedProcess>> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT ZPROCNAME, ZBUNDLENAME, ZTIMESTAMP FROM ZPROCESS
         WHERE ZPROCNAME IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        let name: String = r.get(0)?;
        let bundle: Option<String> = r.get(1)?;
        let ts: Option<f64> = r.get(2)?;
        Ok(ObservedProcess {
            name,
            bundle_id: bundle,
            source: "DataUsage",
            last_seen: ts.map(|t| t as i64 + MAC_ABSOLUTE_EPOCH),
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Parse observed processes from `com.apple.osanalytics.addaily.plist`
/// (HomeDomain, `Library/Preferences/…`). The `netUsageBaseline` dictionary is
/// keyed by process name; each value is `[date, wifi_in, wifi_out, wwan_in,
/// wwan_out]`. This is the artifact class the original Pegasus discovery used.
pub fn parse_addaily(plist_bytes: &[u8]) -> Result<Vec<ObservedProcess>> {
    let root = plist::Value::from_reader(std::io::Cursor::new(plist_bytes))
        .map_err(|e| crate::error::Error::Parse(format!("addaily plist: {e}")))?;
    let Some(baseline) = root
        .as_dictionary()
        .and_then(|d| d.get("netUsageBaseline"))
        .and_then(|v| v.as_dictionary())
    else {
        // No baseline dict (empty/older plist) — nothing to scan, not an error.
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (name, value) in baseline {
        let last_seen = value
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_date())
            .and_then(|d| {
                std::time::SystemTime::from(d)
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|dur| dur.as_secs() as i64)
            });
        out.push(ObservedProcess {
            name: name.clone(),
            bundle_id: None,
            source: "OSAnalytics",
            last_seen,
        });
    }
    Ok(out)
}

/// Union setting keys / values whose presence marks a device-control capability
/// (MDM enrollment, global proxy, always-on VPN, web-content filter). Matched
/// case-insensitively as substrings against a profile's setting keys.
const RISKY_PROFILE_CAPABILITIES: &[(&str, &str)] = &[
    ("mdm", "MDM enrollment"),
    ("globalhttpproxy", "global HTTP proxy"),
    ("proxy", "proxy"),
    ("vpn", "VPN"),
    ("webcontentfilter", "web-content filter"),
    ("contentfilter", "content filter"),
];

/// Recursively collect string leaf values and dictionary keys from a plist.
fn collect_plist(value: &plist::Value, strings: &mut Vec<String>, keys: &mut Vec<String>) {
    match value {
        plist::Value::String(s) => strings.push(s.clone()),
        plist::Value::Array(a) => {
            for v in a {
                collect_plist(v, strings, keys);
            }
        }
        plist::Value::Dictionary(d) => {
            for (k, v) in d {
                keys.push(k.clone());
                collect_plist(v, strings, keys);
            }
        }
        _ => {}
    }
}

/// Split a `ProfileTruth` key of the form `Name from Org (UUID)` (or
/// `Name (UUID)`, or just `Name`) into its parts.
fn parse_profile_key(key: &str) -> (String, Option<String>, Option<String>) {
    let mut head = key.trim();
    let mut uuid = None;
    if head.ends_with(')') {
        if let Some(open) = head.rfind('(') {
            let inside = &head[open + 1..head.len() - 1];
            if !inside.is_empty() && inside.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
                uuid = Some(inside.to_string());
                head = head[..open].trim();
            }
        }
    }
    match head.rsplit_once(" from ") {
        Some((name, org)) => (name.trim().to_string(), Some(org.trim().to_string()), uuid),
        None => (head.to_string(), None, uuid),
    }
}

/// A candidate host/URL string is host-like: has a dot, no whitespace, bounded
/// length. Keeps the indicator scan focused and cheap.
fn host_like(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 4 && s.len() < 256 && s.contains('.') && !s.chars().any(|c| c.is_whitespace())
}

// ---------------------------------------------------------------------------
// Configuration-profile extraction (ProfileTruth + PayloadManifest)
// ---------------------------------------------------------------------------

/// Parse installed configuration profiles from `ProfileTruth.plist` (the
/// authoritative installed-profile list, keyed by `Name from Org (UUID)`) and
/// `PayloadManifest.plist` (which lists `HiddenProfiles`). Both live in the
/// `SysSharedContainerDomain-systemgroup.com.apple.configurationprofiles`
/// domain under `Library/ConfigurationProfiles/`.
pub fn parse_configuration_profiles(
    profiletruth_bytes: &[u8],
    payloadmanifest_bytes: Option<&[u8]>,
) -> Result<Vec<ObservedProfile>> {
    let truth = plist::Value::from_reader(std::io::Cursor::new(profiletruth_bytes))
        .map_err(|e| crate::error::Error::Parse(format!("ProfileTruth plist: {e}")))?;
    let Some(dict) = truth.as_dictionary() else {
        return Ok(Vec::new());
    };

    // Hidden-profile set from PayloadManifest.HiddenProfiles.
    let hidden: HashSet<String> = payloadmanifest_bytes
        .and_then(|b| plist::Value::from_reader(std::io::Cursor::new(b)).ok())
        .and_then(|v| v.as_dictionary().cloned())
        .and_then(|d| d.get("HiddenProfiles").and_then(|v| v.as_array()).cloned())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_string().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for (key, value) in dict {
        let (display_name, organization, uuid) = parse_profile_key(key);
        let mut strings = Vec::new();
        let mut keys = Vec::new();
        collect_plist(value, &mut strings, &mut keys);

        // Capabilities: risky setting keys detected anywhere in the profile.
        let mut capabilities = Vec::new();
        let lower_keys: Vec<String> = keys.iter().map(|k| k.to_ascii_lowercase()).collect();
        for (needle, label) in RISKY_PROFILE_CAPABILITIES {
            if lower_keys.iter().any(|k| k.contains(needle))
                && !capabilities.contains(&label.to_string())
            {
                capabilities.push(label.to_string());
            }
        }

        let hosts: Vec<String> = strings.into_iter().filter(|s| host_like(s)).collect();

        out.push(ObservedProfile {
            display_name,
            organization,
            uuid,
            hidden: hidden.contains(key),
            capabilities,
            hosts,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// TCC (permissions) extraction
// ---------------------------------------------------------------------------

/// Map a `kTCCService*` id to a friendly name and whether it is
/// surveillance-relevant (microphone, camera, screen, location, photos,
/// contacts, speech, motion). Unknown services degrade to a de-prefixed name
/// and non-sensitive (conservative: they won't drive a finding on their own).
fn friendly_service(service: &str) -> (String, bool) {
    match service {
        "kTCCServiceMicrophone" => ("Microphone".into(), true),
        "kTCCServiceCamera" => ("Camera".into(), true),
        "kTCCServiceScreenCapture" => ("Screen recording".into(), true),
        "kTCCServicePhotos" | "kTCCServicePhotosAdd" => ("Photos".into(), true),
        "kTCCServiceMediaLibrary" => ("Media library".into(), true),
        "kTCCServiceAddressBook" => ("Contacts".into(), true),
        "kTCCServiceCalendar" => ("Calendar".into(), true),
        "kTCCServiceReminders" => ("Reminders".into(), true),
        "kTCCServiceSpeechRecognition" => ("Speech recognition".into(), true),
        "kTCCServiceMotion" => ("Motion & fitness".into(), true),
        "kTCCServiceLocation" => ("Location".into(), true),
        "kTCCServiceFaceID" => ("Face ID".into(), true),
        other => (
            other
                .strip_prefix("kTCCService")
                .unwrap_or(other)
                .to_string(),
            false,
        ),
    }
}

/// Parse granted permissions from `TCC.db` (HomeDomain,
/// `Library/TCC/TCC.db`). Only allowed grants (`auth_value` 2/3, or the older
/// `allowed = 1`) are returned. `access.client` is the app's bundle id.
pub fn parse_tcc(db_path: &std::path::Path) -> Result<Vec<PermissionGrant>> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // iOS 14+ uses `auth_value` (0=denied, 2=allowed, 3=limited); older builds
    // use `allowed` (0/1). Try the modern column, fall back to the legacy one.
    let sql_modern = "SELECT service, client, last_modified FROM access WHERE auth_value IN (2, 3)";
    let sql_legacy = "SELECT service, client, last_modified FROM access WHERE allowed = 1";
    let mut stmt = match conn.prepare(sql_modern) {
        Ok(s) => s,
        Err(_) => conn.prepare(sql_legacy)?,
    };
    let rows = stmt.query_map([], |r| {
        let service: String = r.get(0)?;
        let client: String = r.get(1)?;
        let last_modified: Option<i64> = r.get(2).ok();
        Ok((service, client, last_modified))
    })?;
    let mut out = Vec::new();
    for row in rows.filter_map(|r| r.ok()) {
        let (service, client, last_modified) = row;
        let (friendly, sensitive) = friendly_service(&service);
        out.push(PermissionGrant {
            client,
            service: friendly,
            sensitive,
            last_modified,
        });
    }
    Ok(out)
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
                ind(
                    IndicatorKind::ProcessName,
                    "roleaboutd",
                    "TestWare",
                    Severity::Critical,
                ),
            ],
            skipped: vec![],
        }])
    }

    fn test_processes() -> Vec<ObservedProcess> {
        vec![
            // Bare daemon name (ADDaily form) — matches the indicator.
            ObservedProcess {
                name: "roleaboutd".into(),
                bundle_id: None,
                source: "OSAnalytics",
                last_seen: Some(1_700_000_500),
            },
            // UUID/bundle form (DataUsage) whose basename is benign.
            ObservedProcess {
                name: "1FB47783/com.apple.compass".into(),
                bundle_id: Some("com.apple.compass".into()),
                source: "DataUsage",
                last_seen: Some(1_700_000_600),
            },
        ]
    }

    fn test_profiles() -> Vec<ObservedProfile> {
        vec![
            // A hidden profile → Warning (structural).
            ObservedProfile {
                display_name: "Support Helper".into(),
                organization: Some("Acme".into()),
                uuid: Some("abc-123".into()),
                hidden: true,
                capabilities: vec![],
                hosts: vec![],
            },
            // A device-management profile referencing an indicator host → the
            // host matches (Warning) AND a structural device-mgmt finding (Info).
            ObservedProfile {
                display_name: "Work MDM".into(),
                organization: Some("IT".into()),
                uuid: Some("def-456".into()),
                hidden: false,
                capabilities: vec!["MDM enrollment".into()],
                hosts: vec!["evil.example".into()],
            },
            // A plain profile → one Info review finding.
            ObservedProfile {
                display_name: "Printer".into(),
                organization: Some("Office".into()),
                uuid: None,
                hidden: false,
                capabilities: vec![],
                hosts: vec!["printer.local".into()],
            },
        ]
    }

    fn test_grants() -> Vec<PermissionGrant> {
        vec![
            // The stalkerware app holds two sensitive grants → one aggregated
            // finding.
            PermissionGrant {
                client: "com.evil.tracker".into(),
                service: "Microphone".into(),
                sensitive: true,
                last_modified: Some(1_700_000_700),
            },
            PermissionGrant {
                client: "com.evil.tracker".into(),
                service: "Camera".into(),
                sensitive: true,
                last_modified: Some(1_700_000_800),
            },
            // A benign app with camera access → no finding (not an indicator).
            PermissionGrant {
                client: "com.burbn.instagram".into(),
                service: "Camera".into(),
                sensitive: true,
                last_modified: Some(1_700_000_900),
            },
        ]
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
        let processes = test_processes();
        let profiles = test_profiles();
        let grants = test_grants();
        let mut seen_modules = Vec::new();
        let outcome = run_scan(
            &db,
            &set,
            ScanKind::Explicit,
            MODULES,
            Some(&mut manifest),
            &processes,
            &profiles,
            &grants,
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
        // process_names: the bare daemon name matches; the compass basename does
        // not. One finding.
        assert_eq!(count(&db, "process_names"), 1);
        // profiles: hidden profile (Warning structural) + MDM profile (host
        // indicator match Warning + Device-management structural Info) + plain
        // profile (Info structural) = 4.
        assert_eq!(count(&db, "profiles"), 4);
        // The hidden profile is graded Warning, the plain one Info.
        let profile_sev: Vec<String> = {
            let mut stmt = db
                .conn()
                .prepare("SELECT severity FROM findings WHERE module='profiles' ORDER BY severity")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert!(profile_sev.iter().any(|s| s == "warning"));
        assert!(profile_sev.iter().any(|s| s == "info"));
        // tcc: the stalkerware client's two grants aggregate into one finding;
        // the benign instagram grant does not match. One finding.
        assert_eq!(count(&db, "tcc"), 1);
        let tcc_ctx: String = db
            .conn()
            .query_row("SELECT context FROM findings WHERE module='tcc'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(tcc_ctx.contains("Camera") && tcc_ctx.contains("Microphone"));

        // Severity flows from the indicator.
        let critical: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM findings WHERE severity = 'critical'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // app + attachment + manifest bundle + manifest path + process name + tcc.
        assert_eq!(critical, 6);

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
    fn parse_datausage_reads_zprocess() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("DataUsage.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZPROCESS (Z_PK INTEGER PRIMARY KEY, ZFIRSTTIMESTAMP TIMESTAMP,
                ZTIMESTAMP TIMESTAMP, ZBUNDLENAME VARCHAR, ZPROCNAME VARCHAR);
             INSERT INTO ZPROCESS (ZPROCNAME, ZBUNDLENAME, ZTIMESTAMP) VALUES
                ('UUID/com.apple.compass', 'com.apple.compass', 548420038.9),
                ('roleaboutd', NULL, 700000000.0),
                (NULL, 'com.nulls', 1.0);",
        )
        .unwrap();
        drop(conn);
        let procs = parse_datausage(&path).unwrap();
        // The NULL-name row is filtered out.
        assert_eq!(procs.len(), 2);
        let compass = procs.iter().find(|p| p.name.ends_with("compass")).unwrap();
        assert_eq!(compass.bundle_id.as_deref(), Some("com.apple.compass"));
        assert_eq!(compass.source, "DataUsage");
        // Mac-absolute → Unix: 548420038 + 978307200.
        assert_eq!(compass.last_seen, Some(548420038 + MAC_ABSOLUTE_EPOCH));
    }

    #[test]
    fn parse_datausage_rejects_non_datausage_db() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("other.sqlite");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (x);")
            .unwrap();
        assert!(parse_datausage(&path).is_err());
    }

    #[test]
    fn parse_addaily_reads_netusagebaseline_keys() {
        // Build a minimal ADDaily plist: netUsageBaseline dict keyed by process.
        let mut baseline = plist::Dictionary::new();
        baseline.insert(
            "roleaboutd".into(),
            plist::Value::Array(vec![
                plist::Value::Date(std::time::UNIX_EPOCH.into()),
                plist::Value::Integer(1.into()),
            ]),
        );
        baseline.insert(
            "callservicesd".into(),
            plist::Value::Array(vec![plist::Value::Date(std::time::UNIX_EPOCH.into())]),
        );
        let mut root = plist::Dictionary::new();
        root.insert(
            "netUsageBaseline".into(),
            plist::Value::Dictionary(baseline),
        );
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &plist::Value::Dictionary(root)).unwrap();

        let procs = parse_addaily(&bytes).unwrap();
        assert_eq!(procs.len(), 2);
        assert!(procs
            .iter()
            .any(|p| p.name == "roleaboutd" && p.source == "OSAnalytics"));
    }

    #[test]
    fn parse_addaily_missing_baseline_is_empty_not_error() {
        let root = plist::Value::Dictionary(plist::Dictionary::new());
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &root).unwrap();
        assert!(parse_addaily(&bytes).unwrap().is_empty());
    }

    #[test]
    fn profile_key_parsing() {
        assert_eq!(
            parse_profile_key(
                "PaperCut Profile from mobileprint.lund.se (06dd7752-f276-465b-9876-fb7cf674ff55)"
            ),
            (
                "PaperCut Profile".to_string(),
                Some("mobileprint.lund.se".to_string()),
                Some("06dd7752-f276-465b-9876-fb7cf674ff55".to_string())
            )
        );
        // No org, no uuid.
        assert_eq!(
            parse_profile_key("Plain Profile"),
            ("Plain Profile".to_string(), None, None)
        );
        // A trailing parenthetical that isn't a UUID stays part of the name.
        assert_eq!(
            parse_profile_key("Weird (not a uuid)"),
            ("Weird (not a uuid)".to_string(), None, None)
        );
    }

    #[test]
    fn parse_configuration_profiles_extracts_fields_and_hidden() {
        // ProfileTruth: two profiles, one carrying an MDM setting key + a host.
        let mut mdm_settings = plist::Dictionary::new();
        mdm_settings.insert(
            "MDMServerURL".into(),
            plist::Value::String("https://mdm.evil.example/enroll".into()),
        );
        let mut truth = plist::Dictionary::new();
        truth.insert(
            "Work MDM from IT (aaaa-1111)".into(),
            plist::Value::Dictionary(mdm_settings),
        );
        truth.insert(
            "Printer from Office (bbbb-2222)".into(),
            plist::Value::Dictionary(plist::Dictionary::new()),
        );
        let mut truth_bytes = Vec::new();
        plist::to_writer_binary(&mut truth_bytes, &plist::Value::Dictionary(truth)).unwrap();

        // PayloadManifest: the printer profile is hidden.
        let mut manifest = plist::Dictionary::new();
        manifest.insert(
            "HiddenProfiles".into(),
            plist::Value::Array(vec![plist::Value::String(
                "Printer from Office (bbbb-2222)".into(),
            )]),
        );
        let mut manifest_bytes = Vec::new();
        plist::to_writer_binary(&mut manifest_bytes, &plist::Value::Dictionary(manifest)).unwrap();

        let profiles = parse_configuration_profiles(&truth_bytes, Some(&manifest_bytes)).unwrap();
        assert_eq!(profiles.len(), 2);

        let mdm = profiles
            .iter()
            .find(|p| p.display_name == "Work MDM")
            .unwrap();
        assert_eq!(mdm.organization.as_deref(), Some("IT"));
        assert_eq!(mdm.uuid.as_deref(), Some("aaaa-1111"));
        assert!(!mdm.hidden);
        assert!(mdm.capabilities.iter().any(|c| c.contains("MDM")));
        assert!(mdm.hosts.iter().any(|h| h.contains("mdm.evil.example")));

        let printer = profiles
            .iter()
            .find(|p| p.display_name == "Printer")
            .unwrap();
        assert!(printer.hidden);
        assert!(printer.capabilities.is_empty());
    }

    #[test]
    fn parse_configuration_profiles_no_manifest_is_fine() {
        let mut truth = plist::Dictionary::new();
        truth.insert(
            "Solo (cccc-9999)".into(),
            plist::Value::Dictionary(plist::Dictionary::new()),
        );
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &plist::Value::Dictionary(truth)).unwrap();
        let profiles = parse_configuration_profiles(&bytes, None).unwrap();
        assert_eq!(profiles.len(), 1);
        assert!(!profiles[0].hidden);
    }

    #[test]
    fn parse_tcc_reads_granted_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("TCC.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE access (service TEXT, client TEXT, client_type INTEGER,
                auth_value INTEGER, auth_reason INTEGER, auth_version INTEGER, last_modified INTEGER);
             INSERT INTO access VALUES ('kTCCServiceMicrophone','com.evil.tracker',0,2,0,1,100);
             INSERT INTO access VALUES ('kTCCServiceCamera','com.evil.tracker',0,3,0,1,200);
             INSERT INTO access VALUES ('kTCCServiceCamera','com.good.app',0,0,0,1,300);
             INSERT INTO access VALUES ('kTCCServiceLiverpool','com.apple.x',0,2,0,1,400);",
        )
        .unwrap();
        drop(conn);
        let grants = parse_tcc(&path).unwrap();
        // Denied (auth_value 0) row excluded; three granted rows remain.
        assert_eq!(grants.len(), 3);
        let mic = grants.iter().find(|g| g.service == "Microphone").unwrap();
        assert_eq!(mic.client, "com.evil.tracker");
        assert!(mic.sensitive);
        assert_eq!(mic.last_modified, Some(100));
        // A non-surveillance service (Handoff/Liverpool) is not sensitive.
        assert!(grants
            .iter()
            .any(|g| g.service == "Liverpool" && !g.sensitive));
    }

    #[test]
    fn parse_tcc_falls_back_to_legacy_allowed_column() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("TCC.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE access (service TEXT, client TEXT, client_type INTEGER,
                allowed INTEGER, prompt_count INTEGER, last_modified INTEGER);
             INSERT INTO access VALUES ('kTCCServiceCamera','com.old.app',0,1,1,50);
             INSERT INTO access VALUES ('kTCCServiceCamera','com.denied.app',0,0,1,60);",
        )
        .unwrap();
        drop(conn);
        let grants = parse_tcc(&path).unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].client, "com.old.app");
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
            &[],
            &[],
            &[],
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
            &[],
            &[],
            &[],
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
            &[],
            &[],
            &[],
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
            &[],
            &[],
            &[],
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
