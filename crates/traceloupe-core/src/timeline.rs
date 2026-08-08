//! One stream of everything the device did, in time order.
//!
//! Every other view answers "what messages are there", "what photos are there".
//! This one answers **"what happened, and when"** — which is the question an
//! examination usually starts from, and the one the app could not answer at all:
//! reconstructing an afternoon meant opening six views and mentally interleaving
//! them by timestamp.
//!
//! A row carries its **content**, not a label saying something occurred. A
//! message brings its text, a photo its thumbnail, a note its snippet, a visited
//! page its title, an installed app its name. "Photo taken" is a log line; the
//! photo is the evidence.
//!
//! The sources are the timestamped tables the cache already has, so the timeline
//! costs no new parsing — it is a different way of reading what import produced.

use rusqlite::Row;
use serde::Serialize;

use crate::cache::CacheDb;
use crate::query::escape_like;
use crate::Result;

/// What kind of thing happened. Doubles as the filter facet, so the UI offers
/// exactly the kinds a given backup actually contains.
pub const EVENT_KINDS: &[&str] = &[
    "message",
    "photo",
    "video",
    "screenshot",
    "call",
    "visit",
    "note",
    "recording",
    "app",
];

/// One thing that happened, with enough of its content to be read in place.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    /// Row id within its own table — with `kind`, enough to open the source view.
    pub id: i64,
    /// One of [`EVENT_KINDS`].
    pub kind: String,
    /// Unix seconds. Never null: an event with no time cannot be placed.
    pub at: i64,
    /// Who or what it belongs to — a conversation, an album, a Safari profile.
    /// Doubles as the "source app" facet.
    pub source: Option<String>,
    /// The headline: a message's sender, a page's title, an app's name.
    pub title: Option<String>,
    /// The content itself: message text, note snippet, a URL.
    pub body: Option<String>,
    /// Set when this media has a rendered thumbnail. Only a *hint* that one
    /// exists — the servable URL is built from `kind` + `id` by the view, since
    /// a filesystem path is not something a webview can load.
    pub thumb_path: Option<String>,
    /// Seconds, for a call or a recording.
    pub duration_s: Option<f64>,
    /// True when the person did it (an outgoing message or call), so the view can
    /// take the same side it does in a conversation.
    pub is_from_me: bool,
}

/// Read one row of the union query. Column order is fixed by [`SELECT_COLUMNS`].
fn row_to_event(r: &Row) -> rusqlite::Result<TimelineEvent> {
    Ok(TimelineEvent {
        id: r.get(0)?,
        kind: r.get(1)?,
        at: r.get(2)?,
        source: r.get(3)?,
        title: r.get(4)?,
        body: r.get(5)?,
        thumb_path: r.get(6)?,
        duration_s: r.get(7)?,
        is_from_me: r.get::<_, i64>(8)? != 0,
    })
}

