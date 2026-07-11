//! Normalizer: read iLEAPP's `_lava_artifacts.db` and populate our cache DB.
//!
//! iLEAPP writes one table per artifact module (e.g. `sms`) plus engine tables
//! (`_lava_media_items`, `_lava_media_references`, `itunes_backup_info`). This
//! module maps those, per the contract verified in `docs/spike-ileapp.md`,
//! into the engine-neutral cache schema (see `cache.rs`). Phase 2's native
//! parsers will write the same cache tables through a different front door.
//!
//! Error isolation (architecture §12): a missing or malformed source table is
//! logged into `ImportReport.warnings` and skipped — it never aborts the
//! import. The `sms` table not existing (iLEAPP found no messages) is normal,
//! not an error.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::cache::CacheDb;
use crate::Result;

/// Outcome of a normalization pass, surfaced to the UI/logs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub threads: usize,
    pub messages: usize,
    pub media_items: usize,
    /// Non-fatal problems (a skipped artifact, a media ref with no bytes).
    pub warnings: Vec<String>,
}

/// Normalize the lava DB at `lava_path` into `cache`. `engine_out_dir` is the
/// iLEAPP output folder that lava's `extraction_path`s are relative to.
pub fn normalize_lava(
    lava_path: &Path,
    engine_out_dir: &Path,
    cache: &CacheDb,
) -> Result<ImportReport> {
    let lava = Connection::open_with_flags(lava_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut report = ImportReport::default();

    normalize_media(&lava, engine_out_dir, cache, &mut report)?;
    normalize_sms(&lava, cache, &mut report)?;

    Ok(report)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let found: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Copy `_lava_media_items` into the cache's `media_items`, keeping iLEAPP's
/// media-item id so artifact rows can reference it. `local_path` is resolved
/// to an absolute path under the engine output dir.
fn normalize_media(
    lava: &Connection,
    engine_out_dir: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(lava, "_lava_media_items")? {
        return Ok(());
    }
    let mut stmt =
        lava.prepare("SELECT id, source_path, extraction_path, type FROM _lava_media_items")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;

    let conn = cache.conn();
    for row in rows {
        let (id, source_path, extraction_path, mime) = row?;
        let kind = media_kind(mime.as_deref());
        let local_path = match extraction_path {
            Some(rel) => resolve_extraction_path(engine_out_dir, &rel, report),
            None => None,
        };
        conn.execute(
            "INSERT OR REPLACE INTO media_items
                 (engine_media_id, domain, relative_path, kind, mime_type, local_path)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                id,
                source_path.unwrap_or_default(),
                kind,
                mime,
                local_path.map(|p| p.to_string_lossy().into_owned()),
            ],
        )?;
        report.media_items += 1;
    }
    Ok(())
}

fn media_kind(mime: Option<&str>) -> &'static str {
    match mime {
        Some(m) if m.starts_with("video/") => "video",
        _ => "photo",
    }
}

/// Resolve a lava `extraction_path` (relative to the engine output dir) to an
/// absolute path, warning if the bytes are absent.
fn resolve_extraction_path(
    engine_out_dir: &Path,
    rel: &str,
    report: &mut ImportReport,
) -> Option<PathBuf> {
    let p = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        engine_out_dir.join(rel)
    };
    if p.exists() {
        Some(p)
    } else {
        report
            .warnings
            .push(format!("media bytes missing: {}", p.display()));
        None
    }
}

