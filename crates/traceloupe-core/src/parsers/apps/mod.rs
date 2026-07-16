//! Native third-party app chat modules (Phase 2).
//!
//! Each app stores its chats in its own app-group SQLite DB with an app-specific
//! schema. Rather than one bespoke import path per app, every app is a small
//! **module** ([`AppChatModule`]) that only has to: (1) locate its message DB in
//! the Manifest, and (2) parse that DB into a flat [`AppMessage`] stream. The
//! shared [`insert_app_conversation`] then turns that stream into the same
//! `threads` + `messages` cache rows the Messages view already renders — so
//! adding an app is additive and never touches the pipeline (mirrors iLEAPP's
//! plugin model; see product-architecture §13.1).
//!
//! provenance: reference (own implementation, architecture §10). The DB paths,
//! table/column names, and timestamp encodings are *facts* learned from iLEAPP's
//! modules (`whatsApp.py`, `tikTok.py`, `telegramMesssages.py`); the Rust is
//! written from those facts, not ported.

pub mod facebook_messenger;
pub mod imo;
pub mod instagram;
pub mod kik;
pub mod linkedin;
pub mod teams;
pub mod telegram;
pub mod threema;
pub mod tiktok;
pub mod viber;
pub mod whatsapp;

use std::path::Path;

use crate::cache::CacheDb;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::normalize::ImportReport;
use crate::Result;

/// One parsed message, normalized across apps. The shared inserter groups these
/// into threads by `chat_key`, so a module just emits messages in any order.
#[derive(Debug, Clone, Default)]
pub struct AppMessage {
    /// Stable per-conversation key (chat/session id). Groups messages into threads.
    pub chat_key: String,
    /// The conversation's display name, when the app stores one (WhatsApp/Telegram
    /// do; TikTok doesn't → `None`, and the name is derived from the peer).
    pub chat_name: Option<String>,
    /// Unix epoch seconds; `None` if unknown.
    pub timestamp: Option<i64>,
    pub body: Option<String>,
    pub is_from_me: bool,
    /// Sender's display name (for incoming messages).
    pub sender_name: Option<String>,
    /// Sender's `@handle`, when known (used as the 1:1 participant).
    pub sender_handle: Option<String>,
    /// Stable sender id, to count distinct participants (group detection).
    pub sender_id: Option<String>,
    /// Whether this message carries an attachment (media).
    pub has_attachment: bool,
    /// Explicit content class for the message filter ('shared', 'sticker',
    /// 'system', …) when the app knows it (TikTok). `None` → derived from
    /// body/attachment by the inserter.
    pub kind: Option<&'static str>,
    /// Media attachments carried by this message. The inserter resolves each to a
    /// backup file (via the caller's resolver) and records it in `attachments`
    /// (and, for image/video, mirrors it into the gallery). Empty for text-only.
    pub attachments: Vec<AppAttachment>,
}

/// A media file referenced by an app message. `path` is the media path as the app
/// stores it — the inserter's resolver maps it (by basename) to a backup blob.
#[derive(Debug, Clone)]
pub struct AppAttachment {
    pub path: String,
    pub mime: Option<String>,
    pub filename: Option<String>,
}

/// A resolved backup location for an app attachment: `(local blob path, wrapped
/// decrypt key, plaintext size)`.
pub type ResolvedMedia = (String, Option<Vec<u8>>, Option<u64>);

/// Resolves an [`AppAttachment`] to its backup file, or `None` when the media
/// isn't in the backup. Built by the import driver, which holds the
/// `ManifestIndex` + decryptor.
pub type AppMediaResolver<'a> = dyn Fn(&AppAttachment) -> Option<ResolvedMedia> + 'a;

