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

/// Generation budget per chunk: verdicts are short JSON; 800 tokens covers a
/// pathological all-items-flagged window now that inputs are bounded
/// (ITEM_MAX_CHARS / NOTE_WINDOW_CHARS), while cutting a runaway generation
/// off at ~2/3 the previous cost (issue #33: runaways burned 146–212 s each).
const MAX_TOKENS: u32 = 800;

#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub chunks_done: usize,
    pub chunks_total: usize,
    /// Running tally for UI feedback. May briefly over-count a message flagged
    /// by two overlapping windows; the final [`ScanOutcome::findings`] is the
    /// exact row count.
    pub findings: usize,
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
    pub findings: usize,
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
    cancel: &CancelToken,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanOutcome> {
    let chunks = chunker::chunk_all(cache, range, sources)?;
    // Stored on the run so the history can label its scope and "Resume" can
    // re-run the same one.
    let sources_slug = match (sources.messages, sources.notes) {
        (true, false) => "messages",
        (false, true) => "notes",
        _ => "all",
    };
    let scan_id = match resume_scan_id {
        Some(id) => {
            analysis.resume_scan(id, client.model())?;
            id
        }
        None => analysis.begin_scan(
            client.model(),
            (range.start, range.end),
            sources_slug,
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

    let schema = prompt::verdicts_schema();
    let mut outcome = ScanOutcome {
        scan_id,
        status: ScanStatus::Completed,
        chunks_total: chunks.len(),
        classified: 0,
        reused: 0,
        skipped: 0,
        findings: 0,
    };

    // An initial tick with the real chunk total, so the UI flips from "loading"
    // to "scanning" the moment classification begins — otherwise the first
    // progress event only lands after the first (slow ~1 min) chunk completes,
    // leaving the model-loaded server looking like it's still starting up.
    on_progress(ScanProgress {
        chunks_done: 0,
        chunks_total: chunks.len(),
        findings: 0,
    });

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
        if outcome.reused > 0 {
            on_progress(ScanProgress {
                chunks_done: outcome.reused,
                chunks_total: outcome.chunks_total,
                findings: 0,
            });
        }

        // Classify `parallel` chunks concurrently. Workers do ONLY the HTTP
        // call (LlmClient is Sync; all its errors are Error::Inference, so a
        // worker can never surface a fatal storage error); this thread
        // persists results as they arrive, preserving the exact per-chunk
        // commit/audit/resume semantics of the sequential engine.
        let workers = parallel.max(1).min(pending.len().max(1));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (tx, rx) = std::sync::mpsc::channel::<(usize, std::result::Result<Value, Error>)>();
        std::thread::scope(|s| -> Result<()> {
            for _ in 0..workers {
                let tx = tx.clone();
                let next = &next;
                let pending = &pending;
                let schema = &schema;
                s.spawn(move || {
                    loop {
                        if cancel.is_cancelled() {
                            break;
                        }
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if i >= pending.len() {
                            break;
                        }
                        let user = prompt::render_chunk(pending[i]);
                        // One retry, as before: a transient failure shouldn't
                        // skip a chunk, a persistent one shouldn't stall it.
                        let mut result =
                            client.chat_json(prompt::SYSTEM_PROMPT, &user, schema, MAX_TOKENS);
                        if result.is_err() && !cancel.is_cancelled() {
                            result =
                                client.chat_json(prompt::SYSTEM_PROMPT, &user, schema, MAX_TOKENS);
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
                match result {
                    Ok(output) => {
                        let n = persist_classified(analysis, scan_id, chunk, &output)?;
                        outcome.classified += 1;
                        outcome.findings += n;
                    }
                    Err(e) => {
                        persist_failed(analysis, scan_id, chunk, &e)?;
                        outcome.skipped += 1;
                    }
                }
                on_progress(ScanProgress {
                    chunks_done: outcome.reused + outcome.classified + outcome.skipped,
                    chunks_total: outcome.chunks_total,
                    findings: outcome.findings,
                });
            }
            Ok(())
        })?;

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

    // Overlapping windows can flag the same message twice in the running
    // tally; the DB row count for this scan is the truth.
    outcome.findings = analysis.count_scan_findings(scan_id)? as usize;
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

/// Persist one successfully classified chunk: validated findings, the Done
/// checkpoint, and the audit row. Returns the finding count. Storage errors
/// are fatal — never classify into a broken DB (plan T5 AC).
fn persist_classified(
    analysis: &mut AnalysisDb,
    scan_id: i64,
    chunk: &Chunk,
    output: &Value,
) -> Result<usize> {
    let (findings, rejected) = verdicts_to_findings(chunk, output);
    let n = findings.len();
    analysis.replace_findings(scan_id, &findings, now())?;
    analysis.record_chunk(
        scan_id,
        &chunk.key,
        &chunk.fingerprint,
        ChunkStatus::Done,
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
    err: &Error,
) -> Result<()> {
    analysis.record_chunk(
        scan_id,
        &chunk.key,
        &chunk.fingerprint,
        ChunkStatus::Skipped,
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
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome2.reused, 3);
        assert_eq!(outcome2.classified, 0);
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
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Cancelled);
        assert_eq!(outcome.classified, 0);
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
