//! TikTok native chat module.
//!
//! Schema facts (learned from iLEAPP `tikTok.py`, written fresh — provenance
//! reference, §10):
//! - DB: `AwemeIM.db`, stored per-account under a directory named by the local
//!   user id (`account_id = basename(dirname(AwemeIM.db))`) — used for direction.
//! - `TIMMessageORM(localcreatedat, sender, content, belongingConversationIdentifier)`
//!   — one row per message; `content` is JSON with `$.text` for the body;
//!   `localcreatedat` is a Unix timestamp (seconds or ms).
//! - `AwemeContacts*` tables (dynamically named) hold `uid, nickname, customid,
//!   url1`; join `sender → uid` for the sender's display name.
//!
//! NOTE: unvalidated against a real TikTok backup — kept behind the iLEAPP fallback.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "tiktok",
    service: "TikTok",
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%AwemeIM.db")?;
    hits.retain(|e| e.relative_path.ends_with("AwemeIM.db"));
    Ok(hits)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// The local account id is the name of the directory holding `AwemeIM.db`
/// (`.../<account_id>/AwemeIM.db`). Used to tell sent from received.
fn account_id_from_path(rel_path: &str) -> Option<String> {
    let parts: Vec<&str> = rel_path.trim_end_matches('/').split('/').collect();
    // …/<account_id>/AwemeIM.db → second-to-last component.
    parts
        .len()
        .checked_sub(2)
        .and_then(|i| parts.get(i))
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// `uid → nickname` built from every `AwemeContacts*` table that has the needed
/// columns (the exact table name is versioned, e.g. `AwemeContactsV5`).
fn build_contacts(conn: &Connection) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(mut stmt) = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'AwemeContacts%'",
    ) else {
        return out;
    };
    let Ok(tables) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
        return out;
    };
    for table in tables.flatten() {
        // Table name comes from sqlite_master (not user input); still, only proceed
        // if it exposes uid + nickname.
        let sql = format!("SELECT uid, nickname FROM \"{table}\" WHERE uid IS NOT NULL");
        let Ok(mut s) = conn.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = s.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        }) else {
            continue;
        };
        for (uid, nick) in rows.flatten() {
            if let (Some(uid), Some(nick)) = (uid, nick) {
                if !nick.trim().is_empty() {
                    out.entry(uid).or_insert(nick);
                }
            }
        }
    }
    out
}

/// TikTok timestamps are Unix; some columns are milliseconds. Normalize to seconds.
fn to_unix_secs(v: i64) -> i64 {
    if v > 100_000_000_000 {
        v / 1000
    } else {
        v
    }
}

fn parse(db_path: &Path, rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "TIMMessageORM")? {
        return Ok(Vec::new());
    }
    let account_id = account_id_from_path(rel_path);
    let contacts = build_contacts(&conn);

    let mut stmt = conn.prepare(
        "SELECT localcreatedat, sender, content, belongingConversationIdentifier
         FROM TIMMessageORM
         ORDER BY belongingConversationIdentifier, localcreatedat",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let timestamp = r
            .get::<_, Option<i64>>(0)?
            .filter(|t| *t > 1)
            .map(to_unix_secs);
        // `sender` (a uid) may be stored as INTEGER or TEXT — read it type-agnostically.
        let sender: Option<String> = match r.get_ref(1)? {
            rusqlite::types::ValueRef::Integer(i) => Some(i.to_string()),
            rusqlite::types::ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
            _ => None,
        };
        let content: Option<String> = r.get(2)?;
        let chat_key: String = r
            .get::<_, Option<String>>(3)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());

        // The message body is JSON: {"text": "...", ...}. Non-text messages
        // (stickers, media) may have no "text" — keep them (empty body) so the
        // timeline is complete.
        let body = content
            .as_deref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|j| j.get("text").and_then(|t| t.as_str()).map(str::to_string))
            .filter(|s| !s.is_empty());

        let is_from_me = match (&sender, &account_id) {
            (Some(s), Some(a)) => s == a,
            _ => false,
        };
        let sender_name = sender.as_ref().and_then(|uid| contacts.get(uid).cloned());

        out.push(AppMessage {
            chat_key,
            chat_name: None, // derived from the peer
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { sender_name },
            sender_handle: None,
            sender_id: sender,
            has_attachment: false,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;
    use crate::normalize::ImportReport;

    #[test]
    fn account_id_is_the_parent_dir() {
        assert_eq!(
            account_id_from_path("Documents/ChatFiles/9988/AwemeIM.db").as_deref(),
            Some("9988")
        );
    }

    fn make_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("AwemeIM.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE AwemeContactsV5 (uid TEXT, nickname TEXT, customid TEXT, url1 TEXT);
             CREATE TABLE TIMMessageORM (localcreatedat INTEGER, sender INTEGER, content TEXT,
                 belongingConversationIdentifier TEXT);
             INSERT INTO AwemeContactsV5 (uid, nickname) VALUES ('200', 'Robin');
             INSERT INTO TIMMessageORM VALUES (1700000000000, 200, '{\"text\":\"hi from tiktok\"}', 'conv1');
             INSERT INTO TIMMessageORM VALUES (1700000100000, 999, '{\"text\":\"sent by me\"}', 'conv1');",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_tiktok_messages_and_direction() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        // account_id 999 = the local user (from the path).
        let msgs = parse(&db, "ChatFiles/999/AwemeIM.db").unwrap();
        assert_eq!(msgs.len(), 2);

        let incoming = &msgs[0];
        assert_eq!(incoming.chat_key, "conv1");
        assert_eq!(incoming.body.as_deref(), Some("hi from tiktok"));
        assert_eq!(incoming.sender_name.as_deref(), Some("Robin")); // uid 200 → nickname
        assert_eq!(incoming.timestamp, Some(1_700_000_000)); // ms → s
        assert!(!incoming.is_from_me);
        assert!(msgs[1].is_from_me && msgs[1].body.as_deref() == Some("sent by me"));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "TikTok", msgs, &mut report).unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 2);
    }
}
