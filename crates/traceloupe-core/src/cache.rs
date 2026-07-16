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

// Bumped to 5 so caches from earlier builds run the migration block once to catch
// up (v2 added columns/index; v3 adds the `recordings` table; v4 adds the native
// attachment decrypt columns; v5 adds the locked-note columns), then skip it on
// every subsequent open.
const SCHEMA_VERSION: i64 = 24;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS contacts (
    id           INTEGER PRIMARY KEY,
    first_name   TEXT,
    last_name    TEXT,
    organization TEXT,
    phones_json  TEXT NOT NULL DEFAULT '[]',  -- [{"label":..,"value":..}]
    emails_json  TEXT NOT NULL DEFAULT '[]',
    image        BLOB,                         -- contact photo thumbnail, if any
    -- Where the contact came from: 'Address Book' (the device's contacts) or a
    -- third-party app's social graph (e.g. 'TikTok'). Drives the Contacts filter.
    source       TEXT NOT NULL DEFAULT 'Address Book'
);

-- Apps that were installed on the device (from Info.plist), for the Apps view.
CREATE TABLE IF NOT EXISTS installed_apps (
    bundle_id TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS threads (
    id               INTEGER PRIMARY KEY,
    identifier       TEXT NOT NULL,            -- chat guid / group id / phone number
    display_name     TEXT,
    service          TEXT,                     -- 'iMessage' | 'SMS' | app module id
    last_message_at  INTEGER,
    message_count    INTEGER NOT NULL DEFAULT 0,
    participants_json TEXT NOT NULL DEFAULT '[]'  -- group member handles
);
CREATE INDEX IF NOT EXISTS idx_threads_last_message ON threads(last_message_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    sender          TEXT,                     -- handle; NULL when is_from_me
    is_from_me      INTEGER NOT NULL DEFAULT 0,
    body            TEXT,
    sent_at         INTEGER,
    has_attachments INTEGER NOT NULL DEFAULT 0,
    -- Content class for the message filter: 'text' | 'media' | 'link' | 'shared'
    -- | 'sticker' | 'system' | 'other'. NULL on rows imported before v11.
    kind            TEXT
);
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, sent_at);
-- Global chronological order for the cross-conversation timeline.
CREATE INDEX IF NOT EXISTS idx_messages_sent ON messages(sent_at, id);

CREATE TABLE IF NOT EXISTS attachments (
    id         INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id),
    filename   TEXT,
    mime_type  TEXT,
    -- Path to the attachment bytes: an iLEAPP-extracted file, or (native path)
    -- the backup's content-addressed blob (ciphertext on an encrypted backup).
    local_path TEXT,
    -- Encrypted backups only: wrapped key + real size to decrypt/trim local_path
    -- on demand (see media_items). NULL when local_path is already plaintext.
    decrypt_key BLOB,
    plain_size  INTEGER
);

CREATE TABLE IF NOT EXISTS calls (
    id          INTEGER PRIMARY KEY,
    address     TEXT,                         -- phone number / FaceTime handle
    direction   TEXT,                         -- 'incoming' | 'outgoing'
    answered    INTEGER,
    duration_s  INTEGER,
    occurred_at INTEGER,
    service     TEXT,                         -- 'phone' | 'facetime' | ...
    call_type   TEXT,                         -- 'audio' | 'video' (FaceTime), else NULL
    location    TEXT                          -- carrier/geo string stored on the call
);
CREATE INDEX IF NOT EXISTS idx_calls_occurred ON calls(occurred_at DESC);

CREATE TABLE IF NOT EXISTS safari_history (
    id          INTEGER PRIMARY KEY,
    url         TEXT NOT NULL,
    title       TEXT,
    visited_at  INTEGER,
    visit_count INTEGER
);
CREATE INDEX IF NOT EXISTS idx_safari_visited ON safari_history(visited_at DESC);

-- Safari bookmarks, reading-list items, and open tabs (History lives above).
-- `kind` selects which one, so the view can filter by type.
CREATE TABLE IF NOT EXISTS safari_bookmarks (
    id           INTEGER PRIMARY KEY,
    kind         TEXT NOT NULL,      -- 'bookmark' | 'reading_list' | 'tab'
    title        TEXT,
    url          TEXT,
    folder       TEXT,               -- containing folder / tab-group name
    date_added   INTEGER,            -- unix seconds (reading list: DateAdded)
    date_viewed  INTEGER,            -- reading list DateLastViewed
    preview_text TEXT,               -- reading list preview snippet
    position     INTEGER             -- source order_index, for stable sorting
);
CREATE INDEX IF NOT EXISTS idx_safari_bookmarks_kind ON safari_bookmarks(kind, position);

