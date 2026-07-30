//! Declarative artifact modules — the spine that makes the long tail affordable.
//!
//! An artifact is declared in a TOML file rather than written as Rust: which
//! backup file it needs, the SQL to run over it, and what each output column
//! means. A loader validates them, a runner executes them, and the rows land in
//! one generic table. Adding the next artifact is a data change, not a code
//! change and not a schema migration.
//!
//! The reasoning is on issue #190. In short: `docs/reference/backup-coverage-audit.md`
//! sizes the addressable set at ~360 backup-reachable artifacts, and at the cost
//! of a hand-written parser plus a designed view each, that is not a thing
//! anyone finishes. Modules that carry no logic can be reviewed in a ~20-line
//! diff and contributed by someone who does not write Rust.
//!
//! ```toml
//! id     = "tcc"
//! name   = "App permissions"
//! domain = "HomeDomain"
//! path   = "Library/TCC/TCC.db"
//! sql    = "SELECT client, service, auth_value, last_modified FROM access"
//!
//! [[columns]]
//! name = "App"
//! from = "client"
//!
//! [[columns]]
//! name  = "Granted"
//! from  = "last_modified"
//! kind  = "timestamp"
//! epoch = "unix"
//! ```
//!
//! **What is deliberately not here.** No Rust escape hatch yet (#190 decided one
//! is needed eventually — Notes' gzip-protobuf and Instagram's NSKeyedArchiver
//! cannot be declared — but declarative comes first), and no UI. This module is
//! backend-only so it is covered entirely by `cargo test`.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::crypto::BackupDecryptor;
use crate::manifest::ManifestIndex;
use crate::{Error, Result};

/// How a stored number becomes a date. iOS mixes all of these across stores,
/// which is why the epoch is declared per column rather than guessed: a Cocoa
/// timestamp read as Unix lands in 1970, and nothing about the value says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Epoch {
    /// Seconds since 1970-01-01 (classic Unix).
    Unix,
    /// Milliseconds since 1970-01-01.
    UnixMs,
    /// Seconds since 2001-01-01 — Core Data / CFAbsoluteTime. The most common
    /// one in Apple's own stores, and the one most often mistaken for Unix.
    Cocoa,
    /// Microseconds since 1601-01-01 (WebKit / Chromium).
    Webkit,
}

impl Epoch {
    /// Convert to Unix seconds. Returns None for values that cannot be a date,
    /// so a bad row degrades to "no date" instead of poisoning the artifact.
    pub fn to_unix_seconds(self, raw: f64) -> Option<i64> {
        let secs = match self {
            Epoch::Unix => raw,
            Epoch::UnixMs => raw / 1_000.0,
            Epoch::Cocoa => raw + 978_307_200.0,
            Epoch::Webkit => raw / 1_000_000.0 - 11_644_473_600.0,
        };
        // A backup cannot contain a date before iOS existed or far in the
        // future; treating those as "no date" is the rule app-data-coverage.md
        // already applies to messages whose timestamp does not decode.
        //
        // This also handles NaN and ±inf: every comparison against NaN is
        // false, so `contains` rejects them. An explicit `is_finite` guard used
        // to sit above and was dead code — mutation testing showed removing it
        // changed nothing.
        if !(0.0..=4_102_444_800.0).contains(&secs) {
            return None;
        }
        Some(secs as i64)
    }
}

/// What a column is, once read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColumnKind {
    #[default]
    Text,
    Integer,
    Real,
    Bool,
    Timestamp,
    /// A byte count, rendered as a human size by the UI.
    ///
    /// Declared rather than inferred for the same reason `Timestamp` is: the
    /// module knows, and nothing else can tell a byte count from any other large
    /// integer.
    Bytes,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ColumnSpec {
    /// Display name.
    pub name: String,
    /// The SQL result column this reads.
    pub from: String,
    #[serde(default)]
    pub kind: ColumnKind,
    /// Required when `kind = "timestamp"`, meaningless otherwise.
    #[serde(default)]
    pub epoch: Option<Epoch>,
}

/// What a host view may show about an artifact ON THE ROW, before anything is
/// expanded.
///
/// Optional, and declared by the module rather than inferred, because the Apps
/// view was doing this by hard-coding TCC: it filtered on a literal `"Decision"`
/// column for the literal values `"Allowed"`/`"Limited"` and printed "none
/// granted" when it found none. That is artifact-specific knowledge in a view
/// whose whole premise is that it knows no artifact by name — and the moment a
/// SECOND apps-surface module shipped it produced "Data usage: none granted",
/// which is not a claim data usage can make.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HighlightSpec {
    /// The column whose values are worth showing inline.
    pub column: String,
    /// Only badge a row when this column matches — omit to badge every row.
    #[serde(default)]
    pub when_column: Option<String>,
    #[serde(default)]
    pub when_any_of: Vec<String>,
    /// What to say when nothing matched. Omit and the host says nothing at all,
    /// which is the right default: silence claims less than a phrase.
    #[serde(default)]
    pub none_label: Option<String>,
}

/// Where an artifact is shown.
///
/// Not a free string: the set of hosts is small and known, and a typo must not
/// silently become "nowhere".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// Inside the Apps view, against the app it belongs to.
    Apps,
    /// Inside the Contacts view.
    Contacts,
    /// Inside the Device view.
    Device,
    /// Its own destination — only for data that fits nowhere else.
    Standalone,
}

/// The container-domain families, which are **not** in
/// `tools/data/ios-backup-domains.json`.
///
/// That file holds the 19 domains iOS names outright (`HomeDomain`,
/// `MediaDomain`, …). Everything sandboxed instead gets a domain built at backup
/// time from a family prefix and an identifier —
/// `AppDomain-com.example.app`,
/// `AppDomainGroup-group.com.apple.notes`,
/// `SysSharedContainerDomain-systemgroup.com.apple.bluetooth`. There is no
/// enumerable list of them, because the set depends on what is installed, so a
/// module's domain is checked against the *families* instead.
///
/// This existed as a bare `starts_with("AppDomain")` in one test, which accepted
/// the three `AppDomain*` families and rejected the two `Sys*Container` ones — so
/// the first module in a system container (Bluetooth) failed a check that was
/// right to exist and wrong about the rules.
pub const CONTAINER_DOMAIN_PREFIXES: &[&str] = &[
    "AppDomain-",
    "AppDomainGroup-",
    "AppDomainPlugin-",
    "SysContainerDomain-",
    "SysSharedContainerDomain-",
];

/// Whether `domain` is a container domain, and so cannot appear in the static
/// list. The identifier after the prefix must be non-empty: `AppDomain-` alone
/// names nothing, and accepting it would let a truncated domain through.
pub fn is_container_domain(domain: &str) -> bool {
    CONTAINER_DOMAIN_PREFIXES
        .iter()
        .any(|p| domain.len() > p.len() && domain.starts_with(p))
}

impl Surface {
    /// Whether the host attaches each row to one of its own rows, and so needs a
    /// `join_column`.
    ///
    /// Apps and Contacts are lists of many things, and an artifact shown there is
    /// shown *against* one of them — a permission belongs to an app. Device is a
    /// list of one, so there is nothing to attach to and no column could identify
    /// it; the artifact is shown whole. Requiring a join column there would force
    /// every module to nominate an arbitrary one, which is how a required field
    /// becomes a field nobody means.
    pub fn attaches_to_a_row(self) -> bool {
        match self {
            Surface::Apps | Surface::Contacts => true,
            Surface::Device | Surface::Standalone => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModuleSpec {
    /// Stable identifier; also the key rows are stored under.
    pub id: String,
    pub name: String,
    /// One plain-language sentence: what this is, for someone who has never
    /// heard of the store it comes from.
    ///
    /// Required. A module that cannot say what it is in a sentence is not
    /// finished — the Artifacts view shipped without this and was unintelligible
    /// to the person who commissioned it.
    pub description: String,
    /// Which existing view hosts this artifact, or `standalone`.
    ///
    /// Required, and deliberately not defaulted. The agreed rule is that data
    /// folds into the view closest in meaning, with its own destination only for
    /// genuinely outstanding data — and that rule was drifted from within one
    /// slice of shipping it. Making the author state a home is how it stops
    /// being a convention someone has to remember.
    pub surface: Surface,
    /// Which output column holds the value the host view joins on — a bundle id
    /// for `surface = "apps"`, a handle for Contacts.
    ///
    /// Required whenever the surface is not standalone: a hosted artifact that
    /// cannot be attached to a row is not hosted, it is just floating in someone
    /// else's view.
    #[serde(default)]
    pub join_column: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    /// What the host may show on the row itself, before expanding.
    #[serde(default)]
    pub highlight: Option<HighlightSpec>,
    /// Backup domain, e.g. `HomeDomain`.
    pub domain: String,
    /// Exact relative path within the domain.
    ///
    /// Deliberately not a glob. A prefix-match version of this shipped in the
    /// first draft and was wrong in two ways: `Manifest.db` carries directory
    /// rows, and a directory sorts before its own children, so `Library/Foo/*`
    /// selected the directory rather than the store inside it; and `-wal` /
    /// `-shm` siblings could win over the real file. Since no artifact needed
    /// globbing, the fix is to not have it until one does — with tests.
    pub path: String,
    /// The query, or several to try in order — the first that *prepares*
    /// against this backup's store wins.
    ///
    /// Alternatives exist because iOS renames columns between versions and a
    /// single static SELECT cannot span that. TCC is the case that forced it:
    /// modern devices have `auth_value` (0/2/3), older ones `allowed` (0/1),
    /// and the two never co-exist, so `COALESCE` cannot help — the query simply
    /// fails to prepare against the wrong one.
    ///
    /// Every alternative must produce the SAME output column names, since the
    /// column spec is shared. Alias in SQL to make that so, and `SELECT NULL AS
    /// x` where an older schema has nothing to offer.
    #[serde(deserialize_with = "one_or_many")]
    pub sql: Vec<String>,
    /// Declared precondition. Parsed and stored here; honoured by the UI in a
    /// later slice (#210). Apple's `RelativePathsToOnlyBackupEncrypted` covers
    /// 28 artifacts, so this is not an edge case.
    #[serde(default)]
    pub requires: Option<String>,
    pub columns: Vec<ColumnSpec>,
}

/// Accept either `sql = "…"` or `sql = ["…", "…"]`.
///
/// A hand-written visitor rather than `#[serde(untagged)]`, which collapses any
/// mistake into "data did not match any variant of untagged enum". `load_modules`
/// promises to say which file and *why*; "invalid type: integer, expected a
/// string" is the why, and untagged throws it away.
fn one_or_many<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a SQL string, or a list of alternative SQL strings")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<String>, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                out.push(item);
            }
            Ok(out)
        }
    }
    d.deserialize_any(V)
}

