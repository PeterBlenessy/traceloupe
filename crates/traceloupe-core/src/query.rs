//! Read-side queries over the cache DB (architecture §6: "every browse is a
//! cache query"). Pure reads, returning serializable view models the shell
//! hands straight to the UI. No engine or decryption concerns here.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

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
    /// Member handles for a group chat (empty/one for a 1:1).
    pub participants: Vec<String>,
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

/// One message in the cross-conversation timeline: a message plus the thread it
/// belongs to, so the flat stream can label each row with its conversation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineMessage {
    pub thread_id: i64,
    pub thread_title: String,
    /// The thread's identifier — for a 1:1 chat this is the other party's handle,
    /// so the timeline can resolve/show the conversation partner even on your own
    /// outgoing messages (where `message.sender` is you). Empty if unknown.
    pub thread_handle: String,
    pub service: Option<String>,
    pub message: Message,
}

/// A half-open time window `[lo, hi)` in epoch seconds; either bound may be open
/// (`None`). Used to bucket messages by recency for the periods view.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub lo: Option<i64>,
    pub hi: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: i64,
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
                t.last_message_at, t.message_count, t.participants_json,
                (SELECT m.body FROM messages m
                  WHERE m.thread_id = t.id
                  ORDER BY m.sent_at DESC, m.id DESC LIMIT 1) AS snippet
         FROM threads t
         ORDER BY t.last_message_at DESC NULLS LAST, t.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let participants: String = r.get(6)?;
        Ok(ThreadSummary {
            id: r.get(0)?,
            identifier: r.get(1)?,
            display_name: r.get(2)?,
            service: r.get(3)?,
            last_message_at: r.get(4)?,
            message_count: r.get(5)?,
            participants: serde_json::from_str(&participants).unwrap_or_default(),
            snippet: r.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Total number of messages in a thread. Cheap; drives the virtual scroller so
/// the UI can lazily fetch only the windows it renders.
pub fn count_messages(cache: &CacheDb, thread_id: i64) -> Result<i64> {
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM messages WHERE thread_id = ?1",
        [thread_id],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// A window of a thread's messages, oldest first, each with its attachments.
/// `offset` counts from the oldest message. Threads can hold tens of thousands
/// of messages, so the UI never loads a whole thread — it requests the slices
/// it is about to display.
pub fn get_message_window(
    cache: &CacheDb,
    thread_id: i64,
    offset: i64,
    limit: i64,
    desc: bool,
) -> Result<Vec<Message>> {
    let conn = cache.conn();
    // Direction is a fixed keyword chosen here, never interpolated user input.
    let dir = if desc { "DESC" } else { "ASC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id, is_from_me, sender, body, sent_at
         FROM messages
         WHERE thread_id = ?1
         ORDER BY sent_at {dir}, id {dir}
         LIMIT ?2 OFFSET ?3",
    ))?;
    let mut messages = stmt
        .query_map(rusqlite::params![thread_id, limit, offset], row_to_message)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Load attachments only for this window's messages, not the whole thread —
    // otherwise every window fetch rescans all of a large thread's attachments.
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    let atts = attachments_by_ids(conn, &ids)?;
    for m in &mut messages {
        if let Some(a) = atts.get(&m.id) {
            m.attachments = a.clone();
        }
    }
    Ok(messages)
}

/// All messages in a thread, oldest first, each with its attachments. Used by
/// tests and small callers; large threads should use [`get_message_window`].
pub fn get_messages(cache: &CacheDb, thread_id: i64) -> Result<Vec<Message>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, is_from_me, sender, body, sent_at
         FROM messages
         WHERE thread_id = ?1
         ORDER BY sent_at ASC, id ASC",
    )?;
    let mut messages = stmt
        .query_map([thread_id], row_to_message)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    load_attachments(conn, thread_id, &mut messages)?;
    Ok(messages)
}

fn row_to_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        is_from_me: r.get::<_, i64>(1)? != 0,
        sender: r.get(2)?,
        body: r.get(3)?,
        sent_at: r.get(4)?,
        attachments: Vec::new(),
    })
}

