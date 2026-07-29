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

use rusqlite::Connection;
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
        if !raw.is_finite() {
            return None;
        }
        let secs = match self {
            Epoch::Unix => raw,
            Epoch::UnixMs => raw / 1_000.0,
            Epoch::Cocoa => raw + 978_307_200.0,
            Epoch::Webkit => raw / 1_000_000.0 - 11_644_473_600.0,
        };
        // A backup cannot contain a date before iOS existed or far in the
        // future; treating those as "no date" is the rule app-data-coverage.md
        // already applies to messages whose timestamp does not decode.
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
    /// Relative path within the domain. A trailing `*` matches a prefix — which
    /// is all the globbing any artifact has needed so far; more can be added
    /// when one actually needs it.
    pub path: String,
    pub sql: String,
    /// Declared precondition. Parsed and stored here; honoured by the UI in a
    /// later slice (#210). Apple's `RelativePathsToOnlyBackupEncrypted` covers
    /// 28 artifacts, so this is not an edge case.
    #[serde(default)]
    pub requires: Option<String>,
    pub columns: Vec<ColumnSpec>,
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
        let sql = self.sql.trim();
        if sql.is_empty() {
            return Err("`sql` is empty".into());
        }
        // Read-only by construction: a module describes what to read out of a
        // backup, and nothing it declares should be able to modify anything.
        if !sql.to_ascii_lowercase().starts_with("select")
            && !sql.to_ascii_lowercase().starts_with("with")
        {
            return Err("`sql` must be a SELECT (or WITH … SELECT)".into());
        }
        if self.columns.is_empty() {
            return Err("no `[[columns]]` declared".into());
        }
        for c in &self.columns {
            if c.name.trim().is_empty() || c.from.trim().is_empty() {
                return Err(format!("column {:?} has an empty `name` or `from`", c.name));
            }
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
    let mut out = Vec::new();
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
        if let Some(prev) = out.iter().find(|m: &&ModuleSpec| m.id == spec.id) {
            return Err(Error::Parse(format!(
                "artifact module {}: id {:?} is already used by {:?} — ids key stored rows, \
                 so two modules sharing one would overwrite each other",
                path.file_name().unwrap_or_default().to_string_lossy(),
                spec.id,
                prev.name,
            )));
        }
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
    match spec.path.strip_suffix('*') {
        Some(prefix) => Ok(index.find_prefix(&spec.domain, prefix)?.into_iter().next()),
        None => index.find(&spec.domain, &spec.path),
    }
}

/// Run one module against a backup. `Ok(None)` means the backup does not
/// contain this artifact — which is a normal outcome, not a failure.
pub fn run_module(
    spec: &ModuleSpec,
    index: &ManifestIndex,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
) -> Result<Option<Vec<ArtifactRow>>> {
    let Some(entry) = locate(index, spec)? else {
        return Ok(None);
    };

    std::fs::create_dir_all(work_dir).map_err(|e| Error::Io {
        path: work_dir.to_path_buf(),
        source: e,
    })?;
    let dest = work_dir.join(format!("{}.sqlite", spec.id));
    index.extract_db(&entry, decryptor, &dest)?;

    let conn = Connection::open(&dest)?;
    let mut stmt = conn.prepare(&spec.sql)?;
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
            ("bad-toml", "id = \"x\"\nname =", "artifact module"),
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
