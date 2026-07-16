//! Instagram Direct (DMs) native chat module.
//!
//! Schema facts (learned from iLEAPP `instagramThreads.py`, written fresh —
//! provenance reference, §10):
//! - DB: `Library/Application Support/DirectSQLiteDatabase/*.db` in the Instagram
//!   app's Data container.
//! - `THREADS(THREAD_ID, VIEWER_ID, METADATA)` — `VIEWER_ID` is the local user's
//!   pk; `METADATA` is an NSKeyedArchiver blob listing the thread's users.
//! - `MESSAGES(THREAD_ID, ARCHIVE)` — `ARCHIVE` is an NSKeyedArchiver blob; after
//!   [`crate::nska`] resolves it:
//!   - `["IGDirectPublishedMessageMetadata*metadata"]` → `NSString*senderPk`,
//!     `NSString*threadId`, `NSDate*serverTimestamp`.
//!   - `["IGDirectPublishedMessageContent*content"]["NSString*string"]` → text.
//!
//! NOTE: unvalidated against a real Instagram backup — the archive key paths come
//! from the iLEAPP reference. Kept behind the iLEAPP fallback until confirmed.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use plist::Value;
use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::{nska, Result};

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "instagram",
    service: "Instagram",
    // Instagram 1:1 threads use numeric THREAD_IDs, so numeric-id ⇒ group is wrong.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%Application Support/DirectSQLiteDatabase/%.db")?;
    hits.retain(|e| e.relative_path.ends_with(".db"));
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

