//! Native Messages parser (Phase 2): reads a decrypted `sms.db` directly into the
//! cache `threads` + `messages`, so Messages can be materialized natively —
//! without iLEAPP's eager whole-backup pass. Locate + decrypt `sms.db` via the
//! [`crate::manifest::ManifestIndex`], then call [`parse_messages`].
//!
//! This produces the same cache shape as the iLEAPP path
//! ([`crate::normalize`]): threads keyed by `chat.ROWID`, group names/members
//! from `chat`/`chat_handle_join`, per-message sender from `handle`, and
//! Apple-absolute timestamps converted to Unix seconds.
//!
//! provenance: reference (own implementation) from the iMessage `chat.db`/`sms.db`
//! schema (the same schema `chats.rs` reads for metadata).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::crypto::{self, BackupDecryptor};
use crate::manifest::ManifestIndex;
use crate::normalize::ImportReport;
use crate::Result;

/// Context for resolving a message's attachments to their files in the backup.
/// When `None`, attachment rows aren't written (e.g. a caller with no Manifest,
/// or tests) — the messages themselves still parse.
pub struct AttachmentSource<'a> {
    pub index: &'a ManifestIndex,
    /// `Some` for an encrypted backup — attachment blobs are then ciphertext and
    /// their wrapped keys are stored for on-demand decryption at view time.
    pub decryptor: Option<&'a BackupDecryptor>,
}

/// Apple absolute time counts from 2001-01-01 UTC. iOS 11+ stores nanoseconds;
/// older backups store seconds. Convert to Unix epoch seconds; 0 → None.
const MAC_EPOCH: i64 = 978_307_200;
fn mac_to_unix(date: i64) -> Option<i64> {
    if date == 0 {
        return None;
    }
    // Nanoseconds if the value is far larger than any plausible seconds count.
    // `unsigned_abs` avoids the `i64::MIN` overflow panic `abs()` has in debug.
    let secs = if date.unsigned_abs() > 1_000_000_000_000 {
        date / 1_000_000_000
    } else {
        date
    };
    Some(secs + MAC_EPOCH)
}

/// A tapback's emoji from its `associated_message_type`. 2000–2005 are the six
/// built-in tapbacks; 2006/2007 carry a custom emoji/sticker in
/// `associated_message_emoji`. Removals (3000–3007) return None (handled by the
/// caller as "clear this reactor's reaction"). Anything else is not a tapback.
fn reaction_emoji(kind: i64, custom: Option<&str>) -> Option<String> {
    match kind {
        2000 => Some("❤️".into()),
        2001 => Some("👍".into()),
        2002 => Some("👎".into()),
        2003 => Some("😂".into()),
        2004 => Some("‼️".into()),
        2005 => Some("❓".into()),
        2006 | 2007 => custom
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// The target message's GUID from an `associated_message_guid`, which is stored
/// as `p:<part>/<GUID>` or `bp:<GUID>` — the real GUID is the trailing component.
fn associated_target_guid(raw: &str) -> &str {
    raw.rsplit(['/', ':']).next().unwrap_or(raw)
}

/// The column names of `table` (empty on error / missing table).
fn message_table_columns(conn: &Connection, table: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(r)) = rows.next() {
                if let Ok(name) = r.get::<_, String>(1) {
                    set.insert(name);
                }
            }
        }
    }
    set
}

/// A human phrase for an iMessage group-action row (`item_type` != 0), told from
/// the actor's perspective (the UI prefixes the sender). `None` for normal
/// messages and action types whose meaning we don't surface.
fn group_action_body(item_type: i64, action_type: i64, group_title: Option<&str>) -> Option<String> {
    Some(match item_type {
        1 => match group_title {
            Some(t) if !t.trim().is_empty() => format!("named the conversation \u{201c}{t}\u{201d}"),
            _ => "changed the group name".to_string(),
        },
        2 => match action_type {
            1 => "removed a participant".to_string(),
            _ => "added a participant".to_string(),
        },
        3 => "left the conversation".to_string(),
        4 => "changed the group photo".to_string(),
        _ => return None,
    })
}

/// Recover a message's plain text from its `attributedBody` — an NSAttributedString
/// serialized in Apple's `streamtyped` (typedstream) format. Modern iMessage leaves
/// `message.text` NULL and stores the content only here. The text is the first
/// `NSString`: after its class marker comes a `+` (0x2b) inline-string marker, a
/// length (one byte, or `0x81`+u16 / `0x82`+u32), then the UTF-8 bytes.
///
/// Validated against a real backup: reproduces `message.text` exactly on 3000/3000
/// messages that carry both.
fn attributed_body_text(blob: &[u8]) -> Option<String> {
    let pos = blob.windows(8).position(|w| w == b"NSString")?;
    let rest = &blob[pos + 8..];
    let plus = rest.iter().position(|&b| b == 0x2b)?;
    let mut i = plus + 1;
    let len = match *rest.get(i)? {
        0x81 => {
            let l = u16::from_le_bytes([*rest.get(i + 1)?, *rest.get(i + 2)?]) as usize;
            i += 3;
            l
        }
        0x82 => {
            let l = u32::from_le_bytes([
                *rest.get(i + 1)?,
                *rest.get(i + 2)?,
                *rest.get(i + 3)?,
                *rest.get(i + 4)?,
            ]) as usize;
            i += 5;
            l
        }
        b => {
            i += 1;
            b as usize
        }
    };
    let bytes = rest.get(i..i.checked_add(len)?)?;
    let s = String::from_utf8_lossy(bytes).into_owned();
    (!s.trim().is_empty()).then_some(s)
}