/// Attach media to already-loaded messages with a single grouped query,
/// avoiding an N+1 lookup that would stall large threads.
fn load_attachments(
    conn: &rusqlite::Connection,
    thread_id: i64,
    messages: &mut [Message],
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let mut index = std::collections::HashMap::with_capacity(messages.len());
    for (i, m) in messages.iter().enumerate() {
        index.insert(m.id, i);
    }
    let mut att_stmt = conn.prepare(
        "SELECT a.message_id, a.id, a.filename, a.mime_type, a.local_path
         FROM attachments a
         JOIN messages m ON m.id = a.message_id
         WHERE m.thread_id = ?1",
    )?;
    let rows = att_stmt.query_map([thread_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            Attachment {
                id: r.get(1)?,
                filename: r.get(2)?,
                mime_type: r.get(3)?,
                local_path: r.get(4)?,
            },
        ))
    })?;
    for row in rows {
        let (message_id, att) = row?;
        if let Some(&i) = index.get(&message_id) {
            messages[i].attachments.push(att);
        }
    }
    Ok(())
}

/// Path + filename + mime + (encrypted-backup) decrypt fields for a message
/// attachment. `decrypt_key`/`plain_size` are `None` when `local_path` is already
/// plaintext (an iLEAPP-extracted file or an unencrypted backup). The `filename`
/// carries the original name (with its real extension) — needed to detect an
/// image when `mime_type` is NULL, since an encrypted backup's on-disk path is a
/// content-addressed / `.decrypted` temp with no meaningful extension. Returns
/// None when the file wasn't resolved during import.
pub type AttachmentBlob = (
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<i64>,
);

