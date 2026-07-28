//! The home dashboard's metrics (#157).
//!
//! One tile per kind of data the backup actually yielded: how much, over what
//! period, and a sparkline of when it clusters.
//!
//! **Everything is driven by [`METRIC_SOURCES`].** There is no per-module branch
//! anywhere in this file, in the command layer, or in the view — the response
//! carries each tile's label, route and icon as *data*, so the frontend renders
//! whatever arrives without knowing which modules exist. Adding a kind of data
//! is one row in that table and no frontend change at all.
//!
//! Full table introspection was rejected: a tile needs a label, a route and an
//! icon that do not exist in the schema, and the cache holds plenty of tables
//! that are not modules (`attachments`, `note_media`, `meta`, …). So the list is
//! declarative — and [`tests::every_content_table_is_accounted_for`] fails the
//! build when the schema grows past it, which is what makes "add a parser and it
//! appears" true rather than merely intended.

use rusqlite::{params, Connection};

use crate::analysis::TIMELINE_START;
use crate::error::Result;

/// How many buckets a sparkline holds.
///
/// A sparkline is a shape, not an axis: it has no labels, so it does not need
/// the calendar-aware bucketing the report's charts use (#66). Equal slices of
/// the span read the same and stay one query.
///
/// Sixteen rather than twenty-four so the bars can carry the app's 2px gap and
/// still be wide enough to see: at twenty-four they needed a 1px gap, which is
/// off the spacing grid the design lint enforces.
pub const SPARK_BUCKETS: usize = 16;

/// One tile's definition — the whole of what makes a kind of data appear on the
/// dashboard.
#[derive(Debug, Clone, Copy)]
pub struct MetricSource {
    pub id: &'static str,
    pub label: &'static str,
    /// Where clicking the tile goes. Sent to the UI, so a new module routes
    /// correctly without the frontend knowing it exists.
    pub route: &'static str,
    /// Icon name the UI resolves against its own map, falling back to a generic
    /// glyph — an unrecognised module still gets its label, link and numbers
    /// rather than being dropped.
    pub icon: &'static str,
    pub table: &'static str,
    /// The column to take the span and the sparkline from. `None` for data that
    /// is real but undated (contacts, installed apps): those tiles show a count
    /// and nothing else, rather than a fabricated timeline.
    pub time_column: Option<&'static str>,
}

/// Every kind of data that earns a tile. Extend this to extend the dashboard.
pub const METRIC_SOURCES: &[MetricSource] = &[
    MetricSource {
        id: "messages",
        label: "Messages",
        route: "/messages",
        icon: "messages",
        table: "messages",
        time_column: Some("sent_at"),
    },
    MetricSource {
        id: "photos",
        label: "Photos & videos",
        route: "/photos",
        icon: "photos",
        table: "media_items",
        time_column: Some("taken_at"),
    },
    MetricSource {
        id: "contacts",
        label: "Contacts",
        route: "/contacts",
        icon: "contacts",
        table: "contacts",
        time_column: None,
    },
    MetricSource {
        id: "calls",
        label: "Calls",
        route: "/calls",
        icon: "calls",
        table: "calls",
        time_column: Some("occurred_at"),
    },
    MetricSource {
        id: "safari",
        label: "Safari history",
        route: "/safari",
        icon: "safari",
        table: "safari_history",
        time_column: Some("visited_at"),
    },
    MetricSource {
        id: "notes",
        label: "Notes",
        route: "/notes",
        icon: "notes",
        table: "notes",
        time_column: Some("modified_at"),
    },
    MetricSource {
        id: "recordings",
        label: "Voice memos",
        route: "/recordings",
        icon: "recordings",
        table: "recordings",
        time_column: Some("recorded_at"),
    },
    MetricSource {
        id: "calendar",
        label: "Calendar",
        route: "/calendar",
        icon: "calendar",
        table: "calendar_events",
        time_column: Some("start_at"),
    },
    MetricSource {
        id: "reminders",
        label: "Reminders",
        route: "/reminders",
        icon: "reminders",
        table: "reminders",
        time_column: Some("created_at"),
    },
    MetricSource {
        id: "workouts",
        label: "Workouts",
        route: "/health",
        icon: "health",
        table: "workouts",
        time_column: Some("start_at"),
    },
    MetricSource {
        id: "interactions",
        label: "Interactions",
        route: "/interactions",
        icon: "interactions",
        table: "interactions",
        time_column: None,
    },
    MetricSource {
        id: "apps",
        label: "Apps",
        route: "/apps",
        icon: "apps",
        table: "installed_apps",
        time_column: None,
    },
];