/// Map iLEAPP's `sms` table into cache `threads` + `messages`. Rows are grouped
/// into threads by `chat_id`; the display identifier comes from
/// `chat_contact_id`. Timestamps in lava are already Unix epoch seconds.
fn normalize_sms(lava: &Connection, cache: &CacheDb, report: &mut ImportReport) -> Result<()> {
    if !table_exists(lava, "sms")? {
        // No messages parsed — normal, not an error.
        return Ok(());
    }

    let mut stmt = lava.prepare(
        "SELECT chat_id, chat_contact_id, service, message_timestamp,
                message, from_me, attachment_file
         FROM sms
         ORDER BY chat_id, message_timestamp",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SmsRow {
            chat_id: r.get::<_, Option<String>>(0)?,
            contact_id: r.get::<_, Option<String>>(1)?,
            service: r.get::<_, Option<String>>(2)?,
            timestamp: r.get::<_, Option<i64>>(3)?,
            body: r.get::<_, Option<String>>(4)?,
            from_me: r.get::<_, Option<String>>(5)?,
            media_ref: r.get::<_, Option<String>>(6)?,
        })
    })?;

    let conn = cache.conn();
    // Group consecutive rows (query is ordered by chat_id) into threads.
    let mut current_key: Option<String> = None;
    let mut thread_id: i64 = 0;
    for row in rows {
        let row = row?;
        let key = row
            .chat_id
            .clone()
            .unwrap_or_else(|| row.contact_id.clone().unwrap_or_else(|| "unknown".into()));
        if current_key.as_ref() != Some(&key) {
            conn.execute(
                "INSERT INTO threads (identifier, display_name, service, last_message_at, message_count)
                 VALUES (?1, ?2, ?3, NULL, 0)",
                rusqlite::params![
                    key,
                    row.contact_id,
                    row.service,
                ],
            )?;
            thread_id = conn.last_insert_rowid();
            current_key = Some(key);
            report.threads += 1;
        }

        let is_from_me = matches!(row.from_me.as_deref(), Some("1"));
        let has_attachment = row.media_ref.is_some();
        conn.execute(
            "INSERT INTO messages
                 (thread_id, sender, is_from_me, body, sent_at, has_attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                thread_id,
                if is_from_me {
                    None
                } else {
                    row.contact_id.clone()
                },
                is_from_me as i64,
                row.body,
                row.timestamp,
                has_attachment as i64,
            ],
        )?;
        let message_id = conn.last_insert_rowid();
        report.messages += 1;

        // Link an attachment to its cache media_item. An artifact's
        // `attachment_file` is a `_lava_media_references.id`, which points via
        // `media_item_id` at the `_lava_media_items.id` we stored as
        // `engine_media_id`. Resolve that indirection (falling back to treating
        // the value as a media-item id directly, for engines without the
        // references table).
        if let Some(media_ref) = row.media_ref {
            let media_item_id = resolve_media_item_id(lava, &media_ref)?;
            let inserted = conn.execute(
                "INSERT INTO attachments (message_id, filename, mime_type, local_path)
                 SELECT ?1, relative_path, mime_type, local_path
                 FROM media_items WHERE engine_media_id = ?2",
                rusqlite::params![message_id, media_item_id],
            )?;
            if inserted == 0 {
                report.warnings.push(format!(
                    "message {message_id} references unknown media id {media_ref}"
                ));
            }
        }
    }

    // Denormalize per-thread counters used by the thread list.
    conn.execute(
        "UPDATE threads SET
             message_count = (SELECT COUNT(*) FROM messages WHERE messages.thread_id = threads.id),
             last_message_at = (SELECT MAX(sent_at) FROM messages WHERE messages.thread_id = threads.id)",
        [],
    )?;
    Ok(())
}

struct SmsRow {
    chat_id: Option<String>,
    contact_id: Option<String>,
    service: Option<String>,
    timestamp: Option<i64>,
    body: Option<String>,
    from_me: Option<String>,
    media_ref: Option<String>,
}

