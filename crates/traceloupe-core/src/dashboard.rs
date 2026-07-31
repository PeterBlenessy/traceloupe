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

/// Below this many rows there is no shape to draw — two bars is not a chart.
///
/// Above it the bucket count follows the data rather than a fixed threshold.
/// The first version suppressed the sparkline below twelve rows, which meant a
/// handful of voice memos got nothing at all despite having perfectly good
/// timestamps: a cliff where a scale belonged.
pub const SPARK_MIN_ROWS: i64 = 4;

/// How many buckets a set of `n` rows deserves: enough to show a shape, never
/// more than there are rows to put in them.
fn bucket_count(n: i64) -> usize {
    n.clamp(SPARK_MIN_ROWS, SPARK_BUCKETS as i64) as usize
}

/// One table a module draws from.
#[derive(Debug, Clone, Copy)]
pub struct TableSource {
    pub table: &'static str,
    /// The column to take the span and the sparkline from — or any SQL
    /// expression, which is how a TEXT date joins in. `None` for data that is
    /// real but undated (contacts, installed apps): those tiles show a count and
    /// nothing else, rather than a fabricated timeline.
    pub time_column: Option<&'static str>,
    /// What this table is called when the module shows its parts, e.g. Health's
    /// "Workouts" / "Daily activity" / "Sleep".
    pub facet: Option<&'static str>,
}

/// Where a module's facets come from — the small icons that say what is
/// actually inside it, rather than one generic glyph.
///
/// A query rather than a table+column pair: the interesting facets are rarely
/// on the module's own table (a message's service lives on its thread), and a
/// join expressed here is clearer than a join modelled in types.
#[derive(Debug, Clone, Copy)]
pub enum FacetSource {
    /// `SELECT label, count … ORDER BY count DESC`. Labels feed the view's
    /// brand-icon lookup, so they are service names, bundle ids and the like.
    Query(&'static str),
    /// One facet per table, labelled by its `facet` and counted by its rows —
    /// Health's categories.
    PerTable,
}

/// One tile's definition — the whole of what makes a kind of data appear on the
/// dashboard.
#[derive(Debug, Clone, Copy)]
pub struct MetricSource {
    pub id: &'static str,
    /// Fallback label. The view prefers the sidebar's name for this route, so
    /// the two can never drift; this is what a module the sidebar does not know
    /// about falls back to.
    pub label: &'static str,
    pub route: &'static str,
    pub icon: &'static str,
    /// One or more tables. Health is several: a backup with steps and sleep but
    /// no workouts is still a backup with Health data in it.
    pub tables: &'static [TableSource],
    pub facets: Option<FacetSource>,
}

/// How many facets a tile shows before it stops.
pub const FACET_CAP: usize = 4;

/// Every kind of data that earns a tile. Extend this to extend the dashboard.
pub const METRIC_SOURCES: &[MetricSource] = &[
    MetricSource {
        id: "messages",
        label: "Messages",
        route: "/messages",
        icon: "messages",
        tables: &[one("messages", Some("sent_at"))],
        // A message's service lives on its thread, so this counts messages per
        // service rather than threads per service.
        facets: Some(FacetSource::Query(
            "SELECT t.service, COUNT(*) FROM messages m
             JOIN threads t ON t.id = m.thread_id
             WHERE t.service IS NOT NULL AND t.service <> ''
             GROUP BY t.service ORDER BY 2 DESC",
        )),
    },
    MetricSource {
        id: "photos",
        label: "Photos",
        route: "/photos",
        icon: "photos",
        tables: &[one("media_items", Some("taken_at"))],
        facets: Some(FacetSource::Query(
            "SELECT source, COUNT(*) FROM media_items
             WHERE source IS NOT NULL AND source <> ''
             GROUP BY source ORDER BY 2 DESC",
        )),
    },
    MetricSource {
        id: "contacts",
        label: "Contacts",
        route: "/contacts",
        icon: "contacts",
        tables: &[one("contacts", None)],
        facets: None,
    },
    MetricSource {
        id: "calls",
        label: "Calls",
        route: "/calls",
        icon: "calls",
        tables: &[one("calls", Some("occurred_at"))],
        facets: Some(FacetSource::Query(
            "SELECT service, COUNT(*) FROM calls
             WHERE service IS NOT NULL AND service <> ''
             GROUP BY service ORDER BY 2 DESC",
        )),
    },
    MetricSource {
        id: "safari",
        label: "Safari",
        route: "/safari",
        icon: "safari",
        tables: &[one("safari_history", Some("visited_at"))],
        facets: None,
    },
    MetricSource {
        id: "notes",
        label: "Notes",
        route: "/notes",
        icon: "notes",
        tables: &[one("notes", Some("modified_at"))],
        facets: None,
    },
    MetricSource {
        id: "recordings",
        label: "Recordings",
        route: "/recordings",
        icon: "recordings",
        tables: &[one("recordings", Some("recorded_at"))],
        facets: None,
    },
    MetricSource {
        id: "calendar",
        label: "Calendar",
        route: "/calendar",
        icon: "calendar",
        tables: &[one("calendar_events", Some("start_at"))],
        facets: None,
    },
    MetricSource {
        id: "reminders",
        label: "Reminders",
        route: "/reminders",
        icon: "reminders",
        tables: &[one("reminders", Some("created_at"))],
        facets: None,
    },
    // Health is several tables on purpose: pointing this at `workouts` alone
    // meant a backup with steps and sleep but no workouts showed no Health tile
    // at all, which is not what "no Health data" looks like.
    MetricSource {
        id: "health",
        label: "Health",
        route: "/health",
        icon: "health",
        tables: &[
            TableSource {
                table: "workouts",
                time_column: Some("start_at"),
                facet: Some("Workouts"),
            },
            TableSource {
                table: "health_daily",
                // An EXPRESSION, not a column: `day` is TEXT 'YYYY-MM-DD'. CAST
                // because strftime returns text, and a text/integer comparison
                // is lexicographic — which would silently pass the window check
                // and then sort wrongly.
                time_column: Some("CAST(strftime('%s', day) AS INTEGER)"),
                facet: Some("Daily activity"),
            },
            TableSource {
                table: "sleep_sessions",
                time_column: Some("start_at"),
                facet: Some("Sleep"),
            },
            TableSource {
                table: "health_achievements",
                time_column: Some("CAST(strftime('%s', earned_on) AS INTEGER)"),
                facet: Some("Awards"),
            },
        ],
        facets: Some(FacetSource::PerTable),
    },
    MetricSource {
        id: "interactions",
        label: "Interactions",
        route: "/interactions",
        icon: "interactions",
        tables: &[one("interactions", None)],
        facets: Some(FacetSource::Query(
            "SELECT bundle_id, incoming + outgoing FROM interaction_channels
             WHERE bundle_id IS NOT NULL ORDER BY 2 DESC",
        )),
    },
    MetricSource {
        id: "apps",
        label: "Apps",
        route: "/apps",
        icon: "apps",
        tables: &[one("installed_apps", None)],
        // `installed_apps` holds only bundle ids and no ordering worth the name,
        // so this returns candidates and the view shows the first few it has an
        // icon for — recognisable ones rather than alphabetically first ones.
        facets: Some(FacetSource::Query(
            "SELECT bundle_id, 0 FROM installed_apps ORDER BY bundle_id",
        )),
    },
];

/// A single-table source, which most modules are.
const fn one(table: &'static str, time_column: Option<&'static str>) -> TableSource {
    TableSource {
        table,
        time_column,
        facet: None,
    }
}

/// One facet of a tile: what is inside it, and how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facet {
    pub label: String,
    pub count: i64,
}

