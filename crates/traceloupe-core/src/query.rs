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
    /// iMessage receipts (Unix): when the message was read / delivered, if known.
    pub read_at: Option<i64>,
    pub delivered_at: Option<i64>,
    /// Tapback summary folded onto this message, e.g. "❤️×2 👍", or None.
    pub reactions: Option<String>,
    /// Preview of the message this one is an inline reply to, or None.
    pub reply_to_snippet: Option<String>,
    /// The message was edited (iOS 16+).
    pub edited: bool,
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
pub fn count_messages(cache: &CacheDb, thread_id: i64, kind: Option<&str>) -> Result<i64> {
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM messages
         WHERE thread_id = ?1 AND (?2 IS NULL OR kind = ?2)",
        rusqlite::params![thread_id, kind],
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
    kind: Option<&str>,
    desc: bool,
) -> Result<Vec<Message>> {
    let conn = cache.conn();
    // Direction is a fixed keyword chosen here, never interpolated user input.
    let dir = if desc { "DESC" } else { "ASC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id, is_from_me, sender, body, sent_at, read_at, delivered_at, reactions, reply_to_snippet, edited
         FROM messages
         WHERE thread_id = ?1 AND (?4 IS NULL OR kind = ?4)
         ORDER BY sent_at {dir}, id {dir}
         LIMIT ?2 OFFSET ?3",
    ))?;
    let mut messages = stmt
        .query_map(
            rusqlite::params![thread_id, limit, offset, kind],
            row_to_message,
        )?
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
        "SELECT id, is_from_me, sender, body, sent_at, read_at, delivered_at, reactions, reply_to_snippet, edited
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
        read_at: r.get(5)?,
        delivered_at: r.get(6)?,
        reactions: r.get(7)?,
        reply_to_snippet: r.get(8)?,
        edited: r.get::<_, i64>(9)? != 0,
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
/// Distinct content `kind`s present (with counts), for the message content filter
/// pills. `thread_id` scopes to one conversation; otherwise all messages, optionally
/// narrowed to one `service`. NULL kinds (pre-v11 rows) and the catch-all 'other'
/// are omitted (nothing worth a pill).
pub fn message_kinds(
    cache: &CacheDb,
    thread_id: Option<i64>,
    service: Option<&str>,
) -> Result<Vec<(String, i64)>> {
    let conn = cache.conn();
    let map = |r: &rusqlite::Row<'_>| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?));
    if let Some(tid) = thread_id {
        let mut stmt = conn.prepare(
            "SELECT kind, COUNT(*) FROM messages
             WHERE thread_id = ?1 AND kind IS NOT NULL AND kind <> 'other'
             GROUP BY kind ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt
            .query_map([tid], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    } else {
        let mut stmt = conn.prepare(
            "SELECT m.kind, COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.kind IS NOT NULL AND m.kind <> 'other'
               AND (?1 IS NULL OR t.service = ?1)
             GROUP BY m.kind ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt
            .query_map([service], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

pub fn count_all_messages(
    cache: &CacheDb,
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
) -> Result<i64> {
    let conn = cache.conn();
    // Undated messages can't be placed chronologically, so the timeline (and the
    // period buckets, whose range filters already exclude NULLs) omit them —
    // keeping the count and the windowed rows exactly aligned. `service` (None =
    // all) filters to one source app; `search` matches body/sender/conversation.
    // No filter → count messages directly (idx_messages_sent), skipping the join
    // to threads entirely; a service or search filter needs the join.
    let search = search.map(escape_like);
    let n = if service.is_none() && search.is_none() {
        conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE sent_at IS NOT NULL AND (?1 IS NULL OR kind = ?1)",
            rusqlite::params![kind],
            |r| r.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.sent_at IS NOT NULL
               AND (?1 IS NULL OR t.service = ?1)
               AND (?3 IS NULL OR m.kind = ?3)
               AND (?2 IS NULL OR m.body LIKE '%' || ?2 || '%' ESCAPE '\\'
                              OR m.sender LIKE '%' || ?2 || '%' ESCAPE '\\'
                              OR t.display_name LIKE '%' || ?2 || '%' ESCAPE '\\'
                              OR t.identifier LIKE '%' || ?2 || '%' ESCAPE '\\')",
            rusqlite::params![service, search, kind],
            |r| r.get(0),
        )?
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
    search: Option<&str>,
    kind: Option<&str>,
    desc: bool,
) -> Result<Vec<TimelineMessage>> {
    range_window(
        cache,
        TimeRange { lo: None, hi: None },
        offset,
        limit,
        service,
        search,
        kind,
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
    search: Option<&str>,
    kind: Option<&str>,
) -> Result<Vec<i64>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let mut out = Vec::with_capacity(ranges.len());
    // No app/text filter → no join to threads (the common case: one COUNT per
    // bucket over idx_messages_sent). `kind` lives on `messages`, so it stays on
    // the join-free path.
    if service.is_none() && search.is_none() {
        // `sent_at IS NOT NULL` so an all-open range (lo/hi both NULL) counts only
        // what range_window returns — undated messages are excluded from both,
        // keeping count and rows aligned.
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM messages
             WHERE sent_at IS NOT NULL AND (?3 IS NULL OR kind = ?3)
               AND (?1 IS NULL OR sent_at >= ?1) AND (?2 IS NULL OR sent_at < ?2)",
        )?;
        for r in ranges {
            out.push(stmt.query_row(rusqlite::params![r.lo, r.hi, kind], |row| row.get(0))?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.sent_at IS NOT NULL
               AND (?1 IS NULL OR m.sent_at >= ?1)
               AND (?2 IS NULL OR m.sent_at < ?2)
               AND (?3 IS NULL OR t.service = ?3)
               AND (?5 IS NULL OR m.kind = ?5)
               AND (?4 IS NULL OR m.body LIKE '%' || ?4 || '%' ESCAPE '\\'
                              OR m.sender LIKE '%' || ?4 || '%' ESCAPE '\\'
                              OR t.display_name LIKE '%' || ?4 || '%' ESCAPE '\\'
                              OR t.identifier LIKE '%' || ?4 || '%' ESCAPE '\\')",
        )?;
        for r in ranges {
            out.push(stmt.query_row(
                rusqlite::params![r.lo, r.hi, service, search, kind],
                |row| row.get(0),
            )?);
        }
    }
    Ok(out)
}

/// A window of every message whose timestamp falls in `range`, oldest first,
/// across all conversations. Backs a selected period bucket.
#[allow(clippy::too_many_arguments)]
pub fn get_range_window(
    cache: &CacheDb,
    range: TimeRange,
    offset: i64,
    limit: i64,
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
    desc: bool,
) -> Result<Vec<TimelineMessage>> {
    range_window(cache, range, offset, limit, service, search, kind, desc)
}

/// Shared implementation: messages in `range` (open bounds allowed) and optional
/// `service`, joined to their thread for labeling, with attachments, ordered
/// chronologically.
#[allow(clippy::too_many_arguments)]
fn range_window(
    cache: &CacheDb,
    range: TimeRange,
    offset: i64,
    limit: i64,
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
    desc: bool,
) -> Result<Vec<TimelineMessage>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
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
           AND (?7 IS NULL OR m.kind = ?7)
           AND (?6 IS NULL OR m.body LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR m.sender LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR t.display_name LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR t.identifier LIKE '%' || ?6 || '%' ESCAPE '\\')
         ORDER BY m.sent_at {dir}, m.id {dir}
         LIMIT ?3 OFFSET ?4",
    ))?;
    let mut items = stmt
        .query_map(
            rusqlite::params![range.lo, range.hi, limit, offset, service, search, kind],
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
                        // Timeline rows don't show receipts.
                        read_at: None,
                        delivered_at: None,
                        reactions: None,
                        reply_to_snippet: None,
                        edited: false,
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
    /// FaceTime call medium: "audio" | "video". NULL for phone calls.
    pub call_type: Option<String>,
    /// Carrier/geo location string stored on the call, if any.
    pub location: Option<String>,
}

/// Calls, most recent first.
pub fn list_calls(cache: &CacheDb) -> Result<Vec<Call>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, address, direction, answered, duration_s, occurred_at, service, call_type, location
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
        call_type: r.get(7)?,
        location: r.get(8)?,
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
    /// This URL was recorded as deleted from history (a tombstone), not a live visit.
    pub deleted: bool,
}

/// Safari history, most recent first.
pub fn list_safari_history(cache: &CacheDb) -> Result<Vec<HistoryVisit>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, url, title, visited_at, visit_count, deleted
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
        deleted: r.get::<_, i64>(5)? != 0,
    })
}

