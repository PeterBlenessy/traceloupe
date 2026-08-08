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
    "search",
    "note",
    "recording",
    "app",
    "event",
    "reminder",
    "workout",
    "health",
];

/// Every (table, timestamp column) the stream reads, with the act it records.
///
/// Declared as COLUMNS, not tables, because a record is not a single point in
/// time. Anything with a lifecycle has several: a photo is shot, added to the
/// library and deleted; a note is written and edited; a reminder is created,
/// falls due and is completed. Listing tables would let a table "count as
/// covered" while three of its four moments went unshown.
pub const SOURCE_COLUMNS: &[(&str, &str, &str)] = &[
    ("messages", "sent_at", "sent"),
    ("messages", "deleted_at", "deleted"),
    ("media_items", "taken_at", "taken"),
    ("media_items", "added_at", "added"),
    ("media_items", "trashed_at", "deleted"),
    ("calls", "occurred_at", "placed"),
    ("safari_history", "visited_at", "visited"),
    ("safari_searches", "searched_at", "searched"),
    ("notes", "created_at", "created"),
    ("notes", "modified_at", "edited"),
    ("recordings", "recorded_at", "recorded"),
    ("calendar_events", "start_at", "started"),
    ("reminders", "created_at", "created"),
    ("reminders", "due_at", "due"),
    ("reminders", "completed_at", "completed"),
    ("workouts", "start_at", "started"),
    ("cycle_tracking", "logged_at", "logged"),
    ("installed_apps", "downloaded", "installed"),
];

/// Timestamp columns that are deliberately NOT events, with the reason.
///
/// Each line is a decision someone can disagree with, which is the point — an
/// undecided column fails the build rather than quietly never appearing.
pub const NOT_EVENTS: &[(&str, &str, &str)] = &[
    (
        "threads",
        "last_message_at",
        "a derived summary of the thread's own messages, which are already \
         events — it would double every conversation",
    ),
    (
        "contacts",
        "birthday_at",
        "an attribute OF the person, not a moment they acted. The moments that \
         belong here are when the contact was created and last changed, which \
         iOS records and this app does not yet capture (#370)",
    ),
    (
        "messages",
        "read_at",
        "a delivery receipt, not an act by the person — and one per message \
         would double the busiest source in the stream",
    ),
    (
        "messages",
        "delivered_at",
        "as read_at: a transport receipt about a message already shown",
    ),
    (
        "calendar_events",
        "end_at",
        "the far edge of an event already placed at its start; carried as the \
         entry's duration instead",
    ),
    (
        "workouts",
        "end_at",
        "as calendar end_at — carried as duration on the workout that started",
    ),
    (
        "interactions",
        "first_at",
        "an aggregate per person, summarising acts already in the stream",
    ),
    ("interactions", "last_at", "as first_at"),
    (
        "message_deletions",
        "after_at",
        "describes an ABSENCE of messages between two times; Messages is where \
         a gap belongs",
    ),
    ("message_deletions", "before_at", "as after_at"),
    (
        "scan_runs",
        "started_at",
        "when THIS APP ran a scan, not something the device did",
    ),
    ("scan_runs", "finished_at", "as started_at"),
    (
        "scan_runs",
        "feeds_generated_at",
        "provenance of the indicator feeds",
    ),
    (
        "sleep_sessions",
        "start_at",
        "stage-level sensor readings (In Bed / Core / Deep / REM), several an \
         hour — a series, not discrete acts. It would bury a night's real \
         events under dozens of rows. Worth revisiting folded to one row a night",
    ),
    ("sleep_sessions", "end_at", "as start_at"),
    (
        "health_timezones",
        "first_at",
        "which timezone the device was in: metadata about other records",
    ),
    ("health_timezones", "last_at", "as first_at"),
    (
        "health_device_use",
        "first_at",
        "which device recorded a health sample; provenance, not an act",
    ),
    ("health_device_use", "last_at", "as first_at"),
];

