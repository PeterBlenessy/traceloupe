//! MEGA chat (Karere) native module.
//!
//! Schema facts read off Josh Hickman's public iOS 17 image with
//! `explore_real_backup` (provenance: own implementation; iLEAPP has no MEGA
//! chat parser). 162 messages across 7 chats on that device.
//!
//! - DB: `GroupSupport/karere-<account>.db` in `AppDomainGroup-group.mega.ios`.
//!   Karere is MEGA's chat protocol; the `<account>` segment is a per-account
//!   hash, so the path cannot be named literally.
//! - `history(chatid, userid, ts, type, data, is_encrypted)` — one row per
//!   message. `ts` is Unix **seconds**.
//! - `chats(chatid, peer, title, mode)` — `peer` is the other party in a 1:1
//!   chat and `-1` in a group.
//! - `contacts(userid, email)` — MEGA identifies people by **e-mail address**,
//!   not by a display name, so that is what a thread is named after.
//! - `chat_peers(chatid, userid)` — group membership.
//! - `vars(name, value)` — `my_handle` is the LOCAL user's id and `my_email`
//!   their address.
//!
//! WHO SENT IT is not inferred here, unlike LINE: `vars.my_handle` states it.
//! A message whose `userid` equals it is outgoing. If that row is missing,
//! direction is left unknown (`is_from_me = false`) rather than guessed —
//! a wrong attribution is worse than an absent one.
//!
//! A ZERO-KNOWLEDGE SERVICE WITH READABLE MESSAGES is not a contradiction. MEGA
//! encrypts chat end-to-end, which is a claim about what the SERVER holds; the
//! client has the keys and caches the plaintext so it can render history
//! offline. `is_encrypted` is 0 on all 162 messages of the validation device,
//! and a row where it is 1 is skipped rather than shown as mojibake.
//!
//! GROUP TITLES CARRY A LEADING NUL. `title` is a blob whose first byte is a
//! version marker — `\0Android`, `\0iOS 15`. Read as text it looks like a
//! binary field; skipping one byte gives the real name.
//!
//! THE TYPE CODES ARE DOCUMENTED in MEGA's open-source SDK
//! (`MegaChatMessage::TYPE_*`), which is what earns the mapping — an
//! undocumented enum would travel as a number.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::{col_i64, col_string, AppMessage};
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "mega",
    service: "MEGA",
    // Chat ids are signed 64-bit handles, so they are bare numbers — and a 1:1
    // chat has one too. Numeric-id group inference would mislabel every thread.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%karere-%.db")?;
    // `-wal`/`-shm` siblings are not the store.
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

/// MEGA's own message classes, from the SDK's `MegaChatMessage::TYPE_*`.
fn classify(t: i64) -> Option<&'static str> {
    match t {
        1 => None,                   // NORMAL — ordinary text
        2..=5 => Some("system"),     // participants, truncate, privilege, title
        6 | 7 => Some("call"),       // CALL_ENDED / CALL_STARTED
        101 | 103 => Some("shared"), // a file or a contact card
        104 | 105 => Some("shared"), // rich preview / voice clip
        _ => Some("system"),
    }
}

