//! imo (imo.im) native chat module.
//!
//! Schema facts (learned from iLEAPP `imoHD_Chat.py`, written fresh — provenance
//! reference, §10):
//! - DB: `IMODb2.sqlite`.
//! - `ZIMOCHATMSG(ZTEXT, ZTS, ZISSENT, ZA_UID, ZALIAS, ZIMDATA)` — one row per
//!   message. `ZTS` is **nanoseconds since 1970** (÷1e9 → Unix seconds);
//!   `ZISSENT`: 1 = Sent, 0 = Received; `ZA_UID` → the **conversation** (a buddy
//!   for a 1:1, the group for a group chat); `ZALIAS` → the **message author**
//!   (the actual sender, distinct from `ZA_UID` in groups); `ZIMDATA` is a plist
//!   blob (attachment or an action item).
//! - `ZIMOCONTACT(ZBUID, ZDISPLAY, ZDIGIT_PHONE)` — the conversation (grouping key
//!   + display name).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// imo timestamps are nanoseconds since the Unix epoch.
const NANOS_PER_SEC: i64 = 1_000_000_000;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "imo",
    service: "imo",
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%IMODb2.sqlite")?;
    hits.retain(|e| {
        let rp = &e.relative_path;
        rp == "IMODb2.sqlite" || rp.ends_with("/IMODb2.sqlite")
    });
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

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "ZIMOCHATMSG")? {
        return Ok(Vec::new());
    }
    // `ZALIAS` is the per-message author (differs from the conversation contact in
    // groups); `ZDISPLAY` names the conversation. has_attachment is best-effort:
    // `ZIMDATA` present flags media, but the blob can also be a non-media action
    // item, so it may over-flag (documented; the payload isn't parsed here).
    let mut stmt = conn.prepare(
        "SELECT
             COALESCE(m.ZA_UID, '') AS chat_key,
             c.ZDISPLAY,
             m.ZTS,
             m.ZISSENT,
             m.ZTEXT,
             m.ZALIAS,
             (m.ZIMDATA IS NOT NULL) AS has_media
         FROM ZIMOCHATMSG m
         LEFT JOIN ZIMOCONTACT c ON c.ZBUID = m.ZA_UID
         ORDER BY chat_key, m.ZTS",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = super::col_string(r, 0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let conv_name: Option<String> = super::col_string(r, 1)?.filter(|s| !s.trim().is_empty());
        // Nanosecond timestamp — read as i64 (NOT f64, which loses precision > 2^53).
        let timestamp = super::col_i64(r, 2)?
            .filter(|ts| *ts > 0)
            .map(|ts| ts / NANOS_PER_SEC);
        let is_from_me = r.get::<_, Option<i64>>(3)?.unwrap_or(0) == 1;
        let body: Option<String> = super::col_string(r, 4)?;
        let alias: Option<String> = super::col_string(r, 5)?.filter(|s| !s.trim().is_empty());
        let has_attachment = r.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0;

        // Incoming sender = the message author (ZALIAS), falling back to the
        // conversation name for a 1:1 where the alias is absent. `sender_id` (the
        // alias) lets the framework detect a group by distinct-sender count.
        let sender = alias.clone().or_else(|| conv_name.clone());
        out.push(AppMessage {
            chat_key,
            chat_name: conv_name,
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { sender },
            sender_handle: None,
            sender_id: if is_from_me { None } else { alias },
            has_attachment,
            kind: None,
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
        let db = dir.join("IMODb2.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZIMOCONTACT (Z_PK INTEGER PRIMARY KEY, ZBUID TEXT, ZDISPLAY TEXT, ZDIGIT_PHONE TEXT);
             CREATE TABLE ZIMOCHATMSG (Z_PK INTEGER PRIMARY KEY, ZA_UID TEXT, ZTS INTEGER,
                 ZISSENT INTEGER, ZTEXT TEXT, ZALIAS TEXT, ZIMDATA BLOB);
             INSERT INTO ZIMOCONTACT (Z_PK, ZBUID, ZDISPLAY) VALUES (1, 'buid_sam', 'Sam');
             INSERT INTO ZIMOCONTACT (Z_PK, ZBUID, ZDISPLAY) VALUES (2, 'grp_1', 'Book Club');
             -- 1:1: Received then Sent; ZTS = ns since 1970: 1_700_000_000 s * 1e9.
             INSERT INTO ZIMOCHATMSG (Z_PK, ZA_UID, ZTS, ZISSENT, ZTEXT, ZALIAS, ZIMDATA)
                VALUES (1, 'buid_sam', 1700000000000000000, 0, 'hey', 'Sam', x'01');
             INSERT INTO ZIMOCHATMSG (Z_PK, ZA_UID, ZTS, ZISSENT, ZTEXT, ZALIAS, ZIMDATA)
                VALUES (2, 'buid_sam', 1700000100000000000, 1, 'hi Sam', NULL, NULL);
             -- Group grp_1: two DIFFERENT authors (via ZALIAS), so it's a real group.
             INSERT INTO ZIMOCHATMSG (Z_PK, ZA_UID, ZTS, ZISSENT, ZTEXT, ZALIAS, ZIMDATA)
                VALUES (3, 'grp_1', 1700000200000000000, 0, 'chapter 3?', 'Alice', NULL);
             INSERT INTO ZIMOCHATMSG (Z_PK, ZA_UID, ZTS, ZISSENT, ZTEXT, ZALIAS, ZIMDATA)
                VALUES (4, 'grp_1', 1700000300000000000, 0, 'yes!', 'Bob', NULL);",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_1to1_and_group_with_per_author_attribution() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 4);

        // 1:1 with Sam.
        let received = msgs
            .iter()
            .find(|m| m.chat_key == "buid_sam" && !m.is_from_me)
            .unwrap();
        assert_eq!(received.chat_name.as_deref(), Some("Sam"));
        assert_eq!(received.body.as_deref(), Some("hey"));
        assert_eq!(received.timestamp, Some(1_700_000_000)); // ns → s
        assert_eq!(received.sender_name.as_deref(), Some("Sam"));
        assert!(received.has_attachment);

        // Group: each message attributed to its ZALIAS author, NOT the group name.
        let alice = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("chapter 3?"))
            .unwrap();
        assert_eq!(alice.chat_key, "grp_1");
        assert_eq!(alice.sender_name.as_deref(), Some("Alice"));
        let bob = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("yes!"))
            .unwrap();
        assert_eq!(bob.sender_name.as_deref(), Some("Bob"));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "imo", false, msgs, &mut report).unwrap();
        assert_eq!(report.threads, 2);
        assert_eq!(report.messages, 4);
        // The group has a name ("Book Club"), so it's titled by that; the fix is
        // that its messages are attributed to Alice/Bob, not to "Book Club".
        let group_title: String = cache
            .conn()
            .query_row(
                "SELECT display_name FROM threads WHERE identifier = 'grp_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(group_title, "Book Club");
    }
}
