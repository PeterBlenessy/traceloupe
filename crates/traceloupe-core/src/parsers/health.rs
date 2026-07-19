//! Native parser for Apple Health (`healthdb_secure.sqlite`).
//!
//! provenance: reference (own implementation) — schema learned from a real
//! `HealthDomain/Health/healthdb_secure.sqlite`.
//!
//! Health stores hundreds of thousands of numeric `quantity_samples`, which are
//! noise to browse directly. We surface the digestible, high-value things: a
//! **workout** log (`workouts` ⋈ `samples` for dates ⋈ `workout_activities` for
//! type/duration), **daily activity aggregates** (steps / distance / energy /
//! flights summed per UTC day, heart rate min/avg/max — the `health_daily`
//! table) and a **summary** (total samples + date range) stored in the cache
//! `meta` table. Dates are Core Data time (seconds since 2001).

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

/// Cache `health_daily.metric` names — the single source of truth shared by
/// the writer (`parse_daily`) and the reader (`query::health_daily`). Using
/// these consts on both sides makes a rename a compile-time event instead of a
/// metric silently vanishing from the pivot.
pub mod metric {
    pub const HEART_RATE_BPM: &str = "heart_rate_bpm";
    pub const STEPS: &str = "steps";
    pub const DISTANCE_M: &str = "distance_m";
    pub const RESTING_KCAL: &str = "resting_kcal";
    pub const ACTIVE_KCAL: &str = "active_kcal";
    pub const FLIGHTS: &str = "flights";
}

/// `samples.data_type` codes for the quantity metrics we aggregate per day.
/// Verified against a real store: distance is metres, energy is kcal, heart
/// rate is count/sec (×60 → bpm). Code → cache `health_daily.metric` name.
fn metric_name(data_type: i64) -> Option<&'static str> {
    Some(match data_type {
        5 => metric::HEART_RATE_BPM,
        7 => metric::STEPS,
        8 => metric::DISTANCE_M,
        9 => metric::RESTING_KCAL,
        10 => metric::ACTIVE_KCAL,
        12 => metric::FLIGHTS,
        _ => return None,
    })
}

/// Parse Health workouts + daily aggregates + a summary into the cache. With
/// `replace`, clears the cache tables first. Best-effort: an unrecognized
/// schema is a no-op.
pub fn parse_health(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has = |table: &str| -> Result<bool> {
        Ok(src.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
            [table],
            |r| r.get::<_, i64>(0),
        )? > 0)
    };
    if !has("samples")? {
        return Ok(());
    }
    if has("workouts")? {
        parse_workouts(&src, cache, report, replace)?;
    }
    if has("quantity_samples")? {
        let has_provenance = has("objects")? && has("data_provenances")?;
        parse_daily(&src, cache, replace, has_provenance)?;
        summarize_samples(&src, cache)?;
    }
    if has("category_samples")? {
        parse_sleep(&src, cache, replace)?;
    }
    Ok(())
}

/// The workout log: one row per workout with activity/date/duration/distance.
fn parse_workouts(
    src: &Connection,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {

    // One row per workout: its dates (aggregated from `samples`) + activity
    // type/duration + total distance. A workout can have several
    // `workout_activities` rows (multi-sport, or all with a NULL primary flag), so
    // pick ONE deterministically — the explicitly-primary activity, else the
    // longest, else the lowest ROWID — via a correlated subquery. Joining on that
    // single row (rather than filtering `is_primary`) keeps the type/duration
    // stable instead of letting GROUP BY grab an arbitrary matching row. Sample
    // dates use MIN/MAX so multiple samples collapse to the true span.
    let mut stmt = src.prepare(
        "SELECT MIN(s.start_date), MAX(s.end_date), wa.activity_type, wa.duration, w.total_distance,
                w.data_id
         FROM workouts w
         JOIN samples s ON s.data_id = w.data_id
         LEFT JOIN workout_activities wa ON wa.ROWID = (
             SELECT wa2.ROWID FROM workout_activities wa2
             WHERE wa2.owner_id = w.data_id
             ORDER BY COALESCE(wa2.is_primary_activity, 0) DESC,
                      wa2.duration DESC, wa2.ROWID ASC
             LIMIT 1
         )
         GROUP BY w.data_id
         ORDER BY MIN(s.start_date) DESC",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM workouts", [])?;
    }
    let mut inserted = 0usize;
    // Source workout data_id → cache workouts.id, for attaching GPS routes.
    let mut id_map: Vec<(i64, i64)> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let start_at = to_unix(r.get::<_, Option<f64>>(0)?);
        let end_at = to_unix(r.get::<_, Option<f64>>(1)?);
        let activity = r.get::<_, Option<i64>>(2)?.map(activity_name);
        // Duration is stored in seconds; fall back to end − start.
        let duration_s =
            r.get::<_, Option<f64>>(3)?
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
        id_map.push((r.get(5)?, tx.last_insert_rowid()));
        inserted += 1;
    }
    tx.commit()?;
    report.workouts += inserted;
    parse_routes(src, cache, &id_map, replace)?;
    Ok(())
}

