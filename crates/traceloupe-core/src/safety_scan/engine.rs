//! The Safety Scan loop (plan T5): deterministic orchestration around the
//! stateless classifier. Selects chunks, skips already-classified ones,
//! classifies the rest, validates every verdict against the chunk it came
//! from, and persists findings + progress + audit rows after each chunk so a
//! crash resumes exactly where it stopped.

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::chunker::{self, Chunk, TimeRange};
use super::client::LlmClient;
use super::prompt;
use crate::analysis::{AnalysisDb, Category, ChunkStatus, NewFinding, ScanStatus};
use crate::cache::CacheDb;
use crate::sidecar::CancelToken;
use crate::{Error, Result};

/// Max verdicts the grammar allows for a chunk of `items` items — one verdict
/// per item is the norm. Unlike `maxItems` in a JSON schema (which the server
/// ignores), the bounded GBNF grammar in `prompt::verdicts_grammar` ENFORCES
/// this, so the array closes deterministically and never runs away.
fn chunk_max_verdicts(items: usize) -> usize {
    items.max(1)
}

/// Generation budget for a chunk of `items` items. With the array bounded by
/// the grammar, this is only a safety-net ceiling — the model closes the array
/// on its own well under this (a full flagged chunk measured ~45 tokens/verdict
/// including a short rationale). Kept generous so it never clips a legitimate
/// full array.
fn chunk_token_budget(items: usize) -> u32 {
    (items as u32 * 90 + 400).max(400)
}

#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub chunks_done: usize,
    pub chunks_total: usize,
    /// Live findings in this scan's SCOPE — re-read from the DB after every
    /// chunk, so it is exactly the number the Findings panel shows (#59): the
    /// same scope predicate, minus dismissed and stale. Not a running tally, so
    /// re-confirming an already-flagged item or two overlapping windows hitting
    /// the same message can't inflate it, and a scan whose scope already holds
    /// findings reports them from the first frame.
    pub findings: usize,
    /// How many of `findings` were already there when this run started — from
    /// earlier scans of the same scope.
    ///
    /// Without this the UI cannot tell "this run found 8823" from "8823 were
    /// already here", and the two read identically: `0% · 8823 findings so far`
    /// is a contradiction on its face. It is most visible exactly when it
    /// matters — a re-scan whose chunk cache no longer matches (the #97 chunker
    /// change invalidated every chunk containing an attachment) starts at 0%
    /// with every earlier finding already in scope.
    pub preexisting: usize,
}

#[derive(Debug, Clone)]
pub struct ScanOutcome {
    pub scan_id: i64,
    pub status: ScanStatus,
    pub chunks_total: usize,
    /// Chunks classified by the model in THIS run.
    pub classified: usize,
    /// Chunks reused from a previous run (fingerprint unchanged).
    pub reused: usize,
    /// Chunks the model failed on (recorded, scan continued).
    pub skipped: usize,
    /// Live findings in the scan's scope at the end of the run — the same
    /// number as [`ScanProgress::findings`] and the Findings panel.
    pub findings: usize,
    /// Findings this scope already held before the run started, so `findings -
    /// preexisting` is what this run is responsible for.
    pub preexisting: usize,
}

/// A scan's scope — its sources slug and optional time range — as the Findings
/// panel resolves it (from the scan row). Held for the length of the run so
/// every progress tick can ask the DB the SAME question the panel asks.
struct Scope {
    sources: String,
    start: Option<i64>,
    end: Option<i64>,
}

impl Scope {
    /// Live (non-dismissed, non-stale) findings in this scope — the one
    /// definition of "findings" the UI shows.
    fn count(&self, analysis: &AnalysisDb) -> Result<usize> {
        analysis.count_findings_in_scope(&self.sources, self.start, self.end)
    }
}

