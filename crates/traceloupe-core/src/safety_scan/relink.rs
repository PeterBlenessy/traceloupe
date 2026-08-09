//! Re-attach findings to their source rows after a re-import (#96).
//!
//! A finding stores `source_id`, the cache row id of the message or note it came
//! from, purely as a fast join. That id is **volatile**: a re-import rebuilds
//! `cache.db` from scratch, so rows get new ids and every stored `source_id`
//! points at nothing (or, worse, at some unrelated row that happens to have
//! taken the number).
//!
//! Identity, by contrast, is durable: a finding's `fingerprint` is derived from
//! the content itself, which is exactly why it exists. So the mapping can always
//! be rebuilt — this walks the fresh cache, derives the same fingerprints the
//! chunker would, and points each finding back at its row.
//!
//! Anything that no longer matches is genuinely gone from the backup and is
//! marked stale rather than left pointing somewhere wrong.
//!
//! Both fingerprint derivations MUST match [`super::chunker`] exactly, or a
//! finding silently fails to relink. The tests assert that by round-tripping
//! through the chunker rather than re-deriving the expected values here.

use std::collections::HashMap;

use crate::analysis::{AnalysisDb, SourceKind};
use crate::cache::CacheDb;
use crate::Result;

use super::chunker::{html_to_text, message_fingerprint, note_fingerprint};

/// What a relink pass did. Reported so the caller can log it; a large `stale`
/// count after a re-import is a signal worth seeing, not a silent outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelinkOutcome {
    /// Findings whose `source_id` now points at the right row.
    pub relinked: usize,
    /// Findings whose content is no longer in the cache, marked stale.
    pub stale: usize,
    /// Findings that were already correct and needed no write.
    pub unchanged: usize,
}

