//! Threema native chat module.
//!
//! Schema facts (learned from iLEAPP `Threema.py`, written fresh — provenance
//! reference, §10):
//! - DB: `ThreemaData.sqlite` (Core Data).
//! - `ZMESSAGE(ZDATE, ZISOWN, ZTEXT, ZCAPTION, ZCONVERSATION, ZSENDER, ZAUDIO,
//!   ZVIDEO, ZIMAGE, ZFILENAME)` — one row per message. `ZDATE` is Core-Data
//!   seconds (since 2001); `ZISOWN` 1 = sent by the owner; body is `ZTEXT` or a
//!   media `ZCAPTION`.
//! - `ZCONVERSATION(Z_PK, ZGROUPNAME, ZCONTACT)` — the chat; `ZGROUPNAME` set for
//!   a group, else `ZCONTACT` → the 1:1 partner.
//! - `ZCONTACT(Z_PK, ZFIRSTNAME, ZLASTNAME, ZPUBLICNICKNAME)` — display names.
//!   `ZMESSAGE.ZSENDER` → the group message's actual author (per-member
//!   attribution is available, unlike some apps).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// Core Data epoch (2001-01-01) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "threema",
    service: "Threema",
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%ThreemaData.sqlite")?;
    hits.retain(|e| {
        let rp = &e.relative_path;
        rp == "ThreemaData.sqlite" || rp.ends_with("/ThreemaData.sqlite")
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
    if !table_exists(&conn, "ZMESSAGE")? || !table_exists(&conn, "ZCONVERSATION")? {
        return Ok(Vec::new());
    }
    // The chat title is the group name or the 1:1 partner. Message authorship keys
    // off ZSENDER (present ⇒ an explicit author, i.e. a group message) — NOT off
    // ZGROUPNAME, so unnamed groups still attribute correctly. System messages
    // (Z_PRIMARYKEY.Z_NAME = 'SystemMessage') are excluded, matching iLEAPP.
    let mut stmt = conn.prepare(
        "SELECT
             m.ZCONVERSATION AS chat_key,
             COALESCE(
                 conv.ZGROUPNAME,
                 NULLIF(TRIM(COALESCE(cont.ZFIRSTNAME,'') || ' ' || COALESCE(cont.ZLASTNAME,'')), ''),
                 cont.ZPUBLICNICKNAME
             ) AS chat_name,
             m.ZISOWN AS is_own,
             COALESCE(m.ZTEXT, m.ZCAPTION) AS body,
             m.ZDATE AS zdate,
             m.ZSENDER AS sender_pk,
             COALESCE(
                 NULLIF(TRIM(COALESCE(sd.ZFIRSTNAME,'') || ' ' || COALESCE(sd.ZLASTNAME,'')), ''),
                 sd.ZPUBLICNICKNAME
             ) AS sender_name,
             (m.ZAUDIO IS NOT NULL OR m.ZVIDEO IS NOT NULL
                  OR m.ZIMAGE IS NOT NULL OR m.ZFILENAME IS NOT NULL) AS has_media
         FROM ZMESSAGE m
         LEFT JOIN ZCONVERSATION conv ON m.ZCONVERSATION = conv.Z_PK
         LEFT JOIN ZCONTACT cont ON conv.ZCONTACT = cont.Z_PK
         LEFT JOIN ZCONTACT sd ON sd.Z_PK = m.ZSENDER
         LEFT JOIN Z_PRIMARYKEY p ON m.Z_ENT = p.Z_ENT
         WHERE COALESCE(p.Z_NAME, '') <> 'SystemMessage'
         ORDER BY chat_key, m.ZDATE",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = super::col_string(r, 0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let chat_name: Option<String> = super::col_string(r, 1)?.filter(|s| !s.trim().is_empty());
        let is_from_me = r.get::<_, Option<i64>>(2)?.unwrap_or(0) == 1;
        let body: Option<String> = super::col_string(r, 3)?;
        let timestamp = r
            .get::<_, Option<f64>>(4)?
            .filter(|d| *d > 0.0)
            .map(|d| (d + MAC_EPOCH as f64) as i64);
        // ZSENDER present ⇒ an explicit per-message author (a group message).
        let sender_pk: Option<i64> = super::col_i64(r, 5)?;
        let sender_name: Option<String> = super::col_string(r, 6)?.filter(|s| !s.trim().is_empty());
        let has_attachment = r.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0;

        // Incoming sender: an explicit author (ZSENDER → its contact name) when
        // present, else the conversation partner (== chat_name) for a 1:1.
        let (incoming_sender, incoming_id) = if sender_pk.is_some() {
            (
                sender_name.or_else(|| chat_name.clone()),
                sender_pk.map(|p| p.to_string()),
            )
        } else {
            (chat_name.clone(), None)
        };

        out.push(AppMessage {
            attachments: Vec::new(),
            chat_key,
            chat_name,
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { incoming_sender },
            sender_handle: None,
            // The author id (ZSENDER) lets the framework label an UNNAMED group by
            // distinct-sender count; inert for a named group (title wins).
            sender_id: if is_from_me { None } else { incoming_id },
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
        let db = dir.join("ThreemaData.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZCONTACT (Z_PK INTEGER PRIMARY KEY, ZFIRSTNAME TEXT, ZLASTNAME TEXT, ZPUBLICNICKNAME TEXT);
             CREATE TABLE ZCONVERSATION (Z_PK INTEGER PRIMARY KEY, ZGROUPNAME TEXT, ZCONTACT INTEGER);
             CREATE TABLE Z_PRIMARYKEY (Z_ENT INTEGER PRIMARY KEY, Z_NAME TEXT);
             CREATE TABLE ZMESSAGE (Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, ZDATE REAL, ZISOWN INTEGER, ZTEXT TEXT,
                 ZCAPTION TEXT, ZCONVERSATION INTEGER, ZSENDER INTEGER, ZAUDIO INTEGER, ZVIDEO INTEGER,
                 ZIMAGE INTEGER, ZFILENAME TEXT);
             INSERT INTO Z_PRIMARYKEY (Z_ENT, Z_NAME) VALUES (1, 'TextMessage'), (9, 'SystemMessage');
             INSERT INTO ZCONTACT (Z_PK, ZFIRSTNAME, ZLASTNAME) VALUES (1, 'Mika', 'Laine');
             INSERT INTO ZCONTACT (Z_PK, ZFIRSTNAME, ZLASTNAME) VALUES (2, 'Otto', NULL);
             INSERT INTO ZCONTACT (Z_PK, ZFIRSTNAME, ZLASTNAME) VALUES (3, 'Aino', NULL);
             -- 1:1 conversation with Mika.
             INSERT INTO ZCONVERSATION (Z_PK, ZGROUPNAME, ZCONTACT) VALUES (10, NULL, 1);
             -- named group.
             INSERT INTO ZCONVERSATION (Z_PK, ZGROUPNAME, ZCONTACT) VALUES (20, 'Sauna Club', NULL);
             -- UNNAMED group (no ZGROUPNAME, no ZCONTACT).
             INSERT INTO ZCONVERSATION (Z_PK, ZGROUPNAME, ZCONTACT) VALUES (30, NULL, NULL);
             -- 1:1: incoming from Mika, then owner reply. Mac-time 721692800 = unix 1_700_000_000.
             INSERT INTO ZMESSAGE (Z_PK, Z_ENT, ZDATE, ZISOWN, ZTEXT, ZCONVERSATION, ZSENDER)
                VALUES (1, 1, 721692800.0, 0, 'moi', 10, NULL);
             INSERT INTO ZMESSAGE (Z_PK, Z_ENT, ZDATE, ZISOWN, ZTEXT, ZCONVERSATION, ZSENDER)
                VALUES (2, 1, 721692900.0, 1, 'hei Mika', 10, NULL);
             -- named group: incoming authored by Otto (ZSENDER=2).
             INSERT INTO ZMESSAGE (Z_PK, Z_ENT, ZDATE, ZISOWN, ZTEXT, ZCONVERSATION, ZSENDER)
                VALUES (3, 1, 721693000.0, 0, 'sauna at 7', 20, 2);
             -- a SystemMessage (should be skipped).
             INSERT INTO ZMESSAGE (Z_PK, Z_ENT, ZDATE, ZISOWN, ZTEXT, ZCONVERSATION, ZSENDER)
                VALUES (4, 9, 721693100.0, 0, NULL, 20, NULL);
             -- UNNAMED group: two distinct authors (Otto, Aino) → real group.
             INSERT INTO ZMESSAGE (Z_PK, Z_ENT, ZDATE, ZISOWN, ZTEXT, ZCONVERSATION, ZSENDER)
                VALUES (5, 1, 721693200.0, 0, 'moikka', 30, 2);
             INSERT INTO ZMESSAGE (Z_PK, Z_ENT, ZDATE, ZISOWN, ZTEXT, ZCONVERSATION, ZSENDER)
                VALUES (6, 1, 721693300.0, 0, 'hei', 30, 3);",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_1to1_named_and_unnamed_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 5, "the SystemMessage row is excluded");

        // 1:1 with Mika Laine.
        let one = msgs
            .iter()
            .find(|m| m.chat_key == "10" && !m.is_from_me)
            .unwrap();
        assert_eq!(one.chat_name.as_deref(), Some("Mika Laine"));
        assert_eq!(one.body.as_deref(), Some("moi"));
        assert_eq!(one.timestamp, Some(1_700_000_000));
        assert_eq!(one.sender_name.as_deref(), Some("Mika Laine"));

        // Named group: titled by group; incoming attributed to Otto.
        let grp = msgs.iter().find(|m| m.chat_key == "20").unwrap();
        assert_eq!(grp.chat_name.as_deref(), Some("Sauna Club"));
        assert_eq!(grp.sender_name.as_deref(), Some("Otto"));

        // UNNAMED group: authors still attributed (Otto, Aino).
        let u = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("moikka"))
            .unwrap();
        assert_eq!(u.chat_key, "30");
        assert_eq!(u.sender_name.as_deref(), Some("Otto"));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "Threema", false, msgs, &mut report).unwrap();
        assert_eq!(report.threads, 3);
        assert_eq!(report.messages, 5);
        // The unnamed group (2 distinct authors) is labeled a group.
        let title: String = cache
            .conn()
            .query_row(
                "SELECT display_name FROM threads WHERE identifier = '30'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(title.starts_with("Group chat"), "got: {title}");
    }
}