/// Chunk keys embed thread identifiers (phone numbers, emails). The audit log
/// is content-free AND contact-free: it records a short hash of the key, which
/// still correlates entries per chunk without listing who the user talks to.
fn audit_key(key: &str) -> String {
    hex::encode(&Sha256::digest(key.as_bytes())[..6])
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Eval-only view of the verdict validator (T10 live eval): returns just the
/// findings, reusing the exact production parsing/validation path.
#[cfg(test)]
pub(crate) fn verdicts_to_findings_for_eval(chunk: &Chunk, output: &Value) -> Vec<NewFinding> {
    verdicts_to_findings(chunk, output).0
}

/// Parse + validate one chunk's model output into findings. Verdict indexes
/// that don't exist in the chunk are rejected (hallucinated ids must never
/// become findings); the count of rejects is returned for the audit log.
fn verdicts_to_findings(chunk: &Chunk, output: &Value) -> (Vec<NewFinding>, usize) {
    let mut findings = Vec::new();
    let mut rejected = 0usize;
    let Some(verdicts) = output["verdicts"].as_array() else {
        return (findings, rejected);
    };
    for v in verdicts {
        let (Some(index), Some(cat), Some(severity), Some(rationale)) = (
            v["index"].as_u64(),
            v["category"].as_str(),
            v["severity"].as_u64(),
            v["rationale"].as_str(),
        ) else {
            rejected += 1;
            continue;
        };
        let Some(item) = chunk.items.get(index as usize) else {
            rejected += 1;
            continue;
        };
        let (Some(category), true) = (Category::parse(cat), (1..=3).contains(&severity)) else {
            rejected += 1;
            continue;
        };
        findings.push(NewFinding {
            source_kind: chunk.kind,
            source_id: Some(item.source_id),
            thread_identifier: chunk.thread_identifier.clone(),
            occurred_at: item.occurred_at,
            fingerprint: item.fingerprint.clone(),
            category,
            severity: severity as u8,
            rationale: rationale.to_string(),
            service: chunk.service.clone(),
        });
    }
    (findings, rejected)
}

/// Run a full Safety Scan. Progress is reported after every chunk; the scan is
/// cancellable between chunks and resumable across process restarts.
/// `resume_scan_id` continues that existing (non-completed) scan row — same
/// identity, accumulating findings — instead of creating a new one.
/// `parallel` chunks are classified concurrently (the server must be started
/// with at least as many slots); all persistence stays on this thread, so
/// per-chunk commit/resume semantics are identical at any parallelism.
#[allow(clippy::too_many_arguments)] // one scan's distinct inputs, grouped by caller
pub fn run_scan(
    cache: &CacheDb,
    analysis: &mut AnalysisDb,
    client: &LlmClient,
    range: TimeRange,
    sources: chunker::ScanSources,
    resume_scan_id: Option<i64>,
    parallel: usize,
    // Cascade (#35): when present, called AFTER the sweep to boot the
    // stronger tier; its client re-checks every flagged chunk (the caller
    // keeps that server alive until run_scan returns). None = single-tier.
    recheck: Option<&mut dyn FnMut() -> Result<LlmClient>>,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanOutcome> {
    let chunks = chunker::chunk_all(cache, range, &sources)?;
    // Stored on the run so the history can label its scope and "Resume" can
    // re-run the same one.
    let sources_slug = sources.slug();
    let scan_id = match resume_scan_id {
        Some(id) => {
            analysis.resume_scan(id, client.model())?;
            id
        }
        None => analysis.begin_scan(
            client.model(),
            (range.start, range.end),
            &sources_slug,
            now(),
        )?,
    };
    analysis.set_chunks_total(scan_id, chunks.len() as i64)?;
    analysis.audit(
        scan_id,
        now(),
        "scan_started",
        &format!(
            "chunks={} range={:?}..{:?} model={}",
            chunks.len(),
            range.start,
            range.end,
            client.model()
        ),
    )?;

    // The scope the Findings panel resolves for this scan — read from the scan
    // row, exactly as `list_content_findings` does, so the live counter and the
    // panel can't disagree (#59). The row was just written above; the passed
    // arguments are only a defensive fallback.
    let scope = match analysis.scan_by_id(scan_id)? {
        Some(s) => Scope {
            sources: s.sources,
            start: s.range_start,
            end: s.range_end,
        },
        None => Scope {
            sources: sources_slug.clone(),
            start: range.start,
            end: range.end,
        },
    };

    // Read once, before anything is classified: everything this scope already
    // held. `findings` moves from here; this stays put, so the UI can say which
    // part of the count this run is responsible for.
    let preexisting = scope.count(analysis)?;
    let mut outcome = ScanOutcome {
        scan_id,
        status: ScanStatus::Completed,
        chunks_total: chunks.len(),
        classified: 0,
        reused: 0,
        skipped: 0,
        // Not zero: a scan whose scope already contains findings (a re-scan, a
        // resume, or another run that covered the same data) must report them
        // from the very first frame, like the panel does.
        findings: preexisting,
        preexisting,
    };

    let loop_result = (|| -> Result<()> {
        // Settle already-classified chunks first (DB-only, fast), collecting
        // what actually needs inference.
        let mut pending: Vec<&chunker::Chunk> = Vec::new();
        for chunk in &chunks {
            if cancel.is_cancelled() {
                outcome.status = ScanStatus::Cancelled;
                analysis.audit(scan_id, now(), "scan_cancelled", "")?;
                return Ok(());
            }
            if analysis.chunk_is_done(&chunk.key, &chunk.fingerprint)? {
                outcome.reused += 1;
                // Persisted progress must count reused chunks too, or a
                // resumed scan completes with chunks_done < chunks_total.
                analysis.bump_chunks_done(scan_id)?;
            } else {
                pending.push(chunk);
            }
        }
        // The first tick lands AFTER settling, so the UI flips from "loading" to
        // "scanning" already at the TRUE state: a resumed scan shows its reused
        // count (e.g. 53/72) and existing findings at once instead of 0% → jump;
        // a fresh scan shows 0/total. Settling is DB-only (fast), so this still
        // arrives well before the first slow inference chunk.
        on_progress(ScanProgress {
            chunks_done: outcome.reused,
            chunks_total: outcome.chunks_total,
            findings: outcome.findings,
            preexisting: outcome.preexisting,
        });

        // Phase 1: sweep every pending chunk with the primary model.
        classify_batch(
            analysis,
            client,
            &pending,
            scan_id,
            &scope,
            parallel,
            cancel,
            false,
            &mut outcome,
            &mut on_progress,
        )?;

        // Phase 2 (cascade, #35): re-check flagged chunks with the stronger
        // tier. `flagged` (chunks whose sweep produced a finding) is derived
        // from the DB, so an interrupted cascade recomputes it on resume; a
        // chunk's `#recheck` checkpoint marks it independently re-checked.
        if let Some(provider) = recheck {
            if !cancel.is_cancelled() {
                // The flagged set is the DURABLE sweep-time marker, NOT live
                // findings — so a sibling window's re-check deleting a shared
                // item's finding can't drop a chunk that a crash left un-checked
                // (verification Finding A). A flagged chunk not yet re-checked
                // needs the strong tier; one already re-checked keeps its
                // `#recheck` checkpoint even if its finding was cleared.
                let flagged_keys = analysis.flagged_chunk_keys()?;
                let mut todo: Vec<&chunker::Chunk> = Vec::new();
                for chunk in &chunks {
                    if flagged_keys.contains(&chunk.key)
                        && !analysis
                            .chunk_is_done(&format!("{}#recheck", chunk.key), &chunk.fingerprint)?
                    {
                        todo.push(chunk);
                    }
                }
                if !todo.is_empty() {
                    match provider() {
                        Ok(strong) => {
                            outcome.chunks_total += todo.len();
                            analysis.set_chunks_total(scan_id, outcome.chunks_total as i64)?;
                            analysis.audit(
                                scan_id,
                                now(),
                                "recheck_started",
                                &format!("chunks={} model={}", todo.len(), strong.model()),
                            )?;
                            let before_skipped = outcome.skipped;
                            classify_batch(
                                analysis,
                                &strong,
                                &todo,
                                scan_id,
                                &scope,
                                parallel,
                                cancel,
                                true,
                                &mut outcome,
                                &mut on_progress,
                            )?;
                            // Stamp the cascade receipt only when every flagged
                            // chunk was actually re-checked (none skipped, none
                            // left by a cancel) — otherwise the label would
                            // overclaim what the strong tier judged.
                            let all_done =
                                !cancel.is_cancelled() && outcome.skipped == before_skipped;
                            if all_done {
                                analysis.set_model(
                                    scan_id,
                                    &format!("{}→{}", client.model(), strong.model()),
                                )?;
                            }
                        }
                        Err(e) => {
                            // The sweep's verdicts stand; a re-check that can't
                            // start must never sink hours of completed work.
                            analysis.audit(
                                scan_id,
                                now(),
                                "recheck_unavailable",
                                &e.to_string(),
                            )?;
                        }
                    }
                }
                // (A resume where phase 2 already completed has an empty todo
                //  AND keeps its "e2b→e4b" receipt, since resume_scan no longer
                //  overwrites the model — Finding 4.)
            }
        }

        if cancel.is_cancelled() {
            outcome.status = ScanStatus::Cancelled;
            analysis.audit(scan_id, now(), "scan_cancelled", "")?;
        }
        Ok(())
    })();
    if let Err(e) = loop_result {
        // Best effort: a fatal storage error must not strand the scan row as
        // 'running' — that reads as a phantom in-flight scan forever.
        let _ = analysis.finish_scan(scan_id, ScanStatus::Failed, now());
        return Err(e);
    }

    // Final value, same definition as every tick along the way: live findings
    // in this scan's scope — what the Findings panel shows.
    outcome.findings = scope.count(analysis)?;
    analysis.finish_scan(scan_id, outcome.status, now())?;
    analysis.audit(
        scan_id,
        now(),
        "scan_finished",
        &format!(
            "status={:?} classified={} reused={} skipped={} findings={}",
            outcome.status, outcome.classified, outcome.reused, outcome.skipped, outcome.findings
        ),
    )?;
    Ok(outcome)
}

/// Classify `pending` chunks with `parallel` concurrent workers. Workers do
/// ONLY the HTTP call (LlmClient is Sync; all its errors are
/// Error::Inference, so a worker can never surface a fatal storage error);
/// the calling thread persists results as they arrive, preserving the exact
/// per-chunk commit/audit/resume semantics of the sequential engine.
/// `recheck` selects the cascade's strong-tier phase: results persist through
/// the ATOMIC `apply_recheck` (clear-sweep + insert + `#recheck` checkpoint in
/// one transaction) so a crash can't drop a finding, and a failed re-check
/// leaves the sweep verdict standing.
#[allow(clippy::too_many_arguments)] // one batch's distinct inputs
fn classify_batch(
    analysis: &mut AnalysisDb,
    client: &LlmClient,
    pending: &[&Chunk],
    scan_id: i64,
    scope: &Scope,
    parallel: usize,
    cancel: &CancelToken,
    recheck: bool,
    outcome: &mut ScanOutcome,
    on_progress: &mut impl FnMut(ScanProgress),
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let workers = parallel.max(1).min(pending.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, std::result::Result<Value, Error>)>();
    std::thread::scope(|s| -> Result<()> {
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || {
                loop {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if i >= pending.len() {
                        break;
                    }
                    let chunk = pending[i];
                    // Grammar + budget are PER CHUNK: the GBNF grammar bounds
                    // the verdicts array to the item count so a weak over-flagging
                    // tier can't run generation away, and the token budget is a
                    // generous safety net (the bounded array closes on its own).
                    let grammar = prompt::verdicts_grammar(chunk_max_verdicts(chunk.items.len()));
                    let max_tokens = chunk_token_budget(chunk.items.len());
                    let user = prompt::render_chunk(chunk);
                    // One retry: a transient failure shouldn't skip a chunk,
                    // a persistent one shouldn't stall it.
                    let mut result =
                        client.chat_json(prompt::SYSTEM_PROMPT, &user, &grammar, max_tokens);
                    if result.is_err() && !cancel.is_cancelled() {
                        result =
                            client.chat_json(prompt::SYSTEM_PROMPT, &user, &grammar, max_tokens);
                    }
                    if tx.send((i, result)).is_err() {
                        break; // receiver gone (fatal storage error path)
                    }
                }
            });
        }
        drop(tx);

        for (i, result) in rx {
            let chunk = pending[i];
            let suffix = if recheck { "#recheck" } else { "" };
            match result {
                Ok(output) => {
                    if recheck {
                        persist_recheck(analysis, scan_id, chunk, &output)?;
                    } else {
                        persist_classified(analysis, scan_id, chunk, "", &output)?;
                    }
                    outcome.classified += 1;
                    // Re-read rather than accumulate: the DB is the deduped
                    // source of truth, so re-confirming an already-flagged item
                    // (replace_findings transfers ownership) and two overlapping
                    // windows hitting the same message both stop inflating the
                    // number (#59). One indexed COUNT(*) per chunk, next to
                    // ~a minute of inference.
                    outcome.findings = scope.count(analysis)?;
                }
                Err(e) => {
                    // A failed re-check must NOT clear the sweep verdict — it
                    // just records the #recheck checkpoint as skipped so resume
                    // retries it, leaving the sweep finding in place meanwhile.
                    persist_failed(analysis, scan_id, chunk, suffix, &e)?;
                    outcome.skipped += 1;
                }
            }
            on_progress(ScanProgress {
                chunks_done: outcome.reused + outcome.classified + outcome.skipped,
                chunks_total: outcome.chunks_total,
                findings: outcome.findings,
                preexisting: outcome.preexisting,
            });
        }
        Ok(())
    })
}

