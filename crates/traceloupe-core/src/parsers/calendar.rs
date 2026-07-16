//! Native parser for `Calendar.sqlitedb` (Calendar events).
//!
//! provenance: reference (own implementation) — schema learned from a real
//! `HomeDomain/Library/Calendar/Calendar.sqlitedb`.
//!
//! Events live in `CalendarItem` (`entity_type = 2`); reminders (`entity_type
//! = 1`, `due_date`) are a different store and not read here. `start_date` /
//! `end_date` are Core Data time (seconds since 2001). Each item's containing
//! calendar name comes from `Calendar.title`, and its place from the `Location`
//! table via `location_id`.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Core Data epoch (2001-01-01) → Unix.
const MAC_EPOCH: i64 = 978_307_200;

fn to_unix(d: Option<f64>) -> Option<i64> {
    d.filter(|v| *v > 0.0)
        .map(|v| (v + MAC_EPOCH as f64) as i64)
}

/// Parse Calendar events into the cache `calendar_events` table. With `replace`,
/// clears existing events first (in the same transaction).
pub fn parse_calendar(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Not the schema we understand → nothing to do (best-effort artifact).
    let has_table: i64 = src.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='CalendarItem'",
        [],
        |r| r.get(0),
    )?;
    if has_table == 0 {
        return Ok(());
    }

    let mut stmt = src.prepare(
        "SELECT i.summary, i.description, i.start_date, i.end_date, i.all_day,
                c.title, i.url, loc.title, loc.address
         FROM CalendarItem i
         LEFT JOIN Calendar c ON c.ROWID = i.calendar_id
         LEFT JOIN Location loc ON loc.ROWID = i.location_id
         WHERE i.entity_type = 2 AND i.start_date IS NOT NULL
         ORDER BY i.start_date DESC",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM calendar_events", [])?;
    }
    let mut inserted = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let title: Option<String> = r.get(0)?;
        let notes: Option<String> = r.get(1)?;
        let start_at = to_unix(r.get::<_, Option<f64>>(2)?);
        let end_at = to_unix(r.get::<_, Option<f64>>(3)?);
        let all_day = r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0;
        let calendar_name: Option<String> = r.get(5)?;
        let url: Option<String> = r.get(6)?;
        // Prefer the location's title, falling back to its address.
        let location = r
            .get::<_, Option<String>>(7)?
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                r.get::<_, Option<String>>(8)
                    .ok()
                    .flatten()
                    .filter(|s| !s.trim().is_empty())
            });
        tx.execute(
            "INSERT INTO calendar_events
                (title, notes, location, start_at, end_at, all_day, calendar_name, url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                title,
                notes,
                location,
                start_at,
                end_at,
                all_day as i64,
                calendar_name,
                url
            ],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.calendar_events += inserted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_with_calendar_and_location() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("Calendar.sqlitedb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE CalendarItem (ROWID INTEGER PRIMARY KEY, summary TEXT, description TEXT,
                 start_date REAL, end_date REAL, all_day INTEGER, calendar_id INTEGER,
                 location_id INTEGER, url TEXT, entity_type INTEGER, due_date REAL);
             CREATE TABLE Calendar (ROWID INTEGER PRIMARY KEY, title TEXT);
             CREATE TABLE Location (ROWID INTEGER PRIMARY KEY, title TEXT, address TEXT);
             INSERT INTO Calendar VALUES (5, 'Work');
             INSERT INTO Location VALUES (9, 'HQ', '1 Main St');
             -- start 721692800 Mac = 1_700_000_000 unix.
             INSERT INTO CalendarItem VALUES (1,'Standup','daily sync',721692800.0,721694600.0,0,5,9,NULL,2,NULL);
             -- a location with no title falls back to its address.
             INSERT INTO Location VALUES (10, NULL, '2 Elm Ave');
             INSERT INTO CalendarItem VALUES (2,'Lunch',NULL,721700000.0,NULL,0,5,10,NULL,2,NULL);
             -- a reminder (entity_type 1) is ignored.
             INSERT INTO CalendarItem VALUES (3,'Buy milk',NULL,NULL,NULL,0,5,NULL,NULL,1,721800000.0);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_calendar(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.calendar_events, 2, "reminder excluded");

        let c = cache.conn();
        let (title, start, cal, loc): (String, i64, String, String) = c
            .query_row(
                "SELECT title, start_at, calendar_name, location FROM calendar_events WHERE title='Standup'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(title, "Standup");
        assert_eq!(start, 1_700_000_000);
        assert_eq!(cal, "Work");
        assert_eq!(loc, "HQ");

        let loc2: String = c
            .query_row(
                "SELECT location FROM calendar_events WHERE title='Lunch'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(loc2, "2 Elm Ave", "location falls back to address");
    }
}