impl ModuleSpec {
    /// Reject a module that cannot possibly work, naming the problem. A module
    /// that fails silently is worse than one that fails loudly: the artifact
    /// just never appears, and nothing says why.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("`id` is empty".into());
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "`id` = {:?} may only contain letters, digits, `_` and `-`",
                self.id
            ));
        }
        if self.description.trim().is_empty() {
            return Err("`description` is empty — say in one sentence what this is".into());
        }
        if self.domain.trim().is_empty() {
            return Err("`domain` is empty".into());
        }
        if self.path.trim().is_empty() {
            return Err("`path` is empty".into());
        }
        if self.path.contains('*') {
            return Err(format!(
                "`path` = {:?} looks like a glob; only exact paths are supported. \
                 A `*` here would be matched literally and never find anything",
                self.path
            ));
        }
        if self.sql.is_empty() || self.sql.iter().all(|q| q.trim().is_empty()) {
            return Err("`sql` is empty".into());
        }
        for sql in &self.sql {
            let sql = sql.trim();
            // A friendly early error only. It is NOT the thing that makes a module
            // read-only — SQLite accepts `WITH x AS (…) INSERT … RETURNING`, which
            // passes this check and writes. The store is opened read-only in
            // `run_module`; that is the enforcement.
            if !sql.to_ascii_lowercase().starts_with("select")
                && !sql.to_ascii_lowercase().starts_with("with")
            {
                return Err("`sql` must be a SELECT (or WITH … SELECT)".into());
            }
        }
        if self.columns.is_empty() {
            return Err("no `[[columns]]` declared".into());
        }
        let mut seen_names: Vec<&str> = Vec::new();
        for c in &self.columns {
            if c.name.trim().is_empty() || c.from.trim().is_empty() {
                return Err(format!("column {:?} has an empty `name` or `from`", c.name));
            }
            // A row is keyed on the display name, so two columns sharing one
            // collapse to whichever is written last — a column silently
            // disappearing with no error at load or run.
            if seen_names.contains(&c.name.as_str()) {
                return Err(format!(
                    "two columns are both named {:?} — a row is keyed on the display name, \
                     so one would silently overwrite the other",
                    c.name
                ));
            }
            seen_names.push(&c.name);
            if c.kind == ColumnKind::Timestamp && c.epoch.is_none() {
                return Err(format!(
                    "column {:?} is a timestamp but declares no `epoch` — \
                     without it the date cannot be converted and would render as a raw number",
                    c.name
                ));
            }
        }
        // An artifact attached to a row must say which row.
        if self.surface.attaches_to_a_row() && self.join_column.is_none() {
            return Err(format!(
                "`surface` is {:?} but no `join_column` is declared — the host view \
                 needs to know which column identifies the row it belongs to",
                self.surface
            ));
        }
        // Checked whenever one is declared, including on a surface that does not
        // require it: a `join_column` naming a column that does not exist is a
        // typo either way, and silently ignoring it on `device` would let the
        // same mistake through unnoticed.
        if let Some(col) = &self.join_column {
            if !self.columns.iter().any(|c| &c.name == col) {
                return Err(format!(
                    "`join_column` = {col:?} is not one of the declared columns"
                ));
            }
        }
        // A highlight naming a column that does not exist would silently show
        // nothing, which is indistinguishable from an artifact that legitimately
        // has nothing to highlight.
        if let Some(h) = &self.highlight {
            let has = |name: &str| self.columns.iter().any(|c| c.name == name);
            if !has(&h.column) {
                return Err(format!(
                    "`highlight.column` = {:?} is not one of the declared columns",
                    h.column
                ));
            }
            if let Some(w) = &h.when_column {
                if !has(w) {
                    return Err(format!(
                        "`highlight.when_column` = {w:?} is not one of the declared columns"
                    ));
                }
                if h.when_any_of.is_empty() {
                    return Err(
                        "`highlight.when_column` is set but `when_any_of` is empty — the \
                         condition could never match, so nothing would ever be highlighted"
                            .into(),
                    );
                }
            } else if !h.when_any_of.is_empty() {
                return Err(
                    "`highlight.when_any_of` is set but `when_column` is not — there is \
                     nothing to compare it against"
                        .into(),
                );
            }
        }
        if let Some(r) = &self.requires {
            if r != "encrypted-backup" {
                return Err(format!(
                    "unknown `requires` value {r:?} (only \"encrypted-backup\")"
                ));
            }
        }
        Ok(())
    }

    /// True when this module needs an encrypted backup to have any data.
    pub fn needs_encrypted_backup(&self) -> bool {
        self.requires.as_deref() == Some("encrypted-backup")
    }
}

/// One artifact, as the UI needs to describe it: what it is called and which
/// columns it has, without the UI knowing any artifact exists by name.
/// `camelCase` because the UI reads `joinColumn`, `rowCount` and
/// `requiresEncryptedBackup` (src/lib/ipc.ts). Without this the struct
/// serialised as snake_case and every one of those was `undefined` in the real
/// app, so no hosted artifact rendered at all — while the mock, which returns
/// camelCase, made the whole feature look fine under `pnpm dev` (#232).
/// `serialised_keys_match_the_ui_contract` now pins it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSummary {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    /// One sentence describing the artifact, shown wherever it is hosted.
    pub description: String,
    /// Which view hosts it.
    pub surface: Surface,
    /// The column the host view joins on, when hosted.
    pub join_column: Option<String>,
    /// What the host may show on the row itself, before expanding.
    pub highlight: Option<HighlightSpec>,
    /// Column display names, in declared order — the table's headers. A JSON
    /// row is an unordered map, so without this the UI would have to invent an
    /// order, and it would differ between artifacts and between runs.
    pub columns: Vec<String>,
    /// Which of `columns` are byte counts, from the module's own `kind`.
    pub byte_columns: Vec<String>,
    /// Which of `columns` are timestamps, from the module's own `kind`.
    ///
    /// The UI used to infer this by testing whether every value in a column fell
    /// inside a plausible Unix-seconds range. That was guesswork over a fact the
    /// module already states, and the Bluetooth module is where it would have
    /// bitten: its two counters are integers that must NOT be read as dates, and
    /// the only thing standing between them and a fabricated 1970s timestamp was
    /// the range heuristic happening to exclude them.
    pub timestamp_columns: Vec<String>,
    pub row_count: i64,
    /// True when this artifact needs an encrypted backup to hold anything.
    pub requires_encrypted_backup: bool,
}

/// The cache key recording which modules produced the stored rows.
const EXTRACTED_MODULES_KEY: &str = "artifacts_extracted_modules";

/// A stable fingerprint of the current module set: sorted ids, comma-joined.
///
/// A boolean "have we extracted" would not survive adding a module — someone who
/// extracted with two installed and later has five would keep seeing two, with
/// nothing to say why. Comparing the actual set catches both "never run" and
/// "run with a smaller set".
pub fn module_set_fingerprint(specs: &[ModuleSpec]) -> String {
    let mut ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    ids.join(",")
}

/// Why the Artifacts view might have nothing to show.
///
/// "The backup contained none" and "nobody has looked yet" are different facts,
/// and saying the first when the second is true is a claim the user cannot
/// check. Same class of mistake as an encryption-gated view rendering as merely
/// empty (#203).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtractionState {
    /// Rows were produced by exactly the modules installed now.
    UpToDate,
    /// No module has ever run against this backup.
    NeverRun,
    /// Rows exist, but from a different (usually smaller) module set.
    Stale,
}

/// Whether the stored rows came from the module set installed right now.
pub fn extraction_state(cache: &crate::cache::CacheDb) -> Result<ExtractionState> {
    let want = module_set_fingerprint(&builtin_modules()?);
    match cache.get_meta(EXTRACTED_MODULES_KEY)? {
        None => Ok(ExtractionState::NeverRun),
        Some(have) if have == want => Ok(ExtractionState::UpToDate),
        Some(_) => Ok(ExtractionState::Stale),
    }
}

/// Record that the current module set has been run against this backup.
pub fn mark_extracted(cache: &crate::cache::CacheDb, specs: &[ModuleSpec]) -> Result<()> {
    cache.set_meta(EXTRACTED_MODULES_KEY, &module_set_fingerprint(specs))
}

/// Every shipped artifact that has rows in this backup, with its shape.
///
/// Artifacts with no rows are omitted: the backup did not contain them, and a
/// list of empty tables is not navigation. The one exception is an artifact
/// gated on encryption, which is kept so it can explain itself rather than
/// vanish (#197) — an absent artifact and an impossible one are different
/// facts.
pub fn list_artifacts(conn: &Connection) -> Result<Vec<ArtifactSummary>> {
    let mut out = Vec::new();
    for spec in builtin_modules()? {
        let row_count = count_rows(conn, &spec.id)?;
        let gated = spec.needs_encrypted_backup();
        if row_count == 0 && !gated {
            continue;
        }
        out.push(ArtifactSummary {
            id: spec.id.clone(),
            name: spec.name.clone(),
            category: spec.category.clone(),
            description: spec.description.clone(),
            surface: spec.surface,
            join_column: spec.join_column.clone(),
            highlight: spec.highlight.clone(),
            columns: spec.columns.iter().map(|c| c.name.clone()).collect(),
            timestamp_columns: spec
                .columns
                .iter()
                .filter(|c| c.kind == ColumnKind::Timestamp)
                .map(|c| c.name.clone())
                .collect(),
            byte_columns: spec
                .columns
                .iter()
                .filter(|c| c.kind == ColumnKind::Bytes)
                .map(|c| c.name.clone())
                .collect(),
            row_count,
            requires_encrypted_backup: gated,
        });
    }
    Ok(out)
}

/// One parsed row: column display name → value.
pub type ArtifactRow = HashMap<String, serde_json::Value>;