/// A native chat parser for one third-party app.
pub struct AppChatModule {
    /// Import-toggle id (matches the module catalog, e.g. "whatsapp").
    pub id: &'static str,
    /// Service label shown in the Messages view (e.g. "WhatsApp"). Also the tag
    /// used to skip the equivalent iLEAPP stage.
    pub service: &'static str,
    /// Whether an all-numeric `chat_key` denotes a GROUP for this app. True only
    /// for TikTok (its 1:1 ids embed both user ids with `:`, so a bare number is a
    /// group). For apps whose 1:1 threads also use bare-numeric ids (Messenger,
    /// Instagram) this MUST be false, or every 1:1 is mislabeled a group.
    pub numeric_id_groups: bool,
    /// Locate this app's message DB(s) in the Manifest. Most apps have one; some
    /// (e.g. Messenger's per-user `lightspeed-userDatabases/*.db`) have several,
    /// so this returns every candidate and the driver parses each.
    pub locate: fn(&ManifestIndex) -> Result<Vec<FileEntry>>,
    /// Parse one extracted (decrypted) DB into a message stream. The second arg is
    /// the source file's Manifest `relativePath` — needed by apps that encode
    /// context in the path (e.g. TikTok's per-account directory name = the local
    /// user id). A DB that turns out not to hold this app's messages returns an
    /// empty vec (not an error), so non-matching candidates are skipped quietly.
    pub parse: fn(&Path, &str) -> Result<Vec<AppMessage>>,
}

/// The registered native app chat modules. Add an entry to support a new app.
pub const APP_CHAT_MODULES: &[AppChatModule] = &[
    whatsapp::MODULE,
    facebook_messenger::MODULE,
    instagram::MODULE,
    // NOTE: TikTok is NOT here — its messages (`ChatFiles/*/db.sqlite`) and names
    // (`AwemeIM.db`) live in two separate DBs, which the single-file module API
    // can't join, so it's driven by `import_tiktok_messages_native` instead.
    telegram::MODULE,
    kik::MODULE,
    imo::MODULE,
    threema::MODULE,
    viber::MODULE,
    teams::MODULE,
    linkedin::MODULE,
];