/// A friendly placeholder for an iMessage **app-bubble** message
/// (`balloon_bundle_id`) — Digital Touch, Handwriting, an iMessage app
/// extension (GamePigeon, Spotify, …), etc. These carry no `text`, so without
/// a placeholder they render as empty bubbles or get dropped. Used only when
/// the message has no recoverable text/attachment of its own.
fn balloon_label(bundle_id: &str) -> String {
    // iMessage app extensions: com.apple.messages.MSMessageExtensionBalloonPlugin:<team>:<app-bundle-id>
    if bundle_id.contains("MSMessageExtensionBalloonPlugin") {
        let app = bundle_id.rsplit(':').next().unwrap_or("");
        return match app {
            "com.apple.mobileslideshow.PhotosMessagesApp" => "Shared photos".to_string(),
            "com.apple.findmy.FindMyMessagesApp" => "Shared location (Find My)".to_string(),
            "com.apple.PeopleMessageService.AskToBuy"
            | "com.apple.AskToMessagesHost.AskToMessagesExtension" => "Ask to Buy request".to_string(),
            "com.spotify.client.imessage" => "Spotify".to_string(),
            "com.gamerdelights.gamepigeon.ext" => "GamePigeon".to_string(),
            "com.google.ios.youtube.MessagesExtension" => "YouTube".to_string(),
            // Otherwise name it from the app bundle id's last dotted segment.
            other => {
                let name = other
                    .rsplit('.')
                    .find(|s| !s.is_empty() && !s.eq_ignore_ascii_case("ext"))
                    .unwrap_or("iMessage app");
                format!("{name} (iMessage app)")
            }
        };
    }
    match bundle_id {
        "com.apple.messages.URLBalloonProvider" => "Link".to_string(),
        "com.apple.DigitalTouchBalloonProvider" => "Digital Touch".to_string(),
        "com.apple.Handwriting.HandwritingProvider" => "Handwriting".to_string(),
        _ => "iMessage app".to_string(),
    }
}

/// A single-line preview of a message body, capped at `max` chars (char-safe).
fn truncate_snippet(body: &str, max: usize) -> String {
    let one_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > max {
        let truncated: String = one_line.chars().take(max).collect();
        format!("{truncated}…")
    } else {
        one_line
    }
}

/// Classify an attachment as gallery media by MIME, falling back to the filename
/// extension (sms.db often stores a NULL mime for image attachments). Returns the
/// `media_items.kind` ("photo"/"video"), or None for non-media (docs, vCards, …).
fn media_kind(mime: Option<&str>, filename: Option<&str>) -> Option<&'static str> {
    if let Some(m) = mime {
        if m.starts_with("image/") {
            return Some("photo");
        }
        if m.starts_with("video/") {
            return Some("video");
        }
    }
    let f = filename.unwrap_or("").to_ascii_lowercase();
    let ext = f.rsplit('.').next().unwrap_or("");
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "webp" | "tiff" | "tif" | "bmp" => {
            Some("photo")
        }
        "mov" | "mp4" | "m4v" | "avi" | "3gp" | "webm" => Some("video"),
        _ => None,
    }
}

struct Chat {
    identifier: String,
    display_name: Option<String>,
    service: Option<String>,
    participants: Vec<String>,
}

/// SQL predicate for the threads this parser owns — iMessage/SMS, i.e. not the
/// third-party app-chat services. Scopes a `replace` re-import's deletes so
/// TikTok/WhatsApp/Telegram conversations are left intact. A fixed string (no user
/// input), safe to interpolate.
const NATIVE_THREADS: &str = "service IS NULL OR service NOT IN ('TikTok','WhatsApp','Telegram')";

