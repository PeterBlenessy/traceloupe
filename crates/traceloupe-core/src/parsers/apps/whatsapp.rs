//! WhatsApp native chat module.
//!
//! Schema facts (learned from iLEAPP `whatsApp.py`, written fresh — provenance
//! reference, §10):
//! - Message DB: `ChatStorage.sqlite` in the app-group container
//!   (`AppDomainGroup-group.net.whatsapp.WhatsApp.shared`).
//! - `ZWAMESSAGE(ZMESSAGEDATE, ZISFROMME, ZPARTNERNAME, ZTEXT, ZCHATSESSION, …)`
//!   — one row per message. `ZMESSAGEDATE` is Core Data time (seconds since
//!   2001-01-01).
//! - `ZWACHATSESSION(Z_PK, ZCONTACTJID, …)` — the chat; `ZCONTACTJID` is the
//!   stable per-conversation key (a `@g.us` group jid or `@s.whatsapp.net` 1:1).
//! - `ZWAMEDIAITEM(ZMESSAGE, ZMEDIALOCALPATH, …)` — attachment per message.
//!
//! - `ZWAGROUPMEMBER(Z_PK, ZMEMBERJID, ZCONTACTNAME, ZPUSHNAME, …)` — one row per
//!   participant of a group; `ZWAMESSAGE.ZGROUPMEMBER` points at the row for the
//!   message's actual author.
//!
//! `ZPARTNERNAME` is the chat/session *partner*, not the per-message author, so
//! using it in a group attributes every inbound message to one person — worse
//! than showing nothing, because it reads as a fact. The group-member join is
//! what makes a group chat legible (#346). Both the table and its name columns
//! are probed rather than assumed: an older `ChatStorage.sqlite` without them
//! falls back to the partner name, which is correct for 1:1.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::{AppAttachment, AppMessage};
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// Core Data epoch (2001-01-01 UTC) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "whatsapp",
    service: "WhatsApp",
    // WhatsApp threads carry a chat name, so group inference never runs anyway.
    numeric_id_groups: false,
    locate,
    parse,
};

/// Find `ChatStorage.sqlite` under a WhatsApp app-group domain. The relativePath
/// may carry a directory prefix, so match the filename suffix (leading `%`) and
/// require a WhatsApp domain.
fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let hits = index.find_relative_like("%ChatStorage.sqlite")?;
    Ok(hits
        .into_iter()
        .filter(|e| {
            let rp = &e.relative_path;
            // Whole path component, not just a suffix (don't match FooChatStorage.sqlite).
            (rp == "ChatStorage.sqlite" || rp.ends_with("/ChatStorage.sqlite"))
                && e.domain.to_lowercase().contains("whatsapp")
        })
        .collect())
}

/// The SELECT fragments for the group-member join, or literal NULLs when the
/// table (or the columns this schema version spells differently) isn't there.
struct MemberExprs {
    name: String,
    jid: String,
    join: String,
}

fn group_member_exprs(src: &Connection) -> MemberExprs {
    let none = || MemberExprs {
        name: "NULL".into(),
        jid: "NULL".into(),
        join: String::new(),
    };
    let has_table = src
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ZWAGROUPMEMBER'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .is_ok();
    if !has_table || !table_has(src, "ZWAMESSAGE", "ZGROUPMEMBER") {
        return none();
    }
    // Display name first, push name second, bare jid last — a jid is ugly but
    // it still distinguishes one participant from another, which is the point.
    let candidates = ["ZCONTACTNAME", "ZPUSHNAME"]
        .into_iter()
        .filter(|c| table_has(src, "ZWAGROUPMEMBER", c))
        .map(|c| format!("gm.{c}"))
        .collect::<Vec<_>>();
    let has_jid = table_has(src, "ZWAGROUPMEMBER", "ZMEMBERJID");
    if candidates.is_empty() && !has_jid {
        return none();
    }
    let jid = if has_jid { "gm.ZMEMBERJID" } else { "NULL" };
    let mut parts = candidates;
    if has_jid {
        parts.push(jid.to_string());
    }
    MemberExprs {
        name: format!("COALESCE({})", parts.join(", ")),
        jid: jid.to_string(),
        join: "LEFT JOIN ZWAGROUPMEMBER gm ON gm.Z_PK = m.ZGROUPMEMBER".into(),
    }
}