/// Every source, as one SELECT per kind.
///
/// A UNION ALL rather than a join: these tables share nothing but a timestamp,
/// and each contributes its own idea of a title and a body. Written out per kind
/// so what each event *says* is visible here rather than assembled in the UI.
///
/// `?1` is the search term (NULL for none) — bound once and reused by every arm,
/// which is why each arm repeats the same `(?1 IS NULL OR …)` shape.
fn union_sql() -> String {
    // Ordering the arms costs nothing (the outer ORDER BY decides) but keeps
    // this readable next to EVENT_KINDS.
    let arms = [
        // A message brings its text and the conversation it belongs to.
        // The FIRST arm names the columns for the whole union — SQLite takes the
        // result names from it, and without these the outer WHERE cannot say
        // `kind` or `at` at all.
        "SELECT m.id AS id, 'message' AS kind, m.sent_at AS at,
                COALESCE(t.display_name, t.identifier) AS source,
                CASE WHEN m.is_from_me = 1 THEN 'You' ELSE m.sender END AS title,
                m.body AS body, NULL AS thumb, NULL AS dur, m.is_from_me AS mine
           FROM messages m JOIN threads t ON t.id = m.thread_id
          WHERE m.sent_at IS NOT NULL
            AND (?1 IS NULL OR m.body LIKE ?1 ESCAPE '\\' OR m.sender LIKE ?1 ESCAPE '\\'
                 OR COALESCE(t.display_name, t.identifier) LIKE ?1 ESCAPE '\\')",
        // Camera-roll media. `subtype` promotes a screenshot to its own kind:
        // a screenshot is a different act from taking a photo, and reading a day
        // is much easier when the two are told apart.
        "SELECT mi.id,
                CASE WHEN mi.subtype = 'screenshot' THEN 'screenshot'
                     WHEN mi.kind = 'video' THEN 'video' ELSE 'photo' END,
                mi.taken_at, mi.source, mi.location, mi.persons, mi.thumb_path,
                mi.duration_s, 0
           FROM media_items mi
          WHERE mi.taken_at IS NOT NULL
            AND (?1 IS NULL OR mi.location LIKE ?1 ESCAPE '\\'
                 OR mi.persons LIKE ?1 ESCAPE '\\' OR mi.albums LIKE ?1 ESCAPE '\\'
                 OR mi.relative_path LIKE ?1 ESCAPE '\\')",
        "SELECT c.id, 'call', c.occurred_at, c.service, c.address, c.direction,
                NULL, CAST(c.duration_s AS REAL),
                CASE WHEN c.direction = 'outgoing' THEN 1 ELSE 0 END
           FROM calls c
          WHERE c.occurred_at IS NOT NULL
            AND (?1 IS NULL OR c.address LIKE ?1 ESCAPE '\\')",
        "SELECT s.id, 'visit', s.visited_at, s.profile, s.title, s.url, NULL, NULL, 0
           FROM safari_history s
          WHERE s.visited_at IS NOT NULL
            AND (?1 IS NULL OR s.title LIKE ?1 ESCAPE '\\' OR s.url LIKE ?1 ESCAPE '\\')",
        // Created, not modified: the timeline is about when something happened,
        // and a note edited last week did not happen last week.
        "SELECT n.id, 'note', n.created_at, n.folder, n.title, n.snippet, NULL, NULL, 1
           FROM notes n
          WHERE n.created_at IS NOT NULL
            AND (?1 IS NULL OR n.title LIKE ?1 ESCAPE '\\' OR n.snippet LIKE ?1 ESCAPE '\\')",
        "SELECT r.id, 'recording', r.recorded_at, r.folder, r.title, NULL, NULL,
                r.duration_s, 1
           FROM recordings r
          WHERE r.recorded_at IS NOT NULL
            AND (?1 IS NULL OR r.title LIKE ?1 ESCAPE '\\')",
        // `downloaded` is RFC-3339 text, not epoch seconds — strftime converts it
        // rather than the column being read as a number, which would silently
        // place every install at 1970.
        "SELECT a.rowid, 'app', CAST(strftime('%s', a.downloaded) AS INTEGER),
                a.seller, a.name, a.bundle_id, NULL, NULL, 0
           FROM installed_apps a
          WHERE a.downloaded IS NOT NULL
            AND strftime('%s', a.downloaded) IS NOT NULL
            AND (?1 IS NULL OR a.name LIKE ?1 ESCAPE '\\' OR a.bundle_id LIKE ?1 ESCAPE '\\')",
    ];
    arms.join("\n UNION ALL\n")
}

/// The `WHERE` applied to the union: kinds, sources and a time range.
///
/// Kinds and sources arrive as JSON arrays so an empty array means "no filter"
/// without needing a different statement per selection count.
const FILTERS: &str = "WHERE (json_array_length(?2) = 0
                          OR kind IN (SELECT value FROM json_each(?2)))
                        AND (json_array_length(?3) = 0
                          OR COALESCE(source, '') IN (SELECT value FROM json_each(?3)))
                        AND (?4 IS NULL OR at >= ?4)
                        AND (?5 IS NULL OR at <= ?5)";

fn like(search: Option<&str>) -> Option<String> {
    search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", escape_like(s)))
}

fn json_of(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".into())
}

/// Which events to consider: the facets, the range and the search term.
///
/// One struct rather than five loose parameters — every caller passes all of
/// them, and a positional `Option<i64>, Option<i64>` pair is exactly the shape
/// that gets swapped by accident.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventFilter<'a> {
    pub kinds: &'a [String],
    pub sources: &'a [String],
    pub lo: Option<i64>,
    pub hi: Option<i64>,
    pub search: Option<&'a str>,
}