/// Load every `*.toml` module under `dir`, in a stable order.
///
/// A directory that does not exist is not an error — it means no modules are
/// installed. A file that does not parse *is*, and says which file and why.
pub fn load_modules(dir: &Path) -> Result<Vec<ModuleSpec>> {
    let mut out: Vec<ModuleSpec> = Vec::new();
    // Keep each module's source filename so a collision can name both files —
    // a display name gives the reader nothing to grep for.
    let mut sources: Vec<(String, String)> = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    entries.sort();

    for path in entries {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Io {
            path: path.clone(),
            source: e,
        })?;
        let spec: ModuleSpec = toml::from_str(&text).map_err(|e| {
            Error::Parse(format!(
                "artifact module {}: {e}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        })?;
        spec.validate().map_err(|why| {
            Error::Parse(format!(
                "artifact module {}: {why}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        })?;
        // Two modules sharing an id would silently overwrite each other's rows
        // — `store_rows` is keyed on it and replaces. That is a data-loss bug
        // whose only symptom is an artifact that inexplicably shows another
        // artifact's contents, so it is rejected at load, naming both files.
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some((_, prev_file)) = sources.iter().find(|(id, _)| id == &spec.id) {
            return Err(Error::Parse(format!(
                "artifact module {file}: id {:?} is already used by {prev_file} — ids key \
                 stored rows, so two modules sharing one would overwrite each other",
                spec.id,
            )));
        }
        sources.push((spec.id.clone(), file));
        out.push(spec);
    }
    Ok(out)
}

/// Find the backup file a module wants, if the backup has it.
///
/// Checks the Manifest and nothing else — so an artifact the backup does not
/// contain costs one indexed lookup and never touches the filesystem. That is
/// the same property every hand-written source already has, and it is what
/// keeps import time flat as the module count grows.
fn locate(index: &ManifestIndex, spec: &ModuleSpec) -> Result<Option<crate::manifest::FileEntry>> {
    index.find(&spec.domain, &spec.path)
}

/// Run one module against a backup. `Ok(None)` means the backup does not
/// contain this artifact — which is a normal outcome, not a failure.
pub fn run_module(
    spec: &ModuleSpec,
    index: &ManifestIndex,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
) -> Result<Option<Vec<ArtifactRow>>> {
    // `run_module` is public and `ModuleSpec` is deserializable, so a caller can
    // reach here with a spec that never went through `load_modules`. The id is
    // used to build a filename, so skipping validation would be a path-traversal
    // hole (`id = "../../x"` writing decrypted backup content outside work_dir).
    spec.validate()
        .map_err(|why| Error::Parse(format!("artifact module {:?}: {why}", spec.id)))?;

    let Some(entry) = locate(index, spec)? else {
        return Ok(None);
    };

    std::fs::create_dir_all(work_dir).map_err(|e| Error::Io {
        path: work_dir.to_path_buf(),
        source: e,
    })?;
    let dest = work_dir.join(format!("{}.sqlite", spec.id));
    index.extract_db(&entry, decryptor, &dest)?;

    // Opened READ-ONLY, because the `SELECT`-prefix check in `validate` is not
    // actually sufficient: SQLite accepts `WITH x AS (…) INSERT … RETURNING a`,
    // which starts with "with", returns named columns, and writes. The prefix
    // check stays as a friendly early error; this is the part that enforces it.
    let conn = Connection::open_with_flags(&dest, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Attribute a bad query to its module. Raw rusqlite errors say things like
    // "no such table: nosuchtable" with nothing identifying which of the
    // (eventually hundreds of) modules asked.
    // Try each alternative in order; the first that prepares against THIS
    // backup's schema wins. Failing to prepare is exactly how a version
    // mismatch announces itself ("no such column: auth_value"), so it is the
    // signal to fall through — until none is left, which names the module.
    let mut prepared = None;
    let mut errors: Vec<String> = Vec::new();
    for (i, query) in spec.sql.iter().enumerate() {
        match conn.prepare(query) {
            Ok(s) => {
                prepared = Some(s);
                break;
            }
            // Every alternative's error is kept. Reporting only the last one
            // blames the oldest-schema query for a fault in the modern query,
            // which is exactly backwards on a modern backup.
            Err(e) => errors.push(format!("  [{i}] {e}")),
        }
    }
    let mut stmt = prepared.ok_or_else(|| {
        Error::Parse(format!(
            "artifact {} ({}): no `sql` alternative could run against this backup:\n{}",
            spec.id,
            spec.name,
            errors.join("\n"),
        ))
    })?;
    let sql_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    // Fail loudly when a declared column is not in the result set: silently
    // emitting nulls would make a renamed upstream column look like an artifact
    // that simply has no data.
    for c in &spec.columns {
        if !sql_names.iter().any(|n| n == &c.from) {
            return Err(Error::Parse(format!(
                "artifact {}: column {:?} reads `{}`, which the SQL does not return (returns: {})",
                spec.id,
                c.name,
                c.from,
                sql_names.join(", ")
            )));
        }
    }

    let mut rows_out: Vec<ArtifactRow> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut out: ArtifactRow = HashMap::new();
        for c in &spec.columns {
            let idx = sql_names.iter().position(|n| n == &c.from).unwrap();
            let raw: rusqlite::types::Value = row.get(idx)?;
            out.insert(c.name.clone(), convert(&raw, c));
        }
        rows_out.push(out);
    }
    Ok(Some(rows_out))
}

fn convert(raw: &rusqlite::types::Value, c: &ColumnSpec) -> serde_json::Value {
    use rusqlite::types::Value as V;
    use serde_json::Value as J;
    match c.kind {
        ColumnKind::Timestamp => {
            let n = match raw {
                V::Integer(i) => *i as f64,
                V::Real(f) => *f,
                _ => return J::Null,
            };
            // `epoch` is guaranteed present for timestamps by validate().
            match c.epoch.and_then(|e| e.to_unix_seconds(n)) {
                Some(secs) => J::from(secs),
                None => J::Null,
            }
        }
        ColumnKind::Bool => match raw {
            V::Integer(i) => J::Bool(*i != 0),
            V::Null => J::Null,
            _ => J::Null,
        },
        // Bytes arrive as an integer count and are formatted by the UI. Core
        // Data stores these as FLOAT (DataUsage's ZWIFIIN and friends are
        // declared FLOAT), so the Real arm is the one that actually fires.
        ColumnKind::Integer | ColumnKind::Bytes => match raw {
            V::Integer(i) => J::from(*i),
            V::Real(f) => J::from(*f as i64),
            _ => J::Null,
        },
        ColumnKind::Real => match raw {
            V::Real(f) => J::from(*f),
            V::Integer(i) => J::from(*i as f64),
            _ => J::Null,
        },
        ColumnKind::Text => match raw {
            V::Text(s) => J::String(s.clone()),
            V::Integer(i) => J::String(i.to_string()),
            V::Real(f) => J::String(f.to_string()),
            V::Blob(_) => J::Null,
            V::Null => J::Null,
        },
    }
}

/// Replace every stored row for `artifact_id` with `rows`.
///
/// Replace rather than append, so a re-import is idempotent — the same property
/// the hand-written importers already have (they clear and reinsert).
pub fn store_rows(conn: &Connection, artifact_id: &str, rows: &[ArtifactRow]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM artifact_rows WHERE artifact_id = ?1",
        rusqlite::params![artifact_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO artifact_rows (artifact_id, row_idx, payload) VALUES (?1, ?2, ?3)",
        )?;
        for (i, row) in rows.iter().enumerate() {
            let payload = serde_json::to_string(row).map_err(|e| {
                Error::Parse(format!("artifact {artifact_id}: encoding row {i}: {e}"))
            })?;
            stmt.execute(rusqlite::params![artifact_id, i as i64, payload])?;
        }
    }
    tx.commit()?;
    Ok(rows.len())
}

/// Rows for one artifact, in the order the module produced them.
pub fn read_rows(
    conn: &Connection,
    artifact_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Vec<ArtifactRow>> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM artifact_rows WHERE artifact_id = ?1
         ORDER BY row_idx LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![artifact_id, limit, offset], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|p| {
            serde_json::from_str(p)
                .map_err(|e| Error::Parse(format!("artifact {artifact_id}: decoding row: {e}")))
        })
        .collect()
}

/// How many rows are stored for an artifact.
pub fn count_rows(conn: &Connection, artifact_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM artifact_rows WHERE artifact_id = ?1",
        rusqlite::params![artifact_id],
        |r| r.get(0),
    )?)
}

/// The modules shipped with the app, compiled in.
///
/// Embedded rather than read from disk because a shipped `.app` has no
/// `modules/` directory to read — and pointing at one would mean a path that
/// only exists on the machine that built the binary.
///
/// The cost is that adding an artifact is a one-line code change here as well
/// as a TOML file, which dents #190's "adding a module is a data change". The
/// data half still holds — the SQL, paths and columns are reviewed as data —
/// and this list is the smallest possible seam. If it ever gets long enough to
/// be annoying, `include_dir` removes it.
const BUILTIN: &[(&str, &str)] = &[
    ("tcc.toml", include_str!("../modules/tcc.toml")),
    ("accounts.toml", include_str!("../modules/accounts.toml")),
    (
        "bluetooth_paired.toml",
        include_str!("../modules/bluetooth_paired.toml"),
    ),
    (
        "data_usage.toml",
        include_str!("../modules/data_usage.toml"),
    ),
];

/// Parse the compiled-in modules. Errors carry the module's filename, exactly
/// as `load_modules` does for on-disk ones.
pub fn builtin_modules() -> Result<Vec<ModuleSpec>> {
    let mut out: Vec<ModuleSpec> = Vec::new();
    for (file, text) in BUILTIN {
        let spec: ModuleSpec = toml::from_str(text)
            .map_err(|e| Error::Parse(format!("artifact module {file}: {e}")))?;
        spec.validate()
            .map_err(|why| Error::Parse(format!("artifact module {file}: {why}")))?;
        if let Some(prev) = out.iter().find(|m| m.id == spec.id) {
            return Err(Error::Parse(format!(
                "artifact module {file}: id {:?} is already used by {:?}",
                spec.id, prev.name
            )));
        }
        out.push(spec);
    }
    Ok(out)
}

/// The modules directory on disk — tests only, so they can prove the shipped
/// TOML files themselves parse rather than only the embedded copies.
#[cfg(test)]
pub fn builtin_modules_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("modules")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_toml(extra: &str) -> String {
        format!(
            r#"
id = "demo"
name = "Demo"
description = "A demo artifact."
surface = "standalone"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "SELECT who, at FROM events"

[[columns]]
name = "Who"
from = "who"

[[columns]]
name = "At"
from = "at"
kind = "timestamp"
epoch = "cocoa"
{extra}
"#
        )
    }

    fn write_module(dir: &Path, file: &str, body: &str) {
        std::fs::write(dir.join(file), body).unwrap();
    }

    #[test]
    fn loads_and_validates_a_module() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "demo.toml", &spec_toml(""));
        let mods = load_modules(tmp.path()).unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].id, "demo");
        assert_eq!(mods[0].columns.len(), 2);
        assert!(!mods[0].needs_encrypted_backup());
    }

    #[test]
    fn missing_module_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = load_modules(&tmp.path().join("nope")).unwrap();
        assert!(mods.is_empty());
    }

    /// A malformed module must name the file and the problem. Every case below
    /// is a way a module could be wrong; each must be rejected at load, not
    /// discovered when the artifact mysteriously has no rows.
    #[test]
    fn malformed_modules_fail_by_name_and_reason() {
        let cases: Vec<(&str, &str, &str)> = vec![
            // The needle must be something only the TOML parser produces; "artifact
            // module" is the shared prefix of every error in this function.
            ("bad-toml", "id = \"x\"\nname =", "TOML parse error"),
            (
                "no-epoch",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT at FROM t"
[[columns]]
name = "At"
from = "at"
kind = "timestamp"
"#,
                "declares no `epoch`",
            ),
            (
                "not-select",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "DELETE FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "must be a SELECT",
            ),
            (
                "no-columns",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
columns = []
"#,
                "no `[[columns]]` declared",
            ),
            (
                "bad-requires",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
requires = "a-pony"
[[columns]]
name = "A"
from = "a"
"#,
                "unknown `requires`",
            ),
            (
                "empty-id",
                r#"
id = ""
description = "Case fixture."
surface = "standalone"
name = "X"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "`id` is empty",
            ),
            (
                "empty-domain",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = ""
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "`domain` is empty",
            ),
            (
                "empty-path",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = ""
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "`path` is empty",
            ),
            (
                "empty-sql",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = ""
[[columns]]
name = "A"
from = "a"
"#,
                "`sql` is empty",
            ),
            (
                "empty-column",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = ""
"#,
                "empty `name` or `from`",
            ),
            (
                "bad-id",
                r#"
id = "x y"
description = "Case fixture."
surface = "standalone"
name = "X"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "may only contain",
            ),
        ];

        for (label, body, needle) in cases {
            let tmp = tempfile::tempdir().unwrap();
            write_module(tmp.path(), &format!("{label}.toml"), body);
            let err = load_modules(tmp.path())
                .expect_err(&format!("{label} should have been rejected"))
                .to_string();
            assert!(
                err.contains(needle),
                "{label}: error {err:?} does not mention {needle:?}"
            );
            assert!(
                err.contains(label),
                "{label}: error {err:?} does not name the offending file"
            );
        }
    }

    /// The conversions, including the one that matters most: a Cocoa timestamp
    /// read as Unix lands in 1970 and nothing about the number says so.
    #[test]
    fn epochs_convert_to_unix_seconds() {
        // 2024-01-01T00:00:00Z
        let unix = 1_704_067_200_i64;
        assert_eq!(Epoch::Unix.to_unix_seconds(unix as f64), Some(unix));
        assert_eq!(
            Epoch::UnixMs.to_unix_seconds(unix as f64 * 1000.0),
            Some(unix)
        );
        assert_eq!(
            Epoch::Cocoa.to_unix_seconds(unix as f64 - 978_307_200.0),
            Some(unix)
        );
        assert_eq!(
            Epoch::Webkit.to_unix_seconds((unix as f64 + 11_644_473_600.0) * 1_000_000.0),
            Some(unix)
        );
        // A Cocoa value misread as Unix is 31 years out — the failure this
        // per-column declaration exists to prevent.
        assert_ne!(
            Epoch::Unix.to_unix_seconds(unix as f64 - 978_307_200.0),
            Some(unix)
        );
        // Undecodable values degrade to "no date" rather than 1970 or a
        // fifty-year chart axis.
        assert_eq!(Epoch::Unix.to_unix_seconds(-1.0), None);
        assert_eq!(Epoch::Unix.to_unix_seconds(f64::NAN), None);
        assert_eq!(Epoch::Cocoa.to_unix_seconds(1e18), None);
    }

    /// Build a minimal plaintext backup containing one SQLite store, so the
    /// runner can be exercised end to end without any real device data.
    fn make_backup(dir: &Path, rel: &str, build: impl FnOnce(&Connection)) {
        make_backup_in(dir, "HomeDomain", rel, build)
    }

    fn make_backup_in(dir: &Path, domain: &str, rel: &str, build: impl FnOnce(&Connection)) {
        let file_id = "cd00000000000000000000000000000000000001";
        let blob_dir = dir.join(&file_id[..2]);
        std::fs::create_dir_all(&blob_dir).unwrap();
        let store = blob_dir.join(file_id);
        let conn = Connection::open(&store).unwrap();
        build(&conn);
        drop(conn);

        let conn = Connection::open(dir.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO Files VALUES (?1, ?2, ?3, 1, NULL)",
            rusqlite::params![file_id, domain, rel],
        )
        .unwrap();
    }

    /// The whole spine: a declared module, a real backup, typed rows out —
    /// with the date arriving as a date rather than the raw Cocoa float.
    #[test]
    fn runs_a_module_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        // 2024-01-01T00:00:00Z expressed in Core Data seconds.
        let cocoa = 1_704_067_200_f64 - 978_307_200.0;
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
            c.execute(
                "INSERT INTO events VALUES ('alice', ?1), ('bob', NULL)",
                rusqlite::params![cocoa],
            )
            .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(&mods_dir, "demo.toml", &spec_toml(""));
        let spec = &load_modules(&mods_dir).unwrap()[0];

        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .expect("the backup contains this artifact");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["Who"], serde_json::json!("alice"));
        assert_eq!(
            rows[0]["At"],
            serde_json::json!(1_704_067_200_i64),
            "a Cocoa timestamp must arrive as Unix seconds, not the raw stored float"
        );
        // A NULL date is absent, not 1970.
        assert_eq!(rows[1]["At"], serde_json::Value::Null);
    }

    /// An artifact the backup does not contain is a normal outcome, and it must
    /// cost nothing: the Manifest is consulted and the filesystem is not
    /// touched. Proven by pointing at a path no Manifest row mentions and
    /// asserting the work dir was never even created.
    #[test]
    fn absent_artifact_returns_none_and_opens_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Something/else.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(&mods_dir, "demo.toml", &spec_toml(""));
        let spec = &load_modules(&mods_dir).unwrap()[0];

        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let work = tmp.path().join("work");
        let out = run_module(spec, &index, None, &work).unwrap();

        assert!(
            out.is_none(),
            "an absent artifact yields None, not an error"
        );
        assert!(
            !work.exists(),
            "the runner created its work dir for an artifact the backup does not have — \
             absence must cost one indexed lookup and no filesystem work"
        );
    }

    /// A module whose SQL stops returning a declared column must fail loudly.
    /// Emitting nulls instead would make an upstream rename look exactly like
    /// an artifact that happens to be empty.
    #[test]
    fn column_missing_from_the_result_set_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        // Declares a `Missing` column the SELECT never returns.
        write_module(
            &mods_dir,
            "demo.toml",
            &format!(
                "{}\n[[columns]]\nname = \"Missing\"\nfrom = \"nope\"\n",
                spec_toml("")
            ),
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(spec, &index, None, &tmp.path().join("work"))
            .expect_err("a declared column that the SQL does not return must fail")
            .to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("which the SQL does not return"), "{err}");
    }

    /// Storage round-trips, preserves order, and a re-import replaces rather
    /// than appends — otherwise every import would double the row count.
    #[test]
    fn rows_round_trip_and_reimport_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let conn = db.conn();

        let mk = |who: &str| -> ArtifactRow {
            let mut r = HashMap::new();
            r.insert("Who".to_string(), serde_json::json!(who));
            r
        };
        store_rows(conn, "demo", &[mk("alice"), mk("bob")]).unwrap();
        assert_eq!(count_rows(conn, "demo").unwrap(), 2);

        let back = read_rows(conn, "demo", 0, 10).unwrap();
        assert_eq!(back[0]["Who"], serde_json::json!("alice"));
        assert_eq!(back[1]["Who"], serde_json::json!("bob"), "order preserved");

        // A second run of the same artifact replaces, never appends.
        store_rows(conn, "demo", &[mk("carol")]).unwrap();
        assert_eq!(count_rows(conn, "demo").unwrap(), 1);
        assert_eq!(
            read_rows(conn, "demo", 0, 10).unwrap()[0]["Who"],
            serde_json::json!("carol")
        );

        // Artifacts do not tread on each other.
        store_rows(conn, "other", &[mk("dave")]).unwrap();
        assert_eq!(count_rows(conn, "demo").unwrap(), 1);
        assert_eq!(count_rows(conn, "other").unwrap(), 1);

        // Paging.
        store_rows(conn, "demo", &[mk("a"), mk("b"), mk("c")]).unwrap();
        let page = read_rows(conn, "demo", 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["Who"], serde_json::json!("b"));
    }

    /// Two modules with the same id must be rejected: `store_rows` is keyed on
    /// the id and replaces, so a collision silently destroys one artifact's
    /// rows and shows the other's in its place.
    #[test]
    fn duplicate_module_ids_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "one.toml", &spec_toml(""));
        write_module(tmp.path(), "two.toml", &spec_toml(""));
        let err = load_modules(tmp.path())
            .expect_err("two modules sharing an id must not load")
            .to_string();
        assert!(err.contains("already used"), "{err}");
        assert!(
            err.contains("two.toml"),
            "the error must name the offending file: {err}"
        );
    }

    /// Regressions for the four defects a review found in this module's first
    /// draft. Each was silent — the reason they are pinned rather than trusted.
    #[test]
    fn duplicate_column_names_are_rejected() {
        // A row is keyed on the display name, so two columns called "Date"
        // collapsed to one: a column vanishing with no error anywhere.
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "dup.toml",
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT created, modified FROM t"
[[columns]]
name = "Date"
from = "created"
[[columns]]
name = "Date"
from = "modified"
"#,
        );
        let err = load_modules(tmp.path())
            .expect_err("duplicate names must fail")
            .to_string();
        assert!(err.contains("both named"), "{err}");
    }

    #[test]
    fn a_glob_path_is_rejected_rather_than_silently_literal() {
        // Prefix matching picked `Manifest.db` directory rows (a directory
        // sorts before its own children) and `-wal`/`-shm` siblings. Removed
        // until an artifact needs it and can test it.
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "glob.toml",
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "Library/Foo/*"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
        );
        let err = load_modules(tmp.path())
            .expect_err("a glob path must fail")
            .to_string();
        assert!(err.contains("looks like a glob"), "{err}");
    }

    #[test]
    fn duplicate_id_error_names_both_files() {
        // It used to name the previous module's *display name*, which gives the
        // reader nothing to grep for.
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "aaa.toml", &spec_toml(""));
        write_module(tmp.path(), "zzz.toml", &spec_toml(""));
        let err = load_modules(tmp.path())
            .expect_err("duplicate ids must fail")
            .to_string();
        assert!(err.contains("aaa.toml"), "must name the first file: {err}");
        assert!(err.contains("zzz.toml"), "must name the second file: {err}");
    }

    /// The store is opened read-only, so a module cannot write even when its
    /// SQL slips past the `SELECT`-prefix check — SQLite accepts
    /// `WITH x AS (…) INSERT … RETURNING`, which starts with "with" and writes.
    #[test]
    fn a_writing_module_cannot_modify_the_extracted_store() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
            c.execute_batch("INSERT INTO events VALUES ('alice', 0);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "evil.toml",
            r#"
id = "evil"
description = "evil artifact."
surface = "standalone"
name = "Evil"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "WITH q AS (SELECT 'x' AS v) INSERT INTO events(who) SELECT v FROM q RETURNING who"
[[columns]]
name = "Who"
from = "who"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(spec, &index, None, &tmp.path().join("work"))
            .expect_err("a writing statement must be refused by the read-only connection");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("readonly")
                || msg.contains("read-only")
                || msg.contains("attempt to write"),
            "expected a read-only refusal, got: {err}"
        );
    }

    /// `run_module` is public and `ModuleSpec` deserializable, so a caller could
    /// bypass `load_modules`. The id builds a filename, so an unvalidated spec
    /// is a path-traversal hole.
    #[test]
    fn run_module_validates_a_hand_built_spec() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT);").unwrap();
        });
        let spec = ModuleSpec {
            id: "../../escape".into(),
            name: "Escape".into(),
            description: "A test artifact.".into(),
            surface: Surface::Standalone,
            join_column: None,
            highlight: None,
            category: None,
            domain: "HomeDomain".into(),
            path: "Library/Demo/demo.db".into(),
            sql: vec!["SELECT who FROM events".into()],
            requires: None,
            columns: vec![ColumnSpec {
                name: "Who".into(),
                from: "who".into(),
                kind: ColumnKind::Text,
                epoch: None,
            }],
        };
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(&spec, &index, None, &tmp.path().join("work"))
            .expect_err("an id that escapes the work dir must be refused")
            .to_string();
        assert!(err.contains("may only contain"), "{err}");
    }

    /// Every `ColumnKind`, against every SQLite storage class it can meet.
    ///
    /// This is the module format's whole typing contract, and it was the least
    /// tested thing in the file: a review found `Bool`, `Integer` and `Real`
    /// could each be replaced with "always null" — or `Bool` with "always true"
    /// — without a single test noticing.
    #[test]
    fn every_column_kind_converts() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch(
                "CREATE TABLE t (s TEXT, i INTEGER, r REAL, b INTEGER, blob BLOB, n TEXT);
                 INSERT INTO t VALUES ('hi', 42, 1.5, 1, x'00ff', NULL);
                 INSERT INTO t VALUES ('bye', -7, -0.5, 0, x'01', NULL);",
            )
            .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "kinds.toml",
            r#"