pub fn attachment_blob(cache: &CacheDb, attachment_id: i64) -> Result<Option<AttachmentBlob>> {
    let row = cache
        .conn()
        .query_row(
            "SELECT local_path, filename, mime_type, decrypt_key, plain_size FROM attachments
             WHERE id = ?1 AND local_path IS NOT NULL",
            [attachment_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/// Total messages across every conversation. Drives the timeline's virtual
/// scroller. Also ensures the timeline ordering index exists, migrating caches
/// created before the timeline feature.
pub fn count_all_messages(cache: &CacheDb, service: Option<&str>) -> Result<i64> {
    let conn = cache.conn();
    // Undated messages can't be placed chronologically, so the timeline (and the
    // period buckets, whose range filters already exclude NULLs) omit them —
    // keeping the count and the windowed rows exactly aligned. `service` (None =
    // all) filters to one source app (iMessage/SMS/TikTok/…) for the app filter.
    // No app filter → count messages directly (idx_messages_sent), skipping the
    // join to threads entirely; only the service filter needs it.
    let n = match service {
        None => conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE sent_at IS NOT NULL",
            [],
            |r| r.get(0),
        )?,
        Some(svc) => conn.query_row(
            "SELECT COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.sent_at IS NOT NULL AND t.service = ?1",
            [svc],
            |r| r.get(0),
        )?,
    };
    Ok(n)
}

/// A window of the cross-conversation timeline: every message from every thread,
/// oldest first, sliced by `offset`. `service` filters by source app (None=all).
pub fn get_timeline_window(
    cache: &CacheDb,
    offset: i64,
    limit: i64,
    service: Option<&str>,
    desc: bool,
) -> Result<Vec<TimelineMessage>> {
    range_window(
        cache,
        TimeRange { lo: None, hi: None },
        offset,
        limit,
        service,
        desc,
    )
}

/// Message counts for each of the given time windows. Powers the periods view's
/// bucket list (e.g. "Last 7 days: 812"). One row per range, order preserved.
/// `service` filters by source app (None = all).
pub fn count_message_ranges(
    cache: &CacheDb,
    ranges: &[TimeRange],
    service: Option<&str>,
) -> Result<Vec<i64>> {
    let conn = cache.conn();
    // No app filter → no join to threads (the common case: one COUNT per bucket).
    let mut stmt = match service {
        None => conn.prepare(
            // `sent_at IS NOT NULL` so an all-open range (lo/hi both NULL) counts
            // only what the window (range_window) returns — undated messages are
            // excluded from both, keeping count and rows aligned.
            "SELECT COUNT(*) FROM messages
             WHERE sent_at IS NOT NULL
               AND (?1 IS NULL OR sent_at >= ?1) AND (?2 IS NULL OR sent_at < ?2)",
        )?,
        Some(_) => conn.prepare(
            "SELECT COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.sent_at IS NOT NULL
               AND (?1 IS NULL OR m.sent_at >= ?1)
               AND (?2 IS NULL OR m.sent_at < ?2)
               AND t.service = ?3",
        )?,
    };
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(match service {
            None => stmt.query_row(rusqlite::params![r.lo, r.hi], |row| row.get(0))?,
            Some(svc) => stmt.query_row(rusqlite::params![r.lo, r.hi, svc], |row| row.get(0))?,
        });
    }
    Ok(out)
}

/// A window of every message whose timestamp falls in `range`, oldest first,
/// across all conversations. Backs a selected period bucket.
pub fn get_range_window(
    cache: &CacheDb,
    range: TimeRange,
    offset: i64,
    limit: i64,
    service: Option<&str>,
    desc: bool,
) -> Result<Vec<TimelineMessage>> {
    range_window(cache, range, offset, limit, service, desc)
}

/// Shared implementation: messages in `range` (open bounds allowed) and optional
/// `service`, joined to their thread for labeling, with attachments, ordered
/// chronologically.
fn range_window(
    cache: &CacheDb,
    range: TimeRange,
    offset: i64,
    limit: i64,
    service: Option<&str>,
    desc: bool,
) -> Result<Vec<TimelineMessage>> {
    let conn = cache.conn();
    // Direction is a fixed keyword chosen here, never interpolated user input.
    let dir = if desc { "DESC" } else { "ASC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT m.id, m.is_from_me, m.sender, m.body, m.sent_at,
                m.thread_id, t.display_name, t.identifier, t.service
         FROM messages m
         JOIN threads t ON t.id = m.thread_id
         WHERE m.sent_at IS NOT NULL
           AND (?1 IS NULL OR m.sent_at >= ?1)
           AND (?2 IS NULL OR m.sent_at < ?2)
           AND (?5 IS NULL OR t.service = ?5)
         ORDER BY m.sent_at {dir}, m.id {dir}
         LIMIT ?3 OFFSET ?4",
    ))?;
    let mut items = stmt
        .query_map(
            rusqlite::params![range.lo, range.hi, limit, offset, service],
            |r| {
                let display_name: Option<String> = r.get(6)?;
                let identifier: String = r.get(7)?;
                let thread_title = display_name
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| identifier.clone());
                Ok(TimelineMessage {
                    thread_id: r.get(5)?,
                    thread_title,
                    thread_handle: identifier,
                    service: r.get(8)?,
                    message: Message {
                        id: r.get(0)?,
                        is_from_me: r.get::<_, i64>(1)? != 0,
                        sender: r.get(2)?,
                        body: r.get(3)?,
                        sent_at: r.get(4)?,
                        attachments: Vec::new(),
                    },
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Attach media for just this window's messages (they span many threads, so
    // we look up by message id rather than by thread).
    let ids: Vec<i64> = items.iter().map(|it| it.message.id).collect();
    let atts = attachments_by_ids(conn, &ids)?;
    for it in &mut items {
        if let Some(a) = atts.get(&it.message.id) {
            it.message.attachments = a.clone();
        }
    }
    Ok(items)
}

/// Attachments for an explicit set of message ids, grouped by message id.
fn attachments_by_ids(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<Attachment>>> {
    let mut map: std::collections::HashMap<i64, Vec<Attachment>> = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT message_id, id, filename, mime_type, local_path
         FROM attachments WHERE message_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            Attachment {
                id: r.get(1)?,
                filename: r.get(2)?,
                mime_type: r.get(3)?,
                local_path: r.get(4)?,
            },
        ))
    })?;
    for row in rows {
        let (mid, att) = row?;
        map.entry(mid).or_default().push(att);
    }
    Ok(map)
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
    let rows = stmt.query_map([], row_to_call)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn row_to_call(r: &rusqlite::Row<'_>) -> rusqlite::Result<Call> {
    Ok(Call {
        id: r.get(0)?,
        address: r.get(1)?,
        direction: r.get(2)?,
        answered: r.get::<_, Option<i64>>(3)?.map(|a| a != 0),
        duration_s: r.get(4)?,
        occurred_at: r.get(5)?,
        service: r.get(6)?,
    })
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
    let rows = stmt.query_map([], row_to_visit)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn row_to_visit(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryVisit> {
    Ok(HistoryVisit {
        id: r.get(0)?,
        url: r.get(1)?,
        title: r.get(2)?,
        visited_at: r.get(3)?,
        visit_count: r.get(4)?,
    })
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
    /// Whether a photo is stored for this contact (fetched via `contact_image`).
    pub has_image: bool,
    /// 'Address Book' or a third-party app (e.g. 'TikTok'); drives the filter.
    pub source: String,
}

/// Contacts, ordered by name (people first, then organization-only entries).
pub fn list_contacts(cache: &CacheDb) -> Result<Vec<Contact>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, first_name, last_name, organization, phones_json, emails_json,
                image IS NOT NULL, source
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
            has_image: r.get(6)?,
            source: r.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The stored photo thumbnail bytes for a contact, if any.
pub fn contact_image(cache: &CacheDb, contact_id: i64) -> Result<Option<Vec<u8>>> {
    let blob = cache
        .conn()
        .query_row(
            "SELECT image FROM contacts WHERE id = ?1",
            [contact_id],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten();
    Ok(blob)
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
    let rows = stmt.query_map([], row_to_media)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn row_to_media(r: &rusqlite::Row<'_>) -> rusqlite::Result<MediaItem> {
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
}

// --- Windowed, filterable list queries -------------------------------------
// Each artifact list has a `count_*` and `get_*_window` pair so the UI can
// virtualize/lazy-load huge lists (a large camera roll, years of history) the
// same way Messages does — fetching only the visible slice. Filtering/search
// happens in SQL so the count and the windows stay consistent. A NULL filter
// matches everything.

/// Photos/videos in `source` ("Photos", "Messages", …), or all when NULL, whose
/// `taken_at` falls in `range` (open bounds = no limit; undated media only count
/// when both bounds are open).
pub fn count_media(cache: &CacheDb, source: Option<&str>, range: TimeRange) -> Result<i64> {
    // `COALESCE(source,'Other')` so the synthesized "Other" bucket (NULL source)
    // is actually selectable — `source = 'Other'` never matches a NULL. Matches
    // the label built by `media_sources`.
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM media_items
         WHERE local_path IS NOT NULL
           AND (?1 IS NULL OR COALESCE(source, 'Other') = ?1)
           AND (?2 IS NULL OR taken_at >= ?2)
           AND (?3 IS NULL OR taken_at < ?3)",
        rusqlite::params![source, range.lo, range.hi],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Media counts for each `range` in `source` — powers the Photos time-filter
/// chips. One row per range, order preserved.
pub fn count_media_ranges(
    cache: &CacheDb,
    source: Option<&str>,
    ranges: &[TimeRange],
) -> Result<Vec<i64>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM media_items
         WHERE local_path IS NOT NULL
           AND (?1 IS NULL OR COALESCE(source, 'Other') = ?1)
           AND (?2 IS NULL OR taken_at >= ?2)
           AND (?3 IS NULL OR taken_at < ?3)",
    )?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(stmt.query_row(rusqlite::params![source, r.lo, r.hi], |row| row.get(0))?);
    }
    Ok(out)
}

pub fn get_media_window(
    cache: &CacheDb,
    source: Option<&str>,
    range: TimeRange,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<MediaItem>> {
    let conn = cache.conn();
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, kind, source, mime_type, relative_path, taken_at
         FROM media_items
         WHERE local_path IS NOT NULL
           AND (?1 IS NULL OR COALESCE(source, 'Other') = ?1)
           AND (?4 IS NULL OR taken_at >= ?4)
           AND (?5 IS NULL OR taken_at < ?5)
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![source, limit, offset, range.lo, range.hi],
        row_to_media,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A list sort: an allowlisted column expression plus a direction. The column
/// is interpolated into SQL, so it MUST come from a trusted literal (the command
/// layer maps a client-supplied field name to one of a fixed set of `&'static
/// str` columns) — never from raw user input.
#[derive(Debug, Clone, Copy)]
pub struct Sort {
    column: &'static str,
    desc: bool,
}

impl Sort {
    pub fn new(column: &'static str, desc: bool) -> Self {
        Self { column, desc }
    }
    fn column(&self) -> &'static str {
        self.column
    }
    /// `(direction, null-placement)` — nulls sort last when descending (newest
    /// first) and first when ascending, so undated rows stay at the far end.
    fn order_sql(&self) -> (&'static str, &'static str) {
        if self.desc {
            ("DESC", "NULLS LAST")
        } else {
            ("ASC", "NULLS FIRST")
        }
    }
}

/// Escape LIKE metacharacters (`%`, `_`, `\`) in a user search term so they match
/// literally instead of acting as wildcards. Pair with `ESCAPE '\'` in the query.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Calls whose address matches `search` (substring), or all when NULL.
pub fn count_calls(cache: &CacheDb, search: Option<&str>) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM calls
         WHERE (?1 IS NULL OR address LIKE '%' || ?1 || '%' ESCAPE '\\')",
        [search],
        |r| r.get(0),
    )?;
    Ok(n)
}

pub fn get_calls_window(
    cache: &CacheDb,
    search: Option<&str>,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<Call>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    // `sort.column()` is an allowlisted SQL fragment (never raw user input); see
    // the `Sort` type. `id` is the stable tiebreaker.
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, address, direction, answered, duration_s, occurred_at, service
         FROM calls
         WHERE (?1 IS NULL OR address LIKE '%' || ?1 || '%' ESCAPE '\\')
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![search, limit, offset], row_to_call)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Safari visits whose URL or title matches `search`, or all when NULL.
pub fn count_safari(cache: &CacheDb, search: Option<&str>) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM safari_history
         WHERE (?1 IS NULL OR url LIKE '%' || ?1 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?1 || '%' ESCAPE '\\')",
        [search],
        |r| r.get(0),
    )?;
    Ok(n)
}

