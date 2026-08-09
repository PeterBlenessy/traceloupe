//! Read-side queries over the cache DB (architecture §6: "every browse is a
//! cache query"). Pure reads, returning serializable view models the shell
//! hands straight to the UI. No engine or decryption concerns here.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::cache::CacheDb;
use crate::indicators::FeedInfo;
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
    ///
    /// Only the iMessage/SMS path fills this — every app module leaves it empty
    /// even for groups, so it can't be used to decide group-ness. Use
    /// [`ThreadSummary::is_group`] for that.
    pub participants: Vec<String>,
    /// Whether the parser identified this thread as a group chat.
    pub is_group: bool,
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
    /// Content class: text / media / link / shared / sticker / system. `system`
    /// marks a non-bubble group-action row (rename/add/remove/leave).
    pub kind: Option<String>,
    /// Expressive send effect it was sent with (e.g. "Confetti", "Slam"), or None.
    pub effect: Option<String>,
    /// Recovered from the recoverable-message store: deleted but still on-device,
    /// with the deletion time (Unix) when known.
    pub deleted: bool,
    pub deleted_at: Option<i64>,
    pub attachments: Vec<Attachment>,
}

/// One message in the cross-conversation timeline: a message plus the thread it
/// belongs to, so the flat stream can label each row with its conversation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineMessage {
    pub thread_id: i64,
    pub thread_title: String,
    /// The other party's resolvable handle (the thread's `display_name`), so the
    /// timeline shows the conversation partner even on your own outgoing messages
    /// (where `message.sender` is you). Falls back to the identifier if unknown.
    pub thread_handle: String,
    pub service: Option<String>,
    pub message: Message,
}

/// A half-open time window `[lo, hi)` in epoch seconds; either bound may be open
/// (`None`). Used to bucket messages by recency for the periods view.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
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
                t.last_message_at, t.message_count, t.participants_json, t.is_group,
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
            is_group: r.get::<_, i64>(7)? != 0,
            snippet: r.get(8)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Total number of messages in a thread. Cheap; drives the virtual scroller so
/// the UI can lazily fetch only the windows it renders.
pub fn count_messages(
    cache: &CacheDb,
    thread_id: i64,
    kind: Option<&str>,
    search: Option<&str>,
    unsafe_only: bool,
    ranges: &[TimeRange],
) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        &format!(
            "SELECT COUNT(*) FROM messages m
             WHERE m.thread_id = ?1 AND (?2 IS NULL OR m.kind = ?2)
               AND {ranges}
               AND (?3 IS NULL OR m.body LIKE '%' || ?3 || '%' ESCAPE '\\'
                              OR m.sender LIKE '%' || ?3 || '%' ESCAPE '\\')
               AND {marked}",
            ranges = sent_at_in_ranges("?4"),
            marked = marked_clause(unsafe_only, "m")
        ),
        rusqlite::params![thread_id, kind, search, filter_json(ranges)],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// The mark filter as a `WHERE` term, or a true constant when it is off.
///
/// A term rather than a conditionally-appended clause so every query reads the
/// same whether the filter is on or not — the counting query and the windowing
/// query must agree exactly, or the list shows a different number of rows than
/// the header claims.
fn marked_clause(unsafe_only: bool, alias: &str) -> String {
    if unsafe_only {
        crate::marks::marked_predicate(crate::marks::MarkKind::Message, alias)
    } else {
        "1".to_string()
    }
}

/// A window of a thread's messages, oldest first, each with its attachments.
/// `offset` counts from the oldest message. Threads can hold tens of thousands
/// of messages, so the UI never loads a whole thread — it requests the slices
/// it is about to display.
// One more than clippy's ceiling, and each is load-bearing: which thread, which
// window, which content kind, which direction, which search, and whether the
// person's own mark filter is on.
#[allow(clippy::too_many_arguments)]
pub fn get_message_window(
    cache: &CacheDb,
    thread_id: i64,
    offset: i64,
    limit: i64,
    kind: Option<&str>,
    desc: bool,
    search: Option<&str>,
    unsafe_only: bool,
    ranges: &[TimeRange],
) -> Result<Vec<Message>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    // Direction is a fixed keyword chosen here, never interpolated user input.
    let dir = if desc { "DESC" } else { "ASC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT m.id, m.is_from_me, m.sender, m.body, m.sent_at, m.read_at, m.delivered_at, m.reactions, m.reply_to_snippet, m.edited, m.kind, m.effect, m.deleted, m.deleted_at
         FROM messages m
         WHERE m.thread_id = ?1 AND (?4 IS NULL OR m.kind = ?4)
           AND (?5 IS NULL OR m.body LIKE '%' || ?5 || '%' ESCAPE '\\'
                          OR m.sender LIKE '%' || ?5 || '%' ESCAPE '\\')
           AND {marked}
           AND {ranges}
         ORDER BY m.sent_at {dir}, m.id {dir}
         LIMIT ?2 OFFSET ?3",
        marked = marked_clause(unsafe_only, "m"),
        ranges = sent_at_in_ranges("?6"),
    ))?;
    let mut messages = stmt
        .query_map(
            rusqlite::params![thread_id, limit, offset, kind, search, filter_json(ranges)],
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

/// The 0-based position of `message_id` within a thread under the same ordering
/// as [`get_message_window`] (`ORDER BY sent_at, id`, ascending or descending)
/// and the same optional `kind` filter. Returns `None` if the message isn't in
/// the thread (or is excluded by the filter's ordering key). Used to scroll a
/// conversation to a specific message (e.g. a Timeline row the user tapped).
pub fn message_row_index(
    cache: &CacheDb,
    thread_id: i64,
    message_id: i64,
    kind: Option<&str>,
    desc: bool,
) -> Result<Option<i64>> {
    let conn = cache.conn();
    // The target's sort key (sent_at, id). Scoped to the thread so a stray id
    // from another conversation can't match.
    let Some((sent_at, id)) = conn
        .query_row(
            "SELECT sent_at, id FROM messages WHERE id = ?1 AND thread_id = ?2",
            rusqlite::params![message_id, thread_id],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    // Count rows that sort before the target. The row-value comparison mirrors
    // the `ORDER BY sent_at DIR, id DIR` tuple ordering; `>` for descending.
    // NOTE: SQLite yields NULL (not true) for `(sent_at, id) < (…)` when either
    // side's sent_at is NULL, whereas ORDER BY sorts NULLs first — so a thread
    // containing NULL-dated messages could return an index off by the NULL count.
    // Benign in practice: every imported message has a sent_at, and jump targets
    // are always dated timeline rows. Revisit if a source ever yields NULL dates.
    let cmp = if desc { ">" } else { "<" };
    let idx: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM messages
             WHERE thread_id = ?1 AND (?2 IS NULL OR kind = ?2)
               AND (sent_at, id) {cmp} (?3, ?4)",
        ),
        rusqlite::params![thread_id, kind, sent_at, id],
        |r| r.get(0),
    )?;
    Ok(Some(idx))
}

/// All messages in a thread, oldest first, each with its attachments. Used by
/// tests and small callers; large threads should use [`get_message_window`].
pub fn get_messages(cache: &CacheDb, thread_id: i64) -> Result<Vec<Message>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, is_from_me, sender, body, sent_at, read_at, delivered_at, reactions, reply_to_snippet, edited, kind, effect, deleted, deleted_at
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
        kind: r.get(10)?,
        effect: r.get(11)?,
        deleted: r.get::<_, i64>(12)? != 0,
        deleted_at: r.get(13)?,
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

/// Best-effort recovery of a message attachment that's missing from the backup:
/// a camera-roll item with the same file name (the Messages copy was offloaded to
/// iCloud, but the original is still in Photos). Returns `(media_id, kind)`, or
/// `None`. Ambiguous same-name matches are broken by the closest capture time to
/// the message. Name matching can be wrong — especially for *received* files,
/// whose `IMG_####` counter can collide with one of your own photos — so callers
/// gate this behind a user setting and label the result as recovered.
pub fn recover_attachment_media(
    cache: &CacheDb,
    attachment_id: i64,
) -> Result<Option<(i64, String)>> {
    cache
        .conn()
        .query_row(
            "SELECT mi.id, mi.kind
             FROM attachments a
             JOIN messages m ON m.id = a.message_id
             JOIN media_items mi
               ON mi.local_path IS NOT NULL
              AND (mi.relative_path = a.filename
                   -- exact trailing slash+filename match. LIKE is wrong here:
                   -- filenames like IMG_0001.HEIC contain an underscore, a LIKE
                   -- wildcard, so IMG_0001 would also match e.g. IMGX0001. substr
                   -- on the last length(filename)+1 chars compares literally.
                   OR substr('/' || mi.relative_path, -(length(a.filename) + 1))
                        = '/' || a.filename)
             WHERE a.id = ?1 AND a.filename IS NOT NULL AND a.local_path IS NULL
             ORDER BY ABS(COALESCE(mi.taken_at, 0) - COALESCE(m.sent_at, 0))
             LIMIT 1",
            [attachment_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(Into::into)
}

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
    unsafe_only: bool,
    ranges: &[TimeRange],
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
            &format!(
                "SELECT COUNT(*) FROM messages m
                 WHERE m.sent_at IS NOT NULL AND (?1 IS NULL OR m.kind = ?1)
                   AND {marked} AND {ranges}",
                marked = marked_clause(unsafe_only, "m"),
                ranges = sent_at_in_ranges("?2"),
            ),
            rusqlite::params![kind, filter_json(ranges)],
            |r| r.get(0),
        )?
    } else {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
                 WHERE m.sent_at IS NOT NULL
                   AND (?1 IS NULL OR t.service = ?1)
                   AND (?3 IS NULL OR m.kind = ?3)
                   AND {marked} AND {ranges}
                   AND (?2 IS NULL OR m.body LIKE '%' || ?2 || '%' ESCAPE '\\'
                                  OR m.sender LIKE '%' || ?2 || '%' ESCAPE '\\'
                                  OR t.display_name LIKE '%' || ?2 || '%' ESCAPE '\\'
                                  OR t.identifier LIKE '%' || ?2 || '%' ESCAPE '\\')",
                marked = marked_clause(unsafe_only, "m"),
                ranges = sent_at_in_ranges("?4"),
            ),
            rusqlite::params![service, search, kind, filter_json(ranges)],
            |r| r.get(0),
        )?
    };
    Ok(n)
}