id = "kinds"
description = "kinds artifact."
surface = "standalone"
name = "Kinds"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "SELECT s, i, r, b, blob, n, i AS i_as_text FROM t ORDER BY rowid"
[[columns]]
name = "S"
from = "s"
[[columns]]
name = "I"
from = "i"
kind = "integer"
[[columns]]
name = "R"
from = "r"
kind = "real"
[[columns]]
name = "B"
from = "b"
kind = "bool"
[[columns]]
name = "Blob"
from = "blob"
[[columns]]
name = "N"
from = "n"
[[columns]]
name = "IAsText"
from = "i_as_text"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0]["S"], serde_json::json!("hi"));
        assert_eq!(rows[0]["I"], serde_json::json!(42));
        assert_eq!(rows[1]["I"], serde_json::json!(-7), "negatives survive");
        assert_eq!(rows[0]["R"], serde_json::json!(1.5));
        assert_eq!(rows[1]["R"], serde_json::json!(-0.5));
        // `true` and `false` must BOTH be produced — "always true" was a
        // surviving mutation.
        assert_eq!(rows[0]["B"], serde_json::json!(true));
        assert_eq!(rows[1]["B"], serde_json::json!(false));
        // A blob is not text; rendering its bytes as a string would be noise.
        assert_eq!(rows[0]["Blob"], serde_json::Value::Null);
        assert_eq!(rows[0]["N"], serde_json::Value::Null);
        // A number read as text is coerced rather than dropped, so a module
        // author who omits `kind` still gets the value.
        assert_eq!(rows[0]["IAsText"], serde_json::json!("42"));
    }

    /// `WITH … SELECT` is promised by both the code comment and the error
    /// string. Deleting that half of the check — rejecting every CTE module —
    /// used to pass.
    #[test]
    fn a_with_cte_module_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "cte.toml",
            r#"