/// One tile, ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMetric {
    pub id: String,
    pub label: String,
    pub route: String,
    pub icon: String,
    pub count: i64,
    /// The period this data covers. `None` when the source has no timestamp, or
    /// when every row's timestamp is unusable.
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// [`SPARK_BUCKETS`] counts across `first_at..last_at`, gaps included.
    /// Empty when there is nothing datable to draw.
    pub series: Vec<i64>,
}

/// Every tile with something in it.
///
/// Sources with no rows are dropped rather than rendered empty: the dashboard's
/// tiles are navigation, and a tile that leads to an empty view is a dead end.
/// A module absent for a reason the user should know about (an unencrypted
/// backup excludes Safari, calls and Health) is explained by the view, not by a
/// disabled tile.
pub fn module_metrics(conn: &Connection, now: i64) -> Result<Vec<ModuleMetric>> {
    let mut out = Vec::new();
    for src in METRIC_SOURCES {
        // A source whose table has not been created yet (an older cache, a
        // migration not yet run) is simply absent, not an error.
        if !table_exists(conn, src.table)? {
            continue;
        }
        let count: i64 =
            conn.query_row(&format!("SELECT COUNT(*) FROM {}", src.table), [], |r| {
                r.get(0)
            })?;
        if count == 0 {
            continue;
        }

        let (first_at, last_at, series) = match src.time_column {
            None => (None, None, Vec::new()),
            Some(col) => spark(conn, src.table, col, now)?,
        };

        out.push(ModuleMetric {
            id: src.id.to_string(),
            label: src.label.to_string(),
            route: src.route.to_string(),
            icon: src.icon.to_string(),
            count,
            first_at,
            last_at,
            series,
        });
    }
    Ok(out)
}

