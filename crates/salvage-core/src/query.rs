//! Read-side queries over the cache DB (architecture §6: "every browse is a
//! cache query"). Pure reads, returning serializable view models the shell
//! hands straight to the UI. No engine or decryption concerns here.

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::cache::CacheDb;
use crate::Result;

/// One row in the Messages thread list.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: i64,
    pub identifier: String,
    pub display_name: Option<String>,
    pub service: Option<String>,
    /// Unix epoch seconds of the most recent message.
    pub last_message_at: Option<i64>,
    pub message_count: i64,
    /// Body of the most recent message, for the list preview.
    pub snippet: Option<String>,
}

/// One message in a conversation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: i64,
    pub is_from_me: bool,
    pub sender: Option<String>,
    pub body: Option<String>,
    pub sent_at: Option<i64>,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    /// Absolute path to the extracted bytes, if materialized.
    pub local_path: Option<String>,
}

/// Threads ordered most-recent first, for the Messages list.
pub fn list_threads(cache: &CacheDb) -> Result<Vec<ThreadSummary>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT t.id, t.identifier, t.display_name, t.service,
                t.last_message_at, t.message_count,
                (SELECT m.body FROM messages m
                  WHERE m.thread_id = t.id
                  ORDER BY m.sent_at DESC, m.id DESC LIMIT 1) AS snippet
         FROM threads t
         ORDER BY t.last_message_at DESC NULLS LAST, t.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ThreadSummary {
            id: r.get(0)?,
            identifier: r.get(1)?,
            display_name: r.get(2)?,
            service: r.get(3)?,
            last_message_at: r.get(4)?,
            message_count: r.get(5)?,
            snippet: r.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// All messages in a thread, oldest first, each with its attachments.
pub fn get_messages(cache: &CacheDb, thread_id: i64) -> Result<Vec<Message>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, is_from_me, sender, body, sent_at
         FROM messages
         WHERE thread_id = ?1
         ORDER BY sent_at ASC, id ASC",
    )?;
    let mut messages = stmt
        .query_map([thread_id], |r| {
            Ok(Message {
                id: r.get(0)?,
                is_from_me: r.get::<_, i64>(1)? != 0,
                sender: r.get(2)?,
                body: r.get(3)?,
                sent_at: r.get(4)?,
                attachments: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Attach media per message. Small N per thread; a per-message query keeps
    // the mapping obvious. (If this ever shows up in a profile, switch to one
    // grouped query.)
    let mut att_stmt = conn
        .prepare("SELECT filename, mime_type, local_path FROM attachments WHERE message_id = ?1")?;
    for msg in &mut messages {
        msg.attachments = att_stmt
            .query_map([msg.id], |r| {
                Ok(Attachment {
                    filename: r.get(0)?,
                    mime_type: r.get(1)?,
                    local_path: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(messages)
}

/// One call-history entry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    pub id: i64,
    pub address: Option<String>,
    /// "incoming" | "outgoing".
    pub direction: Option<String>,
    pub answered: Option<bool>,
    pub duration_s: Option<i64>,
    pub occurred_at: Option<i64>,
    /// Call type/service, e.g. "Phone Call", "FaceTime Audio".
    pub service: Option<String>,
}

/// Calls, most recent first.
pub fn list_calls(cache: &CacheDb) -> Result<Vec<Call>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, address, direction, answered, duration_s, occurred_at, service
         FROM calls ORDER BY occurred_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Call {
            id: r.get(0)?,
            address: r.get(1)?,
            direction: r.get(2)?,
            answered: r.get::<_, Option<i64>>(3)?.map(|a| a != 0),
            duration_s: r.get(4)?,
            occurred_at: r.get(5)?,
            service: r.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One Safari history visit.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryVisit {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub visited_at: Option<i64>,
    pub visit_count: Option<i64>,
}

/// Safari history, most recent first.
pub fn list_safari_history(cache: &CacheDb) -> Result<Vec<HistoryVisit>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, url, title, visited_at, visit_count
         FROM safari_history ORDER BY visited_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(HistoryVisit {
            id: r.get(0)?,
            url: r.get(1)?,
            title: r.get(2)?,
            visited_at: r.get(3)?,
            visit_count: r.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A contact, with phones/emails decoded from the cache's JSON columns.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: i64,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub organization: Option<String>,
    pub phones: Vec<crate::parsers::address_book::LabeledValue>,
    pub emails: Vec<crate::parsers::address_book::LabeledValue>,
}

/// Contacts, ordered by name (people first, then organization-only entries).
pub fn list_contacts(cache: &CacheDb) -> Result<Vec<Contact>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, first_name, last_name, organization, phones_json, emails_json
         FROM contacts
         ORDER BY last_name IS NULL AND first_name IS NULL,
                  last_name COLLATE NOCASE, first_name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |r| {
        let phones: String = r.get(4)?;
        let emails: String = r.get(5)?;
        Ok(Contact {
            id: r.get(0)?,
            first_name: r.get(1)?,
            last_name: r.get(2)?,
            organization: r.get(3)?,
            phones: serde_json::from_str(&phones).unwrap_or_default(),
            emails: serde_json::from_str(&emails).unwrap_or_default(),
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A media item for the gallery grid. Bytes are served separately via the
/// media protocol (by id), never inlined here.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: i64,
    /// "photo" | "video".
    pub kind: String,
    /// App/artifact the media was found in ("Messages", "WhatsApp", …).
    pub source: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub taken_at: Option<i64>,
}

/// Media items that have materialized bytes, newest first. Only items with a
/// `local_path` on disk are listed — the gallery can't show what isn't there.
pub fn list_media(cache: &CacheDb) -> Result<Vec<MediaItem>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, kind, source, mime_type, relative_path, taken_at
         FROM media_items
         WHERE local_path IS NOT NULL
         ORDER BY taken_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let rel: Option<String> = r.get(4)?;
        Ok(MediaItem {
            id: r.get(0)?,
            kind: r.get(1)?,
            source: r.get(2)?,
            mime_type: r.get(3)?,
            // Show just the basename as the filename.
            filename: rel.map(|p| p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string()),
            taken_at: r.get(5)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Distinct media sources present, with a count each, for the gallery filter.
/// Ordered by count descending (biggest sources first).
pub fn media_sources(cache: &CacheDb) -> Result<Vec<(String, i64)>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(source, 'Other') AS s, COUNT(*) AS n
         FROM media_items
         WHERE local_path IS NOT NULL
         GROUP BY s
         ORDER BY n DESC, s",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The on-disk path and MIME type for one media item, for the media protocol
/// handler. Returns `None` if the id is unknown or has no materialized bytes.
pub fn media_blob(cache: &CacheDb, id: i64) -> Result<Option<(String, Option<String>)>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT local_path, mime_type FROM media_items WHERE id = ?1 AND local_path IS NOT NULL",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?)
}

/// A stored value from the backup's `meta` table (device name, etc.), if set.
pub fn meta_value(cache: &CacheDb, key: &str) -> Result<Option<String>> {
    Ok(cache
        .conn()
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed a cache the way the normalizer would: one thread, three messages,
    /// the last carrying an attachment.
    fn seed(cache: &CacheDb) {
        let c = cache.conn();
        c.execute(
            "INSERT INTO threads (id, identifier, display_name, service, last_message_at, message_count)
             VALUES (1, '+15551234567', '+15551234567', 'iMessage', 1717840920, 3)",
            [],
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at, has_attachments)
                VALUES (1, 1, '+15551234567', 0, 'Hey', 1717840800, 0);
             INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at, has_attachments)
                VALUES (2, 1, NULL, 1, 'Hi!', 1717840860, 0);
             INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at, has_attachments)
                VALUES (3, 1, NULL, 1, 'Here', 1717840920, 1);",
        )
        .unwrap();
        c.execute(
            "INSERT INTO attachments (message_id, filename, mime_type, local_path)
             VALUES (3, 'salvage-test.png', 'image/png', '/cache/media/x.png')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn lists_threads_with_snippet_of_latest() {
        let cache = CacheDb::open_in_memory().unwrap();
        seed(&cache);
        let threads = list_threads(&cache).unwrap();
        assert_eq!(threads.len(), 1);
        let t = &threads[0];
        assert_eq!(t.id, 1);
        assert_eq!(t.message_count, 3);
        assert_eq!(t.snippet.as_deref(), Some("Here"));
        assert_eq!(t.last_message_at, Some(1717840920));
    }

    #[test]
    fn empty_cache_lists_no_threads() {
        let cache = CacheDb::open_in_memory().unwrap();
        assert!(list_threads(&cache).unwrap().is_empty());
    }

    #[test]
    fn gets_messages_in_order_with_attachments() {
        let cache = CacheDb::open_in_memory().unwrap();
        seed(&cache);
        let msgs = get_messages(&cache, 1).unwrap();
        assert_eq!(msgs.len(), 3);
        // Oldest first.
        assert_eq!(msgs[0].body.as_deref(), Some("Hey"));
        assert!(!msgs[0].is_from_me);
        assert!(msgs[1].is_from_me);
        // Last message carries the image attachment.
        assert_eq!(msgs[2].attachments.len(), 1);
        assert_eq!(
            msgs[2].attachments[0].mime_type.as_deref(),
            Some("image/png")
        );
        assert_eq!(msgs[0].attachments.len(), 0);
    }

    #[test]
    fn messages_for_unknown_thread_is_empty() {
        let cache = CacheDb::open_in_memory().unwrap();
        seed(&cache);
        assert!(get_messages(&cache, 999).unwrap().is_empty());
    }

    #[test]
    fn lists_only_materialized_media_and_resolves_blob() {
        let cache = CacheDb::open_in_memory().unwrap();
        let c = cache.conn();
        c.execute_batch(
            "INSERT INTO media_items (id, kind, mime_type, relative_path, taken_at, local_path)
                VALUES (1, 'photo', 'image/png', 'Media/DCIM/IMG_0001.png', 1717841460, '/cache/media/a.png');
             INSERT INTO media_items (id, kind, mime_type, relative_path, taken_at, local_path)
                VALUES (2, 'video', 'video/mp4', 'Media/DCIM/IMG_0002.mp4', 1717841520, '/cache/media/b.mp4');
             -- No bytes materialized: must be excluded from the gallery.
             INSERT INTO media_items (id, kind, mime_type, relative_path, local_path)
                VALUES (3, 'photo', 'image/png', 'Media/DCIM/IMG_0003.png', NULL);",
        )
        .unwrap();

        let media = list_media(&cache).unwrap();
        assert_eq!(media.len(), 2, "item without bytes is excluded");
        // Newest first; basename extracted for filename.
        assert_eq!(media[0].id, 2);
        assert_eq!(media[0].kind, "video");
        assert_eq!(media[1].filename.as_deref(), Some("IMG_0001.png"));

        // media_blob resolves path + mime for the handler, None for unknown/no-bytes.
        assert_eq!(
            media_blob(&cache, 1).unwrap(),
            Some(("/cache/media/a.png".into(), Some("image/png".into())))
        );
        assert_eq!(media_blob(&cache, 3).unwrap(), None);
        assert_eq!(media_blob(&cache, 999).unwrap(), None);
    }
}