pub fn get_safari_window(
    cache: &CacheDb,
    search: Option<&str>,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<HistoryVisit>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, url, title, visited_at, visit_count
         FROM safari_history
         WHERE (?1 IS NULL OR url LIKE '%' || ?1 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?1 || '%' ESCAPE '\\')
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![search, limit, offset], row_to_visit)?;
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

/// What the media protocol needs to serve one item:
/// `(local_path, mime, thumb_path, decrypt_key, plain_size)`. Returns `None` if
/// the id is unknown or has no materialized bytes. `decrypt_key` is the
/// class-prefixed wrapped key for an encrypted backup's original (see
/// [`crate::crypto`]) and `plain_size` its real length (to trim CBC padding);
/// both are `None` when `local_path` is already plaintext.
pub type MediaBlob = (
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<i64>,
);

pub fn media_blob(cache: &CacheDb, id: i64) -> Result<Option<MediaBlob>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT local_path, mime_type, thumb_path, decrypt_key, plain_size
             FROM media_items
             WHERE id = ?1 AND local_path IS NOT NULL",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?)
}

/// One note from the device's Notes app.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: i64,
    pub folder: Option<String>,
    pub title: Option<String>,
    pub snippet: Option<String>,
    /// The note body (plain text). `None` for a locked note until it's unlocked.
    pub body: Option<String>,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    /// Pinned to the top of the Notes app.
    pub pinned: bool,
    /// Password-protected: the body is withheld until unlocked with the password.
    pub locked: bool,
    /// The user's password hint, if the note stored one.
    pub password_hint: Option<String>,
}