/// Rebuild every live finding's `source_id` against the current cache.
///
/// Builds fingerprint→id maps in one pass over the cache rather than querying
/// per finding: the fingerprint is derived, so no index can serve it, and a
/// backup with tens of thousands of messages would otherwise mean one table scan
/// per finding.
pub fn relink_findings(cache: &CacheDb, analysis: &AnalysisDb) -> Result<RelinkOutcome> {
    let mut messages: HashMap<String, i64> = HashMap::new();
    {
        // Same shape the chunker reads, so the fingerprints agree: the thread's
        // identifier, and "me" standing in for an outgoing sender.
        let conn = cache.conn();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.sender, m.is_from_me, m.sent_at, m.body, t.identifier
             FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE m.body IS NOT NULL AND TRIM(m.body) != ''",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (id, sender, is_from_me, sent_at, body, identifier) = row?;
            let sender = if is_from_me {
                "me".to_string()
            } else {
                sender.unwrap_or_else(|| "unknown".into())
            };
            messages.insert(
                message_fingerprint(&identifier, sent_at, &sender, &body),
                id,
            );
        }
    }

    let mut notes: HashMap<String, i64> = HashMap::new();
    {
        let conn = cache.conn();
        // Locked notes are withheld from the pipeline, so they can never be the
        // source of a finding and are skipped here too.
        let mut stmt =
            conn.prepare("SELECT id, title, body_html, created_at FROM notes WHERE locked = 0")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, title, body_html, created_at) = row?;
            let title = title.unwrap_or_default();
            let text = html_to_text(body_html.as_deref().unwrap_or_default());
            notes.insert(note_fingerprint(created_at, &title, &text), id);
        }
    }

    let mut out = RelinkOutcome::default();
    for f in analysis.list_findings(None)? {
        let found = match f.source_kind {
            SourceKind::Message => messages.get(&f.fingerprint).copied(),
            SourceKind::Note => notes.get(&f.fingerprint).copied(),
        };
        match found {
            Some(id) => {
                // A finding can be stale from a PREVIOUS re-import and have come
                // back (the content was restored, or the note un-locked), so
                // clearing the flag is as important as setting it.
                if f.source_id != Some(id) {
                    analysis.set_source_id(&f.fingerprint, Some(id))?;
                    out.relinked += 1;
                } else {
                    out.unchanged += 1;
                }
                if f.stale {
                    analysis.set_stale(&f.fingerprint, false)?;
                }
            }
            None => {
                // Content genuinely absent: drop the dangling id so nothing
                // renders the wrong row, and mark it so the UI can say why.
                if !f.stale || f.source_id.is_some() {
                    analysis.set_source_id(&f.fingerprint, None)?;
                    analysis.set_stale(&f.fingerprint, true)?;
                }
                out.stale += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{Category, NewFinding};
    use crate::safety_scan::chunker::{chunk_messages, chunk_notes, TimeRange};

    /// Build a cache holding one message and one note, and return it.
    fn seeded_cache() -> CacheDb {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute(
                "INSERT INTO threads (id, identifier, display_name, service, last_message_at)
                 VALUES (1, 'chat', 'Chat', 'iMessage', 200)",
                [],
            )
            .unwrap();
        cache
            .conn()
            .execute(
                "INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at, kind)
                 VALUES (11, 1, 'them', 0, 'threatening words', 200, 'text')",
                [],
            )
            .unwrap();
        cache
            .conn()
            .execute(
                "INSERT INTO notes (id, title, body_html, created_at, modified_at, locked)
                 VALUES (22, 'Plans', '<p>a plan</p>', 500, 600, 0)",
                [],
            )
            .unwrap();
        cache
    }

    #[test]
    fn relinks_findings_whose_row_ids_moved() {
        // The re-import case: identical content, different row ids. Fingerprints
        // come from the CHUNKER so this test fails if the two derivations drift.
        let old = seeded_cache();
        let msg_fp = chunk_messages(&old, TimeRange::default(), &Default::default())
            .unwrap()
            .into_iter()
            .flat_map(|c| c.items)
            .next()
            .unwrap()
            .fingerprint;
        let note_fp = chunk_notes(&old, TimeRange::default())
            .unwrap()
            .into_iter()
            .flat_map(|c| c.items)
            .next()
            .unwrap()
            .fingerprint;

        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let scan = analysis.begin_scan("m", (None, None), "all", 1).unwrap();
        analysis
            .replace_findings(
                scan,
                &[
                    NewFinding {
                        source_kind: SourceKind::Message,
                        // Stale ids, as a re-import would leave them.
                        source_id: Some(999),
                        thread_identifier: Some("chat".into()),
                        occurred_at: Some(200),
                        fingerprint: msg_fp.clone(),
                        category: Category::ThreatViolence,
                        severity: 3,
                        rationale: "x".into(),
                        service: Some("iMessage".into()),
                        sender: None,
                    },
                    NewFinding {
                        source_kind: SourceKind::Note,
                        source_id: Some(998),
                        thread_identifier: None,
                        occurred_at: Some(500),
                        fingerprint: note_fp.clone(),
                        category: Category::SelfHarm,
                        severity: 2,
                        rationale: "y".into(),
                        service: None,
                        sender: None,
                    },
                ],
                2,
            )
            .unwrap();

        // A "re-imported" cache: same content, ids renumbered.
        let fresh = CacheDb::open_in_memory().unwrap();
        fresh
            .conn()
            .execute(
                "INSERT INTO threads (id, identifier, display_name, service, last_message_at)
                 VALUES (5, 'chat', 'Chat', 'iMessage', 200)",
                [],
            )
            .unwrap();
        fresh
            .conn()
            .execute(
                "INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at, kind)
                 VALUES (77, 5, 'them', 0, 'threatening words', 200, 'text')",
                [],
            )
            .unwrap();
        fresh
            .conn()
            .execute(
                "INSERT INTO notes (id, title, body_html, created_at, modified_at, locked)
                 VALUES (88, 'Plans', '<p>a plan</p>', 500, 600, 0)",
                [],
            )
            .unwrap();

        let out = relink_findings(&fresh, &analysis).unwrap();
        assert_eq!(out.relinked, 2, "both findings should relink: {out:?}");
        assert_eq!(out.stale, 0);

        let by_fp: HashMap<String, _> = analysis
            .list_findings(None)
            .unwrap()
            .into_iter()
            .map(|f| (f.fingerprint.clone(), f))
            .collect();
        assert_eq!(by_fp[&msg_fp].source_id, Some(77));
        assert_eq!(by_fp[&note_fp].source_id, Some(88));
        assert!(!by_fp[&msg_fp].stale);
    }

    #[test]
    fn marks_findings_stale_when_the_content_is_really_gone() {
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let scan = analysis.begin_scan("m", (None, None), "all", 1).unwrap();
        analysis
            .replace_findings(
                scan,
                &[NewFinding {
                    source_kind: SourceKind::Message,
                    source_id: Some(11),
                    thread_identifier: Some("chat".into()),
                    occurred_at: Some(200),
                    fingerprint: "no-such-content".into(),
                    category: Category::ScamFraud,
                    severity: 1,
                    rationale: "z".into(),
                    service: Some("iMessage".into()),
                    sender: None,
                }],
                2,
            )
            .unwrap();

        let out = relink_findings(&seeded_cache(), &analysis).unwrap();
        assert_eq!(out.stale, 1);
        let f = &analysis.list_findings(None).unwrap()[0];
        assert!(f.stale, "content absent → stale");
        assert_eq!(
            f.source_id, None,
            "the dangling id must be cleared so nothing renders the wrong row",
        );
    }

    #[test]
    fn a_returning_note_clears_its_stale_flag() {
        // Stale is not permanent: re-importing a backup that has the content
        // again must bring the finding back rather than leave it marked gone.
        let cache = seeded_cache();
        let note_fp = chunk_notes(&cache, TimeRange::default())
            .unwrap()
            .into_iter()
            .flat_map(|c| c.items)
            .next()
            .unwrap()
            .fingerprint;
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let scan = analysis.begin_scan("m", (None, None), "all", 1).unwrap();
        analysis
            .replace_findings(
                scan,
                &[NewFinding {
                    source_kind: SourceKind::Note,
                    source_id: None,
                    thread_identifier: None,
                    occurred_at: Some(500),
                    fingerprint: note_fp.clone(),
                    category: Category::SelfHarm,
                    severity: 2,
                    rationale: "y".into(),
                    service: None,
                    sender: None,
                }],
                2,
            )
            .unwrap();
        analysis.set_stale(&note_fp, true).unwrap();

        relink_findings(&cache, &analysis).unwrap();
        let f = &analysis.list_findings(None).unwrap()[0];
        assert!(!f.stale, "the content is back, so the finding is not stale");
        assert_eq!(f.source_id, Some(22));
    }
}
