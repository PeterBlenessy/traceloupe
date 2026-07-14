//! Facebook Messenger native chat module.
//!
//! Schema facts (learned from iLEAPP `facebookMessenger.py`, written fresh —
//! provenance reference, §10):
//! - Message DBs: per-user SQLite files at `.../lightspeed-userDatabases/*.db`
//!   (there can be several; only some hold the `thread_messages` table).
//! - `thread_messages(timestamp_ms, sender_id, text, has_attachment, thread_key,
//!   message_id)` — one row per message; `timestamp_ms` is Unix milliseconds.
//! - `contacts(id, name)` — join `sender_id → id` for the sender's display name.
//! - `_user_info(facebook_user_id)` — the local user's id(s); a message whose
//!   `sender_id` is in here was sent by the owner (direction).
//! - Messenger stores no per-thread name here, so the conversation name is derived
//!   from the peer (the shared inserter's derive mode).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "messenger",
    service: "Messenger",
    locate,
    parse,
};

/// Every `lightspeed-userDatabases/*.db` in the backup (Messenger's per-user
/// message stores). The driver parses each; non-message DBs return empty.
fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%lightspeed-userDatabases/%.db")?;
    // The `.db*` glob in iLEAPP also matches `-wal`/`-shm`; keep only the DB.
    hits.retain(|e| e.relative_path.ends_with(".db"));
    Ok(hits)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // A lightspeed DB without thread_messages isn't a message store — skip quietly.
    if !table_exists(&src, "thread_messages")? {
        return Ok(Vec::new());
    }
    let has_user_info = table_exists(&src, "_user_info")?;
    // Direction: a message is "from me" when its sender is the local user. When
    // `_user_info` is absent, fall back to treating none as from-me.
    let from_me = if has_user_info {
        "(m.sender_id IN (SELECT facebook_user_id FROM _user_info))"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT
             m.thread_key,
             m.timestamp_ms,
             m.text,
             c.name,
             m.sender_id,
             {from_me} AS is_from_me,
             COALESCE(m.has_attachment, 0)
         FROM thread_messages m
         LEFT JOIN contacts c ON c.id = m.sender_id
         ORDER BY m.thread_key, m.timestamp_ms"
    );
    let mut stmt = src.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = r
            .get::<_, Option<String>>(0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let timestamp = r
            .get::<_, Option<i64>>(1)?
            .filter(|ms| *ms > 0)
            .map(|ms| ms / 1000);
        let body: Option<String> = r.get(2)?;
        let sender_name: Option<String> = r
            .get::<_, Option<String>>(3)?
            .filter(|s| !s.trim().is_empty());
        let sender_id: Option<String> = r.get::<_, Option<i64>>(4)?.map(|v| v.to_string());
        let is_from_me = r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0;
        let has_attachment = r.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0;

        out.push(AppMessage {
            chat_key,
            chat_name: None, // derived from the peer by the inserter
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { sender_name },
            sender_handle: None,
            sender_id,
            has_attachment,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;
    use crate::normalize::ImportReport;

    fn make_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("user.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE _user_info (facebook_user_id INTEGER);
             CREATE TABLE contacts (id INTEGER, name TEXT);
             CREATE TABLE thread_messages (message_id INTEGER, thread_key TEXT, timestamp_ms INTEGER,
                 sender_id INTEGER, text TEXT, has_attachment INTEGER);
             INSERT INTO _user_info (facebook_user_id) VALUES (100);
             INSERT INTO contacts (id, name) VALUES (100, 'Me'), (200, 'Jordan');
             INSERT INTO thread_messages VALUES (1, 't1', 1700000000000, 200, 'yo', 0);
             INSERT INTO thread_messages VALUES (2, 't1', 1700000100000, 100, 'hey Jordan', 0);",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_and_inserts_messenger_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 2);
        // Incoming from Jordan, then outgoing from the local user.
        assert!(!msgs[0].is_from_me && msgs[0].sender_name.as_deref() == Some("Jordan"));
        assert_eq!(msgs[0].timestamp, Some(1_700_000_000));
        assert!(msgs[1].is_from_me);

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "Messenger", msgs, &mut report).unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 2);
        // Derived 1:1 title = the peer's name.
        let name: String = cache
            .conn()
            .query_row("SELECT display_name FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Jordan");
    }

    #[test]
    fn non_message_db_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("other.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE something (x INTEGER);")
            .unwrap();
        assert!(parse(&db, "").unwrap().is_empty());
    }
}
