//! Chunker: turns cache text into deterministic classification units (plan T4).
//!
//! Messages are windowed per conversation — WINDOW messages with OVERLAP
//! carried between adjacent windows so a pattern spanning a boundary is seen
//! whole at least once. Windows run oldest→first from the start of the thread,
//! so appending new messages only adds windows at the tail and never disturbs
//! the keys/fingerprints of already-classified ones (that is what makes resume
//! and incremental re-scan cheap). Notes are one chunk each.
//!
//! Thread ordering is newest-activity-first (the scan surfaces useful findings
//! early), but *within* a thread windows stay chronological.
//!
//! Chunk keys are stable for a given cache content; fingerprints are sha256 of
//! the normalized text, so any content change forces re-classification of
//! exactly the windows it touches.

use sha2::{Digest, Sha256};

use crate::analysis::SourceKind;
use crate::cache::CacheDb;
use crate::Result;

/// Messages per window. ~25 keeps enough conversational context for the
/// pattern categories (grooming, coercive-control) while staying far under the
/// model's context budget.
pub const WINDOW: usize = 25;
/// Messages repeated from the previous window so boundary-spanning patterns
/// appear intact in at least one window.
pub const OVERLAP: usize = 5;

/// One classification unit handed to the model.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Stable key, e.g. `m:<thread_identifier>:<start_offset>` or `n:<note_fp>`.
    pub key: String,
    /// sha256 hex of the chunk's normalized text (resume/incremental identity).
    pub fingerprint: String,
    pub kind: SourceKind,
    /// `threads.identifier` for message chunks; None for notes.
    pub thread_identifier: Option<String>,
    /// Human label for prompts/summaries (thread display name or note title).
    pub label: Option<String>,
    pub items: Vec<ChunkItem>,
}

/// One message (or note body) inside a chunk.
#[derive(Debug, Clone)]
pub struct ChunkItem {
    /// Current cache row id (`messages.id` / `notes.id`).
    pub source_id: i64,
    /// Sender label shown to the model: "me" or the handle.
    pub sender: String,
    pub occurred_at: Option<i64>,
    pub text: String,
    /// Per-item fingerprint — the identity Content Findings are keyed on.
    pub fingerprint: String,
}

