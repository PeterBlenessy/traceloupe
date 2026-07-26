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

/// Per-item input cap in chars (~1k tokens). One pasted wall of text must not
/// blow the whole window past the context budget — the item is truncated with
/// an explicit marker instead. The item FINGERPRINT still covers the full
/// text, so finding identity and dismissals are unaffected.
pub const ITEM_MAX_CHARS: usize = 4_000;
/// Long notes are windowed into segments of ~this many chars (~2k tokens).
/// One-chunk-per-note failed in practice: an oversized note runs the model to
/// its output cap and produces nothing, at 10× the cost of a normal chunk
/// (issue #33 — audit-log evidence).
pub const NOTE_WINDOW_CHARS: usize = 8_000;
/// Chars repeated from the previous note segment so a passage spanning a
/// boundary is seen whole at least once (mirrors message OVERLAP).
pub const NOTE_OVERLAP_CHARS: usize = 400;

/// Cap one item's text at [`ITEM_MAX_CHARS`] chars on a char boundary, with a
/// visible marker so the model (and a reviewer reading the prompt) knows.
fn cap_item_text(text: String) -> String {
    // Bytes ≥ chars, so a small byte length can never exceed the char cap.
    if text.len() <= ITEM_MAX_CHARS {
        return text;
    }
    match text.char_indices().nth(ITEM_MAX_CHARS) {
        Some((cut, _)) => format!("{} …[truncated]", &text[..cut]),
        None => text,
    }
}

/// Split a long note into windowed segments, preferring a whitespace boundary
/// near the limit so words stay whole. Notes at or under the window stay as
/// exactly one segment (the common case — and the existing chunk key shape).
fn split_note_text(text: &str) -> Vec<String> {
    if text.len() <= NOTE_WINDOW_CHARS {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut segs = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = usize::min(start + NOTE_WINDOW_CHARS, chars.len());
        let mut cut = end;
        if end < chars.len() {
            // Look for whitespace in the tail 10% of the window.
            let floor = start + NOTE_WINDOW_CHARS * 9 / 10;
            if let Some(ws) = (floor..end).rev().find(|&i| chars[i].is_whitespace()) {
                cut = ws;
            }
        }
        segs.push(chars[start..cut].iter().collect());
        if cut >= chars.len() {
            break;
        }
        // Overlap backwards from the cut, but always move forward — the cut is
        // ≥90% of a window past `start`, far beyond the overlap, so this can't
        // stall; the max() is a belt against future constant changes.
        start = usize::max(cut.saturating_sub(NOTE_OVERLAP_CHARS), start + 1);
    }
    segs
}

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
    /// The message thread's service (iMessage/SMS/TikTok…); None for notes.
    /// Flows onto each finding so a service-scoped scan can count/list its own.
    pub service: Option<String>,
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

/// Which content the scan covers. Everything by default.
#[derive(Debug, Clone)]
pub struct ScanSources {
    pub notes: bool,
    /// Which message services to include: `None` = every service (all message
    /// threads); `Some(list)` = only threads whose service is in `list` (empty
    /// list = no message threads).
    pub message_services: Option<Vec<String>>,
}

impl Default for ScanSources {
    fn default() -> Self {
        Self {
            notes: true,
            message_services: None,
        }
    }
}

impl ScanSources {
    /// Whether any message threads are in scope.
    pub fn includes_messages(&self) -> bool {
        !matches!(&self.message_services, Some(v) if v.is_empty())
    }

    /// Whether a thread with `service` is in scope for this scan.
    fn wants_service(&self, service: Option<&str>) -> bool {
        match &self.message_services {
            None => true, // all services
            Some(list) => service.is_some_and(|s| list.iter().any(|w| w == s)),
        }
    }

    /// The canonical `sources` string stored on the scan row and matched by the
    /// scope predicates: "all", "messages" (every service, no notes), or a
    /// comma-joined list of service names plus optionally "notes".
    pub fn slug(&self) -> String {
        match &self.message_services {
            None if self.notes => "all".to_string(),
            None => "messages".to_string(),
            Some(list) => {
                let mut toks = list.clone();
                if self.notes {
                    toks.push("notes".to_string());
                }
                if toks.is_empty() {
                    "all".to_string()
                } else {
                    toks.join(",")
                }
            }
        }
    }
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
    // Canonicalised for the same reason as notes: identity tracks content, not
    // rendering. Message bodies are plain today, so this is a no-op for them now
    // and insurance against the next formatting change.
    sha256_hex(&format!(
        "message|{thread_identifier}|{}|{sender}|{}",
        sent_at.map(|t| t.to_string()).unwrap_or_default(),
        canonical_for_identity(body)
    ))
}

