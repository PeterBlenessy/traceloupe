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
    pub calls: usize,
    pub safari_visits: usize,
    pub contacts: usize,
    pub notes: usize,
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
    normalize_sms(&lava, engine_out_dir, cache, &mut report)?;
    normalize_calls(&lava, cache, &mut report)?;
    normalize_safari(&lava, cache, &mut report)?;
    // Contacts come from a native parse of the decrypted AddressBook that
    // iLEAPP extracts (its own lava output for contacts is lossy — see
    // docs/spike-ileapp.md). A missing DB just means no contacts.
    normalize_contacts(engine_out_dir, cache, &mut report)?;
    normalize_notes(&lava, cache, &mut report)?;

    Ok(report)
}

/// Map iLEAPP's `notes` table (from NoteStore.sqlite) into cache `notes`.
fn normalize_notes(lava: &Connection, cache: &CacheDb, report: &mut ImportReport) -> Result<()> {
    if !table_exists(lava, "notes")? {
        return Ok(());
    }
    let mut stmt = lava.prepare(
        "SELECT folder, note_title, snippet, note_contents, creation_date, last_modified
         FROM notes",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(NoteRow {
            folder: r.get(0)?,
            title: r.get(1)?,
            snippet: r.get(2)?,
            body: r.get(3)?,
            created_at: epoch_value(r, 4),
            modified_at: epoch_value(r, 5),
        })
    })?;

    let conn = cache.conn();
    for row in rows {
        let row = row?;
        conn.execute(
            "INSERT INTO notes (folder, title, snippet, body_html, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                row.folder,
                row.title,
                row.snippet,
                row.body,
                row.created_at,
                row.modified_at,
            ],
        )?;
        report.notes += 1;
    }
    Ok(())
}

struct NoteRow {
    folder: Option<String>,
    title: Option<String>,
    snippet: Option<String>,
    body: Option<String>,
    created_at: Option<i64>,
    modified_at: Option<i64>,
}

/// Read an epoch-seconds value from a lava column that may be an integer or a
/// numeric string (lava column affinity varies); anything else yields None.
fn epoch_value(r: &rusqlite::Row<'_>, idx: usize) -> Option<i64> {
    match r.get::<_, rusqlite::types::Value>(idx).ok()? {
        rusqlite::types::Value::Integer(i) => Some(i),
        rusqlite::types::Value::Text(s) => s.parse().ok(),
        _ => None,
    }
}

/// Find, parse, and cache contacts from the decrypted `AddressBook.sqlitedb`
/// that iLEAPP extracts under its output `data/` tree.
fn normalize_contacts(
    engine_out_dir: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
) -> Result<()> {
    let Some(db) = find_extracted(engine_out_dir, "AddressBook.sqlitedb") else {
        return Ok(());
    };
    let contacts = match crate::parsers::address_book::parse_address_book(&db) {
        Ok(c) => c,
        Err(e) => {
            report.warnings.push(format!("contacts parse failed: {e}"));
            return Ok(());
        }
    };

    // Contact photos live in a sibling DB, keyed by the same ABPerson ROWID that
    // `ParsedContact.id` carries. Missing/odd-schema images are non-fatal.
    let images = find_extracted(engine_out_dir, "AddressBookImages.sqlitedb")
        .and_then(|p| crate::parsers::address_book::parse_address_book_images(&p).ok())
        .unwrap_or_default();

    let conn = cache.conn();
    for c in contacts {
        conn.execute(
            "INSERT INTO contacts (first_name, last_name, organization, phones_json, emails_json, image)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                c.first_name,
                c.last_name,
                c.organization,
                serde_json::to_string(&c.phones).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&c.emails).unwrap_or_else(|_| "[]".into()),
                images.get(&c.id),
            ],
        )?;
        report.contacts += 1;
    }
    Ok(())
}