/// Parse a decrypted `sms.db` into the cache's `threads` + `messages`.
///
/// With `replace = false` it appends (for a fresh cache, like the normalizer).
/// With `replace = true` it first deletes this parser's existing rows — child
/// `attachments`, then `messages`, then `threads` (that order satisfies the
/// foreign key) — **inside the same transaction as the re-insert**, so a partial
/// re-import is atomic: a parse failure rolls the deletes back too.
///
/// When `attachments` is `Some`, each message's attachments are resolved to their
/// backup files and written to the `attachments` table (with a wrapped key for
/// encrypted backups, so they decrypt on demand at view time).
pub fn parse_messages(
    sms_db: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
    attachments: Option<&AttachmentSource>,
) -> Result<()> {
    let src = Connection::open_with_flags(sms_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // Attachment tables (only when a caller can resolve files). `attachment.filename`
    // is the on-device path; `transfer_name` the display name.
    let (att_meta, msg_atts) = if attachments.is_some() {
        load_attachments(&src)?
    } else {
        (HashMap::new(), HashMap::new())
    };

    // Message IDs that have any attachment-join row — loaded regardless of whether
    // a caller can resolve the files, so an attachment-only message is kept and
    // flagged even when `cache_has_attachments` is stale (pairs with the SELECT's
    // EXISTS clause below, which is what lets such a row reach this loop at all).
    let att_msg_ids: std::collections::HashSet<i64> =
        if table_exists(&src, "message_attachment_join") {
            let mut stmt =
                src.prepare("SELECT DISTINCT message_id FROM message_attachment_join")?;
            let ids = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            ids
        } else {
            std::collections::HashSet::new()
        };

    // handle.ROWID → phone/email.
    let handles: HashMap<i64, String> = {
        let mut stmt = src.prepare("SELECT ROWID, id FROM handle")?;
        let map = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
            .collect();
        map
    };

    // chat.ROWID → metadata (+ participants from chat_handle_join).
    let mut chats: HashMap<i64, Chat> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT ROWID, chat_identifier, display_name, service_name FROM chat")?;
        for row in stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .flatten()
        {
            let (id, ident, name, service) = row;
            chats.insert(
                id,
                Chat {
                    identifier: ident.unwrap_or_else(|| id.to_string()),
                    display_name: name.filter(|s| !s.trim().is_empty()),
                    service,
                    participants: Vec::new(),
                },
            );
        }
        let mut pstmt = src.prepare(
            "SELECT chj.chat_id, h.id FROM chat_handle_join chj JOIN handle h ON h.ROWID = chj.handle_id",
        )?;
        for row in pstmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
        {
            let (cid, h) = row;
            if let Some(c) = chats.get_mut(&cid) {
                if !h.trim().is_empty() && !c.participants.contains(&h) {
                    c.participants.push(h);
                }
            }
        }
    }

    let conn = cache.conn();
    // One transaction — messages run to tens of thousands of rows.
    let tx = conn.unchecked_transaction()?;

    // Replace mode: clear this parser's prior rows before re-inserting, in the
    // same transaction. Attachments first (they FK-reference messages), then
    // messages, then threads.
    if replace {
        tx.execute(
            &format!(
                "DELETE FROM attachments WHERE message_id IN
                   (SELECT id FROM messages WHERE thread_id IN
                     (SELECT id FROM threads WHERE {NATIVE_THREADS}))"
            ),
            [],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM messages WHERE thread_id IN
                   (SELECT id FROM threads WHERE {NATIVE_THREADS})"
            ),
            [],
        )?;
        tx.execute(&format!("DELETE FROM threads WHERE {NATIVE_THREADS}"), [])?;
        // Also clear the gallery mirror of message media (see below).
        tx.execute("DELETE FROM media_items WHERE source = 'Messages'", [])?;
    }

    // Messages in chat order (grouped), then time. Skip pure "action" items
    // (group renames, joins) which carry no text and no attachment.
    //
    // `cache_has_attachments` is a denormalized flag that is occasionally stale
    // (0 despite real `message_attachment_join` rows), which would drop an
    // attachment-only message. Also admit any row that actually has a join row,
    // guarding the EXISTS on the table's presence (old/partial DBs may omit it).
    let att_exists = if table_exists(&src, "message_attachment_join") {
        "OR EXISTS (SELECT 1 FROM message_attachment_join maj WHERE maj.message_id = m.ROWID)"
    } else {
        ""
    };
    // Group-action columns (item_type/group_action_type/group_title) are absent
    // from older schemas + the test fixtures; fall back to 0/NULL so the SELECT
    // and the `<> 0` filter are inert there.
    let mcols = message_table_columns(&src, "message");
    let it_expr = if mcols.contains("item_type") { "m.item_type" } else { "0" };
    let ga_expr = if mcols.contains("group_action_type") { "m.group_action_type" } else { "0" };
    let gt_expr = if mcols.contains("group_title") { "m.group_title" } else { "NULL" };
    // App-bubble bundle id (Digital Touch / Handwriting / iMessage extensions);
    // absent from older schemas + the test fixtures, so fall back to NULL.
    let bb_expr = if mcols.contains("balloon_bundle_id") { "m.balloon_bundle_id" } else { "NULL" };
    let mut mstmt = src.prepare(&format!(
        "SELECT cmj.chat_id, m.text, m.is_from_me, m.date, m.handle_id, m.cache_has_attachments, m.ROWID,
                m.date_read, m.date_delivered, m.guid,
                m.associated_message_guid, m.associated_message_type, m.associated_message_emoji,
                m.thread_originator_guid, m.attributedBody, m.date_edited,
                {it_expr}, {ga_expr}, {gt_expr}, {bb_expr}
         FROM message m
         JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
         WHERE m.text IS NOT NULL OR m.cache_has_attachments <> 0
               OR COALESCE(m.associated_message_type, 0) <> 0
               OR m.attributedBody IS NOT NULL
               OR ({it_expr}) <> 0
               OR ({bb_expr}) IS NOT NULL
               {att_exists}
         ORDER BY cmj.chat_id, m.date, m.ROWID",
    ))?;
    let mut rows = mstmt.query([])?;

    let mut current_chat: Option<i64> = None;
    let mut thread_id: i64 = 0;
    let mut is_group = false;
    // Tapbacks are separate message rows that point at their target by GUID. Map
    // each real message's GUID → its cache id as we go, and collect reaction events
    // to fold in after the pass (a reaction may be ordered before or after its
    // target). Event = (target_guid, reactor_key, assoc_type, custom_emoji).
    let mut guid_to_id: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut reaction_events: Vec<(String, String, i64, Option<String>)> = Vec::new();
    // Inline replies: (this message's cache id, the replied-to message's GUID),
    // resolved to a snippet after the pass (the target may come later).
    let mut reply_links: Vec<(i64, String)> = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_id: i64 = r.get(0)?;
        let text: Option<String> = r.get(1)?;
        let is_from_me = r.get::<_, i64>(2)? != 0;
        // `date` is `INTEGER DEFAULT 0` (not NOT NULL); read it optionally so one
        // NULL-dated row can't abort the whole import. 0 → mac_to_unix yields None.
        let date = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
        let handle_id: i64 = r.get::<_, Option<i64>>(4)?.unwrap_or(0);
        let msg_rowid: i64 = r.get(6)?;
        // Trust the actual join rows over the sometimes-stale cache flag, so an
        // attachment-only message isn't mis-flagged (or skipped by the guard below).
        let has_attachment =
            r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0 || att_msg_ids.contains(&msg_rowid);
        let read_at = mac_to_unix(r.get::<_, Option<i64>>(7)?.unwrap_or(0));
        let delivered_at = mac_to_unix(r.get::<_, Option<i64>>(8)?.unwrap_or(0));
        let guid: Option<String> = r.get(9)?;
        let assoc_guid: Option<String> = r.get(10)?;
        let assoc_type = r.get::<_, Option<i64>>(11)?.unwrap_or(0);
        let assoc_emoji: Option<String> = r.get(12)?;
        let reply_guid: Option<String> = r.get(13)?;
        // Recover the body from attributedBody when `text` is NULL (modern iMessage
        // stores content only in the archived NSAttributedString).
        let text = match text {
            Some(t) => Some(t),
            None => r
                .get::<_, Option<Vec<u8>>>(14)?
                .as_deref()
                .and_then(attributed_body_text),
        };
        let edited = r.get::<_, Option<i64>>(15)?.unwrap_or(0) != 0;
        // Group-action rows (renames/adds/removes/leaves) carry no text or
        // attachment; synthesize a system-message body for them.
        let item_type = r.get::<_, Option<i64>>(16)?.unwrap_or(0);
        let action_type = r.get::<_, Option<i64>>(17)?.unwrap_or(0);
        let group_title: Option<String> = r.get(18)?;
        let system_body = if item_type != 0 {
            group_action_body(item_type, action_type, group_title.as_deref())
        } else {
            None
        };
        // App-bubble messages (Digital Touch, iMessage extensions, …) carry no
        // text of their own; give them a typed placeholder body so they surface
        // instead of rendering blank or being dropped. Only when there's nothing
        // else to show (a URL bubble keeps its URL text; a sticker keeps media).
        let balloon_bundle_id: Option<String> = r.get(19)?;
        let app_body = balloon_bundle_id
            .as_deref()
            .filter(|_| text.is_none() && !has_attachment)
            .map(balloon_label);
        // A text-less URL bubble is still a link (its "Link" placeholder belongs
        // in the Links filter, not App messages).
        let is_url_balloon =
            balloon_bundle_id.as_deref() == Some("com.apple.messages.URLBalloonProvider");

        // A tapback row: record the event and skip it — it is not a chat message.
        if assoc_type >= 2000 {
            if let Some(target) = assoc_guid.as_deref() {
                let reactor = if is_from_me {
                    "me".to_string()
                } else {
                    handle_id.to_string()
                };
                reaction_events.push((
                    associated_target_guid(target).to_string(),
                    reactor,
                    assoc_type,
                    assoc_emoji,
                ));
            }
            continue;
        }

        // A row with nothing to show — no text, attachment, group-action, or
        // app-bubble placeholder — is not a chat message. Guards the broadened
        // `attributedBody`/`balloon_bundle_id` selection above.
        if text.is_none() && !has_attachment && system_body.is_none() && app_body.is_none() {
            continue;
        }

        if current_chat != Some(chat_id) {
            let chat = chats.get(&chat_id);
            let participants = chat.map(|c| c.participants.clone()).unwrap_or_default();
            is_group = participants.len() > 1;
            // Group → its name; 1:1 → the peer's identifier (the UI resolves it to
            // a saved contact). Thread identifier is the chat ROWID, matching the
            // iLEAPP path.
            let display_name = if is_group {
                chat.and_then(|c| c.display_name.clone())
            } else {
                chat.map(|c| c.identifier.clone())
            };
            tx.execute(
                "INSERT INTO threads (identifier, display_name, service, last_message_at, message_count, participants_json)
                 VALUES (?1, ?2, ?3, NULL, 0, ?4)",
                rusqlite::params![
                    chat_id.to_string(),
                    display_name,
                    chat.and_then(|c| c.service.clone()),
                    serde_json::to_string(&participants).unwrap_or_else(|_| "[]".into()),
                ],
            )?;
            thread_id = tx.last_insert_rowid();
            current_chat = Some(chat_id);
            report.threads += 1;
        }

        // Sender: the real member for an incoming group message; the peer for a
        // 1:1; None (the device owner) for outgoing.
        let sender = if is_from_me {
            None
        } else {
            handles.get(&handle_id).cloned().or_else(|| {
                if is_group {
                    None
                } else {
                    chats.get(&chat_id).map(|c| c.identifier.clone())
                }
            })
        };
        let sent_unix = mac_to_unix(date);
        // A group-action row: store it as a `system` message and move on (it's not
        // a chat bubble, has no attachments/tapbacks/replies).
        if let Some(sysbody) = system_body {
            tx.execute(
                "INSERT INTO messages
                    (thread_id, sender, is_from_me, body, sent_at, has_attachments, kind,
                     read_at, delivered_at, edited)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, 'system', ?6, ?7, 0)",
                rusqlite::params![
                    thread_id,
                    sender,
                    is_from_me as i64,
                    sysbody,
                    sent_unix,
                    read_at,
                    delivered_at,
                ],
            )?;
            report.messages += 1;
            continue;
        }
        // Content class for the filter: does any attachment look like image/video?
        let has_media = msg_atts.get(&msg_rowid).is_some_and(|ids| {
            ids.iter().any(|aid| {
                att_meta
                    .get(aid)
                    .is_some_and(|a| media_kind(a.mime.as_deref(), a.filename.as_deref()).is_some())
            })
        });
        // An app-bubble placeholder takes precedence (it's only set when there's
        // no text/attachment) and gets the "app" content kind for the filter.
        let (body, kind) = match app_body {
            Some(b) => (Some(b), if is_url_balloon { "link" } else { "app" }),
            None => {
                let k = crate::normalize::message_kind(text.as_deref(), has_media);
                (text, k)
            }
        };
        tx.execute(
            "INSERT INTO messages
                (thread_id, sender, is_from_me, body, sent_at, has_attachments, kind,
                 read_at, delivered_at, edited)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                thread_id,
                sender,
                is_from_me as i64,
                body,
                sent_unix,
                has_attachment as i64,
                kind,
                read_at,
                delivered_at,
                edited as i64,
            ],
        )?;
        let message_id = tx.last_insert_rowid();
        report.messages += 1;
        // Remember this message's GUID so tapbacks can be mapped onto it below.
        if let Some(g) = guid {
            guid_to_id.insert(g, message_id);
        }
        // If this message is an inline reply, note its target for later resolution.
        if let Some(target) = reply_guid.as_deref() {
            reply_links.push((message_id, associated_target_guid(target).to_string()));
        }

        // Attachment rows: resolve each to its backup file so the UI can serve it.
        if let (Some(src), Some(ids)) = (attachments, msg_atts.get(&msg_rowid)) {
            for aid in ids {
                let Some(a) = att_meta.get(aid) else { continue };
                let resolved = a.path.as_deref().and_then(|p| resolve_attachment(src, p));
                let (local_path, decrypt_key, plain_size) = match resolved {
                    Some((path, key, size)) => {
                        (Some(path.to_string_lossy().into_owned()), key, size)
                    }
                    None => (None, None, None),
                };
                // Display name: transfer_name, else the file's basename.
                let filename = a.filename.clone().or_else(|| {
                    a.path
                        .as_deref()
                        .and_then(|p| p.rsplit('/').next())
                        .map(str::to_string)
                });
                tx.execute(
                    "INSERT INTO attachments
                        (message_id, filename, mime_type, local_path, decrypt_key, plain_size)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        message_id,
                        filename,
                        a.mime,
                        local_path,
                        decrypt_key,
                        plain_size,
                    ],
                )?;
                // Mirror image/video attachments into `media_items` (source
                // 'Messages') so they also appear in the Photos gallery — restoring
                // the pre-iLEAPP behavior (message photos were a gallery source).
                // Only materialized media (has a local_path); docs/vCards stay out.
                if let (Some(kind), Some(lp)) = (
                    media_kind(a.mime.as_deref(), filename.as_deref()),
                    &local_path,
                ) {
                    tx.execute(
                        "INSERT INTO media_items
                            (domain, relative_path, kind, source, mime_type, taken_at,
                             thumb_path, local_path, decrypt_key, plain_size)
                         VALUES ('MediaDomain', ?1, ?2, 'Messages', ?3, ?4, NULL, ?5, ?6, ?7)",
                        rusqlite::params![
                            a.path.clone().unwrap_or_else(|| lp.clone()),
                            kind,
                            a.mime,
                            sent_unix,
                            lp,
                            decrypt_key,
                            plain_size,
                        ],
                    )?;
                }
            }
        }
    }
    drop(rows);
    drop(mstmt);

    // Fold tapback add/remove events into a per-message summary. Each reactor holds
    // at most one reaction per target at a time; a removal clears theirs.
    if !reaction_events.is_empty() {
        use std::collections::{BTreeMap, HashMap};
        // target_guid → reactor → emoji.
        let mut state: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (target, reactor, kind, emoji) in reaction_events {
            let per = state.entry(target).or_default();
            match reaction_emoji(kind, emoji.as_deref()) {
                Some(e) => {
                    per.insert(reactor, e);
                }
                None => {
                    per.remove(&reactor);
                }
            }
        }
        for (target, per) in state {
            let Some(&mid) = guid_to_id.get(&target) else {
                continue;
            };
            if per.is_empty() {
                continue;
            }
            // Count identical emojis → "❤️×2 👍" (sorted for determinism).
            let mut counts: BTreeMap<String, i64> = BTreeMap::new();
            for e in per.values() {
                *counts.entry(e.clone()).or_insert(0) += 1;
            }
            let summary = counts
                .into_iter()
                .map(|(e, c)| if c > 1 { format!("{e}×{c}") } else { e })
                .collect::<Vec<_>>()
                .join(" ");
            tx.execute(
                "UPDATE messages SET reactions = ?1 WHERE id = ?2",
                rusqlite::params![summary, mid],
            )?;
        }
    }

    // Resolve inline replies to a short preview of the message they reply to.
    for (reply_id, target_guid) in reply_links {
        let Some(&target_id) = guid_to_id.get(&target_guid) else {
            continue;
        };
        let snippet: Option<String> = tx
            .query_row(
                "SELECT body FROM messages WHERE id = ?1",
                [target_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        if let Some(snippet) = snippet {
            let snippet = truncate_snippet(&snippet, 80);
            tx.execute(
                "UPDATE messages SET reply_to_snippet = ?1 WHERE id = ?2",
                rusqlite::params![snippet, reply_id],
            )?;
        }
    }

    // Denormalize the per-thread counters (only the threads we just wrote).
    tx.execute_batch(&format!(
        "UPDATE threads SET
           message_count = (SELECT COUNT(*) FROM messages WHERE thread_id = threads.id),
           last_message_at = (SELECT MAX(sent_at) FROM messages WHERE thread_id = threads.id)
         WHERE {NATIVE_THREADS}"
    ))?;
    tx.commit()?;
    Ok(())
}

/// One row of the `attachment` table we care about.
struct Att {
    /// On-device path (`attachment.filename`), e.g. `~/Library/SMS/Attachments/…`.
    path: Option<String>,
    /// Display name (`attachment.transfer_name`).
    filename: Option<String>,
    mime: Option<String>,
}

/// Read `attachment` (by ROWID) and `message_attachment_join` (message ROWID →
/// attachment ROWIDs) so the main loop can attach files to each message.
#[allow(clippy::type_complexity)]
fn load_attachments(src: &Connection) -> Result<(HashMap<i64, Att>, HashMap<i64, Vec<i64>>)> {
    // A minimal/older sms.db may lack these tables — then there are simply no
    // attachments (not an error).
    if !table_exists(src, "attachment") || !table_exists(src, "message_attachment_join") {
        return Ok((HashMap::new(), HashMap::new()));
    }
    let mut meta: HashMap<i64, Att> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT ROWID, filename, transfer_name, mime_type FROM attachment")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Att {
                    path: r.get::<_, Option<String>>(1)?,
                    filename: r
                        .get::<_, Option<String>>(2)?
                        .filter(|s| !s.trim().is_empty()),
                    mime: r.get::<_, Option<String>>(3)?,
                },
            ))
        })?;
        for row in rows.flatten() {
            meta.insert(row.0, row.1);
        }
    }
    let mut joins: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT message_id, attachment_id FROM message_attachment_join")?;
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
            .flatten()
        {
            joins.entry(row.0).or_default().push(row.1);
        }
    }
    Ok((meta, joins))
}