/// A window of the cross-conversation timeline: every message from every thread,
/// oldest first, sliced by `offset`. `service` filters by source app (None=all).
#[allow(clippy::too_many_arguments)] // every one is a filter the toolbar shows
pub fn get_timeline_window(
    cache: &CacheDb,
    offset: i64,
    limit: i64,
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
    desc: bool,
    unsafe_only: bool,
) -> Result<Vec<TimelineMessage>> {
    range_window(
        cache,
        &[],
        offset,
        limit,
        service,
        search,
        kind,
        desc,
        unsafe_only,
    )
}

/// Message counts for each of the given time windows. Powers the periods view's
/// bucket list (e.g. "Last 7 days: 812"). One row per range, order preserved.
/// `service` filters by source app (None = all).
/// SQL matching a JSON array of {lo,hi} ranges against `m.sent_at`.
///
/// A message matches if it falls in ANY selected period — a union, because the
/// filter is multi-select everywhere in this app and "2023 or 2025" has to mean
/// both, not neither. `'[]'` means no restriction. Undated messages match no
/// range, so they appear only under "all time", which is the same rule the
/// gallery's ranges use.
pub(crate) fn sent_at_in_ranges(param: &str) -> String {
    format!(
        "({param} = '[]' OR EXISTS (SELECT 1 FROM json_each({param}) tr
            WHERE (json_extract(tr.value, '$.lo') IS NULL OR m.sent_at >= json_extract(tr.value, '$.lo'))
              AND (json_extract(tr.value, '$.hi') IS NULL OR m.sent_at <  json_extract(tr.value, '$.hi'))))"
    )
}

/// The conversations with at least one message in ANY of `ranges`.
///
/// "Active in this period", not "last spoke in this period" — a thread whose
/// most recent message is today was still active in 2023 if it has messages
/// there, and filtering on `threads.last_message_at` would hide exactly the long
/// conversations someone is most likely to be looking for.
pub fn threads_in_ranges(cache: &CacheDb, ranges: &[TimeRange]) -> Result<Vec<i64>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT m.thread_id FROM messages m
          WHERE m.sent_at IS NOT NULL AND {}",
        sent_at_in_ranges("?1")
    ))?;
    let rows = stmt.query_map([filter_json(ranges)], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The earliest and latest dated message (Unix seconds), or `None` when there are
/// no dated messages. Drives the Timeline's per-year quick filters.
pub fn message_date_bounds(cache: &CacheDb) -> Result<Option<(i64, i64)>> {
    let (lo, hi): (Option<i64>, Option<i64>) = cache.conn().query_row(
        "SELECT MIN(sent_at), MAX(sent_at) FROM messages WHERE sent_at IS NOT NULL",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(lo.zip(hi))
}

/// The earliest and latest CAPTURE date across the gallery, so the Photos time
/// filter can offer one chip per year the library actually spans instead of just
/// the current calendar year. `None` when nothing has a date.
pub fn media_date_bounds(cache: &CacheDb) -> Result<Option<(i64, i64)>> {
    let (lo, hi): (Option<i64>, Option<i64>) = cache.conn().query_row(
        "SELECT MIN(taken_at), MAX(taken_at) FROM media_items WHERE taken_at IS NOT NULL",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(lo.zip(hi))
}

pub fn count_message_ranges(
    cache: &CacheDb,
    ranges: &[TimeRange],
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
    unsafe_only: bool,
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
        let mut stmt = conn.prepare(&format!(
            "SELECT COUNT(*) FROM messages m
             WHERE m.sent_at IS NOT NULL AND (?3 IS NULL OR m.kind = ?3)
               AND (?1 IS NULL OR m.sent_at >= ?1) AND (?2 IS NULL OR m.sent_at < ?2)
               AND {}",
            marked_clause(unsafe_only, "m")
        ))?;
        for r in ranges {
            out.push(stmt.query_row(rusqlite::params![r.lo, r.hi, kind], |row| row.get(0))?);
        }
    } else {
        let mut stmt = conn.prepare(&format!(
            "SELECT COUNT(*) FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.sent_at IS NOT NULL
               AND {marked}
               AND (?1 IS NULL OR m.sent_at >= ?1)
               AND (?2 IS NULL OR m.sent_at < ?2)
               AND (?3 IS NULL OR t.service = ?3)
               AND (?5 IS NULL OR m.kind = ?5)
               AND (?4 IS NULL OR m.body LIKE '%' || ?4 || '%' ESCAPE '\\'
                              OR m.sender LIKE '%' || ?4 || '%' ESCAPE '\\'
                              OR t.display_name LIKE '%' || ?4 || '%' ESCAPE '\\'
                              OR t.identifier LIKE '%' || ?4 || '%' ESCAPE '\\')",
            marked = marked_clause(unsafe_only, "m")
        ))?;
        for r in ranges {
            out.push(stmt.query_row(
                rusqlite::params![r.lo, r.hi, service, search, kind],
                |row| row.get(0),
            )?);
        }
    }
    Ok(out)
}

/// Notes per time window, dated by last-modified (falling back to created).
/// Counts every note the Notes view shows — so a Safety-Scan filter count and
/// the Notes view agree for the same period. Undated notes are excluded from
/// bounded and open ranges alike (like [`count_message_ranges`]).
pub fn count_note_ranges(cache: &CacheDb, ranges: &[TimeRange]) -> Result<Vec<i64>> {
    let conn = cache.conn();
    let mut out = Vec::with_capacity(ranges.len());
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM notes
         WHERE COALESCE(modified_at, created_at) IS NOT NULL
           AND (?1 IS NULL OR COALESCE(modified_at, created_at) >= ?1)
           AND (?2 IS NULL OR COALESCE(modified_at, created_at) < ?2)",
    )?;
    for r in ranges {
        out.push(stmt.query_row(rusqlite::params![r.lo, r.hi], |row| row.get(0))?);
    }
    Ok(out)
}

/// A window of every message whose timestamp falls in `range`, oldest first,
/// across all conversations. Backs a selected period bucket.
#[allow(clippy::too_many_arguments)]
pub fn get_range_window(
    cache: &CacheDb,
    ranges: &[TimeRange],
    offset: i64,
    limit: i64,
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
    desc: bool,
    unsafe_only: bool,
) -> Result<Vec<TimelineMessage>> {
    range_window(
        cache,
        ranges,
        offset,
        limit,
        service,
        search,
        kind,
        desc,
        unsafe_only,
    )
}

/// Shared implementation: messages in `range` (open bounds allowed) and optional
/// `service`, joined to their thread for labeling, with attachments, ordered
/// chronologically.
#[allow(clippy::too_many_arguments)] // every one is a filter the toolbar shows
fn range_window(
    cache: &CacheDb,
    ranges: &[TimeRange],
    offset: i64,
    limit: i64,
    service: Option<&str>,
    search: Option<&str>,
    kind: Option<&str>,
    desc: bool,
    unsafe_only: bool,
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
           AND {ranges}
           AND (?5 IS NULL OR t.service = ?5)
           AND (?7 IS NULL OR m.kind = ?7)
           AND (?6 IS NULL OR m.body LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR m.sender LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR t.display_name LIKE '%' || ?6 || '%' ESCAPE '\\'
                          OR t.identifier LIKE '%' || ?6 || '%' ESCAPE '\\')
           AND {marked}
         ORDER BY m.sent_at {dir}, m.id {dir}
         LIMIT ?3 OFFSET ?4",
        marked = marked_clause(unsafe_only, "m"),
        ranges = sent_at_in_ranges("?1"),
    ))?;
    let mut items = stmt
        .query_map(
            rusqlite::params![
                filter_json(ranges),
                Option::<i64>::None, // ?2 is unused now that ranges are one array
                limit,
                offset,
                service,
                search,
                kind
            ],
            |r| {
                let display_name: Option<String> = r.get(6)?;
                let identifier: String = r.get(7)?;
                // iLEAPP stores the chat ROWID in `identifier` and the real
                // contact handle in `display_name`, so both the label and the
                // resolvable handle come from `display_name` (identifier is only
                // a last resort). Using the ROWID as the handle left outgoing
                // rows unresolvable — they fell back to a bare "#".
                let handle = display_name
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| identifier.clone());
                Ok(TimelineMessage {
                    thread_id: r.get(5)?,
                    thread_title: handle.clone(),
                    thread_handle: handle,
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
                        kind: None,
                        effect: None,
                        deleted: false,
                        deleted_at: None,
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
    /// The number's ISO country code (lowercase alpha-2, e.g. "se"), or None.
    pub country_code: Option<String>,
}

/// Calls, most recent first.
pub fn list_calls(cache: &CacheDb) -> Result<Vec<Call>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, address, direction, answered, duration_s, occurred_at, service, call_type, location, country_code
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
        country_code: r.get(9)?,
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
    /// Safari profile the visit belongs to (iOS 17+): `Default` for the main
    /// history, otherwise the profile's name. NULL on pre-v51 imports.
    pub profile: Option<String>,
    /// The visit happened on another iCloud-synced device, not this one.
    pub synced: bool,
    /// URL that redirected *to* this visit, when Safari recorded a chain.
    pub redirect_source: Option<String>,
    /// URL this visit redirected *to*.
    pub redirect_destination: Option<String>,
}

/// Safari history, most recent first.
pub fn list_safari_history(cache: &CacheDb) -> Result<Vec<HistoryVisit>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, url, title, visited_at, visit_count, deleted,
                profile, synced, redirect_source, redirect_destination
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
        profile: r.get(6)?,
        synced: r.get::<_, i64>(7)? != 0,
        redirect_source: r.get(8)?,
        redirect_destination: r.get(9)?,
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
    /// Related people: label = relationship (Mother / custom), value = name.
    pub related: Vec<crate::parsers::address_book::LabeledValue>,
    /// Names of the address-book groups this contact belongs to.
    pub groups: Vec<String>,
    /// Social / IM profiles: label = service, value = username.
    pub social: Vec<crate::parsers::address_book::LabeledValue>,
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
    /// A GPS route was recorded (rows in `workout_routes`).
    pub has_route: bool,
}

/// A digest of the Health store's raw-sample volume (from the `meta` table).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    pub sample_count: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub workout_count: i64,
    /// Days with activity aggregates / sleep sessions / recorded timezones —
    /// lets the view show section counts without materializing the lists.
    pub day_count: i64,
    pub sleep_count: i64,
    pub timezone_count: i64,
    pub achievement_count: i64,
    pub cycle_count: i64,
}

/// Workouts, most recent first.
pub fn list_workouts(cache: &CacheDb) -> Result<Vec<Workout>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, activity, start_at, end_at, duration_s, distance_m,
                EXISTS(SELECT 1 FROM workout_routes r WHERE r.workout_id = workouts.id)
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
            has_route: r.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One point of a workout's GPS route.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoutePoint {
    pub at: Option<i64>,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
}

