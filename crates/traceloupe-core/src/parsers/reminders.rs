//! Native parser for the Reminders store (`ZREMCDREMINDER`).
//!
//! provenance: reference (own implementation) — schema learned from a real
//! `AppDomainGroup-group.com.apple.reminders/Container_v1/Stores/Data-*.sqlite`.
//!
//! Each reminder carries a title/notes, an optional due date, completion state +
//! date, a flag, and a priority; its list name is `ZREMCDBASELIST.ZNAME` via
//! `ZLIST`. Dates are Core Data time (seconds since 2001). Trashed reminders
//! (`ZMARKEDFORDELETION = 1`) are excluded.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

const MAC_EPOCH: i64 = 978_307_200;

fn to_unix(d: Option<f64>) -> Option<i64> {
    d.filter(|v| *v > 0.0)
        .map(|v| (v + MAC_EPOCH as f64) as i64)
}

/// Parse reminders into the cache `reminders` table. With `replace`, clears
/// existing reminders first. A store without `ZREMCDREMINDER` is a no-op (the
/// reminders container has several stores; only one holds the data).
pub fn parse_reminders(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_table: i64 = src.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ZREMCDREMINDER'",
        [],
        |r| r.get(0),
    )?;
    if has_table == 0 {
        return Ok(());
    }

    let mut stmt = src.prepare(
        "SELECT r.ZTITLE, r.ZNOTES, l.ZNAME, r.ZDUEDATE, r.ZCOMPLETED,
                r.ZCOMPLETIONDATE, r.ZFLAGGED, r.ZPRIORITY, r.ZCREATIONDATE
         FROM ZREMCDREMINDER r
         LEFT JOIN ZREMCDBASELIST l ON l.Z_PK = r.ZLIST
         WHERE COALESCE(r.ZMARKEDFORDELETION, 0) = 0
         ORDER BY r.ZCOMPLETED, r.ZDUEDATE",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM reminders", [])?;
    }
    let mut inserted = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let title: Option<String> = r.get(0)?;
        let notes: Option<String> = r.get(1)?;
        let list_name: Option<String> = r.get(2)?;
        let due_at = to_unix(r.get::<_, Option<f64>>(3)?);
        let completed = r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0;
        let completed_at = to_unix(r.get::<_, Option<f64>>(5)?);
        let flagged = r.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0;
        let priority: Option<i64> = r.get(7)?;
        let created_at = to_unix(r.get::<_, Option<f64>>(8)?);
        tx.execute(
            "INSERT INTO reminders
                (title, notes, list_name, due_at, completed, completed_at, flagged, priority, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                title,
                notes,
                list_name,
                due_at,
                completed as i64,
                completed_at,
                flagged as i64,
                priority,
                created_at
            ],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.reminders += inserted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reminders_with_list_and_completion() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("Data.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZREMCDREMINDER (Z_PK INTEGER PRIMARY KEY, ZTITLE TEXT, ZNOTES TEXT,
                 ZDUEDATE REAL, ZCOMPLETED INTEGER, ZCOMPLETIONDATE REAL, ZFLAGGED INTEGER,
                 ZPRIORITY INTEGER, ZCREATIONDATE REAL, ZLIST INTEGER, ZMARKEDFORDELETION INTEGER);
             CREATE TABLE ZREMCDBASELIST (Z_PK INTEGER PRIMARY KEY, ZNAME TEXT);
             INSERT INTO ZREMCDBASELIST VALUES (3, 'Groceries');
             -- due 721692800 Mac = 1_700_000_000 unix.
             INSERT INTO ZREMCDREMINDER VALUES (1,'Buy milk','2%',721692800.0,0,NULL,0,1,721600000.0,3,0);
             INSERT INTO ZREMCDREMINDER VALUES (2,'Call bank',NULL,NULL,1,721695000.0,0,0,721600000.0,3,0);
             -- trashed → excluded.
             INSERT INTO ZREMCDREMINDER VALUES (3,'old',NULL,NULL,0,NULL,0,0,NULL,3,1);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_reminders(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.reminders, 2, "trashed reminder excluded");

        let c = cache.conn();
        let (title, list, due, done): (String, String, i64, i64) = c
            .query_row(
                "SELECT title, list_name, due_at, completed FROM reminders WHERE title='Buy milk'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(title, "Buy milk");
        assert_eq!(list, "Groceries");
        assert_eq!(due, 1_700_000_000);
        assert_eq!(done, 0);

        let (done2, cat): (i64, Option<i64>) = c
            .query_row(
                "SELECT completed, completed_at FROM reminders WHERE title='Call bank'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(done2, 1);
        assert!(cat.is_some());
    }
}
