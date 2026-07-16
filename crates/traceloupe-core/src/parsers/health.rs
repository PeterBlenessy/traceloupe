//! Native parser for Apple Health (`healthdb_secure.sqlite`).
//!
//! provenance: reference (own implementation) — schema learned from a real
//! `HealthDomain/Health/healthdb_secure.sqlite`.
//!
//! Health stores hundreds of thousands of numeric `quantity_samples`, which are
//! noise to browse directly. We surface the two digestible, high-value things: a
//! **workout** log (`workouts` ⋈ `samples` for dates ⋈ `workout_activities` for
//! type/duration) and a **summary** (total samples + date range) stored in the
//! cache `meta` table. Dates are Core Data time (seconds since 2001).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

const MAC_EPOCH: i64 = 978_307_200;

fn to_unix(d: Option<f64>) -> Option<i64> {
    d.filter(|v| *v > 0.0).map(|v| v as i64 + MAC_EPOCH)
}

/// `HKWorkoutActivityType` code → friendly name (the common subset; others fall
/// back to "Workout"). See Apple's HealthKit activity-type enumeration.
fn activity_name(code: i64) -> &'static str {
    match code {
        1 => "American Football",
        13 => "Cycling",
        16 => "Elliptical",
        20 => "Functional Strength",
        24 => "Hiking",
        35 => "Pilates",
        37 => "Running",
        44 => "Rowing",
        46 => "Strength Training",
        52 => "Walking",
        57 => "Swimming",
        63 => "Yoga",
        70 => "Core Training",
        71 => "Cross Training",
        79 => "High-Intensity Interval Training",
        _ => "Workout",
    }
}

/// Parse Health workouts + a summary into the cache. With `replace`, clears the
/// `workouts` table first. Best-effort: an unrecognized schema is a no-op.
pub fn parse_health(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_tables: i64 = src.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type='table' AND name IN ('workouts','samples')",
        [],
        |r| r.get(0),
    )?;
    if has_tables < 2 {
        return Ok(());
    }

    // One row per workout: its dates (from `samples`) + activity type/duration
    // (from the primary `workout_activities` row) + total distance.
    let mut stmt = src.prepare(
        "SELECT s.start_date, s.end_date, wa.activity_type, wa.duration, w.total_distance
         FROM workouts w
         JOIN samples s ON s.data_id = w.data_id
         LEFT JOIN workout_activities wa
                ON wa.owner_id = w.data_id
               AND COALESCE(wa.is_primary_activity, 1) = 1
         GROUP BY w.data_id
         ORDER BY s.start_date DESC",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM workouts", [])?;
    }
    let mut inserted = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let start_at = to_unix(r.get::<_, Option<f64>>(0)?);
        let end_at = to_unix(r.get::<_, Option<f64>>(1)?);
        let activity = r.get::<_, Option<i64>>(2)?.map(activity_name);
        // Duration is stored in seconds; fall back to end − start.
        let duration_s = r
            .get::<_, Option<f64>>(3)?
            .map(|d| d as i64)
            .or_else(|| match (start_at, end_at) {
                (Some(s), Some(e)) if e > s => Some(e - s),
                _ => None,
            });
        let distance_m: Option<f64> = r.get(4)?;
        tx.execute(
            "INSERT INTO workouts (activity, start_at, end_at, duration_s, distance_m)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![activity, start_at, end_at, duration_s, distance_m],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.workouts += inserted;

    // Summary of the raw sample volume, for the Health view header — stored in
    // `meta` so the UI can show scale without materializing 344k rows.
    if let Ok((count, first, last)) = src.query_row(
        "SELECT COUNT(*), MIN(start_date), MAX(start_date) FROM quantity_samples
         JOIN samples ON samples.data_id = quantity_samples.data_id",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, Option<f64>>(2)?,
            ))
        },
    ) {
        cache.set_meta("health_sample_count", &count.to_string())?;
        if let Some(f) = to_unix(first) {
            cache.set_meta("health_first_at", &f.to_string())?;
        }
        if let Some(l) = to_unix(last) {
            cache.set_meta("health_last_at", &l.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workouts_with_type_and_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE workouts (data_id INTEGER PRIMARY KEY, total_distance REAL);
             CREATE TABLE workout_activities (ROWID INTEGER PRIMARY KEY, owner_id INTEGER,
                 is_primary_activity INTEGER, activity_type INTEGER, duration REAL);
             CREATE TABLE quantity_samples (data_id INTEGER, quantity REAL);
             -- start 721692800 Mac = 1_700_000_000 unix; 30-min run, 5 km.
             INSERT INTO samples VALUES (1, 721692800.0, 721694600.0, 80);
             INSERT INTO workouts VALUES (1, 5000.0);
             INSERT INTO workout_activities VALUES (1, 1, 1, 37, 1800.0);
             -- two quantity samples for the summary.
             INSERT INTO samples VALUES (2, 721600000.0, 721600001.0, 5);
             INSERT INTO samples VALUES (3, 721700000.0, 721700001.0, 5);
             INSERT INTO quantity_samples VALUES (2, 100.0);
             INSERT INTO quantity_samples VALUES (3, 200.0);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.workouts, 1);

        let c = cache.conn();
        let (activity, start, dur, dist): (String, i64, i64, f64) = c
            .query_row(
                "SELECT activity, start_at, duration_s, distance_m FROM workouts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(activity, "Running");
        assert_eq!(start, 1_700_000_000);
        assert_eq!(dur, 1800);
        assert_eq!(dist, 5000.0);

        // Sample summary stored in meta.
        assert_eq!(cache.get_meta("health_sample_count").unwrap().as_deref(), Some("2"));
        assert!(cache.get_meta("health_first_at").unwrap().is_some());
    }
}
