//! Native Messages parser (Phase 2): reads a decrypted `sms.db` directly into the
//! cache `threads` + `messages`, so Messages can be materialized natively —
//! without iLEAPP's eager whole-backup pass. Locate + decrypt `sms.db` via the
//! [`crate::manifest::ManifestIndex`], then call [`parse_messages`].
//!
//! This produces the same cache shape as the iLEAPP path
//! ([`crate::normalize`]): threads keyed by `chat.ROWID`, group names/members
//! from `chat`/`chat_handle_join`, per-message sender from `handle`, and
//! Apple-absolute timestamps converted to Unix seconds.
//!
//! provenance: reference (own implementation) from the iMessage `chat.db`/`sms.db`
//! schema (the same schema `chats.rs` reads for metadata).

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Apple absolute time counts from 2001-01-01 UTC. iOS 11+ stores nanoseconds;
/// older backups store seconds. Convert to Unix epoch seconds; 0 → None.
const MAC_EPOCH: i64 = 978_307_200;
fn mac_to_unix(date: i64) -> Option<i64> {
    if date == 0 {
        return None;
    }
    // Nanoseconds if the value is far larger than any plausible seconds count.
    let secs = if date.abs() > 1_000_000_000_000 {
        date / 1_000_000_000
    } else {
        date
    };
    Some(secs + MAC_EPOCH)
}

struct Chat {
    identifier: String,
    display_name: Option<String>,
    service: Option<String>,
    participants: Vec<String>,
}

