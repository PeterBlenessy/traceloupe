//! LINE native chat module.
//!
//! Schema facts, read off Josh Hickman's public iOS 17 image with
//! `explore_real_backup` rather than from a reference (provenance: own
//! implementation; iLEAPP has no iOS LINE message parser).
//!
//! - DB: `…/PrivateStore/P_<account>/Messages/Line.sqlite` in
//!   `AppDomainGroup-group.com.linecorp.line`. The `P_<account>` segment is a
//!   per-ACCOUNT hash, so the path cannot be named literally — and a device
//!   signed into LINE twice would have two of them.
//! - `ZMESSAGE(ZCHAT, ZSENDER, ZTIMESTAMP, ZTEXT, ZCONTENTTYPE, ZLATITUDE,
//!   ZLONGITUDE)` — one row per message. `ZTIMESTAMP` is Unix **milliseconds**.
//! - `ZUSER(Z_PK, ZMID, ZNAME, ZADDRESSBOOKNAME)` — `ZMID` is LINE's own user id
//!   (`u` + 32 hex).
//! - `Z_1MEMBERS(Z_1CHATS, Z_12MEMBERS)` — the Core Data join table from a chat
//!   to its member users. This is what names a thread.
//!
//! WHO SENT IT. There is no `is_from_me` column. `ZSENDER` is NULL on the
//! owner's own messages, because Core Data models the sender as a relationship
//! to a `ZUSER` row and the owner has none. That is an inference, so it was
//! corroborated three ways on the validation device before being relied on:
//!
//! 1. Every chat's set of non-null senders is exactly its `Z_1MEMBERS` peer —
//!    no message is attributed to someone who is not in the conversation.
//! 2. The two-way threads are balanced (12 of 23, 15 of 29 null), which is what
//!    a conversation looks like; a deleted-sender artefact would not be.
//! 3. LINE's own official-account threads are 0-of-N null — nobody replies to a
//!    security notice, and if NULL meant "sender missing" those would be the
//!    threads full of them.
//!
//! `ZSENDSTATUS` deliberately is NOT used for this: it is 1 on both incoming and
//! outgoing messages on the validation device, so it looks like a direction flag
//! and is not one.
//!
//! THE SELF-CHAT. A chat with no row in `Z_1MEMBERS` is LINE's "Keep memo" —
//! notes to oneself. It is named as such rather than left blank, because an
//! unnamed thread of entirely outgoing messages reads like a bug.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::{col_i64, col_string, AppMessage};
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "line",
    service: "LINE",
    // Chat keys are Core Data primary keys, so they ARE bare numbers — and every
    // thread on the validation device is 1:1. Numeric-id group inference would
    // mislabel all of them.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%/Messages/Line.sqlite")?;
    // `LineSquare.sqlite` (LINE's OpenChat) and `MessageExt.sqlite` sit beside
    // it; neither is the message store, and a looser suffix would take them.
    hits.retain(|e| e.relative_path.ends_with("/Messages/Line.sqlite"));
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