/// Depth-first search under `root` for a file named `name`. iLEAPP nests
/// extracted files under `data/<domain path>/…`, so we can't hard-code a path.
fn find_extracted(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(path);
            }
        }
    }
    None
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
    let has_refs = table_exists(lava, "_lava_media_references")?;

    // Originals first (is_embedded=0) so that when the same source file is
    // checked in as both an original and a generated thumbnail, the original
    // is the one kept by the dedup below.
    let mut stmt = lava.prepare(
        "SELECT id, source_path, extraction_path, type, is_embedded
         FROM _lava_media_items
         ORDER BY is_embedded ASC, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;

    // Same source file checked in more than once (e.g. by two photo modules)
    // should appear once. Keyed on the non-empty source path. Note: this does
    // not merge differently-keyed thumbnails of one asset across modules — see
    // docs/spike-ileapp.md.
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    let conn = cache.conn();
    for row in rows {
        let (id, source_path, extraction_path, mime) = row?;
        if let Some(path) = source_path.as_deref() {
            if !path.is_empty() && !seen_paths.insert(path.to_string()) {
                continue; // already inserted this source file
            }
        }
        let kind = media_kind(mime.as_deref());
        let source = if has_refs {
            media_source(lava, &id)?
        } else {
            None
        };
        let local_path = match extraction_path {
            Some(rel) => resolve_extraction_path(engine_out_dir, &rel, report),
            None => None,
        };
        conn.execute(
            "INSERT OR REPLACE INTO media_items
                 (engine_media_id, domain, relative_path, kind, source, mime_type, local_path)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                id,
                source_path.unwrap_or_default(),
                kind,
                source,
                mime,
                local_path.map(|p| p.to_string_lossy().into_owned()),
            ],
        )?;
        report.media_items += 1;
    }
    Ok(())
}

