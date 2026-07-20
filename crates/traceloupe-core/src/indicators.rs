//! Indicator-of-compromise model and feed loaders for the Security Check.
//!
//! Two feed formats are supported (see docs/spyware-analyzer-prd.md §6.2):
//! - STIX2 JSON bundles (Amnesty investigations, mvt-indicators, iMazing IOC
//!   repo): `malware` objects name the threat, `indicator` objects carry a
//!   pattern string, `relationship` objects tie them together.
//! - Echap stalkerware-indicators YAML (`ioc.yaml` / `watchware.yaml`): one
//!   entry per family with packages, websites, distribution and C2 hosts.
//!
//! Loaders normalize both into one `Indicator` shape. Severity is assigned at
//! load time from indicator kind and feed class; the Echap YAML deliberately
//! distinguishes vendor `websites` (Info — visiting one proves nothing) from
//! `c2` endpoints (Warning — devices talk to those only when infected).

use std::collections::HashMap;
use std::net::IpAddr;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorKind {
    Domain,
    Url,
    Email,
    ProcessName,
    FileName,
    FilePath,
    BundleId,
    CertSha1,
    FileHash,
    Ip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// How a feed's indicators should be graded. Mercenary spyware traces are
/// graded by kind; stalkerware grading favors installed-app evidence; every
/// watchware indicator is informational (the apps do not hide themselves).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedClass {
    Mercenary,
    Stalkerware,
    Watchware,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Indicator {
    pub kind: IndicatorKind,
    /// Normalized value: domains/emails/bundle ids are lowercased and trimmed.
    pub value: String,
    /// Threat attribution ("Pegasus", "TheTruthSpy", …).
    pub malware: String,
    /// Feed labels this indicator came from (deduped union).
    pub sources: Vec<String>,
    pub severity: Severity,
}

/// One parsed feed, before merging.
#[derive(Debug)]
pub struct LoadedFeed {
    pub source: String,
    pub indicators: Vec<Indicator>,
    /// Human-readable notes about skipped material (unknown STIX pattern
    /// types etc.). Never fatal; the caller decides whether to log them.
    pub skipped: Vec<String>,
}

/// Merged, deduplicated indicator collection the scan engine runs against.
#[derive(Debug, Default)]
pub struct IndicatorSet {
    pub indicators: Vec<Indicator>,
}

impl IndicatorSet {
    /// Merge feeds, deduplicating on (kind, value, malware). Duplicates keep
    /// the union of sources and the highest severity (the same domain can be
    /// both a vendor website and a C2 endpoint).
    pub fn from_feeds(feeds: Vec<LoadedFeed>) -> Self {
        let mut by_key: HashMap<(IndicatorKind, String, String), Indicator> = HashMap::new();
        for feed in feeds {
            for ind in feed.indicators {
                let key = (ind.kind, ind.value.clone(), ind.malware.clone());
                match by_key.get_mut(&key) {
                    Some(existing) => {
                        existing.severity = existing.severity.max(ind.severity);
                        for s in ind.sources {
                            if !existing.sources.contains(&s) {
                                existing.sources.push(s);
                            }
                        }
                    }
                    None => {
                        by_key.insert(key, ind);
                    }
                }
            }
        }
        let mut indicators: Vec<Indicator> = by_key.into_values().collect();
        indicators.sort_by(|a, b| (a.kind as u8, &a.value).cmp(&(b.kind as u8, &b.value)));
        IndicatorSet { indicators }
    }

    pub fn len(&self) -> usize {
        self.indicators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indicators.is_empty()
    }

    pub fn count_by_kind(&self, kind: IndicatorKind) -> usize {
        self.indicators.iter().filter(|i| i.kind == kind).count()
    }

    pub fn of_kind(&self, kind: IndicatorKind) -> impl Iterator<Item = &Indicator> {
        self.indicators.iter().filter(move |i| i.kind == kind)
    }
}

fn severity_for(kind: IndicatorKind, class: FeedClass) -> Severity {
    use IndicatorKind::*;
    match class {
        FeedClass::Watchware => Severity::Info,
        FeedClass::Mercenary | FeedClass::Stalkerware => match kind {
            // Presence evidence: these exist on a device only when the
            // threat does (or did).
            ProcessName | FileName | FilePath | BundleId | FileHash | CertSha1 => {
                Severity::Critical
            }
            // Traffic/contact evidence: meaningful but explainable.
            Domain | Url | Email | Ip => Severity::Warning,
        },
    }
}

fn normalize(kind: IndicatorKind, raw: &str) -> String {
    let v = raw.trim();
    match kind {
        IndicatorKind::Domain => v.trim_end_matches('.').to_ascii_lowercase(),
        IndicatorKind::Email | IndicatorKind::BundleId => v.to_ascii_lowercase(),
        IndicatorKind::CertSha1 | IndicatorKind::FileHash => v.to_ascii_uppercase(),
        _ => v.to_string(),
    }
}

// ---------------------------------------------------------------------------
// STIX2 bundles
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StixBundle {
    #[serde(default)]
    objects: Vec<StixObject>,
}

#[derive(Deserialize)]
struct StixObject {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    source_ref: Option<String>,
    #[serde(default)]
    target_ref: Option<String>,
}

/// Map a STIX object path (`domain-name:value`) to our kind. `None` means the
/// path is recognized as unsupported-by-design vs unknown — both are skipped,
/// the distinction only matters for the note text.
fn stix_path_kind(path: &str) -> Option<IndicatorKind> {
    // Hash paths appear both quoted (file:hashes.'SHA-256') and bare
    // (file:hashes.sha256) in the wild.
    let p = path.replace('\'', "").replace('"', "");
    match p.as_str() {
        "domain-name:value" => Some(IndicatorKind::Domain),
        "url:value" => Some(IndicatorKind::Url),
        "email-addr:value" => Some(IndicatorKind::Email),
        "process:name" => Some(IndicatorKind::ProcessName),
        "file:name" => Some(IndicatorKind::FileName),
        "file:path" => Some(IndicatorKind::FilePath),
        "app:id" | "software:name" => Some(IndicatorKind::BundleId),
        "app:cert.sha1" => Some(IndicatorKind::CertSha1),
        "ipv4-addr:value" | "ipv6-addr:value" => Some(IndicatorKind::Ip),
        s if s.starts_with("file:hashes.") => Some(IndicatorKind::FileHash),
        s if s.starts_with("app:hashes.") => Some(IndicatorKind::FileHash),
        _ => None,
    }
}

/// Extract `(path, value)` comparisons from a STIX pattern expression, e.g.
/// `[domain-name:value = 'a.com' OR domain-name:value = 'b.com']`.
/// Anything that is not a simple `path = 'literal'` comparison is reported in
/// the second return value.
fn parse_stix_pattern(pattern: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut pairs = Vec::new();
    let mut unknown = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            // Find the closing quote, honoring backslash escapes.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'\'' {
                    break;
                }
                j += 1;
            }
            if j >= bytes.len() {
                unknown.push(format!("unterminated quote in pattern: {pattern}"));
                break;
            }
            let value = pattern[start..j].replace("\\'", "'").replace("\\\\", "\\");
            // Look back for `path =` immediately before the quote.
            let before = pattern[..i].trim_end();
            if let Some(rest) = before.strip_suffix('=') {
                let path = rest
                    .trim_end()
                    .rsplit(|c: char| {
                        c.is_whitespace() || c == '[' || c == '(' || c == ']' || c == ')'
                    })
                    .next()
                    .unwrap_or("")
                    .to_string();
                if path.is_empty() {
                    unknown.push(format!("no object path before value in: {pattern}"));
                } else {
                    pairs.push((path, value));
                }
            } else {
                // A quoted string not preceded by `=`: part of a hash key like
                // file:hashes.'SHA-256' — stitch it into the following path by
                // skipping; the recomposed path is handled in stix_path_kind
                // via quote stripping only when it precedes '='. Detect the
                // `file:hashes.'SHA-256' = 'value'` form: the NEXT quoted
                // string is the value and `before` ends with the path prefix.
                let after = &pattern[j + 1..];
                if after.trim_start().starts_with('=') {
                    // This quoted segment is a path component, not a value.
                    // Rewind: treat path as prefix + quoted part.
                    let prefix = before
                        .rsplit(|c: char| c.is_whitespace() || c == '[' || c == '(')
                        .next()
                        .unwrap_or("");
                    let full_path = format!("{prefix}'{}'", &pattern[start..j]);
                    // Find the value after '='.
                    let eq_rel = after.find('=').unwrap();
                    let rest = &after[eq_rel + 1..];
                    if let Some(vstart_rel) = rest.find('\'') {
                        let vstart = j + 1 + eq_rel + 1 + vstart_rel + 1;
                        let mut k = vstart;
                        while k < bytes.len() {
                            if bytes[k] == b'\\' {
                                k += 2;
                                continue;
                            }
                            if bytes[k] == b'\'' {
                                break;
                            }
                            k += 1;
                        }
                        if k < bytes.len() {
                            pairs.push((full_path, pattern[vstart..k].to_string()));
                            i = k + 1;
                            continue;
                        }
                    }
                    unknown.push(format!("unparsed hash comparison in: {pattern}"));
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    (pairs, unknown)
}

/// Load a STIX2 bundle. `source` is a short feed label recorded on every
/// indicator (e.g. "amnesty/pegasus").
pub fn load_stix_bundle(json_text: &str, source: &str, class: FeedClass) -> Result<LoadedFeed> {
    let bundle: StixBundle =
        serde_json::from_str(json_text).map_err(|e| Error::IndicatorFeed {
            feed: source.to_string(),
            message: e.to_string(),
        })?;

    // malware id → name
    let mut malware_names: HashMap<&str, &str> = HashMap::new();
    for o in &bundle.objects {
        if o.kind == "malware" {
            if let Some(name) = &o.name {
                malware_names.insert(o.id.as_str(), name.as_str());
            }
        }
    }
    // indicator id → malware name (via "indicates" relationships)
    let mut attributed: HashMap<&str, &str> = HashMap::new();
    for o in &bundle.objects {
        if o.kind == "relationship" {
            if let (Some(src), Some(dst)) = (&o.source_ref, &o.target_ref) {
                if let Some(name) = malware_names.get(dst.as_str()) {
                    attributed.insert(src.as_str(), name);
                }
            }
        }
    }
    let single_malware = (malware_names.len() == 1)
        .then(|| malware_names.values().next().copied().unwrap_or_default());

    let mut indicators = Vec::new();
    let mut skipped = Vec::new();
    for o in &bundle.objects {
        if o.kind != "indicator" {
            continue;
        }
        let Some(pattern) = &o.pattern else {
            skipped.push(format!("indicator {} has no pattern", o.id));
            continue;
        };
        let malware = attributed
            .get(o.id.as_str())
            .copied()
            .or(single_malware)
            .unwrap_or("Unknown")
            .to_string();
        let (pairs, mut unknown) = parse_stix_pattern(pattern);
        skipped.append(&mut unknown);
        if pairs.is_empty() {
            continue;
        }
        for (path, value) in pairs {
            match stix_path_kind(&path) {
                Some(kind) => indicators.push(Indicator {
                    kind,
                    value: normalize(kind, &value),
                    malware: malware.clone(),
                    sources: vec![source.to_string()],
                    severity: severity_for(kind, class),
                }),
                None => skipped.push(format!("unsupported STIX object path: {path}")),
            }
        }
    }
    Ok(LoadedFeed {
        source: source.to_string(),
        indicators,
        skipped,
    })
}

// ---------------------------------------------------------------------------
// Echap stalkerware-indicators YAML
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EchapC2 {
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    ips: Vec<String>,
}

#[derive(Deserialize)]
struct EchapEntry {
    name: String,
    #[serde(rename = "type", default)]
    _kind: Option<String>,
    #[serde(default)]
    packages: Vec<String>,
    #[serde(default)]
    certificates: Vec<String>,
    #[serde(default)]
    websites: Vec<String>,
    #[serde(default)]
    distribution: Vec<String>,
    #[serde(default)]
    c2: Option<EchapC2>,
}

fn host_kind(value: &str) -> IndicatorKind {
    if value.parse::<IpAddr>().is_ok() {
        IndicatorKind::Ip
    } else {
        IndicatorKind::Domain
    }
}

/// Load Echap `ioc.yaml` (class Stalkerware) or `watchware.yaml` (class
/// Watchware). Severity encodes evidence strength: an installed package is
/// Critical, a C2 contact Warning, and vendor websites / distribution hosts /
/// signing certificates only Info — visiting a vendor site proves nothing.
pub fn load_echap_yaml(yaml_text: &str, source: &str, class: FeedClass) -> Result<LoadedFeed> {
    let entries: Vec<EchapEntry> =
        serde_yaml::from_str(yaml_text).map_err(|e| Error::IndicatorFeed {
            feed: source.to_string(),
            message: e.to_string(),
        })?;

    let mut indicators = Vec::new();
    let mut push = |kind: IndicatorKind, value: &str, malware: &str, severity: Severity| {
        let value = normalize(kind, value);
        if value.is_empty() {
            return;
        }
        indicators.push(Indicator {
            kind,
            value,
            malware: malware.to_string(),
            sources: vec![source.to_string()],
            severity,
        });
    };

    let info = Severity::Info;
    for e in &entries {
        let critical = severity_for(IndicatorKind::BundleId, class);
        let warning = severity_for(IndicatorKind::Domain, class);
        for p in &e.packages {
            push(IndicatorKind::BundleId, p, &e.name, critical);
        }
        for c in &e.certificates {
            push(IndicatorKind::CertSha1, c, &e.name, info);
        }
        for w in &e.websites {
            push(host_kind(w), w, &e.name, info);
        }
        for d in &e.distribution {
            push(host_kind(d), d, &e.name, info);
        }
        if let Some(c2) = &e.c2 {
            for d in &c2.domains {
                push(host_kind(d), d, &e.name, warning);
            }
            for ip in &c2.ips {
                push(IndicatorKind::Ip, ip, &e.name, warning);
            }
        }
    }
    Ok(LoadedFeed {
        source: source.to_string(),
        indicators,
        skipped: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Snapshot directory (bundled or fetched)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SnapshotManifest {
    generated_at: String,
    feeds: Vec<SnapshotFeedEntry>,
}

#[derive(Deserialize)]
struct SnapshotFeedEntry {
    file: String,
    source: String,
    class: String,
    format: String,
}

/// Per-feed summary reported to the UI (feed freshness screen, scan footer).
#[derive(Debug, serde::Serialize)]
pub struct FeedInfo {
    pub source: String,
    pub class: String,
    pub count: usize,
    pub skipped: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct SnapshotInfo {
    pub generated_at: String,
    pub feeds: Vec<FeedInfo>,
}

/// Load every feed listed in a snapshot directory's `manifest.json` (written
/// by scripts/update-indicator-snapshot.sh — the bundled resources dir or the
/// fetched copy under Application Support). A feed that fails to parse is
/// reported with count 0 rather than failing the whole snapshot; a missing or
/// malformed manifest is an error.
pub fn load_snapshot_dir(dir: &std::path::Path) -> Result<(IndicatorSet, SnapshotInfo)> {
    let manifest_path = dir.join("manifest.json");
    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|e| Error::io(&manifest_path, e))?;
    let manifest: SnapshotManifest =
        serde_json::from_str(&manifest_text).map_err(|e| Error::IndicatorFeed {
            feed: "manifest.json".to_string(),
            message: e.to_string(),
        })?;

    let mut feeds = Vec::new();
    let mut infos = Vec::new();
    for entry in &manifest.feeds {
        let class = match entry.class.as_str() {
            "stalkerware" => FeedClass::Stalkerware,
            "watchware" => FeedClass::Watchware,
            _ => FeedClass::Mercenary,
        };
        let path = dir.join(&entry.file);
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| Error::io(&path, e))
            .and_then(|text| match entry.format.as_str() {
                "echap_yaml" => load_echap_yaml(&text, &entry.source, class),
                _ => load_stix_bundle(&text, &entry.source, class),
            });
        match loaded {
            Ok(feed) => {
                infos.push(FeedInfo {
                    source: entry.source.clone(),
                    class: entry.class.clone(),
                    count: feed.indicators.len(),
                    skipped: feed.skipped.len(),
                });
                feeds.push(feed);
            }
            Err(e) => infos.push(FeedInfo {
                source: format!("{} (failed: {e})", entry.source),
                class: entry.class.clone(),
                count: 0,
                skipped: 0,
            }),
        }
    }
    Ok((
        IndicatorSet::from_feeds(feeds),
        SnapshotInfo {
            generated_at: manifest.generated_at,
            feeds: infos,
        },
    ))
}

/// The snapshot directory vendored into the repo (and bundled as an app
/// resource). Callers with a Tauri resource dir should prefer that path;
/// this constant serves tests and dev builds running from the workspace.
pub fn bundled_snapshot_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/indicators")
}

/// A feed's canonical download URL, carried in the bundled manifest.
#[derive(Deserialize)]
struct FetchManifest {
    feeds: Vec<FetchFeedEntry>,
}

#[derive(Deserialize)]
struct FetchFeedEntry {
    file: String,
    #[serde(default)]
    url: Option<String>,
}

/// Refresh the indicator snapshot at `dest` from the feed URLs recorded in the
/// bundled snapshot's `manifest.json`. Downloads to a temp file per feed and
/// swaps into place only on success, so a failed/partial fetch leaves the
/// previous snapshot intact. Copies the manifest and attribution over so
/// `dest` is a self-contained loadable snapshot.
///
/// Privacy (ADR 0001): every request URL is a static public-repo feed path
/// from the bundled manifest. No backup-derived data is sent. `agent` is
/// injected for testability.
pub fn fetch_snapshot_with(
    agent: &ureq::Agent,
    bundled_dir: &std::path::Path,
    dest: &std::path::Path,
    mut on_progress: impl FnMut(&str, usize, usize),
) -> Result<SnapshotInfo> {
    let manifest_path = bundled_dir.join("manifest.json");
    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|e| Error::io(&manifest_path, e))?;
    let manifest: FetchManifest =
        serde_json::from_str(&manifest_text).map_err(|e| Error::IndicatorFeed {
            feed: "manifest.json".to_string(),
            message: e.to_string(),
        })?;

    std::fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    // Each indicator feed is a few MB at most; cap to guard a MITM'd host from
    // filling the disk (verification/parse happens after the whole file lands).
    const MAX_FEED_BYTES: u64 = 64 * 1024 * 1024;

    let total = manifest.feeds.len();
    for (idx, feed) in manifest.feeds.iter().enumerate() {
        let Some(url) = &feed.url else { continue };
        on_progress(&feed.file, idx, total);
        let resp = agent
            .get(url)
            .call()
            .map_err(|e| Error::IndicatorFeed {
                feed: feed.file.clone(),
                message: format!("request failed: {e}"),
            })?;
        let tmp = dest.join(format!("{}.download", feed.file));
        let mut reader = std::io::Read::take(resp.into_reader(), MAX_FEED_BYTES + 1);
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut buf).map_err(|e| Error::IndicatorFeed {
            feed: feed.file.clone(),
            message: e.to_string(),
        })?;
        if buf.len() as u64 > MAX_FEED_BYTES {
            return Err(Error::IndicatorFeed {
                feed: feed.file.clone(),
                message: "feed exceeded size cap".into(),
            });
        }
        std::fs::write(&tmp, &buf).map_err(|e| Error::io(&tmp, e))?;
        let final_path = dest.join(&feed.file);
        std::fs::rename(&tmp, &final_path).map_err(|e| Error::io(&final_path, e))?;
    }

    // Refresh manifest + attribution so `dest` loads standalone. Stamp the
    // manifest's generated_at with the fetch time by rewriting the field.
    let mut manifest_value: serde_json::Value =
        serde_json::from_str(&manifest_text).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = manifest_value.as_object_mut() {
        obj.insert(
            "generated_at".into(),
            serde_json::Value::String(fetch_timestamp()),
        );
    }
    std::fs::write(
        dest.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_value).unwrap_or(manifest_text),
    )
    .map_err(|e| Error::io(dest.join("manifest.json"), e))?;
    let attribution = bundled_dir.join("ATTRIBUTION.md");
    if attribution.exists() {
        let _ = std::fs::copy(&attribution, dest.join("ATTRIBUTION.md"));
    }

    let (_, info) = load_snapshot_dir(dest)?;
    Ok(info)
}