/// Collapse a body to its canonical form for IDENTITY purposes: whitespace runs
/// (including line breaks) become single spaces.
///
/// A finding's fingerprint is what dismissals are keyed on, so it must depend on
/// what the text SAYS, not on how we happen to render it. Without this, improving
/// the HTML converter — as this change does — would silently invalidate every
/// note dismissal the user had made, because the same note would hash
/// differently. Presentation may keep evolving; identity must not move with it.
fn canonical_for_identity(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn note_fingerprint(created_at: Option<i64>, title: &str, text: &str) -> String {
    sha256_hex(&format!(
        "note|{}|{}|{}",
        created_at.map(|t| t.to_string()).unwrap_or_default(),
        canonical_for_identity(title),
        canonical_for_identity(text)
    ))
}

/// HTML → text, preserving the line structure both a reader and the classifier
/// benefit from. The ONE converter: display surfaces and the chunker share it.
///
/// There were briefly two — a flattening one for the model and a structured one
/// for display — to avoid changing chunk fingerprints. That was rejected: the two
/// duplicated all the entity and tag handling, so a fix to one would silently
/// miss the other, which is the same drift that produced the list/count and
/// Settings-spacing bugs. A single converter costs one re-scan, once.
///
/// Structure is also signal for the classifier: a note that is a shopping list
/// reads very differently from one that is a paragraph, and flattening erased
/// that distinction before the model ever saw it.
pub fn html_to_text(html: &str) -> String {
    /// Tags that end a line. Everything else (b, i, span, a…) is inline and
    /// yields a space, so words don't run together.
    const BLOCK: &[&str] = &[
        "p",
        "div",
        "br",
        "li",
        "ul",
        "ol",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "tr",
        "table",
        "blockquote",
        "pre",
        "section",
        "article",
        "header",
        "footer",
        "hr",
    ];
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Only a '<' starting a plausible tag enters tag mode; a stray
            // literal '<' ("3 < 5") must not swallow the rest of the note.
            '<' if !in_tag => match chars.peek() {
                Some(n) if n.is_ascii_alphabetic() || *n == '/' || *n == '!' => {
                    in_tag = true;
                    tag.clear();
                }
                _ => out.push('<'),
            },
            '>' if in_tag => {
                in_tag = false;
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if BLOCK.contains(&name.as_str()) {
                    if !out.ends_with('\n') && !out.is_empty() {
                        out.push('\n');
                    }
                } else if !out.ends_with(' ') && !out.ends_with('\n') && !out.is_empty() {
                    out.push(' ');
                }
            }
            _ if in_tag => tag.push(c),
            _ => out.push(c),
        }
    }
    // `&amp;` last, or text that literally discusses "&lt;" double-decodes.
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    // Tidy each line, drop runs of blank lines, keep the paragraph breaks.
    // Owned Strings: each line also has its internal space runs collapsed (an
    // inline tag adjacent to a literal space yields two), which borrowing from
    // `out` can't express.
    let mut lines: Vec<String> = Vec::new();
    for line in out.lines() {
        let t = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            if lines.last().map(|l| l.is_empty()).unwrap_or(true) {
                continue;
            }
            lines.push(String::new());
        } else {
            lines.push(t);
        }
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