/// One thing that happened, with enough of its content to be read in place.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    /// Row id within its own table — with `kind`, enough to open the source view.
    pub id: i64,
    /// One of [`EVENT_KINDS`] — what the thing IS.
    pub kind: String,
    /// What HAPPENED to it: "taken", "added", "edited", "deleted", "sent", …
    ///
    /// A record is not a single point in time. A contact is created and updated;
    /// a photo is shot, added to the library, edited and deleted. Reading only
    /// the first of those puts the row at one moment and loses every other
    /// moment it was actually involved in.
    pub action: String,
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
        action: r.get(9)?,
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
    // Each arm is ONE (table, timestamp column) pair, because a row is not a
    // single point in time. A photo is shot, added to the library and deleted;
    // a note is written and later edited. Emitting only the first loses every
    // other moment that thing was actually involved in, which is why this is a
    // list of columns rather than a list of tables.
    //
    // The FIRST arm names the columns for the whole union — SQLite takes the
    // result names from it, and without these the outer WHERE cannot say `kind`
    // or `at` at all.
    let arms = [
        // --- messages -------------------------------------------------------
        "SELECT m.id AS id, 'message' AS kind, m.sent_at AS at,
                COALESCE(t.display_name, t.identifier) AS source,
                CASE WHEN m.is_from_me = 1 THEN 'You' ELSE m.sender END AS title,
                m.body AS body, NULL AS thumb, NULL AS dur, m.is_from_me AS mine,
                'sent' AS action
           FROM messages m JOIN threads t ON t.id = m.thread_id
          WHERE m.sent_at IS NOT NULL AND (?1 IS NULL OR m.body LIKE ?1 ESCAPE '\\'
                 OR m.sender LIKE ?1 ESCAPE '\\'
                 OR COALESCE(t.display_name, t.identifier) LIKE ?1 ESCAPE '\\')",
        // Deleting a message is one of the most telling things in a backup, and
        // it happened at a different moment from sending it.
        "SELECT m.id, 'message', m.deleted_at, COALESCE(t.display_name, t.identifier),
                CASE WHEN m.is_from_me = 1 THEN 'You' ELSE m.sender END,
                m.body, NULL, NULL, m.is_from_me, 'deleted'
           FROM messages m JOIN threads t ON t.id = m.thread_id
          WHERE m.deleted_at IS NOT NULL AND (?1 IS NULL OR m.body LIKE ?1 ESCAPE '\\')",
        // --- camera roll ----------------------------------------------------
        "SELECT mi.id,
                CASE WHEN mi.subtype = 'screenshot' THEN 'screenshot'
                     WHEN mi.kind = 'video' THEN 'video' ELSE 'photo' END,
                mi.taken_at, mi.source, mi.location, mi.persons, mi.thumb_path,
                mi.duration_s, 0, 'taken'
           FROM media_items mi
          WHERE mi.taken_at IS NOT NULL
            AND (?1 IS NULL OR mi.location LIKE ?1 ESCAPE '\\'
                 OR mi.persons LIKE ?1 ESCAPE '\\' OR mi.albums LIKE ?1 ESCAPE '\\'
                 OR mi.relative_path LIKE ?1 ESCAPE '\\')",
        // Added to the library — saving someone else's picture, or importing an
        // old one. Distinct from when it was shot, sometimes by years, and that
        // gap is often the interesting part.
        "SELECT mi.id,
                CASE WHEN mi.subtype = 'screenshot' THEN 'screenshot'
                     WHEN mi.kind = 'video' THEN 'video' ELSE 'photo' END,
                mi.added_at, mi.source, mi.location, mi.persons, mi.thumb_path,
                mi.duration_s, 0, 'added'
           FROM media_items mi
          WHERE mi.added_at IS NOT NULL
            AND (mi.taken_at IS NULL OR mi.added_at <> mi.taken_at)
            AND (?1 IS NULL OR mi.location LIKE ?1 ESCAPE '\\'
                 OR mi.relative_path LIKE ?1 ESCAPE '\\')",
        "SELECT mi.id,
                CASE WHEN mi.subtype = 'screenshot' THEN 'screenshot'
                     WHEN mi.kind = 'video' THEN 'video' ELSE 'photo' END,
                mi.trashed_at, mi.source, mi.location, mi.persons, mi.thumb_path,
                mi.duration_s, 0, 'deleted'
           FROM media_items mi
          WHERE mi.trashed_at IS NOT NULL
            AND (?1 IS NULL OR mi.location LIKE ?1 ESCAPE '\\'
                 OR mi.relative_path LIKE ?1 ESCAPE '\\')",
        // --- calls, browsing, searches ---------------------------------------
        "SELECT c.id, 'call', c.occurred_at, c.service, c.address, c.direction,
                NULL, CAST(c.duration_s AS REAL),
                CASE WHEN c.direction = 'outgoing' THEN 1 ELSE 0 END, 'placed'
           FROM calls c
          WHERE c.occurred_at IS NOT NULL
            AND (?1 IS NULL OR c.address LIKE ?1 ESCAPE '\\')",
        "SELECT s.id, 'visit', s.visited_at, s.profile, s.title, s.url, NULL, NULL, 0,
                'visited'
           FROM safari_history s
          WHERE s.visited_at IS NOT NULL
            AND (?1 IS NULL OR s.title LIKE ?1 ESCAPE '\\' OR s.url LIKE ?1 ESCAPE '\\')",
        "SELECT sr.id, 'search', sr.searched_at, sr.engine, sr.term, sr.url, NULL, NULL, 1,
                'searched'
           FROM safari_searches sr
          WHERE sr.searched_at IS NOT NULL
            AND (?1 IS NULL OR sr.term LIKE ?1 ESCAPE '\\')",
        // --- notes ------------------------------------------------------------
        "SELECT n.id, 'note', n.created_at, n.folder, n.title, n.snippet, NULL, NULL, 1,
                'created'
           FROM notes n
          WHERE n.created_at IS NOT NULL
            AND (?1 IS NULL OR n.title LIKE ?1 ESCAPE '\\' OR n.snippet LIKE ?1 ESCAPE '\\')",
        // Editing is its own act. Suppressed when it equals creation, which is
        // what an untouched note looks like — otherwise every note would appear
        // twice at the same instant.
        "SELECT n.id, 'note', n.modified_at, n.folder, n.title, n.snippet, NULL, NULL, 1,
                'edited'
           FROM notes n
          WHERE n.modified_at IS NOT NULL
            AND (n.created_at IS NULL OR n.modified_at <> n.created_at)
            AND (?1 IS NULL OR n.title LIKE ?1 ESCAPE '\\' OR n.snippet LIKE ?1 ESCAPE '\\')",
        "SELECT r.id, 'recording', r.recorded_at, r.folder, r.title, NULL, NULL,
                r.duration_s, 1, 'recorded'
           FROM recordings r
          WHERE r.recorded_at IS NOT NULL
            AND (?1 IS NULL OR r.title LIKE ?1 ESCAPE '\\')",
        // --- calendar, reminders, health --------------------------------------
        "SELECT ce.id, 'event', ce.start_at, ce.calendar_name, ce.title,
                COALESCE(ce.location, ce.notes), NULL,
                CASE WHEN ce.end_at IS NOT NULL AND ce.end_at > ce.start_at
                     THEN CAST(ce.end_at - ce.start_at AS REAL) END, 1, 'started'
           FROM calendar_events ce
          WHERE ce.start_at IS NOT NULL
            AND (?1 IS NULL OR ce.title LIKE ?1 ESCAPE '\\'
                 OR ce.location LIKE ?1 ESCAPE '\\' OR ce.notes LIKE ?1 ESCAPE '\\')",
        // Creating, being due, and completing are three different moments, and a
        // COALESCE over them silently picked one and threw the others away.
        "SELECT rm.id, 'reminder', rm.created_at, rm.list_name, rm.title, rm.notes,
                NULL, NULL, 1, 'created'
           FROM reminders rm
          WHERE rm.created_at IS NOT NULL
            AND (?1 IS NULL OR rm.title LIKE ?1 ESCAPE '\\' OR rm.notes LIKE ?1 ESCAPE '\\')",
        "SELECT rm.id, 'reminder', rm.due_at, rm.list_name, rm.title, rm.notes,
                NULL, NULL, 1, 'due'
           FROM reminders rm
          WHERE rm.due_at IS NOT NULL
            AND (?1 IS NULL OR rm.title LIKE ?1 ESCAPE '\\' OR rm.notes LIKE ?1 ESCAPE '\\')",
        "SELECT rm.id, 'reminder', rm.completed_at, rm.list_name, rm.title, rm.notes,
                NULL, NULL, 1, 'completed'
           FROM reminders rm
          WHERE rm.completed_at IS NOT NULL
            AND (?1 IS NULL OR rm.title LIKE ?1 ESCAPE '\\' OR rm.notes LIKE ?1 ESCAPE '\\')",
        "SELECT w.id, 'workout', w.start_at, 'Health', w.activity, NULL, NULL,
                CAST(w.duration_s AS REAL), 1, 'started'
           FROM workouts w
          WHERE w.start_at IS NOT NULL
            AND (?1 IS NULL OR w.activity LIKE ?1 ESCAPE '\\')",
        "SELECT ct.id, 'health', ct.logged_at, 'Health', ct.category, ct.detail,
                NULL, NULL, 1, 'logged'
           FROM cycle_tracking ct
          WHERE ct.logged_at IS NOT NULL
            AND (?1 IS NULL OR ct.category LIKE ?1 ESCAPE '\\'
                 OR ct.detail LIKE ?1 ESCAPE '\\')",
        // --- apps -------------------------------------------------------------
        // `downloaded` is RFC-3339 text, not epoch seconds — strftime converts it
        // rather than the column being read as a number, which would silently
        // place every install at 1970.
        "SELECT a.rowid, 'app', CAST(strftime('%s', a.downloaded) AS INTEGER),
                a.seller, a.name, a.bundle_id, NULL, NULL, 0, 'installed'
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
             SELECT id, kind, at, source, title, body, thumb, dur, mine, action FROM ({})
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
        "SELECT id, kind, at, source, title, body, thumb, dur, mine, action FROM (
             SELECT id, kind, at, source, title, body, thumb, dur, mine, action FROM ({})
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
                 SELECT id, kind, at, source, title, body, thumb, dur, mine, action FROM ({base})
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
                 VALUES ('com.example.app', 'Example App', '2023-11-14T22:13:20Z');
             INSERT INTO safari_searches (id, term, searched_at, source, engine)
                 VALUES (1, 'tide times', 1090, 'typed', 'google.com');
             INSERT INTO calendar_events (id, title, start_at, end_at, calendar_name, location)
                 VALUES (1, 'Dentist', 1100, 1700, 'Work', 'Clinic');
             INSERT INTO reminders (id, title, list_name, due_at, completed_at)
                 VALUES (1, 'Buy stamps', 'Errands', 1200, 1110);
             INSERT INTO workouts (id, activity, start_at, end_at, duration_s)
                 VALUES (1, 'Outdoor Walk', 1120, 1400, 280);
             INSERT INTO cycle_tracking (id, category, detail, logged_at)
                 VALUES (1, 'Cramps', 'Mild', 1130);",
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
        let seen: Vec<(&str, &str)> = all
            .iter()
            .map(|e| (e.kind.as_str(), e.action.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("message", "sent"),
                ("message", "sent"),
                ("photo", "taken"),
                ("screenshot", "taken"),
                ("video", "taken"),
                ("call", "placed"),
                ("visit", "visited"),
                ("note", "created"),
                ("recording", "recorded"),
                ("search", "searched"),
                ("event", "started"),
                // The reminder appears TWICE, at the two different moments it
                // was actually involved in — completed here, due later.
                ("reminder", "completed"),
                ("workout", "started"),
                ("health", "logged"),
                ("reminder", "due"),
                ("app", "installed"),
            ],
            "oldest first, one row per moment — not per row"
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
        assert_eq!(total, 16);
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

    /// The sources added after the first cut have to carry their content too,
    /// and be placed at the moment they happened rather than a nearby one.
    #[test]
    fn the_later_sources_carry_their_content_and_their_moment() {
        let c = seeded();
        let all = get_events(&c, &EventFilter::default(), 0, 100, false).unwrap();
        let of = |k: &str| all.iter().find(|e| e.kind == k).unwrap();

        let search = of("search");
        assert_eq!(search.title.as_deref(), Some("tide times"));
        assert_eq!(search.source.as_deref(), Some("google.com"));

        let event = of("event");
        assert_eq!(event.at, 1100, "a calendar entry is placed at its START");
        assert_eq!(event.body.as_deref(), Some("Clinic"));
        assert_eq!(event.duration_s, Some(600.0));

        // Completing it is the thing that happened; the due date was only a plan.
        let reminder = of("reminder");
        assert_eq!(reminder.at, 1110);
        assert_eq!(reminder.title.as_deref(), Some("Buy stamps"));

        assert_eq!(of("workout").title.as_deref(), Some("Outdoor Walk"));
        assert_eq!(of("health").title.as_deref(), Some("Cramps"));
        assert_eq!(of("health").body.as_deref(), Some("Mild"));
    }

    /// The correction that shaped this module: one ROW is many MOMENTS.
    ///
    /// A photo shot in 2019 and saved to the library in 2024 belongs at both
    /// points; filing it only at the shot date hides the fact that someone went
    /// and saved it years later, which is often the interesting part.
    #[test]
    fn one_row_yields_an_event_for_each_moment_of_its_life() {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute_batch(
                "INSERT INTO media_items (id, relative_path, kind, source, taken_at,
                                          added_at, trashed_at)
                     VALUES (1, 'DCIM/a.HEIC', 'photo', 'Photos', 1000, 5000, 9000);
                 INSERT INTO notes (id, title, created_at, modified_at)
                     VALUES (1, 'Draft', 2000, 6000);
                 -- Untouched: modified equals created, so it is NOT an edit.
                 INSERT INTO notes (id, title, created_at, modified_at)
                     VALUES (2, 'Untouched', 3000, 3000);",
            )
            .unwrap();
        }
        let all = get_events(&c, &EventFilter::default(), 0, 100, false).unwrap();
        let photo: Vec<(&str, i64)> = all
            .iter()
            .filter(|e| e.kind == "photo")
            .map(|e| (e.action.as_str(), e.at))
            .collect();
        assert_eq!(
            photo,
            vec![("taken", 1000), ("added", 5000), ("deleted", 9000)],
            "shot, saved to the library, then deleted — three moments, one row"
        );

        let edits: Vec<(&str, i64)> = all
            .iter()
            .filter(|e| e.kind == "note")
            .map(|e| (e.action.as_str(), e.at))
            .collect();
        assert_eq!(
            edits,
            vec![("created", 2000), ("created", 3000), ("edited", 6000)],
            "the untouched note must not appear twice at the same instant"
        );
    }

    /// A photo saved the moment it was shot has one moment, not two.
    #[test]
    fn an_added_time_equal_to_the_taken_time_is_not_a_second_event() {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute(
                "INSERT INTO media_items (id, relative_path, kind, taken_at, added_at)
                     VALUES (1, 'DCIM/a.HEIC', 'photo', 1000, 1000)",
                [],
            )
            .unwrap();
        }
        let all = get_events(&c, &EventFilter::default(), 0, 100, false).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].action, "taken");
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

    /// A record is not a single point in time, so coverage is checked per
    /// COLUMN, not per table.
    ///
    /// Anything with a lifecycle has several moments — a photo is shot, added
    /// to the library and deleted; a note is written and edited; a reminder is
    /// created, falls due and is completed. A table-level check would call
    /// `media_items` "covered" by `taken_at` alone while `added_at` and
    /// `trashed_at` went unshown, which is exactly what happened.
    ///
    /// So every timestamp column in the schema has to be either read by the
    /// stream or listed in NOT_EVENTS with a reason. A new column — the shape a
    /// new parser takes — fails the build instead of being discovered months
    /// later.
    #[test]
    fn timeline_accounts_for_every_timestamp_column() {
        let cache = CacheDb::open_in_memory().unwrap();
        let conn = cache.conn();
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                  WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        let declared: std::collections::HashSet<(&str, &str)> = SOURCE_COLUMNS
            .iter()
            .map(|(t, c, _)| (*t, *c))
            .chain(NOT_EVENTS.iter().map(|(t, c, _)| (*t, *c)))
            .collect();

        let mut missed = Vec::new();
        for table in &tables {
            let mut cols = conn
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .unwrap();
            let names: Vec<String> = cols
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .filter_map(std::result::Result::ok)
                .collect();
            for c in names {
                let lower = c.to_lowercase();
                // What a timestamp column looks like here. Deliberately broad: a
                // false positive costs one line of explanation, a false negative
                // costs a moment nobody notices is missing.
                let temporal =
                    lower.ends_with("_at") || lower == "downloaded" || lower.ends_with("_date");
                if temporal && !declared.contains(&(table.as_str(), c.as_str())) {
                    missed.push(format!("{table}.{c}"));
                }
            }
        }
        missed.sort();
        assert!(
            missed.is_empty(),
            "these columns carry a time the Timeline neither shows nor excuses: \
             {missed:?}\n\nEvery lifecycle moment is an event — created, edited, \
             deleted, shared. Add a UNION arm and a SOURCE_COLUMNS entry, or add \
             the column to NOT_EVENTS with the reason it is not something that \
             happened."
        );
    }

    /// Nothing may be declared that does not exist, in either list.
    #[test]
    fn every_declared_column_is_real() {
        let cache = CacheDb::open_in_memory().unwrap();
        let conn = cache.conn();
        for (table, column, _) in SOURCE_COLUMNS.iter().chain(NOT_EVENTS.iter()) {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .unwrap();
            let names: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .filter_map(std::result::Result::ok)
                .collect();
            assert!(
                names.iter().any(|n| n == column),
                "{table}.{column} is declared but no such column exists"
            );
        }
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