fn parse(path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "ZMESSAGE")? {
        return Ok(Vec::new());
    }

    // Z_PK → display name. ZNAME is LINE's own profile name; the address-book
    // name is preferred when present, because it is what the owner calls them.
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut mids: HashMap<i64, String> = HashMap::new();
    if table_exists(&conn, "ZUSER")? {
        let mut stmt = conn.prepare("SELECT Z_PK, ZMID, ZNAME, ZADDRESSBOOKNAME FROM ZUSER")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let Some(pk) = col_i64(r, 0)? else { continue };
            if let Some(mid) = col_string(r, 1)? {
                mids.insert(pk, mid);
            }
            let name = col_string(r, 3)?
                .filter(|s| !s.trim().is_empty())
                .or(col_string(r, 2)?)
                .filter(|s| !s.trim().is_empty());
            if let Some(n) = name {
                names.insert(pk, n);
            }
        }
    }

    // chat → its member users. One member is a 1:1 thread; more is a group; none
    // is the self-chat.
    let mut members: HashMap<i64, Vec<i64>> = HashMap::new();
    if table_exists(&conn, "Z_1MEMBERS")? {
        let mut stmt = conn.prepare("SELECT Z_1CHATS, Z_12MEMBERS FROM Z_1MEMBERS")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            if let (Some(chat), Some(user)) = (col_i64(r, 0)?, col_i64(r, 1)?) {
                members.entry(chat).or_default().push(user);
            }
        }
    }

    let mut stmt = conn.prepare(
        "SELECT ZCHAT, ZSENDER, ZTIMESTAMP, ZTEXT, ZCONTENTTYPE, ZLATITUDE, ZLONGITUDE
         FROM ZMESSAGE ORDER BY ZTIMESTAMP",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let Some(chat) = col_i64(r, 0)? else { continue };
        let sender = col_i64(r, 1)?;
        // Unix MILLISECONDS. Divided rather than passed through: every other
        // module hands over seconds, and a millisecond value renders in 56000.
        let timestamp = col_i64(r, 2)?.map(|ms| ms / 1000);
        let body = col_string(r, 3)?;
        let content_type = col_i64(r, 4)?.unwrap_or(0);
        let has_coords = col_i64(r, 5)?.is_some() && col_i64(r, 6)?.is_some();

        let peers = members.get(&chat);
        let is_group = peers.is_some_and(|m| m.len() > 1);
        let chat_name = match peers {
            Some(m) if m.len() == 1 => names.get(&m[0]).cloned(),
            Some(_) => None,
            // No members at all: LINE's "Keep memo", the note-to-self thread.
            None => Some("Keep memo".to_string()),
        };

        // ZCONTENTTYPE: 0 is text; everything else is media of some kind. The
        // individual codes are not documented anywhere this can cite, so they
        // are not translated into words — but "not text" is enough to say an
        // attachment was carried, which is what the filter needs.
        // Media and a shared location are both "shared" to the filter, for
        // different reasons: one carries a file, the other carries coordinates
        // and no body at all. Kept as one arm because the filter has one bucket
        // for "not something they typed", not because the two are the same.
        let kind = if content_type != 0 || has_coords {
            Some("shared")
        } else {
            None
        };

        out.push(AppMessage {
            source_id: None,
            is_group,
            chat_key: chat.to_string(),
            chat_name,
            timestamp,
            body,
            // NULL sender means the owner. See the module header for the three
            // ways this was corroborated before being relied on.
            is_from_me: sender.is_none(),
            // A 1:1 peer is the thread itself; repeating its name on every
            // incoming message adds nothing.
            sender_name: if is_group {
                sender.and_then(|s| names.get(&s).cloned())
            } else {
                None
            },
            sender_handle: sender.and_then(|s| mids.get(&s).cloned()),
            sender_id: sender.map(|s| s.to_string()),
            has_attachment: content_type != 0,
            kind,
            attachments: Vec::new(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store shaped like the real one: two-way thread, an official-account
    /// thread nobody replies to, and the memberless Keep-memo chat.
    fn db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Line.sqlite");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE ZUSER (Z_PK INTEGER PRIMARY KEY, ZMID VARCHAR,
                ZNAME VARCHAR, ZADDRESSBOOKNAME VARCHAR);
             CREATE TABLE Z_1MEMBERS (Z_1CHATS INTEGER, Z_12MEMBERS INTEGER);
             CREATE TABLE ZMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHAT INTEGER,
                ZSENDER INTEGER, ZTIMESTAMP INTEGER, ZTEXT VARCHAR,
                ZCONTENTTYPE INTEGER, ZLATITUDE FLOAT, ZLONGITUDE FLOAT,
                ZSENDSTATUS INTEGER);
             INSERT INTO ZUSER VALUES
                (1, 'u2353528e', 'Profile Name', 'Address Book Name'),
                (3, 'u085311ec', 'LINE', NULL);
             INSERT INTO Z_1MEMBERS VALUES (3, 1), (2, 3);
             -- chat 3: a real two-way conversation.
             INSERT INTO ZMESSAGE VALUES
                (1, 3, 1,    1703447747382, 'How is the football?', 0, NULL, NULL, 1),
                (2, 3, NULL, 1703447801293, 'Miserable.',           0, NULL, NULL, 1),
                -- a shared location: coordinates, no media type.
                (3, 3, NULL, 1703447901293, NULL, 0, 35.6, 139.7, 1),
                -- media: a non-zero content type.
                (4, 3, 1,    1703447911293, NULL, 7, NULL, NULL, 1),
             -- chat 2: LINE's own notices. Nobody replies, so ZSENDSTATUS = 1
             -- on an INCOMING message -- which is why it is not a direction flag.
                (5, 2, 3,    1703447536593, 'A request to verify', 0, NULL, NULL, 1),
             -- chat 1: no Z_1MEMBERS row at all -- the Keep memo self-chat.
                (6, 1, NULL, 1703447599195, 'note to self', 0, NULL, NULL, 1);",
        )
        .unwrap();
        drop(c);
        (dir, path)
    }

    #[test]
    fn a_null_sender_is_the_owner_and_a_named_one_is_not() {
        let (_d, p) = db();
        let msgs = parse(&p, "").unwrap();
        let thread: Vec<_> = msgs.iter().filter(|m| m.chat_key == "3").collect();
        assert_eq!(thread.len(), 4);
        assert!(!thread[0].is_from_me, "a message with a sender is incoming");
        assert!(thread[1].is_from_me, "a NULL sender is the owner");
        // ZSENDSTATUS is 1 on both, which is why it must not be used for this.
        let notice = msgs.iter().find(|m| m.chat_key == "2").unwrap();
        assert!(!notice.is_from_me);
    }

    #[test]
    fn a_thread_is_named_after_its_member() {
        let (_d, p) = db();
        let msgs = parse(&p, "").unwrap();
        let t = msgs.iter().find(|m| m.chat_key == "3").unwrap();
        // The address-book name wins over LINE's profile name: it is what the
        // owner calls them.
        assert_eq!(t.chat_name.as_deref(), Some("Address Book Name"));
    }

    #[test]
    fn a_chat_with_no_members_is_the_keep_memo() {
        // Otherwise it is an unnamed thread of entirely outgoing messages,
        // which reads like a bug rather than like notes to oneself.
        let (_d, p) = db();
        let msgs = parse(&p, "").unwrap();
        let memo = msgs.iter().find(|m| m.chat_key == "1").unwrap();
        assert_eq!(memo.chat_name.as_deref(), Some("Keep memo"));
        assert!(memo.is_from_me);
    }

    #[test]
    fn milliseconds_become_seconds() {
        // Passed through, a 2023 message renders in the year 56000.
        let (_d, p) = db();
        let msgs = parse(&p, "").unwrap();
        let t = msgs[0].timestamp.unwrap();
        assert!(
            (1_700_000_000..1_800_000_000).contains(&t),
            "timestamp {t} is not Unix seconds"
        );
    }

    #[test]
    fn media_and_shared_locations_are_marked() {
        let (_d, p) = db();
        let msgs = parse(&p, "").unwrap();
        let media = msgs.iter().find(|m| m.chat_key == "3" && m.has_attachment);
        assert!(media.is_some(), "a non-zero ZCONTENTTYPE carries media");
        let located = msgs
            .iter()
            .find(|m| m.chat_key == "3" && m.body.is_none() && !m.has_attachment);
        assert_eq!(
            located.and_then(|m| m.kind),
            Some("shared"),
            "a message with coordinates is a shared location, not an empty message"
        );
    }
}
