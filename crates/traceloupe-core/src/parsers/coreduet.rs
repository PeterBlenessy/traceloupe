//! Native parser for the CoreDuet interaction graph (`interactionC.db`).
//!
//! provenance: reference (own implementation) — schema learned from a real
//! `HomeDomain/Library/CoreDuet/People/interactionC.db`.
//!
//! `ZCONTACTS` is a *pre-aggregated* per-person communication summary that iOS
//! builds across apps (Messages, Mail, calls, FaceTime, …): incoming/outgoing
//! counts + first/last dates. We surface it as a "who you communicated with, how
//! much, and when" graph. Dates are Core Data time (seconds since 2001).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

const MAC_EPOCH: i64 = 978_307_200;

fn to_unix(d: Option<f64>) -> Option<i64> {
    d.filter(|v| *v > 0.0).map(|v| v as i64 + MAC_EPOCH)
}

/// Parse the CoreDuet contacts summary into the cache `interactions` table. With
/// `replace`, clears it first. Best-effort: an unrecognized schema is a no-op.
pub fn parse_interactions(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_table: i64 = src.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ZCONTACTS'",
        [],
        |r| r.get(0),
    )?;
    if has_table == 0 {
        return Ok(());
    }

    // SQLite's scalar MIN/MAX return NULL if *any* argument is NULL, so read the
    // three first-/last-date columns and reduce them in Rust (ignoring NULLs).
    let mut stmt = src.prepare(
        "SELECT ZDISPLAYNAME, ZIDENTIFIER,
                COALESCE(ZINCOMINGSENDERCOUNT, 0), COALESCE(ZOUTGOINGRECIPIENTCOUNT, 0),
                ZFIRSTINCOMINGSENDERDATE, ZFIRSTOUTGOINGRECIPIENTDATE, ZFIRSTINCOMINGRECIPIENTDATE,
                ZLASTINCOMINGSENDERDATE, ZLASTOUTGOINGRECIPIENTDATE, ZLASTINCOMINGRECIPIENTDATE
         FROM ZCONTACTS
         WHERE ZIDENTIFIER IS NOT NULL
         ORDER BY (COALESCE(ZINCOMINGSENDERCOUNT,0) + COALESCE(ZOUTGOINGRECIPIENTCOUNT,0)) DESC",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM interactions", [])?;
    }
    let mut inserted = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let display_name: Option<String> = r.get(0)?;
        let identifier: Option<String> = r.get(1)?;
        let incoming: i64 = r.get(2)?;
        let outgoing: i64 = r.get(3)?;
        // Earliest of the three first-interaction dates; latest of the last three.
        let first_at = [4, 5, 6]
            .into_iter()
            .filter_map(|i| to_unix(r.get::<_, Option<f64>>(i).ok().flatten()))
            .min();
        let last_at = [7, 8, 9]
            .into_iter()
            .filter_map(|i| to_unix(r.get::<_, Option<f64>>(i).ok().flatten()))
            .max();
        // Skip rows with no actual interactions recorded.
        if incoming == 0 && outgoing == 0 {
            continue;
        }
        tx.execute(
            "INSERT INTO interactions
                (display_name, identifier, incoming, outgoing, first_at, last_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![display_name, identifier, incoming, outgoing, first_at, last_at],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.interactions += inserted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_interaction_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("interactionC.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZCONTACTS (Z_PK INTEGER PRIMARY KEY, ZDISPLAYNAME TEXT, ZIDENTIFIER TEXT,
                 ZINCOMINGSENDERCOUNT INTEGER, ZOUTGOINGRECIPIENTCOUNT INTEGER,
                 ZFIRSTINCOMINGSENDERDATE REAL, ZFIRSTOUTGOINGRECIPIENTDATE REAL,
                 ZFIRSTINCOMINGRECIPIENTDATE REAL, ZLASTINCOMINGSENDERDATE REAL,
                 ZLASTOUTGOINGRECIPIENTDATE REAL, ZLASTINCOMINGRECIPIENTDATE REAL);
             -- first 721692800 Mac = 1_700_000_000 unix.
             INSERT INTO ZCONTACTS VALUES (1,'Robin','+15551234567',10,25,721692800.0,721692900.0,NULL,721700000.0,721710000.0,NULL);
             -- an identifier-only contact with interactions.
             INSERT INTO ZCONTACTS VALUES (2,NULL,'a@b.com',3,0,721695000.0,NULL,NULL,721696000.0,NULL,NULL);
             -- zero interactions → excluded.
             INSERT INTO ZCONTACTS VALUES (3,'Ghost','x@y.com',0,0,NULL,NULL,NULL,NULL,NULL,NULL);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_interactions(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.interactions, 2, "zero-interaction contact excluded");

        let c = cache.conn();
        let (name, inc, out, first): (String, i64, i64, i64) = c
            .query_row(
                "SELECT display_name, incoming, outgoing, first_at FROM interactions WHERE identifier='+15551234567'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(name, "Robin");
        assert_eq!(inc, 10);
        assert_eq!(out, 25);
        assert_eq!(first, 1_700_000_000);
    }
}