/// A contact, with phones/emails decoded from the cache's JSON columns.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: i64,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub middle_name: Option<String>,
    pub nickname: Option<String>,
    pub organization: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    /// Birthday as a Unix timestamp, or None.
    pub birthday_at: Option<i64>,
    pub note: Option<String>,
    pub phones: Vec<crate::parsers::address_book::LabeledValue>,
    pub emails: Vec<crate::parsers::address_book::LabeledValue>,
    pub addresses: Vec<crate::parsers::address_book::LabeledValue>,
    /// Whether a photo is stored for this contact (fetched via `contact_image`).
    pub has_image: bool,
    /// 'Address Book' or a third-party app (e.g. 'TikTok'); drives the filter.
    pub source: String,
}

/// One calendar event.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: i64,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub location: Option<String>,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub all_day: bool,
    pub calendar_name: Option<String>,
    pub url: Option<String>,
    /// Free/busy status: "busy" | "free" | "tentative" | "unavailable" | None.
    pub availability: Option<String>,
    /// Part of a repeating series.
    pub recurring: bool,
}

/// Map Calendar's `availability` code to a label (0=busy…3=unavailable).
fn availability_label(code: Option<i64>) -> Option<String> {
    Some(
        match code? {
            0 => "busy",
            1 => "free",
            2 => "tentative",
            3 => "unavailable",
            _ => return None,
        }
        .to_string(),
    )
}