/// Build every message chunk in scan order: threads by most recent activity
/// first, windows chronological within each thread.
pub fn chunk_messages(
    cache: &CacheDb,
    range: TimeRange,
    sources: &ScanSources,
) -> Result<Vec<Chunk>> {
    let conn = cache.conn();
    let mut threads = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, identifier, display_name, service FROM threads ORDER BY last_message_at DESC, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            threads.push(row?);
        }
    }

    let where_range = range.sql_between("sent_at");
    let range_params = range.params();
    let mut chunks = Vec::new();
    for (thread_id, identifier, display_name, service) in threads {
        // Skip threads whose service the scan didn't select.
        if !sources.wants_service(service.as_deref()) {
            continue;
        }
        let sql = format!(
            // Emptiness is decided by the BODY test above, not by `kind`.
            //
            // `kind` previously excluded 'media' and 'sticker' too, which looked
            // like "skip messages with nothing to read" but wasn't:
            // `message_kind` returns "media" whenever an attachment is present —
            // BEFORE it looks at the body — so every message sent WITH a photo
            // was dropped from the scan along with its text. A threatening
            // message with an image attached was never classified, and the scan
            // still reported clean. `kind` exists for the Messages content
            // filter; using it to decide scan scope was the mistake.
            //
            // Only 'system' stays excluded: those are app-generated notices
            // ("you renamed this conversation"), not anything a person wrote.
            "SELECT id, sender, is_from_me, sent_at, body FROM messages
             WHERE thread_id = ?{p} AND body IS NOT NULL AND TRIM(body) != ''
               AND (kind IS NULL OR kind != 'system')
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
            // Fingerprint the FULL body (finding identity/dismissals survive),
            // then cap what the model actually sees.
            let fingerprint = message_fingerprint(&identifier, sent_at, &sender, &body);
            items.push(ChunkItem {
                source_id: id,
                sender,
                occurred_at: sent_at,
                text: cap_item_text(body),
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
                service: service.clone(),
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
        let text = html_to_text(body_html.as_deref().unwrap_or_default());
        if text.trim().is_empty() && title.trim().is_empty() {
            continue;
        }
        let fingerprint = note_fingerprint(created_at, &title, &text);
        let full = if title.is_empty() {
            text
        } else {
            format!("{title}\n{text}")
        };
        let segments = split_note_text(&full);
        if segments.len() == 1 {
            // The common case keeps the existing key shape — already-scanned
            // short notes must not re-classify after this change.
            chunks.push(Chunk {
                // Content-derived key: stable across re-imports even though
                // the cache row id is not.
                key: format!("n:{}", &fingerprint[..16]),
                // Hash the TEXT THE MODEL WILL SEE, not the note's identity.
                // These are two different questions: the chunk fingerprint gates
                // "does this need re-classifying?", while `fingerprint` (now
                // whitespace-canonical, so dismissals survive re-rendering) answers
                // "is this the same note?". Reusing identity here would mean a
                // change to how text is rendered never triggers a re-scan — the
                // model would keep verdicts formed on text it no longer sees.
                fingerprint: sha256_hex(&full),
                kind: SourceKind::Note,
                thread_identifier: None,
                label: if title.is_empty() {
                    None
                } else {
                    Some(title.clone())
                },
                service: None,
                items: vec![ChunkItem {
                    source_id: id,
                    sender: "me".into(),
                    occurred_at: modified_at.or(created_at),
                    text: full,
                    fingerprint,
                }],
            });
        } else {
            // A long note becomes several windowed chunks (issue #33): each
            // segment classifies independently within the context budget, so
            // one oversized note can no longer fail expensively/unclassified.
            let total = segments.len();
            for (i, seg) in segments.into_iter().enumerate() {
                let seg_fp = sha256_hex(&format!("note-seg|{fingerprint}|{i}|{seg}"));
                chunks.push(Chunk {
                    key: format!("n:{}:{i}", &fingerprint[..16]),
                    fingerprint: seg_fp.clone(),
                    kind: SourceKind::Note,
                    thread_identifier: None,
                    label: Some(if title.is_empty() {
                        format!("Note (part {}/{total})", i + 1)
                    } else {
                        format!("{title} (part {}/{total})", i + 1)
                    }),
                    service: None,
                    items: vec![ChunkItem {
                        source_id: id,
                        sender: "me".into(),
                        occurred_at: modified_at.or(created_at),
                        text: seg,
                        fingerprint: seg_fp,
                    }],
                });
            }
        }
    }
    Ok(chunks)
}

/// Full scan order: all in-scope message chunks (newest threads first), then
/// notes. `sources` selects which message services and whether notes are covered.
pub fn chunk_all(cache: &CacheDb, range: TimeRange, sources: &ScanSources) -> Result<Vec<Chunk>> {
    let mut chunks = Vec::new();
    if sources.includes_messages() {
        chunks.extend(chunk_messages(cache, range, sources)?);
    }
    if sources.notes {
        chunks.extend(chunk_notes(cache, range)?);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {

    #[test]
    fn chunk_fingerprint_tracks_text_while_item_identity_tracks_content() {
        // Two fingerprints answer two different questions, and conflating them
        // breaks one or the other:
        //   chunk fingerprint  -> "does this need re-classifying?"  (tracks TEXT)
        //   item  fingerprint  -> "is this the same note?"          (tracks CONTENT)
        //
        // If the chunk fingerprint tracked identity, a rendering change would
        // never trigger a re-scan and the model would keep verdicts formed on
        // text it no longer sees. If the item fingerprint tracked text, the same
        // change would wipe the user's dismissals.
        let flat = "Shopping First line milk";
        let structured = "Shopping\nFirst line\nmilk";

        // Identity: stable across rendering.
        assert_eq!(
            note_fingerprint(Some(1), "T", flat),
            note_fingerprint(Some(1), "T", structured),
        );
        // Chunk text: must differ, so resume re-classifies.
        assert_ne!(sha256_hex(flat), sha256_hex(structured));
    }

    #[test]
    fn finding_identity_survives_a_rendering_change() {
        // The guarantee that makes future formatting work safe: a finding's
        // fingerprint — which dismissals are keyed on — must not move when only
        // the RENDERING of the same content changes. Flattened and structured
        // renderings of one note must hash identically.
        let flat = "Shopping First line Second line milk eggs";
        let structured = "Shopping\nFirst line\nSecond line\nmilk\neggs";
        assert_eq!(
            note_fingerprint(Some(10), "T", flat),
            note_fingerprint(Some(10), "T", structured),
            "improving the HTML converter must not invalidate note dismissals",
        );
        // Different CONTENT must still differ, or the fingerprint is useless.
        assert_ne!(
            note_fingerprint(Some(10), "T", structured),
            note_fingerprint(Some(10), "T", "something else entirely"),
        );
        // Same for messages, which are plain today but need the same insurance.
        assert_eq!(
            message_fingerprint("t", Some(1), "me", "a\nb"),
            message_fingerprint("t", Some(1), "me", "a b"),
        );
    }

    #[test]
    fn html_to_text_keeps_the_line_structure_a_reader_expects() {
        // The bug: every tag boundary became a space and the final pass collapsed
        // all whitespace, so a note written as paragraphs arrived as one block.
        let html = "<h1>Shopping</h1><p>First line</p><p>Second line</p><ul><li>milk</li><li>eggs</li></ul>";
        let out = html_to_text(html);
        assert_eq!(
            out, "Shopping\nFirst line\nSecond line\nmilk\neggs",
            "block tags must break lines, got: {out:?}"
        );

        // <br> is the common case inside a single paragraph.
        assert_eq!(html_to_text("a<br>b<br/>c"), "a\nb\nc");

        // Inline tags must NOT break — they just keep words apart.
        assert_eq!(
            html_to_text("<p>a <b>bold</b> and <i>italic</i></p>"),
            "a bold and italic"
        );

        // Entities still decode, and a literal '<' survives.
        assert_eq!(html_to_text("<p>3 &lt; 5 &amp; more</p>"), "3 < 5 & more");
        assert_eq!(html_to_text("<p>3 < 5</p>"), "3 < 5");

        // Runs of empty blocks collapse to at most one blank line.
        assert_eq!(html_to_text("<p>a</p><p></p><p></p><p>b</p>"), "a\nb");

        // ONE converter: the chunker sees the same structured text the reader
        // does. Structure is signal — a shopping list should not read as prose
        // to the classifier either.
    }
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
        let a = chunk_all(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
        let b = chunk_all(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
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
        let before = chunk_messages(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
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
        let after = chunk_messages(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
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
        let before = chunk_messages(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
        // Edit one message near the start (offset 2 → only window 0 sees it).
        cache
            .conn()
            .execute(
                "UPDATE messages SET body = 'EDITED' WHERE sent_at = 1002",
                [],
            )
            .unwrap();
        let after = chunk_messages(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
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
            &ScanSources::default(),
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
            &ScanSources::default(),
        )
        .unwrap();
        assert_eq!(from_only[0].items.len(), 2);
        let until_only = chunk_messages(
            &cache,
            TimeRange {
                start: None,
                end: Some(999),
            },
            &ScanSources::default(),
        )
        .unwrap();
        assert_eq!(until_only[0].items.len(), 1);
    }

    #[test]
    fn scans_captions_on_media_messages_but_skips_empty_and_system() {
        // `kind` is the Messages CONTENT FILTER's classification, not a statement
        // about whether there is anything to read: `message_kind` returns "media"
        // whenever an attachment exists, before it looks at the body. The scanner
        // used to exclude that bucket, so a message sent WITH a photo was dropped
        // along with its text — and the scan still reported clean (#97).
        let cache = cache_with(&[
            ("old-chat", 100, "old text", false),
            ("new-chat", 5000, "new text", false),
        ]);
        cache
            .conn()
            .execute(
                "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at, kind)
                 VALUES (1, 'them', 0, '  ', 101, 'text'),
                        (1, 'them', 0, 'look at this, I know where you live', 102, 'media'),
                        (1, 'them', 0, NULL, 103, 'media'),
                        (1, 'them', 0, 'you renamed the conversation', 104, 'system')",
                [],
            )
            .unwrap();
        let chunks = chunk_messages(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].thread_identifier.as_deref(), Some("new-chat"));

        let bodies: Vec<&str> = chunks[1].items.iter().map(|i| i.text.as_str()).collect();
        // The caption on a media message IS scanned — this is the bug fix.
        assert!(
            bodies.iter().any(|b| b.contains("I know where you live")),
            "a caption sent with a photo must be scanned, got {bodies:?}",
        );
        // Blank body, NULL body and app-generated system notices stay out.
        assert!(!bodies.iter().any(|b| b.trim().is_empty()));
        assert!(!bodies
            .iter()
            .any(|b| b.contains("renamed the conversation")));
        assert_eq!(bodies.len(), 2, "old text + the caption, got {bodies:?}");
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
    fn long_note_windows_into_stable_segment_chunks() {
        let cache = cache_with(&[]);
        // ~3 windows of text; multi-byte chars sprinkled in so a segment cut
        // on a non-ASCII boundary would panic if slicing were byte-based.
        let body = "wörd ålpha ".repeat(2_000); // ~22k chars
        cache
            .conn()
            .execute(
                "INSERT INTO notes (id, title, body_html, created_at, modified_at, locked)
                 VALUES (7, 'Long', ?1, 500, 600, 0)",
                params![body],
            )
            .unwrap();
        let chunks = chunk_notes(&cache, TimeRange::default()).unwrap();
        assert!(
            chunks.len() >= 3,
            "expected several segments, got {}",
            chunks.len()
        );
        // Segment keys: n:<fp16>:<i>, deterministic across runs.
        for (i, c) in chunks.iter().enumerate() {
            assert!(
                c.key.ends_with(&format!(":{i}")),
                "key {} lacks segment index",
                c.key
            );
            // The limit is in CHARS (the splitter is char-based; bytes can be
            // larger with multi-byte text like this fixture's ö/å).
            assert!(c.items[0].text.chars().count() <= NOTE_WINDOW_CHARS + NOTE_OVERLAP_CHARS + 16);
            assert_eq!(c.items[0].source_id, 7);
        }
        let again = chunk_notes(&cache, TimeRange::default()).unwrap();
        let keys: Vec<_> = chunks.iter().map(|c| (&c.key, &c.fingerprint)).collect();
        let keys2: Vec<_> = again.iter().map(|c| (&c.key, &c.fingerprint)).collect();
        assert_eq!(keys, keys2);
        // A short note keeps the single-chunk key shape (no ':' segment
        // suffix) — already-classified notes must not re-classify.
        cache
            .conn()
            .execute(
                "INSERT INTO notes (id, title, body_html, created_at, modified_at, locked)
                 VALUES (8, 'Short', 'tiny', 500, 600, 0)",
                [],
            )
            .unwrap();
        let with_short = chunk_notes(&cache, TimeRange::default()).unwrap();
        let short = with_short
            .iter()
            .find(|c| c.items[0].source_id == 8)
            .unwrap();
        assert_eq!(
            short.key.matches(':').count(),
            1,
            "short note key gained a segment suffix"
        );
    }

    #[test]
    fn oversized_message_item_is_capped_with_marker() {
        let big = "α".repeat(ITEM_MAX_CHARS + 500);
        let msgs: Vec<(&str, i64, &str, bool)> = vec![("chatA", 1000, big.as_str(), false)];
        let cache = cache_with(&msgs);
        let chunks = chunk_messages(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
        let text = &chunks[0].items[0].text;
        assert!(text.ends_with("…[truncated]"));
        assert!(text.chars().count() < ITEM_MAX_CHARS + 20);
        // Finding identity still covers the FULL body: fingerprint must match
        // the uncapped text, so dismissals survive the cap.
        assert_eq!(
            chunks[0].items[0].fingerprint,
            message_fingerprint("chatA", Some(1000), "them", &big)
        );
    }

    #[test]
    fn html_to_text_handles_tags_and_entities() {
        // Block boundaries break lines now (they used to collapse to a space).
        assert_eq!(html_to_text("<p>a&nbsp;&lt;b&gt;</p><p>c</p>"), "a <b>\nc");
        assert_eq!(html_to_text("plain"), "plain");
        assert_eq!(html_to_text(""), "");
        // A stray literal '<' must not swallow the rest of the note.
        assert_eq!(html_to_text("score was 3 < 5 ok"), "score was 3 < 5 ok");
        // '&amp;' decodes last: a note discussing "&lt;" must not double-decode.
        assert_eq!(html_to_text("&amp;lt;"), "&lt;");
    }
}