/// Persist the cascade strong tier's verdict for one chunk via the atomic
/// `apply_recheck`. Returns the finding count it wrote.
fn persist_recheck(
    analysis: &mut AnalysisDb,
    scan_id: i64,
    chunk: &Chunk,
    output: &Value,
) -> Result<usize> {
    let (findings, rejected) = verdicts_to_findings(chunk, output);
    let n = findings.len();
    let item_fps: Vec<String> = chunk.items.iter().map(|i| i.fingerprint.clone()).collect();
    analysis.apply_recheck(
        scan_id,
        &chunk.key,
        &chunk.fingerprint,
        &item_fps,
        &findings,
        now(),
    )?;
    analysis.audit(
        scan_id,
        now(),
        "chunk_rechecked",
        &format!(
            "chunk={} items={} verdicts={n} rejected={rejected}",
            audit_key(&chunk.key),
            chunk.items.len()
        ),
    )?;
    Ok(n)
}

/// Persist one successfully classified chunk: validated findings, the Done
/// checkpoint, and the audit row. Returns the finding count. Storage errors
/// are fatal — never classify into a broken DB (plan T5 AC).
fn persist_classified(
    analysis: &mut AnalysisDb,
    scan_id: i64,
    chunk: &Chunk,
    key_suffix: &str,
    output: &Value,
) -> Result<usize> {
    let (findings, rejected) = verdicts_to_findings(chunk, output);
    let n = findings.len();
    analysis.replace_findings(scan_id, &findings, now())?;
    analysis.record_chunk(
        scan_id,
        &format!("{}{}", chunk.key, key_suffix),
        &chunk.fingerprint,
        ChunkStatus::Done,
        // Durable cascade marker: this sweep chunk produced ≥1 finding, so the
        // strong tier must re-check it (survives a sibling clearing the item).
        n > 0,
        now(),
    )?;
    analysis.audit(
        scan_id,
        now(),
        "chunk_classified",
        &format!(
            "chunk={} items={} verdicts={n} rejected={rejected}",
            audit_key(&chunk.key),
            chunk.items.len()
        ),
    )?;
    Ok(n)
}

