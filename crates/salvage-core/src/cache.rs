//! Cache / index store: one SQLite database per imported backup.
//!
//! Both import paths write here — the iLEAPP normalizer (MVP) and the native
//! lazy parsers (Phase 2) — and every UI view reads from here. The schema is
//! therefore engine-neutral: rows record artifacts, not iLEAPP report shapes.
//!
//! Timestamps are Unix epoch seconds (INTEGER). Migrations are tracked with
//! `PRAGMA user_version`.

use std::path::Path;

use rusqlite::Connection;

use crate::Result;

pub struct CacheDb {
    conn: Connection,
}

const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- One row per import attempt, so partial/failed imports are visible and
-- resumable rather than silently half-populated.
CREATE TABLE import_runs (
    id             INTEGER PRIMARY KEY,
    engine         TEXT NOT NULL,             -- 'ileapp' | 'native'
    engine_version TEXT,
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    status         TEXT NOT NULL,             -- 'running' | 'succeeded' | 'failed' | 'cancelled'
    error          TEXT
);

CREATE TABLE contacts (
    id           INTEGER PRIMARY KEY,
    first_name   TEXT,
    last_name    TEXT,
    organization TEXT,
    phones_json  TEXT NOT NULL DEFAULT '[]',  -- [{"label":..,"value":..}]
    emails_json  TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE threads (
    id              INTEGER PRIMARY KEY,
    identifier      TEXT NOT NULL,            -- chat guid / group id / phone number
    display_name    TEXT,
    service         TEXT,                     -- 'iMessage' | 'SMS' | app module id
    last_message_at INTEGER,
    message_count   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_threads_last_message ON threads(last_message_at DESC);

CREATE TABLE messages (
    id              INTEGER PRIMARY KEY,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    sender          TEXT,                     -- handle; NULL when is_from_me
    is_from_me      INTEGER NOT NULL DEFAULT 0,
    body            TEXT,
    sent_at         INTEGER,
    has_attachments INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_messages_thread ON messages(thread_id, sent_at);

CREATE TABLE attachments (
    id         INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id),
    filename   TEXT,
    mime_type  TEXT,
    -- Path to extracted bytes under the app cache dir, when materialized.
    local_path TEXT
);

CREATE TABLE calls (
    id          INTEGER PRIMARY KEY,
    address     TEXT,                         -- phone number / FaceTime handle
    direction   TEXT,                         -- 'incoming' | 'outgoing'
    answered    INTEGER,
    duration_s  INTEGER,
    occurred_at INTEGER,
    service     TEXT                          -- 'phone' | 'facetime' | ...
);
CREATE INDEX idx_calls_occurred ON calls(occurred_at DESC);

CREATE TABLE safari_history (
    id          INTEGER PRIMARY KEY,
    url         TEXT NOT NULL,
    title       TEXT,
    visited_at  INTEGER,
    visit_count INTEGER
);
CREATE INDEX idx_safari_visited ON safari_history(visited_at DESC);

CREATE TABLE notes (
    id          INTEGER PRIMARY KEY,
    folder      TEXT,
    title       TEXT,
    snippet     TEXT,
    body_html   TEXT,
    created_at  INTEGER,
    modified_at INTEGER
);

CREATE TABLE media_items (
    id              INTEGER PRIMARY KEY,
    -- iLEAPP's `_lava_media_items.id`, so artifact rows can be linked back to
    -- their media during normalization. NULL for natively-parsed media.
    engine_media_id TEXT UNIQUE,
    domain          TEXT,                     -- e.g. 'CameraRollDomain'
    relative_path   TEXT NOT NULL,
    kind            TEXT NOT NULL,            -- 'photo' | 'video'
    mime_type       TEXT,
    taken_at        INTEGER,
    width           INTEGER,
    height          INTEGER,
    duration_s      REAL,
    -- Paths under the app cache dir; NULL until materialized.
    thumb_path      TEXT,
    local_path      TEXT
);
CREATE INDEX idx_media_taken ON media_items(taken_at DESC);

-- Cross-artifact full-text search. ref_kind/ref_id point back at the source row.
CREATE VIRTUAL TABLE search_fts USING fts5(
    ref_kind UNINDEXED,
    ref_id   UNINDEXED,
    title,
    body
);
"#;

impl CacheDb {
    /// Open (creating and migrating as needed) the cache DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// In-memory DB for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA_V1)?;
        }
        // Future migrations: `if version < 2 { ... }`
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(CacheDb { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_schema_and_reopens_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cache.db");
        {
            let db = CacheDb::open(&path).unwrap();
            assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        }
        // Second open must not re-run CREATEs.
        let db = CacheDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn fts_round_trip() {
        let db = CacheDb::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO search_fts (ref_kind, ref_id, title, body) VALUES (?1, ?2, ?3, ?4)",
                ("note", 42, "Grocery list", "milk eggs bread"),
            )
            .unwrap();
        let (kind, id): (String, i64) = db
            .conn()
            .query_row(
                "SELECT ref_kind, ref_id FROM search_fts WHERE search_fts MATCH 'eggs'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), id), ("note", 42));
    }

    #[test]
    fn messages_belong_to_threads() {
        let db = CacheDb::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO threads (id, identifier, service) VALUES (1, '+46700000000', 'SMS')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at)
                 VALUES (1, '+46700000000', 0, 'hej', 1700000000)",
                [],
            )
            .unwrap();
        // FK enforcement: inserting into a missing thread must fail.
        assert!(db
            .conn()
            .execute(
                "INSERT INTO messages (thread_id, body) VALUES (999, 'orphan')",
                [],
            )
            .is_err());
    }
}