/// One tile, ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMetric {
    pub id: String,
    pub label: String,
    pub route: String,
    pub icon: String,
    pub count: i64,
    /// The period this data covers. `None` when nothing in it is datable.
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Bucket counts across the span, sized to the data. Empty when there is
    /// too little to make a shape.
    pub series: Vec<i64>,
    /// What is inside, biggest first — services, channels, categories. The view
    /// draws these as brand icons and falls back to the module icon when empty.
    pub facets: Vec<Facet>,
}

/// Every tile with something in it.
///
/// Sources with no rows are dropped rather than rendered empty: the dashboard's
/// tiles are navigation, and a tile that leads to an empty view is a dead end.
pub fn module_metrics(conn: &Connection, now: i64) -> Result<Vec<ModuleMetric>> {
    let mut out = Vec::new();
    for src in METRIC_SOURCES {
        // Tables a migration has not created yet are simply absent, not errors.
        let live: Vec<&TableSource> = src
            .tables
            .iter()
            .filter(|t| table_exists(conn, t.table).unwrap_or(false))
            .collect();
        if live.is_empty() {
            continue;
        }

        let mut count = 0i64;
        for t in &live {
            count += conn.query_row(&format!("SELECT COUNT(*) FROM {}", t.table), [], |r| {
                r.get::<_, i64>(0)
            })?;
        }
        if count == 0 {
            continue;
        }

        let (first_at, last_at, series) = spark(conn, &live, now)?;
        let facets = facets(conn, src, &live)?;

        out.push(ModuleMetric {
            id: src.id.to_string(),
            label: src.label.to_string(),
            route: src.route.to_string(),
            icon: src.icon.to_string(),
            count,
            first_at,
            last_at,
            series,
            facets,
        });
    }
    Ok(out)
}