/// Resolve an on-device attachment path to its backup blob (+ wrapped key/size
/// for an encrypted backup). None if the file isn't in the backup.
fn resolve_attachment(
    src: &AttachmentSource,
    on_device_path: &str,
) -> Option<(PathBuf, Option<Vec<u8>>, Option<u64>)> {
    let rel = normalize_attachment_path(on_device_path)?;
    let entry = src.index.find("MediaDomain", &rel).ok().flatten()?;
    let path = src.index.blob_path(&entry.file_id);
    let (key, size) = if src.decryptor.is_some() {
        match crypto::file_key_field(&entry.file_blob) {
            Ok((k, s)) => (Some(k), s),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };
    Some((path, key, size))
}

/// Map an on-device SMS attachment path to its `MediaDomain` relativePath. iOS
/// stores these as `~/Library/SMS/Attachments/…` (or an absolute variant); the
/// backup keys them by `Library/SMS/Attachments/…`.
fn normalize_attachment_path(p: &str) -> Option<String> {
    p.find("Library/SMS/Attachments/")
        .map(|i| p[i..].to_string())
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sms_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("sms.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT, service_name TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER, date_read INTEGER, date_delivered INTEGER, guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER, associated_message_emoji TEXT, thread_originator_guid TEXT, attributedBody BLOB, date_edited INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111'), (2,'+15550002222');
             -- A 1:1 chat with one peer, and a group chat with a name + two peers.
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat VALUES (20,'chat99','Hiking Crew','iMessage');
             INSERT INTO chat_handle_join VALUES (10,1),(20,1),(20,2);
             -- date is Apple-absolute nanoseconds; unix 1_700_000_000 = 721692800000000000.
             INSERT INTO message VALUES (100,'hey there',0,721692800000000000,1,0,0,0,'GUID-100',NULL,0,NULL,NULL,NULL,0);
             -- outgoing, delivered + read (date_delivered / date_read set).
             INSERT INTO message VALUES (101,'hi back',1,721692860000000000,0,0,721692900000000000,721692880000000000,'GUID-101',NULL,0,NULL,NULL,NULL,0);
             -- an inline reply to message 100 ('hey there').
             INSERT INTO message VALUES (102,'reply body',1,721692920000000000,0,0,0,0,'GUID-102',NULL,0,NULL,'p:0/GUID-100',NULL,0);
             INSERT INTO message VALUES (200,'who is in?',0,721700000000000000,2,0,0,0,'GUID-200',NULL,0,NULL,NULL,NULL,0);
             -- an attachment-only message (NULL text) is kept.
             INSERT INTO message VALUES (201,NULL,1,721700060000000000,0,1,0,0,'GUID-201',NULL,0,NULL,NULL,NULL,0);
             -- a pure action item (NULL text, no attachment) is skipped.
             INSERT INTO message VALUES (202,NULL,0,721700120000000000,1,0,0,0,'GUID-202',NULL,0,NULL,NULL,NULL,0);
             -- a tapback (Loved) on message 100 from the device owner; not a message.
             INSERT INTO message VALUES (300,NULL,1,721692900000000000,0,0,0,0,'GUID-300','p:0/GUID-100',2000,NULL,NULL,NULL,0);
             INSERT INTO chat_message_join VALUES (10,100),(10,101),(10,102),(20,200),(20,201),(20,202),(10,300);",
        )
        .unwrap();
        db
    }

    #[test]
    fn attachment_only_message_kept_despite_stale_cache_flag() {
        // A message with NULL text and cache_has_attachments = 0 (stale) but a real
        // message_attachment_join row must still be kept and flagged — the flag is
        // denormalized and occasionally lags the join table.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sms.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT, service_name TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER, date_read INTEGER, date_delivered INTEGER, guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER, associated_message_emoji TEXT, thread_originator_guid TEXT, attributedBody BLOB, date_edited INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111');
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat_handle_join VALUES (10,1);
             -- NULL text, STALE cache_has_attachments = 0, but a real join row.
             INSERT INTO message VALUES (500,NULL,1,721700060000000000,0,0,0,0,'GUID-500',NULL,0,NULL,NULL,NULL,0);
             INSERT INTO chat_message_join VALUES (10,500);
             INSERT INTO message_attachment_join VALUES (500,1);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        // Even without an attachment source (msg_atts empty), the message survives.
        parse_messages(&db, &cache, &mut report, false, None).unwrap();
        assert_eq!(report.messages, 1, "stale-flag attachment message kept");
        let (body, has_att): (Option<String>, i64) = cache
            .conn()
            .query_row("SELECT body, has_attachments FROM messages", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(body, None);
        assert_eq!(has_att, 1, "flagged as having an attachment");
    }

    #[test]
    fn app_bubble_messages_get_typed_placeholders() {
        // A message table WITH balloon_bundle_id. Three app-bubble rows that carry
        // no text/attachment: Digital Touch, a GamePigeon extension, and a URL
        // balloon that DOES have text (its URL) — the URL keeps its text, the
        // others get a placeholder body + the "app" kind.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sms.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT, service_name TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER, date_read INTEGER, date_delivered INTEGER, guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER, associated_message_emoji TEXT, thread_originator_guid TEXT, attributedBody BLOB, date_edited INTEGER, balloon_bundle_id TEXT);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111');
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat_handle_join VALUES (10,1);
             INSERT INTO message VALUES (1,NULL,0,721700010000000000,1,0,0,0,'G1',NULL,0,NULL,NULL,NULL,0,'com.apple.DigitalTouchBalloonProvider');
             INSERT INTO message VALUES (2,NULL,1,721700020000000000,1,0,0,0,'G2',NULL,0,NULL,NULL,NULL,0,'com.apple.messages.MSMessageExtensionBalloonPlugin:EWFNLB79LQ:com.gamerdelights.gamepigeon.ext');
             INSERT INTO message VALUES (3,'https://example.com',0,721700030000000000,1,0,0,0,'G3',NULL,0,NULL,NULL,NULL,0,'com.apple.messages.URLBalloonProvider');
             -- A URL balloon with NO recoverable text still buckets as a link.
             INSERT INTO message VALUES (4,NULL,0,721700040000000000,1,0,0,0,'G4',NULL,0,NULL,NULL,NULL,0,'com.apple.messages.URLBalloonProvider');
             INSERT INTO chat_message_join VALUES (10,1),(10,2),(10,3),(10,4);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_messages(&db, &cache, &mut report, false, None).unwrap();
        assert_eq!(report.messages, 4, "all four app bubbles surface");
        let rows: Vec<(String, String)> = cache
            .conn()
            .prepare("SELECT body, kind FROM messages ORDER BY sent_at")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows[0], ("Digital Touch".to_string(), "app".to_string()));
        assert_eq!(rows[1], ("GamePigeon".to_string(), "app".to_string()));
        // The URL balloon keeps its own text and is classified as a link.
        assert_eq!(rows[2], ("https://example.com".to_string(), "link".to_string()));
        // A text-less URL balloon: "Link" placeholder, still the link kind (not app).
        assert_eq!(rows[3], ("Link".to_string(), "link".to_string()));
    }

    #[test]
    fn attributed_body_recovers_text_from_typedstream() {
        // A minimal streamtyped blob: the NSString marker, a '+' inline-string
        // marker, a one-byte length, then the UTF-8 bytes.
        let mut blob = b"streamtyped...NSAttributedString...NSString".to_vec();
        blob.push(0x2b); // '+'
        blob.push(5); // length
        blob.extend_from_slice(b"hello");
        assert_eq!(attributed_body_text(&blob).as_deref(), Some("hello"));
        // No NSString marker → None (not a body we can read).
        assert_eq!(attributed_body_text(b"nothing here"), None);
    }

    #[test]
    fn parses_threads_and_messages_from_sms_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_sms_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        parse_messages(&db, &cache, &mut report, false, None).unwrap();

        assert_eq!(report.threads, 2);
        assert_eq!(report.messages, 5); // action item (202) skipped; reply (102) kept

        let c = cache.conn();
        // Group thread named, 1:1 thread shows the peer identifier.
        let (group_name, group_count): (Option<String>, i64) = c
            .query_row(
                "SELECT display_name, message_count FROM threads WHERE identifier = '20'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(group_name.as_deref(), Some("Hiking Crew"));
        assert_eq!(group_count, 2);
        let direct_name: Option<String> = c
            .query_row(
                "SELECT display_name FROM threads WHERE identifier = '10'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(direct_name.as_deref(), Some("+15550001111"));

        // Apple-time → Unix seconds, and sender attribution for the group.
        let (sent_at, sender): (i64, Option<String>) = c
            .query_row(
                "SELECT m.sent_at, m.sender FROM messages m JOIN threads t ON t.id = m.thread_id
                 WHERE t.identifier = '20' AND m.body = 'who is in?'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sent_at, 1_700_007_200); // 721700000000000000 ns → Unix
        assert_eq!(sender.as_deref(), Some("+15550002222"));

        // Read/delivered receipts on the outgoing "hi back" message (ns → Unix).
        let (read_at, delivered_at): (Option<i64>, Option<i64>) = c
            .query_row(
                "SELECT read_at, delivered_at FROM messages WHERE body = 'hi back'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(delivered_at, Some(1_700_000_080));
        assert_eq!(read_at, Some(1_700_000_100));

        // The tapback folds onto its target ('hey there'), and the tapback row
        // itself is not stored as a message.
        let reactions: Option<String> = c
            .query_row(
                "SELECT reactions FROM messages WHERE body = 'hey there'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reactions.as_deref(), Some("❤️"));
        let msg_count: i64 = c
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msg_count, 5, "tapback row is not a message; reply is kept");

        // The reply carries a preview of the message it replies to.
        let reply_snippet: Option<String> = c
            .query_row(
                "SELECT reply_to_snippet FROM messages WHERE body = 'reply body'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reply_snippet.as_deref(), Some("hey there"));

        // Attachment-only message kept, flagged, outgoing.
        let (has_att, from_me): (i64, i64) = c
            .query_row(
                "SELECT has_attachments, is_from_me FROM messages WHERE body IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(has_att, 1);
        assert_eq!(from_me, 1);
    }

    #[test]
    fn replace_clears_prior_rows_including_attachment_children() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_sms_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();

        // Simulate a prior iLEAPP import: an iMessage thread + message with an
        // attachment child, plus an app-chat (TikTok) thread that must survive.
        cache
            .conn()
            .execute_batch(
                "INSERT INTO threads (id, identifier, service) VALUES (900, 'old', 'iMessage');
                 INSERT INTO threads (id, identifier, service) VALUES (901, 'tt', 'TikTok');
                 INSERT INTO messages (id, thread_id, is_from_me, body, has_attachments)
                   VALUES (9000, 900, 0, 'old msg', 1);
                 INSERT INTO messages (id, thread_id, is_from_me, body) VALUES (9001, 901, 0, 'tiktok');
                 INSERT INTO attachments (message_id, filename) VALUES (9000, 'old.jpg');",
            )
            .unwrap();

        // replace=true must delete the attachment child before the message
        // (FK ON, no cascade) — a bare message delete would fail here.
        let mut report = ImportReport::default();
        parse_messages(&db, &cache, &mut report, true, None).unwrap();

        let c = cache.conn();
        // Old iMessage thread + message + attachment gone; fresh ones inserted.
        let old: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE identifier = 'old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 0);
        let orphan_att: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM attachments WHERE message_id = 9000",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_att, 0);
        assert_eq!(report.threads, 2); // the two chats from the fresh sms.db
                                       // The app-chat thread is untouched by a native-messages replace.
        let tiktok: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE service = 'TikTok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tiktok, 1);
    }
}