/// Calendar events, most recent first (undated last).
pub fn list_calendar_events(cache: &CacheDb) -> Result<Vec<CalendarEvent>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, title, notes, location, start_at, end_at, all_day, calendar_name, url,
                availability, has_recurrences
         FROM calendar_events
         ORDER BY start_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(CalendarEvent {
            id: r.get(0)?,
            title: r.get(1)?,
            notes: r.get(2)?,
            location: r.get(3)?,
            start_at: r.get(4)?,
            end_at: r.get(5)?,
            all_day: r.get::<_, i64>(6)? != 0,
            calendar_name: r.get(7)?,
            url: r.get(8)?,
            availability: availability_label(r.get(9)?),
            recurring: r.get::<_, i64>(10)? != 0,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One row of the CoreDuet interaction graph: a contact + how much you've
/// communicated with them (across apps) and over what span.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub id: i64,
    pub display_name: Option<String>,
    pub identifier: Option<String>,
    pub incoming: i64,
    pub outgoing: i64,
    /// Messages they sent to a group you were in (recipient, not direct sender).
    pub incoming_recipient: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
}

/// The interaction graph, most-contacted first.
pub fn list_interactions(cache: &CacheDb) -> Result<Vec<Interaction>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, display_name, identifier, incoming, outgoing, incoming_recipient,
                first_at, last_at
         FROM interactions ORDER BY (incoming + outgoing) DESC, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Interaction {
            id: r.get(0)?,
            display_name: r.get(1)?,
            identifier: r.get(2)?,
            incoming: r.get(3)?,
            outgoing: r.get(4)?,
            incoming_recipient: r.get(5)?,
            first_at: r.get(6)?,
            last_at: r.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One Health workout.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Workout {
    pub id: i64,
    pub activity: Option<String>,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub duration_s: Option<i64>,
    pub distance_m: Option<f64>,
}

/// A digest of the Health store's raw-sample volume (from the `meta` table).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    pub sample_count: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub workout_count: i64,
}

/// Workouts, most recent first.
pub fn list_workouts(cache: &CacheDb) -> Result<Vec<Workout>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, activity, start_at, end_at, duration_s, distance_m
         FROM workouts ORDER BY start_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Workout {
            id: r.get(0)?,
            activity: r.get(1)?,
            start_at: r.get(2)?,
            end_at: r.get(3)?,
            duration_s: r.get(4)?,
            distance_m: r.get(5)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The Health summary (sample count + date range + workout count), or a zeroed
/// summary when no Health data was imported.
pub fn health_summary(cache: &CacheDb) -> Result<HealthSummary> {
    let meta_i = |k: &str| -> Option<i64> { cache.get_meta(k).ok().flatten()?.parse().ok() };
    let workout_count: i64 = cache
        .conn()
        .query_row("SELECT COUNT(*) FROM workouts", [], |r| r.get(0))?;
    Ok(HealthSummary {
        sample_count: meta_i("health_sample_count").unwrap_or(0),
        first_at: meta_i("health_first_at"),
        last_at: meta_i("health_last_at"),
        workout_count,
    })
}

/// One reminder.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub id: i64,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub list_name: Option<String>,
    pub due_at: Option<i64>,
    pub completed: bool,
    pub completed_at: Option<i64>,
    pub flagged: bool,
    pub priority: Option<i64>,
    pub created_at: Option<i64>,
}

/// Reminders: open first (by due date), then completed.
pub fn list_reminders(cache: &CacheDb) -> Result<Vec<Reminder>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, title, notes, list_name, due_at, completed, completed_at, flagged, priority,
                created_at
         FROM reminders
         ORDER BY completed, due_at IS NULL, due_at, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Reminder {
            id: r.get(0)?,
            title: r.get(1)?,
            notes: r.get(2)?,
            list_name: r.get(3)?,
            due_at: r.get(4)?,
            completed: r.get::<_, i64>(5)? != 0,
            completed_at: r.get(6)?,
            flagged: r.get::<_, i64>(7)? != 0,
            priority: r.get(8)?,
            created_at: r.get(9)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Contacts, ordered by name (people first, then organization-only entries).
pub fn list_contacts(cache: &CacheDb) -> Result<Vec<Contact>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, first_name, last_name, organization, phones_json, emails_json,
                image IS NOT NULL, source,
                middle_name, nickname, job_title, department, birthday_at, note, addresses_json
         FROM contacts
         ORDER BY last_name IS NULL AND first_name IS NULL,
                  last_name COLLATE NOCASE, first_name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |r| {
        let phones: String = r.get(4)?;
        let emails: String = r.get(5)?;
        let addresses: String = r.get(14)?;
        Ok(Contact {
            id: r.get(0)?,
            first_name: r.get(1)?,
            last_name: r.get(2)?,
            organization: r.get(3)?,
            phones: serde_json::from_str(&phones).unwrap_or_default(),
            emails: serde_json::from_str(&emails).unwrap_or_default(),
            addresses: serde_json::from_str(&addresses).unwrap_or_default(),
            has_image: r.get(6)?,
            source: r.get(7)?,
            middle_name: r.get(8)?,
            nickname: r.get(9)?,
            job_title: r.get(10)?,
            department: r.get(11)?,
            birthday_at: r.get(12)?,
            note: r.get(13)?,
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
#[derive(Debug, Clone, Serialize, PartialEq)]
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
    /// Comma-separated names of people detected in the photo, or None.
    pub persons: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub favorite: bool,
    /// Moment place/event name (e.g. "Florida"), or None.
    pub location: Option<String>,
    /// User album names this photo is in, comma-separated, or None.
    pub albums: Option<String>,
    /// Pixel dimensions and (video) duration.
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_s: Option<f64>,
    /// Original file size in bytes.
    pub file_size: Option<i64>,
    /// Camera "<make> <model>", lens model, and a formatted EXIF exposure summary.
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub exif: Option<String>,
    /// In the device's Hidden album (surfaced as a badge, not excluded).
    pub hidden: bool,
    /// Media subtype ("screenshot" | "panorama"), or None.
    pub subtype: Option<String>,
}

/// Media items that have materialized bytes, newest first. Only items with a
/// `local_path` on disk are listed — the gallery can't show what isn't there.
pub fn list_media(cache: &CacheDb) -> Result<Vec<MediaItem>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, kind, source, mime_type, relative_path, taken_at, persons,
                latitude, longitude, is_favorite, location, albums,
                width, height, duration_s, file_size, camera, lens, exif, hidden, subtype
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
        persons: r.get(6)?,
        latitude: r.get(7)?,
        longitude: r.get(8)?,
        favorite: r.get::<_, i64>(9)? != 0,
        location: r.get(10)?,
        albums: r.get(11)?,
        width: r.get(12)?,
        height: r.get(13)?,
        duration_s: r.get(14)?,
        file_size: r.get(15)?,
        camera: r.get(16)?,
        lens: r.get(17)?,
        exif: r.get(18)?,
        hidden: r.get::<_, i64>(19)? != 0,
        subtype: r.get(20)?,
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
pub fn count_media(
    cache: &CacheDb,
    source: Option<&str>,
    range: TimeRange,
    search: Option<&str>,
) -> Result<i64> {
    // `COALESCE(source,'Other')` so the synthesized "Other" bucket (NULL source)
    // is actually selectable — `source = 'Other'` never matches a NULL. Matches
    // the label built by `media_sources`. `search` matches the filename.
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM media_items
         WHERE local_path IS NOT NULL
           AND (?1 IS NULL OR COALESCE(source, 'Other') = ?1)
           AND (?2 IS NULL OR taken_at >= ?2)
           AND (?3 IS NULL OR taken_at < ?3)
           AND (?4 IS NULL OR relative_path LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR persons LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR location LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR albums LIKE '%' || ?4 || '%' ESCAPE '\\')",
        rusqlite::params![source, range.lo, range.hi, search],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Media counts for each `range` in `source` (respecting `search`) — powers the
/// Photos time-filter chips. One row per range, order preserved.
pub fn count_media_ranges(
    cache: &CacheDb,
    source: Option<&str>,
    ranges: &[TimeRange],
    search: Option<&str>,
) -> Result<Vec<i64>> {
    let search = search.map(escape_like);
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM media_items
         WHERE local_path IS NOT NULL
           AND (?1 IS NULL OR COALESCE(source, 'Other') = ?1)
           AND (?2 IS NULL OR taken_at >= ?2)
           AND (?3 IS NULL OR taken_at < ?3)
           AND (?4 IS NULL OR relative_path LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR persons LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR location LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR albums LIKE '%' || ?4 || '%' ESCAPE '\\')",
    )?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(
            stmt.query_row(rusqlite::params![source, r.lo, r.hi, search], |row| {
                row.get(0)
            })?,
        );
    }
    Ok(out)
}

pub fn get_media_window(
    cache: &CacheDb,
    source: Option<&str>,
    range: TimeRange,
    search: Option<&str>,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<MediaItem>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, kind, source, mime_type, relative_path, taken_at, persons,
                latitude, longitude, is_favorite, location, albums,
                width, height, duration_s, file_size, camera, lens, exif, hidden, subtype
         FROM media_items
         WHERE local_path IS NOT NULL
           AND (?1 IS NULL OR COALESCE(source, 'Other') = ?1)
           AND (?4 IS NULL OR taken_at >= ?4)
           AND (?5 IS NULL OR taken_at < ?5)
           AND (?6 IS NULL OR relative_path LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR persons LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR location LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR albums LIKE '%' || ?6 || '%' ESCAPE '\\')
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![source, limit, offset, range.lo, range.hi, search],
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
pub fn count_calls(cache: &CacheDb, search: Option<&str>, range: TimeRange) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM calls
         WHERE (?1 IS NULL OR address LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?2 IS NULL OR occurred_at >= ?2)
           AND (?3 IS NULL OR occurred_at < ?3)",
        rusqlite::params![search, range.lo, range.hi],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Call counts for each `range` (respecting `search`) — powers the time-filter chips.
pub fn count_call_ranges(
    cache: &CacheDb,
    ranges: &[TimeRange],
    search: Option<&str>,
) -> Result<Vec<i64>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM calls
         WHERE (?1 IS NULL OR address LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?2 IS NULL OR occurred_at >= ?2)
           AND (?3 IS NULL OR occurred_at < ?3)",
    )?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(stmt.query_row(rusqlite::params![search, r.lo, r.hi], |row| row.get(0))?);
    }
    Ok(out)
}

pub fn get_calls_window(
    cache: &CacheDb,
    search: Option<&str>,
    range: TimeRange,
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
        "SELECT id, address, direction, answered, duration_s, occurred_at, service, call_type, location
         FROM calls
         WHERE (?1 IS NULL OR address LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?4 IS NULL OR occurred_at >= ?4)
           AND (?5 IS NULL OR occurred_at < ?5)
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![search, limit, offset, range.lo, range.hi],
        row_to_call,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Safari visits whose URL or title matches `search` (or all when NULL) and
/// whose `visited_at` falls in `range` (open bounds = no limit; undated visits
/// only count when both bounds are open).
pub fn count_safari(cache: &CacheDb, search: Option<&str>, range: TimeRange) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM safari_history
         WHERE (?1 IS NULL OR url LIKE '%' || ?1 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?2 IS NULL OR visited_at >= ?2)
           AND (?3 IS NULL OR visited_at < ?3)",
        rusqlite::params![search, range.lo, range.hi],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Safari-visit counts for each `range` (respecting `search`) — the time-filter
/// chips. One row per range, order preserved.
pub fn count_safari_ranges(
    cache: &CacheDb,
    search: Option<&str>,
    ranges: &[TimeRange],
) -> Result<Vec<i64>> {
    let search = search.map(escape_like);
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM safari_history
         WHERE (?1 IS NULL OR url LIKE '%' || ?1 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?2 IS NULL OR visited_at >= ?2)
           AND (?3 IS NULL OR visited_at < ?3)",
    )?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(stmt.query_row(rusqlite::params![search, r.lo, r.hi], |row| row.get(0))?);
    }
    Ok(out)
}

