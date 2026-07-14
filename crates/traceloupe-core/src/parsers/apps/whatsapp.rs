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

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// Core Data epoch (2001-01-01 UTC) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "whatsapp",
    service: "WhatsApp",
    locate,
    parse,
};

/// Find `ChatStorage.sqlite` under a WhatsApp app-group domain. The app-group
/// UUID varies, so match on the filename and require a WhatsApp domain.
fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let hits = index.find_relative_like("ChatStorage.sqlite")?;
    Ok(hits
        .into_iter()
        .filter(|e| e.domain.to_lowercase().contains("whatsapp"))
        .collect())
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = src.prepare(
        "SELECT
             s.ZCONTACTJID,
             m.ZPARTNERNAME,
             m.ZMESSAGEDATE,
             m.ZISFROMME,
             m.ZTEXT,
             md.ZMEDIALOCALPATH
         FROM ZWAMESSAGE m
         LEFT JOIN ZWACHATSESSION s ON s.Z_PK = m.ZCHATSESSION
         LEFT JOIN ZWAMEDIAITEM md ON md.ZMESSAGE = m.Z_PK
         ORDER BY s.ZCONTACTJID, m.ZMESSAGEDATE",
    )?;
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
            .map(|d| d as i64 + MAC_EPOCH);
        let is_from_me = r.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0;
        let body: Option<String> = r.get(4)?;
        let media: Option<String> = r.get(5)?;

        out.push(AppMessage {
            chat_key,
            chat_name: name.clone(),
            timestamp,
            body,
            is_from_me,
            // iLEAPP surfaces the partner name as the sender; match that for 1:1.
            sender_name: if is_from_me { None } else { name },
            sender_handle: None,
            sender_id: None,
            has_attachment: media.is_some(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;
    use crate::normalize::ImportReport;

    fn make_chatstorage(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("ChatStorage.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID TEXT);
             CREATE TABLE ZWAMEDIAITEM (Z_PK INTEGER PRIMARY KEY, ZMESSAGE INTEGER, ZMEDIALOCALPATH TEXT);
             CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER,
                 ZPARTNERNAME TEXT, ZMESSAGEDATE REAL, ZISFROMME INTEGER, ZTEXT TEXT);
             INSERT INTO ZWACHATSESSION (Z_PK, ZCONTACTJID) VALUES (1, '15551234567@s.whatsapp.net');
             -- Incoming, Mac-time 721692800 = unix 1_700_000_000.
             INSERT INTO ZWAMESSAGE (Z_PK, ZCHATSESSION, ZPARTNERNAME, ZMESSAGEDATE, ZISFROMME, ZTEXT)
                VALUES (1, 1, 'Sam', 721692800.0, 0, 'hey there');
             INSERT INTO ZWAMESSAGE (Z_PK, ZCHATSESSION, ZPARTNERNAME, ZMESSAGEDATE, ZISFROMME, ZTEXT)
                VALUES (2, 1, 'Sam', 721692900.0, 1, 'hi Sam');
             INSERT INTO ZWAMEDIAITEM (Z_PK, ZMESSAGE, ZMEDIALOCALPATH)
                VALUES (5, 1, 'Media/x.jpg');",
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
        super::super::insert_app_conversation(&cache, "WhatsApp", msgs, &mut report).unwrap();
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
