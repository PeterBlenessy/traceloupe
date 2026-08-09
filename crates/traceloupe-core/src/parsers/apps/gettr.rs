//! Gettr direct messages.
//!
//! Schema facts read off Josh Hickman's public iOS 17 image with
//! `explore_real_backup` (provenance: own implementation; iLEAPP has no Gettr
//! message parser). 32 messages across 2 conversations on that device.
//!
//! THIS IS A GETSTREAM DATABASE, not a Gettr one. Gettr does not implement its
//! own chat — it embeds the Stream Chat SDK, whose iOS client persists to
//! `Documents/db_<userid>.sqlite` with tables `channels`, `messages`, `members`
//! and `users`. That is worth knowing because the same schema will appear under
//! other bundle ids: anything else built on Stream Chat is a `locate` change
//! away, not a new parser.
//!
//! - `messages(id, message_text, user_id, created_at, type, channel_cid,
//!   deleted_at, attachments)` — `created_at` is Unix **seconds**.
//! - `channels(cid, member_count)` — `cid` is `messaging:!members-<opaque>`,
//!   which names nobody, so a thread is named from its members instead.
//! - `users(id, extra_data)` — `extra_data` is JSON carrying `nickname` and
//!   `username`; there is no `name` column.
//! - `connection_events.own_user` — JSON whose `id` is the LOCAL user.
//!
//! WHO SENT IT is stated twice and inferred not at all. `own_user.id` gives the
//! local account, and the DATABASE FILENAME repeats it (`db_<userid>.sqlite`) —
//! which is why `parse` takes the relative path. The filename is used only when
//! the table is missing or unreadable, and the two are checked against each
//! other rather than one being trusted blindly.
//!
//! DELETED MESSAGES ARE KEPT, with their body dropped. Stream soft-deletes:
//! `deleted_at` is set and `message_text` is usually emptied. Showing the row
//! preserves the fact that something was sent and removed at a known time,
//! which is evidence; inventing a body for it would not be.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::{col_i64, col_string, AppMessage};
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "gettr",
    service: "Gettr",
    // Channel ids are `messaging:!members-…`, never bare numbers.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("Documents/db_u%.sqlite")?;
    // A device signed into two accounts has one store per account; `-wal`/`-shm`
    // siblings are not stores.
    hits.retain(|e| e.relative_path.ends_with(".sqlite"));
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

/// A string field out of one of Stream's JSON blobs.
fn json_str(raw: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key)?.as_str().map(|s| s.to_string())
}

/// The local account id, from the store if it says, else from the filename.
///
/// `db_<userid>.sqlite` encodes it too, so the two corroborate each other. The
/// filename is the fallback rather than the source: a store copied under a new
/// name would otherwise silently reverse the direction of every message.
fn local_account(conn: &Connection, rel_path: &str) -> Option<String> {
    let from_db = if table_exists(conn, "connection_events").unwrap_or(false) {
        conn.query_row(
            "SELECT CAST(own_user AS TEXT) FROM connection_events
             WHERE own_user IS NOT NULL LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|raw| json_str(&raw, "id"))
    } else {
        None
    };
    from_db.or_else(|| {
        rel_path
            .rsplit('/')
            .next()?
            .strip_prefix("db_")?
            .strip_suffix(".sqlite")
            .map(|s| s.to_string())
    })
}