/// The (downsampled) GPS route of one workout, in recording order.
pub fn workout_route(cache: &CacheDb, workout_id: i64) -> Result<Vec<RoutePoint>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT at, latitude, longitude, altitude
         FROM workout_routes WHERE workout_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([workout_id], |r| {
        Ok(RoutePoint {
            at: r.get(0)?,
            latitude: r.get(1)?,
            longitude: r.get(2)?,
            altitude: r.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One day of Health activity: the cumulative metrics summed over the (UTC)
/// day plus the heart-rate spread, pivoted from `health_daily` rows.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct HealthDay {
    /// Midnight UTC of the day, unix seconds (sortable/filterable timestamp).
    pub day_at: i64,
    pub steps: Option<i64>,
    pub distance_m: Option<f64>,
    pub flights: Option<i64>,
    pub active_kcal: Option<f64>,
    pub resting_kcal: Option<f64>,
    pub hr_min: Option<f64>,
    pub hr_avg: Option<f64>,
    pub hr_max: Option<f64>,
    /// Headphone audio exposure, loudest sample of the day (dB).
    pub audio_db_max: Option<f64>,
    /// Walking/mobility daily averages.
    pub walk_speed_ms: Option<f64>,
    pub step_length_m: Option<f64>,
    pub double_support_pct: Option<f64>,
    pub walk_asymmetry_pct: Option<f64>,
    /// Activity rings (NULL when the device never tracked that ring).
    pub move_kcal: Option<f64>,
    pub move_goal_kcal: Option<f64>,
    pub exercise_min: Option<f64>,
    pub exercise_goal_min: Option<f64>,
    pub stand_hours: Option<f64>,
    pub stand_goal_hours: Option<f64>,
}

/// Daily activity aggregates, most recent day first.
pub fn health_daily(cache: &CacheDb) -> Result<Vec<HealthDay>> {
    use crate::parsers::health::metric;
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT CAST(strftime('%s', day) AS INTEGER), metric,
                value_sum, value_min, value_max, value_avg
         FROM health_daily ORDER BY day DESC",
    )?;
    let mut out: Vec<HealthDay> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let day_at: i64 = r.get(0)?;
        let metric: String = r.get(1)?;
        let sum: Option<f64> = r.get(2)?;
        if out.last().map(|d| d.day_at) != Some(day_at) {
            out.push(HealthDay {
                day_at,
                ..Default::default()
            });
        }
        let d = out.last_mut().expect("pushed above");
        match metric.as_str() {
            metric::STEPS => d.steps = sum.map(|v| v.round() as i64),
            metric::DISTANCE_M => d.distance_m = sum,
            metric::FLIGHTS => d.flights = sum.map(|v| v.round() as i64),
            metric::ACTIVE_KCAL => d.active_kcal = sum,
            metric::RESTING_KCAL => d.resting_kcal = sum,
            metric::HEART_RATE_BPM => {
                d.hr_min = r.get(3)?;
                d.hr_max = r.get(4)?;
                d.hr_avg = r.get(5)?;
            }
            metric::AUDIO_DB => d.audio_db_max = r.get(4)?,
            metric::WALK_SPEED_MS => d.walk_speed_ms = r.get(5)?,
            metric::STEP_LENGTH_M => d.step_length_m = r.get(5)?,
            metric::DOUBLE_SUPPORT_PCT => d.double_support_pct = r.get(5)?,
            metric::WALK_ASYMMETRY_PCT => d.walk_asymmetry_pct = r.get(5)?,
            _ => {}
        }
    }

    // Merge in the activity rings; a day tracked only by the rings (no
    // quantity samples) still gets a row.
    let mut stmt = conn.prepare(
        "SELECT CAST(strftime('%s', day) AS INTEGER),
                move_kcal, move_goal_kcal, exercise_min, exercise_goal_min,
                stand_hours, stand_goal_hours
         FROM activity_rings",
    )?;
    let mut idx: std::collections::HashMap<i64, usize> =
        out.iter().enumerate().map(|(i, d)| (d.day_at, i)).collect();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let day_at: i64 = r.get(0)?;
        let i = *idx.entry(day_at).or_insert_with(|| {
            out.push(HealthDay {
                day_at,
                ..Default::default()
            });
            out.len() - 1
        });
        let d = &mut out[i];
        d.move_kcal = r.get(1)?;
        d.move_goal_kcal = r.get(2)?;
        d.exercise_min = r.get(3)?;
        d.exercise_goal_min = r.get(4)?;
        d.stand_hours = r.get(5)?;
        d.stand_goal_hours = r.get(6)?;
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.day_at));
    Ok(out)
}

/// One Cycle Tracking entry (a reproductive-health / symptom category sample).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CycleEntry {
    pub id: i64,
    pub category: String,
    /// Decoded value (e.g. menstrual-flow "Medium"), or None.
    pub detail: Option<String>,
    pub logged_at: Option<i64>,
}