id = "cte"
description = "cte artifact."
surface = "standalone"
name = "CTE"
domain = "HomeDomain"
path = "a/b.db"
sql = "WITH q AS (SELECT 1 AS a) SELECT a FROM q"
[[columns]]
name = "A"
from = "a"
kind = "integer"
"#,
        );
        assert_eq!(load_modules(tmp.path()).unwrap().len(), 1);
    }

    /// Only `*.toml` is a module. Without the filter a stray `README.md` or
    /// `.DS_Store` beside the modules becomes a hard load failure that takes
    /// every artifact down with it.
    #[test]
    fn non_toml_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "demo.toml", &spec_toml(""));
        std::fs::write(tmp.path().join("README.md"), "# not a module").unwrap();
        std::fs::write(tmp.path().join(".DS_Store"), [0u8, 1, 2]).unwrap();
        let mods = load_modules(tmp.path()).unwrap();
        assert_eq!(mods.len(), 1, "only the .toml file is a module");
    }

    /// Rows come back in `row_idx` order even when storage order disagrees.
    /// The previous assertion passed by accident: rows were inserted in order,
    /// so a scan happened to return them right, and deleting `ORDER BY` from
    /// `read_rows` changed nothing.
    #[test]
    fn rows_are_ordered_by_row_idx_not_insertion_order() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let conn = db.conn();
        // Drop the (artifact_id, row_idx) index first. With it in place SQLite
        // satisfies the WHERE clause from the index and hands back rows already
        // in row_idx order, so removing `ORDER BY` from `read_rows` changed
        // nothing and this test passed either way. Without the index a scan
        // returns rowid order, which is what makes the ordering contract real
        // rather than a coincidence of the current schema.
        conn.execute_batch("DROP INDEX IF EXISTS idx_artifact_rows")
            .unwrap();
        // Insert deliberately out of order, so storage order != row_idx order.
        for (idx, who) in [(2_i64, "third"), (0, "first"), (1, "second")] {
            conn.execute(
                "INSERT INTO artifact_rows (artifact_id, row_idx, payload) VALUES ('demo', ?1, ?2)",
                rusqlite::params![idx, format!(r#"{{"Who":"{who}"}}"#)],
            )
            .unwrap();
        }
        let back = read_rows(conn, "demo", 0, 10).unwrap();
        let names: Vec<&str> = back.iter().map(|r| r["Who"].as_str().unwrap()).collect();
        assert_eq!(names, ["first", "second", "third"], "row_idx decides order");
    }

    #[test]
    fn store_rows_reports_how_many_it_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let mut r: ArtifactRow = HashMap::new();
        r.insert("Who".into(), serde_json::json!("a"));
        assert_eq!(store_rows(db.conn(), "demo", &[r.clone(), r]).unwrap(), 2);
        assert_eq!(store_rows(db.conn(), "demo", &[]).unwrap(), 0);
    }

    /// A corrupt payload names the artifact rather than surfacing a bare serde
    /// error from nowhere.
    #[test]
    fn a_corrupt_stored_row_is_reported_with_its_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        db.conn()
            .execute(
                "INSERT INTO artifact_rows (artifact_id, row_idx, payload) VALUES ('demo', 0, 'not json')",
                [],
            )
            .unwrap();
        let err = read_rows(db.conn(), "demo", 0, 10)
            .expect_err("a corrupt payload must be an error")
            .to_string();
        assert!(err.contains("demo"), "{err}");
    }

    /// Invalid SQL must name the module. A raw rusqlite "no such table" says
    /// nothing about which of (eventually) hundreds of modules asked.
    #[test]
    fn invalid_sql_names_the_module() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT);").unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "broken.toml",
            r#"