/// Cap on stored route points per workout — plenty for a preview polyline, and
/// keeps a heavy multi-hour GPS series (8k+ points here) from bloating the cache.
const MAX_ROUTE_POINTS: usize = 1000;

/// Workout GPS routes: each workout may have an associated location series
/// (`associations` → `data_series` → `location_series_data`). Stored
/// downsampled (every n-th point plus the last) in insertion order.
fn parse_routes(
    src: &Connection,
    cache: &CacheDb,
    id_map: &[(i64, i64)],
    replace: bool,
) -> Result<()> {
    // Clear BEFORE the table guards: on a replace import the old routes must
    // go even when this store has no route tables, or stale traces would
    // attach to the re-inserted workouts' reused rowids.
    if replace {
        cache.conn().execute("DELETE FROM workout_routes", [])?;
    }
    let has = |table: &str| -> Result<bool> {
        Ok(src.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
            [table],
            |r| r.get::<_, i64>(0),
        )? > 0)
    };
    if !(has("associations")? && has("data_series")? && has("location_series_data")?) {
        return Ok(());
    }
    // Skip tombstoned links when the schema has the flag (older ones may not).
    let has_deleted = src
        .prepare("PRAGMA table_info(associations)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .any(|c| c == "deleted");
    let sql = format!(
        "SELECT l.timestamp, l.latitude, l.longitude, l.altitude
         FROM associations a
         JOIN data_series ds ON ds.data_id = a.source_object_id
         JOIN location_series_data l ON l.series_identifier = ds.hfd_key
         WHERE a.destination_object_id = ?1{}
         ORDER BY l.timestamp",
        if has_deleted { " AND a.deleted = 0" } else { "" }
    );
    let mut stmt = src.prepare(&sql)?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    for &(src_id, cache_id) in id_map {
        let points: Vec<(Option<i64>, f64, f64, Option<f64>)> = stmt
            .query_map([src_id], |r| {
                Ok((
                    to_unix(r.get::<_, Option<f64>>(0)?),
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        if points.is_empty() {
            continue;
        }
        // Even stride down to the cap; always keep the final point so the
        // route ends where the workout did.
        let stride = points.len().div_ceil(MAX_ROUTE_POINTS);
        let last = points.len() - 1;
        let mut seq = 0i64;
        for (i, (at, lat, lon, alt)) in points.iter().enumerate() {
            if i % stride != 0 && i != last {
                continue;
            }
            tx.execute(
                "INSERT INTO workout_routes (workout_id, seq, at, latitude, longitude, altitude)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![cache_id, seq, at, lat, lon, alt],
            )?;
            seq += 1;
        }
    }
    tx.commit()?;
    Ok(())
}

/// One source's per-day stats for a metric, before cross-source merging.
struct SourceStats {
    sum: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    avg: Option<f64>,
    n: i64,
}

/// Daily activity aggregates: steps / distance / energy / flights (per UTC
/// day) and heart rate (min/avg/max, ×60 → bpm). One `health_daily` row per
/// (day, metric) — a few thousand rows instead of 344k raw samples.
///
/// A phone and a watch both record steps/distance/energy for the same walk, so
/// naively summing every sample double-counts multi-device days. When the
/// provenance tables exist we aggregate per (day, metric, `source_id`) and
/// keep the **largest source's total** for cumulative metrics — never
/// double-counted, at worst a slight undercount when devices trade off within
/// a day (HealthKit's own priority-merge is not reproducible offline). Heart
/// rate merges across sources instead: min of mins, max of maxes,
/// sample-weighted mean.
fn parse_daily(
    src: &Connection,
    cache: &CacheDb,
    replace: bool,
    has_provenance: bool,
) -> Result<()> {
    // Without the provenance tables (old schema, minimal fixtures) everything
    // lands in one group per (day, type) and the merge below degrades to a
    // plain SUM. (A literal in GROUP BY would be read as a column index, so
    // the source term is only present when it's a real column.)
    let (provenance_join, group_by) = if has_provenance {
        (
            "JOIN objects o ON o.data_id = s.data_id
             JOIN data_provenances dp ON dp.ROWID = o.provenance",
            "day, s.data_type, dp.source_id",
        )
    } else {
        ("", "day, s.data_type")
    };
    let mut stmt = src.prepare(&format!(
        "SELECT date(s.start_date + 978307200, 'unixepoch') AS day, s.data_type,
                SUM(q.quantity), MIN(q.quantity), MAX(q.quantity), AVG(q.quantity), COUNT(*)
         FROM samples s
         JOIN quantity_samples q ON q.data_id = s.data_id
         {provenance_join}
         WHERE s.data_type IN (5, 7, 8, 9, 10, 12) AND s.start_date > 0
         GROUP BY {group_by}
         ORDER BY day, s.data_type",
    ))?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM health_daily", [])?;
    }

    // Rows arrive grouped by (day, data_type); collect each group's per-source
    // stats, merge, write one row.
    let mut current: Option<(String, i64, Vec<SourceStats>)> = None;
    let mut flush = |tx: &rusqlite::Transaction,
                     day: &str,
                     data_type: i64,
                     sources: &[SourceStats]|
     -> Result<()> {
        let Some(metric) = metric_name(data_type) else {
            return Ok(());
        };
        let merged = if metric == "heart_rate_bpm" {
            merge_spread(sources)
        } else {
            merge_cumulative(sources)
        };
        // Heart rate is stored in canonical count/sec; scale every stat to bpm.
        let scale = if metric == "heart_rate_bpm" { 60.0 } else { 1.0 };
        let s = |v: Option<f64>| v.map(|v| v * scale);
        tx.execute(
            "INSERT OR REPLACE INTO health_daily
                 (day, metric, value_sum, value_min, value_max, value_avg, samples)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                day,
                metric,
                s(merged.sum),
                s(merged.min),
                s(merged.max),
                s(merged.avg),
                merged.n
            ],
        )?;
        Ok(())
    };
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let day: String = r.get(0)?;
        let data_type: i64 = r.get(1)?;
        let stats = SourceStats {
            sum: r.get(2)?,
            min: r.get(3)?,
            max: r.get(4)?,
            avg: r.get(5)?,
            n: r.get(6)?,
        };
        match &mut current {
            Some((d, t, sources)) if *d == day && *t == data_type => sources.push(stats),
            _ => {
                if let Some((d, t, sources)) = current.take() {
                    flush(&tx, &d, t, &sources)?;
                }
                current = Some((day, data_type, vec![stats]));
            }
        }
    }
    if let Some((d, t, sources)) = current.take() {
        flush(&tx, &d, t, &sources)?;
    }
    tx.commit()?;
    Ok(())
}

/// Cumulative metrics (steps/distance/energy/flights): keep the source with
/// the largest daily total — overlapping devices never double-count.
fn merge_cumulative(sources: &[SourceStats]) -> SourceStats {
    let best = sources
        .iter()
        .max_by(|a, b| {
            a.sum
                .unwrap_or(0.0)
                .total_cmp(&b.sum.unwrap_or(0.0))
        })
        .expect("flush is only called with at least one source");
    SourceStats {
        sum: best.sum,
        min: best.min,
        max: best.max,
        avg: best.avg,
        n: best.n,
    }
}

/// Spread metrics (heart rate): all sources' readings are real measurements,
/// so merge them — min of mins, max of maxes, sample-weighted mean.
fn merge_spread(sources: &[SourceStats]) -> SourceStats {
    let mut out = SourceStats {
        sum: None,
        min: None,
        max: None,
        avg: None,
        n: 0,
    };
    let mut weighted = 0.0f64;
    for s in sources {
        out.sum = match (out.sum, s.sum) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
        out.min = match (out.min, s.min) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        out.max = match (out.max, s.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        if let Some(avg) = s.avg {
            weighted += avg * s.n as f64;
        }
        out.n += s.n;
    }
    if out.n > 0 {
        out.avg = Some(weighted / out.n as f64);
    }
    out
}

/// `HKCategoryValueSleepAnalysis` → friendly stage name. Older iOS only writes
/// 0/1; watch-tracked sleep adds the 2–5 stages.
fn sleep_stage(value: i64) -> &'static str {
    match value {
        1 => "Asleep",
        2 => "Awake",
        3 => "Core",
        4 => "Deep",
        5 => "REM",
        _ => "In Bed",
    }
}

/// Sleep-analysis sessions: `category_samples` of data type 63, one cache row
/// per sample with its stage name.
fn parse_sleep(src: &Connection, cache: &CacheDb, replace: bool) -> Result<()> {
    let mut stmt = src.prepare(
        "SELECT s.start_date, s.end_date, c.value
         FROM samples s
         JOIN category_samples c ON c.data_id = s.data_id
         WHERE s.data_type = 63
         ORDER BY s.start_date",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM sleep_sessions", [])?;
    }
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let start_at = to_unix(r.get::<_, Option<f64>>(0)?);
        let end_at = to_unix(r.get::<_, Option<f64>>(1)?);
        let stage = sleep_stage(r.get::<_, Option<i64>>(2)?.unwrap_or(0));
        tx.execute(
            "INSERT INTO sleep_sessions (start_at, end_at, stage) VALUES (?1, ?2, ?3)",
            rusqlite::params![start_at, end_at, stage],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Summary of the raw sample volume, for the Health view header — stored in
/// `meta` so the UI can show scale without materializing 344k rows.
fn summarize_samples(src: &Connection, cache: &CacheDb) -> Result<()> {
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
    fn to_unix_saturates_on_absurd_date_without_overflow() {
        // A corrupt/adversarial Core Data date (~1e19) must not panic (dev builds
        // keep overflow checks) nor wrap to a negative garbage time. Adding in f64
        // before the cast makes the i64 conversion saturate cleanly. This guards
        // the shared timestamp-conversion pattern used across every parser.
        let huge = to_unix(Some(1e19)).unwrap();
        assert!(
            huge > 0,
            "saturates to a large positive i64, not a wrapped negative"
        );
        assert_eq!(to_unix(Some(f64::MAX)), Some(i64::MAX));
        assert_eq!(to_unix(Some(0.0)), None);
        assert_eq!(to_unix(Some(-5.0)), None);
    }

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
        assert_eq!(
            cache.get_meta("health_sample_count").unwrap().as_deref(),
            Some("2")
        );
        assert!(cache.get_meta("health_first_at").unwrap().is_some());
    }

    #[test]
    fn aggregates_daily_metrics_per_utc_day() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE quantity_samples (data_id INTEGER, quantity REAL);
             -- 721692800 Mac = 1_700_000_000 unix = 2023-11-14 22:13 UTC.
             -- Two step samples the same UTC day → one summed row.
             INSERT INTO samples VALUES (1, 721692800.0, 721692860.0, 7);
             INSERT INTO samples VALUES (2, 721693000.0, 721693060.0, 7);
             INSERT INTO quantity_samples VALUES (1, 120.0);
             INSERT INTO quantity_samples VALUES (2, 80.0);
             -- A step sample the next UTC day → its own row.
             INSERT INTO samples VALUES (3, 721779200.0, 721779260.0, 7);
             INSERT INTO quantity_samples VALUES (3, 50.0);
             -- Heart rate is canonical count/sec: 1.5/s → 90 bpm, 2.0/s → 120 bpm.
             INSERT INTO samples VALUES (4, 721692900.0, 721692901.0, 5);
             INSERT INTO samples VALUES (5, 721693100.0, 721693101.0, 5);
             INSERT INTO quantity_samples VALUES (4, 1.5);
             INSERT INTO quantity_samples VALUES (5, 2.0);
             -- An unsurfaced data type must be ignored.
             INSERT INTO samples VALUES (6, 721692900.0, 721692901.0, 999);
             INSERT INTO quantity_samples VALUES (6, 42.0);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, false).unwrap();

        let c = cache.conn();
        let rows: Vec<(String, String, f64, i64)> = c
            .prepare("SELECT day, metric, value_sum, samples FROM health_daily ORDER BY day, metric")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![
                ("2023-11-14".into(), "heart_rate_bpm".into(), 210.0, 2),
                ("2023-11-14".into(), "steps".into(), 200.0, 2),
                ("2023-11-15".into(), "steps".into(), 50.0, 1),
            ]
        );
        // Heart-rate min/max scaled to bpm.
        let (min, max): (f64, f64) = c
            .query_row(
                "SELECT value_min, value_max FROM health_daily WHERE metric='heart_rate_bpm'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((min, max), (90.0, 120.0));
    }

    #[test]
    fn multi_source_days_do_not_double_count() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE quantity_samples (data_id INTEGER, quantity REAL);
             CREATE TABLE objects (data_id INTEGER PRIMARY KEY, provenance INTEGER);
             CREATE TABLE data_provenances (ROWID INTEGER PRIMARY KEY, source_id INTEGER);
             INSERT INTO data_provenances VALUES (1, 100), (2, 200);
             -- Phone (source 100) and watch (source 200) both record the same
             -- walk on one UTC day: 200 vs 150 steps. The day must report the
             -- larger source (200), not the 350 double-count.
             INSERT INTO samples VALUES (1, 721692800.0, 721692860.0, 7);
             INSERT INTO samples VALUES (2, 721693000.0, 721693060.0, 7);
             INSERT INTO samples VALUES (3, 721692900.0, 721692960.0, 7);
             INSERT INTO quantity_samples VALUES (1, 120.0);
             INSERT INTO quantity_samples VALUES (2, 80.0);
             INSERT INTO quantity_samples VALUES (3, 150.0);
             INSERT INTO objects VALUES (1, 1), (2, 1), (3, 2);
             -- Heart rate from both sources merges: min/max span both.
             INSERT INTO samples VALUES (4, 721692900.0, 721692901.0, 5);
             INSERT INTO samples VALUES (5, 721693100.0, 721693101.0, 5);
             INSERT INTO quantity_samples VALUES (4, 1.0);  -- 60 bpm, phone
             INSERT INTO quantity_samples VALUES (5, 2.0);  -- 120 bpm, watch
             INSERT INTO objects VALUES (4, 1), (5, 2);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, false).unwrap();

        let c = cache.conn();
        let steps: f64 = c
            .query_row(
                "SELECT value_sum FROM health_daily WHERE metric='steps'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(steps, 200.0, "largest source wins, no 350 double-count");
        let (hr_min, hr_max, hr_n): (f64, f64, i64) = c
            .query_row(
                "SELECT value_min, value_max, samples FROM health_daily WHERE metric='heart_rate_bpm'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((hr_min, hr_max, hr_n), (60.0, 120.0, 2), "HR merges across sources");
    }

    #[test]
    fn replace_clears_stale_routes_even_without_route_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        // A store with workouts but NO route tables.
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE workouts (data_id INTEGER PRIMARY KEY, total_distance REAL);
             CREATE TABLE workout_activities (ROWID INTEGER PRIMARY KEY, owner_id INTEGER,
                 is_primary_activity INTEGER, activity_type INTEGER, duration REAL);
             INSERT INTO workouts VALUES (1, 0.0);
             INSERT INTO samples VALUES (1, 721692800.0, 721694600.0, 80);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        // Simulate a previous import's route for the workout rowid this one reuses.
        cache
            .conn()
            .execute(
                "INSERT INTO workout_routes (workout_id, seq, at, latitude, longitude, altitude)
                 VALUES (1, 0, 0, 56.0, 13.0, 0.0)",
                [],
            )
            .unwrap();

        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, true).unwrap();
        let n: i64 = cache
            .conn()
            .query_row("SELECT COUNT(*) FROM workout_routes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "replace must clear routes even when the store has no route tables");
    }

    #[test]
    fn parses_and_downsamples_workout_routes() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE workouts (data_id INTEGER PRIMARY KEY, total_distance REAL);
             CREATE TABLE workout_activities (ROWID INTEGER PRIMARY KEY, owner_id INTEGER,
                 is_primary_activity INTEGER, activity_type INTEGER, duration REAL);
             CREATE TABLE quantity_samples (data_id INTEGER, quantity REAL);
             CREATE TABLE associations (destination_object_id INTEGER, source_object_id INTEGER,
                 deleted INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE data_series (data_id INTEGER PRIMARY KEY, hfd_key INTEGER);
             CREATE TABLE location_series_data (series_identifier INTEGER, timestamp REAL,
                 latitude REAL, longitude REAL, altitude REAL);
             INSERT INTO workouts VALUES (10, 5000.0);
             INSERT INTO samples VALUES (10, 721692800.0, 721694600.0, 80);
             INSERT INTO workout_activities VALUES (1, 10, 1, 37, 1800.0);
             -- Route series 7 linked to the workout; a deleted link (series 8)
             -- must be ignored.
             INSERT INTO associations VALUES (10, 20, 0);
             INSERT INTO associations VALUES (10, 21, 1);
             INSERT INTO data_series VALUES (20, 7);
             INSERT INTO data_series VALUES (21, 8);
             INSERT INTO location_series_data VALUES (8, 721692800.0, 99.0, 99.0, 0.0);",
        )
        .unwrap();
        // 2500 points → stride 3 → ceil(2500/3)=834 kept (+ the last point).
        {
            let mut stmt = conn
                .prepare("INSERT INTO location_series_data VALUES (7, ?1, ?2, ?3, ?4)")
                .unwrap();
            for i in 0..2500 {
                stmt.execute(rusqlite::params![
                    721692800.0 + i as f64,
                    56.0 + i as f64 * 1e-5,
                    13.0,
                    20.0
                ])
                .unwrap();
            }
        }

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, false).unwrap();

        let c = cache.conn();
        let (n, wid): (i64, i64) = c
            .query_row(
                "SELECT COUNT(*), MIN(workout_id) FROM workout_routes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(n <= 1001, "downsampled to the cap (+last), got {n}");
        assert!(n >= 800, "kept a dense preview, got {n}");
        // Route rows attach to the cache workout id.
        let cache_wid: i64 = c.query_row("SELECT id FROM workouts", [], |r| r.get(0)).unwrap();
        assert_eq!(wid, cache_wid);
        // The deleted link's series (lat 99) must not appear; the final point is kept.
        let (max_lat, last_lat): (f64, f64) = c
            .query_row(
                "SELECT MAX(latitude),
                        (SELECT latitude FROM workout_routes ORDER BY seq DESC LIMIT 1)
                 FROM workout_routes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(max_lat < 57.0, "deleted association leaked in: {max_lat}");
        assert!((last_lat - (56.0 + 2499.0 * 1e-5)).abs() < 1e-9);
    }

    #[test]
    fn parses_sleep_sessions_with_stage_names() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE category_samples (data_id INTEGER, value INTEGER);
             -- An in-bed session (value 0) and a deep-sleep stage (value 4).
             INSERT INTO samples VALUES (1, 721692800.0, 721721600.0, 63);
             INSERT INTO samples VALUES (2, 721695000.0, 721698600.0, 63);
             INSERT INTO category_samples VALUES (1, 0);
             INSERT INTO category_samples VALUES (2, 4);
             -- A non-sleep category sample must be ignored.
             INSERT INTO samples VALUES (3, 721692800.0, 721692900.0, 95);
             INSERT INTO category_samples VALUES (3, 1);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, false).unwrap();

        let c = cache.conn();
        let rows: Vec<(i64, i64, String)> = c
            .prepare("SELECT start_at, end_at, stage FROM sleep_sessions ORDER BY start_at, id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![
                (1_700_000_000, 1_700_028_800, "In Bed".to_string()),
                (1_700_002_200, 1_700_005_800, "Deep".to_string()),
            ]
        );
    }

    #[test]
    fn multi_activity_workout_picks_deterministically() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("healthdb_secure.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (data_id INTEGER, start_date REAL, end_date REAL, data_type INTEGER);
             CREATE TABLE workouts (data_id INTEGER PRIMARY KEY, total_distance REAL);
             CREATE TABLE workout_activities (ROWID INTEGER PRIMARY KEY, owner_id INTEGER,
                 is_primary_activity INTEGER, activity_type INTEGER, duration REAL);
             CREATE TABLE quantity_samples (data_id INTEGER, quantity REAL);
             -- Workout 1: two activities, both with a NULL primary flag → the
             -- longest one (Running, 1800s) must win, not an arbitrary row.
             INSERT INTO workouts VALUES (1, 0.0);
             INSERT INTO samples VALUES (1, 721692800.0, 721694600.0, 80);
             INSERT INTO workout_activities VALUES (10, 1, NULL, 52, 600.0);
             INSERT INTO workout_activities VALUES (11, 1, NULL, 37, 1800.0);
             -- Workout 2: an explicit primary (Walking, 300s) must win over a
             -- longer non-primary activity (Running, 5000s).
             INSERT INTO workouts VALUES (2, 0.0);
             INSERT INTO samples VALUES (2, 721600000.0, 721601000.0, 80);
             INSERT INTO workout_activities VALUES (20, 2, 1, 52, 300.0);
             INSERT INTO workout_activities VALUES (21, 2, 0, 37, 5000.0);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_health(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.workouts, 2);

        let c = cache.conn();
        // Newest first (workout 1 starts later).
        let rows: Vec<(String, i64)> = c
            .prepare("SELECT activity, duration_s FROM workouts ORDER BY start_at DESC")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows[0], ("Running".to_string(), 1800)); // longest, NULL flags
        assert_eq!(rows[1], ("Walking".to_string(), 300)); // explicit primary wins
    }
}