/// Notes, most-recently-modified first.
pub fn list_notes(cache: &CacheDb) -> Result<Vec<Note>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, folder, title, snippet, body_html, created_at, modified_at, locked, password_hint, pinned
         FROM notes
         ORDER BY modified_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Note {
            id: r.get(0)?,
            folder: r.get(1)?,
            title: r.get(2)?,
            snippet: r.get(3)?,
            body: r.get(4)?,
            created_at: r.get(5)?,
            modified_at: r.get(6)?,
            locked: r.get::<_, i64>(7)? != 0,
            password_hint: r.get(8)?,
            pinned: r.get::<_, i64>(9)? != 0,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A locked note's crypto params: `(salt, iterations, iv, tag, encrypted_data)`.
pub type NoteCrypto = (Vec<u8>, i64, Vec<u8>, Vec<u8>, Vec<u8>);

/// The crypto params needed to unlock note `id`, if it's a locked note with all
/// params present. Used by the unlock command to decrypt on demand.
pub fn note_crypto(cache: &CacheDb, id: i64) -> Result<Option<NoteCrypto>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT crypto_salt, crypto_iter, crypto_iv, crypto_tag, encrypted_data
             FROM notes
             WHERE id = ?1 AND locked = 1
               AND crypto_salt IS NOT NULL AND crypto_iter IS NOT NULL
               AND crypto_iv IS NOT NULL AND crypto_tag IS NOT NULL
               AND encrypted_data IS NOT NULL",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?)
}