/// The span and the sparkline for one dated source.
///
/// The same window the report's charts use: a timestamp before the iPhone
/// existed, or in the future, is a decode failure rather than a date (Apple
/// stores seconds since 2001; read as Unix time one lands in 1970, and a zeroed
/// column lands there too). Left in, a single such row would flatten the whole
/// sparkline into its last bucket.
fn spark(
    conn: &Connection,
    table: &str,
    col: &str,
    now: i64,
) -> Result<(Option<i64>, Option<i64>, Vec<i64>)> {
    let horizon = now + 86_400;
    let datable = format!("{col} IS NOT NULL AND {col} >= {TIMELINE_START} AND {col} <= {horizon}");

    let (lo, hi): (Option<i64>, Option<i64>) = conn.query_row(
        &format!("SELECT MIN({col}), MAX({col}) FROM {table} WHERE {datable}"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Ok((None, None, Vec::new()));
    };

    // Equal slices of the span. `hi - lo + 1` keeps the last row inside the last
    // bucket instead of falling one past the end, and a single-instant span
    // (lo == hi) collapses to bucket 0 rather than dividing by zero.
    let n = SPARK_BUCKETS as i64;
    let mut series = vec![0i64; SPARK_BUCKETS];
    let mut stmt = conn.prepare(&format!(
        "SELECT ({col} - ?1) * ?2 / ?3 AS b, COUNT(*)
         FROM {table} WHERE {datable}
         GROUP BY b"
    ))?;
    let rows = stmt.query_map(params![lo, n, hi - lo + 1], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (bucket, count) = row?;
        // Clamped rather than trusted: integer division on a degenerate span is
        // exactly the kind of arithmetic that lands one past the end.
        let idx = bucket.clamp(0, n - 1) as usize;
        series[idx] += count;
    }
    Ok((Some(lo), Some(hi), series))
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;

    /// Cache tables that are deliberately NOT dashboard tiles, each with the
    /// reason. Adding a table to the cache without either giving it a
    /// [`MetricSource`] or listing it here fails
    /// [`every_content_table_is_accounted_for`] — which is the whole mechanism
    /// behind "a new parser appears on the dashboard by itself".
    const NOT_A_TILE: &[(&str, &str)] = &[
        ("meta", "schema version and import bookkeeping"),
        (
            "attachments",
            "belongs to a message, counted as part of Messages",
        ),
        ("note_media", "belongs to a note"),
        (
            "safari_bookmarks",
            "shown inside the Safari view, not a tile of its own",
        ),
        (
            "scan_runs",
            "Security Check runs — surfaced as a scan tile, not a data tile",
        ),
        ("findings", "Security Check results — same"),
        (
            "health_daily",
            "Health is one tile (workouts); its series live in the view",
        ),
        ("sleep_sessions", "Health detail"),
        ("workout_routes", "Health detail — one row per GPS sample"),
        ("activity_rings", "Health detail"),
        ("health_timezones", "Health detail"),
        ("health_achievements", "Health detail"),
        ("cycle_tracking", "Health detail"),
        (
            "interaction_channels",
            "per-channel breakdown inside Interactions",
        ),
        ("threads", "conversations — counted as part of Messages"),
        ("sqlite_sequence", "SQLite's own"),
        // The full-text index and its five shadow tables. Found by this very
        // guard on its first run, which is the behaviour it exists for.
        (
            "search_fts",
            "full-text search index over the content above",
        ),
        ("search_fts_data", "FTS5 shadow table"),
        ("search_fts_idx", "FTS5 shadow table"),
        ("search_fts_content", "FTS5 shadow table"),
        ("search_fts_docsize", "FTS5 shadow table"),
        ("search_fts_config", "FTS5 shadow table"),
    ];

    /// Messages hang off a thread; without one the FK rejects the insert.
    fn messages_db() -> CacheDb {
        let db = CacheDb::open_in_memory().unwrap();
        db.conn()
            .execute("INSERT INTO threads (id, identifier) VALUES (1,'t1')", [])
            .unwrap();
        db
    }

    fn seeded() -> CacheDb {
        let db = CacheDb::open_in_memory().unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO contacts (id, first_name) VALUES (1,'A'),(2,'B');
                 INSERT INTO threads (id, identifier) VALUES (1,'t1');",
            )
            .unwrap();
        db
    }

    /// The guard that makes the dashboard extend itself.
    ///
    /// A parser that adds a table now fails the build until someone decides
    /// whether it is a tile. Without this, "the dashboard is data-driven" would
    /// only mean the hardcoded list moved one layer down.
    #[test]
    fn every_content_table_is_accounted_for() {
        let db = CacheDb::open_in_memory().unwrap();
        let mut stmt = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        let tiled: Vec<&str> = METRIC_SOURCES.iter().map(|s| s.table).collect();
        let excused: Vec<&str> = NOT_A_TILE.iter().map(|(t, _)| *t).collect();
        let orphans: Vec<&String> = tables
            .iter()
            .filter(|t| !tiled.contains(&t.as_str()) && !excused.contains(&t.as_str()))
            .collect();
        assert!(
            orphans.is_empty(),
            "cache tables with no dashboard decision: {orphans:?}.\n\
             Give each one a MetricSource so it becomes a tile, or list it in \
             NOT_A_TILE with the reason it is not."
        );
    }

    /// Every tile names a table that exists — catches a rename or a typo, which
    /// would otherwise just make a tile quietly never appear.
    #[test]
    fn every_metric_source_points_at_a_real_table_and_column() {
        let db = CacheDb::open_in_memory().unwrap();
        for src in METRIC_SOURCES {
            assert!(
                table_exists(db.conn(), src.table).unwrap(),
                "{} points at missing table {}",
                src.id,
                src.table
            );
            if let Some(col) = src.time_column {
                // Reading it is the only honest check that it exists.
                db.conn()
                    .query_row(&format!("SELECT MIN({col}) FROM {}", src.table), [], |r| {
                        r.get::<_, Option<i64>>(0)
                    })
                    .unwrap_or_else(|e| panic!("{}.{col} is not queryable: {e}", src.table));
            }
        }
    }

    /// Ids and routes are the tile's identity to the frontend; duplicates would
    /// collide as React keys and send two tiles to the same place.
    #[test]
    fn tile_ids_are_unique() {
        let mut ids: Vec<&str> = METRIC_SOURCES.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate MetricSource id");
    }

    #[test]
    fn a_source_with_no_rows_gets_no_tile() {
        let db = seeded();
        let m = module_metrics(db.conn(), 1_760_000_000).unwrap();
        let ids: Vec<&str> = m.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["contacts"], "only the seeded table earns a tile");
        assert_eq!(m[0].count, 2);
        // Undated by design: contacts have no timestamp, so the tile says so by
        // carrying no span rather than inventing one.
        assert_eq!(m[0].first_at, None);
        assert!(m[0].series.is_empty());
    }

    #[test]
    fn a_dated_source_gets_a_span_and_a_sparkline() {
        let db = messages_db();
        let now = 1_760_000_000;
        let start = now - 240 * 86_400;
        for i in 0..48i64 {
            db.conn()
                .execute(
                    "INSERT INTO messages (id, thread_id, sent_at, body) VALUES (?1, 1, ?2, 'x')",
                    params![i, start + i * 5 * 86_400],
                )
                .unwrap();
        }
        let m = module_metrics(db.conn(), now).unwrap();
        let msg = m.iter().find(|x| x.id == "messages").unwrap();
        assert_eq!(msg.count, 48);
        assert_eq!(msg.first_at, Some(start));
        assert_eq!(msg.series.len(), SPARK_BUCKETS);
        assert_eq!(
            msg.series.iter().sum::<i64>(),
            48,
            "every dated row lands in exactly one bucket"
        );
        assert!(
            msg.series.iter().all(|&n| n > 0),
            "evenly spread data fills every bucket: {:?}",
            msg.series
        );
    }

    #[test]
    fn one_undecodable_timestamp_cannot_flatten_a_sparkline() {
        // The failure #66 hit on the report's axis, in sparkline form: Apple's
        // 2001 epoch read as Unix time lands in 1970, and without the window
        // every real row would collapse into the final bucket.
        let db = messages_db();
        let now = 1_760_000_000;
        let start = now - 120 * 86_400;
        for i in 0..24i64 {
            db.conn()
                .execute(
                    "INSERT INTO messages (id, thread_id, sent_at, body) VALUES (?1, 1, ?2, 'x')",
                    params![i, start + i * 5 * 86_400],
                )
                .unwrap();
        }
        db.conn()
            .execute(
                "INSERT INTO messages (id, thread_id, sent_at, body) VALUES (99, 1, 0, 'epoch')",
                [],
            )
            .unwrap();

        let msg = module_metrics(db.conn(), now)
            .unwrap()
            .into_iter()
            .find(|x| x.id == "messages")
            .unwrap();
        assert_eq!(msg.count, 25, "the row still counts");
        assert_eq!(msg.first_at, Some(start), "but it does not set the span");
        assert_eq!(
            msg.series.iter().sum::<i64>(),
            24,
            "and it is not drawn on the sparkline"
        );
        assert!(msg.series.iter().filter(|&&n| n > 0).count() > 1);
    }

    #[test]
    fn a_single_instant_does_not_divide_by_zero() {
        let db = messages_db();
        let now = 1_760_000_000;
        for i in 0..3i64 {
            db.conn()
                .execute(
                    "INSERT INTO messages (id, thread_id, sent_at, body) VALUES (?1, 1, ?2, 'x')",
                    params![i, now - 3600],
                )
                .unwrap();
        }
        let msg = module_metrics(db.conn(), now)
            .unwrap()
            .into_iter()
            .find(|x| x.id == "messages")
            .unwrap();
        assert_eq!(msg.first_at, msg.last_at);
        assert_eq!(msg.series.len(), SPARK_BUCKETS);
        assert_eq!(msg.series[0], 3, "all in the first bucket");
        assert_eq!(msg.series.iter().sum::<i64>(), 3);
    }
}