/// Cycle Tracking entries, most recent first.
pub fn list_cycle(cache: &CacheDb) -> Result<Vec<CycleEntry>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, category, detail, logged_at
         FROM cycle_tracking ORDER BY logged_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(CycleEntry {
            id: r.get(0)?,
            category: r.get(1)?,
            detail: r.get(2)?,
            logged_at: r.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One earned Apple Fitness achievement.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthAchievement {
    pub id: i64,
    /// Template id, e.g. "MoveGoal200Percent" (humanized in the UI).
    pub name: String,
    /// Midnight UTC of the earned day, unix seconds.
    pub earned_at: Option<i64>,
    pub value: Option<f64>,
    pub unit: Option<String>,
}

/// Earned achievements, most recent first.
pub fn list_health_achievements(cache: &CacheDb) -> Result<Vec<HealthAchievement>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, name, CAST(strftime('%s', earned_on) AS INTEGER), value, unit
         FROM health_achievements
         ORDER BY earned_on DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(HealthAchievement {
            id: r.get(0)?,
            name: r.get(1)?,
            earned_at: r.get(2)?,
            value: r.get(3)?,
            unit: r.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One timezone Health samples were recorded in — a travel-timeline entry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthTimezone {
    /// IANA name, e.g. "Europe/Stockholm".
    pub tz_name: String,
    /// Device product types that recorded there (e.g. "iPhone12,8").
    pub devices: Vec<String>,
    pub samples: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
}

/// Timezones Health data was recorded in, most samples first.
pub fn list_health_timezones(cache: &CacheDb) -> Result<Vec<HealthTimezone>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT tz_name, GROUP_CONCAT(DISTINCT device), SUM(samples),
                MIN(first_at), MAX(last_at)
         FROM health_timezones
         GROUP BY tz_name ORDER BY SUM(samples) DESC, tz_name",
    )?;
    let rows = stmt.query_map([], |r| {
        let devices: Option<String> = r.get(1)?;
        Ok(HealthTimezone {
            tz_name: r.get(0)?,
            devices: devices
                .unwrap_or_default()
                .split(',')
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .collect(),
            samples: r.get(2)?,
            first_at: r.get(3)?,
            last_at: r.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One sleep-analysis session (a raw HealthKit category sample).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SleepSession {
    pub id: i64,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub stage: String,
}

/// Sleep sessions, most recent first.
pub fn list_sleep(cache: &CacheDb) -> Result<Vec<SleepSession>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, start_at, end_at, stage
         FROM sleep_sessions ORDER BY start_at DESC NULLS LAST, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SleepSession {
            id: r.get(0)?,
            start_at: r.get(1)?,
            end_at: r.get(2)?,
            stage: r.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The Health summary (sample count + date range + workout count), or a zeroed
/// summary when no Health data was imported.
pub fn health_summary(cache: &CacheDb) -> Result<HealthSummary> {
    let meta_i = |k: &str| -> Option<i64> { cache.get_meta(k).ok().flatten()?.parse().ok() };
    let count = |sql: &str| -> Result<i64> { Ok(cache.conn().query_row(sql, [], |r| r.get(0))?) };
    Ok(HealthSummary {
        sample_count: meta_i("health_sample_count").unwrap_or(0),
        first_at: meta_i("health_first_at"),
        last_at: meta_i("health_last_at"),
        workout_count: count("SELECT COUNT(*) FROM workouts")?,
        day_count: count("SELECT COUNT(DISTINCT day) FROM health_daily")?,
        sleep_count: count("SELECT COUNT(*) FROM sleep_sessions")?,
        timezone_count: count("SELECT COUNT(DISTINCT tz_name) FROM health_timezones")?,
        achievement_count: count("SELECT COUNT(*) FROM health_achievements")?,
        cycle_count: count("SELECT COUNT(*) FROM cycle_tracking")?,
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
                middle_name, nickname, job_title, department, birthday_at, note, addresses_json,
                related_json, groups_json, social_json
         FROM contacts
         ORDER BY last_name IS NULL AND first_name IS NULL,
                  last_name COLLATE NOCASE, first_name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |r| {
        let phones: String = r.get(4)?;
        let emails: String = r.get(5)?;
        let addresses: String = r.get(14)?;
        let related: String = r.get(15)?;
        let groups: String = r.get(16)?;
        let social: String = r.get(17)?;
        Ok(Contact {
            id: r.get(0)?,
            first_name: r.get(1)?,
            last_name: r.get(2)?,
            organization: r.get(3)?,
            phones: serde_json::from_str(&phones).unwrap_or_default(),
            emails: serde_json::from_str(&emails).unwrap_or_default(),
            addresses: serde_json::from_str(&addresses).unwrap_or_default(),
            related: serde_json::from_str(&related).unwrap_or_default(),
            groups: serde_json::from_str(&groups).unwrap_or_default(),
            social: serde_json::from_str(&social).unwrap_or_default(),
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
    /// "original" | "thumbnail" | "metadata": how much of the asset this backup
    /// holds. A "thumbnail" row is a real photo shown at thumbnail resolution
    /// because its full-size original stayed in iCloud, and the grid has to say
    /// so rather than let it pass as a full-resolution image.
    pub availability: String,
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
    /// In Recently Deleted (surfaced as a badge, not excluded), with the deletion
    /// timestamp when known.
    pub trashed: bool,
    pub trashed_at: Option<i64>,
    /// When the asset was added to the library (Unix), which differs from capture
    /// for received/saved/imported media, or None.
    pub added_at: Option<i64>,
    /// A caption someone wrote on this photo in a SHARED ALBUM, and how many
    /// people liked it there — activity by others on a photo this device put in
    /// front of them. `None` when the photo was never shared.
    pub shared_caption: Option<String>,
    pub shared_likes: Option<i64>,
    /// Media subtype ("screenshot" | "panorama"), or None.
    pub subtype: Option<String>,
    /// The USER's own star, set in this app (distinct from `favorite`, the
    /// device's Photos.sqlite flag).
    pub user_favorite: bool,
}

/// Media items that have materialized bytes, newest first. Only items with a
/// `local_path` on disk are listed — the gallery can't show what isn't there.
pub fn list_media(cache: &CacheDb) -> Result<Vec<MediaItem>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT id, kind, source, mime_type, relative_path, taken_at, persons,
                latitude, longitude, is_favorite, location, albums,
                width, height, duration_s, file_size, camera, lens, exif, hidden, subtype,
                trashed, trashed_at, added_at, shared_caption, shared_likes, user_favorite,
                availability
         FROM media_items
         WHERE {HAS_PIXELS}
         ORDER BY taken_at DESC NULLS LAST, id DESC"
    ))?;
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
        trashed: r.get::<_, i64>(21)? != 0,
        trashed_at: r.get(22)?,
        added_at: r.get(23)?,
        shared_caption: r.get(24)?,
        shared_likes: r.get(25)?,
        user_favorite: r.get::<_, i64>(26)? != 0,
        availability: r
            .get::<_, Option<String>>(27)?
            .unwrap_or_else(|| "original".into()),
    })
}

// --- Windowed, filterable list queries -------------------------------------
// Each artifact list has a `count_*` and `get_*_window` pair so the UI can
// virtualize/lazy-load huge lists (a large camera roll, years of history) the
// same way Messages does — fetching only the visible slice. Filtering/search
// happens in SQL so the count and the windows stay consistent. A NULL filter
// matches everything.

/// SQL for "this row has something to display".
///
/// It used to be `local_path IS NOT NULL` — bytes on disk or nothing. That was
/// right while the camera roll only ever enumerated files, but with iCloud
/// Photos on most assets have no original in the backup and only a thumbnail,
/// and the old predicate hid every one of them. A message attachment with
/// neither is still excluded, which is what that rule was really protecting.
///
/// Shared by every gallery query so the count, the windows and the facets can
/// never disagree about which rows exist.
const HAS_PIXELS: &str = "(local_path IS NOT NULL OR thumb_path IS NOT NULL)";

/// SQL matching a JSON array of source labels bound at `?1`; `'[]'` = no source
/// restriction. `COALESCE(source,'Other')` so the synthesized "Other" bucket
/// (NULL source) is selectable, matching `media_sources`' label.
const SOURCE_IN: &str =
    "(?1 = '[]' OR COALESCE(source, 'Other') IN (SELECT value FROM json_each(?1)))";

/// SQL matching a JSON array of availability values ('original' | 'thumbnail')
/// bound at `?8`; `'[]'` = no restriction. Multi-select like every other facet,
/// so "show me the ones whose originals are missing" is one click.
const AVAIL_IN: &str =
    "(?8 = '[]' OR COALESCE(availability, 'original') IN (SELECT value FROM json_each(?8)))";

/// SQL matching a JSON array of {lo,hi} time ranges bound at `?2`; `'[]'` = no
/// time restriction. An item matches if it falls in ANY selected range (union);
/// a null bound is open on that end. Undated media (`taken_at` NULL) match no
/// range, so they only appear under "all time" — the same rule the old single
/// range had.
const RANGES_IN: &str = "(?2 = '[]' OR EXISTS (SELECT 1 FROM json_each(?2) tr
        WHERE (json_extract(tr.value, '$.lo') IS NULL OR taken_at >= json_extract(tr.value, '$.lo'))
          AND (json_extract(tr.value, '$.hi') IS NULL OR taken_at <  json_extract(tr.value, '$.hi'))))";

/// JSON-encode a filter list for the `json_each` clauses above. `[]` on failure.
fn filter_json<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

/// Photos/videos in `source` ("Photos", "Messages", …), or all when NULL, whose
/// `taken_at` falls in `range` (open bounds = no limit; undated media only count
/// when both bounds are open).
pub fn count_media(
    cache: &CacheDb,
    sources: &[String],
    ranges: &[TimeRange],
    search: Option<&str>,
    favorites_only: bool,
    hidden_only: bool,
    availability: &[String],
) -> Result<i64> {
    let search = search.map(escape_like);
    let sql = format!(
        "SELECT COUNT(*) FROM media_items
         WHERE {HAS_PIXELS}
           AND {SOURCE_IN}
           AND {RANGES_IN}
           AND {AVAIL_IN}
           AND (?4 = 0 OR user_favorite = 1)
           AND (?5 = 0 OR hidden = 1)
           AND (?3 IS NULL OR relative_path LIKE '%' || ?3 || '%' ESCAPE '\\'
                          OR persons LIKE '%' || ?3 || '%' ESCAPE '\\'
                          OR location LIKE '%' || ?3 || '%' ESCAPE '\\'
                          OR albums LIKE '%' || ?3 || '%' ESCAPE '\\')"
    );
    let n = cache.conn().query_row(
        &sql,
        rusqlite::params![
            filter_json(sources),
            filter_json(ranges),
            search,
            favorites_only as i64,
            hidden_only as i64,
            "",
            "",
            filter_json(availability)
        ],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Media counts for each preset `range` in `sources` (respecting `search`) —
/// powers the Photos time-filter chips. One row per range, order preserved. Each
/// chip is counted against its own single range (not the current selection).
pub fn count_media_ranges(
    cache: &CacheDb,
    sources: &[String],
    ranges: &[TimeRange],
    search: Option<&str>,
    favorites_only: bool,
    hidden_only: bool,
    availability: &[String],
) -> Result<Vec<i64>> {
    let search = search.map(escape_like);
    let conn = cache.conn();
    // `?1` is the sources JSON (see SOURCE_IN); the per-chip range is a single
    // `?2`/`?3` pair, so RANGES_IN is not used here.
    let sql = format!(
        "SELECT COUNT(*) FROM media_items
         WHERE {HAS_PIXELS}
           AND {SOURCE_IN}
           AND {AVAIL_IN}
           AND (?2 IS NULL OR taken_at >= ?2)
           AND (?3 IS NULL OR taken_at < ?3)
           AND (?5 = 0 OR user_favorite = 1)
           AND (?6 = 0 OR hidden = 1)
           AND (?4 IS NULL OR relative_path LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR persons LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR location LIKE '%' || ?4 || '%' ESCAPE '\\'
                          OR albums LIKE '%' || ?4 || '%' ESCAPE '\\')"
    );
    let sources_json = filter_json(sources);
    let availability_json = filter_json(availability);
    let mut stmt = conn.prepare(&sql)?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(stmt.query_row(
            rusqlite::params![
                sources_json,
                r.lo,
                r.hi,
                search,
                favorites_only as i64,
                hidden_only as i64,
                "",
                availability_json.as_str()
            ],
            |row| row.get(0),
        )?);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn get_media_window(
    cache: &CacheDb,
    sources: &[String],
    ranges: &[TimeRange],
    search: Option<&str>,
    offset: i64,
    limit: i64,
    sort: Sort,
    favorites_only: bool,
    hidden_only: bool,
    availability: &[String],
) -> Result<Vec<MediaItem>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let (dir, nulls) = sort.order_sql();
    // ?1 sources JSON, ?2 ranges JSON (see SOURCE_IN / RANGES_IN), then paging,
    // search and the favourite flag.
    let sql = format!(
        "SELECT id, kind, source, mime_type, relative_path, taken_at, persons,
                latitude, longitude, is_favorite, location, albums,
                width, height, duration_s, file_size, camera, lens, exif, hidden, subtype,
                trashed, trashed_at, added_at, shared_caption, shared_likes, user_favorite,
                availability
         FROM media_items
         WHERE {HAS_PIXELS}
           AND {SOURCE_IN}
           AND {RANGES_IN}
           AND {AVAIL_IN}
           AND (?6 = 0 OR user_favorite = 1)
           AND (?7 = 0 OR hidden = 1)
           AND (?5 IS NULL OR relative_path LIKE '%' || ?5 || '%' ESCAPE '\\'
                          OR persons LIKE '%' || ?5 || '%' ESCAPE '\\'
                          OR location LIKE '%' || ?5 || '%' ESCAPE '\\'
                          OR albums LIKE '%' || ?5 || '%' ESCAPE '\\')
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?3 OFFSET ?4",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![
            filter_json(sources),
            filter_json(ranges),
            limit,
            offset,
            search,
            favorites_only as i64,
            hidden_only as i64,
            filter_json(availability)
        ],
        row_to_media,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Toggle the user's star on one media item. Returns its `relative_path` (the
/// stable per-backup identity a re-import preserves), so the caller can persist
/// the star to the per-backup favorites file. `None` if the id isn't a media row.
pub fn set_user_favorite(cache: &CacheDb, id: i64, on: bool) -> Result<Option<String>> {
    let conn = cache.conn();
    let rel: Option<String> = conn
        .query_row(
            "SELECT relative_path FROM media_items WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    if rel.is_some() {
        conn.execute(
            "UPDATE media_items SET user_favorite = ?2 WHERE id = ?1",
            rusqlite::params![id, on as i64],
        )?;
    }
    Ok(rel)
}

/// Re-apply a set of user-starred `relative_path`s onto the (freshly rebuilt)
/// cache. Called after import and on open, because the favorites file — not the
/// cache — is the durable home of the star.
pub fn apply_user_favorites(cache: &CacheDb, paths: &[String]) -> Result<()> {
    let conn = cache.conn();
    conn.execute("UPDATE media_items SET user_favorite = 0", [])?;
    let json = serde_json::to_string(paths).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "UPDATE media_items SET user_favorite = 1
         WHERE relative_path IN (SELECT value FROM json_each(?1))",
        [json],
    )?;
    Ok(())
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
pub(crate) fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The distinct peer addresses in the call log.
///
/// Exists so the CLIENT can do the name matching (#279). Calls display resolved
/// contact names, but the log stores only addresses -- and matching a typed name
/// back to an address needs phone normalisation, which lives in exactly one
/// place: `use-contact-resolver.ts`, the same code that produced the name on
/// screen. Re-implementing it in SQL would be a second normalisation that can
/// disagree with the first, and a disagreement here shows the WRONG PERSON's
/// calls -- worse than not matching at all.
///
/// So: hand the client the addresses, let it resolve them with the resolver it
/// already trusts, and take back the subset that matched. The query then does a
/// plain `IN` over values that came out of this same column, which cannot
/// mis-normalise because it never normalises.
pub fn call_addresses(cache: &CacheDb) -> Result<Vec<String>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT address FROM calls WHERE address IS NOT NULL AND address <> ''",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// JSON array for the `IN` clause, or NULL when the client sent no matches.
fn address_json(addresses: Option<&[String]>) -> Option<String> {
    match addresses {
        Some(a) if !a.is_empty() => serde_json::to_string(a).ok(),
        _ => None,
    }
}

/// Calls whose address matches `search` (substring) or is one of `addresses`
/// (the client's name matches, see [`call_addresses`]); all when both are NULL.
pub fn count_calls(
    cache: &CacheDb,
    search: Option<&str>,
    range: TimeRange,
    addresses: Option<&[String]>,
) -> Result<i64> {
    let search = search.map(escape_like);
    let addr = address_json(addresses);
    let n = cache.conn().query_row(
        "SELECT COUNT(*) FROM calls
         WHERE (?1 IS NULL
                OR address LIKE '%' || ?1 || '%' ESCAPE '\\'
                OR address IN (SELECT value FROM json_each(COALESCE(?4, '[]'))))
           AND (?2 IS NULL OR occurred_at >= ?2)
           AND (?3 IS NULL OR occurred_at < ?3)",
        rusqlite::params![search, range.lo, range.hi, addr],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Call counts for each `range` (respecting `search`) — powers the time-filter chips.
pub fn count_call_ranges(
    cache: &CacheDb,
    ranges: &[TimeRange],
    search: Option<&str>,
    addresses: Option<&[String]>,
) -> Result<Vec<i64>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let addr = address_json(addresses);
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM calls
         WHERE (?1 IS NULL
                OR address LIKE '%' || ?1 || '%' ESCAPE '\\'
                OR address IN (SELECT value FROM json_each(COALESCE(?4, '[]'))))
           AND (?2 IS NULL OR occurred_at >= ?2)
           AND (?3 IS NULL OR occurred_at < ?3)",
    )?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(
            stmt.query_row(rusqlite::params![search, r.lo, r.hi, addr], |row| {
                row.get(0)
            })?,
        );
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
    addresses: Option<&[String]>,
) -> Result<Vec<Call>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let addr = address_json(addresses);
    // `sort.column()` is an allowlisted SQL fragment (never raw user input); see
    // the `Sort` type. `id` is the stable tiebreaker.
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, address, direction, answered, duration_s, occurred_at, service, call_type, location, country_code
         FROM calls
         WHERE (?1 IS NULL
                OR address LIKE '%' || ?1 || '%' ESCAPE '\\'
                OR address IN (SELECT value FROM json_each(COALESCE(?6, '[]'))))
           AND (?4 IS NULL OR occurred_at >= ?4)
           AND (?5 IS NULL OR occurred_at < ?5)
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?2 OFFSET ?3",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![search, limit, offset, range.lo, range.hi, addr],
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
        "SELECT id, url, title, visited_at, visit_count, deleted,
                profile, synced, redirect_source, redirect_destination
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
    /// An open tab in a private-browsing window (BrowserState.db). Always false
    /// for bookmarks / reading-list rows.
    pub private: bool,
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
        private: r.get::<_, i64>(8)? != 0,
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
        "SELECT id, kind, title, url, folder, date_added, date_viewed, preview_text, private
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

/// One Safari web search — a term recovered from a search-engine URL in history
/// (`source = "visited"`) or typed into the search field (`source = "typed"`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSearch {
    pub id: i64,
    pub term: String,
    pub searched_at: Option<i64>,
    /// `"visited"` (recovered from a history URL) or `"typed"` (RecentWebSearches).
    pub source: String,
    /// Search-engine host, when the term came from a URL.
    pub engine: Option<String>,
    /// The result-page URL, when the term came from a URL.
    pub url: Option<String>,
    /// Safari profile the search belongs to, when it came from history.
    pub profile: Option<String>,
}

fn row_to_search(r: &rusqlite::Row<'_>) -> rusqlite::Result<WebSearch> {
    Ok(WebSearch {
        id: r.get(0)?,
        term: r.get(1)?,
        searched_at: r.get(2)?,
        source: r.get(3)?,
        engine: r.get(4)?,
        url: r.get(5)?,
        profile: r.get(6)?,
    })
}

/// The `WHERE` shared by every Safari-search query, so the count, the chips and
/// the window can never disagree about what a filter means.
const SEARCH_FILTER: &str = "(?1 IS NULL OR term LIKE '%' || ?1 || '%' ESCAPE '\\'
                                        OR engine LIKE '%' || ?1 || '%' ESCAPE '\\')
           AND (?2 IS NULL OR searched_at >= ?2)
           AND (?3 IS NULL OR searched_at < ?3)";

pub fn count_safari_searches(
    cache: &CacheDb,
    search: Option<&str>,
    range: TimeRange,
) -> Result<i64> {
    let search = search.map(escape_like);
    let n = cache.conn().query_row(
        &format!("SELECT COUNT(*) FROM safari_searches WHERE {SEARCH_FILTER}"),
        rusqlite::params![search, range.lo, range.hi],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Per-range search counts (respecting `search`) — the time chips.
pub fn count_safari_search_ranges(
    cache: &CacheDb,
    search: Option<&str>,
    ranges: &[TimeRange],
) -> Result<Vec<i64>> {
    let search = search.map(escape_like);
    let conn = cache.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT COUNT(*) FROM safari_searches WHERE {SEARCH_FILTER}"
    ))?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        out.push(stmt.query_row(rusqlite::params![search, r.lo, r.hi], |row| row.get(0))?);
    }
    Ok(out)
}

/// A window of Safari searches matching `search` within `range`, ordered by
/// `sort` (an allowlisted column from the command layer).
pub fn get_safari_searches_window(
    cache: &CacheDb,
    search: Option<&str>,
    range: TimeRange,
    offset: i64,
    limit: i64,
    sort: Sort,
) -> Result<Vec<WebSearch>> {
    let conn = cache.conn();
    let search = search.map(escape_like);
    let (dir, nulls) = sort.order_sql();
    let sql = format!(
        "SELECT id, term, searched_at, source, engine, url, profile
         FROM safari_searches
         WHERE {SEARCH_FILTER}
         ORDER BY {} {dir} {nulls}, id {dir}
         LIMIT ?4 OFFSET ?5",
        sort.column(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![search, range.lo, range.hi, limit, offset],
        row_to_search,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One Apple device that contributed Health data to this phone.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceUse {
    /// `ProductType` as the store holds it (`iPhone12,1`) — an identifier, not a
    /// marketing name. The UI maps it via `device-names.ts`.
    pub model: String,
    /// OS build (`21D50`). None on the per-device rollup, which spans builds.
    pub os_build: Option<String>,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub samples: i64,
}

fn row_to_device_use(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceUse> {
    Ok(DeviceUse {
        model: r.get(0)?,
        os_build: r.get(1)?,
        first_at: r.get(2)?,
        last_at: r.get(3)?,
        samples: r.get(4)?,
    })
}

/// Every device that ever wrote Health data here, oldest first.
///
/// Rolled up across OS builds, so one row per device. Health survives migration
/// between phones, so this reaches back past devices the person no longer owns.
pub fn list_devices_used(cache: &CacheDb) -> Result<Vec<DeviceUse>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT model, NULL, MIN(first_at), MAX(last_at), SUM(samples)
         FROM health_device_use
         GROUP BY model
         ORDER BY MIN(first_at)",
    )?;
    let rows = stmt.query_map([], row_to_device_use)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One row per (device, OS build) — an upgrade timeline, oldest first.
///
/// Pairs whose window is zero-length are excluded: a single sample says the
/// device existed, which `list_devices_used` already reports, but it cannot date
/// an upgrade, and a row claiming a device ran a build "from T to T" invites
/// exactly that reading. The pair is still in the table, so the rollup above
/// keeps the device.
pub fn list_device_os_history(cache: &CacheDb) -> Result<Vec<DeviceUse>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT model, os_build, first_at, last_at, samples
         FROM health_device_use
         WHERE os_build IS NOT NULL
           AND first_at IS NOT NULL AND last_at IS NOT NULL
           AND first_at <> last_at
         ORDER BY first_at",
    )?;
    let rows = stmt.query_map([], row_to_device_use)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// What a backup can say about messages that are GONE — content no longer in
/// `sms.db` at all, as opposed to the recently-deleted ones that keep their row
/// and are flagged by `Message::deleted_at`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeletionEvidence {
    /// Deletions iOS recorded itself, in `sync_deleted_messages`.
    pub recorded: i64,
    /// ROWIDs that were allocated and have no row. `message` is AUTOINCREMENT,
    /// so SQLite never reissues one and a gap means a row existed.
    pub missing_rowids: i64,
    /// How many separate runs those missing ROWIDs fall into — two gaps of one
    /// is a different story from one gap of two.
    pub gaps: i64,
    /// Earliest and latest surviving message bracketing any gap, so the absence
    /// can be placed in time at all.
    pub first_gap_at: Option<i64>,
    pub last_gap_at: Option<i64>,
}

/// Evidence about deleted messages.
///
/// `recorded` and `missing_rowids` are NOT added together and must not be
/// presented as one total: on the validation device both describe the same two
/// deletions, so summing them would double the count. They are separate because
/// they fail differently — iOS prunes its sync record once every device has
/// caught up, while a ROWID gap survives that but says nothing about what was
/// deleted.
pub fn message_deletion_evidence(cache: &CacheDb) -> Result<DeletionEvidence> {
    let conn = cache.conn();
    let (recorded, gaps, missing, first, last) = conn.query_row(
        "SELECT
             COALESCE(SUM(source = 'recorded'), 0),
             COALESCE(SUM(source = 'gap'), 0),
             COALESCE(SUM(CASE WHEN source = 'gap' THEN missing END), 0),
             MIN(after_at),
             MAX(before_at)
         FROM message_deletions",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        },
    )?;
    Ok(DeletionEvidence {
        recorded,
        missing_rowids: missing,
        gaps,
        first_gap_at: first,
        last_gap_at: last,
    })
}

/// Availability values present, with a count each, for the gallery filter.
///
/// Only values that actually occur are returned, so a backup taken without
/// iCloud Photos shows a single "original" bucket and the filter stays out of
/// the way instead of offering an option that would match nothing.
pub fn media_availability(cache: &CacheDb) -> Result<Vec<(String, i64)>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT COALESCE(availability, 'original') AS a, COUNT(*) AS n
         FROM media_items
         WHERE {HAS_PIXELS}
         GROUP BY a
         ORDER BY n DESC, a"
    ))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Distinct media sources present, with a count each, for the gallery filter.
/// Ordered by count descending (biggest sources first).
pub fn media_sources(cache: &CacheDb) -> Result<Vec<(String, i64)>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT COALESCE(source, 'Other') AS s, COUNT(*) AS n
         FROM media_items
         WHERE {HAS_PIXELS}
         GROUP BY s
         ORDER BY n DESC, s"
    ))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// What the media protocol needs to serve one item:
/// `(local_path, mime, thumb_path, decrypt_key, plain_size, thumb_key,
/// thumb_size)`. Returns `None` if the id is unknown or has no pixels at all.
///
/// `local_path` is `None` for an asset whose original stayed in iCloud — it is
/// still servable from its thumbnail, so this is a normal state, not an error. `decrypt_key` is the
/// class-prefixed wrapped key for an encrypted backup's original (see
/// [`crate::crypto`]) and `plain_size` its real length (to trim CBC padding);
/// both are `None` when `local_path` is already plaintext.
pub type MediaBlob = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<Vec<u8>>,
    Option<i64>,
);

pub fn media_blob(cache: &CacheDb, id: i64) -> Result<Option<MediaBlob>> {
    Ok(cache
        .conn()
        .query_row(
            &format!(
                "SELECT local_path, mime_type, thumb_path, decrypt_key, plain_size,
                        thumb_key, thumb_size
                 FROM media_items
                 WHERE id = ?1 AND {HAS_PIXELS}"
            ),
            [id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()?)
}

/// One person a note is shared with, from its CloudKit share archive.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareParticipant {
    pub name: Option<String>,
    pub email: Option<String>,
    /// CloudKit's acceptance code, passed through rather than translated.
    pub status: Option<i64>,
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
    /// Rich HTML rendering of the body (headings/lists/checklists); `None` to fall
    /// back to `body`. Withheld for a locked note.
    pub body_rich: Option<String>,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    /// Pinned to the top of the Notes app.
    pub pinned: bool,
    /// Who the note is shared with, decoded from its CloudKit share archive.
    ///
    /// Empty when the note is not shared. The OWNER is in this list too — a
    /// share always names them alongside the people invited — so a one-entry
    /// list means "shared, and only the owner has been fetched", not "shared
    /// with one stranger".
    #[serde(default)]
    pub shared_with: Vec<ShareParticipant>,
    /// Password-protected: the body is withheld until unlocked with the password.
    pub locked: bool,
    /// The user's password hint, if the note stored one.
    pub password_hint: Option<String>,
    /// Rich-content indicators: has a checklist, and counts of embedded
    /// image/video attachments vs total attachments (tables, drawings, files…).
    pub has_checklist: bool,
    /// Image attachments the note *references* (from its metadata). These may
    /// not be present in the backup — Notes media is often stored in iCloud and
    /// not downloaded to the device, in which case none can be displayed.
    pub image_count: i64,
    /// Image attachments actually present in the backup (rows in `note_media`),
    /// i.e. the number the detail gallery can display. `<= image_count`.
    pub available_image_count: i64,
    pub attachment_count: i64,
    /// Hashtag tags on the note (iOS 15+); empty when none.
    pub tags: Vec<String>,
    /// Whether the note has a resolved first image (served as a list thumbnail).
    pub has_image: bool,
}

/// Notes, most-recently-modified first.
pub fn list_notes(cache: &CacheDb) -> Result<Vec<Note>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT id, folder, title, snippet, body_html, created_at, modified_at, locked, password_hint, pinned,
                has_checklist, image_count, attachment_count, tags, image_local_path IS NOT NULL, body_rich,
                (SELECT COUNT(*) FROM note_media WHERE note_media.note_id = notes.id),
                shared_with_json
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
            body_rich: r.get(15)?,
            created_at: r.get(5)?,
            modified_at: r.get(6)?,
            locked: r.get::<_, i64>(7)? != 0,
            password_hint: r.get(8)?,
            pinned: r.get::<_, i64>(9)? != 0,
            has_checklist: r.get::<_, i64>(10)? != 0,
            image_count: r.get(11)?,
            available_image_count: r.get(16)?,
            attachment_count: r.get(12)?,
            tags: r
                .get::<_, Option<String>>(13)?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            has_image: r.get::<_, i64>(14)? != 0,
            shared_with: r
                .get::<_, Option<String>>(17)?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A note's first-image blob for the thumbnail protocol: `(local_path, mime,
/// decrypt_key, plain_size)` — same shape as [`media_blob`]. None if the note has
/// no resolved image.
pub fn note_image_blob(cache: &CacheDb, id: i64) -> Result<Option<MediaBlob>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT image_local_path, image_mime, NULL, image_decrypt_key, image_plain_size,
                    NULL, NULL
             FROM notes
             WHERE id = ?1 AND image_local_path IS NOT NULL",
            [id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()?)
}

/// The `index`-th embedded image of note `note_id` (0-based), for the detail
/// gallery. Mirrors `note_image_blob` but reads the `note_media` table.
pub fn note_media_blob(cache: &CacheDb, note_id: i64, index: i64) -> Result<Option<MediaBlob>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT local_path, mime, NULL, decrypt_key, plain_size,
                    NULL, NULL
             FROM note_media
             WHERE note_id = ?1 AND position = ?2",
            [note_id, index],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()?)
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

/// An installed app with the App Store metadata the backup carries.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub bundle_id: String,
    pub name: Option<String>,
    pub seller: Option<String>,
    pub version: Option<String>,
    pub genre: Option<String>,
    /// The app's App Store release date (RFC-3339); the UI formats it.
    pub released: Option<String>,
    /// When this copy was downloaded on the account (RFC-3339).
    pub downloaded: Option<String>,
    /// The Apple ID (account email) that downloaded the app.
    pub apple_id: Option<String>,
    /// App Store age rating label, e.g. "17+".
    pub content_rating: Option<String>,
    /// Finer App Store category, e.g. "Social".
    pub subgenre: Option<String>,
}