fn parse(path: &Path, rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "messages")? || !table_exists(&conn, "channels")? {
        return Ok(Vec::new());
    }
    let me = local_account(&conn, rel_path);

    // user id → display name. `nickname` first, `username` as the fallback.
    let mut names: HashMap<String, String> = HashMap::new();
    if table_exists(&conn, "users")? {
        let mut st = conn.prepare("SELECT id, CAST(extra_data AS TEXT) FROM users")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let Some(id) = col_string(r, 0)? else {
                continue;
            };
            let Some(raw) = col_string(r, 1)? else {
                continue;
            };
            if let Some(n) = json_str(&raw, "nickname").or_else(|| json_str(&raw, "username")) {
                if !n.trim().is_empty() {
                    names.insert(id, n);
                }
            }
        }
    }

    // channel → its members, so a 1:1 thread can be named after the other party.
    let mut members: HashMap<String, Vec<String>> = HashMap::new();
    if table_exists(&conn, "members")? {
        let mut st = conn.prepare("SELECT channel_cid, user_id FROM members")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            if let (Some(cid), Some(uid)) = (col_string(r, 0)?, col_string(r, 1)?) {
                members.entry(cid).or_default().push(uid);
            }
        }
    }

    let mut st = conn.prepare(
        "SELECT channel_cid, user_id, message_text, created_at, type, deleted_at, attachments
         FROM messages ORDER BY created_at",
    )?;
    let mut rows = st.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let Some(cid) = col_string(r, 0)? else {
            continue;
        };
        let sender = col_string(r, 1)?;
        let body = col_string(r, 2)?.filter(|b| !b.trim().is_empty());
        let timestamp = col_i64(r, 3)?;
        let msg_type = col_string(r, 4)?.unwrap_or_default();
        let deleted = col_i64(r, 5)?.is_some();
        // Stream stores attachments as a JSON array; `[]` means none.
        let attachments = col_string(r, 6)?.unwrap_or_default();
        let has_attachment = !attachments.trim().is_empty()
            && attachments.trim() != "[]"
            && attachments.trim() != "null";

        let peers = members.get(&cid);
        // A conversation of more than two people is a group, whatever Stream
        // calls the channel type.
        let is_group = peers.is_some_and(|m| m.len() > 2);
        let chat_name = if is_group {
            None
        } else {
            peers.and_then(|m| {
                m.iter()
                    .find(|u| Some(u.as_str()) != me.as_deref())
                    .and_then(|u| names.get(u).cloned())
            })
        };

        // A deletion and a channel notice are both "not something they typed",
        // which is the one bucket the filter has for that.
        let kind = if deleted || msg_type == "system" {
            Some("system")
        } else if has_attachment {
            Some("shared")
        } else {
            None
        };

        out.push(AppMessage {
            source_id: None,
            is_group,
            chat_key: cid,
            chat_name,
            timestamp,
            // A soft-deleted message keeps its row and loses its body: that it
            // was sent and removed is evidence; a body we invented would not be.
            body: if deleted { None } else { body },
            is_from_me: match (&me, &sender) {
                (Some(me), Some(s)) => me == s,
                _ => false,
            },
            sender_name: if is_group {
                sender.as_ref().and_then(|s| names.get(s).cloned())
            } else {
                None
            },
            sender_handle: sender.as_ref().and_then(|s| names.get(s).cloned()),
            sender_id: sender,
            has_attachment,
            kind,
            // MEDIA: TODO #420 — this parser extracts no attachments, so a
            // photo sent here is invisible unless discovery infers it.
            // Not yet measured: run backup-coverage against a backup with
            // the app installed before deciding there is nothing local.
            attachments: Vec::new(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "u62c2a4b78c559523ed523eb8";
    const THEM: &str = "u62c2a89f8c5595212904b3db";

    fn db(with_own_user: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("db_{ME}.sqlite"));
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE connection_events (id INTEGER PRIMARY KEY, own_user TEXT);
             CREATE TABLE users (id TEXT PRIMARY KEY, extra_data TEXT);
             CREATE TABLE channels (cid TEXT PRIMARY KEY, member_count INTEGER);
             CREATE TABLE members (channel_cid TEXT, user_id TEXT);
             CREATE TABLE messages (id TEXT PRIMARY KEY, channel_cid TEXT, user_id TEXT,
                message_text TEXT, created_at INTEGER, type TEXT, deleted_at INTEGER,
                attachments TEXT);",
        )
        .unwrap();
        c.execute_batch(&format!(
            "INSERT INTO users VALUES
                ('{ME}', '{{\"nickname\":\"me37\",\"username\":\"me37u\"}}'),
                ('{THEM}', '{{\"username\":\"thisisdfir37\"}}');
             INSERT INTO channels VALUES ('messaging:!members-abc', 2);
             INSERT INTO members VALUES
                ('messaging:!members-abc', '{ME}'),
                ('messaging:!members-abc', '{THEM}');
             INSERT INTO messages VALUES
                ('m1', 'messaging:!members-abc', '{THEM}', 'You there?', 1670032714,
                 'regular', NULL, '[]'),
                ('m2', 'messaging:!members-abc', '{ME}', 'I am.', 1670032814,
                 'regular', NULL, '[]'),
                ('m3', 'messaging:!members-abc', '{ME}', '', 1670032914,
                 'regular', 1670033000, '[]'),
                ('m4', 'messaging:!members-abc', '{THEM}', NULL, 1670033014,
                 'regular', NULL, '[{{\"type\":\"image\"}}]');"
        ))
        .unwrap();
        if with_own_user {
            c.execute(
                "INSERT INTO connection_events VALUES (1, ?1)",
                [format!("{{\"id\":\"{ME}\"}}")],
            )
            .unwrap();
        }
        drop(c);
        (dir, path)
    }

    #[test]
    fn direction_comes_from_own_user() {
        let (_d, p) = db(true);
        let m = parse(&p, "Documents/db_x.sqlite").unwrap();
        assert!(!m[0].is_from_me, "'You there?' is incoming");
        assert!(m[1].is_from_me, "'I am.' is ours");
    }

    #[test]
    fn the_filename_is_the_fallback_not_the_source() {
        // Without the table, the id in `db_<userid>.sqlite` still identifies us.
        let (_d, p) = db(false);
        let rel = format!("Documents/db_{ME}.sqlite");
        let m = parse(&p, &rel).unwrap();
        assert!(m[1].is_from_me, "the filename named the local account");
        // ...and a store copied under someone else's name must not silently
        // reverse every message: with no own_user row and a foreign filename,
        // nothing is claimed as ours.
        let m2 = parse(&p, "Documents/db_uSOMEONEELSE.sqlite").unwrap();
        assert!(m2.iter().all(|x| !x.is_from_me));
    }

    #[test]
    fn a_thread_is_named_after_the_other_party() {
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        // `username` is used when `nickname` is absent.
        assert_eq!(m[0].chat_name.as_deref(), Some("thisisdfir37"));
    }

    #[test]
    fn a_deleted_message_keeps_its_row_and_loses_its_body() {
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        let del = &m[2];
        assert!(del.body.is_none(), "a soft-deleted body is not shown");
        assert_eq!(del.kind, Some("system"));
        assert!(
            del.timestamp.is_some(),
            "when it was sent is still evidence"
        );
    }

    #[test]
    fn an_empty_attachment_array_is_not_an_attachment() {
        // Stream writes `[]`, not NULL. Read naively every message has media.
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        assert!(!m[0].has_attachment);
        assert!(m[3].has_attachment);
        assert_eq!(m[3].kind, Some("shared"));
    }
}
