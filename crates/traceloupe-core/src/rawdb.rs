//! Read an app's database as it is — no mapping, no parser in the way.
//!
//! Every parser in this codebase is an opinion about a schema, and this session
//! showed how badly that fails without saying so: WhatsApp imported *nothing*
//! for months because one column was read off the wrong table, and the unit
//! fixture agreed with the bug. Nothing in the app could have revealed it — an
//! app with no messages looks exactly like a device with no messages.
//!
//! This module is the fallback that makes such a thing visible. It lists the
//! tables in a database, counts them, and hands back rows exactly as SQLite
//! stores them, so "is the data actually there, and does our reading of it
//! match?" can be answered by looking rather than by rebuilding the app.
//!
//! Read-only throughout. The backup is never modified.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

use crate::Result;

/// A database belonging to an app, as found in the backup.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawDatabase {
    pub domain: String,
    pub relative_path: String,
    /// Basename, for a short label.
    pub name: String,
}

/// Every SQLite database in the backup belonging to `bundle_id`.
///
/// An app's files are spread over more than one domain — `AppDomain-<bundle>` for
/// its own container and `AppDomainGroup-group.<...>` for anything it shares with
/// its extensions — and the interesting store is as often in the group as in the
/// app. Matching on the bundle id appearing anywhere in the domain catches both,
/// which is how WhatsApp's ChatStorage (a group container) is found at all.
///
/// Extension matching is deliberately loose: apps ship SQLite under `.sqlite`,
/// `.db`, `.sqlitedb`, `.data`, `.storedata` and no extension at all, so the
/// file's own header is the only reliable test — but that needs the bytes, and
/// this runs over the Manifest. Candidates are listed; a file that turns out not
/// to be a database simply fails to open when asked for its tables.
pub fn databases_for_app(
    index: &crate::manifest::ManifestIndex,
    bundle_id: &str,
) -> Result<Vec<RawDatabase>> {
    const DB_EXTS: [&str; 6] = [
        ".sqlite",
        ".db",
        ".sqlitedb",
        ".data",
        ".storedata",
        ".sqlite3",
    ];
    let needle = bundle_id.to_lowercase();
    let mut out = Vec::new();
    index.for_each_path(|domain, path| {
        if !domain.to_lowercase().contains(&needle) {
            return;
        }
        let lower = path.to_lowercase();
        // -wal and -shm are the same database's journal, not separate stores.
        if lower.ends_with("-wal") || lower.ends_with("-shm") {
            return;
        }
        if !DB_EXTS.iter().any(|e| lower.ends_with(e)) {
            return;
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        out.push(RawDatabase {
            domain,
            relative_path: path,
            name,
        });
    })?;
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(out)
}

/// One table in a database, and how much is in it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTable {
    pub name: String,
    pub rows: i64,
    pub columns: Vec<String>,
}

/// A page of rows, with the values rendered for display.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawRows {
    pub columns: Vec<String>,
    /// Row-major cells, already stringified — see [`RawCell`].
    pub rows: Vec<Vec<RawCell>>,
    /// Total rows matching the query, for the pager.
    pub total: i64,
}

/// One cell, described rather than dumped.
///
/// A BLOB is the awkward case: printing it is useless and printing nothing hides
/// it. So a blob reports its size and, when the bytes carry an image signature,
/// what it actually is — which is how Threema's photos turned out to be sitting
/// in plain sight inside the database.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCell {
    /// What to show. Never the raw bytes of a blob.
    pub text: String,
    /// `"null" | "text" | "integer" | "real" | "blob"`.
    pub kind: &'static str,
    /// For a timestamp-looking integer/real, the Unix seconds it decodes to —
    /// shown *beside* the raw value, never instead of it. A raw view is for
    /// reading what is stored; the decode is for understanding it, and both
    /// matter. Formatted by the UI, so it obeys the app's date rules (which
    /// include always showing the year).
    pub decoded_unix: Option<i64>,
}

/// How many rows one page may return, whatever the caller asks for.
const MAX_PAGE: i64 = 500;

/// Open a database read-only, tolerating a missing WAL.
fn open(db: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

/// Every user table in `db`, with row counts and column names.
///
/// SQLite's own bookkeeping is left out: `sqlite_sequence` and friends say
/// nothing about the device.
pub fn tables(db: &Path) -> Result<Vec<RawTable>> {
    let conn = open(db)?;
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
          WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
          ORDER BY name",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(std::result::Result::ok)
        .collect();

    let mut out = Vec::new();
    for name in names {
        let columns = columns_of(&conn, &name);
        // A table that cannot be counted (corrupt page, odd virtual table) is
        // still worth listing — reporting -1 says "here, but unreadable", which
        // is a fact, where omitting it would be a silent gap.
        let rows = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{name}\""), [], |r| {
                r.get(0)
            })
            .unwrap_or(-1);
        out.push(RawTable {
            name,
            rows,
            columns,
        });
    }
    Ok(out)
}

fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
        return Vec::new();
    };
    rows.filter_map(std::result::Result::ok).collect()
}

/// A page of `table`, optionally filtered by `search`.
///
/// `search` matches any column, compared as text — which is what someone
/// scanning an unfamiliar schema actually wants, rather than having to know
/// which column holds the thing they remember.
pub fn rows(
    db: &Path,
    table: &str,
    offset: i64,
    limit: i64,
    search: Option<&str>,
) -> Result<RawRows> {
    let conn = open(db)?;
    // The table name comes from `tables()`, but it is still interpolated into
    // SQL, so confirm it is one this database really has before using it.
    if !table_exists(&conn, table)? {
        return Err(crate::Error::Parse(format!("no table named {table}")));
    }
    let columns = columns_of(&conn, table);
    if columns.is_empty() {
        return Ok(RawRows {
            columns,
            rows: Vec::new(),
            total: 0,
        });
    }

    let (where_sql, params): (String, Vec<String>) =
        match search.map(str::trim).filter(|s| !s.is_empty()) {
            Some(q) => {
                let clause = columns
                    .iter()
                    .map(|c| format!("CAST(\"{c}\" AS TEXT) LIKE ?1 ESCAPE '\\'"))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                (
                    format!("WHERE {clause}"),
                    vec![format!("%{}%", escape_like(q))],
                )
            }
            None => (String::new(), Vec::new()),
        };

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{table}\" {where_sql}"),
            rusqlite::params_from_iter(params.iter()),
            |r| r.get(0),
        )
        .unwrap_or(0);

    let limit = limit.clamp(1, MAX_PAGE);
    let sel = columns
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT {sel} FROM \"{table}\" {where_sql} LIMIT {limit} OFFSET {}",
        offset.max(0)
    ))?;
    let mut q = stmt.query(rusqlite::params_from_iter(params.iter()))?;

    let mut out_rows = Vec::new();
    while let Some(r) = q.next()? {
        let mut cells = Vec::with_capacity(columns.len());
        for (i, name) in columns.iter().enumerate() {
            cells.push(cell(r, i, name));
        }
        out_rows.push(cells);
    }
    Ok(RawRows {
        columns,
        rows: out_rows,
        total,
    })
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn cell(r: &rusqlite::Row, i: usize, column: &str) -> RawCell {
    match r.get_ref(i) {
        Ok(ValueRef::Null) => RawCell {
            text: String::new(),
            kind: "null",
            decoded_unix: None,
        },
        Ok(ValueRef::Integer(n)) => RawCell {
            text: n.to_string(),
            kind: "integer",
            decoded_unix: decode_time(n as f64, column),
        },
        Ok(ValueRef::Real(f)) => RawCell {
            text: format!("{f}"),
            kind: "real",
            decoded_unix: decode_time(f, column),
        },
        Ok(ValueRef::Text(b)) => RawCell {
            text: String::from_utf8_lossy(b).into_owned(),
            kind: "text",
            decoded_unix: None,
        },
        Ok(ValueRef::Blob(b)) => {
            let what = crate::parsers::apps::discovery::media_magic(b)
                .map(|m| format!("{m} image"))
                .unwrap_or_else(|| "binary".to_string());
            RawCell {
                text: format!("<{} bytes, {what}>", b.len()),
                kind: "blob",
                decoded_unix: None,
            }
        }
        Err(_) => RawCell {
            text: String::new(),
            kind: "null",
            decoded_unix: None,
        },
    }
}

/// Core Data epoch (2001-01-01) in Unix seconds.
const COCOA_EPOCH: f64 = 978_307_200.0;
/// Plausible range for a real timestamp: 2001-09 to about 2035. Narrow on
/// purpose — a row id or a file size should not sprout a date beside it.
const MIN_UNIX: f64 = 1_000_000_000.0;
const MAX_UNIX: f64 = 2_051_222_400.0;