pub fn get_safari_window(
    cache: &CacheDb,
    search: Option<&str>,
    range: TimeRange,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<HistoryVisit>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, url, title, visited_at, visit_count, deleted
         FROM safari_history
         WHERE (?1 IS NULL OR url LIKE '%' || ?1 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?4 IS NULL OR visited_at >= ?4)
           AND (?5 IS NULL OR visited_at < ?5)
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![search, limit, offset, range.lo, range.hi],
        row_to_visit,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A Safari bookmark, reading-list item, or open tab (kind selects which).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafariBookmark {
    pub id: i64,
    pub kind: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub folder: Option<String>,
    pub date_added: Option<i64>,
    pub date_viewed: Option<i64>,
    pub preview_text: Option<String>,
}

fn row_to_bookmark(r: &rusqlite::Row<'_>) -> rusqlite::Result<SafariBookmark> {
    Ok(SafariBookmark {
        id: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        url: r.get(3)?,
        folder: r.get(4)?,
        date_added: r.get(5)?,
        date_viewed: r.get(6)?,
        preview_text: r.get(7)?,
    })
}

/// Count of one Safari `kind` ('bookmark' | 'reading_list' | 'tab') matching
/// `search` (url/title substring) within `range` (over `date_added`).
pub fn count_safari_bookmarks(
    cache: &CacheDb,
    kind: &str,
    search: Option<&str>,
    range: TimeRange,
) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM safari_bookmarks
         WHERE kind = ?1
           AND (?2 IS NULL OR url LIKE '%' || ?2 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?2 || '%' ESCAPE '\\')
           AND (?3 IS NULL OR date_added >= ?3)
           AND (?4 IS NULL OR date_added < ?4)",
        rusqlite::params![kind, search, range.lo, range.hi],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Per-range counts of one Safari `kind` (respecting `search`) — the time chips.
pub fn count_safari_bookmark_ranges(
    cache: &CacheDb,
    kind: &str,
    search: Option<&str>,
    ranges: &[TimeRange],
) -> Result<Vec<i64>> {
    let search = search.map(escape_like);
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM safari_bookmarks
         WHERE kind = ?1
           AND (?2 IS NULL OR url LIKE '%' || ?2 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?2 || '%' ESCAPE '\\')
           AND (?3 IS NULL OR date_added >= ?3)
           AND (?4 IS NULL OR date_added < ?4)",
    )?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(
            stmt.query_row(rusqlite::params![kind, search, r.lo, r.hi], |row| {
                row.get(0)
            })?,
        );
    }
    Ok(out)
}