/// How many events match, for the virtualizer.
pub fn count_events(cache: &CacheDb, f: &EventFilter) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM (
             SELECT id, kind, at, source, title, body, thumb, dur, mine FROM ({})
         ) {FILTERS}",
        union_sql()
    );
    let n = cache.conn().query_row(
        &sql,
        rusqlite::params![
            like(f.search),
            json_of(f.kinds),
            json_of(f.sources),
            f.lo,
            f.hi
        ],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// A window of the timeline, newest first when `desc`.
pub fn get_events(
    cache: &CacheDb,
    f: &EventFilter,
    offset: i64,
    limit: i64,
    desc: bool,
) -> Result<Vec<TimelineEvent>> {
    // `id` is the tiebreaker so paging is stable: without it two events sharing a
    // second can swap places between windows, and a row appears twice or not at
    // all while scrolling.
    let order = if desc {
        "at DESC, id DESC"
    } else {
        "at ASC, id ASC"
    };
    let sql = format!(
        "SELECT id, kind, at, source, title, body, thumb, dur, mine FROM (
             SELECT id, kind, at, source, title, body, thumb, dur, mine FROM ({})
         ) {FILTERS}
         ORDER BY {order} LIMIT ?6 OFFSET ?7",
        union_sql()
    );
    let conn = cache.conn();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![
            like(f.search),
            json_of(f.kinds),
            json_of(f.sources),
            f.lo,
            f.hi,
            limit.max(1),
            offset.max(0)
        ],
        row_to_event,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One facet value and how many events carry it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineFacet {
    pub value: String,
    pub count: i64,
}

/// Counts per kind and per source, so the filter offers only what this backup
/// has — an empty facet is a promise the data cannot keep.
pub fn facets(cache: &CacheDb) -> Result<(Vec<TimelineFacet>, Vec<TimelineFacet>)> {
    let base = union_sql();
    let conn = cache.conn();
    let collect = |column: &str| -> Result<Vec<TimelineFacet>> {
        let sql = format!(
            "SELECT {column}, COUNT(*) FROM (
                 SELECT id, kind, at, source, title, body, thumb, dur, mine FROM ({base})
             ) WHERE {column} IS NOT NULL AND {column} <> ''
             GROUP BY {column} ORDER BY COUNT(*) DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![Option::<String>::None], |r| {
            Ok(TimelineFacet {
                value: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    Ok((collect("kind")?, collect("source")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> CacheDb {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute_batch(
                "INSERT INTO threads (id, identifier, display_name, service)
                 VALUES (1, 'chat1', 'Sam', 'iMessage');
             INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at)
                 VALUES (1, 1, 'Sam', 0, 'are we still on?', 1000),
                        (2, 1, NULL, 1, 'yes, on my way', 1010);
             INSERT INTO media_items (id, relative_path, kind, source, taken_at, subtype, location)
                 VALUES (1, 'DCIM/a.HEIC', 'photo', 'Photos', 1020, NULL, 'Kungsholmen'),
                        (2, 'DCIM/b.PNG', 'photo', 'Photos', 1030, 'screenshot', NULL),
                        (3, 'DCIM/c.MOV', 'video', 'Photos', 1040, NULL, NULL);
             INSERT INTO calls (id, address, direction, occurred_at, service, duration_s)
                 VALUES (1, '+15551234567', 'outgoing', 1050, 'phone', 62);
             INSERT INTO safari_history (id, url, title, visited_at, profile)
                 VALUES (1, 'https://example.com', 'Example', 1060, 'Default');
             INSERT INTO notes (id, title, snippet, created_at, folder)
                 VALUES (1, 'Shopping', 'Milk', 1070, 'Notes');
             INSERT INTO recordings (id, title, recorded_at, relative_path, local_path, duration_s)
                 VALUES (1, 'Memo', 1080, 'r.m4a', '/tmp/r.m4a', 12.5);
             INSERT INTO installed_apps (bundle_id, name, downloaded)
                 VALUES ('com.example.app', 'Example App', '2023-11-14T22:13:20Z');",
            )
            .unwrap();
        }
        c
    }

    /// Every source lands in one stream, in time order.
    #[test]
    fn every_source_appears_in_one_ordered_stream() {
        let c = seeded();
        let all = get_events(
            &c,
            &EventFilter {
                kinds: &[],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        let kinds: Vec<&str> = all.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "message",
                "message",
                "photo",
                "screenshot",
                "video",
                "call",
                "visit",
                "note",
                "recording",
                "app",
            ],
            "oldest first, one row per thing that happened"
        );
        assert!(all.windows(2).all(|w| w[0].at <= w[1].at));
    }

    /// A row has to carry the thing itself, not a note that it exists. "Photo
    /// taken" is a log line; the content is the evidence.
    #[test]
    fn events_carry_their_content() {
        let c = seeded();
        let all = get_events(
            &c,
            &EventFilter {
                kinds: &[],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        let msg = &all[0];
        assert_eq!(msg.body.as_deref(), Some("are we still on?"));
        assert_eq!(msg.title.as_deref(), Some("Sam"));
        assert_eq!(msg.source.as_deref(), Some("Sam"), "its conversation");
        assert!(!msg.is_from_me);
        assert!(all[1].is_from_me, "an outgoing message takes my side");

        let note = all.iter().find(|e| e.kind == "note").unwrap();
        assert_eq!(note.title.as_deref(), Some("Shopping"));
        assert_eq!(note.body.as_deref(), Some("Milk"));

        let visit = all.iter().find(|e| e.kind == "visit").unwrap();
        assert_eq!(visit.title.as_deref(), Some("Example"));
        assert_eq!(visit.body.as_deref(), Some("https://example.com"));

        let call = all.iter().find(|e| e.kind == "call").unwrap();
        assert_eq!(call.duration_s, Some(62.0));
        assert!(call.is_from_me, "an outgoing call takes my side");
    }

    /// A screenshot is a different act from taking a photo, and a day reads far
    /// better when the two are told apart.
    #[test]
    fn a_screenshot_is_its_own_kind() {
        let c = seeded();
        let shots = get_events(
            &c,
            &EventFilter {
                kinds: &["screenshot".into()],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        assert_eq!(shots.len(), 1);
        assert_eq!(shots[0].id, 2);
        let photos = get_events(
            &c,
            &EventFilter {
                kinds: &["photo".into()],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        assert_eq!(
            photos.len(),
            1,
            "the screenshot must not also count as a photo"
        );
    }

    /// An install date is RFC-3339 text, not epoch seconds. Reading the column
    /// as a number would place every app at 1970 and quietly ruin the ordering.
    #[test]
    fn an_app_install_date_is_parsed_from_its_text() {
        let c = seeded();
        let app = get_events(
            &c,
            &EventFilter {
                kinds: &["app".into()],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            10,
            false,
        )
        .unwrap();
        assert_eq!(app.len(), 1);
        assert_eq!(app[0].at, 1_700_000_000);
        assert_eq!(app[0].title.as_deref(), Some("Example App"));
    }

    #[test]
    fn filters_compose() {
        let c = seeded();
        let media = get_events(
            &c,
            &EventFilter {
                kinds: &["photo".into(), "video".into()],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        assert_eq!(media.len(), 2);

        let ranged = get_events(
            &c,
            &EventFilter {
                kinds: &[],
                sources: &[],
                lo: Some(1050),
                hi: Some(1070),
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        assert_eq!(ranged.len(), 3, "call, visit, note");

        let by_source = get_events(
            &c,
            &EventFilter {
                kinds: &[],
                sources: &["Photos".into()],
                lo: None,
                hi: None,
                search: None,
            },
            0,
            100,
            false,
        )
        .unwrap();
        assert_eq!(by_source.len(), 3);
    }

    /// Search reaches the content, not only the labels.
    #[test]
    fn search_matches_across_every_source() {
        let c = seeded();
        let hit = |q: &str| {
            get_events(
                &c,
                &EventFilter {
                    search: Some(q),
                    ..Default::default()
                },
                0,
                100,
                false,
            )
            .unwrap()
        };
        assert_eq!(hit("still on").len(), 1, "message body");
        assert_eq!(hit("Kungsholmen").len(), 1, "a photo's place");
        assert_eq!(hit("example.com").len(), 1, "a visited url");
        assert_eq!(hit("Milk").len(), 1, "a note's snippet");
        assert_eq!(hit("Example App").len(), 1, "an app name");
    }

    #[test]
    fn counting_matches_the_window() {
        let c = seeded();
        let total = count_events(
            &c,
            &EventFilter {
                kinds: &[],
                sources: &[],
                lo: None,
                hi: None,
                search: None,
            },
        )
        .unwrap();
        assert_eq!(total, 10);
        assert_eq!(
            count_events(
                &c,
                &EventFilter {
                    kinds: &["message".into()],
                    sources: &[],
                    lo: None,
                    hi: None,
                    search: None
                }
            )
            .unwrap(),
            2
        );
    }

    /// Paging must not drop or repeat a row when events share a second.
    #[test]
    fn paging_is_stable_when_timestamps_collide() {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute_batch(
                "INSERT INTO threads (id, identifier, service) VALUES (1, 'c', 'iMessage');",
            )
            .unwrap();
            for i in 1..=6 {
                conn.execute(
                    "INSERT INTO messages (id, thread_id, body, sent_at) VALUES (?1, 1, ?2, 500)",
                    rusqlite::params![i, format!("m{i}")],
                )
                .unwrap();
            }
        }
        let mut seen = Vec::new();
        for page in 0..3 {
            for e in get_events(&c, &EventFilter::default(), page * 2, 2, false).unwrap() {
                seen.push(e.id);
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3, 4, 5, 6], "no row lost, none repeated");
    }

    /// The filter must offer only what this backup holds.
    #[test]
    fn facets_report_what_is_actually_there() {
        let c = seeded();
        let (kinds, sources) = facets(&c).unwrap();
        let kind = |k: &str| kinds.iter().find(|f| f.value == k).map(|f| f.count);
        assert_eq!(kind("message"), Some(2));
        assert_eq!(kind("photo"), Some(1));
        assert_eq!(kind("screenshot"), Some(1));
        assert!(sources.iter().any(|f| f.value == "Photos" && f.count == 3));
    }
}