fn table_has(src: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = src.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
        return false;
    };
    let names: Vec<String> = rows.flatten().collect();
    names.iter().any(|c| c.eq_ignore_ascii_case(column))
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let member = group_member_exprs(&src);
    // `ZPARTNERNAME` is a column of the SESSION, not the message. Reading it off
    // `m` made the statement fail to prepare — "no such column: m.ZPARTNERNAME"
    // — which aborts the parse, so WhatsApp imported NOTHING at all on this
    // schema: no messages, no media, no threads. The unit fixture happened to
    // declare it on ZWAMESSAGE, so the test agreed with the bug. Probe both, and
    // prefer the session, which is where every real ChatStorage.sqlite has it.
    let partner = if table_has(&src, "ZWACHATSESSION", "ZPARTNERNAME") {
        "s.ZPARTNERNAME"
    } else if table_has(&src, "ZWAMESSAGE", "ZPARTNERNAME") {
        "m.ZPARTNERNAME"
    } else {
        "NULL"
    };
    // Per-message author, straight off the message row: WhatsApp stores the
    // sender's push name here, which is present even when the group-member row
    // is not.
    let push = if table_has(&src, "ZWAMESSAGE", "ZPUSHNAME") {
        "m.ZPUSHNAME"
    } else {
        "NULL"
    };
    let from_jid = if table_has(&src, "ZWAMESSAGE", "ZFROMJID") {
        "m.ZFROMJID"
    } else {
        "NULL"
    };
    // The media file's path on the device, relative to the app container —
    // e.g. "Media/1911111111@s.whatsapp.net/a/9/a929….jpg". Without it every
    // photo and video sent in WhatsApp was flagged as an attachment and then
    // never resolved to anything.
    let media = if table_has(&src, "ZWAMEDIAITEM", "ZMEDIALOCALPATH") {
        "md.ZMEDIALOCALPATH"
    } else {
        "NULL"
    };
    let mut stmt = src.prepare(&format!(
        // has_attachment via EXISTS (not a JOIN) so a message with several media
        // items isn't fanned out into duplicate rows.
        "SELECT
             s.ZCONTACTJID,
             {partner},
             m.ZMESSAGEDATE,
             m.ZISFROMME,
             m.ZTEXT,
             EXISTS(SELECT 1 FROM ZWAMEDIAITEM x WHERE x.ZMESSAGE = m.Z_PK) AS has_media,
             {name},
             {jid},
             {push},
             {from_jid},
             {media}
         FROM ZWAMESSAGE m
         LEFT JOIN ZWACHATSESSION s ON s.Z_PK = m.ZCHATSESSION
         LEFT JOIN ZWAMEDIAITEM md ON md.ZMESSAGE = m.Z_PK
         {join}
         ORDER BY s.ZCONTACTJID, m.ZMESSAGEDATE",
        name = member.name,
        jid = member.jid,
        join = member.join,
    ))?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = r
            .get::<_, Option<String>>(0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let name: Option<String> = r
            .get::<_, Option<String>>(1)?
            .filter(|s| !s.trim().is_empty());
        let timestamp = r
            .get::<_, Option<f64>>(2)?
            .filter(|d| *d > 0.0)
            .map(|d| (d + MAC_EPOCH as f64) as i64);
        let is_from_me = r.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0;
        let body: Option<String> = super::col_string(r, 4)?;
        let has_attachment = r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0;
        let member_name: Option<String> = r
            .get::<_, Option<String>>(6)?
            .filter(|s| !s.trim().is_empty());
        let member_jid: Option<String> = r
            .get::<_, Option<String>>(7)?
            .filter(|s| !s.trim().is_empty());
        let push_name: Option<String> = r
            .get::<_, Option<String>>(8)?
            .filter(|s| !s.trim().is_empty());
        let from_jid: Option<String> = r
            .get::<_, Option<String>>(9)?
            .filter(|s| !s.trim().is_empty());
        let media_path: Option<String> = super::col_string(r, 10)?.filter(|s| !s.trim().is_empty());
        // A group jid is definitive, and it has to be, because the shared
        // inserter's fallback (count distinct senders) is only accumulated for
        // chats WITHOUT a name of their own — and WhatsApp always supplies one.
        let is_group = chat_key.ends_with("@g.us");

        let attachments = media_path
            .map(|p| {
                let filename = p
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                vec![AppAttachment {
                    path: p,
                    mime: None,
                    filename,
                }]
            })
            .unwrap_or_default();

        out.push(AppMessage {
            source_id: None,
            is_group,
            attachments,
            chat_key,
            chat_name: name.clone(),
            timestamp,
            body,
            is_from_me,
            // In a group the author is the joined member row; `ZPARTNERNAME` is
            // the session partner and would label everyone the same person.
            // Outside a group the partner name IS the author (matching iLEAPP).
            sender_name: if is_from_me {
                None
            } else if is_group {
                // Group-member row first (a saved contact name), then the push
                // name the sender set, then their bare jid. Never the session
                // partner: that labels every message in the group one person.
                member_name
                    .or_else(|| push_name.clone())
                    .or_else(|| member_jid.clone())
                    .or_else(|| from_jid.clone())
            } else {
                name
            },
            sender_handle: None,
            sender_id: member_jid.or(from_jid),
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

    /// A ChatStorage with one GROUP chat whose three inbound messages come from
    /// three different members, all sharing one `ZPARTNERNAME`.
    fn make_group_chatstorage(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("ChatStorageGroup.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID TEXT,
                 ZPARTNERNAME TEXT);
             CREATE TABLE ZWAMEDIAITEM (Z_PK INTEGER PRIMARY KEY, ZMESSAGE INTEGER, ZMEDIALOCALPATH TEXT);
             CREATE TABLE ZWAGROUPMEMBER (Z_PK INTEGER PRIMARY KEY, ZMEMBERJID TEXT,
                 ZCONTACTNAME TEXT, ZPUSHNAME TEXT);
             CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER,
                 ZGROUPMEMBER INTEGER, ZPUSHNAME TEXT, ZMESSAGEDATE REAL,
                 ZISFROMME INTEGER, ZTEXT TEXT);
             INSERT INTO ZWACHATSESSION (Z_PK, ZCONTACTJID, ZPARTNERNAME)
                VALUES (1, '12036300@g.us', 'Hiking Crew');
             INSERT INTO ZWAGROUPMEMBER (Z_PK, ZMEMBERJID, ZCONTACTNAME, ZPUSHNAME) VALUES
                (1, '1@s.whatsapp.net', 'Nadia', NULL),
                (2, '2@s.whatsapp.net', NULL, 'Tom'),
                (3, '3@s.whatsapp.net', NULL, NULL);
             INSERT INTO ZWAMESSAGE
                (Z_PK, ZCHATSESSION, ZGROUPMEMBER, ZPUSHNAME, ZMESSAGEDATE, ZISFROMME, ZTEXT)
             VALUES
                (1, 1, 1, NULL, 721692800.0, 0, 'are we on?'),
                (2, 1, 2, NULL, 721692900.0, 0, 'yes'),
                (3, 1, 3, NULL, 721693000.0, 0, 'bringing snacks'),
                (4, 1, NULL, NULL, 721693100.0, 1, 'see you there');",
        )
        .unwrap();
        db
    }

    /// In a group, each message must carry its own author. `ZPARTNERNAME` is the
    /// session partner, so using it labels every inbound message the same
    /// person — a wrong attribution stated as fact (#346).
    #[test]
    fn group_messages_are_attributed_to_their_own_member_not_the_session_partner() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_group_chatstorage(dir.path());
        let msgs = parse(&db, "ChatStorage.sqlite").unwrap();

        assert!(msgs.iter().all(|m| m.is_group), "a @g.us jid is a group");
        let senders: Vec<Option<&str>> = msgs.iter().map(|m| m.sender_name.as_deref()).collect();
        assert_eq!(
            senders,
            vec![
                Some("Nadia"),            // ZCONTACTNAME
                Some("Tom"),              // falls back to ZPUSHNAME
                Some("3@s.whatsapp.net"), // ...and to the bare jid
                None,                     // outgoing has no sender
            ]
        );
        assert!(
            !senders.contains(&Some("Hiking Crew")),
            "the session partner name must never be used as a group author"
        );
    }

    /// The whole parse used to die on `no such column: m.ZPARTNERNAME`, because
    /// that column belongs to ZWACHATSESSION. A statement that fails to prepare
    /// aborts everything, so WhatsApp imported nothing at all — no messages, no
    /// threads, no media — while the unit fixture, which declared the column in
    /// the wrong place, reported success.
    #[test]
    fn parses_against_the_real_schema_where_partner_name_is_on_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_chatstorage(dir.path());
        let msgs = parse(&db, "ChatStorage.sqlite").unwrap();
        assert_eq!(msgs.len(), 2, "both messages, or the statement never ran");
        assert_eq!(msgs[0].chat_name.as_deref(), Some("Sam"));
    }

    /// A photo sent in WhatsApp has to reach the gallery. `ZMEDIALOCALPATH` was
    /// never read, so every message was flagged as having an attachment and the
    /// attachment list was left empty — the resolver downstream had nothing to
    /// resolve, and no WhatsApp media ever appeared in Photos.
    #[test]
    fn media_items_yield_an_attachment_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_chatstorage(dir.path());
        let msgs = parse(&db, "ChatStorage.sqlite").unwrap();
        let with_media = msgs
            .iter()
            .find(|m| m.has_attachment)
            .expect("the first message has a ZWAMEDIAITEM");
        assert_eq!(
            with_media.attachments.len(),
            1,
            "has_attachment without a path is a dead end"
        );
        assert_eq!(
            with_media.attachments[0].path,
            "Media/15551234567@s.whatsapp.net/a/9/photo.jpg"
        );
        assert_eq!(
            with_media.attachments[0].filename.as_deref(),
            Some("photo.jpg")
        );
    }

    /// The 1:1 path must keep using ZPARTNERNAME, which is the author there.
    #[test]
    fn one_to_one_messages_still_use_the_partner_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_chatstorage(dir.path());
        let msgs = parse(&db, "ChatStorage.sqlite").unwrap();
        assert!(!msgs.iter().any(|m| m.is_group));
        assert_eq!(msgs[0].sender_name.as_deref(), Some("Sam"));
        assert_eq!(msgs[1].sender_name, None, "outgoing has no sender");
    }

    fn make_chatstorage(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("ChatStorage.sqlite");
        let conn = Connection::open(&db).unwrap();
        // The REAL ChatStorage layout: ZPARTNERNAME belongs to the SESSION. The
        // fixture used to declare it on ZWAMESSAGE, which is what let the parser
        // ship a `m.ZPARTNERNAME` that no real backup can satisfy.
        conn.execute_batch(
            "CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID TEXT,
                 ZPARTNERNAME TEXT);
             CREATE TABLE ZWAMEDIAITEM (Z_PK INTEGER PRIMARY KEY, ZMESSAGE INTEGER,
                 ZMEDIALOCALPATH TEXT, ZTHUMBNAILLOCALPATH TEXT);
             CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER,
                 ZGROUPMEMBER INTEGER, ZPUSHNAME TEXT, ZFROMJID TEXT,
                 ZMESSAGEDATE REAL, ZISFROMME INTEGER, ZTEXT TEXT);
             INSERT INTO ZWACHATSESSION (Z_PK, ZCONTACTJID, ZPARTNERNAME)
                VALUES (1, '15551234567@s.whatsapp.net', 'Sam');
             -- Incoming, Mac-time 721692800 = unix 1_700_000_000.
             INSERT INTO ZWAMESSAGE (Z_PK, ZCHATSESSION, ZMESSAGEDATE, ZISFROMME, ZTEXT)
                VALUES (1, 1, 721692800.0, 0, 'hey there');
             INSERT INTO ZWAMESSAGE (Z_PK, ZCHATSESSION, ZMESSAGEDATE, ZISFROMME, ZTEXT)
                VALUES (2, 1, 721692900.0, 1, 'hi Sam');
             INSERT INTO ZWAMEDIAITEM (Z_PK, ZMESSAGE, ZMEDIALOCALPATH)
                VALUES (5, 1, 'Media/15551234567@s.whatsapp.net/a/9/photo.jpg');",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_and_inserts_whatsapp_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_chatstorage(tmp.path());

        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].chat_key, "15551234567@s.whatsapp.net");
        assert_eq!(msgs[0].sender_name.as_deref(), Some("Sam"));
        assert!(msgs[0].has_attachment);
        assert!(msgs[1].is_from_me && msgs[1].sender_name.is_none());

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "WhatsApp", false, msgs, &mut report)
            .unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 2);

        let c = cache.conn();
        let (name, service, count): (String, String, i64) = c
            .query_row(
                "SELECT display_name, service, message_count FROM threads",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Sam");
        assert_eq!(service, "WhatsApp");
        assert_eq!(count, 2);
    }
}
