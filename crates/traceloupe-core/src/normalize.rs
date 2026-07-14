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
    pub recordings: usize,
    /// Non-fatal problems (a skipped artifact, a media ref with no bytes).
    pub warnings: Vec<String>,
}

/// Normalize the lava DB at `lava_path` into `cache`. `engine_out_dir` is the
/// iLEAPP output folder that lava's `extraction_path`s are relative to.
/// Which iLEAPP stages to skip because the orchestrator already materialized that
/// artifact natively (Phase 2). Each `true` suppresses the corresponding iLEAPP
/// normalize stage so the native and iLEAPP paths never double-insert.
#[derive(Debug, Default, Clone)]
pub struct NativeSkips {
    pub messages: bool,
    pub notes: bool,
    pub calls: bool,
    pub safari: bool,
    pub contacts: bool,
    /// Service labels of third-party chats already materialized natively (e.g.
    /// "WhatsApp"); the matching iLEAPP app-conversation stage is skipped.
    pub app_services: Vec<&'static str>,
}

pub fn normalize_lava(
    lava_path: &Path,
    engine_out_dir: &Path,
    cache: &CacheDb,
) -> Result<ImportReport> {
    normalize_lava_with_progress(
        lava_path,
        engine_out_dir,
        cache,
        NativeSkips::default(),
        |_| {},
    )
}

