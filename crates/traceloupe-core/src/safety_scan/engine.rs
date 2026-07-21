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

/// Generation budget per chunk: verdicts are short JSON; 1200 tokens covers a
/// pathological all-items-flagged window without letting a runaway loop stall
/// the scan for minutes.
const MAX_TOKENS: u32 = 1200;

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
pub fn run_scan(
    cache: &CacheDb,
    analysis: &mut AnalysisDb,
    client: &LlmClient,
    range: TimeRange,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanOutcome> {
    let chunks = chunker::chunk_all(cache, range)?;
    let scan_id = analysis.begin_scan(client.model(), (range.start, range.end), now())?;
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

    let loop_result = (|| -> Result<()> {
        for chunk in &chunks {
            if cancel.is_cancelled() {
                outcome.status = ScanStatus::Cancelled;
                analysis.audit(scan_id, now(), "scan_cancelled", "")?;
                break;
            }
            if analysis.chunk_is_done(&chunk.key, &chunk.fingerprint)? {
                outcome.reused += 1;
                // Persisted progress must count reused chunks too, or a
                // resumed scan completes with chunks_done < chunks_total.
                analysis.bump_chunks_done(scan_id)?;
            } else {
                match classify_chunk(analysis, client, &schema, scan_id, chunk)? {
                    ChunkResult::Classified(n) => {
                        outcome.classified += 1;
                        outcome.findings += n;
                    }
                    ChunkResult::Failed => outcome.skipped += 1,
                }
            }
            on_progress(ScanProgress {
                chunks_done: outcome.reused + outcome.classified + outcome.skipped,
                chunks_total: outcome.chunks_total,
                findings: outcome.findings,
            });
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

enum ChunkResult {
    Classified(usize),
    Failed,
}

/// Classify one chunk with one retry. A model/transport failure records the
/// chunk as skipped and lets the scan continue — a poisoned window must never
/// abort hours of work (plan T5 AC).
fn classify_chunk(
    analysis: &mut AnalysisDb,
    client: &LlmClient,
    schema: &Value,
    scan_id: i64,
    chunk: &Chunk,
) -> Result<ChunkResult> {
    let user = prompt::render_chunk(chunk);
    let mut last_err: Option<Error> = None;
    for _attempt in 0..2 {
        match client.chat_json(prompt::SYSTEM_PROMPT, &user, schema, MAX_TOKENS) {
            Ok(output) => {
                let (findings, rejected) = verdicts_to_findings(chunk, &output);
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
                return Ok(ChunkResult::Classified(n));
            }
            Err(e @ Error::Inference(_)) => last_err = Some(e),
            Err(e) => return Err(e), // storage errors are fatal — never classify into a broken DB
        }
    }
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
        &format!(
            "chunk={} reason={}",
            audit_key(&chunk.key),
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ),
    )?;
    Ok(ChunkResult::Failed)
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
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanStatus::Completed);
        assert_eq!(outcome.findings, 1, "only the valid verdict survives");
        let rows = analysis.list_findings().unwrap();
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
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            outcome.findings, 1,
            "same message via two windows is one finding"
        );
        assert_eq!(analysis.list_findings().unwrap().len(), 1);
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
            &CancelToken::new(),
            |p| {
                seen.push((p.chunks_done, p.chunks_total));
            },
        )
        .unwrap();
        assert_eq!(seen, vec![(1, 2), (2, 2)]);
    }
}
