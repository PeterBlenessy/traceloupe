//! Viber native chat module.
//!
//! Schema facts (learned from iLEAPP `viber.py`, written fresh — provenance
//! reference, §10):
//! - DB: `com.viber/database/Contacts.data` (a Core Data store).
//! - `ZVIBERMESSAGE(ZCONVERSATION, ZTEXT, ZSTATEDATE, ZDATE, ZSTATE,
//!   ZPHONENUMINDEX, ZATTACHMENT)` — one row per message. `ZSTATEDATE` is the
//!   message **creation** time (Core-Data seconds, 2001), `ZDATE` its last
//!   state-change; we use `ZSTATEDATE` (iLEAPP's primary timestamp). `ZSTATE` is
//!   text: `received` = incoming, sent-side states (`send`/`delivered`/… ) =
//!   outgoing; `ZPHONENUMINDEX` → the sender member's `ZMEMBER.Z_PK`.
//! - `ZCONVERSATION(Z_PK, ZNAME, ZINTERLOCUTOR)` — the chat; `ZNAME` set for a
//!   group, else `ZINTERLOCUTOR` → the 1:1 partner member.
//! - `ZMEMBER(Z_PK, ZDISPLAYFULLNAME)` — display names.
//!
//! Focused scope: text messages, conversation grouping, sender, timestamp,
//! direction, attachment flag. Calls/location/time-bomb metadata are not surfaced.
//! NOTE: the `ZPHONENUMINDEX → ZMEMBER.Z_PK` mapping is from iLEAPP's join and is
//! unvalidated against a real backup; a wrong mapping degrades to NULL sender
//! names, not wrong data. Behind the iLEAPP fallback.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// Core Data epoch (2001-01-01) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "viber",
    service: "Viber",
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%com.viber/database/Contacts.data")?;
    hits.retain(|e| {
        e.relative_path
            .ends_with("com.viber/database/Contacts.data")
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
    if !table_exists(&conn, "ZVIBERMESSAGE")? || !table_exists(&conn, "ZCONVERSATION")? {
        return Ok(Vec::new());
    }
    // Title = group ZNAME or the 1:1 partner's name. Sender = the message's member
    // (ZPHONENUMINDEX), falling back to the conversation interlocutor for a 1:1.
    let mut stmt = conn.prepare(
        "SELECT
             m.ZCONVERSATION AS chat_key,
             COALESCE(conv.ZNAME, interloc.ZDISPLAYFULLNAME) AS chat_name,
             m.ZSTATE AS state,
             m.ZTEXT AS body,
             COALESCE(m.ZSTATEDATE, m.ZDATE) AS ts,
             COALESCE(m.ZPHONENUMINDEX, conv.ZINTERLOCUTOR) AS sender_id,
             sender.ZDISPLAYFULLNAME AS sender_name,
             (m.ZATTACHMENT IS NOT NULL) AS has_media,
             (m.ZPHONENUMINDEX IS NOT NULL) AS has_member_sender
         FROM ZVIBERMESSAGE m
         LEFT JOIN ZCONVERSATION conv ON m.ZCONVERSATION = conv.Z_PK
         LEFT JOIN ZMEMBER interloc ON interloc.Z_PK = conv.ZINTERLOCUTOR
         LEFT JOIN ZMEMBER sender
             ON sender.Z_PK = COALESCE(m.ZPHONENUMINDEX, conv.ZINTERLOCUTOR)
         ORDER BY chat_key, ts",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = super::col_string(r, 0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let chat_name: Option<String> = super::col_string(r, 1)?.filter(|s| !s.trim().is_empty());
        let state: Option<String> = super::col_string(r, 2)?;
        let body: Option<String> = super::col_string(r, 3)?;
        let timestamp = r
            .get::<_, Option<f64>>(4)?
            .filter(|d| *d > 0.0)
            .map(|d| d as i64 + MAC_EPOCH);
        let sender_id: Option<String> = super::col_string(r, 5)?;
        let sender_name: Option<String> = super::col_string(r, 6)?.filter(|s| !s.trim().is_empty());
        let has_attachment = r.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0;
        let has_member_sender = r.get::<_, Option<i64>>(8)?.unwrap_or(0) != 0;

        // Direction: `received` is the only inbound state; the sent-side states
        // (send/delivered/sending/failed — the owner's own actions) are outbound.
        // For an unknown/NULL state, infer from ZPHONENUMINDEX: an inbound message
        // carries a member-sender, so its absence means the owner sent it. This
        // avoids the serious mislabel of attributing an owner message to the peer.
        let is_from_me = match state.as_deref() {
            Some("received") => false,
            Some("send") | Some("delivered") | Some("sending") | Some("failed") => true,
            _ => !has_member_sender,
        };

        out.push(AppMessage {
            chat_key,
            chat_name,
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { sender_name },
            sender_handle: None,
            // The member id lets the framework label an unnamed group by distinct
            // senders; inert for a named group (title wins).
            sender_id: if is_from_me { None } else { sender_id },
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
        let db = dir.join("Contacts.data");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZMEMBER (Z_PK INTEGER PRIMARY KEY, ZDISPLAYFULLNAME TEXT);
             CREATE TABLE ZCONVERSATION (Z_PK INTEGER PRIMARY KEY, ZNAME TEXT, ZINTERLOCUTOR INTEGER);
             CREATE TABLE ZVIBERMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCONVERSATION INTEGER, ZTEXT TEXT,
                 ZSTATEDATE REAL, ZDATE REAL, ZSTATE TEXT, ZPHONENUMINDEX INTEGER, ZATTACHMENT INTEGER);
             INSERT INTO ZMEMBER (Z_PK, ZDISPLAYFULLNAME) VALUES (1, 'Lena Vibe'), (2, 'Max'), (3, 'Ivy');
             -- 1:1 with Lena (interlocutor 1, no name).
             INSERT INTO ZCONVERSATION (Z_PK, ZNAME, ZINTERLOCUTOR) VALUES (10, NULL, 1);
             -- named group.
             INSERT INTO ZCONVERSATION (Z_PK, ZNAME, ZINTERLOCUTOR) VALUES (20, 'Trailhead', NULL);
             -- 1:1: incoming from Lena (ZSTATEDATE = creation = unix 1_700_000_000;
             -- ZDATE is a LATER state-change, which we must NOT use).
             INSERT INTO ZVIBERMESSAGE (Z_PK, ZCONVERSATION, ZTEXT, ZSTATEDATE, ZDATE, ZSTATE, ZPHONENUMINDEX, ZATTACHMENT)
                VALUES (1, 10, 'privet', 721692800.0, 721699999.0, 'received', 1, 5);
             INSERT INTO ZVIBERMESSAGE (Z_PK, ZCONVERSATION, ZTEXT, ZSTATEDATE, ZDATE, ZSTATE, ZPHONENUMINDEX, ZATTACHMENT)
                VALUES (2, 10, 'hi Lena', 721692900.0, 721692900.0, 'delivered', NULL, NULL);
             -- a FAILED outgoing message (no member-sender): must be outgoing, not
             -- attributed to Lena.
             INSERT INTO ZVIBERMESSAGE (Z_PK, ZCONVERSATION, ZTEXT, ZSTATEDATE, ZDATE, ZSTATE, ZPHONENUMINDEX, ZATTACHMENT)
                VALUES (3, 10, 'oops', 721692950.0, 721692950.0, 'failed', NULL, NULL);
             -- group: incoming authored by Max (ZPHONENUMINDEX 2).
             INSERT INTO ZVIBERMESSAGE (Z_PK, ZCONVERSATION, ZTEXT, ZSTATEDATE, ZDATE, ZSTATE, ZPHONENUMINDEX, ZATTACHMENT)
                VALUES (4, 20, 'meet at 8', 721693000.0, 721693000.0, 'received', 2, NULL);",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_1to1_and_group() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 4);

        // 1:1 with Lena — timestamp is ZSTATEDATE (creation), NOT the later ZDATE.
        let one = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("privet"))
            .unwrap();
        assert_eq!(one.chat_name.as_deref(), Some("Lena Vibe"));
        assert!(!one.is_from_me);
        assert_eq!(one.timestamp, Some(1_700_000_000)); // ZSTATEDATE, not 721699999
        assert_eq!(one.sender_name.as_deref(), Some("Lena Vibe"));
        assert!(one.has_attachment);
        assert!(msgs
            .iter()
            .any(|m| m.is_from_me && m.body.as_deref() == Some("hi Lena")));

        // A FAILED outgoing message is outbound, not attributed to the partner.
        let failed = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("oops"))
            .unwrap();
        assert!(failed.is_from_me, "a failed SENT message must be outgoing");
        assert!(failed.sender_name.is_none());

        // Group: titled "Trailhead"; incoming attributed to Max.
        let grp = msgs.iter().find(|m| m.chat_key == "20").unwrap();
        assert_eq!(grp.chat_name.as_deref(), Some("Trailhead"));
        assert_eq!(grp.sender_name.as_deref(), Some("Max"));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "Viber", false, msgs, &mut report).unwrap();
        assert_eq!(report.threads, 2);
        assert_eq!(report.messages, 4);
    }
}