/// User-selected scan window (unix seconds, inclusive); `None` = unbounded.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeRange {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl TimeRange {
    fn sql_between(self, col: &str) -> String {
        // Rows with NULL timestamps are only included on an unbounded scan —
        // a bounded range can't place them, so it must not classify them.
        match (self.start, self.end) {
            (None, None) => "1=1".into(),
            (Some(_), None) => format!("{col} >= ?1"),
            (None, Some(_)) => format!("{col} <= ?1"),
            (Some(_), Some(_)) => format!("{col} BETWEEN ?1 AND ?2"),
        }
    }

    fn params(self) -> Vec<i64> {
        [self.start, self.end].iter().flatten().copied().collect()
    }
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The identity of a message for finding purposes: survives re-import (cache
/// row ids do not) and changes when the visible content changes.
pub fn message_fingerprint(
    thread_identifier: &str,
    sent_at: Option<i64>,
    sender: &str,
    body: &str,
) -> String {
    sha256_hex(&format!(
        "message|{thread_identifier}|{}|{sender}|{body}",
        sent_at.map(|t| t.to_string()).unwrap_or_default()
    ))
}

fn note_fingerprint(created_at: Option<i64>, title: &str, text: &str) -> String {
    sha256_hex(&format!(
        "note|{}|{title}|{text}",
        created_at.map(|t| t.to_string()).unwrap_or_default()
    ))
}

/// Strip HTML tags and decode the handful of entities Apple Notes bodies use,
/// so fingerprints and prompts see prose, not markup.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Only a '<' that starts a plausible tag ('<b', '</p', '<!--')
            // enters tag mode; a stray literal '<' ("3 < 5") must not swallow
            // the rest of the note.
            '<' if !in_tag => match chars.peek() {
                Some(n) if n.is_ascii_alphabetic() || *n == '/' || *n == '!' => in_tag = true,
                _ => out.push('<'),
            },
            '>' => {
                if in_tag {
                    in_tag = false;
                    // Block-ish boundary → keep words apart.
                    if !out.ends_with(' ') && !out.ends_with('\n') && !out.is_empty() {
                        out.push(' ');
                    }
                } else {
                    out.push('>');
                }
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // `&amp;` last, or a note that literally discusses "&lt;" double-decodes.
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build every message chunk in scan order: threads by most recent activity
/// first, windows chronological within each thread.
pub fn chunk_messages(cache: &CacheDb, range: TimeRange) -> Result<Vec<Chunk>> {
    let conn = cache.conn();
    let mut threads = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, identifier, display_name FROM threads ORDER BY last_message_at DESC, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            threads.push(row?);
        }
    }

    let where_range = range.sql_between("sent_at");
    let range_params = range.params();
    let mut chunks = Vec::new();
    for (thread_id, identifier, display_name) in threads {
        let sql = format!(
            "SELECT id, sender, is_from_me, sent_at, body FROM messages
             WHERE thread_id = ?{p} AND body IS NOT NULL AND TRIM(body) != ''
               AND (kind IS NULL OR kind NOT IN ('media', 'sticker', 'system'))
               AND {where_range}
             ORDER BY sent_at, id",
            p = range_params.len() + 1
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for p in &range_params {
            sql_params.push(p);
        }
        sql_params.push(&thread_id);
        let rows = stmt.query_map(sql_params.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (id, sender, is_from_me, sent_at, body) = row?;
            let sender = if is_from_me {
                "me".to_string()
            } else {
                sender.unwrap_or_else(|| "unknown".into())
            };
            let fingerprint = message_fingerprint(&identifier, sent_at, &sender, &body);
            items.push(ChunkItem {
                source_id: id,
                sender,
                occurred_at: sent_at,
                text: body,
                fingerprint,
            });
        }
        if items.is_empty() {
            continue;
        }

        // Fixed stride from the start of the thread: appends only create new
        // tail windows; earlier windows keep their key AND fingerprint.
        let stride = WINDOW - OVERLAP;
        let mut start = 0usize;
        loop {
            let end = usize::min(start + WINDOW, items.len());
            let window = &items[start..end];
            let joined = window
                .iter()
                .map(|i| format!("{}|{}", i.sender, i.text))
                .collect::<Vec<_>>()
                .join("\n");
            chunks.push(Chunk {
                key: format!("m:{identifier}:{start}"),
                fingerprint: sha256_hex(&joined),
                kind: SourceKind::Message,
                thread_identifier: Some(identifier.clone()),
                label: display_name.clone(),
                items: window.to_vec(),
            });
            if end == items.len() {
                break;
            }
            start += stride;
        }
    }
    Ok(chunks)
}

/// Build one chunk per (unlocked, non-empty) note. Locked notes are withheld —
/// their plaintext is never available to the pipeline.
pub fn chunk_notes(cache: &CacheDb, range: TimeRange) -> Result<Vec<Chunk>> {
    let conn = cache.conn();
    let where_range = range.sql_between("COALESCE(modified_at, created_at)");
    let sql = format!(
        "SELECT id, title, body_html, created_at, modified_at FROM notes
         WHERE locked = 0 AND {where_range}
         ORDER BY COALESCE(modified_at, created_at) DESC, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let range_params = range.params();
    let sql_params: Vec<&dyn rusqlite::ToSql> = range_params
        .iter()
        .map(|p| p as &dyn rusqlite::ToSql)
        .collect();
    let rows = stmt.query_map(sql_params.as_slice(), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        let (id, title, body_html, created_at, modified_at) = row?;
        let title = title.unwrap_or_default();
        let text = strip_html(body_html.as_deref().unwrap_or_default());
        if text.trim().is_empty() && title.trim().is_empty() {
            continue;
        }
        let fingerprint = note_fingerprint(created_at, &title, &text);
        chunks.push(Chunk {
            // Content-derived key: stable across re-imports even though the
            // cache row id is not.
            key: format!("n:{}", &fingerprint[..16]),
            fingerprint: fingerprint.clone(),
            kind: SourceKind::Note,
            thread_identifier: None,
            label: if title.is_empty() {
                None
            } else {
                Some(title.clone())
            },
            items: vec![ChunkItem {
                source_id: id,
                sender: "me".into(),
                occurred_at: modified_at.or(created_at),
                text: if title.is_empty() {
                    text
                } else {
                    format!("{title}\n{text}")
                },
                fingerprint,
            }],
        });
    }
    Ok(chunks)
}

/// Full scan order: all message chunks (newest threads first), then notes.
pub fn chunk_all(cache: &CacheDb, range: TimeRange) -> Result<Vec<Chunk>> {
    let mut chunks = chunk_messages(cache, range)?;
    chunks.extend(chunk_notes(cache, range)?);
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn cache_with(messages: &[(&str, i64, &str, bool)]) -> CacheDb {
        // (thread_identifier, sent_at, body, is_from_me)
        let cache = CacheDb::open_in_memory().unwrap();
        let conn = cache.conn();
        let mut thread_ids = std::collections::HashMap::new();
        for (ident, sent_at, body, from_me) in messages {
            let tid = *thread_ids.entry(ident.to_string()).or_insert_with(|| {
                conn.execute(
                    "INSERT INTO threads (identifier, service, last_message_at) VALUES (?1, 'SMS', 0)",
                    params![ident],
                )
                .unwrap();
                conn.last_insert_rowid()
            });
            conn.execute(
                "UPDATE threads SET last_message_at = MAX(last_message_at, ?2) WHERE id = ?1",
                params![tid, sent_at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'text')",
                params![
                    tid,
                    if *from_me { None::<&str> } else { Some("them") },
                    from_me,
                    body,
                    sent_at
                ],
            )
            .unwrap();
        }
        cache
    }

    #[test]
    fn chunking_is_deterministic() {
        let msgs: Vec<(&str, i64, &str, bool)> = (0..60)
            .map(|i| ("chatA", 1000 + i, "hello world", i % 2 == 0))
            .collect();
        let cache = cache_with(&msgs);
        let a = chunk_all(&cache, TimeRange::default()).unwrap();
        let b = chunk_all(&cache, TimeRange::default()).unwrap();
        let keys_a: Vec<_> = a.iter().map(|c| (&c.key, &c.fingerprint)).collect();
        let keys_b: Vec<_> = b.iter().map(|c| (&c.key, &c.fingerprint)).collect();
        assert_eq!(keys_a, keys_b);
        // 60 msgs, window 25, stride 20 → windows at 0, 20, 40 (last covers to 60).
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].items.len(), 25);
        assert_eq!(a[2].items.len(), 20);
    }

    #[test]
    fn appends_do_not_disturb_existing_windows() {
        let msgs: Vec<(&str, i64, &str, bool)> = (0..40)
            .map(|i| ("chatA", 1000 + i, "steady text", false))
            .collect();
        let cache = cache_with(&msgs);
        let before = chunk_messages(&cache, TimeRange::default()).unwrap();
        // Append newer messages (later sent_at) — the realistic re-import delta.
        for i in 0..20 {
            cache
                .conn()
                .execute(
                    "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at, kind)
                     VALUES (1, 'them', 0, 'new tail', ?1, 'text')",
                    params![2000 + i],
                )
                .unwrap();
        }
        let after = chunk_messages(&cache, TimeRange::default()).unwrap();
        assert!(after.len() > before.len());
        for (b, a) in before.iter().zip(after.iter()) {
            // Every pre-existing *complete* window is untouched; the final
            // (partial) window legitimately absorbs new tail messages.
            if b.items.len() == WINDOW {
                assert_eq!(b.key, a.key);
                assert_eq!(b.fingerprint, a.fingerprint);
            }
        }
    }

    #[test]
    fn edit_changes_only_touched_windows() {
        let msgs: Vec<(&str, i64, &str, bool)> = (0..60)
            .map(|i| ("chatA", 1000 + i, "original", false))
            .collect();
        let cache = cache_with(&msgs);
        let before = chunk_messages(&cache, TimeRange::default()).unwrap();
        // Edit one message near the start (offset 2 → only window 0 sees it).
        cache
            .conn()
            .execute(
                "UPDATE messages SET body = 'EDITED' WHERE sent_at = 1002",
                [],
            )
            .unwrap();
        let after = chunk_messages(&cache, TimeRange::default()).unwrap();
        assert_ne!(before[0].fingerprint, after[0].fingerprint);
        for i in 1..before.len() {
            assert_eq!(
                before[i].fingerprint, after[i].fingerprint,
                "window {i} moved"
            );
        }
    }

    #[test]
    fn time_range_boundaries_inclusive() {
        let msgs: Vec<(&str, i64, &str, bool)> = vec![
            ("chatA", 999, "before", false),
            ("chatA", 1000, "at start", false),
            ("chatA", 1500, "inside", false),
            ("chatA", 2000, "at end", false),
            ("chatA", 2001, "after", false),
        ];
        let cache = cache_with(&msgs);
        let chunks = chunk_messages(
            &cache,
            TimeRange {
                start: Some(1000),
                end: Some(2000),
            },
        )
        .unwrap();
        let texts: Vec<_> = chunks[0].items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, vec!["at start", "inside", "at end"]);
        // Half-open variants.
        let from_only = chunk_messages(
            &cache,
            TimeRange {
                start: Some(2000),
                end: None,
            },
        )
        .unwrap();
        assert_eq!(from_only[0].items.len(), 2);
        let until_only = chunk_messages(
            &cache,
            TimeRange {
                start: None,
                end: Some(999),
            },
        )
        .unwrap();
        assert_eq!(until_only[0].items.len(), 1);
    }

    #[test]
    fn skips_empty_and_media_bodies_and_orders_threads_by_recency() {
        let cache = cache_with(&[
            ("old-chat", 100, "old text", false),
            ("new-chat", 5000, "new text", false),
        ]);
        cache
            .conn()
            .execute(
                "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at, kind)
                 VALUES (1, 'them', 0, '  ', 101, 'text'),
                        (1, 'them', 0, 'IMG_1.HEIC', 102, 'media')",
                [],
            )
            .unwrap();
        let chunks = chunk_messages(&cache, TimeRange::default()).unwrap();
        assert_eq!(chunks.len(), 2);
        // Newest thread first.
        assert_eq!(chunks[0].thread_identifier.as_deref(), Some("new-chat"));
        // Blank + media rows dropped from old-chat.
        assert_eq!(chunks[1].items.len(), 1);
    }

    #[test]
    fn note_chunks_are_content_keyed_and_skip_locked() {
        let cache = cache_with(&[]);
        cache
            .conn()
            .execute(
                "INSERT INTO notes (id, title, body_html, created_at, modified_at, locked)
                 VALUES (7, 'Plans', '<div>Meet at &amp; the <b>docks</b></div>', 500, 600, 0),
                        (8, 'Secret', NULL, 500, 600, 1)",
                [],
            )
            .unwrap();
        let chunks = chunk_notes(&cache, TimeRange::default()).unwrap();
        assert_eq!(chunks.len(), 1, "locked note must be withheld");
        assert_eq!(chunks[0].items[0].text, "Plans\nMeet at & the docks");
        let key_before = chunks[0].key.clone();
        // Same content under a different row id (re-import) → same key.
        cache
            .conn()
            .execute("UPDATE notes SET id = 70 WHERE id = 7", [])
            .unwrap();
        let again = chunk_notes(&cache, TimeRange::default()).unwrap();
        assert_eq!(again[0].key, key_before);
        assert_eq!(again[0].items[0].source_id, 70);
    }

    #[test]
    fn strip_html_handles_tags_and_entities() {
        assert_eq!(strip_html("<p>a&nbsp;&lt;b&gt;</p><p>c</p>"), "a <b> c");
        assert_eq!(strip_html("plain"), "plain");
        assert_eq!(strip_html(""), "");
        // A stray literal '<' must not swallow the rest of the note.
        assert_eq!(strip_html("score was 3 < 5 ok"), "score was 3 < 5 ok");
        // '&amp;' decodes last: a note discussing "&lt;" must not double-decode.
        assert_eq!(strip_html("&amp;lt;"), "&lt;");
    }
}