fn facets(conn: &Connection, src: &MetricSource, live: &[&TableSource]) -> Result<Vec<Facet>> {
    let Some(source) = src.facets else {
        return Ok(Vec::new());
    };
    match source {
        FacetSource::PerTable => {
            let mut out = Vec::new();
            for t in live {
                let Some(label) = t.facet else { continue };
                let n: i64 =
                    conn.query_row(&format!("SELECT COUNT(*) FROM {}", t.table), [], |r| {
                        r.get(0)
                    })?;
                if n > 0 {
                    out.push(Facet {
                        label: label.to_string(),
                        count: n,
                    });
                }
            }
            out.sort_by_key(|f| std::cmp::Reverse(f.count));
            Ok(out)
        }
        FacetSource::Query(sql) => {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| {
                Ok(Facet {
                    label: r.get::<_, String>(0)?,
                    count: r.get::<_, i64>(1)?,
                })
            })?;
            // Not truncated here: a tile shows the first few it can draw an icon
            // for, and which those are is the view's business. Bounded by the
            // number of distinct services / channels / installed apps.
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        }
    }
}

/// The span and the sparkline across every table a module draws from.
///
/// The same window the report's charts use: a timestamp before the iPhone
/// existed, or in the future, is a decode failure rather than a date (Apple
/// stores seconds since 2001; read as Unix time one lands in 1970, and a zeroed
/// column lands there too). Left in, a single such row would flatten the whole
/// sparkline into its last bucket.
fn spark(
    conn: &Connection,
    live: &[&TableSource],
    now: i64,
) -> Result<(Option<i64>, Option<i64>, Vec<i64>)> {
    let horizon = now + 86_400;
    // One SELECT per dated table, unioned, so a multi-table module gets one
    // span and one shape rather than one per part.
    let parts: Vec<String> = live
        .iter()
        .filter_map(|t| t.time_column.map(|c| (t.table, c)))
        .map(|(table, col)| {
            format!(
                "SELECT ({col}) AS t FROM {table}
                 WHERE ({col}) IS NOT NULL AND ({col}) >= {TIMELINE_START}
                   AND ({col}) <= {horizon}"
            )
        })
        .collect();
    if parts.is_empty() {
        return Ok((None, None, Vec::new()));
    }
    let dated = parts.join(" UNION ALL ");

    let (lo, hi, n): (Option<i64>, Option<i64>, i64) = conn.query_row(
        &format!("SELECT MIN(t), MAX(t), COUNT(*) FROM ({dated})"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Ok((None, None, Vec::new()));
    };
    if n < SPARK_MIN_ROWS {
        // Datable, but not enough of it to be a shape. The span still stands.
        return Ok((Some(lo), Some(hi), Vec::new()));
    }

    // Equal slices of the span. `hi - lo + 1` keeps the last row inside the last
    // bucket instead of falling one past the end, and a single-instant span
    // (lo == hi) collapses to bucket 0 rather than dividing by zero.
    let buckets = bucket_count(n);
    let b = buckets as i64;
    let mut series = vec![0i64; buckets];
    let mut stmt = conn.prepare(&format!(
        "SELECT (t - ?1) * ?2 / ?3 AS b, COUNT(*) FROM ({dated}) GROUP BY b"
    ))?;
    let rows = stmt.query_map(params![lo, b, hi - lo + 1], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (bucket, count) = row?;
        // Clamped rather than trusted: integer division on a degenerate span is
        // exactly the kind of arithmetic that lands one past the end.
        //
        // A mutation pass reports the `b - 1` here as untested, and always will:
        // the divisor above guarantees an index inside the range, so this is
        // defence against a bug that does not currently exist. Unreachable
        // defence cannot be tested without constructing an impossible state —
        // an accepted survivor, not a missing test.
        series[bucket.clamp(0, b - 1) as usize] += count;
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
            "safari_searches",
            "shown inside the Safari view, same as safari_bookmarks",
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
        ("workout_routes", "one row per GPS sample inside a workout"),
        ("activity_rings", "Health detail"),
        (
            "health_device_use",
            "device provenance — shown in the Device view, not a data tile",
        ),
        ("health_timezones", "Health detail"),
        ("cycle_tracking", "Health detail"),
        (
            "interaction_channels",
            "per-channel breakdown inside Interactions",
        ),
        ("threads", "conversations — counted as part of Messages"),
        (
            "message_deletions",
            "evidence about messages that are gone — shown inside Messages, and \
             counting it as a data tile would imply we hold their content",
        ),
        ("sqlite_sequence", "SQLite's own"),
        // One tile over every artifact at once would be meaningless — it mixes
        // unrelated data behind a single number. Per-artifact tiles may well be
        // right, but that cannot be decided before the artifact list is
        // navigable at all (#195/#209); the row lands here as "not yet",
        // deliberately, rather than as a permanent no.
        (
            "artifact_rows",
            "declarative-artifact rows; per-artifact tiles wait on the navigation decision",
        ),
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

        let tiled: Vec<&str> = METRIC_SOURCES
            .iter()
            .flat_map(|s| s.tables.iter().map(|t| t.table))
            .collect();
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
            for t in src.tables {
                assert!(
                    table_exists(db.conn(), t.table).unwrap(),
                    "{} points at missing table {}",
                    src.id,
                    t.table
                );
                if let Some(col) = t.time_column {
                    // Running it is the only honest check — and it catches an
                    // expression that does not parse, which comparing strings
                    // never would.
                    db.conn()
                        .query_row(&format!("SELECT MIN({col}) FROM {}", t.table), [], |r| {
                            r.get::<_, Option<i64>>(0)
                        })
                        .unwrap_or_else(|e| panic!("{}.{col} is not queryable: {e}", t.table));
                }
            }
            // A facet query that does not run leaves a tile silently plain.
            if let Some(FacetSource::Query(sql)) = src.facets {
                let mut stmt = db
                    .conn()
                    .prepare(sql)
                    .unwrap_or_else(|e| panic!("{}'s facet query does not parse: {e}", src.id));
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .unwrap_or_else(|e| panic!("{}'s facet query does not run: {e}", src.id))
                    .for_each(drop);
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
        for i in 0..5i64 {
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
        assert_eq!(msg.series.len(), 5, "buckets follow the row count");
        assert_eq!(msg.series[0], 5, "all in the first bucket");
        assert_eq!(msg.series.iter().sum::<i64>(), 5);
    }

    #[test]
    fn a_small_module_still_gets_a_shape() {
        // The cliff this replaces: six voice memos used to render no sparkline
        // at all, despite recordings.recorded_at being wired the whole time.
        let db = CacheDb::open_in_memory().unwrap();
        let now = 1_760_000_000;
        for i in 0..6i64 {
            db.conn()
                .execute(
                    "INSERT INTO recordings (id, recorded_at, title, relative_path, local_path)
                     VALUES (?1, ?2, 'memo', 'r.m4a', '/tmp/r.m4a')",
                    params![i, now - (6 - i) * 30 * 86_400],
                )
                .unwrap();
        }
        let rec = module_metrics(db.conn(), now)
            .unwrap()
            .into_iter()
            .find(|x| x.id == "recordings")
            .unwrap();
        assert_eq!(rec.count, 6);
        assert_eq!(rec.series.len(), 6, "six rows, six bars — not nothing");
        assert_eq!(rec.series.iter().sum::<i64>(), 6);
    }

    /// The four gaps a mutation pass found in the tests above — each one a
    /// mutant that survived, meaning the code could be wrong there and nothing
    /// noticed. Written from that report rather than from imagination, which is
    /// the point of running it.
    #[test]
    fn the_sparkline_boundaries_are_pinned() {
        let now = 1_760_000_000;
        let spread = |n: i64| {
            let db = messages_db();
            for i in 0..n {
                db.conn()
                    .execute(
                        "INSERT INTO messages (id, thread_id, sent_at, body)
                         VALUES (?1, 1, ?2, 'x')",
                        params![i, now - (n - i) * 10 * 86_400],
                    )
                    .unwrap();
            }
            module_metrics(db.conn(), now)
                .unwrap()
                .into_iter()
                .find(|x| x.id == "messages")
                .unwrap()
        };

        // Exactly at the floor draws; one below does not. Without both sides the
        // comparison could be `<=` or `==` and no test would care.
        assert_eq!(
            spread(SPARK_MIN_ROWS - 1).series.len(),
            0,
            "below the floor there is no shape"
        );
        assert_eq!(
            spread(SPARK_MIN_ROWS).series.len(),
            SPARK_MIN_ROWS as usize,
            "at the floor there is"
        );

        // `hi - lo + 1` is what keeps the newest row inside the LAST bucket. Drop
        // the +1 and it lands one past the end — clamped, so the only visible
        // symptom is a last bucket that is too heavy and a first that is short.
        let m = spread(8);
        assert_eq!(m.series.len(), 8);
        assert_eq!(
            m.series,
            vec![1, 1, 1, 1, 1, 1, 1, 1],
            "eight evenly spread rows fill eight buckets one each; a bad divisor \
             piles them up instead"
        );
    }

    /// The window has two ends, and only the lower one was tested. A mutant that
    /// widened `now + 86_400` into something enormous survived, because nothing
    /// asked what happens to a timestamp in the future.
    #[test]
    fn a_future_timestamp_is_not_datable_either() {
        let db = messages_db();
        let now = 1_760_000_000;
        for i in 0..6i64 {
            db.conn()
                .execute(
                    "INSERT INTO messages (id, thread_id, sent_at, body)
                     VALUES (?1, 1, ?2, 'x')",
                    params![i, now - (6 - i) * 20 * 86_400],
                )
                .unwrap();
        }
        // A year from now: a clock that was wrong when the message was written,
        // or a field read as the wrong epoch.
        db.conn()
            .execute(
                "INSERT INTO messages (id, thread_id, sent_at, body)
                 VALUES (99, 1, ?1, 'from the future')",
                params![now + 365 * 86_400],
            )
            .unwrap();

        let m = module_metrics(db.conn(), now)
            .unwrap()
            .into_iter()
            .find(|x| x.id == "messages")
            .unwrap();
        assert_eq!(m.count, 7, "it is still a message");
        assert_eq!(
            m.last_at,
            Some(now - 20 * 86_400),
            "but it does not end the span"
        );
        assert_eq!(
            m.series.iter().sum::<i64>(),
            6,
            "and it is not on the sparkline"
        );
    }

    /// `table_exists` returning a constant `true` survived every test — nothing
    /// asked it about a table that is not there, which is the only question it
    /// exists to answer.
    #[test]
    fn a_missing_table_is_absent_rather_than_an_error() {
        let db = messages_db();
        assert!(table_exists(db.conn(), "messages").unwrap());
        assert!(
            !table_exists(db.conn(), "podcasts").unwrap(),
            "a table the schema does not have must read as absent"
        );
        // And a source whose table is missing must drop out of the metrics
        // rather than blowing up the whole dashboard.
        db.conn().execute("DROP TABLE recordings", []).unwrap();
        let ids: Vec<String> = module_metrics(db.conn(), 1_760_000_000)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert!(!ids.contains(&"recordings".to_string()));
    }

    #[test]
    fn health_counts_every_table_it_is_made_of() {
        // Pointing Health at `workouts` alone meant a backup with steps and
        // sleep but no workouts showed no Health tile at all.
        let db = CacheDb::open_in_memory().unwrap();
        let now = 1_760_000_000;
        db.conn()
            .execute_batch(
                "INSERT INTO health_daily (day, metric, value_sum, samples)
                   VALUES ('2025-01-01','steps',1000,1),('2025-02-01','steps',2000,1),
                          ('2025-03-01','steps',3000,1),('2025-04-01','steps',4000,1);
                 INSERT INTO sleep_sessions (id, start_at, end_at, stage)
                   VALUES (1, 1735689600, 1735718400, 'Asleep');",
            )
            .unwrap();

        let health = module_metrics(db.conn(), now)
            .unwrap()
            .into_iter()
            .find(|x| x.id == "health")
            .expect("Health has data even with no workouts");
        assert_eq!(health.count, 5, "four daily rows plus one sleep session");
        assert!(!health.series.is_empty(), "dated across both tables");
        let labels: Vec<&str> = health.facets.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily activity", "Sleep"], "biggest first");
    }

    #[test]
    fn facets_say_what_is_inside_a_module() {
        let db = messages_db();
        db.conn()
            .execute_batch(
                "UPDATE threads SET service = 'iMessage' WHERE id = 1;
                 INSERT INTO threads (id, identifier, service) VALUES (2,'t2','SMS');
                 INSERT INTO messages (id, thread_id, sent_at, body)
                   VALUES (1,«redacted»5689600,'a'),(2,«redacted»5689601,'b'),(3,«redacted»5689602,'c');",
            )
            .unwrap();
        let msg = module_metrics(db.conn(), 1_760_000_000)
            .unwrap()
            .into_iter()
            .find(|x| x.id == "messages")
            .unwrap();
        assert_eq!(
            msg.facets,
            vec![
                Facet {
                    label: "iMessage".into(),
                    count: 2
                },
                Facet {
                    label: "SMS".into(),
                    count: 1
                },
            ],
            "messages per service, biggest first — not threads per service"
        );
    }
}