/// Apps installed on the device with their metadata, sorted by bundle id.
pub fn list_installed_apps(cache: &CacheDb) -> Result<Vec<InstalledApp>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT bundle_id, name, seller, version, genre, released,
                downloaded, apple_id, content_rating, subgenre
         FROM installed_apps ORDER BY bundle_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(InstalledApp {
            bundle_id: r.get(0)?,
            name: r.get(1)?,
            seller: r.get(2)?,
            version: r.get(3)?,
            genre: r.get(4)?,
            released: r.get(5)?,
            downloaded: r.get(6)?,
            apple_id: r.get(7)?,
            content_rating: r.get(8)?,
            subgenre: r.get(9)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// A Security Check scan run (Explicit Scan or Passive Check), newest first.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanRun {
    pub id: i64,
    pub kind: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub modules: Vec<String>,
    pub indicator_count: Option<i64>,
    /// The indicator feeds this run actually ran against (the per-run receipt,
    /// stamped at scan start — independent of later feed updates).
    pub feeds: Vec<FeedInfo>,
    /// The snapshot's generated-at (unix seconds) at scan time; None on runs
    /// recorded before the column existed.
    pub feeds_generated_at: Option<i64>,
    /// Rollup of this run's findings by severity.
    pub critical: i64,
    pub warning: i64,
    pub info: i64,
}

/// One indicator match for the findings table / detail view.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: i64,
    pub run_id: i64,
    pub severity: String,
    pub kind: String,
    pub module: String,
    pub malware: String,
    pub matched_value: String,
    pub context: Option<String>,
    pub ref_kind: Option<String>,
    pub ref_id: Option<i64>,
    pub event_time: Option<i64>,
    /// True when this finding did not appear in the previous completed scan of
    /// this backup — i.e. it is new since the last scan. False on the first
    /// scan (no baseline to diff against).
    pub is_new: bool,
}