/// Parse a decrypted `sms.db` into the cache's `threads` + `messages`. Idempotent
/// only against a fresh cache (it appends rows, like the normalizer).
pub fn parse_messages(sms_db: &Path, cache: &CacheDb, report: &mut ImportReport) -> Result<()> {
    let src = Connection::open_with_flags(sms_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // handle.ROWID → phone/email.
    let handles: HashMap<i64, String> = {
        let mut stmt = src.prepare("SELECT ROWID, id FROM handle")?;
        let map = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
            .collect();
        map
    };

    // chat.ROWID → metadata (+ participants from chat_handle_join).
    let mut chats: HashMap<i64, Chat> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT ROWID, chat_identifier, display_name, service_name FROM chat")?;
        for row in stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .flatten()
        {
            let (id, ident, name, service) = row;
            chats.insert(
                id,
                Chat {
                    identifier: ident.unwrap_or_else(|| id.to_string()),
                    display_name: name.filter(|s| !s.trim().is_empty()),
                    service,
                    participants: Vec::new(),
                },
            );
        }
        let mut pstmt = src.prepare(
            "SELECT chj.chat_id, h.id FROM chat_handle_join chj JOIN handle h ON h.ROWID = chj.handle_id",
        )?;
        for row in pstmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
        {
            let (cid, h) = row;
            if let Some(c) = chats.get_mut(&cid) {
                if !h.trim().is_empty() && !c.participants.contains(&h) {
                    c.participants.push(h);
                }
            }
        }
    }

    let conn = cache.conn();
    // One transaction — messages run to tens of thousands of rows.
    let tx = conn.unchecked_transaction()?;

    // Messages in chat order (grouped), then time. Skip pure "action" items
    // (group renames, joins) which carry no text and no attachment.
    let mut mstmt = src.prepare(
        "SELECT cmj.chat_id, m.text, m.is_from_me, m.date, m.handle_id, m.cache_has_attachments
         FROM message m
         JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
         WHERE m.text IS NOT NULL OR m.cache_has_attachments <> 0
         ORDER BY cmj.chat_id, m.date, m.ROWID",
    )?;
    let mut rows = mstmt.query([])?;

    let mut current_chat: Option<i64> = None;
    let mut thread_id: i64 = 0;
    let mut is_group = false;
    while let Some(r) = rows.next()? {
        let chat_id: i64 = r.get(0)?;
        let text: Option<String> = r.get(1)?;
        let is_from_me = r.get::<_, i64>(2)? != 0;
        let date: i64 = r.get(3)?;
        let handle_id: i64 = r.get::<_, Option<i64>>(4)?.unwrap_or(0);
        let has_attachment = r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0;

        if current_chat != Some(chat_id) {
            let chat = chats.get(&chat_id);
            let participants = chat.map(|c| c.participants.clone()).unwrap_or_default();
            is_group = participants.len() > 1;
            // Group → its name; 1:1 → the peer's identifier (the UI resolves it to
            // a saved contact). Thread identifier is the chat ROWID, matching the
            // iLEAPP path.
            let display_name = if is_group {
                chat.and_then(|c| c.display_name.clone())
            } else {
                chat.map(|c| c.identifier.clone())
            };
            tx.execute(
                "INSERT INTO threads (identifier, display_name, service, last_message_at, message_count, participants_json)
                 VALUES (?1, ?2, ?3, NULL, 0, ?4)",
                rusqlite::params![
                    chat_id.to_string(),
                    display_name,
                    chat.and_then(|c| c.service.clone()),
                    serde_json::to_string(&participants).unwrap_or_else(|_| "[]".into()),
                ],
            )?;
            thread_id = tx.last_insert_rowid();
            current_chat = Some(chat_id);
            report.threads += 1;
        }

        // Sender: the real member for an incoming group message; the peer for a
        // 1:1; None (the device owner) for outgoing.
        let sender = if is_from_me {
            None
        } else {
            handles.get(&handle_id).cloned().or_else(|| {
                if is_group {
                    None
                } else {
                    chats.get(&chat_id).map(|c| c.identifier.clone())
                }
            })
        };
        tx.execute(
            "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at, has_attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                thread_id,
                sender,
                is_from_me as i64,
                text,
                mac_to_unix(date),
                has_attachment as i64,
            ],
        )?;
        report.messages += 1;
    }
    drop(rows);
    drop(mstmt);

    // Denormalize the per-thread counters (only the threads we just wrote).
    tx.execute_batch(
        "UPDATE threads SET
           message_count = (SELECT COUNT(*) FROM messages WHERE thread_id = threads.id),
           last_message_at = (SELECT MAX(sent_at) FROM messages WHERE thread_id = threads.id)
         WHERE service IS NULL OR service NOT IN ('TikTok','WhatsApp','Telegram')",
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sms_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("sms.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT, service_name TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111'), (2,'+15550002222');
             -- A 1:1 chat with one peer, and a group chat with a name + two peers.
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat VALUES (20,'chat99','Hiking Crew','iMessage');
             INSERT INTO chat_handle_join VALUES (10,1),(20,1),(20,2);
             -- date is Apple-absolute nanoseconds; unix 1_700_000_000 = 721692800000000000.
             INSERT INTO message VALUES (100,'hey there',0,721692800000000000,1,0);
             INSERT INTO message VALUES (101,'hi back',1,721692860000000000,0,0);
             INSERT INTO message VALUES (200,'who is in?',0,721700000000000000,2,0);
             -- an attachment-only message (NULL text) is kept.
             INSERT INTO message VALUES (201,NULL,1,721700060000000000,0,1);
             -- a pure action item (NULL text, no attachment) is skipped.
             INSERT INTO message VALUES (202,NULL,0,721700120000000000,1,0);
             INSERT INTO chat_message_join VALUES (10,100),(10,101),(20,200),(20,201),(20,202);",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_threads_and_messages_from_sms_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_sms_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        parse_messages(&db, &cache, &mut report).unwrap();

        assert_eq!(report.threads, 2);
        assert_eq!(report.messages, 4); // action item (202) skipped

        let c = cache.conn();
        // Group thread named, 1:1 thread shows the peer identifier.
        let (group_name, group_count): (Option<String>, i64) = c
            .query_row(
                "SELECT display_name, message_count FROM threads WHERE identifier = '20'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(group_name.as_deref(), Some("Hiking Crew"));
        assert_eq!(group_count, 2);
        let direct_name: Option<String> = c
            .query_row(
                "SELECT display_name FROM threads WHERE identifier = '10'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(direct_name.as_deref(), Some("+15550001111"));

        // Apple-time → Unix seconds, and sender attribution for the group.
        let (sent_at, sender): (i64, Option<String>) = c
            .query_row(
                "SELECT m.sent_at, m.sender FROM messages m JOIN threads t ON t.id = m.thread_id
                 WHERE t.identifier = '20' AND m.body = 'who is in?'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sent_at, 1_700_007_200); // 721700000000000000 ns → Unix
        assert_eq!(sender.as_deref(), Some("+15550002222"));

        // Attachment-only message kept, flagged, outgoing.
        let (has_att, from_me): (i64, i64) = c
            .query_row(
                "SELECT has_attachments, is_from_me FROM messages WHERE body IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(has_att, 1);
        assert_eq!(from_me, 1);
    }
}