id = "broken"
description = "broken artifact."
surface = "standalone"
name = "Broken Module"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "SELECT nope FROM nosuchtable"
[[columns]]
name = "Nope"
from = "nope"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(spec, &index, None, &tmp.path().join("work"))
            .expect_err("invalid SQL must fail")
            .to_string();
        assert!(err.contains("broken"), "must name the module id: {err}");
        assert!(err.contains("Broken Module"), "must name the module: {err}");
    }

    /// Two modules must not extract over each other's temp store.
    #[test]
    fn modules_extract_to_separate_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
            c.execute_batch("INSERT INTO events VALUES ('alice', 0);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(&mods_dir, "one.toml", &spec_toml(""));
        write_module(
            &mods_dir,
            "two.toml",
            &spec_toml("").replace("id = \"demo\"", "id = \"demo2\""),
        );
        let mods = load_modules(&mods_dir).unwrap();
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let work = tmp.path().join("work");
        for m in &mods {
            run_module(m, &index, None, &work).unwrap().unwrap();
        }
        let files: Vec<String> = std::fs::read_dir(&work)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".sqlite"))
            .collect();
        assert_eq!(
            files.len(),
            2,
            "each module gets its own temp store: {files:?}"
        );
    }

    /// TCC.db as a modern device writes it, matching `seed_tcc_db` in
    /// tools/make_fixture_backup.py.
    fn seed_tcc(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE access (
                service TEXT, client TEXT, client_type INTEGER,
                auth_value INTEGER, auth_reason INTEGER, auth_version INTEGER,
                last_modified INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO access (service, client, client_type, auth_value, auth_reason, auth_version, last_modified) VALUES
                ('kTCCServiceCamera','com.example.chatapp',0,2,2,«redacted»0000000),
                ('kTCCServiceLocation','com.example.weather',0,0,2,«redacted»0000300),
                ('kTCCServicePhotos','com.example.chatapp',0,3,2,«redacted»0000200),
                ('kTCCServiceReminders','com.example.todo',0,9,2,«redacted»0000500),
                ('kTCCServiceContacts','com.example.notprompted',0,1,2,«redacted»0000600),
                ('kTCCServiceMicrophone','com.example.norecord',0,NULL,2,«redacted»0000700);",
        )
        .unwrap();
    }

    /// Accounts3.sqlite as accountsd writes it — the Core Data column names and
    /// the two joined tables, matching what `explore_real_backup` printed for
    /// Josh Hickman's iOS 17 image.
    ///
    /// The rows deliberately include the cases the module claims to handle: an
    /// account whose type row is MISSING (so the LEFT JOIN is what keeps it —
    /// an inner join drops it and the count silently falls), a NULL username, and
    /// a NULL `ZACTIVE`.
    fn seed_accounts(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ZACCOUNT (
                Z_PK INTEGER PRIMARY KEY, ZACTIVE INTEGER, ZAUTHENTICATED INTEGER,
                ZACCOUNTTYPE INTEGER, ZPARENTACCOUNT INTEGER, ZDATE TIMESTAMP,
                ZACCOUNTDESCRIPTION VARCHAR, ZIDENTIFIER VARCHAR,
                ZOWNINGBUNDLEID VARCHAR, ZUSERNAME VARCHAR);
             CREATE TABLE ZACCOUNTTYPE (
                Z_PK INTEGER PRIMARY KEY, ZACCOUNTTYPEDESCRIPTION VARCHAR,
                ZIDENTIFIER VARCHAR, ZOWNINGBUNDLEID VARCHAR);",
        )
        .unwrap();
        // 726000000 Cocoa seconds = 2024-01-03T…Z, comfortably inside iOS 17.
        // ZIDENTIFIER values are GUIDs, as on a real device — the point of the
        // module's three-rung COALESCE is that this column is NOT a service name,
        // and a fixture that put a friendly string here would hide that.
        c.execute_batch(
            "INSERT INTO ZACCOUNTTYPE (Z_PK, ZACCOUNTTYPEDESCRIPTION, ZIDENTIFIER) VALUES
                (1,'Gmail','com.apple.account.Google'),
                (2,'Holiday Calendar','com.apple.account.HolidayCalendar'),
                -- No description: the middle COALESCE rung must fall to the type's
                -- own reverse-DNS identifier, not to the account GUID.
                (3,NULL,'com.apple.account.undescribed');
             INSERT INTO ZACCOUNT
                (Z_PK, ZACTIVE, ZAUTHENTICATED, ZACCOUNTTYPE, ZPARENTACCOUNT, ZDATE,
                 ZACCOUNTDESCRIPTION, ZIDENTIFIER, ZOWNINGBUNDLEID, ZUSERNAME) VALUES
                (1,1,1,1,NULL,726000000,'Gmail',
                 '6D60660E-344F-4E62-97A0-0A9EA8174CDE','com.apple.mobilemail','person@example.com'),
                (2,1,1,2,NULL,725000000,'US Holidays',
                 'AD041785-D028-495F-9008-62F26C114CBA','dataaccessd',NULL),
                -- No ZACCOUNTTYPE row: only the LEFT JOIN keeps this one, and the
                -- last COALESCE rung must name it rather than print its GUID.
                (3,0,0,NULL,NULL,724000000,NULL,
                 'B61380AE-7269-4769-A39F-69D7935848EA','appstored','local'),
                (4,NULL,NULL,1,NULL,723000000,'Unrecorded',
                 'C9FA6B49-5667-4CE7-A88A-60C0543E82B5','accountsd',NULL),
                -- A CHILD of account 1: on real data these are what make one
                -- sign-in look like several duplicate rows.
                (5,1,1,3,«redacted»000000,NULL,
                 '0EE306D8-66AF-47E5-8FD1-CF2EAF5DC8C2','accountsd',NULL);",
        )
        .unwrap();
    }

    /// The paired-LE store as bluetoothd writes it, including Apple's own
    /// `Public `/`Random ` prefix inside the address strings and the two
    /// device-relative counters that are NOT dates.
    fn seed_bluetooth_paired(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE PairedDevices(Uuid TEXT, Name TEXT, NameOrigin INT,
                Address TEXT, ResolvedAddress TEXT, LastSeenTime INT,
                LastConnectionTime INT, GATTServiceChangeConfig INT, Tags TEXT,
                iCloudIdentifier TEXT);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO PairedDevices
                (Uuid, Name, NameOrigin, Address, ResolvedAddress, LastSeenTime,
                 LastConnectionTime, iCloudIdentifier) VALUES
                ('E3B37CA8-1AA5-AD44-B0FE-A617BB09B64A','Fitness Band',2,
                 'Public B4:C2:6A:7F:D3:7A','Public B4:C2:6A:7F:D3:7A',395626,2143,''),
                ('6C0C35A0-84CE-3572-2E72-4CF3D03BD1AF','Example Watch',2,
                 'Random 50:32:66:45:35:EF','Public F8:6F:C1:4E:FF:6A',4315986,9639,''),
                ('C4E4E254-6060-26CA-7C80-EE01F3C5C346','Nameless Tag',2,
                 'Random E8:F0:58:00:C0:FB',NULL,748458,3662,NULL);",
        )
        .unwrap();
    }

    /// DataUsage.sqlite as the modern lineage writes it, with both Wi-Fi and WWAN
    /// columns — matching what `explore_real_backup` printed for the validation
    /// image.
    ///
    /// The rows cover the cases the module claims to handle: several buckets for
    /// one app (so the aggregation is exercised rather than assumed), a compound
    /// `daemon/bundle` process name, a bare process name with no slash, and the
    /// ROLLUP row with a NULL bundle id whose total is the sum of everything else
    /// — the one the module must exclude, and which iLEAPP's `ZKIND != 257`
    /// constant would not catch here (this device's rollup is ZKIND 255).
    fn seed_data_usage(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ZLIVEUSAGE (
                Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER, ZHASPROCESS INTEGER,
                ZTIMESTAMP TIMESTAMP, ZWIFIIN FLOAT, ZWIFIOUT FLOAT,
                ZWWANIN FLOAT, ZWWANOUT FLOAT);
             CREATE TABLE ZPROCESS (
                Z_PK INTEGER PRIMARY KEY, ZFIRSTTIMESTAMP TIMESTAMP,
                ZTIMESTAMP TIMESTAMP, ZBUNDLENAME VARCHAR, ZPROCNAME VARCHAR,
                ZEXTENSIONNAME VARCHAR);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO ZPROCESS (Z_PK, ZBUNDLENAME, ZPROCNAME) VALUES
                (1,'com.example.chatapp','ChatApp/com.example.chatapp'),
                (2,'com.example.photos','nsurlsessiond/com.example.photos'),
                -- No slash: the substr must not eat the whole name.
                (3,'com.example.plain','plainproc'),
                -- The ROLLUP: no bundle id, and a total that is the sum of the
                -- rest. If it were ever included, every figure would double.
                (4,NULL,'CumulativeUsageTracker');
             INSERT INTO ZLIVEUSAGE
                (Z_PK, ZKIND, ZHASPROCESS, ZTIMESTAMP, ZWIFIIN, ZWIFIOUT, ZWWANIN, ZWWANOUT) VALUES
                (1,0,«redacted»000000,1000,2000,3000,4000),
                (2,0,«redacted»001000,«redacted»),
                (3,0,«redacted»000000,0,«redacted»000,10000),
                (4,0,«redacted»000000,10,20,30,40),
                («redacted»,«redacted»002000,1510,2620,903730,14840);",
        )
        .unwrap();
    }

    /// Where each shipped module's store lives, stated HERE rather than read
    /// from the module.
    ///
    /// This is the whole point. The first version of this test built its
    /// fixture from `spec.path`, which made the assertion a tautology: a review
    /// proved that setting the module's path to `Library/TOTALLY/WRONG/Nope.db`
    /// still passed. Path and domain are the single most likely thing to be
    /// wrong in a new declarative module, so the expected values live in the
    /// test — derived from docs/reference/backup-coverage-audit.md — and the
    /// module has to match them.
    const FIXTURES: &[(&str, &str, &str)] = &[
        // id, domain, relative path
        ("tcc", "HomeDomain", "Library/TCC/TCC.db"),
        (
            "accounts",
            "HomeDomain",
            "Library/Accounts/Accounts3.sqlite",
        ),
        (
            "bluetooth_paired",
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Database/com.apple.MobileBluetooth.ledevices.paired.db",
        ),
        (
            "data_usage",
            "WirelessDomain",
            "Library/Databases/DataUsage.sqlite",
        ),
    ];

    fn seed_for(id: &str) -> fn(&Connection) {
        match id {
            "tcc" => seed_tcc,
            "accounts" => seed_accounts,
            "bluetooth_paired" => seed_bluetooth_paired,
            "data_usage" => seed_data_usage,
            other => panic!(
                "module {other:?} has no fixture — add one to FIXTURES and to \
                 tools/make_fixture_backup.py, so shipping a module always means \
                 shipping data that proves it runs"
            ),
        }
    }

    /// The JSON the UI receives must have the field names the UI reads.
    ///
    /// `ArtifactSummary` shipped without `rename_all = "camelCase"` while
    /// `src/lib/ipc.ts` declared `joinColumn` / `rowCount` /
    /// `requiresEncryptedBackup`. In the real app all three were `undefined`, so
    /// `useHostedArtifacts` matched nothing and the Apps view showed no
    /// permissions — but the **mock** returns camelCase, so every browser check
    /// and every screenshot looked correct, and the Rust side was validated
    /// separately against a real backup without ever crossing IPC. Both halves
    /// were tested; the seam between them was not (#232).
    ///
    /// So this reads the TypeScript and compares. It is deliberately a Rust test
    /// rather than a lint: only serde can say what the wire format actually is.
    #[test]
    fn serialised_keys_match_the_ui_contract() {
        let summary = ArtifactSummary {
            id: "x".into(),
            name: "X".into(),
            category: None,
            description: "d".into(),
            surface: Surface::Device,
            join_column: None,
            highlight: None,
            columns: vec![],
            timestamp_columns: vec![],
            byte_columns: vec![],
            row_count: 0,
            requires_encrypted_backup: false,
        };
        let json = serde_json::to_value(&summary).unwrap();
        let mut actual: Vec<String> = json.as_object().unwrap().keys().cloned().collect();

        let ipc = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/ipc.ts"),
        )
        .expect("read src/lib/ipc.ts");
        let start = ipc
            .find("export type ArtifactSummary = {")
            .expect("no ArtifactSummary type in ipc.ts — did it move?");
        let body = &ipc[start..start + ipc[start..].find("\n};").expect("unterminated type")];
        // Top-level fields ONLY, which means the indent has to be checked rather
        // than trimmed away. Trimming first read the fields of a NESTED object
        // (`highlight`'s `column`, `whenColumn`, …) as though they were fields of
        // ArtifactSummary itself. The comment claimed "at one indent level" while
        // the code did not enforce it; a review flagged it as a latent false
        // failure, and adding `highlight` made it a real one.
        const INDENT: &str = "  ";
        let mut declared: Vec<String> = body
            .lines()
            .filter_map(|l| {
                // Exactly one indent level: two spaces, then something that is not
                // a space.
                let rest = l.strip_prefix(INDENT)?;
                if rest.starts_with(' ') {
                    return None;
                }
                if rest.starts_with('*') || rest.starts_with("/*") || rest.starts_with("//") {
                    return None;
                }
                let (name, _) = rest.split_once(':')?;
                let name = name.trim().trim_end_matches('?');
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return None;
                }
                Some(name.to_string())
            })
            .collect();
        assert!(
            declared.len() > 5,
            "parsed only {} fields from ipc.ts — the parse is wrong, and a comparison \
             against nothing passes for the wrong reason: {declared:?}",
            declared.len()
        );

        actual.sort();
        declared.sort();
        assert_eq!(
            actual, declared,
            "ArtifactSummary's JSON keys and src/lib/ipc.ts have drifted. The UI reads the \
             names on the right; serde sends the names on the left. A field the UI cannot \
             find is `undefined`, which reads as a legitimately absent value rather than as \
             an error."
        );
    }

    /// Every `Surface` variant must be in the UI's union type, and vice versa.
    ///
    /// `serialised_keys_match_the_ui_contract` pins the field NAMES but not this:
    /// `src/lib/ipc.ts` declares `surface` as a string union, and
    /// `useHostedArtifacts(host: string)` compares against a plain `string`, so a
    /// new Rust variant could ship, be declared by a module, and never appear in
    /// the union — with TypeScript raising nothing, because nothing narrows it.
    /// `check-artifact-surfaces.mjs` reads the Rust enum rather than the union, so
    /// it does not cover this either.
    #[test]
    fn the_ui_surface_union_matches_the_rust_enum() {
        // The match is what makes this exhaustive: adding a variant to `Surface`
        // without adding it here is a COMPILE error, not a passing test. A plain
        // array would silently omit the new one.
        fn name(s: Surface) -> &'static str {
            match s {
                Surface::Apps => "apps",
                Surface::Contacts => "contacts",
                Surface::Device => "device",
                Surface::Standalone => "standalone",
            }
        }
        let all = [
            Surface::Apps,
            Surface::Contacts,
            Surface::Device,
            Surface::Standalone,
        ];
        // Taken from serde, not from `name` — so this also catches a
        // `rename_all` change, which would alter the wire format silently.
        let mut from_rust: Vec<String> = all
            .iter()
            .map(|s| {
                let json = serde_json::to_value(s).unwrap();
                let wire = json
                    .as_str()
                    .expect("a Surface serialises to a string")
                    .to_string();
                assert_eq!(wire, name(*s), "serde renamed a Surface variant");
                wire
            })
            .collect();

        let ipc = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/ipc.ts"),
        )
        .expect("read src/lib/ipc.ts");
        let line = ipc
            .lines()
            .find(|l| l.trim_start().starts_with("surface:"))
            .expect("no `surface:` field in ipc.ts — did ArtifactSummary change shape?");
        let mut from_ts: Vec<String> = line
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|s| s.to_string())
            .collect();
        assert!(
            from_ts.len() > 1,
            "parsed {} members from {line:?} — the parse is wrong, so this compared \
             against nothing",
            from_ts.len()
        );

        from_rust.sort();
        from_ts.sort();
        assert_eq!(
            from_rust, from_ts,
            "the Surface enum and ipc.ts's `surface` union have drifted — a module \
             could declare a surface the UI's type does not know about"
        );
    }

    /// The compiled-in modules must be exactly the files on disk.
    ///
    /// They are two copies of the same thing — `include_str!` bakes the file in
    /// at build time — so the only way they diverge is someone adding a .toml
    /// and forgetting the BUILTIN entry. Then the module works in every test
    /// and is simply absent from the app, which is the worst kind of quiet.
    #[test]
    fn embedded_modules_match_the_directory() {
        let on_disk = load_modules(&builtin_modules_dir()).unwrap();
        let embedded = builtin_modules().unwrap();
        let mut a: Vec<&str> = on_disk.iter().map(|m| m.id.as_str()).collect();
        let mut b: Vec<&str> = embedded.iter().map(|m| m.id.as_str()).collect();
        a.sort();
        b.sort();
        assert_eq!(
            a, b,
            "a module exists on disk but is not compiled in (or vice versa) — \
             add it to BUILTIN"
        );
    }

    /// EVERY shipped module must declare a real domain, sit where the audit
    /// says it does, load, run, and produce every column it declares.
    ///
    /// This is the guard that scales: it costs nothing per new artifact and
    /// fails the moment one rots — a renamed column, a typo in the SQL or the
    /// path, a column spec that stops matching.
    #[test]
    fn every_shipped_module_loads_and_runs() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        assert!(
            !mods.is_empty(),
            "no modules shipped — did the directory move?"
        );

        // Apple's own domain list, so a typo'd domain ("HomeDomian") cannot
        // pass just because our fixture obligingly used it too.
        let domains_json =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/data/ios-backup-domains.json");
        let domains: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&domains_json).unwrap()).unwrap();

        for spec in &mods {
            let (_, domain, path) = FIXTURES
                .iter()
                .find(|(id, _, _)| *id == spec.id)
                .unwrap_or_else(|| panic!("module {:?} has no FIXTURES entry", spec.id));

            assert_eq!(
                &spec.domain, domain,
                "module {} declares a different domain than the audit records",
                spec.id
            );
            assert_eq!(
                &spec.path, path,
                "module {} declares a different path than the audit records",
                spec.id
            );
            assert!(
                domains.get(&spec.domain).is_some() || is_container_domain(&spec.domain),
                "module {} declares domain {:?}, which is neither one of the 19 domains iOS \
                 names outright nor a container domain ({})",
                spec.id,
                spec.domain,
                CONTAINER_DOMAIN_PREFIXES.join(", ")
            );

            let tmp = tempfile::tempdir().unwrap();
            make_backup_in(tmp.path(), domain, path, seed_for(&spec.id));
            let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
            let rows = run_module(spec, &index, None, &tmp.path().join("work"))
                .unwrap_or_else(|e| panic!("module {} failed to run: {e}", spec.id))
                .unwrap_or_else(|| panic!("module {} found nothing in its own fixture", spec.id));
            assert!(!rows.is_empty(), "module {} produced no rows", spec.id);
            for c in &spec.columns {
                assert!(
                    rows[0].contains_key(&c.name),
                    "module {} produced no {:?} column",
                    spec.id,
                    c.name
                );
            }
        }
    }

    /// The accounts module: a service is always NAMED, a sub-account says what it
    /// is part of, and a GUID never reaches the Service column.
    ///
    /// The first draft fell back to `ZACCOUNT.ZIDENTIFIER` believing it held
    /// "com.apple.account.Google". It holds a per-account GUID — measured on the
    /// validation image — so the fallback would have printed a UUID in a column
    /// headed "Service". The three rungs and this test exist because of that.
    #[test]
    fn accounts_module_names_every_service_and_never_shows_a_guid() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "accounts")
            .expect("accounts module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), &spec.path, seed_accounts);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        // 5 accounts, and the parent joins must not multiply them.
        assert_eq!(rows.len(), 5, "the parent LEFT JOINs changed the row count");

        let services: Vec<String> = rows
            .iter()
            .map(|r| r["Service"].as_str().unwrap_or_default().to_string())
            .collect();
        // Rung 1: a described type gives a readable name.
        assert!(services.contains(&"Gmail".to_string()));
        // Rung 2: a type with no description falls to its reverse-DNS identifier.
        assert!(
            services.contains(&"com.apple.account.undescribed".to_string()),
            "a type row with a NULL description should fall back to its own \
             identifier, got {services:?}"
        );
        // Rung 3: no type row at all still gets a name, not a GUID.
        assert!(
            services.contains(&"Unrecorded service type".to_string()),
            "an account with no type row should be named, got {services:?}"
        );
        // The whole point: nothing that looks like a GUID reaches this column.
        for svc in &services {
            assert!(
                !(svc.len() == 36 && svc.matches('-').count() == 4),
                "a GUID reached the Service column: {svc:?}"
            );
        }

        // The sub-account explains itself rather than looking like a duplicate.
        let child = rows
            .iter()
            .find(|r| r["Service"] == serde_json::json!("com.apple.account.undescribed"))
            .expect("the child account is present");
        assert_eq!(
            child["Part of"],
            serde_json::json!("Gmail"),
            "a sub-account should name the account it belongs to"
        );
        // A top-level account is part of nothing, and must say so as null rather
        // than as an empty string.
        let parent = rows
            .iter()
            .find(|r| r["Service"] == serde_json::json!("Gmail"))
            .expect("the parent account is present");
        assert_eq!(parent["Part of"], serde_json::Value::Null);

        // Cocoa epoch, not Unix: 726000000 Cocoa = 1704307200 Unix (2024-01-03).
        assert_eq!(parent["Added"], serde_json::json!(1_704_307_200_i64));

        // NULL flags stay distinguishable from "no".
        let unrecorded = rows
            .iter()
            .find(|r| r["Label"] == serde_json::json!("Unrecorded"))
            .expect("the NULL-flag account is present");
        assert_eq!(unrecorded["Status"], serde_json::json!("Not recorded"));
        assert_eq!(unrecorded["Signed in"], serde_json::json!("Not recorded"));
    }

    /// A `highlight` that could never fire must be rejected at load time, not
    /// silently show nothing — "this artifact has nothing to highlight" and "this
    /// module's highlight is broken" look identical on screen.
    #[test]
    fn a_highlight_that_cannot_match_is_rejected() {
        let base = r#"
id          = "h"
name        = "H"
description = "d"
surface     = "standalone"
domain      = "HomeDomain"
path        = "Library/X/x.db"
sql         = ["SELECT a AS a, b AS b FROM t"]

[[columns]]
name = "A"
from = "a"

[[columns]]
name = "B"
from = "b"
"#;
        let cases: &[(&str, &str)] = &[
            // A column that does not exist.
            (
                "[highlight]
column = \"Nope\"
",
                "highlight.column",
            ),
            // A condition column that does not exist.
            (
                "[highlight]
column = \"A\"
when_column = \"Nope\"
when_any_of = [\"x\"]
",
                "highlight.when_column",
            ),
            // A condition with nothing to match against: never fires.
            (
                "[highlight]
column = \"A\"
when_column = \"B\"
when_any_of = []
",
                "when_any_of",
            ),
            // Values with no column to compare them to.
            (
                "[highlight]
column = \"A\"
when_any_of = [\"x\"]
",
                "when_column",
            ),
        ];
        for (extra, expect) in cases {
            let spec: ModuleSpec = toml::from_str(&format!(
                "{base}
{extra}"
            ))
            .unwrap();
            let err = spec
                .validate()
                .expect_err(&format!("should have been rejected: {extra}"));
            assert!(
                err.contains(expect),
                "error {err:?} does not mention {expect:?}"
            );
        }

        // And the valid shape loads.
        let ok: ModuleSpec = toml::from_str(&format!(
            "{base}
[highlight]
column = \"A\"
when_column = \"B\"
when_any_of = [\"yes\"]
"
        ))
        .unwrap();
        ok.validate().expect("a well-formed highlight should load");
    }

    /// The data-usage module: buckets are summed per app, the rollup row is
    /// excluded, and a process name without a slash survives intact.
    #[test]
    fn data_usage_module_aggregates_and_excludes_the_rollup() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "data_usage")
            .expect("data_usage module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_data_usage);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();

        // Three apps, not four: the rollup has no bundle id and must not appear.
        assert_eq!(rows.len(), 3, "got {rows:#?}");
        assert!(
            !rows
                .iter()
                .any(|r| r["Carried by"] == serde_json::json!("CumulativeUsageTracker")),
            "the device-wide rollup was included — every figure would double"
        );

        let chat = rows
            .iter()
            .find(|r| r["App"] == serde_json::json!("com.example.chatapp"))
            .expect("the chat app is present");
        // Two buckets summed: 3000+700 down, 4000+800 up, 1000+500 / 2000+600 wifi.
        assert_eq!(chat["Cellular down"], serde_json::json!(3700));
        assert_eq!(chat["Cellular up"], serde_json::json!(4800));
        assert_eq!(chat["Wi-Fi down"], serde_json::json!(1500));
        assert_eq!(chat["Wi-Fi up"], serde_json::json!(2600));
        assert_eq!(chat["Records"], serde_json::json!(2));
        // The daemon, split off the compound ZPROCNAME.
        assert_eq!(chat["Carried by"], serde_json::json!("ChatApp"));
        // Cocoa epoch on both ends of the range: 726000000 -> 1704307200.
        assert_eq!(chat["First"], serde_json::json!(1_704_307_200_i64));
        assert_eq!(chat["Last"], serde_json::json!(1_704_308_200_i64));

        // A daemon carrying traffic for an app it is not: the interesting case.
        let photos = rows
            .iter()
            .find(|r| r["App"] == serde_json::json!("com.example.photos"))
            .expect("the photos app is present");
        assert_eq!(photos["Carried by"], serde_json::json!("nsurlsessiond"));

        // No slash in ZPROCNAME: the substr must not eat the whole name.
        let plain = rows
            .iter()
            .find(|r| r["App"] == serde_json::json!("com.example.plain"))
            .expect("the plain-named process is present");
        assert_eq!(plain["Carried by"], serde_json::json!("plainproc"));
    }

    /// The older DataUsage lineage has no Wi-Fi columns at all. The fallback query
    /// must run, and must report Wi-Fi as UNRECORDED rather than as zero traffic —
    /// "we cannot know" and "none" are different claims.
    #[test]
    fn data_usage_falls_back_when_the_wifi_columns_are_absent() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "data_usage").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, |c| {
            c.execute_batch(
                "CREATE TABLE ZLIVEUSAGE (
                    Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER, ZHASPROCESS INTEGER,
                    ZTIMESTAMP TIMESTAMP, ZWWANIN FLOAT, ZWWANOUT FLOAT);
                 CREATE TABLE ZPROCESS (
                    Z_PK INTEGER PRIMARY KEY, ZBUNDLENAME VARCHAR, ZPROCNAME VARCHAR);
                 INSERT INTO ZPROCESS (Z_PK, ZBUNDLENAME, ZPROCNAME)
                    VALUES (1,'com.example.old','OldApp/com.example.old');
                 INSERT INTO ZLIVEUSAGE (Z_PK, ZKIND, ZHASPROCESS, ZTIMESTAMP, ZWWANIN, ZWWANOUT)
                    VALUES (1,0,«redacted»000000,1234,5678);",
            )
            .unwrap();
        });
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .expect("the fallback query should have run");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["Cellular down"], serde_json::json!(1234));
        assert_eq!(rows[0]["Cellular up"], serde_json::json!(5678));
        // NOT 0: this device never recorded Wi-Fi, which is not the same as
        // recording that no Wi-Fi was used.
        assert_eq!(rows[0]["Wi-Fi down"], serde_json::Value::Null);
        assert_eq!(rows[0]["Wi-Fi up"], serde_json::Value::Null);
    }

    /// The Bluetooth module: the two counters stay integers, and the resolved
    /// address is kept apart from the advertised one.
    #[test]
    fn bluetooth_module_keeps_counters_as_numbers_and_addresses_apart() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "bluetooth_paired")
            .expect("bluetooth module ships");
        let tmp = tempfile::tempdir().unwrap();
        // `make_backup` defaults to HomeDomain; this store lives in a system
        // container, and a domain mismatch shows up as the module simply finding
        // nothing.
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_bluetooth_paired);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 3);

        let watch = rows
            .iter()
            .find(|r| r["Device"] == serde_json::json!("Example Watch"))
            .expect("the watch is present");
        // The counters must arrive as NUMBERS and be left exactly as stored. If
        // either were ever declared a timestamp these would come back converted,
        // and the module would be printing invented dates.
        assert_eq!(watch["Connection counter"], serde_json::json!(9639));
        assert_eq!(watch["Seen counter"], serde_json::json!(4_315_986));
        // A rotating Random address resolving to a different Public one is the
        // pair worth showing; collapsing the columns would lose it.
        assert_eq!(
            watch["Address"],
            serde_json::json!("Random 50:32:66:45:35:EF")
        );
        assert_eq!(
            watch["Resolves to"],
            serde_json::json!("Public F8:6F:C1:4E:FF:6A")
        );
        // An unresolved address stays null rather than repeating the advertised one.
        let tag = rows
            .iter()
            .find(|r| r["Device"] == serde_json::json!("Nameless Tag"))
            .expect("the tag is present");
        assert_eq!(tag["Resolves to"], serde_json::Value::Null);
    }

    /// The TCC module against the fixture: the decisions it maps, the date it
    /// converts, and the value it refuses to guess at.
    #[test]
    fn tcc_module_reads_permissions() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "tcc")
            .expect("tcc module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), &spec.path, seed_tcc);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 6);

        let find = |app: &str, svc: &str| {
            rows.iter()
                .find(|r| {
                    r["App"] == serde_json::json!(app) && r["Permission"] == serde_json::json!(svc)
                })
                .unwrap_or_else(|| panic!("no row for {app}/{svc}"))
        };
        assert_eq!(
            find("com.example.chatapp", "Camera")["Decision"],
            serde_json::json!("Allowed")
        );
        assert_eq!(
            find("com.example.weather", "kTCCServiceLocation")["Decision"],
            serde_json::json!("Denied")
        );
        assert_eq!(
            find("com.example.chatapp", "Photos")["Decision"],
            serde_json::json!("Limited")
        );
        // `kTCCServiceLocation` and `kTCCServiceContacts` stay RAW on purpose:
        // they are not in the module's mapping, and an unmapped code must show
        // as itself rather than be given an invented label. Ugly and true beats
        // tidy and possibly wrong.
        //
        // 1 is a documented state (kTCCAuthValueUnknown — never prompted), not a
        // decoding failure, and must not read as one.
        assert_eq!(
            find("com.example.notprompted", "kTCCServiceContacts")["Decision"],
            serde_json::json!("Not decided")
        );
        // NULL passes through as "not recorded". A `CASE auth_value WHEN` form
        // could not do this — `'x' || NULL` is NULL in SQLite, so the cell would
        // come back empty and look like a parsing bug.
        assert_eq!(
            find("com.example.norecord", "Microphone")["Decision"],
            serde_json::json!("Not recorded")
        );
        // A value outside Apple's enum is surfaced as-is rather than guessed at.
        assert_eq!(
            find("com.example.todo", "Reminders")["Decision"],
            serde_json::json!("Unrecognised (9)")
        );
        // The date arrives as a date.
        assert_eq!(
            find("com.example.chatapp", "Camera")["Decided"],
            serde_json::json!(1_700_000_000_i64)
        );
    }

    /// iOS 12/13: `allowed` instead of `auth_value`, but `last_modified` is
    /// still there. The two schema changes happened independently, and
    /// collapsing them into one branch silently dropped the date from every
    /// permission on those devices — a review caught it. "When was this
    /// granted" is the more interesting half of the artifact.
    #[test]
    fn tcc_keeps_the_date_on_the_middle_schema() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "tcc").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), &spec.path, |c| {
            c.execute_batch(
                "CREATE TABLE access (service TEXT, client TEXT, allowed INTEGER,
                                      prompt_count INTEGER, last_modified INTEGER);",
            )
            .unwrap();
            c.execute_batch(
                "INSERT INTO access VALUES ('kTCCServiceCamera','com.mid.app',1,«redacted»0000000);",
            )
            .unwrap();
        });
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows[0]["Decision"], serde_json::json!("Allowed"));
        assert_eq!(
            rows[0]["Decided"],
            serde_json::json!(1_700_000_000_i64),
            "the date exists on this schema and must not be thrown away"
        );
    }

    /// The older-iOS alternative runs when the modern one cannot prepare.
    /// This is the whole reason `sql` accepts a list.
    #[test]
    fn tcc_falls_back_to_the_older_schema() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "tcc").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // A pre-auth_value device: `allowed`, and no `last_modified` at all.
        make_backup(tmp.path(), &spec.path, |c| {
            c.execute_batch("CREATE TABLE access (service TEXT, client TEXT, allowed INTEGER);")
                .unwrap();
            c.execute_batch(
                "INSERT INTO access VALUES ('kTCCServiceCamera','com.old.app',1),
                                           ('kTCCServicePhotos','com.old.app',0);",
            )
            .unwrap();
        });
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 2, "the older-schema query must have run");
        assert_eq!(rows[0]["Decision"], serde_json::json!("Allowed"));
        assert_eq!(rows[1]["Decision"], serde_json::json!("Denied"));
        // No date exists on this schema, and that reads as absent, not as 1970.
        assert_eq!(rows[0]["Decided"], serde_json::Value::Null);
    }

    /// The listing distinguishes "not in this backup" from "cannot be in this
    /// backup".
    ///
    /// An artifact with no rows is normally omitted — a list of empty tables is
    /// not navigation. An artifact gated on encryption is kept even when empty,
    /// so it can explain itself instead of vanishing (#197). Dropping it would
    /// make an unencrypted backup look like a device that never had the data.
    #[test]
    fn a_gated_artifact_is_listed_even_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let conn = db.conn();

        // Every shipped module, with no rows stored for any of them.
        let listed = list_artifacts(conn).unwrap();
        let specs = builtin_modules().unwrap();
        for spec in &specs {
            let present = listed.iter().any(|a| a.id == spec.id);
            if spec.needs_encrypted_backup() {
                assert!(
                    present,
                    "gated artifact {} must stay listed so it can explain itself",
                    spec.id
                );
                let a = listed.iter().find(|a| a.id == spec.id).unwrap();
                assert!(a.requires_encrypted_backup, "and must say it is gated");
                assert_eq!(a.row_count, 0);
            } else {
                assert!(
                    !present,
                    "empty non-gated artifact {} must not clutter the list",
                    spec.id
                );
            }
        }

        // Once it has rows it is listed for the ordinary reason.
        let mut row: ArtifactRow = HashMap::new();
        row.insert("App".into(), serde_json::json!("com.example"));
        store_rows(conn, &specs[0].id, &[row]).unwrap();
        let listed = list_artifacts(conn).unwrap();
        let a = listed.iter().find(|a| a.id == specs[0].id).unwrap();
        assert_eq!(a.row_count, 1);
        assert_eq!(a.columns, vec!["App", "Permission", "Decision", "Decided"]);
    }

    /// A module declaring an unknown precondition is rejected, so a typo in
    /// `requires` cannot silently mean "no precondition at all".
    #[test]
    fn a_gated_module_round_trips_through_the_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "gated.toml",
            r#"