pub fn list_scan_runs(cache: &CacheDb) -> Result<Vec<ScanRun>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare(
        "SELECT r.id, r.kind, r.started_at, r.finished_at, r.status, r.modules_json,
                r.indicator_count, r.feeds_json, r.feeds_generated_at,
                coalesce(sum(f.severity = 'critical'), 0),
                coalesce(sum(f.severity = 'warning'), 0),
                coalesce(sum(f.severity = 'info'), 0)
         FROM scan_runs r
         LEFT JOIN findings f ON f.run_id = r.id
         GROUP BY r.id
         ORDER BY r.started_at DESC, r.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let modules_json: String = r.get(5)?;
        let feeds_json: String = r.get(7)?;
        Ok(ScanRun {
            id: r.get(0)?,
            kind: r.get(1)?,
            started_at: r.get(2)?,
            finished_at: r.get(3)?,
            status: r.get(4)?,
            modules: serde_json::from_str(&modules_json).unwrap_or_default(),
            indicator_count: r.get(6)?,
            feeds: serde_json::from_str(&feeds_json).unwrap_or_default(),
            feeds_generated_at: r.get(8)?,
            critical: r.get(9)?,
            warning: r.get(10)?,
            info: r.get(11)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The id of the most recent completed scan run, if any.
pub fn latest_scan_run(cache: &CacheDb) -> Result<Option<i64>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT id FROM scan_runs WHERE status = 'done'
             ORDER BY started_at DESC, id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

/// The completed run immediately before `run_id` (by start time), for diffing.
pub fn previous_completed_run(cache: &CacheDb, run_id: i64) -> Result<Option<i64>> {
    Ok(cache
        .conn()
        .query_row(
            "SELECT id FROM scan_runs
             WHERE status = 'done'
               AND started_at < (SELECT started_at FROM scan_runs WHERE id = ?1)
             ORDER BY started_at DESC, id DESC LIMIT 1",
            [run_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Findings for a run, most severe first. `min_severity` filters (info counts
/// everything). `module` optionally restricts to one analyzer module. Each
/// finding's `is_new` flag is set by diffing against the previous completed
/// scan (same module + matched value + source artifact) — false when there is
/// no earlier scan to compare against.
pub fn list_findings(
    cache: &CacheDb,
    run_id: i64,
    min_severity: Option<&str>,
    module: Option<&str>,
) -> Result<Vec<Finding>> {
    let rank = |s: &str| match s {
        "critical" => 3,
        "warning" => 2,
        _ => 1,
    };
    let min = min_severity.map(rank).unwrap_or(1);
    let prev = previous_completed_run(cache, run_id)?;
    let conn = cache.conn();
    // A finding is "new" when there is a previous run AND no finding in it
    // shares this one's (module, matched_value, ref_kind, ref_id). `IS` treats
    // NULL ref_kind/ref_id as equal so structural findings match correctly.
    let mut stmt = conn.prepare(
        "SELECT f.id, f.run_id, f.severity, f.kind, f.module, f.malware, f.matched_value,
                f.context, f.ref_kind, f.ref_id, f.event_time,
                CASE f.severity WHEN 'critical' THEN 3 WHEN 'warning' THEN 2 ELSE 1 END AS rank,
                CASE
                  WHEN ?4 IS NULL THEN 0
                  WHEN EXISTS (
                    SELECT 1 FROM findings p
                    WHERE p.run_id = ?4 AND p.module = f.module
                      AND p.matched_value = f.matched_value
                      AND p.ref_kind IS f.ref_kind AND p.ref_id IS f.ref_id
                  ) THEN 0 ELSE 1
                END AS is_new
         FROM findings f
         WHERE f.run_id = ?1 AND rank >= ?2 AND (?3 IS NULL OR f.module = ?3)
         ORDER BY rank DESC, f.module, f.id",
    )?;
    let rows = stmt.query_map(rusqlite::params![run_id, min, module, prev], |r| {
        Ok(Finding {
            id: r.get(0)?,
            run_id: r.get(1)?,
            severity: r.get(2)?,
            kind: r.get(3)?,
            module: r.get(4)?,
            malware: r.get(5)?,
            matched_value: r.get(6)?,
            context: r.get(7)?,
            ref_kind: r.get(8)?,
            ref_id: r.get(9)?,
            event_time: r.get(10)?,
            is_new: r.get::<_, i64>(12)? != 0,
        })
    })?;
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
    /// A conversation narrowed to a period shows only that period's messages,
    /// and the thread list offers the conversations that were ACTIVE then —
    /// which is not the same as the ones that spoke last then.
    #[test]
    fn the_chats_date_filter_narrows_both_the_list_and_the_conversation() {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute_batch(
                "INSERT INTO threads (id, identifier, service, last_message_at)
                     VALUES (1, 'old-and-new', 'iMessage', 9000),
                            (2, 'only-old', 'iMessage', 1500);
                 INSERT INTO messages (id, thread_id, body, sent_at)
                     VALUES (1, 1, 'back then', 1000),
                            (2, 1, 'recently', 9000),
                            (3, 2, 'back then too', 1500);",
            )
            .unwrap();
        }
        let early = [TimeRange {
            lo: Some(0),
            hi: Some(2000),
        }];

        // Thread 1's LAST message is well outside the window, but it was active
        // inside it — filtering on last_message_at would have hidden it.
        let mut ids = threads_in_ranges(&c, &early).unwrap();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2]);

        // And the conversation itself shows only what happened then.
        assert_eq!(count_messages(&c, 1, None, None, false, &early).unwrap(), 1);
        let win = get_message_window(&c, 1, 0, 50, None, false, None, false, &early).unwrap();
        assert_eq!(win.len(), 1);
        assert_eq!(win[0].body.as_deref(), Some("back then"));

        // MULTI-SELECT: two disjoint periods mean BOTH, not neither. This is the
        // contract every filter in the app follows, and a union that quietly
        // behaved like an intersection would return nothing at all.
        let both = [
            TimeRange {
                lo: Some(0),
                hi: Some(2000),
            },
            TimeRange {
                lo: Some(8000),
                hi: Some(10_000),
            },
        ];
        assert_eq!(count_messages(&c, 1, None, None, false, &both).unwrap(), 2);
        let mut ids2 = threads_in_ranges(&c, &both).unwrap();
        ids2.sort_unstable();
        assert_eq!(ids2, vec![1, 2]);

        // Empty selection is "all time", not "nothing".
        assert_eq!(count_messages(&c, 1, None, None, false, &[]).unwrap(), 2);

        // A window with nothing in it says so, rather than falling back to all.
        let empty = [TimeRange {
            lo: Some(100_000),
            hi: Some(200_000),
        }];
        assert!(threads_in_ranges(&c, &empty).unwrap().is_empty());
        assert_eq!(count_messages(&c, 1, None, None, false, &empty).unwrap(), 0);
    }

    /// The mark filter has to give the same answer to "how many" and "which" —
    /// a header that says 2 over a list showing 5 is worse than no filter.
    #[test]
    fn the_unsafe_filter_agrees_between_the_count_and_the_window() {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute_batch(
                "INSERT INTO threads (id, identifier, display_name, service)
                     VALUES (1, 'chat-1', 'Sam', 'iMessage');
                 INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at)
                     VALUES (1, 1, 'Sam', 0, 'one', 1000),
                            (2, 1, 'Sam', 0, 'two', 2000),
                            (3, 1, NULL, 1, 'three', 3000);",
            )
            .unwrap();
        }
        crate::marks::set(&c, crate::marks::MarkKind::Message, 2, true).unwrap();

        // Off: everything.
        assert_eq!(count_messages(&c, 1, None, None, false, &[]).unwrap(), 3);
        assert_eq!(
            get_message_window(&c, 1, 0, 50, None, false, None, false, &[])
                .unwrap()
                .len(),
            3
        );

        // On: the marked one, and the same number both ways.
        assert_eq!(count_messages(&c, 1, None, None, true, &[]).unwrap(), 1);
        let win = get_message_window(&c, 1, 0, 50, None, false, None, true, &[]).unwrap();
        assert_eq!(win.len(), 1);
        assert_eq!(win[0].body.as_deref(), Some("two"));

        // And across conversations, where the timeline reads.
        assert_eq!(
            count_all_messages(&c, None, None, None, true, &[]).unwrap(),
            1
        );
        let tl = get_timeline_window(&c, 0, 50, None, None, None, false, true).unwrap();
        assert_eq!(tl.len(), 1);
        assert_eq!(tl[0].message.body.as_deref(), Some("two"));
    }

    use super::*;

    fn seed_calls(cache: &CacheDb) {
        cache
            .conn()
            .execute_batch(
                "INSERT INTO calls (id, address, direction, answered, duration_s, occurred_at)
                    VALUES (1, '+46 70 123 45 67', 'incoming', 1, 60, 1717840800);
                 INSERT INTO calls (id, address, direction, answered, duration_s, occurred_at)
                    VALUES (2, '+15551234567', 'outgoing', 1, 30, 1717840900);
                 INSERT INTO calls (id, address, direction, answered, duration_s, occurred_at)
                    VALUES (3, '+46 70 123 45 67', 'outgoing', 0, 0, 1717841000);",
            )
            .unwrap();
    }

    #[test]
    fn call_addresses_are_distinct_and_non_empty() {
        let cache = CacheDb::open_in_memory().unwrap();
        seed_calls(&cache);
        let mut got = call_addresses(&cache).unwrap();
        got.sort();
        assert_eq!(got, vec!["+15551234567", "+46 70 123 45 67"]);
    }

    #[test]
    fn a_name_match_finds_calls_the_substring_cannot() {
        // What #279 is about: the row shows "Anna", the column holds
        // "+46 70 123 45 67", and no substring of "Anna" is in that number.
        // The client resolves the name to the address; the query matches it
        // exactly, without ever normalising a phone number itself.
        let cache = CacheDb::open_in_memory().unwrap();
        seed_calls(&cache);
        let all = TimeRange { lo: None, hi: None };

        // Search alone: the term is a name, so nothing matches.
        assert_eq!(count_calls(&cache, Some("Anna"), all, None).unwrap(), 0);

        // With the client's resolution, both of Anna's calls are found -- and
        // the third, someone else's, is not.
        let anna = vec!["+46 70 123 45 67".to_string()];
        assert_eq!(
            count_calls(&cache, Some("Anna"), all, Some(&anna)).unwrap(),
            2
        );
        let rows = get_calls_window(
            &cache,
            Some("Anna"),
            all,
            0,
            50,
            Sort::new("occurred_at", true),
            Some(&anna),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|c| c.address.as_deref() == Some("+46 70 123 45 67")));
    }

    #[test]
    fn an_empty_address_list_does_not_widen_the_search() {
        // The dangerous failure: `IN ()` accidentally matching everything, or
        // an empty list being read as "no filter". A search that matches
        // nothing must still return nothing.
        let cache = CacheDb::open_in_memory().unwrap();
        seed_calls(&cache);
        let all = TimeRange { lo: None, hi: None };
        assert_eq!(count_calls(&cache, Some("zzz"), all, Some(&[])).unwrap(), 0);
        assert_eq!(count_calls(&cache, Some("zzz"), all, None).unwrap(), 0);
        // ...and no search at all still returns every call.
        assert_eq!(count_calls(&cache, None, all, None).unwrap(), 3);
        assert_eq!(count_calls(&cache, None, all, Some(&[])).unwrap(), 3);
    }

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
    fn message_row_index_matches_window_order() {
        let cache = CacheDb::open_in_memory().unwrap();
        seed(&cache); // ids 1,2,3 with ascending sent_at
        let idx = |id, desc| message_row_index(&cache, 1, id, None, desc).unwrap();
        // Ascending (oldest-first): 1,2,3.
        assert_eq!(idx(1, false), Some(0));
        assert_eq!(idx(2, false), Some(1));
        assert_eq!(idx(3, false), Some(2));
        // Descending (newest-first): 3,2,1.
        assert_eq!(idx(3, true), Some(0));
        assert_eq!(idx(2, true), Some(1));
        assert_eq!(idx(1, true), Some(2));
        // Unknown message id, or a real id in the wrong thread → None.
        assert_eq!(idx(999, false), None);
        assert_eq!(message_row_index(&cache, 2, 1, None, false).unwrap(), None);
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
                Some("/cache/media/a.png".into()),
                Some("image/png".into()),
                None,
                None,
                None,
                None,
                None
            ))
        );
        assert_eq!(media_blob(&cache, 3).unwrap(), None);
        assert_eq!(media_blob(&cache, 999).unwrap(), None);
    }

    #[test]
    fn user_favorites_toggle_persist_key_and_filter() {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute_batch(
                "INSERT INTO media_items (id, kind, relative_path, taken_at, local_path)
                    VALUES (1, 'photo', 'Media/DCIM/IMG_0001.png', 100, '/c/a.png');
                 INSERT INTO media_items (id, kind, relative_path, taken_at, local_path)
                    VALUES (2, 'photo', 'Media/DCIM/IMG_0002.png', 200, '/c/b.png');",
            )
            .unwrap();

        // Starring returns the row's stable relative_path (what gets persisted).
        assert_eq!(
            set_user_favorite(&cache, 1, true).unwrap().as_deref(),
            Some("Media/DCIM/IMG_0001.png")
        );
        assert_eq!(set_user_favorite(&cache, 999, true).unwrap(), None); // no such row

        // favorites_only now returns just the starred one.
        assert_eq!(
            count_media(&cache, &[], &[], None, true, false, &[]).unwrap(),
            1
        );
        assert_eq!(
            count_media(&cache, &[], &[], None, false, false, &[]).unwrap(),
            2
        );
        let starred = get_media_window(
            &cache,
            &[],
            &[],
            None,
            0,
            50,
            Sort::new("taken_at", true),
            true,
            false,
            &[],
        )
        .unwrap();
        assert_eq!(starred.len(), 1);
        assert_eq!(starred[0].id, 1);
        assert!(starred[0].user_favorite);

        // Re-applying from the persisted paths (as after a re-import) restores the
        // star by relative_path, even though the cache column was cleared.
        cache
            .conn()
            .execute("UPDATE media_items SET user_favorite = 0", [])
            .unwrap();
        assert_eq!(
            count_media(&cache, &[], &[], None, true, false, &[]).unwrap(),
            0
        );
        apply_user_favorites(&cache, &["Media/DCIM/IMG_0001.png".to_string()]).unwrap();
        assert_eq!(
            count_media(&cache, &[], &[], None, true, false, &[]).unwrap(),
            1
        );

        // Unstarring clears it.
        set_user_favorite(&cache, 1, false).unwrap();
        assert_eq!(
            count_media(&cache, &[], &[], None, true, false, &[]).unwrap(),
            0
        );
    }

    #[test]
    fn media_filters_union_multiple_sources_and_ranges() {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute_batch(
                // 2021 Photos, 2023 Photos, 2023 Messages, 2024 Photos, undated.
                "INSERT INTO media_items (id, kind, source, relative_path, taken_at, local_path) VALUES
                    (1,'photo','Photos','a', 1609500000, '/a'),
                    (2,'photo','Photos','b', 1672550000, '/b'),
                    (3,'photo','Messages','c', 1672560000, '/c'),
                    (4,'photo','Photos','d', 1704090000, '/d'),
                    (5,'photo','Photos','e', NULL, '/e');",
            )
            .unwrap();
        let y2023 = TimeRange {
            lo: Some(1672531200),
            hi: Some(1704067200),
        };
        let y2024 = TimeRange {
            lo: Some(1704067200),
            hi: Some(1735689600),
        };
        let s = Sort::new("taken_at", true);
        let count = |src: &[String], r: &[TimeRange]| {
            count_media(&cache, src, r, None, false, false, &[]).unwrap()
        };

        // Empty = everything (including the undated item).
        assert_eq!(count(&[], &[]), 5);
        // One source narrows.
        assert_eq!(count(&["Messages".into()], &[]), 1);
        // TWO sources = union.
        assert_eq!(count(&["Photos".into(), "Messages".into()], &[]), 5);
        // One year range (undated excluded once a range is active).
        assert_eq!(count(&[], &[y2023]), 2);
        // TWO year ranges = union.
        assert_eq!(count(&[], &[y2023, y2024]), 3);
        // Sources AND ranges compose: Photos in {2023,2024} = ids 2,4.
        let rows = get_media_window(
            &cache,
            &["Photos".into()],
            &[y2023, y2024],
            None,
            0,
            50,
            s,
            false,
            false,
            &[],
        )
        .unwrap();
        let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![4, 2]); // newest first
    }

    /// An offloaded asset has NO local_path — only a thumbnail. The old
    /// `local_path IS NOT NULL` rule hid every one of them, which with iCloud
    /// Photos on is most of a library. They must be listed, countable, and
    /// selectable on their own.
    #[test]
    fn offloaded_assets_are_listed_and_filterable() {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute_batch(
                "INSERT INTO media_items
                    (id, kind, source, relative_path, taken_at, local_path, thumb_path, availability)
                 VALUES
                    (1,'photo','Photos','a.heic', 100, '/a', '/at', 'original'),
                    (2,'photo','Photos','b.heic', 200, NULL, '/bt', 'thumbnail'),
                    (3,'photo','Photos','c.heic', 300, NULL, '/ct', 'thumbnail'),
                    -- A message attachment with no bytes at all: still excluded,
                    -- which is what the old local_path rule was really for.
                    (4,'photo','Messages','d.heic', 400, NULL, NULL, 'metadata');",
            )
            .unwrap();

        let n =
            |avail: &[String]| count_media(&cache, &[], &[], None, false, false, avail).unwrap();
        assert_eq!(n(&[]), 3, "offloaded assets must be counted, not hidden");
        assert_eq!(n(&["thumbnail".into()]), 2);
        assert_eq!(n(&["original".into()]), 1);
        // Multi-select is a union, like every other facet.
        assert_eq!(n(&["original".into(), "thumbnail".into()]), 3);

        let rows = get_media_window(
            &cache,
            &[],
            &[],
            None,
            0,
            50,
            Sort::new("taken_at", true),
            false,
            false,
            &["thumbnail".into()],
        )
        .unwrap();
        assert_eq!(rows.iter().map(|m| m.id).collect::<Vec<_>>(), vec![3, 2]);
        assert!(rows.iter().all(|m| m.availability == "thumbnail"));

        // The facet must not offer a bucket that selects nothing visible.
        let facets = media_availability(&cache).unwrap();
        assert_eq!(
            facets,
            vec![("thumbnail".into(), 2), ("original".into(), 1)]
        );
    }

    #[test]
    fn hidden_filter_selects_only_the_hidden_and_composes_with_the_rest() {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute_batch(
                "INSERT INTO media_items (id, kind, source, relative_path, taken_at, local_path, hidden, user_favorite) VALUES
                    (1,'photo','Photos','a', 100, '/a', 0, 0),
                    (2,'photo','Photos','b', 200, '/b', 1, 0),
                    (3,'photo','Messages','c', 300, '/c', 1, 1);",
            )
            .unwrap();
        let n = |hidden_only: bool, fav: bool, src: &[String]| {
            count_media(&cache, src, &[], None, fav, hidden_only, &[]).unwrap()
        };

        // Off = everything; on = only the hidden ones.
        assert_eq!(n(false, false, &[]), 3);
        assert_eq!(n(true, false, &[]), 2);

        // Composes with the other facets rather than replacing them: hidden AND
        // marked unsafe, hidden AND from a given source.
        assert_eq!(n(true, true, &[]), 1);
        assert_eq!(n(true, false, &["Messages".to_string()]), 1);
        assert_eq!(n(true, false, &["Photos".to_string()]), 1);

        // The window returns the same rows the count promised.
        let rows = get_media_window(
            &cache,
            &[],
            &[],
            None,
            0,
            50,
            Sort::new("taken_at", true),
            false,
            true,
            &[],
        )
        .unwrap();
        assert_eq!(rows.iter().map(|m| m.id).collect::<Vec<_>>(), vec![3, 2]);
        assert!(rows.iter().all(|m| m.hidden));
    }

    #[test]
    fn findings_diff_flags_new_since_previous_scan() {
        let cache = CacheDb::open_in_memory().unwrap();
        let c = cache.conn();
        // Older run (id 1) with one finding; newer run (id 2) with the same
        // finding plus a new one.
        c.execute_batch(
            "INSERT INTO scan_runs (id, kind, started_at, status) VALUES (1, 'explicit', 100, 'done');
             INSERT INTO scan_runs (id, kind, started_at, status) VALUES (2, 'explicit', 200, 'done');
             INSERT INTO findings (run_id, severity, kind, module, malware, matched_value, ref_kind, ref_id)
                VALUES (1, 'warning', 'domain', 'safari', 'M', 'evil.example', 'safari_history', 7);
             INSERT INTO findings (run_id, severity, kind, module, malware, matched_value, ref_kind, ref_id)
                VALUES (2, 'warning', 'domain', 'safari', 'M', 'evil.example', 'safari_history', 7);
             INSERT INTO findings (run_id, severity, kind, module, malware, matched_value, ref_kind, ref_id)
                VALUES (2, 'critical', 'bundle_id', 'apps', 'Stalk', 'com.evil.app', 'app', NULL);",
        )
        .unwrap();

        // First run: no baseline → nothing is "new".
        let first = list_findings(&cache, 1, None, None).unwrap();
        assert!(first.iter().all(|f| !f.is_new));

        // Second run: the carried-over safari finding is not new; the app one is.
        let second = list_findings(&cache, 2, None, None).unwrap();
        let carried = second
            .iter()
            .find(|f| f.matched_value == "evil.example")
            .unwrap();
        assert!(!carried.is_new);
        let fresh = second
            .iter()
            .find(|f| f.matched_value == "com.evil.app")
            .unwrap();
        assert!(fresh.is_new);

        assert_eq!(previous_completed_run(&cache, 2).unwrap(), Some(1));
        assert_eq!(previous_completed_run(&cache, 1).unwrap(), None);
    }

    #[test]
    fn scan_runs_round_trip_feed_receipt() {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute(
                "INSERT INTO scan_runs (id, kind, started_at, status, feeds_json, feeds_generated_at)
                 VALUES (1, 'explicit', 100, 'done',
                         '[{\"source\":\"AmnestyTech/pegasus\",\"class\":\"mercenary\",\"count\":1549,\"skipped\":0}]',
                         1752940800)",
                [],
            )
            .unwrap();
        let runs = list_scan_runs(&cache).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].feeds_generated_at, Some(1_752_940_800));
        assert_eq!(
            runs[0].feeds,
            vec![FeedInfo {
                source: "AmnestyTech/pegasus".into(),
                class: "mercenary".into(),
                count: 1549,
                skipped: 0,
            }]
        );
    }

    #[test]
    fn scan_runs_legacy_row_without_receipt_date() {
        let cache = CacheDb::open_in_memory().unwrap();
        // A pre-v49 row: feeds_json populated (since v47), no generated-at.
        cache
            .conn()
            .execute(
                "INSERT INTO scan_runs (id, kind, started_at, status, feeds_json)
                 VALUES (1, 'passive', 100, 'done',
                         '[{\"source\":\"echap/ioc\",\"class\":\"stalkerware\",\"count\":2746,\"skipped\":1}]')",
                [],
            )
            .unwrap();
        let runs = list_scan_runs(&cache).unwrap();
        assert_eq!(runs[0].feeds_generated_at, None);
        assert_eq!(runs[0].feeds.len(), 1);
        assert_eq!(runs[0].feeds[0].source, "echap/ioc");
        assert_eq!(runs[0].feeds[0].skipped, 1);
    }
}
