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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModuleSpec {
    /// Stable identifier; also the key rows are stored under.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
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
fn one_or_many<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
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
    let mut last_err = String::new();
    for query in &spec.sql {
        match conn.prepare(query) {
            Ok(s) => {
                prepared = Some(s);
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let mut stmt = prepared.ok_or_else(|| {
        Error::Parse(format!(
            "artifact {} ({}): no `sql` alternative could run against this backup ({} tried, last error: {last_err})",
            spec.id,
            spec.name,
            spec.sql.len(),
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
        ColumnKind::Integer => match raw {
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

/// The modules shipped with the crate.
///
/// Dev/test only: `CARGO_MANIFEST_DIR` is baked at compile time and points at
/// the machine that built the binary, so a shipped app must resolve modules
/// some other way. Wiring that up belongs with the app layer (#209), and until
/// then this exists so tests can run every real module.
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
            "INSERT INTO Files VALUES (?1, 'HomeDomain', ?2, 1, NULL)",
            rusqlite::params![file_id, rel],
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
                ('kTCCServiceReminders','com.example.todo',0,9,2,«redacted»0000500);",
        )
        .unwrap();
    }

    /// EVERY shipped module must load and run against the fixture.
    ///
    /// This is the guard that scales: it costs nothing per new artifact and
    /// fails the moment one rots — a renamed column, a typo in the SQL, a
    /// column spec that stops matching. Without it a broken module is only
    /// discovered by a user seeing an empty artifact.
    #[test]
    fn every_shipped_module_loads_and_runs() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        assert!(
            !mods.is_empty(),
            "no modules shipped — did the directory move?"
        );

        for spec in &mods {
            let tmp = tempfile::tempdir().unwrap();
            // Give each module the store it asks for, seeded for its id.
            match spec.id.as_str() {
                "tcc" => make_backup(tmp.path(), &spec.path, seed_tcc),
                other => panic!(
                    "module {other:?} has no fixture in this test — add one to \
                     seed_* and to tools/make_fixture_backup.py, so shipping a \
                     module always means shipping data that proves it runs"
                ),
            }
            let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
            let rows = run_module(spec, &index, None, &tmp.path().join("work"))
                .unwrap_or_else(|e| panic!("module {} failed to run: {e}", spec.id))
                .unwrap_or_else(|| panic!("module {} found nothing in its own fixture", spec.id));
            assert!(!rows.is_empty(), "module {} produced no rows", spec.id);
            // Every declared column is present on every row.
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
        assert_eq!(rows.len(), 4);

        let find = |app: &str, svc: &str| {
            rows.iter()
                .find(|r| {
                    r["App"] == serde_json::json!(app) && r["Permission"] == serde_json::json!(svc)
                })
                .unwrap_or_else(|| panic!("no row for {app}/{svc}"))
        };
        assert_eq!(
            find("com.example.chatapp", "kTCCServiceCamera")["Decision"],
            serde_json::json!("Allowed")
        );
        assert_eq!(
            find("com.example.weather", "kTCCServiceLocation")["Decision"],
            serde_json::json!("Denied")
        );
        assert_eq!(
            find("com.example.chatapp", "kTCCServicePhotos")["Decision"],
            serde_json::json!("Limited")
        );
        // An auth_value we have not verified is surfaced as-is rather than
        // guessed at — the audit's rule for undecodable values.
        assert_eq!(
            find("com.example.todo", "kTCCServiceReminders")["Decision"],
            serde_json::json!("Unknown (9)")
        );
        // The date arrives as a date.
        assert_eq!(
            find("com.example.chatapp", "kTCCServiceCamera")["Decided"],
            serde_json::json!(1_700_000_000_i64)
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