/// The app/artifact a media item was found in, from `_lava_media_references`
/// (which app referenced it). Uses the friendlier `artifact_name`; if a photo
/// is referenced by several artifacts, takes the first.
fn media_source(lava: &Connection, media_item_id: &str) -> Result<Option<String>> {
    Ok(lava
        .query_row(
            "SELECT artifact_name FROM _lava_media_references
             WHERE media_item_id = ?1 AND artifact_name IS NOT NULL
             ORDER BY artifact_name LIMIT 1",
            [media_item_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|a| friendly_source(&a)))
}

/// Map an iLEAPP artifact name to a gallery-friendly source label.
fn friendly_source(artifact: &str) -> String {
    match artifact {
        "SMS" => "Messages".to_string(),
        // Both camera-roll modules collapse to one "Photos" source.
        "Photos.sqlite Metadata" | "Photos.sqlite EXIF Analysis" => "Photos".to_string(),
        other => other.to_string(),
    }
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
fn normalize_sms(
    lava: &Connection,
    engine_out_dir: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(lava, "sms")? {
        // No messages parsed — normal, not an error.
        return Ok(());
    }

    // Group names + participants come from the raw sms.db (iLEAPP's sms artifact
    // exposes neither), keyed by chat.ROWID == our thread identifier. The same
    // sms.db also gives us the per-message sender (message.handle_id), which the
    // lava `sms` table lacks — needed to attribute group-chat messages.
    let sms_db = find_extracted(engine_out_dir, "sms.db");
    let chats = sms_db
        .as_ref()
        .and_then(|p| crate::parsers::chats::parse_chats(p).ok())
        .unwrap_or_default();
    let senders = sms_db
        .as_ref()
        .and_then(|p| crate::parsers::chats::parse_message_senders(p).ok())
        .unwrap_or_default();

    let mut stmt = lava.prepare(
        "SELECT chat_id, chat_contact_id, service, message_timestamp,
                message, from_me, attachment_file, message_row_id
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
            // lava columns are TEXT-affinity; accept an int or a numeric string.
            message_row_id: match r.get::<_, rusqlite::types::Value>(7)? {
                rusqlite::types::Value::Integer(i) => Some(i),
                rusqlite::types::Value::Text(s) => s.parse().ok(),
                _ => None,
            },
        })
    })?;

    let conn = cache.conn();
    // Group consecutive rows (query is ordered by chat_id) into threads.
    let mut current_key: Option<String> = None;
    let mut thread_id: i64 = 0;
    let mut current_is_group = false;
    for row in rows {
        let row = row?;
        let key = row
            .chat_id
            .clone()
            .unwrap_or_else(|| row.contact_id.clone().unwrap_or_else(|| "unknown".into()));
        if current_key.as_ref() != Some(&key) {
            // Enrich with native chat metadata (group name + members). For a
            // group we show its name (or, later, member names); for a 1:1 the
            // display handle stays the contact id.
            let info = key.parse::<i64>().ok().and_then(|id| chats.get(&id));
            let participants = info.map(|i| i.participants.clone()).unwrap_or_default();
            let is_group = participants.len() > 1;
            current_is_group = is_group;
            let display_name = if is_group {
                info.and_then(|i| i.display_name.clone())
            } else {
                row.contact_id.clone()
            };
            conn.execute(
                "INSERT INTO threads (identifier, display_name, service, last_message_at, message_count, participants_json)
                 VALUES (?1, ?2, ?3, NULL, 0, ?4)",
                rusqlite::params![
                    key,
                    display_name,
                    row.service,
                    serde_json::to_string(&participants).unwrap_or_else(|_| "[]".into()),
                ],
            )?;
            thread_id = conn.last_insert_rowid();
            current_key = Some(key);
            report.threads += 1;
        }

        let is_from_me = matches!(row.from_me.as_deref(), Some("1"));
        let has_attachment = row.media_ref.is_some();
        // Per-message sender from sms.db (real member in group chats). For a 1:1
        // fall back to the chat contact id; for a group, leave it unknown rather
        // than stamp the group identifier as if it were a member.
        let sender = if is_from_me {
            None
        } else {
            row.message_row_id
                .and_then(|rid| senders.get(&rid).cloned())
                .or_else(|| {
                    if current_is_group {
                        None
                    } else {
                        row.contact_id.clone()
                    }
                })
        };
        conn.execute(
            "INSERT INTO messages
                 (thread_id, sender, is_from_me, body, sent_at, has_attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                thread_id,
                sender,
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
    message_row_id: Option<i64>,
}

/// Map iLEAPP's `callhistory` table into cache `calls`. iLEAPP renders
/// direction/answered/type as strings; we normalize back to booleans/lowercase.
/// Duration comes from the start/end timestamp delta (the text `call_duration`
/// is display-formatted). Missed calls have no end time → 0 duration.
fn normalize_calls(lava: &Connection, cache: &CacheDb, report: &mut ImportReport) -> Result<()> {
    if !table_exists(lava, "callhistory")? {
        return Ok(());
    }
    let mut stmt = lava.prepare(
        "SELECT starting_timestamp, ending_timestamp, phone_number,
                call_direction, answered, call_type
         FROM callhistory
         ORDER BY starting_timestamp DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<i64>>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;

    let conn = cache.conn();
    for row in rows {
        let (start, end, address, direction, answered, call_type) = row?;
        let duration = match (start, end) {
            (Some(s), Some(e)) if e >= s => e - s,
            _ => 0,
        };
        conn.execute(
            "INSERT INTO calls (address, direction, answered, duration_s, occurred_at, service)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                address,
                direction.as_deref().map(str::to_lowercase),
                answered.as_deref().map(|a| (a == "Yes") as i64),
                duration,
                start,
                call_type,
            ],
        )?;
        report.calls += 1;
    }
    Ok(())
}

/// Map iLEAPP's `safarihistory` table into cache `safari_history`. `visit_count`
/// arrives as text; parse it to an integer where possible.
fn normalize_safari(lava: &Connection, cache: &CacheDb, report: &mut ImportReport) -> Result<()> {
    if !table_exists(lava, "safarihistory")? {
        return Ok(());
    }
    let mut stmt = lava.prepare(
        "SELECT url, title, visit_timestamp, visit_count
         FROM safarihistory
         WHERE url IS NOT NULL
         ORDER BY visit_timestamp DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;

    let conn = cache.conn();
    for row in rows {
        let (url, title, visited_at, visit_count) = row?;
        conn.execute(
            "INSERT INTO safari_history (url, title, visited_at, visit_count)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                url,
                title,
                visited_at,
                visit_count.and_then(|c| c.parse::<i64>().ok()),
            ],
        )?;
        report.safari_visits += 1;
    }
    Ok(())
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
            "CREATE TABLE _lava_media_items (id TEXT, source_path TEXT, extraction_path TEXT, type TEXT, is_embedded INTEGER);
             INSERT INTO _lava_media_items VALUES ('m1', 'x', 'media/gone.png', 'image/png', 0);",
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
    fn dedups_media_by_source_and_labels_photos() {
        // Mimics a camera-roll asset checked in twice for the same source file:
        // a non-embedded original and an embedded generated thumbnail, plus a
        // second distinct Photos asset. The dupe must collapse to the original,
        // and both survivors must be sourced "Photos".
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        fs::create_dir_all(tmp.path().join("media")).unwrap();
        fs::write(tmp.path().join("media/orig.heic"), b"heic").unwrap();
        fs::write(tmp.path().join("media/thumb.jpg"), b"jpg").unwrap();
        fs::write(tmp.path().join("media/other.jpg"), b"jpg2").unwrap();
        conn.execute_batch(
            "CREATE TABLE _lava_media_items (
                 id TEXT, source_path TEXT, extraction_path TEXT, type TEXT, is_embedded INTEGER);
             CREATE TABLE _lava_media_references (
                 id TEXT, media_item_id TEXT, module_name TEXT, artifact_name TEXT, name TEXT);
             -- Same source file, two check-ins: embedded thumbnail + original.
             INSERT INTO _lava_media_items VALUES
                 ('m_thumb', 'Media/DCIM/IMG_1.HEIC', 'media/thumb.jpg', 'image/jpeg', 1);
             INSERT INTO _lava_media_items VALUES
                 ('m_orig', 'Media/DCIM/IMG_1.HEIC', 'media/orig.heic', 'image/heic', 0);
             -- A second, distinct asset.
             INSERT INTO _lava_media_items VALUES
                 ('m_other', 'Media/DCIM/IMG_2.JPG', 'media/other.jpg', 'image/jpeg', 0);
             INSERT INTO _lava_media_references VALUES
                 ('r1', 'm_thumb', 'photosMetadata', 'Photos.sqlite Metadata', 'IMG_1');
             INSERT INTO _lava_media_references VALUES
                 ('r2', 'm_orig', 'photosDbexif', 'Photos.sqlite EXIF Analysis', 'IMG_1');
             INSERT INTO _lava_media_references VALUES
                 ('r3', 'm_other', 'photosMetadata', 'Photos.sqlite Metadata', 'IMG_2');",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();

        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        // Two visible items: the deduped IMG_1 and the distinct IMG_2.
        assert_eq!(report.media_items, 2);

        let c = cache.conn();
        let total: i64 = c
            .query_row("SELECT COUNT(*) FROM media_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2);
        // The kept IMG_1 row is the non-embedded original (heic), not the thumb.
        let (kind, engine_id): (String, String) = c
            .query_row(
                "SELECT kind, engine_media_id FROM media_items
                 WHERE relative_path = 'Media/DCIM/IMG_1.HEIC'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(engine_id, "m_orig");
        assert_eq!(kind, "photo");
        // Both items are labeled "Photos".
        let photos: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM media_items WHERE source = 'Photos'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(photos, 2);
    }

    #[test]
    fn normalizes_calls_from_callhistory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        // Real callhistory lava schema (see docs/spike-ileapp.md).
        conn.execute_batch(
            "CREATE TABLE callhistory (
                 starting_timestamp INTEGER, ending_timestamp INTEGER, service_provider TEXT,
                 call_type TEXT, call_direction TEXT, phone_number TEXT, answered TEXT,
                 call_duration TEXT, facetime_data TEXT, disconnected_cause TEXT,
                 iso_country_code TEXT, location TEXT);
             INSERT INTO callhistory VALUES
                 (1717783200, 1717783512, 'x', 'Phone Call', 'Outgoing', '+15551234567', 'Yes', '00:05:12', NULL, 'Ended', 'US', NULL);
             INSERT INTO callhistory VALUES
                 (1717785000, NULL, 'x', 'Phone Call', 'Incoming', '+15559876543', 'No', '00:00:00', NULL, 'Ended', 'US', NULL);",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report.calls, 2);

        let c = cache.conn();
        // Outgoing answered call with a 312s duration.
        let (addr, dir, ans, dur): (String, String, i64, i64) = c
            .query_row(
                "SELECT address, direction, answered, duration_s FROM calls WHERE address = '+15551234567'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (addr.as_str(), dir.as_str(), ans, dur),
            ("+15551234567", "outgoing", 1, 312)
        );
        // Missed incoming call: not answered, zero duration (no end time).
        let (ans2, dur2): (i64, i64) = c
            .query_row(
                "SELECT answered, duration_s FROM calls WHERE address = '+15559876543'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((ans2, dur2), (0, 0));
    }

    #[test]
    fn normalizes_safari_history() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE safarihistory (
                 visit_timestamp INTEGER, title TEXT, url TEXT, visit_count TEXT,
                 redirect_source TEXT, redirect_destination TEXT, visit_id TEXT, origin TEXT);
             INSERT INTO safarihistory VALUES
                 (1717794000, 'Apple', 'https://www.apple.com/', '12', '', '', '1', 'Local Device');
             INSERT INTO safarihistory VALUES
                 (1717797600, 'HN', 'https://news.ycombinator.com/', 'notanumber', '', '', '2', 'Local Device');",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report.safari_visits, 2);

        let c = cache.conn();
        let (title, count): (String, i64) = c
            .query_row(
                "SELECT title, visit_count FROM safari_history WHERE url = 'https://www.apple.com/'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((title.as_str(), count), ("Apple", 12));
        // Non-numeric visit_count degrades to NULL, not an error.
        let count2: Option<i64> = c
            .query_row(
                "SELECT visit_count FROM safari_history WHERE title = 'HN'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count2, None);
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
