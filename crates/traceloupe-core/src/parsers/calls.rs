//! Native Calls parser (Phase 2): reads `CallHistory.storedata` directly into the
//! cache `calls` table, replacing iLEAPP's `callhistory` normalize step. Locate +
//! decrypt the DB via the [`crate::manifest::ManifestIndex`], then call
//! [`parse_calls`].
//!
//! `CallHistory.storedata` is a Core Data SQLite store at
//! `HomeDomain/Library/CallHistoryDB/CallHistory.storedata`; one row per call in
//! `ZCALLRECORD`. Columns carry the Core Data `Z`-prefix and are broadly stable
//! across modern iOS, but we still introspect the schema and fall back to `NULL`
//! for any optional column that's absent.
//!
//! provenance: reference (own implementation) from the reverse-engineered
//! CallHistory Core Data schema.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Core Data counts time in seconds since 2001-01-01 UTC; add this to reach Unix.
const MAC_EPOCH: i64 = 978_307_200;

/// The set of column names on `table`, for schema-tolerant SELECT building.
fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut cols = HashSet::new();
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        cols.insert(r.get::<_, String>(1)?);
    }
    Ok(cols)
}

/// `n.<col>` if it exists, else the literal `NULL` so the SQL still parses.
fn col_or_null(cols: &HashSet<String>, name: &str) -> String {
    if cols.contains(name) {
        format!("n.{name}")
    } else {
        "NULL".to_string()
    }
}

/// Map a call's Core Data service provider to our coarse `service` label.
fn classify_service(provider: Option<&str>) -> Option<String> {
    match provider {
        Some(p) if p.contains("FaceTime") => Some("facetime".to_string()),
        Some(p) if p.contains("Telephony") => Some("phone".to_string()),
        Some(p) if !p.trim().is_empty() => Some(p.to_lowercase()),
        _ => None,
    }
}

/// Parse a `CallHistory.storedata` into the cache `calls` table.
///
/// With `replace = true` the table is cleared first, in the same transaction as
/// the re-insert, so a partial re-import is atomic.
pub fn parse_calls(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let cols = table_columns(&src, "ZCALLRECORD")?;
    if cols.is_empty() {
        return Err(crate::Error::Parse(
            "CallHistory.storedata has no ZCALLRECORD table".into(),
        ));
    }

    let address = col_or_null(&cols, "ZADDRESS");
    let date = col_or_null(&cols, "ZDATE");
    let duration = col_or_null(&cols, "ZDURATION");
    let originated = col_or_null(&cols, "ZORIGINATED");
    let answered = col_or_null(&cols, "ZANSWERED");
    let provider = col_or_null(&cols, "ZSERVICE_PROVIDER");

    let sql = format!(
        "SELECT {address}, {date}, {duration}, {originated}, {answered}, {provider}
         FROM ZCALLRECORD n
         ORDER BY {date} DESC"
    );

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM calls", [])?;
    }
    let mut inserted: usize = 0;
    let mut stmt = src.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        // ZADDRESS is usually a BLOB of ASCII bytes but can be TEXT. rusqlite's
        // Vec<u8> reader rejects TEXT, so read the raw value and accept both.
        let address = match r.get_ref(0)? {
            rusqlite::types::ValueRef::Blob(b) | rusqlite::types::ValueRef::Text(b) => Some(
                String::from_utf8_lossy(b)
                    .trim_matches('\0')
                    .trim()
                    .to_string(),
            ),
            _ => None,
        }
        .filter(|s| !s.is_empty());
        let occurred_at = r
            .get::<_, Option<f64>>(1)?
            .filter(|d| *d > 0.0)
            .map(|d| d as i64 + MAC_EPOCH);
        let duration_s = r
            .get::<_, Option<f64>>(2)?
            .map(|d| d.max(0.0) as i64)
            .unwrap_or(0);
        let originated = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
        let direction = if originated != 0 {
            "outgoing"
        } else {
            "incoming"
        };
        let answered = r.get::<_, Option<i64>>(4)?.map(|a| (a != 0) as i64);
        let service = classify_service(r.get::<_, Option<String>>(5)?.as_deref());

        tx.execute(
            "INSERT INTO calls (address, direction, answered, duration_s, occurred_at, service)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                address,
                direction,
                answered,
                duration_s,
                occurred_at,
                service
            ],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    // Count only committed rows — a mid-loop error rolls back, adding nothing.
    report.calls += inserted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call_store(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("CallHistory.storedata");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZCALLRECORD (
                Z_PK INTEGER PRIMARY KEY, ZADDRESS BLOB, ZDATE REAL, ZDURATION REAL,
                ZORIGINATED INTEGER, ZANSWERED INTEGER, ZSERVICE_PROVIDER TEXT);
             -- Outgoing answered phone call, 65s, at Mac-time 721692800 (unix 1_700_000_000).
             INSERT INTO ZCALLRECORD (Z_PK, ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER)
                VALUES (1, CAST('+15551234567' AS BLOB), 721692800.0, 65.4, 1, 1, 'com.apple.Telephony');
             -- Incoming missed FaceTime call.
             INSERT INTO ZCALLRECORD (Z_PK, ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER)
                VALUES (2, CAST('friend@icloud.com' AS BLOB), 721692700.0, 0.0, 0, 0, 'com.apple.FaceTime');",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_calls_from_storedata() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_call_store(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        parse_calls(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.calls, 2);

        let c = cache.conn();
        // Newest-first: the outgoing phone call.
        let (address, direction, answered, duration, occurred, service): (
            String,
            String,
            i64,
            i64,
            i64,
            String,
        ) = c
            .query_row(
                "SELECT address, direction, answered, duration_s, occurred_at, service
                 FROM calls ORDER BY occurred_at DESC LIMIT 1",
                [],
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
            )
            .unwrap();
        assert_eq!(address, "+15551234567");
        assert_eq!(direction, "outgoing");
        assert_eq!(answered, 1);
        assert_eq!(duration, 65); // truncated from 65.4
        assert_eq!(occurred, 1_700_000_000);
        assert_eq!(service, "phone");

        // The FaceTime row classified as facetime, incoming, unanswered.
        let (direction, service): (String, String) = c
            .query_row(
                "SELECT direction, service FROM calls ORDER BY occurred_at ASC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(direction, "incoming");
        assert_eq!(service, "facetime");
    }

    #[test]
    fn replace_clears_prior_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_call_store(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_calls(&db, &cache, &mut report, false).unwrap();
        parse_calls(&db, &cache, &mut report, true).unwrap();
        let n: i64 = cache
            .conn()
            .query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "replace should not accumulate duplicates");
    }
}