/// Like [`normalize_lava`], but calls `on_step` with a human label before each
/// sub-stage so the UI can show live progress during the (potentially long)
/// normalize pass instead of one opaque "organizing" spinner.
/// `skips` suppresses each iLEAPP stage the caller already handled natively
/// (Phase 2) — Messages from `sms.db`, Notes from `NoteStore.sqlite`, Calls from
/// `CallHistory.storedata`, Safari from `History.db` — to avoid double-inserting.
pub fn normalize_lava_with_progress(
    lava_path: &Path,
    engine_out_dir: &Path,
    cache: &CacheDb,
    skips: NativeSkips,
    mut on_step: impl FnMut(&str),
) -> Result<ImportReport> {
    let lava = Connection::open_with_flags(lava_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut report = ImportReport::default();

    // Isolate each stage: a failure in one artifact (a malformed iLEAPP table, a
    // missing column) becomes a warning and the rest of the import still
    // completes, rather than aborting everything. Each normalizer runs in its own
    // transaction, so a failed stage rolls back its partial work cleanly.
    macro_rules! stage {
        ($label:literal, $call:expr) => {{
            on_step($label);
            if let Err(e) = $call {
                report
                    .warnings
                    .push(format!("{} could not be imported: {}", $label, e));
            }
        }};
    }

    stage!(
        "Media",
        normalize_media(&lava, engine_out_dir, cache, &mut report)
    );
    // Messages: skipped when the orchestrator already materialized them natively
    // from sms.db (Phase 2). Otherwise the iLEAPP `sms` path runs.
    if !skips.messages {
        stage!(
            "Messages",
            normalize_sms(&lava, engine_out_dir, cache, &mut report)
        );
    }
    // Calls / Safari: skipped when materialized natively from CallHistory.storedata
    // / History.db (Phase 2); otherwise the iLEAPP path runs.
    if !skips.calls {
        stage!("Call history", normalize_calls(&lava, cache, &mut report));
    }
    if !skips.safari {
        stage!(
            "Safari history",
            normalize_safari(&lava, cache, &mut report)
        );
    }
    // Contacts: parsed from the AddressBook DB. Skipped when the orchestrator
    // already self-extracted + parsed it natively (Phase 2); otherwise it parses
    // the copy iLEAPP extracted. A missing DB just means no contacts.
    if !skips.contacts {
        stage!(
            "Contacts",
            normalize_contacts(engine_out_dir, cache, &mut report)
        );
    }
    // Notes: skipped when the orchestrator materialized them natively from
    // NoteStore.sqlite (Phase 2). Otherwise the iLEAPP `notes` path runs.
    if !skips.notes {
        stage!("Notes", normalize_notes(&lava, cache, &mut report));
    }
    // Third-party chat apps → the Messages view, tagged by service. Each is a
    // no-op unless its lava table is present (the app was installed + parsed), and
    // is skipped entirely when the app was already materialized natively.
    if !skips.app_services.contains(&TIKTOK_CHAT.service) {
        stage!(
            "TikTok messages",
            normalize_app_conversation(&lava, cache, &mut report, &TIKTOK_CHAT)
        );
    }
    if !skips.app_services.contains(&WHATSAPP_CHAT.service) {
        stage!(
            "WhatsApp messages",
            normalize_app_conversation(&lava, cache, &mut report, &WHATSAPP_CHAT)
        );
    }
    if !skips.app_services.contains(&TELEGRAM_CHAT.service) {
        stage!(
            "Telegram messages",
            normalize_app_conversation(&lava, cache, &mut report, &TELEGRAM_CHAT)
        );
    }
    stage!(
        "TikTok contacts",
        normalize_tiktok_contacts(&lava, cache, &mut report)
    );

    Ok(report)
}

/// Map iLEAPP's `tiktok_contacts` (the TikTok social graph) into cache
/// `contacts`, tagged `source = "TikTok"` so the Contacts view can filter them
/// away from the device's address book. These carry only a nickname + `@handle`
/// (no phone/email, and the avatar URL is remote/expired so we don't fetch it),
/// so the handle goes in `organization` to show as the contact's subtitle.
fn normalize_tiktok_contacts(
    lava: &Connection,
    cache: &CacheDb,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(lava, "tiktok_contacts")? {
        return Ok(());
    }
    let mut stmt = lava.prepare(
        "SELECT unique_id, nickname, custom_id FROM tiktok_contacts
         WHERE nickname IS NOT NULL OR custom_id IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    // A person recurs across source tables (friends, followers, …); dedup by uid.
    let mut seen = std::collections::HashSet::new();
    while let Some(r) = rows.next()? {
        let unique_id: Option<String> = r.get(0)?;
        let nickname: Option<String> = r.get(1)?;
        let custom_id: Option<String> = r.get(2)?;
        if let Some(uid) = &unique_id {
            if !seen.insert(uid.clone()) {
                continue;
            }
        }
        let handle = custom_id.map(|h| format!("@{h}"));
        tx.execute(
            "INSERT INTO contacts
                 (first_name, last_name, organization, phones_json, emails_json, image, source)
             VALUES (?1, NULL, ?2, '[]', '[]', NULL, 'TikTok')",
            rusqlite::params![nickname, handle],
        )?;
        report.contacts += 1;
    }
    tx.commit()?;
    Ok(())
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
    let tx = conn.unchecked_transaction()?;
    for row in rows {
        let row = row?;
        tx.execute(
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
    tx.commit()?;
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

    report.contacts +=
        crate::parsers::address_book::insert_contacts(cache, &contacts, &images, false)?;
    Ok(())
}

/// Depth-first search under `root` for a file named `name`. iLEAPP nests
/// extracted files under `data/<domain path>/…`, so we can't hard-code a path.
fn find_extracted(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        // Skip an unreadable directory rather than abandoning the whole search —
        // one permission-denied folder shouldn't hide sms.db / the address book.
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
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

    // Batch the inserts: one transaction instead of an fsync per row.
    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
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
        tx.execute(
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
    tx.commit()?;
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

    // Order by the SAME effective key the grouping uses below
    // (COALESCE(chat_id, chat_contact_id, 'unknown')), not by chat_id alone:
    // otherwise rows with a NULL chat_id but different contacts interleave by
    // time and the consecutive-grouping logic splits each conversation into many
    // one-message threads.
    let mut stmt = lava.prepare(
        "SELECT chat_id, chat_contact_id, service, message_timestamp,
                message, from_me, attachment_file, message_row_id
         FROM sms
         ORDER BY COALESCE(chat_id, chat_contact_id, 'unknown'), message_timestamp",
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

    // Loop-invariant: whether the lava has a media-references table (probed once
    // rather than per attachment).
    let has_media_refs = table_exists(lava, "_lava_media_references")?;

    // One transaction for the whole table — messages run to tens of thousands of
    // rows; a commit (and fsync) per row is what made import stall.
    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
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
            tx.execute(
                "INSERT INTO threads (identifier, display_name, service, last_message_at, message_count, participants_json)
                 VALUES (?1, ?2, ?3, NULL, 0, ?4)",
                rusqlite::params![
                    key,
                    display_name,
                    row.service,
                    serde_json::to_string(&participants).unwrap_or_else(|_| "[]".into()),
                ],
            )?;
            thread_id = tx.last_insert_rowid();
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
        tx.execute(
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
        let message_id = tx.last_insert_rowid();
        report.messages += 1;

        // Link an attachment to its cache media_item. An artifact's
        // `attachment_file` is a `_lava_media_references.id`, which points via
        // `media_item_id` at the `_lava_media_items.id` we stored as
        // `engine_media_id`. Resolve that indirection (falling back to treating
        // the value as a media-item id directly, for engines without the
        // references table).
        if let Some(media_ref) = row.media_ref {
            let media_item_id = resolve_media_item_id(lava, &media_ref, has_media_refs)?;
            let inserted = tx.execute(
                "INSERT INTO attachments (message_id, filename, mime_type, local_path)
                 SELECT ?1, relative_path, mime_type, local_path
                 FROM media_items WHERE engine_media_id = ?2",
                rusqlite::params![message_id, media_item_id],
            )?;
            if inserted == 0 {
                // The referenced media isn't in the cache — clear the flag so the
                // UI doesn't show an attachment indicator with nothing behind it.
                tx.execute(
                    "UPDATE messages SET has_attachments = 0 WHERE id = ?1",
                    [message_id],
                )?;
                report.warnings.push(format!(
                    "message {message_id} references unknown media id {media_ref}"
                ));
            }
        }
    }

    // Denormalize per-thread counters used by the thread list. Scoped to the
    // threads this pass created (native SMS/iMessage) so it can't clobber the
    // app normalizers' counters regardless of dispatch order.
    tx.execute(
        "UPDATE threads SET
             message_count = (SELECT COUNT(*) FROM messages WHERE messages.thread_id = threads.id),
             last_message_at = (SELECT MAX(sent_at) FROM messages WHERE messages.thread_id = threads.id)
         WHERE service IS NULL OR service NOT IN ('TikTok', 'WhatsApp', 'Telegram')",
        [],
    )?;
    tx.commit()?;
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

/// How to read one third-party chat app's iLEAPP messages table into the shared
/// `threads`/`messages` schema. All the apps iLEAPP parses share a "conversation"
/// shape — they differ only in table and column names. Column fields are trusted
/// constants (SQL identifiers or expressions), never user input.
struct AppChatSpec {
    /// Lava table name (iLEAPP function name, sanitized), e.g. "tiktok_messages".
    table: &'static str,
    /// Value stamped on each thread's `service` (and the app filter label).
    service: &'static str,
    /// Column that groups messages into a thread.
    chat_id: &'static str,
    /// Unix-epoch-seconds timestamp column.
    timestamp: &'static str,
    /// Message text column (rows where it's NULL — media-only — are skipped).
    body: &'static str,
    /// Direction column whose value is "Outgoing" for messages the owner sent.
    direction: &'static str,
    /// Sender display-name column (expression allowed), used for incoming rows.
    sender: &'static str,
    /// Optional `@handle` column captured for thread enrichment (TikTok only).
    handle: Option<&'static str>,
    /// Optional per-row thread label (WhatsApp/Telegram carry the chat name);
    /// when absent, the thread is named after the first incoming sender.
    chat_name: Option<&'static str>,
    /// Optional raw sender-id column (a stable uid, distinct from the display
    /// name). Only used in derive-name mode to count distinct participants so a
    /// group chat is labelled by headcount instead of a raw conversation id.
    sender_id: Option<&'static str>,
}

const TIKTOK_CHAT: AppChatSpec = AppChatSpec {
    table: "tiktok_messages",
    service: "TikTok",
    chat_id: "conversation_id",
    timestamp: "timestamp",
    body: "message",
    direction: "direction",
    sender: "COALESCE(nickname, custom_id)",
    handle: Some("custom_id"),
    chat_name: None,
    sender_id: Some("sender"),
};

const WHATSAPP_CHAT: AppChatSpec = AppChatSpec {
    table: "whatsappmessages",
    service: "WhatsApp",
    chat_id: "chat_id",
    timestamp: "timestamp",
    body: "message",
    direction: "direction",
    sender: "sender_name",
    handle: None,
    chat_name: Some("chat_name"),
    sender_id: None,
};

const TELEGRAM_CHAT: AppChatSpec = AppChatSpec {
    table: "telegrammessages",
    service: "Telegram",
    chat_id: "chat_id",
    timestamp: "timestamp",
    body: "text",
    direction: "direction",
    sender: "author",
    handle: None,
    chat_name: Some("chat"),
    sender_id: None,
};

/// Map a third-party chat app's iLEAPP output into cache `threads` + `messages`,
/// tagged with `spec.service` so the Messages view's app filter can surface it.
/// Rows group into threads by the chat id; `direction` = "Outgoing" is from-me;
/// threads are named by their per-row chat label, or (TikTok) by the first
/// incoming sender, whose `@handle` is stored as the sole participant so the
/// header can show it. Media rows (NULL body) are kept so a conversation that is
/// only media stays visible and its senders still count toward the group
/// headcount. One transaction — these run to hundreds of thousands of rows.
fn normalize_app_conversation(
    lava: &Connection,
    cache: &CacheDb,
    report: &mut ImportReport,
    spec: &AppChatSpec,
) -> Result<()> {
    if !table_exists(lava, spec.table)? {
        return Ok(());
    }
    // Columns are trusted constants from the spec, not user input.
    let sql = format!(
        "SELECT {chat}, {ts}, {body}, {dir}, {sender}, {handle}, {chat_name}, {sender_id}
         FROM {table}
         ORDER BY COALESCE({chat}, 'unknown'), {ts}",
        chat = spec.chat_id,
        ts = spec.timestamp,
        body = spec.body,
        dir = spec.direction,
        sender = spec.sender,
        handle = spec.handle.unwrap_or("NULL"),
        chat_name = spec.chat_name.unwrap_or("NULL"),
        sender_id = spec.sender_id.unwrap_or("NULL"),
        table = spec.table,
    );
    let mut stmt = lava.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;

    let mut current_key: Option<String> = None;
    let mut thread_id: i64 = 0;
    // Derived (no chat-name column): the peer's display name + @handle, taken
    // from the first incoming message and applied when the conversation ends.
    let derive_name = spec.chat_name.is_none();
    let mut peer_nick: Option<String> = None;
    let mut peer_handle: Option<String> = None;
    // Distinct incoming sender ids in the current conversation, to tell a group
    // (many senders) from a 1:1 (one) in derive-name mode.
    let mut member_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    let finalize = |tx: &rusqlite::Connection,
                    id: i64,
                    key: &str,
                    nick: &mut Option<String>,
                    handle: &mut Option<String>,
                    members: &mut std::collections::HashSet<String>|
     -> Result<()> {
        let member_count = members.len();
        members.clear();
        // A group chat: either several distinct senders wrote text, or (TikTok)
        // the conversation id is a bare number — 1:1 ids embed the two user ids
        // separated by ':'. Group members usually aren't in the user's contacts
        // (their names come back NULL), so name the thread rather than leak the
        // raw id, which is what the user would otherwise see as the "recipient".
        // A bare all-digit id is a TikTok group (1:1 ids embed both user ids with
        // ':'); only trust this in derive mode so it never overrides a real
        // chat name (WhatsApp/Telegram), whose numeric chat ids aren't groups.
        let id_is_group = derive_name && !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit());
        if member_count > 1 || id_is_group {
            let label = if member_count > 1 {
                format!("Group chat · {} people", member_count + 1)
            } else {
                "Group chat".to_string()
            };
            nick.take();
            handle.take();
            tx.execute(
                "UPDATE threads SET display_name = ?1, participants_json = '[]' WHERE id = ?2",
                rusqlite::params![label, id],
            )?;
        } else {
            // 1:1 (or chat-name mode): the peer @handle is the sole participant so
            // the header can show it. COALESCE keeps a chat-name set at insert.
            let participants: Vec<String> = handle.take().into_iter().collect();
            let pj = serde_json::to_string(&participants).unwrap_or_else(|_| "[]".into());
            tx.execute(
                "UPDATE threads SET display_name = COALESCE(?1, display_name),
                     participants_json = ?2 WHERE id = ?3",
                rusqlite::params![nick.take(), pj, id],
            )?;
        }
        Ok(())
    };

    while let Some(r) = rows.next()? {
        let key: String = r
            .get::<_, Option<String>>(0)?
            .unwrap_or_else(|| "unknown".into());
        let timestamp = epoch_value(r, 1);
        let body: Option<String> = r.get(2)?;
        let direction: Option<String> = r.get(3)?;
        let sender_name: Option<String> = r.get(4)?;
        let handle: Option<String> = r.get(5)?;
        let chat_name: Option<String> = r.get(6)?;
        let sender_id: Option<String> = r.get(7)?;

        if current_key.as_ref() != Some(&key) {
            if let Some(prev) = current_key.as_deref() {
                finalize(
                    &tx,
                    thread_id,
                    prev,
                    &mut peer_nick,
                    &mut peer_handle,
                    &mut member_ids,
                )?;
            }
            // chat_name is NULL for derive-mode apps (filled in at finalize).
            tx.execute(
                "INSERT INTO threads
                    (identifier, display_name, service, last_message_at, message_count, participants_json)
                 VALUES (?1, ?2, ?3, NULL, 0, '[]')",
                rusqlite::params![key, chat_name, spec.service],
            )?;
            thread_id = tx.last_insert_rowid();
            current_key = Some(key);
            peer_nick = None;
            peer_handle = None;
            member_ids.clear();
            report.threads += 1;
        }

        let is_from_me = matches!(direction.as_deref(), Some("Outgoing"));
        let sender = if is_from_me {
            None
        } else {
            sender_name.clone()
        };
        if derive_name && !is_from_me {
            if let Some(sid) = sender_id {
                member_ids.insert(sid);
            }
            if peer_nick.is_none() {
                peer_nick = sender_name;
                peer_handle = handle.map(|h| {
                    if h.starts_with('@') {
                        h
                    } else {
                        format!("@{h}")
                    }
                });
            }
        }
        tx.execute(
            "INSERT INTO messages
                 (thread_id, sender, is_from_me, body, sent_at, has_attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            rusqlite::params![thread_id, sender, is_from_me as i64, body, timestamp],
        )?;
        report.messages += 1;
    }
    if let Some(prev) = current_key.as_deref() {
        finalize(
            &tx,
            thread_id,
            prev,
            &mut peer_nick,
            &mut peer_handle,
            &mut member_ids,
        )?;
    }

    // Denormalize the per-thread counters the thread list reads.
    tx.execute(
        "UPDATE threads SET
             message_count = (SELECT COUNT(*) FROM messages WHERE messages.thread_id = threads.id),
             last_message_at = (SELECT MAX(sent_at) FROM messages WHERE messages.thread_id = threads.id)
         WHERE service = ?1",
        rusqlite::params![spec.service],
    )?;
    tx.commit()?;
    Ok(())
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
    let tx = conn.unchecked_transaction()?;
    for row in rows {
        let (start, end, address, direction, answered, call_type) = row?;
        let duration = match (start, end) {
            (Some(s), Some(e)) if e >= s => e - s,
            _ => 0,
        };
        tx.execute(
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
    tx.commit()?;
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
    let tx = conn.unchecked_transaction()?;
    for row in rows {
        let (url, title, visited_at, visit_count) = row?;
        tx.execute(
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
    tx.commit()?;
    Ok(())
}

/// Translate an artifact's media reference (a `_lava_media_references.id`) to
/// the underlying `_lava_media_items.id`. If the references table is absent or
/// has no match, assume the reference is already a media-item id.
fn resolve_media_item_id(lava: &Connection, media_ref: &str, has_refs: bool) -> Result<String> {
    if has_refs {
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
             VALUES ('media-1', 'private/var/mobile/.../traceloupe-test.png', ?1, 'image/png')",
            [media_rel],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO _lava_media_references (id, media_item_id, module_name, artifact_name, name)
             VALUES ('ref-1', 'media-1', 'sms', 'SMS', 'traceloupe-test.png')",
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
    fn one_malformed_artifact_warns_but_others_import() {
        // An `sms` table missing the `message` column makes normalize_sms fail to
        // prepare its query. Isolation must turn that into a warning and still
        // import the other artifacts (here: media).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        fs::create_dir_all(tmp.path().join("media")).unwrap();
        fs::write(tmp.path().join("media/x.png"), b"bytes").unwrap();
        conn.execute_batch(
            "CREATE TABLE _lava_media_items (id TEXT, source_path TEXT, extraction_path TEXT, type TEXT, is_embedded INTEGER);
             INSERT INTO _lava_media_items VALUES ('m1', 'x', 'media/x.png', 'image/png', 0);
             -- sms table present but missing the `message` column normalize_sms needs.
             CREATE TABLE sms (chat_id TEXT, chat_contact_id TEXT, message_timestamp INTEGER);
             INSERT INTO sms VALUES ('1', '+15551234567', 100);",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();

        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        // Media still imported despite the broken sms table.
        assert_eq!(report.media_items, 1);
        assert_eq!(report.messages, 0);
        assert!(
            report.warnings.iter().any(|w| w.contains("Messages")),
            "expected a Messages warning, got {:?}",
            report.warnings
        );
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
    fn normalizes_tiktok_dms_into_threads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE tiktok_messages (
                 timestamp INTEGER, sender TEXT, custom_id TEXT, nickname TEXT,
                 message TEXT, local_response TEXT, link_gif_name TEXT,
                 link_gif_url TEXT, server_created_timestamp INTEGER,
                 profile_pic_url TEXT, contact_table TEXT, account_id TEXT,
                 source_file TEXT, conversation_id TEXT, direction TEXT);
             -- Conversation A: an incoming then an outgoing text message.
             INSERT INTO tiktok_messages VALUES
                 (1675838000,'peerA','peer.a','Peer A','hi there',NULL,NULL,NULL,0,NULL,NULL,'me','f','convA','Incoming');
             INSERT INTO tiktok_messages VALUES
                 (1675838551,'me','me.handle','Me','hello back',NULL,NULL,NULL,0,NULL,NULL,'me','f','convA','Outgoing');
             -- A non-text share (NULL message) — kept, so media-only rows and
             -- media-only conversations stay visible and count their senders.
             INSERT INTO tiktok_messages VALUES
                 (1675838600,'peerA','peer.a','Peer A',NULL,NULL,NULL,NULL,0,NULL,NULL,'me','f','convA','Incoming');
             -- Conversation B: one outgoing only (peer name unknown → NULL display).
             INSERT INTO tiktok_messages VALUES
                 (1675900000,'me','me.handle','Me','yo',NULL,NULL,NULL,0,NULL,NULL,'me','f','convB','Outgoing');",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report.threads, 2);
        assert_eq!(report.messages, 4); // includes the NULL-message media share

        let c = cache.conn();
        // Conversation A is named after the peer's nickname and counted.
        let (display, count, last): (Option<String>, i64, i64) = c
            .query_row(
                "SELECT display_name, message_count, last_message_at FROM threads
                 WHERE identifier = 'convA' AND service = 'TikTok'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(display.as_deref(), Some("Peer A"));
        assert_eq!(count, 3); // two texts + the media share
        assert_eq!(last, 1675838600); // the media share is the latest

        // from-me / sender attribution.
        let (from_me, sender): (i64, Option<String>) = c
            .query_row(
                "SELECT m.is_from_me, m.sender FROM messages m
                 JOIN threads t ON t.id = m.thread_id
                 WHERE t.identifier = 'convA' AND m.body = 'hi there'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(from_me, 0);
        assert_eq!(sender.as_deref(), Some("Peer A"));
    }

    #[test]
    fn tiktok_group_chats_are_named_not_shown_as_raw_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        // A group: a bare-numeric conversation id and members with no
        // nickname/custom_id (not in the user's contacts) — the bug case.
        conn.execute_batch(
            "CREATE TABLE tiktok_messages (
                 timestamp INTEGER, sender TEXT, custom_id TEXT, nickname TEXT,
                 message TEXT, local_response TEXT, link_gif_name TEXT,
                 link_gif_url TEXT, server_created_timestamp INTEGER,
                 profile_pic_url TEXT, contact_table TEXT, account_id TEXT,
                 source_file TEXT, conversation_id TEXT, direction TEXT);
             INSERT INTO tiktok_messages VALUES
                 (1675838000,'7107289401973097477',NULL,NULL,'hi all',NULL,NULL,NULL,0,NULL,NULL,'me','f','7600077391893709063','Incoming');
             INSERT INTO tiktok_messages VALUES
                 (1675838100,'7525560543878546454',NULL,NULL,'hey',NULL,NULL,NULL,0,NULL,NULL,'me','f','7600077391893709063','Incoming');
             INSERT INTO tiktok_messages VALUES
                 (1675838200,'me',NULL,NULL,'sup',NULL,NULL,NULL,0,NULL,NULL,'me','f','7600077391893709063','Outgoing');",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        normalize_lava(&path, tmp.path(), &cache).unwrap();

        let (display, identifier): (Option<String>, String) = cache
            .conn()
            .query_row(
                "SELECT display_name, identifier FROM threads WHERE service = 'TikTok'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        // Named "Group chat · N people", never the raw numeric conversation id.
        assert!(
            display.as_deref().unwrap_or("").starts_with("Group chat"),
            "group not labelled: {display:?}",
        );
        assert_eq!(identifier, "7600077391893709063");
    }

    #[test]
    fn normalizes_tiktok_contacts_with_source_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE tiktok_contacts (
                 timestamp INTEGER, nickname TEXT, unique_id TEXT, custom_id TEXT,
                 url TEXT, source_table TEXT, source_file TEXT);
             INSERT INTO tiktok_contacts VALUES (0,'Alice','111','alice_h','u',NULL,NULL);
             -- Same person in a second source table → deduped by unique_id.
             INSERT INTO tiktok_contacts VALUES (0,'Alice','111','alice_h','u',NULL,NULL);
             INSERT INTO tiktok_contacts VALUES (0,'Bob','222','bobby','u',NULL,NULL);",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report.contacts, 2); // deduped

        let c = cache.conn();
        let (name, org, source): (String, String, String) = c
            .query_row(
                "SELECT first_name, organization, source FROM contacts WHERE first_name = 'Alice'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), org.as_str(), source.as_str()),
            ("Alice", "@alice_h", "TikTok")
        );
    }

    #[test]
    fn normalizes_whatsapp_and_telegram_via_shared_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("_lava_artifacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE whatsappmessages (
                 timestamp INTEGER, sender_name TEXT, from_id TEXT, receiver TEXT,
                 to_id TEXT, message TEXT, attachment_file TEXT, thumb TEXT,
                 starred TEXT, number_of_forwardings TEXT, forwarded_from TEXT,
                 latitude TEXT, longitude TEXT, direction TEXT, chat_id TEXT, chat_name TEXT);
             INSERT INTO whatsappmessages VALUES
                 (1700000000,'Sam','x','','','hey there',NULL,NULL,'',0,'','','','Incoming','jid1','Sam Q');
             INSERT INTO whatsappmessages VALUES
                 (1700000060,'Local User','','','','hi Sam',NULL,NULL,'',0,'','','','Outgoing','jid1','Sam Q');
             CREATE TABLE telegrammessages (
                 timestamp INTEGER, chat TEXT, chat_id TEXT, thread_id TEXT,
                 direction TEXT, author TEXT, author_id TEXT, text TEXT,
                 action_data TEXT, thumbnail TEXT, forward_from TEXT, forward_timestamp INTEGER);
             INSERT INTO telegrammessages VALUES
                 (1700001000,'Group Chat','g99','','Incoming','Kim','5','yo everyone','','','',0);",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let report = normalize_lava(&path, tmp.path(), &cache).unwrap();
        assert_eq!(report.threads, 2); // one WhatsApp, one Telegram
        assert_eq!(report.messages, 3);

        let c = cache.conn();
        // WhatsApp: thread named by chat_name, direction mapped, from-me sender dropped.
        let (name, service, from_me): (String, String, i64) = c
            .query_row(
                "SELECT t.display_name, t.service, m.is_from_me FROM messages m
                 JOIN threads t ON t.id = m.thread_id WHERE m.body = 'hi Sam'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), service.as_str(), from_me),
            ("Sam Q", "WhatsApp", 1)
        );

        // Telegram: thread named by `chat`, sender preserved for incoming.
        let (tname, sender): (String, Option<String>) = c
            .query_row(
                "SELECT t.display_name, m.sender FROM messages m
                 JOIN threads t ON t.id = m.thread_id WHERE t.service = 'Telegram'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(tname.as_str(), "Group Chat");
        assert_eq!(sender.as_deref(), Some("Kim"));
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
