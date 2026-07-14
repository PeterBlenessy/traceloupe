//! Microsoft Teams native chat module.
//!
//! Schema facts (learned from iLEAPP `teams.py`, written fresh — provenance
//! reference, §10):
//! - DB: `.../SkypeSpacesDogfood/*/Skype*.sqlite` (Teams uses the Skype spaces DB).
//! - `ZSMESSAGE(ZARRIVALTIME, ZIMDISPLAYNAME, ZCONTENT, ZFROM, ZTHREADID,
//!   ZTS_ISSENTBYME)` — one row per message. `ZARRIVALTIME` is Core-Data seconds
//!   (2001); `ZCONTENT` is HTML; `ZIMDISPLAYNAME`/`ZFROM` are the per-message
//!   sender (so group per-author attribution works); `ZTS_ISSENTBYME` 1 = owner.
//! - `ZTHREAD(ZTSID, ZTHREADTOPIC)` — the conversation; `ZTHREADTOPIC` is the
//!   group title (NULL for a 1:1, where the peer's name is derived).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// Core Data epoch (2001-01-01) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "teams",
    service: "Teams",
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%/SkypeSpacesDogfood/%.sqlite")?;
    // The message DB is `Skype<...>.sqlite`; keep only .sqlite (not -wal/-shm).
    hits.retain(|e| {
        e.relative_path.ends_with(".sqlite") && e.relative_path.contains("/SkypeSpacesDogfood/")
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

/// Reduce Teams' HTML message content to plain text: drop tags, decode the common
/// entities, and collapse whitespace. Good enough for a readable message body.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            // A tag becomes a separator so `a<br>b` → `a b`, not `ab`.
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "ZSMESSAGE")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT
             m.ZTHREADID AS chat_key,
             t.ZTHREADTOPIC AS chat_name,
             m.ZARRIVALTIME AS arrival,
             m.ZTS_ISSENTBYME AS is_own,
             m.ZCONTENT AS content,
             m.ZIMDISPLAYNAME AS sender_name,
             m.ZFROM AS sender_id
         FROM ZSMESSAGE m
         LEFT JOIN ZTHREAD t ON m.ZTHREADID = t.ZTSID
         ORDER BY chat_key, arrival",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = super::col_string(r, 0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let chat_name: Option<String> = super::col_string(r, 1)?.filter(|s| !s.trim().is_empty());
        let timestamp = r
            .get::<_, Option<f64>>(2)?
            .filter(|d| *d > 0.0)
            .map(|d| d as i64 + MAC_EPOCH);
        let is_from_me = r.get::<_, Option<i64>>(3)?.unwrap_or(0) == 1;
        let body = super::col_string(r, 4)?
            .map(|h| html_to_text(&h))
            .filter(|s| !s.is_empty());
        let sender_name: Option<String> = super::col_string(r, 5)?.filter(|s| !s.trim().is_empty());
        let sender_id: Option<String> = super::col_string(r, 6)?;

        out.push(AppMessage {
            chat_key,
            chat_name,
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me { None } else { sender_name },
            sender_handle: None,
            sender_id: if is_from_me { None } else { sender_id },
            has_attachment: false, // media lives in cached content; not surfaced
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;
    use crate::normalize::ImportReport;

    #[test]
    fn html_to_text_strips_and_decodes() {
        assert_eq!(html_to_text("<p>hi &amp; bye</p>"), "hi & bye");
        assert_eq!(html_to_text("a<br>b   c"), "a b c");
    }

    fn make_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("SkypeMessages.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZTHREAD (Z_PK INTEGER PRIMARY KEY, ZTSID TEXT, ZTHREADTOPIC TEXT);
             CREATE TABLE ZSMESSAGE (Z_PK INTEGER PRIMARY KEY, ZTHREADID TEXT, ZARRIVALTIME REAL,
                 ZTS_ISSENTBYME INTEGER, ZCONTENT TEXT, ZIMDISPLAYNAME TEXT, ZFROM TEXT);
             INSERT INTO ZTHREAD (ZTSID, ZTHREADTOPIC) VALUES ('t_proj', 'Project Falcon');
             -- 1:1 thread (no topic).
             -- group thread 't_proj': two authors (Nadia, Sam).
             INSERT INTO ZSMESSAGE (ZTHREADID, ZARRIVALTIME, ZTS_ISSENTBYME, ZCONTENT, ZIMDISPLAYNAME, ZFROM)
                VALUES ('t_dm', 721692800.0, 0, '<p>ping</p>', 'Nadia', 'u_nadia');
             INSERT INTO ZSMESSAGE (ZTHREADID, ZARRIVALTIME, ZTS_ISSENTBYME, ZCONTENT, ZIMDISPLAYNAME, ZFROM)
                VALUES ('t_dm', 721692900.0, 1, '<p>pong</p>', 'Me', 'u_me');
             INSERT INTO ZSMESSAGE (ZTHREADID, ZARRIVALTIME, ZTS_ISSENTBYME, ZCONTENT, ZIMDISPLAYNAME, ZFROM)
                VALUES ('t_proj', 721693000.0, 0, 'standup at 9', 'Sam', 'u_sam');",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_dm_and_group() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 3);

        // 1:1 (no topic): incoming from Nadia, HTML stripped.
        let dm = msgs
            .iter()
            .find(|m| m.chat_key == "t_dm" && !m.is_from_me)
            .unwrap();
        assert_eq!(dm.chat_name, None); // derived from peer by the framework
        assert_eq!(dm.body.as_deref(), Some("ping"));
        assert_eq!(dm.timestamp, Some(1_700_000_000));
        assert_eq!(dm.sender_name.as_deref(), Some("Nadia"));
        assert!(msgs
            .iter()
            .any(|m| m.is_from_me && m.body.as_deref() == Some("pong")));

        // Group thread titled by topic; incoming attributed to Sam.
        let grp = msgs.iter().find(|m| m.chat_key == "t_proj").unwrap();
        assert_eq!(grp.chat_name.as_deref(), Some("Project Falcon"));
        assert_eq!(grp.sender_name.as_deref(), Some("Sam"));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "Teams", false, msgs, &mut report).unwrap();
        assert_eq!(report.threads, 2);
        assert_eq!(report.messages, 3);
    }
}