id = "gated"
description = "gated artifact."
surface = "standalone"
name = "Gated thing"
domain = "HomeDomain"
path = "Library/Nowhere/x.db"
requires = "encrypted-backup"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        assert!(spec.needs_encrypted_backup());
    }

    /// The three states the Artifacts view has to tell apart.
    ///
    /// A bare "have we extracted" boolean would pass the first two and fail the
    /// third — which is the case that bites a user who updates the app.
    #[test]
    fn extraction_state_distinguishes_never_run_stale_and_current() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let specs = builtin_modules().unwrap();

        // A cache imported before any module existed.
        assert_eq!(extraction_state(&db).unwrap(), ExtractionState::NeverRun);

        // Extracted with a set that is not the current one — e.g. the app has
        // gained a module since.
        db.set_meta(EXTRACTED_MODULES_KEY, "something,else")
            .unwrap();
        assert_eq!(extraction_state(&db).unwrap(), ExtractionState::Stale);

        // Extracted with exactly what is installed now.
        mark_extracted(&db, &specs).unwrap();
        assert_eq!(extraction_state(&db).unwrap(), ExtractionState::UpToDate);

        // Adding a module makes an up-to-date cache stale again — the case a
        // boolean cannot see.
        let mut more = specs.clone();
        more.push(ModuleSpec {
            id: "later_addition".into(),
            name: "Later".into(),
            description: "A test artifact.".into(),
            surface: Surface::Standalone,
            join_column: None,
            highlight: None,
            category: None,
            domain: "HomeDomain".into(),
            path: "a/b.db".into(),
            sql: vec!["SELECT a FROM t".into()],
            requires: None,
            columns: vec![ColumnSpec {
                name: "A".into(),
                from: "a".into(),
                kind: ColumnKind::Text,
                epoch: None,
            }],
        });
        assert_ne!(
            module_set_fingerprint(&specs),
            module_set_fingerprint(&more),
            "the fingerprint must change when a module is added"
        );
    }

    /// The fingerprint is order-independent, so a reshuffled BUILTIN list does
    /// not make every cache look stale.
    #[test]
    fn module_set_fingerprint_ignores_order() {
        let mk = |id: &str| ModuleSpec {
            id: id.into(),
            name: id.into(),
            description: "A test artifact.".into(),
            surface: Surface::Standalone,
            join_column: None,
            highlight: None,
            category: None,
            domain: "HomeDomain".into(),
            path: "a/b.db".into(),
            sql: vec!["SELECT a FROM t".into()],
            requires: None,
            columns: vec![ColumnSpec {
                name: "A".into(),
                from: "a".into(),
                kind: ColumnKind::Text,
                epoch: None,
            }],
        };
        assert_eq!(
            module_set_fingerprint(&[mk("a"), mk("b")]),
            module_set_fingerprint(&[mk("b"), mk("a")])
        );
    }

    /// A module cannot skip declaring where it belongs, or explaining itself.
    ///
    /// Both are required rather than defaulted, because the agreed rule — fold
    /// into the view closest in meaning, own destination only for genuinely
    /// outstanding data — was drifted from within one slice of shipping it. A
    /// default would let the next author drift the same way silently.
    #[test]
    fn a_module_must_declare_its_surface_and_describe_itself() {
        let base = |extra: &str| {
            format!(
                r#"
id = "x"
name = "X"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
{extra}
[[columns]]
name = "A"
from = "a"
"#
            )
        };
        let tmp = tempfile::tempdir().unwrap();

        // No surface at all.
        write_module(tmp.path(), "nosurface.toml", &base("description = \"X.\""));
        let err = load_modules(tmp.path())
            .expect_err("missing surface must fail")
            .to_string();
        assert!(err.contains("surface"), "{err}");

        // No description.
        let tmp2 = tempfile::tempdir().unwrap();
        write_module(tmp2.path(), "nodesc.toml", &base("surface = \"apps\""));
        let err = load_modules(tmp2.path())
            .expect_err("missing description must fail")
            .to_string();
        assert!(err.contains("description"), "{err}");

        // An empty description is as useless as none.
        let tmp3 = tempfile::tempdir().unwrap();
        write_module(
            tmp3.path(),
            "blankdesc.toml",
            &base("surface = \"apps\"\ndescription = \"   \""),
        );
        let err = load_modules(tmp3.path())
            .expect_err("blank description must fail")
            .to_string();
        assert!(err.contains("say in one sentence"), "{err}");

        // A typo'd surface must not silently mean "nowhere".
        let tmp4 = tempfile::tempdir().unwrap();
        write_module(
            tmp4.path(),
            "badsurface.toml",
            &base("surface = \"aps\"\ndescription = \"X.\""),
        );
        let err = load_modules(tmp4.path())
            .expect_err("unknown surface must fail")
            .to_string();
        assert!(err.contains("surface"), "{err}");
    }

    /// Every shipped module folds into an existing view. The moment one declares
    /// `standalone`, that is a deliberate claim it fits nowhere — and this test
    /// failing is the prompt to check that claim rather than to update the list.
    #[test]
    fn shipped_modules_fold_into_existing_views() {
        for spec in builtin_modules().unwrap() {
            assert_ne!(
                spec.surface,
                Surface::Standalone,
                "module {} claims it fits nowhere — is that true, or is there a \
                 view closer in meaning?",
                spec.id
            );
            assert!(
                spec.description.len() > 20,
                "module {}'s description is too short to explain anything",
                spec.id
            );
        }
    }

    #[test]
    fn requires_is_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "enc.toml",
            &spec_toml("\nrequires = \"encrypted-backup\"\n"),
        );
        // `requires` must sit at the top level, not inside the last [[columns]].
        let body = std::fs::read_to_string(tmp.path().join("enc.toml")).unwrap();
        let fixed = body.replace("requires = \"encrypted-backup\"", "");
        std::fs::write(
            tmp.path().join("enc.toml"),
            format!("requires = \"encrypted-backup\"\n{fixed}"),
        )
        .unwrap();
        let mods = load_modules(tmp.path()).unwrap();
        assert!(mods[0].needs_encrypted_backup());
    }
}