/// One voice recording (Voice Memos).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub id: i64,
    /// User label, or None for an auto-named memo (the UI derives one).
    pub title: Option<String>,
    pub folder: Option<String>,
    pub recorded_at: Option<i64>,
    pub duration_s: Option<f64>,
    /// Trailing filename of the `.m4a`, so the UI can label an untitled memo.
    pub file_name: Option<String>,
}

/// Voice recordings, most-recent first (undated memos last).
pub fn list_recordings(cache: &CacheDb) -> Result<Vec<Recording>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, title, folder, recorded_at, duration_s, relative_path
         FROM recordings
         ORDER BY recorded_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let relative_path: String = r.get(5)?;
        let file_name = relative_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(Recording {
            id: r.get(0)?,
            title: r.get(1)?,
            folder: r.get(2)?,
            recorded_at: r.get(3)?,
            duration_s: r.get(4)?,
            file_name,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The bytes-serving fields for one recording: `(local_path, mime, decrypt_key,
/// plain_size)`. `decrypt_key`/`plain_size` are `None` when the `.m4a` is already
/// plaintext (see [`media_blob`]).
pub type RecordingBlob = (String, Option<String>, Option<Vec<u8>>, Option<i64>);

pub fn recording_blob(cache: &CacheDb, id: i64) -> Result<Option<RecordingBlob>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT local_path, mime_type, decrypt_key, plain_size
             FROM recordings WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?)
}

/// Bundle IDs of apps installed on the device, sorted.
pub fn list_installed_apps(cache: &CacheDb) -> Result<Vec<String>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare("SELECT bundle_id FROM installed_apps ORDER BY bundle_id")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
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
             VALUES (3, 'traceloupe-test.png', 'image/png', '/cache/media/x.png')",
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
            Some((
                "/cache/media/a.png".into(),
                Some("image/png".into()),
                None,
                None,
                None
            ))
        );
        assert_eq!(media_blob(&cache, 3).unwrap(), None);
        assert_eq!(media_blob(&cache, 999).unwrap(), None);
    }
}
