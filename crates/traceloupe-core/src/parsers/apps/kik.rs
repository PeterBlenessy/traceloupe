//! Kik native chat module.
//!
//! Schema facts (learned from iLEAPP `kikMessages.py`, written fresh — provenance
//! reference, §10):
//! - DB: `kik.sqlite` (Core Data).
//! - `ZKIKMESSAGE(ZBODY, ZTIMESTAMP, ZTYPE, ZUSER)` — one row per message.
//!   `ZTIMESTAMP` is Core-Data time (seconds since 2001). `ZTYPE`: 1 = Received,
//!   2 = Sent, 3 = Group Admin, 4 = Group Message. `ZUSER` → the conversation
//!   partner's `ZKIKUSER.Z_PK`.
//! - `ZKIKUSER(Z_PK, ZDISPLAYNAME, ZUSERNAME)` — the partner (grouping key +
//!   display name).
//! - `ZKIKATTACHMENT(ZMESSAGE, ZCONTENT)` — an attachment per message.
//!
//! Conversations group by the partner (`ZUSERNAME`, per iLEAPP's
//! `conversationDiscriminatorColumn`).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// Core Data epoch (2001-01-01) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "kik",
    service: "Kik",
    // Grouped by username (with a display name), so group inference never runs.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%kik.sqlite")?;
    hits.retain(|e| e.relative_path.ends_with("kik.sqlite"));
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
    if !table_exists(&conn, "ZKIKMESSAGE")? {
        return Ok(Vec::new());
    }
    // has_attachment via EXISTS (not a JOIN) so a multi-attachment message isn't
    // fanned out into duplicate rows.
    let mut stmt = conn.prepare(
        "SELECT
             COALESCE(u.ZUSERNAME, CAST(m.ZUSER AS TEXT)) AS chat_key,
             u.ZDISPLAYNAME,
             m.ZTIMESTAMP,
             m.ZTYPE,
             m.ZBODY,
             EXISTS(SELECT 1 FROM ZKIKATTACHMENT a WHERE a.ZMESSAGE = m.Z_PK) AS has_media
         FROM ZKIKMESSAGE m
         LEFT JOIN ZKIKUSER u ON u.Z_PK = m.ZUSER
         ORDER BY chat_key, m.ZTIMESTAMP",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = super::col_string(r, 0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let name: Option<String> = super::col_string(r, 1)?.filter(|s| !s.trim().is_empty());
        let timestamp = r
            .get::<_, Option<f64>>(2)?
            .filter(|d| *d > 0.0)
            .map(|d| d as i64 + MAC_EPOCH);
        let ztype = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
        let body: Option<String> = r.get(4)?;
        let has_attachment = r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0;

        // ZTYPE 2 = Sent (from me); everything else is inbound to the owner.
        let is_from_me = ztype == 2;

        out.push(AppMessage {
            chat_key,
            chat_name: name.clone(),
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { name },
            sender_handle: None,
            sender_id: None,
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
        let db = dir.join("kik.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZKIKUSER (Z_PK INTEGER PRIMARY KEY, ZDISPLAYNAME TEXT, ZUSERNAME TEXT);
             CREATE TABLE ZKIKATTACHMENT (Z_PK INTEGER PRIMARY KEY, ZMESSAGE INTEGER, ZCONTENT TEXT);
             CREATE TABLE ZKIKMESSAGE (Z_PK INTEGER PRIMARY KEY, ZUSER INTEGER, ZTIMESTAMP REAL,
                 ZTYPE INTEGER, ZBODY TEXT);
             INSERT INTO ZKIKUSER (Z_PK, ZDISPLAYNAME, ZUSERNAME) VALUES (1, 'Robin', 'robin_k');
             -- Received then Sent, Mac-time 721692800 = unix 1_700_000_000.
             INSERT INTO ZKIKMESSAGE (Z_PK, ZUSER, ZTIMESTAMP, ZTYPE, ZBODY)
                VALUES (1, 1, 721692800.0, 1, 'hey');
             INSERT INTO ZKIKMESSAGE (Z_PK, ZUSER, ZTIMESTAMP, ZTYPE, ZBODY)
                VALUES (2, 1, 721692900.0, 2, 'hi Robin');
             INSERT INTO ZKIKATTACHMENT (Z_PK, ZMESSAGE, ZCONTENT) VALUES (5, 1, 'x.jpg');",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_and_inserts_kik_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 2);

        let received = msgs.iter().find(|m| !m.is_from_me).unwrap();
        assert_eq!(received.chat_key, "robin_k");
        assert_eq!(received.chat_name.as_deref(), Some("Robin"));
        assert_eq!(received.body.as_deref(), Some("hey"));
        assert_eq!(received.timestamp, Some(1_700_000_000));
        assert!(received.has_attachment);
        assert!(msgs
            .iter()
            .any(|m| m.is_from_me && m.body.as_deref() == Some("hi Robin")));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "Kik", false, msgs, &mut report).unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 2);
        let name: String = cache
            .conn()
            .query_row("SELECT display_name FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Robin");
    }
}
