//! Native Safari history parser (Phase 2): reads `History.db` directly into the
//! cache `safari_history` table, replacing iLEAPP's `safarihistory` step. Locate +
//! decrypt the DB via the [`crate::manifest::ManifestIndex`], then call
//! [`parse_safari`].
//!
//! `History.db` (at `HomeDomain/Library/Safari/History.db`) is a plain SQLite DB:
//! `history_items` holds the URL + total visit count, `history_visits` holds one
//! row per visit (with the page title and a Mac-absolute `visit_time`). We join
//! them to emit one cache row per visit, matching the iLEAPP output.
//!
//! provenance: reference (own implementation) from the reverse-engineered Safari
//! `History.db` schema.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Core Data / CFAbsoluteTime epoch (2001-01-01 UTC) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Parse a Safari `History.db` into the cache `safari_history` table (one row per
/// visit). With `replace = true` the table is cleared first, in the same
/// transaction as the re-insert, so a partial re-import is atomic.
pub fn parse_safari(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&src, "history_items")? || !table_exists(&src, "history_visits")? {
        return Err(crate::Error::Parse(
            "History.db is not a recognized Safari schema".into(),
        ));
    }

    // One row per visit: the item's URL + total visit_count, the visit's title +
    // time. `visit_time` is CFAbsoluteTime (seconds since 2001).
    let mut stmt = src.prepare(
        "SELECT i.url, v.title, v.visit_time, i.visit_count
         FROM history_visits v
         JOIN history_items i ON i.id = v.history_item
         WHERE i.url IS NOT NULL
         ORDER BY v.visit_time DESC",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM safari_history", [])?;
    }
    let mut inserted: usize = 0;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let url: String = r.get(0)?;
        let title: Option<String> = r
            .get::<_, Option<String>>(1)?
            .filter(|s| !s.trim().is_empty());
        let visited_at = r
            .get::<_, Option<f64>>(2)?
            .filter(|t| *t > 0.0)
            .map(|t| t as i64 + MAC_EPOCH);
        let visit_count: Option<i64> = r.get(3)?;
        tx.execute(
            "INSERT INTO safari_history (url, title, visited_at, visit_count)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![url, title, visited_at, visit_count],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    // Count only committed rows — a mid-loop error rolls back, adding nothing.
    report.safari_visits += inserted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_history_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("History.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
             CREATE TABLE history_visits (id INTEGER PRIMARY KEY, history_item INTEGER,
                 title TEXT, visit_time REAL);
             INSERT INTO history_items (id, url, visit_count) VALUES (1, 'https://example.com', 3);
             -- Two visits of the same item; Mac-time 721692800 = unix 1_700_000_000.
             INSERT INTO history_visits (id, history_item, title, visit_time)
                VALUES (10, 1, 'Example Domain', 721692800.0);
             INSERT INTO history_visits (id, history_item, title, visit_time)
                VALUES (11, 1, 'Example Domain', 721692500.0);
             -- An item with no url is ignored.
             INSERT INTO history_items (id, url, visit_count) VALUES (2, NULL, 1);
             INSERT INTO history_visits (id, history_item, title, visit_time)
                VALUES (12, 2, NULL, 721692400.0);",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_safari_visits() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_history_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        parse_safari(&db, &cache, &mut report, false).unwrap();
        assert_eq!(
            report.safari_visits, 2,
            "one row per visit, url-less item skipped"
        );

        let c = cache.conn();
        let (url, title, visited, count): (String, String, i64, i64) = c
            .query_row(
                "SELECT url, title, visited_at, visit_count
                 FROM safari_history ORDER BY visited_at DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(url, "https://example.com");
        assert_eq!(title, "Example Domain");
        assert_eq!(visited, 1_700_000_000);
        assert_eq!(count, 3);
    }
}