/// Read a column as a String whether it's stored TEXT or INTEGER — app schemas
/// have inconsistent column affinity across versions, and a strict typed read
/// would abort the whole DB on one mistyped row. NULL/other types → None.
pub(crate) fn col_string(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<String>> {
    Ok(match r.get_ref(i)? {
        rusqlite::types::ValueRef::Integer(n) => Some(n.to_string()),
        rusqlite::types::ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        _ => None,
    })
}

/// Read a column as i64 tolerantly (INTEGER, or a TEXT/REAL that converts) so one
/// oddly-typed row can't abort the whole DB. NULL/unparseable → None. Preferred
/// over `get::<Option<f64>>` for large integers (e.g. nanosecond timestamps),
/// which lose precision beyond 2^53 when routed through f64.
pub(crate) fn col_i64(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<i64>> {
    Ok(match r.get_ref(i)? {
        rusqlite::types::ValueRef::Integer(n) => Some(n),
        rusqlite::types::ValueRef::Real(f) => Some(f as i64),
        rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).trim().parse::<i64>().ok(),
        _ => None,
    })
}

/// Insert a parsed app conversation stream into the cache as `threads` + messages,
/// tagged with `service`. Messages are grouped by `chat_key`; a thread's name is
/// the app-provided `chat_name` when present, else derived from the peer (a group
/// when several distinct senders appear). Mirrors the iLEAPP app-chat normalizer's
/// output so the Messages view renders native and iLEAPP-sourced chats identically.
pub fn insert_app_conversation(
    cache: &CacheDb,
    service: &str,
    numeric_id_groups: bool,
    messages: Vec<AppMessage>,
    report: &mut ImportReport,
) -> Result<()> {
    // No media resolver → attachment file bytes aren't linked (still records the
    // attachment metadata). The import driver uses the `_with_media` variant.
    insert_app_conversation_with_media(cache, service, numeric_id_groups, messages, report, &|_| {
        None
    })
}

/// Like [`insert_app_conversation`] but resolves each message's [`AppAttachment`]s
/// to backup files via `resolve`, recording them in `attachments` (and mirroring
/// image/video into `media_items` so app-chat media also shows in the gallery).
pub fn insert_app_conversation_with_media(
    cache: &CacheDb,
    service: &str,
    numeric_id_groups: bool,
    mut messages: Vec<AppMessage>,
    report: &mut ImportReport,
    resolve: &AppMediaResolver,
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    // Stable grouping: by chat, then time (None sorts first).
    messages.sort_by(|a, b| {
        a.chat_key
            .cmp(&b.chat_key)
            .then(a.timestamp.cmp(&b.timestamp))
    });

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;

    let mut current_key: Option<String> = None;
    let mut thread_id: i64 = 0;
    let mut has_chat_name = false;
    // Count into locals; fold into `report` only after commit, so a rollback
    // doesn't leave phantom counts behind (which would double up if iLEAPP re-runs).
    let mut n_threads: usize = 0;
    let mut n_messages: usize = 0;
    let mut peer_nick: Option<String> = None;
    let mut peer_handle: Option<String> = None;
    let mut member_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Set a finished thread's name + participants. A group (several distinct
    // incoming senders, or a bare-numeric TikTok id in derive mode) is labeled;
    // a 1:1 keeps the peer @handle as its sole participant.
    let finalize = |tx: &rusqlite::Connection,
                    id: i64,
                    key: &str,
                    named: bool,
                    nick: &mut Option<String>,
                    handle: &mut Option<String>,
                    members: &mut std::collections::HashSet<String>|
     -> Result<()> {
        let member_count = members.len();
        members.clear();
        // Bare-numeric key ⇒ group ONLY for apps that encode 1:1s differently
        // (TikTok). For Messenger/Instagram, whose 1:1 threads also use numeric
        // ids, this must stay off or every 1:1 is mislabeled a group.
        let id_is_group = numeric_id_groups
            && !named
            && !key.is_empty()
            && key.bytes().all(|b| b.is_ascii_digit());
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

    for m in &messages {
        if current_key.as_deref() != Some(m.chat_key.as_str()) {
            if let Some(prev) = current_key.as_deref() {
                finalize(
                    &tx,
                    thread_id,
                    prev,
                    has_chat_name,
                    &mut peer_nick,
                    &mut peer_handle,
                    &mut member_ids,
                )?;
            }
            tx.execute(
                "INSERT INTO threads
                    (identifier, display_name, service, last_message_at, message_count, participants_json)
                 VALUES (?1, ?2, ?3, NULL, 0, '[]')",
                rusqlite::params![m.chat_key, m.chat_name, service],
            )?;
            thread_id = tx.last_insert_rowid();
            current_key = Some(m.chat_key.clone());
            has_chat_name = m.chat_name.is_some();
            peer_nick = None;
            peer_handle = None;
            member_ids.clear();
            n_threads += 1;
        }

        let sender = if m.is_from_me {
            None
        } else {
            m.sender_name.clone()
        };
        // Derive the peer name/handle only when the app gave no chat name.
        if !has_chat_name && !m.is_from_me {
            if let Some(sid) = &m.sender_id {
                member_ids.insert(sid.clone());
            }
            if peer_nick.is_none() {
                peer_nick = m.sender_name.clone();
                peer_handle = m.sender_handle.as_ref().map(|h| {
                    if h.starts_with('@') {
                        h.clone()
                    } else {
                        format!("@{h}")
                    }
                });
            }
        }
        let has_att = m.has_attachment || !m.attachments.is_empty();
        // App-provided content class (TikTok markers) or derived from body/media.
        let kind = m
            .kind
            .unwrap_or_else(|| crate::normalize::message_kind(m.body.as_deref(), has_att));
        tx.execute(
            "INSERT INTO messages
                 (thread_id, sender, is_from_me, body, sent_at, has_attachments, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                thread_id,
                sender,
                m.is_from_me as i64,
                m.body,
                m.timestamp,
                has_att as i64,
                kind,
            ],
        )?;
        let message_id = tx.last_insert_rowid();
        n_messages += 1;

        // Attachment rows: resolve each to a backup file so the UI can serve it.
        // An unresolved attachment (media not in the backup) still records its
        // metadata so the message shows it carried media.
        for att in &m.attachments {
            let (local_path, key, size) = match resolve(att) {
                Some((p, k, s)) => (Some(p), k, s),
                None => (None, None, None),
            };
            let filename = att.filename.clone().or_else(|| {
                att.path
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
            tx.execute(
                "INSERT INTO attachments
                     (message_id, filename, mime_type, local_path, decrypt_key, plain_size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![message_id, filename, att.mime, local_path, key, size],
            )?;
            // Mirror image/video into the gallery (source = the app), like iMessage.
            if let (Some(lp), Some(mime)) = (&local_path, att.mime.as_deref()) {
                let media_kind = if mime.starts_with("video/") {
                    Some("video")
                } else if mime.starts_with("image/") {
                    Some("photo")
                } else {
                    None
                };
                if let Some(mk) = media_kind {
                    tx.execute(
                        "INSERT INTO media_items
                             (domain, relative_path, kind, source, mime_type, taken_at,
                              thumb_path, local_path, decrypt_key, plain_size)
                         VALUES ('AppDomain', ?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8)",
                        rusqlite::params![att.path, mk, service, mime, m.timestamp, lp, key, size],
                    )?;
                }
            }
        }
    }
    if let Some(prev) = current_key.as_deref() {
        finalize(
            &tx,
            thread_id,
            prev,
            has_chat_name,
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
        rusqlite::params![service],
    )?;
    tx.commit()?;
    // Committed — now it's safe to count.
    report.threads += n_threads;
    report.messages += n_messages;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;

    #[test]
    fn media_attachments_resolve_and_mirror_to_gallery() {
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        // One incoming message carrying an image attachment.
        let msg = AppMessage {
            chat_key: "chat1".into(),
            timestamp: Some(1_700_000_000),
            body: Some("look at this".into()),
            sender_name: Some("Robin".into()),
            attachments: vec![AppAttachment {
                path: "Media/MediaFiles/abc-123.jpg".into(),
                mime: Some("image/jpeg".into()),
                filename: None,
            }],
            ..Default::default()
        };

        // A resolver that "finds" the media in the backup.
        let resolve = |_a: &AppAttachment| Some(("blob/xyz".to_string(), None, Some(4096)));
        insert_app_conversation_with_media(
            &cache,
            "WhatsApp",
            false,
            vec![msg],
            &mut report,
            &resolve,
        )
        .unwrap();

        let conn = cache.conn();
        // The message is flagged as having an attachment, and an attachments row
        // was inserted with the resolved backup path + basename filename.
        let (has_att, mid): (i64, i64) = conn
            .query_row(
                "SELECT has_attachments, id FROM messages WHERE body = 'look at this'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(has_att, 1);
        let (fname, local, mime): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT filename, local_path, mime_type FROM attachments WHERE message_id = ?1",
                [mid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(fname.as_deref(), Some("abc-123.jpg"));
        assert_eq!(local.as_deref(), Some("blob/xyz"));
        assert_eq!(mime.as_deref(), Some("image/jpeg"));

        // The image is mirrored into the gallery, tagged with the app as its source.
        let (src, kind, gpath): (String, String, Option<String>) = conn
            .query_row(
                "SELECT source, kind, local_path FROM media_items WHERE source = 'WhatsApp'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(src, "WhatsApp");
        assert_eq!(kind, "photo");
        assert_eq!(gpath.as_deref(), Some("blob/xyz"));
    }

    #[test]
    fn unresolved_attachment_still_records_metadata() {
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        let msg = AppMessage {
            chat_key: "c".into(),
            body: Some("gone".into()),
            attachments: vec![AppAttachment {
                path: "x/evicted.mov".into(),
                mime: Some("video/quicktime".into()),
                filename: None,
            }],
            ..Default::default()
        };
        // Resolver finds nothing (media not in the backup).
        insert_app_conversation_with_media(&cache, "Kik", false, vec![msg], &mut report, &|_| None)
            .unwrap();

        let conn = cache.conn();
        // Attachment metadata is recorded (filename/mime) with no servable path,
        // and nothing is mirrored to the gallery.
        let (fname, local): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT filename, local_path FROM attachments LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(fname.as_deref(), Some("evicted.mov"));
        assert_eq!(local, None);
        let media: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(media, 0);
    }
}