/// Record a chunk whose inference failed (after the worker's retry) as
/// skipped, so the scan continues — a poisoned window must never abort hours
/// of work (plan T5 AC).
fn persist_failed(
    analysis: &mut AnalysisDb,
    scan_id: i64,
    chunk: &Chunk,
    key_suffix: &str,
    err: &Error,
) -> Result<()> {
    analysis.record_chunk(
        scan_id,
        &format!("{}{}", chunk.key, key_suffix),
        &chunk.fingerprint,
        ChunkStatus::Skipped,
        false, // a failed chunk produced no finding
        now(),
    )?;
    analysis.audit(
        scan_id,
        now(),
        "chunk_skipped",
        // Counts only (never content): enough to diagnose an oversized-input
        // regression straight from the log (issue #33).
        &format!(
            "chunk={} items={} input_chars={} reason={err}",
            audit_key(&chunk.key),
            chunk.items.len(),
            chunk.items.iter().map(|i| i.text.len()).sum::<usize>(),
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{Category, NewFinding, SourceKind};
    use rusqlite::params;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Tiny canned-response HTTP server. Each connection gets the next
    /// response from the list (last one repeats); returns (base_url, hits).
    fn mock_server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let i = hits2.fetch_add(1, Ordering::SeqCst);
                let resp = responses
                    .get(i)
                    .unwrap_or_else(|| responses.last().unwrap());
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp.len(),
                    resp
                );
            }
        });
        (base, hits)
    }

    fn envelope(content: &Value) -> String {
        serde_json::json!({
            "choices": [{ "message": { "content": content.to_string() } }]
        })
        .to_string()
    }

    fn small_cache(n: i64) -> CacheDb {
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute(
                "INSERT INTO threads (identifier, service, last_message_at) VALUES ('chatA', 'SMS', 999)",
                [],
            )
            .unwrap();
        for i in 0..n {
            cache
                .conn()
                .execute(
                    "INSERT INTO messages (thread_id, sender, is_from_me, body, sent_at, kind)
                     VALUES (1, 'them', 0, ?1, ?2, 'text')",
                    params![format!("msg {i}"), 1000 + i],
                )
                .unwrap();
        }
        cache
    }

    fn client_for(base: &str) -> LlmClient {
        LlmClient::new(base, "test-model", std::time::Duration::from_secs(5))
    }

    #[test]
    fn parallel_classification_covers_every_chunk() {
        // 60 messages → 3 windows; one canned clean response serves them all
        // (mock_server repeats its last response). Workers race for chunks;
        // every chunk must still classify exactly once.
        let clean = serde_json::json!({ "verdicts": [] });
        let (base, hits) = mock_server(vec![envelope(&clean)]);
        let cache = small_cache(60);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            3,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Completed);
        assert_eq!(outcome.chunks_total, 3);
        assert_eq!(outcome.classified, 3);
        assert_eq!(outcome.skipped, 0);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            3,
            "each chunk hits the model once"
        );
        // Every chunk checkpointed: a re-run reuses all three.
        let outcome2 = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            3,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome2.reused, 3);
        assert_eq!(outcome2.classified, 0);
    }

    #[test]
    fn cascade_recheck_overrules_sweep_false_positive() {
        // Sweep tier flags item 0; strong tier returns clean — the finding
        // must be REMOVED (silence overrules), the receipt must name both
        // tiers, and a resumed run must have nothing left to do.
        let flagged = serde_json::json!({ "verdicts": [
            { "index": 0, "category": "scam-fraud", "severity": 2, "rationale": "sweep verdict" }
        ]});
        let clean = serde_json::json!({ "verdicts": [] });
        let (base1, _h1) = mock_server(vec![envelope(&flagged)]);
        let (base2, h2) = mock_server(vec![envelope(&clean)]);
        let cache = small_cache(3);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let mut provider = || Ok(client_for(&base2));
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base1),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            Some(&mut provider),
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Completed);
        assert_eq!(outcome.chunks_total, 2, "one sweep chunk + one re-check");
        assert_eq!(
            h2.load(Ordering::SeqCst),
            1,
            "strong tier saw the flagged chunk"
        );
        assert_eq!(
            outcome.findings, 0,
            "strong tier's silence overrules the flag"
        );
        assert!(analysis.list_findings(None).unwrap().is_empty());
        let scans = analysis.list_scans(10).unwrap();
        assert_eq!(scans[0].model, "test-model→test-model");

        // Resume: sweep chunk reused, flagged set now empty → the provider
        // must not even be called.
        let mut provider2 =
            || -> crate::Result<LlmClient> { panic!("no re-check should be needed") };
        let o2 = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base1),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            Some(&mut provider2),
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(o2.reused, 1);
        assert_eq!(o2.classified, 0);
    }

    #[test]
    fn cascade_unavailable_keeps_sweep_findings_then_resume_rechecks() {
        // Sweep flags item 0; the strong tier is UNAVAILABLE (provider errors).
        // The sweep finding must survive and the scan complete — then a resume
        // with a working provider must re-check the still-flagged chunk (its
        // #recheck checkpoint was never written), i.e. no finding is stranded.
        let flagged = serde_json::json!({ "verdicts": [
            { "index": 0, "category": "scam-fraud", "severity": 2, "rationale": "sweep" }
        ]});
        let (base1, _h1) = mock_server(vec![envelope(&flagged)]);
        let cache = small_cache(3);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();

        let mut failing = || -> crate::Result<LlmClient> {
            Err(crate::Error::Inference("strong tier won't load".into()))
        };
        let o1 = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base1),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            Some(&mut failing),
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(o1.status, ScanStatus::Completed);
        assert_eq!(
            o1.findings, 1,
            "sweep finding stands when re-check can't run"
        );
        // Receipt stays single-tier: the strong model never judged anything.
        assert_eq!(analysis.list_scans(10).unwrap()[0].model, "test-model");

        // Resume with a working strong tier that CONFIRMS the finding.
        let confirm = serde_json::json!({ "verdicts": [
            { "index": 0, "category": "scam-fraud", "severity": 3, "rationale": "confirmed" }
        ]});
        let (base2, h2) = mock_server(vec![envelope(&confirm)]);
        let mut working = || Ok(client_for(&base2));
        run_scan(
            &cache,
            &mut analysis,
            &client_for(&base1),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            Some(&mut working),
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            h2.load(Ordering::SeqCst),
            1,
            "resume re-checked the flagged chunk"
        );
        let rows = analysis.list_findings(None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].severity, 3,
            "strong tier's confirmed verdict replaced the sweep's"
        );
        assert_eq!(
            analysis.list_scans(10).unwrap()[0].model,
            "test-model→test-model"
        );
    }

    #[test]
    fn scan_writes_validated_findings_and_rejects_hallucinated_indexes() {
        let content = serde_json::json!({
            "verdicts": [
                { "index": 0, "category": "threat-violence", "severity": 3, "rationale": "explicit threat" },
                { "index": 99, "category": "threat-violence", "severity": 3, "rationale": "hallucinated" },
                { "index": 1, "category": "not-a-category", "severity": 2, "rationale": "bad slug" }
            ]
        });
        let (base, _hits) = mock_server(vec![envelope(&content)]);
        let cache = small_cache(3);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Completed);
        assert_eq!(outcome.findings, 1, "only the valid verdict survives");
        let rows = analysis.list_findings(None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, Category::ThreatViolence);
        assert_eq!(rows[0].severity, 3);
        assert_eq!(rows[0].thread_identifier.as_deref(), Some("chatA"));
    }

    #[test]
    fn malformed_output_skips_chunk_but_scan_completes() {
        // Content that is not JSON at all, twice (initial + retry).
        let bad = serde_json::json!({
            "choices": [{ "message": { "content": "I think this looks fine!" } }]
        })
        .to_string();
        let (base, hits) = mock_server(vec![bad]);
        let cache = small_cache(2);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Completed);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.findings, 0);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "exactly one retry");
    }

    #[test]
    fn second_run_reuses_everything_with_zero_model_calls() {
        let content = serde_json::json!({ "verdicts": [] });
        let (base, hits) = mock_server(vec![envelope(&content)]);
        let cache = small_cache(30); // 2 windows
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let first = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(first.classified, 2);
        let calls_after_first = hits.load(Ordering::SeqCst);
        let second = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(second.reused, 2);
        assert_eq!(second.classified, 0);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            calls_after_first,
            "no new model calls"
        );
        // Persisted progress counts reused chunks: a fully-reused scan must
        // not read as "completed 0 of 2" in the scans table.
        let (done, total): (i64, i64) = analysis
            .conn()
            .query_row(
                "SELECT chunks_done, chunks_total FROM scans WHERE id = ?1",
                rusqlite::params![second.scan_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((done, total), (2, 2));
    }

    #[test]
    fn a_rescan_reports_earlier_findings_as_preexisting_not_as_its_own() {
        // The shape a user actually hit: a re-scan whose chunk cache no longer
        // matches starts at 0% while the scope already holds findings from an
        // earlier run. Reporting one number made that read "0% · N findings so
        // far", which is a contradiction — the run had found nothing yet.
        let content = serde_json::json!({
            "verdicts": [
                { "index": 0, "category": "threat-violence", "severity": 3, "rationale": "explicit threat" }
            ]
        });
        let (base, _hits) = mock_server(vec![envelope(&content)]);
        let cache = small_cache(3);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let first = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(first.findings, 1);
        assert_eq!(first.preexisting, 0, "nothing existed before the first run");

        // Change a message's text, the way the #97 chunker fix did for every
        // message carrying an attachment: the chunk's fingerprint moves, so
        // none of the cached work applies and the re-scan starts from zero.
        cache
            .conn()
            .execute(
                "UPDATE messages SET body = 'edited so the chunk fingerprint moves' WHERE id = 1",
                [],
            )
            .unwrap();

        let mut ticks: Vec<ScanProgress> = Vec::new();
        let second = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |p| ticks.push(p),
        )
        .unwrap();

        let first_tick = ticks.first().expect("a tick lands before any inference");
        assert_eq!(first_tick.chunks_done, 0, "nothing was reusable");
        assert_eq!(
            first_tick.preexisting, 1,
            "the earlier run's finding is in scope from the first frame",
        );
        assert_eq!(
            first_tick.findings - first_tick.preexisting,
            0,
            "and NONE of it is this run's work yet — the number the UI shows as `new`",
        );
        assert_eq!(second.reused, 0);
        assert_eq!(second.preexisting, 1);
    }

    #[test]
    fn overlap_double_flag_counts_as_one_finding() {
        // 30 messages → windows [0..25] and [20..30]. Message 22 appears in
        // both (offset 22 and offset 2). Flag it from BOTH windows; the
        // outcome must count one finding, not two.
        let v = |idx: u64| {
            envelope(&serde_json::json!({
                "verdicts": [
                    { "index": idx, "category": "harassment-bullying", "severity": 2, "rationale": "insults" }
                ]
            }))
        };
        let (base, _hits) = mock_server(vec![v(22), v(2)]);
        let cache = small_cache(30);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            outcome.findings, 1,
            "same message via two windows is one finding"
        );
        assert_eq!(analysis.list_findings(None).unwrap().len(), 1);
    }

    #[test]
    fn cancellation_finishes_scan_as_cancelled() {
        let content = serde_json::json!({ "verdicts": [] });
        let (base, _hits) = mock_server(vec![envelope(&content)]);
        let cache = small_cache(3);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Cancelled);
        assert_eq!(outcome.classified, 0);
    }

    #[test]
    fn progress_counts_pre_existing_scope_findings_from_the_first_frame() {
        // The reported symptom (#59): a new scan over content an EARLIER scan
        // already flagged showed 0 in the progress bar while the Findings panel
        // showed the pre-existing total, and the two then drifted further apart.
        // Both must report the same number — live findings in scope — throughout.
        let cache = small_cache(30);
        let mut analysis = AnalysisDb::open_in_memory().unwrap();

        // An earlier scan owns a finding that falls in the new scan's scope.
        let prior = analysis.begin_scan("m", (None, None), "all", 10).unwrap();
        analysis
            .replace_findings(
                prior,
                &[NewFinding {
                    source_kind: SourceKind::Message,
                    source_id: Some(1),
                    thread_identifier: Some("t".into()),
                    occurred_at: Some(1_000),
                    fingerprint: "pre-existing".into(),
                    category: Category::ScamFraud,
                    severity: 2,
                    rationale: "earlier run".into(),
                    service: Some("iMessage".into()),
                }],
                11,
            )
            .unwrap();
        analysis
            .finish_scan(prior, ScanStatus::Completed, 12)
            .unwrap();

        let content = serde_json::json!({ "verdicts": [] });
        let (base, _hits) = mock_server(vec![envelope(&content)]);
        let mut seen: Vec<usize> = Vec::new();
        let outcome = run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |p| seen.push(p.findings),
        )
        .unwrap();

        // Every frame — including the first — sees the pre-existing finding,
        // because the new scan's scope covers it. Previously these were 0.
        assert!(!seen.is_empty(), "expected progress frames");
        assert!(
            seen.iter().all(|&n| n == 1),
            "every frame must report the in-scope total, got {seen:?}",
        );
        // And the terminal count agrees with the panel's definition.
        assert_eq!(outcome.findings, 1);
        assert_eq!(
            outcome.findings,
            analysis.count_findings_in_scope("all", None, None).unwrap(),
        );
    }

    #[test]
    fn progress_is_reported_per_chunk() {
        let content = serde_json::json!({ "verdicts": [] });
        let (base, _hits) = mock_server(vec![envelope(&content)]);
        let cache = small_cache(30); // 2 windows
        let mut analysis = AnalysisDb::open_in_memory().unwrap();
        let mut seen = Vec::new();
        run_scan(
            &cache,
            &mut analysis,
            &client_for(&base),
            TimeRange::default(),
            chunker::ScanSources::default(),
            None,
            1,
            None,
            &CancelToken::new(),
            |p| {
                seen.push((p.chunks_done, p.chunks_total));
            },
        )
        .unwrap();
        // (0, 2) is the initial "starting to scan" tick before any chunk runs.
        assert_eq!(seen, vec![(0, 2), (1, 2), (2, 2)]);
    }
}