/// Map a resolved NSDate `Value` to Unix seconds.
fn date_secs(v: &Value) -> Option<i64> {
    let d = v.as_date()?;
    SystemTime::from(d)
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Build `pk → display name` from every thread's METADATA archive
/// (`NSArray<IGUser *>*users` → each user's `userDict.{pk, full_name}`).
fn build_userdict(conn: &Connection) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT METADATA FROM THREADS WHERE METADATA IS NOT NULL")
    else {
        return out;
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0)) else {
        return out;
    };
    for blob in rows.flatten() {
        let Ok(v) = nska::resolve(&blob) else {
            continue;
        };
        let Some(users) = v
            .as_dictionary()
            .and_then(|d| d.get("NSArray<IGUser *>*users"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for user in users {
            let Some(ud) = user.as_dictionary() else {
                continue;
            };
            // The pk + name live under a nested userDict (or on the user itself).
            let inner = ud
                .get("NSDictionary<NSString *, id>*userDict")
                .and_then(Value::as_dictionary);
            let pk = inner
                .and_then(|d| d.get("pk"))
                .or_else(|| ud.get("pk"))
                .and_then(scalar_string);
            let name = inner
                .and_then(|d| d.get("full_name"))
                .or_else(|| ud.get("fullName"))
                .or_else(|| ud.get("userName"))
                .and_then(Value::as_string)
                .map(str::to_string);
            if let (Some(pk), Some(name)) = (pk, name) {
                out.insert(pk, name);
            }
        }
    }
    out
}

/// A pk may be stored as a string or an integer; normalize to a string.
fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Same, for a SQLite column value (TEXT or INTEGER → String).
fn scalar_string_ref(v: rusqlite::types::ValueRef) -> Option<String> {
    match v {
        rusqlite::types::ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        rusqlite::types::ValueRef::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "MESSAGES")? || !table_exists(&conn, "THREADS")? {
        return Ok(Vec::new());
    }
    let users = build_userdict(&conn);

    // LEFT JOIN so a message whose THREAD_ID has no THREADS row is still kept
    // (viewer_id None → treated as received) rather than silently dropped.
    let mut stmt = conn.prepare(
        "SELECT m.THREAD_ID, m.ARCHIVE, t.VIEWER_ID
         FROM MESSAGES m LEFT JOIN THREADS t ON t.THREAD_ID = m.THREAD_ID
         WHERE m.ARCHIVE IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let thread_id: String = r
            .get_ref(0)
            .ok()
            .and_then(scalar_string_ref)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        // ARCHIVE is a BLOB; a non-blob value is skipped (not fatal to the parse).
        let archive: Vec<u8> = match r.get_ref(1) {
            Ok(rusqlite::types::ValueRef::Blob(b)) => b.to_vec(),
            _ => continue,
        };
        // VIEWER_ID (an Instagram pk) may be TEXT or INTEGER — read either.
        let viewer_id: Option<String> = r.get_ref(2).ok().and_then(scalar_string_ref);

        // One unparseable/unsupported archive must not abort the whole conversation
        // import — skip the row (like build_userdict does), not the parse.
        let Ok(v) = nska::resolve(&archive) else {
            continue;
        };
        let Some(root) = v.as_dictionary() else {
            continue;
        };
        let metadata = root
            .get("IGDirectPublishedMessageMetadata*metadata")
            .and_then(Value::as_dictionary);
        let content = root
            .get("IGDirectPublishedMessageContent*content")
            .and_then(Value::as_dictionary);

        let sender_pk = metadata
            .and_then(|m| m.get("NSString*senderPk"))
            .and_then(scalar_string);
        let timestamp = metadata
            .and_then(|m| m.get("NSDate*serverTimestamp"))
            .and_then(date_secs);
        let body = content
            .and_then(|c| c.get("NSString*string"))
            .and_then(Value::as_string)
            .map(str::to_string);
        // A message with neither text nor a resolvable sender is a system/among
        // -unsupported payload — skip it rather than emit an empty row.
        if body.is_none() && sender_pk.is_none() {
            continue;
        }

        let is_from_me = match (&sender_pk, &viewer_id) {
            (Some(s), Some(v)) => s == v,
            _ => false,
        };
        let sender_name = sender_pk.as_ref().and_then(|pk| users.get(pk).cloned());

        out.push(AppMessage {
            attachments: Vec::new(),
            chat_key: thread_id,
            chat_name: None, // derived from the peer
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { sender_name },
            sender_handle: None,
            sender_id: sender_pk,
            has_attachment: false,
            kind: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Uid};

    /// Build a tiny NSKeyedArchiver message archive: root object references a
    /// `metadata` object (senderPk) and a `content` object (string). Mirrors the
    /// `IG*` key layout the parser navigates.
    fn message_archive(sender_pk: &str, text: &str) -> Vec<u8> {
        // $objects: [null, root, metaKey, meta, "senderPk", pk, contentKey, content,
        //            "string", text]
        let mut root = Dictionary::new();
        root.insert(
            "NS.keys".into(),
            Value::Array(vec![Value::Uid(Uid::new(2)), Value::Uid(Uid::new(6))]),
        );
        root.insert(
            "NS.objects".into(),
            Value::Array(vec![Value::Uid(Uid::new(3)), Value::Uid(Uid::new(7))]),
        );
        let mut meta = Dictionary::new();
        meta.insert(
            "NS.keys".into(),
            Value::Array(vec![Value::Uid(Uid::new(4))]),
        );
        meta.insert(
            "NS.objects".into(),
            Value::Array(vec![Value::Uid(Uid::new(5))]),
        );
        let mut content = Dictionary::new();
        content.insert(
            "NS.keys".into(),
            Value::Array(vec![Value::Uid(Uid::new(8))]),
        );
        content.insert(
            "NS.objects".into(),
            Value::Array(vec![Value::Uid(Uid::new(9))]),
        );
        let objects = Value::Array(vec![
            Value::String("$null".into()),
            Value::Dictionary(root),
            Value::String("IGDirectPublishedMessageMetadata*metadata".into()),
            Value::Dictionary(meta),
            Value::String("NSString*senderPk".into()),
            Value::String(sender_pk.into()),
            Value::String("IGDirectPublishedMessageContent*content".into()),
            Value::Dictionary(content),
            Value::String("NSString*string".into()),
            Value::String(text.into()),
        ]);
        let mut top = Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));
        let mut archive = Dictionary::new();
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$top".into(), Value::Dictionary(top));
        archive.insert("$objects".into(), objects);
        let mut buf = Vec::new();
        Value::Dictionary(archive)
            .to_writer_binary(&mut buf)
            .unwrap();
        buf
    }

    #[test]
    fn parses_instagram_dm_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("direct.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE THREADS (THREAD_ID TEXT, VIEWER_ID TEXT, METADATA BLOB);
             CREATE TABLE MESSAGES (THREAD_ID TEXT, ARCHIVE BLOB);
             INSERT INTO THREADS (THREAD_ID, VIEWER_ID, METADATA) VALUES ('t1', '100', NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO MESSAGES (THREAD_ID, ARCHIVE) VALUES ('t1', ?1)",
            [message_archive("200", "hey from insta")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO MESSAGES (THREAD_ID, ARCHIVE) VALUES ('t1', ?1)",
            [message_archive("100", "reply from me")],
        )
        .unwrap();
        drop(conn);

        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 2);
        let incoming = &msgs[0];
        assert_eq!(incoming.chat_key, "t1");
        assert_eq!(incoming.body.as_deref(), Some("hey from insta"));
        assert_eq!(incoming.sender_id.as_deref(), Some("200"));
        assert!(!incoming.is_from_me);
        // Viewer 100 → from me.
        assert!(msgs
            .iter()
            .any(|m| m.is_from_me && m.body.as_deref() == Some("reply from me")));
    }
}