fn fetch_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match time::OffsetDateTime::from_unix_timestamp(secs as i64) {
        Ok(dt) => dt
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| secs.to_string()),
        Err(_) => secs.to_string(),
    }
}

/// Convenience wrapper building a default agent.
pub fn fetch_snapshot(
    bundled_dir: &std::path::Path,
    dest: &std::path::Path,
    on_progress: impl FnMut(&str, usize, usize),
) -> Result<SnapshotInfo> {
    let agent = ureq::AgentBuilder::new().build();
    fetch_snapshot_with(&agent, bundled_dir, dest, on_progress)
}

/// Resolve the active snapshot directory: a previously fetched one under
/// `app_data` if present and non-empty, else the bundled directory.
pub fn active_snapshot_dir(
    app_data: &std::path::Path,
    bundled_dir: &std::path::Path,
) -> std::path::PathBuf {
    let fetched = app_data.join("indicators");
    if fetched.join("manifest.json").exists() {
        fetched
    } else {
        bundled_dir.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_single_comparison() {
        let (pairs, unknown) = parse_stix_pattern("[domain-name:value = 'Example.COM.']");
        assert_eq!(pairs, vec![("domain-name:value".into(), "Example.COM.".into())]);
        assert!(unknown.is_empty());
    }

    #[test]
    fn pattern_or_list() {
        let (pairs, _) =
            parse_stix_pattern("[url:value = 'https://a/x' OR url:value = 'https://b/y']");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[1].1, "https://b/y");
    }

    #[test]
    fn pattern_quoted_hash_key() {
        let (pairs, unknown) =
            parse_stix_pattern("[file:hashes.'SHA-256' = 'abc123']");
        assert_eq!(pairs.len(), 1, "unknown: {unknown:?}");
        assert_eq!(pairs[0].0, "file:hashes.'SHA-256'");
        assert_eq!(pairs[0].1, "abc123");
        assert_eq!(stix_path_kind(&pairs[0].0), Some(IndicatorKind::FileHash));
    }

    #[test]
    fn pattern_escaped_quote() {
        let (pairs, _) = parse_stix_pattern(r"[file:name = 'it\'s.dylib']");
        assert_eq!(pairs[0].1, "it's.dylib");
    }

    #[test]
    fn domain_normalization() {
        assert_eq!(normalize(IndicatorKind::Domain, " Ex.COM. "), "ex.com");
    }

    #[test]
    fn dedupe_keeps_max_severity_and_all_sources() {
        let a = LoadedFeed {
            source: "a".into(),
            indicators: vec![Indicator {
                kind: IndicatorKind::Domain,
                value: "x.com".into(),
                malware: "M".into(),
                sources: vec!["a".into()],
                severity: Severity::Info,
            }],
            skipped: vec![],
        };
        let b = LoadedFeed {
            source: "b".into(),
            indicators: vec![Indicator {
                kind: IndicatorKind::Domain,
                value: "x.com".into(),
                malware: "M".into(),
                sources: vec!["b".into()],
                severity: Severity::Warning,
            }],
            skipped: vec![],
        };
        let set = IndicatorSet::from_feeds(vec![a, b]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.indicators[0].severity, Severity::Warning);
        assert_eq!(set.indicators[0].sources, vec!["a".to_string(), "b".to_string()]);
    }
}