/// A window of one Safari `kind`, matching `search` within `range`, ordered by
/// `sort` (an allowlisted column from the command layer).
pub fn get_safari_bookmarks_window(
    cache: &CacheDb,
    kind: &str,
    search: Option<&str>,
    range: TimeRange,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<SafariBookmark>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, kind, title, url, folder, date_added, date_viewed, preview_text
         FROM safari_bookmarks
         WHERE kind = ?1
           AND (?2 IS NULL OR url LIKE '%' || ?2 || '%' ESCAPE '\\'
                          OR title LIKE '%' || ?2 || '%' ESCAPE '\\')
           AND (?5 IS NULL OR date_added >= ?5)
           AND (?6 IS NULL OR date_added < ?6)
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?3 OFFSET ?4",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![kind, search, limit, offset, range.lo, range.hi],
        row_to_bookmark,
    )?;
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
    /// Rich-content indicators: has a checklist, and counts of embedded
    /// image/video attachments vs total attachments (tables, drawings, files…).
    pub has_checklist: bool,
    pub image_count: i64,
    pub attachment_count: i64,
    /// Hashtag tags on the note (iOS 15+); empty when none.
    pub tags: Vec<String>,
}

/// Notes, most-recently-modified first.
pub fn list_notes(cache: &CacheDb) -> Result<Vec<Note>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, folder, title, snippet, body_html, created_at, modified_at, locked, password_hint, pinned,
                has_checklist, image_count, attachment_count, tags
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
            has_checklist: r.get::<_, i64>(10)? != 0,
            image_count: r.get(11)?,
            attachment_count: r.get(12)?,
            tags: r
                .get::<_, Option<String>>(13)?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A locked note's crypto params: `(salt, iterations, iv, tag, encrypted_data,
/// wrapped_key)`. `wrapped_key` is empty when the note key is derived directly.
pub type NoteCrypto = (Vec<u8>, i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

/// The crypto params needed to unlock note `id`, if it's a locked note with all
/// params present. Used by the unlock command to decrypt on demand.
pub fn note_crypto(cache: &CacheDb, id: i64) -> Result<Option<NoteCrypto>> {
    Ok(cache
        .conn()
        .query_row(
            // `crypto_iter` is intentionally NOT required: decrypt_note treats a 0/
            // absent iteration count as the 20000 default, so a schema that omits
            // ZCRYPTOITERATIONCOUNT should still get a password prompt, not a
            // "data missing" error. Read it optionally and default to 0.
            "SELECT crypto_salt, crypto_iter, crypto_iv, crypto_tag, encrypted_data,
                    crypto_wrapped_key
             FROM notes
             WHERE id = ?1 AND locked = 1
               AND crypto_salt IS NOT NULL
               AND crypto_iv IS NOT NULL AND crypto_tag IS NOT NULL
               AND encrypted_data IS NOT NULL",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?.unwrap_or_default(),
                ))
            },
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