/// Unix seconds for a value that is probably a timestamp, or None.
///
/// Apple stores time three ways in the same database — Unix seconds, Core Data
/// seconds since 2001, and nanoseconds — so a raw column of large integers is
/// unreadable without help. The guess is deliberately conservative and always
/// shown *alongside* the stored value, never in place of it.
fn decode_time(v: f64, column: &str) -> Option<i64> {
    let looks_temporal = {
        let c = column.to_uppercase();
        ["DATE", "TIME", "TS", "CREATED", "MODIFIED", "SEEN", "SENT"]
            .iter()
            .any(|k| c.contains(k))
    };
    if !looks_temporal || v <= 0.0 {
        return None;
    }
    // Nanoseconds (imo, TikTok) -> seconds; then Unix or Core Data.
    let secs = if v > 1e17 {
        v / 1e9
    } else if v > 1e12 {
        v / 1e3
    } else {
        v
    };
    let unix = if (MIN_UNIX..=MAX_UNIX).contains(&secs) {
        secs
    } else if (MIN_UNIX..=MAX_UNIX).contains(&(secs + COCOA_EPOCH)) {
        secs + COCOA_EPOCH
    } else {
        return None;
    };
    Some(unix as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("raw.sqlite");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            // AUTOINCREMENT makes SQLite create `sqlite_sequence` for us, which
            // is the bookkeeping table the listing must leave out.
            "CREATE TABLE ZMESSAGE (Z_PK INTEGER PRIMARY KEY AUTOINCREMENT, ZTEXT TEXT,
                 ZDATE REAL, ZBLOB BLOB);
             INSERT INTO ZMESSAGE (Z_PK, ZTEXT, ZDATE) VALUES (1, 'hello there', 721692800.0);
             INSERT INTO ZMESSAGE (Z_PK, ZTEXT, ZDATE) VALUES (2, 'goodbye', 721692900.0);
             INSERT INTO ZMESSAGE (Z_PK, ZTEXT) VALUES (3, 'about photo.jpg');",
        )
        .unwrap();
        c.execute(
            "INSERT INTO ZMESSAGE (Z_PK, ZBLOB) VALUES (4, ?1)",
            rusqlite::params![vec![
                0x01u8, 0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, 74, 70, 73, 70, 0, 1
            ]],
        )
        .unwrap();
        p
    }

    #[test]
    fn lists_user_tables_with_counts_and_skips_sqlite_internals() {
        let dir = tempfile::tempdir().unwrap();
        let t = tables(&db(dir.path())).unwrap();
        assert_eq!(t.len(), 1, "sqlite_* is bookkeeping, not device data");
        assert_eq!(t[0].name, "ZMESSAGE");
        assert_eq!(t[0].rows, 4);
        assert_eq!(t[0].columns, vec!["Z_PK", "ZTEXT", "ZDATE", "ZBLOB"]);
    }

    /// Search has to work without knowing which column holds the thing — that is
    /// the whole point of looking at an unfamiliar schema.
    #[test]
    fn search_matches_any_column() {
        let dir = tempfile::tempdir().unwrap();
        let p = db(dir.path());
        let r = rows(&p, "ZMESSAGE", 0, 100, Some("goodbye")).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.rows[0][1].text, "goodbye");

        // A numeric column is searched as text too.
        assert_eq!(
            rows(&p, "ZMESSAGE", 0, 100, Some("721692800"))
                .unwrap()
                .total,
            1
        );
    }

    /// A `%` typed into the search box is a character, not "match everything".
    #[test]
    fn search_wildcards_are_literal() {
        let dir = tempfile::tempdir().unwrap();
        let r = rows(&db(dir.path()), "ZMESSAGE", 0, 100, Some("%")).unwrap();
        assert_eq!(r.total, 0);
    }

    /// Dumping a blob is useless and hiding it is worse. Say what it is —
    /// this is how photos stored inside a database become visible at all.
    #[test]
    fn a_blob_reports_its_size_and_that_it_is_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let r = rows(&db(dir.path()), "ZMESSAGE", 0, 100, None).unwrap();
        let blob = &r.rows[3][3];
        assert_eq!(blob.kind, "blob");
        assert!(blob.text.contains("jpeg image"), "got {}", blob.text);
        assert!(blob.text.contains("13 bytes"), "got {}", blob.text);
    }

    /// The stored value is what a raw view is for; the date is an aid beside it.
    #[test]
    fn a_timestamp_column_is_decoded_alongside_the_raw_value() {
        let dir = tempfile::tempdir().unwrap();
        let r = rows(&db(dir.path()), "ZMESSAGE", 0, 100, None).unwrap();
        let date = &r.rows[0][2];
        assert_eq!(date.text, "721692800");
        // 721692800 Core Data seconds = 1700000000 Unix.
        assert_eq!(date.decoded_unix, Some(1_700_000_000));
        // A primary key is not a date, however large it gets.
        assert_eq!(r.rows[0][0].decoded_unix, None);
    }

    #[test]
    fn paging_is_bounded_and_reports_the_true_total() {
        let dir = tempfile::tempdir().unwrap();
        let r = rows(&db(dir.path()), "ZMESSAGE", 1, 2, None).unwrap();
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.total, 4, "total counts the table, not the page");
        assert_eq!(r.rows[0][0].text, "2");
    }

    #[test]
    fn an_unknown_table_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(rows(&db(dir.path()), "ZMESSAGE; DROP TABLE x", 0, 10, None).is_err());
    }
}
