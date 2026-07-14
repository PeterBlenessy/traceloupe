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
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::crypto::{self, BackupDecryptor};
use crate::manifest::ManifestIndex;
use crate::normalize::ImportReport;
use crate::Result;

/// Context for resolving a message's attachments to their files in the backup.
/// When `None`, attachment rows aren't written (e.g. a caller with no Manifest,
/// or tests) — the messages themselves still parse.
pub struct AttachmentSource<'a> {
    pub index: &'a ManifestIndex,
    /// `Some` for an encrypted backup — attachment blobs are then ciphertext and
    /// their wrapped keys are stored for on-demand decryption at view time.
    pub decryptor: Option<&'a BackupDecryptor>,
}

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

/// SQL predicate for the threads this parser owns — iMessage/SMS, i.e. not the
/// third-party app-chat services. Scopes a `replace` re-import's deletes so
/// TikTok/WhatsApp/Telegram conversations are left intact. A fixed string (no user
/// input), safe to interpolate.
const NATIVE_THREADS: &str = "service IS NULL OR service NOT IN ('TikTok','WhatsApp','Telegram')";

/// Parse a decrypted `sms.db` into the cache's `threads` + `messages`.
///
/// With `replace = false` it appends (for a fresh cache, like the normalizer).
/// With `replace = true` it first deletes this parser's existing rows — child
/// `attachments`, then `messages`, then `threads` (that order satisfies the
/// foreign key) — **inside the same transaction as the re-insert**, so a partial
/// re-import is atomic: a parse failure rolls the deletes back too.
///
/// When `attachments` is `Some`, each message's attachments are resolved to their
/// backup files and written to the `attachments` table (with a wrapped key for
/// encrypted backups, so they decrypt on demand at view time).
pub fn parse_messages(
    sms_db: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
    attachments: Option<&AttachmentSource>,
) -> Result<()> {
    let src = Connection::open_with_flags(sms_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // Attachment tables (only when a caller can resolve files). `attachment.filename`
    // is the on-device path; `transfer_name` the display name.
    let (att_meta, msg_atts) = if attachments.is_some() {
        load_attachments(&src)?
    } else {
        (HashMap::new(), HashMap::new())
    };

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

    // Replace mode: clear this parser's prior rows before re-inserting, in the
    // same transaction. Attachments first (they FK-reference messages), then
    // messages, then threads.
    if replace {
        tx.execute(
            &format!(
                "DELETE FROM attachments WHERE message_id IN
                   (SELECT id FROM messages WHERE thread_id IN
                     (SELECT id FROM threads WHERE {NATIVE_THREADS}))"
            ),
            [],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM messages WHERE thread_id IN
                   (SELECT id FROM threads WHERE {NATIVE_THREADS})"
            ),
            [],
        )?;
        tx.execute(&format!("DELETE FROM threads WHERE {NATIVE_THREADS}"), [])?;
    }

    // Messages in chat order (grouped), then time. Skip pure "action" items
    // (group renames, joins) which carry no text and no attachment.
    let mut mstmt = src.prepare(
        "SELECT cmj.chat_id, m.text, m.is_from_me, m.date, m.handle_id, m.cache_has_attachments, m.ROWID
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
        let msg_rowid: i64 = r.get(6)?;

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
        let message_id = tx.last_insert_rowid();
        report.messages += 1;

        // Attachment rows: resolve each to its backup file so the UI can serve it.
        if let (Some(src), Some(ids)) = (attachments, msg_atts.get(&msg_rowid)) {
            for aid in ids {
                let Some(a) = att_meta.get(aid) else { continue };
                let resolved = a.path.as_deref().and_then(|p| resolve_attachment(src, p));
                let (local_path, decrypt_key, plain_size) = match resolved {
                    Some((path, key, size)) => {
                        (Some(path.to_string_lossy().into_owned()), key, size)
                    }
                    None => (None, None, None),
                };
                // Display name: transfer_name, else the file's basename.
                let filename = a.filename.clone().or_else(|| {
                    a.path
                        .as_deref()
                        .and_then(|p| p.rsplit('/').next())
                        .map(str::to_string)
                });
                tx.execute(
                    "INSERT INTO attachments
                        (message_id, filename, mime_type, local_path, decrypt_key, plain_size)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        message_id,
                        filename,
                        a.mime,
                        local_path,
                        decrypt_key,
                        plain_size,
                    ],
                )?;
            }
        }
    }
    drop(rows);
    drop(mstmt);

    // Denormalize the per-thread counters (only the threads we just wrote).
    tx.execute_batch(&format!(
        "UPDATE threads SET
           message_count = (SELECT COUNT(*) FROM messages WHERE thread_id = threads.id),
           last_message_at = (SELECT MAX(sent_at) FROM messages WHERE thread_id = threads.id)
         WHERE {NATIVE_THREADS}"
    ))?;
    tx.commit()?;
    Ok(())
}