CREATE TABLE IF NOT EXISTS notes (
    id          INTEGER PRIMARY KEY,
    folder      TEXT,
    title       TEXT,
    snippet     TEXT,
    body_html   TEXT,
    created_at  INTEGER,
    modified_at INTEGER,
    -- Pinned to the top of the Notes app (ZISPINNED).
    pinned      INTEGER NOT NULL DEFAULT 0,
    -- Password-protected (Apple Notes locked note): body is withheld until the
    -- user supplies the note password. The crypto_* columns + encrypted_data are
    -- everything needed to decrypt on demand (never the plaintext at rest).
    locked         INTEGER NOT NULL DEFAULT 0,
    password_hint  TEXT,
    crypto_salt    BLOB,
    crypto_iter    INTEGER,
    crypto_iv      BLOB,
    crypto_tag     BLOB,
    encrypted_data BLOB,
    -- The per-note key, wrapped (RFC 3394) by the PBKDF2 key from the password.
    crypto_wrapped_key BLOB
);

CREATE TABLE IF NOT EXISTS media_items (
    id              INTEGER PRIMARY KEY,
    -- iLEAPP's `_lava_media_items.id`, so artifact rows can be linked back to
    -- their media during normalization. NULL for natively-parsed media.
    engine_media_id TEXT UNIQUE,
    domain          TEXT,                     -- e.g. 'CameraRollDomain'
    relative_path   TEXT NOT NULL,
    kind            TEXT NOT NULL,            -- 'photo' | 'video'
    -- Which app/artifact the media was found in ('Messages', 'Photos',
    -- 'WhatsApp', …), for the gallery's source filter. NULL if unknown.
    source          TEXT,
    mime_type       TEXT,
    taken_at        INTEGER,
    width           INTEGER,
    height          INTEGER,
    duration_s      REAL,
    -- Comma-separated names of people detected in the photo (from Photos.sqlite
    -- face recognition); NULL if none/unknown. Searchable + shown on the item.
    persons         TEXT,
    -- GPS coordinates + favorite flag, from Photos.sqlite (camera roll only).
    latitude        REAL,
    longitude       REAL,
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    -- Moment place/event name + user-album names (Photos.sqlite); searchable.
    location        TEXT,
    albums          TEXT,
    -- Paths under the app cache dir; NULL until materialized.
    thumb_path      TEXT,
    local_path      TEXT,
    -- Encrypted backups only: the class-prefixed wrapped key that decrypts
    -- local_path on demand (useless without the backup keys). NULL otherwise.
    decrypt_key     BLOB,
    -- Encrypted backups only: the original's real plaintext size, to trim CBC
    -- block padding after on-demand decryption. NULL otherwise.
    plain_size      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_media_taken ON media_items(taken_at DESC);

CREATE TABLE IF NOT EXISTS recordings (
    id            INTEGER PRIMARY KEY,
    -- User-set label, or NULL for an auto-named memo (the view falls back to the
    -- filename / date).
    title         TEXT,
    -- Voice Memos folder ("All Recordings", "Deleted", a custom folder), if known.
    folder        TEXT,
    recorded_at   INTEGER,                  -- unix seconds
    duration_s    REAL,
    relative_path TEXT NOT NULL,            -- e.g. 'Recordings/20240101 090000.m4a'
    -- Full path to the .m4a blob in the backup (ciphertext on encrypted backups).
    local_path    TEXT NOT NULL,
    mime_type     TEXT,
    -- Encrypted backups only: wrapped key + real size to decrypt/trim local_path
    -- on demand (see media_items). NULL for plaintext backups.
    decrypt_key   BLOB,
    plain_size    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_recordings_at ON recordings(recorded_at DESC);

CREATE TABLE IF NOT EXISTS calendar_events (
    id            INTEGER PRIMARY KEY,
    title         TEXT,
    notes         TEXT,
    location      TEXT,                        -- resolved place name / address
    start_at      INTEGER,                     -- unix seconds
    end_at        INTEGER,
    all_day       INTEGER NOT NULL DEFAULT 0,
    calendar_name TEXT,                         -- the containing calendar's title
    url           TEXT
);
CREATE INDEX IF NOT EXISTS idx_calendar_start ON calendar_events(start_at DESC);

CREATE TABLE IF NOT EXISTS reminders (
    id           INTEGER PRIMARY KEY,
    title        TEXT,
    notes        TEXT,
    list_name    TEXT,                          -- the containing reminders list
    due_at       INTEGER,                       -- unix seconds
    completed    INTEGER NOT NULL DEFAULT 0,
    completed_at INTEGER,
    flagged      INTEGER NOT NULL DEFAULT 0,
    priority     INTEGER,
    created_at   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_reminders_due ON reminders(due_at DESC);

CREATE TABLE IF NOT EXISTS workouts (
    id            INTEGER PRIMARY KEY,
    activity      TEXT,                          -- friendly activity name
    start_at      INTEGER,                       -- unix seconds
    end_at        INTEGER,
    duration_s    INTEGER,
    distance_m    REAL                           -- total distance in metres
);
CREATE INDEX IF NOT EXISTS idx_workouts_start ON workouts(start_at DESC);

-- CoreDuet cross-app communication graph: one row per contact interacted with.
CREATE TABLE IF NOT EXISTS interactions (
    id           INTEGER PRIMARY KEY,
    display_name TEXT,
    identifier   TEXT,                          -- phone / email / handle
    incoming     INTEGER NOT NULL DEFAULT 0,     -- messages/calls they sent you
    outgoing     INTEGER NOT NULL DEFAULT 0,     -- you sent them
    first_at     INTEGER,                        -- unix seconds
    last_at      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_interactions_total
    ON interactions((incoming + outgoing) DESC);

-- Cross-artifact full-text search. ref_kind/ref_id point back at the source row.
CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
    ref_kind UNINDEXED,
    ref_id   UNINDEXED,
    title,
    body
);
"#;

/// Add `column` to `table` if it isn't already present (SQLite has no
/// `ADD COLUMN IF NOT EXISTS`). Names are trusted constants, not user input.
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .any(|c| c == column);
    if !has {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
            [],
        )?;
    }
    Ok(())
}

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
        // WAL + NORMAL is the recommended durable-but-fast setting: commits append
        // to the WAL without an fsync each (fsync happens at checkpoint), which —
        // together with per-artifact transactions in the normalizer — keeps a
        // large import (hundreds of thousands of rows) from stalling on per-row
        // fsyncs. Safe: the cache is rebuilt on re-import if a crash truncates it.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA_V1)?;
        }
        // Migrations only run when the cache is older than the current schema —
        // not on every open. They're additive/idempotent, so running the whole
        // set to catch a cache up is safe. A cache newer than us (a future build)
        // is left untouched: we never stamp a lower user_version (no downgrade).
        if version < SCHEMA_VERSION {
            ensure_column(&conn, "contacts", "image", "BLOB")?;
            ensure_column(
                &conn,
                "threads",
                "participants_json",
                "TEXT NOT NULL DEFAULT '[]'",
            )?;
            ensure_column(&conn, "media_items", "decrypt_key", "BLOB")?;
            ensure_column(&conn, "media_items", "plain_size", "INTEGER")?;
            ensure_column(
                &conn,
                "contacts",
                "source",
                "TEXT NOT NULL DEFAULT 'Address Book'",
            )?;
            // Backfill the timeline index for caches created before it was in the
            // schema (new caches already have it from SCHEMA_V1).
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_messages_sent ON messages(sent_at, id)",
            )?;
            // v3: the recordings table (voice memos). CREATE IF NOT EXISTS so a
            // fresh cache (already has it from SCHEMA_V1) and an older cache both
            // end up with it.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS recordings (
                    id            INTEGER PRIMARY KEY,
                    title         TEXT,
                    folder        TEXT,
                    recorded_at   INTEGER,
                    duration_s    REAL,
                    relative_path TEXT NOT NULL,
                    local_path    TEXT NOT NULL,
                    mime_type     TEXT,
                    decrypt_key   BLOB,
                    plain_size    INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_recordings_at ON recordings(recorded_at DESC);",
            )?;
            // v4: native message attachments decrypt on demand like camera roll.
            ensure_column(&conn, "attachments", "decrypt_key", "BLOB")?;
            ensure_column(&conn, "attachments", "plain_size", "INTEGER")?;
            // v5: locked (password-protected) notes.
            ensure_column(&conn, "notes", "locked", "INTEGER NOT NULL DEFAULT 0")?;
            ensure_column(&conn, "notes", "password_hint", "TEXT")?;
            ensure_column(&conn, "notes", "crypto_salt", "BLOB")?;
            ensure_column(&conn, "notes", "crypto_iter", "INTEGER")?;
            ensure_column(&conn, "notes", "crypto_iv", "BLOB")?;
            ensure_column(&conn, "notes", "crypto_tag", "BLOB")?;
            ensure_column(&conn, "notes", "encrypted_data", "BLOB")?;
            // v6: pinned notes.
            ensure_column(&conn, "notes", "pinned", "INTEGER NOT NULL DEFAULT 0")?;
            // v7: Safari bookmarks / reading list / tabs (History is unchanged).
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS safari_bookmarks (
                    id           INTEGER PRIMARY KEY,
                    kind         TEXT NOT NULL,
                    title        TEXT,
                    url          TEXT,
                    folder       TEXT,
                    date_added   INTEGER,
                    date_viewed  INTEGER,
                    preview_text TEXT,
                    position     INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_safari_bookmarks_kind
                    ON safari_bookmarks(kind, position);",
            )?;
            // v8: people detected in each photo (Photos.sqlite face recognition).
            ensure_column(&conn, "media_items", "persons", "TEXT")?;
            // v9: GPS + favorite from Photos.sqlite.
            ensure_column(&conn, "media_items", "latitude", "REAL")?;
            ensure_column(&conn, "media_items", "longitude", "REAL")?;
            ensure_column(
                &conn,
                "media_items",
                "is_favorite",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            // v10: photo location (moment title) + user album names.
            ensure_column(&conn, "media_items", "location", "TEXT")?;
            ensure_column(&conn, "media_items", "albums", "TEXT")?;
            // v11: message content class, for the Messages content filter.
            ensure_column(&conn, "messages", "kind", "TEXT")?;
            // v12: FaceTime audio-vs-video type + carrier/geo location on calls.
            ensure_column(&conn, "calls", "call_type", "TEXT")?;
            ensure_column(&conn, "calls", "location", "TEXT")?;
            // v13: camera-roll EXIF (camera/lens/exposure) + original file size.
            ensure_column(&conn, "media_items", "camera", "TEXT")?;
            ensure_column(&conn, "media_items", "lens", "TEXT")?;
            ensure_column(&conn, "media_items", "exif", "TEXT")?;
            ensure_column(&conn, "media_items", "file_size", "INTEGER")?;
            // v14: additional contact detail fields.
            ensure_column(&conn, "contacts", "middle_name", "TEXT")?;
            ensure_column(&conn, "contacts", "nickname", "TEXT")?;
            ensure_column(&conn, "contacts", "job_title", "TEXT")?;
            ensure_column(&conn, "contacts", "department", "TEXT")?;
            ensure_column(&conn, "contacts", "birthday_at", "INTEGER")?;
            ensure_column(&conn, "contacts", "note", "TEXT")?;
            // v15: structured postal addresses (JSON [{label,value}], like phones).
            ensure_column(
                &conn,
                "contacts",
                "addresses_json",
                "TEXT NOT NULL DEFAULT '[]'",
            )?;
            // v16: camera-roll hidden-album flag (surfaced as a badge, not excluded).
            ensure_column(&conn, "media_items", "hidden", "INTEGER NOT NULL DEFAULT 0")?;
            // v17: iMessage read/delivered receipts.
            ensure_column(&conn, "messages", "read_at", "INTEGER")?;
            ensure_column(&conn, "messages", "delivered_at", "INTEGER")?;
            // v18: tapback/reaction summary on the target message (e.g. "❤️×2 👍").
            ensure_column(&conn, "messages", "reactions", "TEXT")?;
            // v23: message was edited (iOS 16+).
            ensure_column(&conn, "messages", "edited", "INTEGER NOT NULL DEFAULT 0")?;
            // v19: inline-reply preview (snippet of the message this one replies to).
            ensure_column(&conn, "messages", "reply_to_snippet", "TEXT")?;
            // v20: Safari deleted-history tombstones, flagged in the history list.
            ensure_column(
                &conn,
                "safari_history",
                "deleted",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            // v21: camera-roll media subtype ("screenshot" | "panorama").
            ensure_column(&conn, "media_items", "subtype", "TEXT")?;
            // v22: notes rich-content indicators (checklist + attachment counts).
            ensure_column(
                &conn,
                "notes",
                "has_checklist",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(&conn, "notes", "image_count", "INTEGER NOT NULL DEFAULT 0")?;
            ensure_column(
                &conn,
                "notes",
                "attachment_count",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            // v24: per-note wrapped key for locked-note decryption.
            ensure_column(&conn, "notes", "crypto_wrapped_key", "BLOB")?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
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

    /// Store a small key/value in the cache's `meta` table (e.g. the backup's
    /// source directory, so an encrypted backup can be reopened and decrypted).
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Read a value previously stored with [`Self::set_meta`].
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
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