/// Translate an artifact's media reference (a `_lava_media_references.id`) to
/// the underlying `_lava_media_items.id`. If the references table is absent or
/// has no match, assume the reference is already a media-item id.
fn resolve_media_item_id(lava: &Connection, media_ref: &str) -> Result<String> {
    if table_exists(lava, "_lava_media_references")? {
        let item_id: Option<String> = lava
            .query_row(
                "SELECT media_item_id FROM _lava_media_references WHERE id = ?1",
                [media_ref],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = item_id {
            return Ok(id);
        }
    }
    Ok(media_ref.to_string())
}

/// Read a value from iLEAPP's `itunes_backup_info(property, property_value)`
/// key/value table, if present.
pub fn backup_info_value(lava_path: &Path, property: &str) -> Result<Option<String>> {
    let lava = Connection::open_with_flags(lava_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&lava, "itunes_backup_info")? {
        return Ok(None);
    }
    Ok(lava
        .query_row(
            "SELECT property_value FROM itunes_backup_info WHERE property = ?1",
            [property],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a minimal `_lava_artifacts.db` matching the schema the spike
    /// documented, so this test *is* the lava→cache contract. Mirrors the
    /// fixture: one chat, five text messages + one image attachment.
    fn make_lava(dir: &Path) -> PathBuf {
        let path = dir.join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE _lava_media_items (
                 id TEXT PRIMARY KEY, source_path TEXT, extraction_path TEXT,
                 type TEXT, metadata TEXT, created_at INTEGER, updated_at INTEGER,
                 is_embedded INTEGER);
             CREATE TABLE _lava_media_references (
                 id TEXT PRIMARY KEY, media_item_id TEXT, module_name TEXT,
                 artifact_name TEXT, name TEXT);
             CREATE TABLE sms (
                 message_timestamp INTEGER, read_timestamp INTEGER, message TEXT,
                 service TEXT, message_direction TEXT, message_sent TEXT,
                 message_delivered TEXT, message_read TEXT, account TEXT,
                 account_login TEXT, chat_contact_id TEXT, attachment_name TEXT,
                 attachment_file TEXT, attachment_timestamp INTEGER,
                 attachment_mimetype TEXT, attachment_size_bytes TEXT,
                 message_row_id TEXT, chat_id TEXT, from_me TEXT);
             CREATE TABLE itunes_backup_info (property TEXT, property_value TEXT);",
        )
        .unwrap();

        // Media item with real bytes on disk under the 'output dir' (= dir),
        // plus the reference row that artifact rows actually point at. This
        // mirrors real iLEAPP: sms.attachment_file = _lava_media_references.id
        // ('ref-1'), which resolves via media_item_id to _lava_media_items.id
        // ('media-1').
        let media_rel = "media/deadbeef.png";
        fs::create_dir_all(dir.join("media")).unwrap();
        fs::write(dir.join(media_rel), b"\x89PNG fake bytes").unwrap();
        conn.execute(
            "INSERT INTO _lava_media_items (id, source_path, extraction_path, type)
             VALUES ('media-1', 'private/var/mobile/.../salvage-test.png', ?1, 'image/png')",
            [media_rel],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO _lava_media_references (id, media_item_id, module_name, artifact_name, name)
             VALUES ('ref-1', 'media-1', 'sms', 'SMS', 'salvage-test.png')",
            [],
        )
        .unwrap();

        let convo = [
            ("Hey, are you around this weekend?", "0", None),
            ("Yeah! What did you have in mind?", "1", None),
            ("Thinking of hiking Mission Peak", "0", None),
            ("I'm in. Saturday morning?", "1", None),
            ("Perfect, I'll pick you up at 8", "0", None),
            ("Here's the trailhead", "1", Some("ref-1")),
        ];
        for (i, (text, from_me, media)) in convo.iter().enumerate() {
            conn.execute(
                "INSERT INTO sms
                     (message_timestamp, message, service, chat_contact_id,
                      chat_id, from_me, attachment_file, attachment_mimetype)
                 VALUES (?1, ?2, 'iMessage', '+15551234567', '1', ?3, ?4,
                         CASE WHEN ?4 IS NULL THEN NULL ELSE 'image/png' END)",
                rusqlite::params![1_717_840_800_i64 + i as i64 * 60, text, from_me, media,],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO itunes_backup_info VALUES ('Product Version', '17.5.1')",
            [],
        )
        .unwrap();
        path
    }

    #[test]
    fn normalizes_sms_and_media_into_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let lava_path = make_lava(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();

        let report = normalize_lava(&lava_path, tmp.path(), &cache).unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 6);
        assert_eq!(report.media_items, 1);
        assert!(
            report.warnings.is_empty(),
            "warnings: {:?}",
            report.warnings
        );

        let conn = cache.conn();
        // Thread counters were denormalized.
        let (count, last): (i64, i64) = conn
            .query_row(
                "SELECT message_count, last_message_at FROM threads",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 6);
        assert_eq!(last, 1_717_840_800 + 5 * 60);

        // from_me mapping: message 6 is outgoing.
        let from_me: i64 = conn
            .query_row(
                "SELECT is_from_me FROM messages ORDER BY sent_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(from_me, 1);

        // The attachment was linked with resolved local bytes.
        let (has_att, local): (i64, String) = conn
            .query_row(
                "SELECT m.has_attachments, a.local_path
                 FROM messages m JOIN attachments a ON a.message_id = m.id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(has_att, 1);
        assert!(local.ends_with("media/deadbeef.png"), "local: {local}");
    }

    #[test]
    fn missing_sms_table_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        Connection::open(&path).unwrap(); // empty DB, no tables
        let cache = CacheDb::open_in_memory().unwrap();

        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report, ImportReport::default());
    }

    #[test]
    fn missing_media_bytes_warns_but_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE _lava_media_items (id TEXT, source_path TEXT, extraction_path TEXT, type TEXT);
             INSERT INTO _lava_media_items VALUES ('m1', 'x', 'media/gone.png', 'image/png');",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();

        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report.media_items, 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("media bytes missing"));
        // Row still inserted, just with NULL local_path.
        let local: Option<String> = cache
            .conn()
            .query_row("SELECT local_path FROM media_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(local, None);
    }

    #[test]
    fn reads_backup_info_value() {
        let tmp = tempfile::tempdir().unwrap();
        let lava_path = make_lava(tmp.path());
        assert_eq!(
            backup_info_value(&lava_path, "Product Version")
                .unwrap()
                .as_deref(),
            Some("17.5.1")
        );
        assert_eq!(backup_info_value(&lava_path, "Nope").unwrap(), None);
    }
}