/// One row of the `attachment` table we care about.
struct Att {
    /// On-device path (`attachment.filename`), e.g. `~/Library/SMS/Attachments/…`.
    path: Option<String>,
    /// Display name (`attachment.transfer_name`).
    filename: Option<String>,
    mime: Option<String>,
}

/// Read `attachment` (by ROWID) and `message_attachment_join` (message ROWID →
/// attachment ROWIDs) so the main loop can attach files to each message.
#[allow(clippy::type_complexity)]
fn load_attachments(src: &Connection) -> Result<(HashMap<i64, Att>, HashMap<i64, Vec<i64>>)> {
    // A minimal/older sms.db may lack these tables — then there are simply no
    // attachments (not an error).
    if !table_exists(src, "attachment") || !table_exists(src, "message_attachment_join") {
        return Ok((HashMap::new(), HashMap::new()));
    }
    let mut meta: HashMap<i64, Att> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT ROWID, filename, transfer_name, mime_type FROM attachment")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Att {
                    path: r.get::<_, Option<String>>(1)?,
                    filename: r
                        .get::<_, Option<String>>(2)?
                        .filter(|s| !s.trim().is_empty()),
                    mime: r.get::<_, Option<String>>(3)?,
                },
            ))
        })?;
        for row in rows.flatten() {
            meta.insert(row.0, row.1);
        }
    }
    let mut joins: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT message_id, attachment_id FROM message_attachment_join")?;
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
            .flatten()
        {
            joins.entry(row.0).or_default().push(row.1);
        }
    }
    Ok((meta, joins))
}

/// Resolve an on-device attachment path to its backup blob (+ wrapped key/size
/// for an encrypted backup). None if the file isn't in the backup.
fn resolve_attachment(
    src: &AttachmentSource,
    on_device_path: &str,
) -> Option<(PathBuf, Option<Vec<u8>>, Option<u64>)> {
    let rel = normalize_attachment_path(on_device_path)?;
    let entry = src.index.find("MediaDomain", &rel).ok().flatten()?;
    let path = src.index.blob_path(&entry.file_id);
    let (key, size) = if src.decryptor.is_some() {
        match crypto::file_key_field(&entry.file_blob) {
            Ok((k, s)) => (Some(k), s),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };
    Some((path, key, size))
}

/// Map an on-device SMS attachment path to its `MediaDomain` relativePath. iOS
/// stores these as `~/Library/SMS/Attachments/…` (or an absolute variant); the
/// backup keys them by `Library/SMS/Attachments/…`.
fn normalize_attachment_path(p: &str) -> Option<String> {
    p.find("Library/SMS/Attachments/")
        .map(|i| p[i..].to_string())
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
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

        parse_messages(&db, &cache, &mut report, false, None).unwrap();

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

    #[test]
    fn replace_clears_prior_rows_including_attachment_children() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_sms_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();

        // Simulate a prior iLEAPP import: an iMessage thread + message with an
        // attachment child, plus an app-chat (TikTok) thread that must survive.
        cache
            .conn()
            .execute_batch(
                "INSERT INTO threads (id, identifier, service) VALUES (900, 'old', 'iMessage');
                 INSERT INTO threads (id, identifier, service) VALUES (901, 'tt', 'TikTok');
                 INSERT INTO messages (id, thread_id, is_from_me, body, has_attachments)
                   VALUES (9000, 900, 0, 'old msg', 1);
                 INSERT INTO messages (id, thread_id, is_from_me, body) VALUES (9001, 901, 0, 'tiktok');
                 INSERT INTO attachments (message_id, filename) VALUES (9000, 'old.jpg');",
            )
            .unwrap();

        // replace=true must delete the attachment child before the message
        // (FK ON, no cascade) — a bare message delete would fail here.
        let mut report = ImportReport::default();
        parse_messages(&db, &cache, &mut report, true, None).unwrap();

        let c = cache.conn();
        // Old iMessage thread + message + attachment gone; fresh ones inserted.
        let old: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE identifier = 'old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 0);
        let orphan_att: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM attachments WHERE message_id = 9000",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_att, 0);
        assert_eq!(report.threads, 2); // the two chats from the fresh sms.db
                                       // The app-chat thread is untouched by a native-messages replace.
        let tiktok: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE service = 'TikTok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tiktok, 1);
    }
}