fn parse(path: &Path, _rel: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "history")? {
        return Ok(Vec::new());
    }

    // The local account, stated rather than inferred.
    let me: Option<i64> = conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM vars WHERE name = 'my_handle'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| s.trim().parse().ok());

    // userid → e-mail. MEGA has no display names; the address IS the identity.
    let mut emails: HashMap<i64, String> = HashMap::new();
    if table_exists(&conn, "contacts")? {
        let mut st = conn.prepare("SELECT userid, email FROM contacts")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            if let (Some(id), Some(mail)) = (col_i64(r, 0)?, col_string(r, 1)?) {
                if !mail.trim().is_empty() {
                    emails.insert(id, mail);
                }
            }
        }
    }

    // chatid → (peer, title, is_group)
    let mut chats: HashMap<i64, (Option<i64>, Option<String>)> = HashMap::new();
    if table_exists(&conn, "chats")? {
        // `substr(title, 2)` drops the leading NUL version marker.
        let mut st =
            conn.prepare("SELECT chatid, peer, CAST(substr(title, 2) AS TEXT) FROM chats")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let Some(chat) = col_i64(r, 0)? else { continue };
            let peer = col_i64(r, 1)?.filter(|p| *p != -1);
            let title = col_string(r, 2)?.filter(|t| !t.trim().is_empty());
            chats.insert(chat, (peer, title));
        }
    }

    let mut st = conn.prepare(
        "SELECT chatid, userid, ts, type, CAST(data AS TEXT), is_encrypted
         FROM history ORDER BY ts",
    )?;
    let mut rows = st.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let Some(chat) = col_i64(r, 0)? else { continue };
        // An encrypted row would render as mojibake. Skipped rather than shown.
        if col_i64(r, 5)?.unwrap_or(0) != 0 {
            continue;
        }
        let sender = col_i64(r, 1)?;
        let timestamp = col_i64(r, 2)?;
        let msg_type = col_i64(r, 3)?.unwrap_or(1);
        let body = col_string(r, 4)?.filter(|b| !b.is_empty());
        let kind = classify(msg_type);

        let (peer, title) = chats.get(&chat).cloned().unwrap_or((None, None));
        let is_group = peer.is_none();
        // A 1:1 thread is named after the other party's address; a group after
        // its own title.
        let chat_name = if is_group {
            title
        } else {
            peer.and_then(|p| emails.get(&p).cloned())
        };

        out.push(AppMessage {
            source_id: None,
            is_group,
            chat_key: chat.to_string(),
            chat_name,
            timestamp,
            body,
            // Stated by the store, not inferred. Without `my_handle` every
            // message reads as incoming rather than as confidently wrong.
            is_from_me: match (me, sender) {
                (Some(me), Some(s)) => me == s,
                _ => false,
            },
            sender_name: if is_group {
                sender.and_then(|s| emails.get(&s).cloned())
            } else {
                None
            },
            sender_handle: sender.and_then(|s| emails.get(&s).cloned()),
            sender_id: sender.map(|s| s.to_string()),
            has_attachment: matches!(msg_type, 101 | 103 | 105),
            kind,
            attachments: Vec::new(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(with_handle: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("karere-abc.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE vars (name TEXT PRIMARY KEY, value BLOB);
             CREATE TABLE contacts (userid INT PRIMARY KEY, email TEXT);
             CREATE TABLE chats (chatid INT PRIMARY KEY, peer INT, title BLOB);
             CREATE TABLE history (chatid INT, userid INT, ts INT, type INT,
                data BLOB, is_encrypted INT);
             INSERT INTO contacts VALUES (99, 'peer@example.com'), (7, 'other@example.com');
             -- a 1:1 chat, and a group whose title carries the leading NUL.
             INSERT INTO chats VALUES (1, 99, NULL), (2, -1, x'00416E64726F6964');
             INSERT INTO history VALUES
                (1, 42, 1714244658, 1, 'mine', 0),
                (1, 99, 1714245030, 1, 'theirs', 0),
                (2, 7,  1714245100, 101, 'file.jpg', 0),
                (2, 7,  1714245200, 7, NULL, 0),
                -- encrypted: must be skipped, not shown as mojibake.
                (2, 7,  1714245300, 1, x'ff00ff', 1);",
        )
        .unwrap();
        if with_handle {
            c.execute("INSERT INTO vars VALUES ('my_handle', '42')", [])
                .unwrap();
        }
        drop(c);
        (dir, path)
    }

    #[test]
    fn direction_comes_from_my_handle_not_a_guess() {
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        let mine = m
            .iter()
            .find(|x| x.body.as_deref() == Some("mine"))
            .unwrap();
        let theirs = m
            .iter()
            .find(|x| x.body.as_deref() == Some("theirs"))
            .unwrap();
        assert!(mine.is_from_me);
        assert!(!theirs.is_from_me);
    }

    #[test]
    fn without_my_handle_nothing_is_claimed_as_mine() {
        // A wrong attribution is worse than an absent one.
        let (_d, p) = db(false);
        let m = parse(&p, "").unwrap();
        assert!(m.iter().all(|x| !x.is_from_me));
    }

    #[test]
    fn a_one_to_one_thread_is_named_after_the_peers_address() {
        // MEGA has no display names; the e-mail IS the identity.
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        let t = m.iter().find(|x| x.chat_key == "1").unwrap();
        assert_eq!(t.chat_name.as_deref(), Some("peer@example.com"));
    }

    #[test]
    fn a_group_title_drops_its_leading_nul() {
        // Read whole, the blob looks binary and the group looks unnamed.
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        let g = m.iter().find(|x| x.chat_key == "2").unwrap();
        assert_eq!(g.chat_name.as_deref(), Some("Android"));
    }

    #[test]
    fn encrypted_rows_are_skipped_and_types_are_classified() {
        let (_d, p) = db(true);
        let m = parse(&p, "").unwrap();
        assert_eq!(m.len(), 4, "the is_encrypted row must not be rendered");
        let att = m
            .iter()
            .find(|x| x.body.as_deref() == Some("file.jpg"))
            .unwrap();
        assert!(att.has_attachment);
        assert_eq!(att.kind, Some("shared"));
        let call = m
            .iter()
            .find(|x| x.chat_key == "2" && x.body.is_none())
            .unwrap();
        assert_eq!(call.kind, Some("call"));
    }
}
