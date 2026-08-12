//! The triage scan orchestrator (#459): census → rank → focused deep-scan.
//!
//! This is the composition of the merged triage primitives. It is written
//! against closures for the two model calls — `embed` and `classify` — so the
//! whole phase flow (score every message, budget the worklist, build context
//! windows, persist findings, report coverage) tests end to end with fake
//! models against a real analysis store. The command layer wires the real
//! sandboxed sidecars into the same function.
//!
//! End-to-end on realistic data this pipeline reached recall 0.94 at precision
//! 0.95 versus the shipped batch scan's 0.30 / 0.89
//! (docs/validation/safety-scan-validation.md).

use crate::analysis::{AnalysisDb, Category, CensusRow, NewFinding, SourceKind};
use crate::safety_scan::chunker::{Chunk, ChunkItem};
use crate::safety_scan::client::LlmClient;
use crate::safety_scan::triage::{self, CensusInput, FocusWindow, ScanMode};
use crate::safety_scan::{engine, eval, prompt};
use crate::sidecar::CancelToken;
use crate::Result;

/// One verdict a focused classify call returns for the item it judged.
#[derive(Debug, Clone)]
pub struct FocusVerdict {
    pub category: Category,
    pub severity: u8,
    pub rationale: String,
}

/// Where a triage scan is, for the progress stream. Emitted at phase entry and
/// then per unit of work, so the first event of each phase carries `done: 0`.
#[derive(Debug, Clone, Copy)]
pub enum TriageProgress {
    /// Census phase: `done` of `total` messages scored THIS run (already-scored
    /// messages are skipped and not counted — a resumed census starts small).
    Census { done: usize, total: usize },
    /// Focused deep-scan: `done` of `total` worklist items, `findings` so far
    /// (pre-confirmation).
    DeepScan {
        done: usize,
        total: usize,
        findings: usize,
    },
    /// Confirmation of provisional findings (only when the mode confirms).
    Confirm { done: usize, total: usize },
}

/// What a triage scan did, for the honest coverage report.
#[derive(Debug, Clone, Default)]
pub struct TriageOutcome {
    /// Messages scored by the census.
    pub censused: usize,
    /// Messages at or above the mode's threshold — the deep-scan demand.
    pub candidates: usize,
    /// Messages actually deep-scanned (candidates, minus what a budget cut).
    pub deep_scanned: usize,
    /// Findings written.
    pub findings: usize,
    /// Findings the confirmation stage removed (0 when the mode has it off).
    pub unconfirmed: usize,
    /// Worklist items whose focused call failed even after a retry — skipped,
    /// audited, and the scan continued. ALL of them failing fails the scan
    /// (the §10.6 harness-bug signature).
    pub deep_scan_failed: usize,
    /// Messages the embedder could not score even after capping. Never
    /// candidates, so never deep-scanned — coverage the scan does not have,
    /// reported rather than hidden.
    pub unscorable: usize,
    /// Provisional findings whose CONFIRMATION call failed even after a retry
    /// — dropped (the mode promised a second opinion) and audited. ALL of them
    /// failing fails the scan, same as `deep_scan_failed`.
    pub confirm_failed: usize,
    /// Verdicts the validation rejected (malformed, or naming an item outside
    /// the window). Diagnosability: a zero-finding scan with high `rejected`
    /// is a harness bug, not a clean backup.
    pub rejected: usize,
    /// Verdicts refused because the judged item has no standalone content.
    pub contentless: usize,
    /// True when a cancel stopped the scan early. Whatever was completed is
    /// persisted (census rows, confirmed findings); the caller marks the scan
    /// row cancelled rather than done.
    pub cancelled: bool,
}

/// What one focused classify call produced: the verdicts the validation kept,
/// plus what it threw away. The rejection counts ride along for §10.6
/// diagnosability — a scan that finds nothing because every verdict was
/// malformed must be distinguishable from a genuinely clean scan.
#[derive(Debug, Clone, Default)]
pub struct FocusOutcome {
    pub verdicts: Vec<FocusVerdict>,
    pub rejected: usize,
    pub contentless: usize,
}

impl TriageOutcome {
    /// Candidates a budget left unscanned — the tail the report must not call
    /// "clean". Zero means full coverage of everything above the threshold.
    pub fn unscanned(&self) -> usize {
        self.candidates.saturating_sub(self.deep_scanned)
    }
}

/// Census rows are persisted every this-many scored messages, so cancelling an
/// hours-long census keeps the work already done (the next run skips scored
/// ids). Small enough to lose little on cancel, large enough that the insert
/// batching stays efficient.
const CENSUS_RECORD_BATCH: usize = 256;

/// Focused calls generate one short verdict object; 600 tokens is ample and is
/// what the validated reference pipeline used for its focused stage.
const FOCUSED_MAX_TOKENS: u32 = 600;

/// The (category, text) examples the census prototypes are built from, drawn
/// from the committed fixture positives and filtered to the SELECTED
/// categories (`triage::build_prototypes` turns them into per-category
/// centroids). A multi-message case is joined into one example: the pattern
/// categories (grooming, coercive control) are defined across messages, and
/// the prototype should carry that shape.
///
/// An empty result means "cannot triage" — the caller must refuse to scan
/// rather than census against nothing (every score would be 0 and the scan
/// would report a clean-looking silence).
pub fn prototype_examples(categories: &[Category]) -> Vec<(String, String)> {
    let fixtures = eval::load_fixtures();
    let mut out = Vec::new();
    for case in fixtures.cases.iter().filter(|c| c.kind == "positive") {
        let text = case
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for cat in case.expected_categories() {
            if categories.contains(&cat) {
                out.push((cat.as_str().to_string(), text.clone()));
            }
        }
    }
    out
}

/// A [`FocusWindow`] rendered as the [`Chunk`] the production prompt and
/// verdict validation operate on. The chunk-level fingerprint/key are synthetic
/// — nothing persists them; finding identity comes from the per-item
/// fingerprints, which are real.
fn window_chunk(window: &FocusWindow) -> Chunk {
    let focus = &window.items[window.focus];
    Chunk {
        key: format!("triage:{}:{}", focus.thread_identifier, focus.source_id),
        fingerprint: String::new(),
        kind: SourceKind::Message,
        thread_identifier: Some(focus.thread_identifier.clone()),
        label: None,
        service: focus.service.clone(),
        items: window
            .items
            .iter()
            .map(|m| ChunkItem {
                source_id: m.source_id,
                sender: m.sender.clone(),
                occurred_at: m.occurred_at,
                text: m.text.clone(),
                fingerprint: m.fingerprint.clone(),
            })
            .collect(),
    }
}

/// Judge one context window through the PRODUCTION focused path: the real
/// system prompt, the real GBNF grammar, the real parse/validation and the
/// focus clamp. This is the `classify` closure the command layer hands
/// `run_triage` — kept here so no caller can accidentally re-implement the
/// prompt or grammar (the mistake that produced three false "recall 0.00"
/// results; journey §10.6).
pub fn classify_focused(client: &LlmClient, window: &FocusWindow) -> Result<FocusOutcome> {
    let chunk = window_chunk(window);
    let user = prompt::render_focused(&chunk, window.focus);
    let grammar = prompt::verdicts_grammar(chunk.items.len());
    let out = client.chat_json(prompt::SYSTEM_PROMPT, &user, &grammar, FOCUSED_MAX_TOKENS)?;
    let verdicts = engine::verdicts_to_findings_focused(&chunk, &out, window.focus);
    Ok(FocusOutcome {
        rejected: verdicts.rejected,
        contentless: verdicts.contentless,
        verdicts: verdicts
            .findings
            .into_iter()
            .map(|f| FocusVerdict {
                category: f.category,
                severity: f.severity,
                rationale: f.rationale,
            })
            .collect(),
    })
}

/// Run a triage scan.
///
/// `threads` is the in-scope messages grouped by thread, each ordered oldest
/// first — the shape a context window needs. `prototypes` are the selected
/// categories' centroids (empty ⇒ the caller should not have called this; every
/// score would be 0). `budget` caps deep-scan work; None runs every candidate.
///
/// `classify(window)` judges `window.focus` with the whole window as context and
/// returns verdicts for that item only. `confirm(window, verdict)` is the
/// optional second opinion; it runs as its own phase AFTER the whole worklist is
/// classified (only when `mode.confirm()`). Batching confirmation matters
/// operationally: the classifier and the confirmer are different multi-GB
/// models, and a phase boundary is what lets the command layer swap one out for
/// the other instead of holding both resident. It also mirrors the validated
/// reference pipeline (tools/validate-triage-pipeline.py), which confirms as a
/// separate stage.
///
/// `cancel` is checked between units of work AND on every model error — the
/// command layer's cancel-watcher kills the server to abort an in-flight
/// request, so the dominant Stop path surfaces as a connection error, and that
/// error must read as "you stopped it", never as a failed scan. On cancel the
/// outcome comes back `cancelled: true` with everything completed so far
/// persisted. A finding whose confirmation was cancelled before it ran is NOT
/// written — the mode promised a confirmed result, and a resume re-classifies
/// the worklist.
#[allow(clippy::too_many_arguments)]
pub fn run_triage<E, C, F, P>(
    analysis: &mut AnalysisDb,
    scan_id: i64,
    threads: &[Vec<CensusInput>],
    prototypes: &[Vec<f32>],
    mode: ScanMode,
    budget: Option<usize>,
    now: i64,
    mut embed: E,
    mut classify: C,
    mut confirm: F,
    cancel: &CancelToken,
    mut progress: P,
) -> Result<TriageOutcome>
where
    E: FnMut(&str) -> Result<Vec<f32>>,
    C: FnMut(&FocusWindow) -> Result<FocusOutcome>,
    F: FnMut(&FocusWindow, &FocusVerdict) -> Result<bool>,
    P: FnMut(TriageProgress),
{
    let mut out = TriageOutcome::default();

    // --- phase 1: census every message ---
    // The skip key is (id, fingerprint), not the id: this store survives
    // re-import while cache row ids do not, so an id match alone could be a
    // different message wearing a recycled id.
    let flat: Vec<&CensusInput> = threads.iter().flatten().collect();
    let already = analysis.census_scored()?;
    let todo: Vec<CensusInput> = flat
        .iter()
        .filter(|m| !already.contains(&(m.source_id, m.fingerprint.clone())))
        .map(|m| (*m).clone())
        .collect();
    // "Censused" counts messages with a score: what was already scored in
    // scope, plus what this run scores — never the whole scope up front, or a
    // cancelled census would claim coverage it does not have.
    let in_scope_already = flat.len() - todo.len();
    out.censused = in_scope_already;
    progress(TriageProgress::Census {
        done: 0,
        total: todo.len(),
    });
    let mut census_done = 0usize;
    for batch in todo.chunks(CENSUS_RECORD_BATCH) {
        if cancel.is_cancelled() {
            out.cancelled = true;
            return Ok(out);
        }
        let scored = match triage::census_messages(batch, prototypes, &mut embed) {
            Ok(s) => s,
            // The Stop path: the watcher killed the server under an in-flight
            // embed. The already-recorded prefix stands; report cancelled.
            Err(_) if cancel.is_cancelled() => {
                out.cancelled = true;
                return Ok(out);
            }
            Err(e) => return Err(e),
        };
        out.unscorable += scored.unscorable;
        let rows: Vec<CensusRow> = scored
            .scored
            .iter()
            .map(|s| CensusRow {
                source_id: s.source_id,
                thread_identifier: s.thread_identifier.clone(),
                sender: s.sender.clone(),
                occurred_at: s.occurred_at,
                score: s.score as f64,
                fingerprint: batch
                    .iter()
                    .find(|m| m.source_id == s.source_id)
                    .map(|m| m.fingerprint.clone())
                    .unwrap_or_default(),
            })
            .collect();
        analysis.record_census(&rows, now)?;
        census_done += batch.len();
        out.censused = in_scope_already + census_done;
        progress(TriageProgress::Census {
            done: census_done,
            total: todo.len(),
        });
    }

    // Locate a message by id: its thread and index within it, for the window.
    // Built once — a scan touches many work items.
    let mut locate: std::collections::HashMap<i64, (usize, usize)> =
        std::collections::HashMap::new();
    for (ti, thread) in threads.iter().enumerate() {
        for (mi, m) in thread.iter().enumerate() {
            locate.insert(m.source_id, (ti, mi));
        }
    }

    // --- phase 2: rank + budget ---
    // The census table is global (it survives across scans and scopes), so the
    // ranked list is filtered to THIS scan's scope — and to rows whose
    // fingerprint still matches the message their id resolves to — BEFORE the
    // budget is applied. Otherwise a scoped scan after a wider one spends its
    // budget on out-of-scope rows and reports other threads' messages in its
    // coverage numbers. (The fingerprint half is defense-in-depth: the census
    // upsert above already refreshed every in-scope id, so a stale row only
    // reaches this filter through a future path that skips phase 1.)
    let threshold = mode.census_threshold() as f64;
    let worklist: Vec<_> = analysis
        .triage_worklist(threshold, None)?
        .into_iter()
        .filter(|item| {
            locate
                .get(&item.source_id)
                .is_some_and(|&(ti, mi)| threads[ti][mi].fingerprint == item.fingerprint)
        })
        .collect();
    out.candidates = worklist.len();
    let worklist: Vec<_> = match budget {
        Some(b) => worklist.into_iter().take(b).collect(),
        None => worklist,
    };

    // --- phase 3: focused deep-scan the worklist ---
    // Verdicts are provisional here; confirmation (when the mode has it) is the
    // next phase, over the collected results, so the two model tiers never need
    // to be resident together.
    let mut provisional: Vec<(FocusWindow, FocusVerdict)> = Vec::new();
    progress(TriageProgress::DeepScan {
        done: 0,
        total: worklist.len(),
        findings: 0,
    });
    'deep: for (done, item) in worklist.iter().enumerate() {
        if cancel.is_cancelled() {
            out.cancelled = true;
            break;
        }
        // The filter above proved membership; a miss here cannot happen, but
        // stay defensive rather than panicking mid-scan.
        let Some(&(ti, mi)) = locate.get(&item.source_id) else {
            continue;
        };
        out.deep_scanned += 1;
        let window = triage::context_window(&threads[ti], mi, ScanMode::default_radius());
        // One retry per item, then skip it and continue — a transient server
        // hiccup on item 109 of 110 must not discard 108 completed verdicts
        // (the batch engine has the same per-chunk forgiveness).
        let mut focus_out = None;
        for attempt in 0..2 {
            match classify(&window) {
                Ok(o) => {
                    focus_out = Some(o);
                    break;
                }
                Err(_) if cancel.is_cancelled() => {
                    out.cancelled = true;
                    break 'deep;
                }
                Err(e) if attempt == 1 => {
                    out.deep_scan_failed += 1;
                    let _ = analysis.audit(scan_id, now, "triage_item_failed", &format!("{e}"));
                }
                Err(_) => {}
            }
        }
        if let Some(o) = focus_out {
            out.rejected += o.rejected;
            out.contentless += o.contentless;
            for v in o.verdicts {
                provisional.push((window.clone(), v));
            }
        }
        progress(TriageProgress::DeepScan {
            done: done + 1,
            total: worklist.len(),
            findings: provisional.len(),
        });
    }
    // Every attempted item failing is not "a clean backup" — it is the §10.6
    // signature of a harness bug (wrong grammar, dead server), and it must
    // fail the scan loudly rather than complete with zero findings.
    if !out.cancelled && out.deep_scanned > 0 && out.deep_scan_failed == out.deep_scanned {
        return Err(crate::Error::Inference(
            "every focused classification failed — suspect the harness (server/grammar), \
             not a clean backup"
                .into(),
        ));
    }

    // --- phase 4: confirm the provisional findings (mode-dependent) ---
    let mut findings: Vec<NewFinding> = Vec::new();
    let to_confirm = provisional.len();
    if mode.confirm() {
        progress(TriageProgress::Confirm {
            done: 0,
            total: to_confirm,
        });
    }
    'confirm: for (done, (window, v)) in provisional.into_iter().enumerate() {
        if mode.confirm() {
            if out.cancelled || cancel.is_cancelled() {
                // Unconfirmed-on-cancel findings are dropped, not written: the
                // mode promised a second opinion. The scan is marked cancelled,
                // so nothing reads this as "clean".
                out.cancelled = true;
                break;
            }
            // One retry then drop-and-audit, exactly like a classify error: a
            // transient hiccup on confirmation 89 of 90 must not abort the run
            // and discard every already-vetted finding (replace_findings only
            // runs after this loop). A dropped finding is NOT written — the
            // mode promised a second opinion — and all-of-them failing fails
            // the scan loudly below.
            let mut kept = None;
            for attempt in 0..2 {
                match confirm(&window, &v) {
                    Ok(k) => {
                        kept = Some(k);
                        break;
                    }
                    // Stop under an in-flight confirm call: the vetted
                    // findings so far are written; this one and the rest are
                    // dropped unvetted.
                    Err(_) if cancel.is_cancelled() => {
                        out.cancelled = true;
                        break 'confirm;
                    }
                    Err(e) if attempt == 1 => {
                        out.confirm_failed += 1;
                        let _ =
                            analysis.audit(scan_id, now, "triage_confirm_failed", &format!("{e}"));
                    }
                    Err(_) => {}
                }
            }
            progress(TriageProgress::Confirm {
                done: done + 1,
                total: to_confirm,
            });
            match kept {
                Some(true) => {}
                Some(false) => {
                    out.unconfirmed += 1;
                    continue;
                }
                None => continue, // failed after retry; audited above
            }
        }
        let judged = &window.items[window.focus];
        findings.push(NewFinding {
            source_kind: SourceKind::Message,
            source_id: Some(judged.source_id),
            thread_identifier: Some(judged.thread_identifier.clone()),
            occurred_at: judged.occurred_at,
            fingerprint: judged.fingerprint.clone(),
            category: v.category,
            severity: v.severity,
            rationale: v.rationale,
            service: judged.service.clone(),
            sender: Some(judged.sender.clone()),
            content_key: crate::safety_scan::content_key::content_key(&judged.text),
        });
    }
    // Every confirmation failing is the same §10.6 signature as every classify
    // failing: a dead or mis-driven confirmer, not a backup with no confirmable
    // findings — surface it instead of writing zero findings that read clean.
    if !out.cancelled && to_confirm > 0 && out.confirm_failed == to_confirm {
        return Err(crate::Error::Inference(
            "every confirmation failed — suspect the harness (confirmer server/template), \
             not the findings"
                .into(),
        ));
    }
    analysis.replace_findings(scan_id, &findings, now)?;
    out.findings = findings.len();
    // The per-scan diagnosability record (mirrors the batch engine's per-chunk
    // audit): with this, a silent-zero scan is distinguishable from a clean
    // one straight from the audit trail.
    let _ = analysis.audit(
        scan_id,
        now,
        "triage_deep_scan",
        &format!(
            "scanned={} findings={} rejected={} contentless={} failed={} unconfirmed={} confirm_failed={} unscorable={}",
            out.deep_scanned,
            out.findings,
            out.rejected,
            out.contentless,
            out.deep_scan_failed,
            out.unconfirmed,
            out.confirm_failed,
            out.unscorable
        ),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::CancelToken;

    /// Wrap plain verdicts as a clean FocusOutcome (tests don't exercise the
    /// rejected/contentless counters unless they say so).
    fn fo(verdicts: Vec<FocusVerdict>) -> FocusOutcome {
        FocusOutcome {
            verdicts,
            ..Default::default()
        }
    }

    // Fake embedder: "threat"/"kill" -> x-axis, else z-axis (orthogonal).
    fn embed(t: &str) -> Result<Vec<f32>> {
        let t = t.to_lowercase();
        Ok(if t.contains("kill") || t.contains("threat") {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        })
    }
    fn threat_proto() -> Vec<Vec<f32>> {
        vec![vec![1.0, 0.0]]
    }

    fn thread(msgs: &[(i64, &str, &str)]) -> Vec<CensusInput> {
        msgs.iter()
            .map(|(id, sender, text)| CensusInput {
                source_id: *id,
                thread_identifier: "t".into(),
                sender: (*sender).into(),
                occurred_at: Some(1000 + id),
                text: (*text).into(),
                fingerprint: format!("fp{id}"),
                service: None,
            })
            .collect()
    }

    #[test]
    fn a_threat_among_chatter_is_censused_scanned_and_found() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![thread(&[
            (1, "a", "grab milk"),
            (2, "a", "i will kill you"),
            (3, "a", "see you later"),
        ])];
        // classify flags the focused item iff it is the threat.
        let classify = |w: &FocusWindow| -> Result<FocusOutcome> {
            let judged = &w.items[w.focus];
            Ok(fo(if judged.text.contains("kill") {
                vec![FocusVerdict {
                    category: Category::ThreatViolence,
                    severity: 3,
                    rationale: "threat".into(),
                }]
            } else {
                vec![]
            }))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();

        assert_eq!(out.censused, 3);
        assert_eq!(
            out.candidates, 1,
            "only the threat scores above the census threshold"
        );
        assert_eq!(out.deep_scanned, 1, "chatter never reaches the classifier");
        assert_eq!(out.findings, 1);
        assert_eq!(out.unscanned(), 0);

        let rows = db.list_findings(Some(scan)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, Category::ThreatViolence);
        assert_eq!(
            rows[0].source_id,
            Some(2),
            "the finding points at the threat message"
        );
    }

    #[test]
    fn confirmation_off_keeps_what_precise_would_drop() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![thread(&[(1, "a", "i will kill you")])];
        let classify = |_: &FocusWindow| {
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        // A confirmer that vetoes EVERYTHING.
        let veto = |_: &FocusWindow, _: &FocusVerdict| Ok(false);

        // Thorough: confirmation off, so the veto never runs — the finding stays.
        let t = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            veto,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(t.findings, 1);
        assert_eq!(t.unconfirmed, 0);

        // Precise: confirmation on, the veto fires, the finding is dropped.
        let scan2 = db.begin_scan("m", (None, None), "all", 2).unwrap();
        let p = run_triage(
            &mut db,
            scan2,
            &threads,
            &threat_proto(),
            ScanMode::Precise,
            None,
            10,
            embed,
            classify,
            veto,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(p.findings, 0);
        assert_eq!(p.unconfirmed, 1);
    }

    #[test]
    fn a_budget_leaves_an_honest_unscanned_tail() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        // Five threats — all candidates.
        let threads = vec![thread(&[
            (1, "a", "kill 1"),
            (2, "a", "kill 2"),
            (3, "a", "kill 3"),
            (4, "a", "kill 4"),
            (5, "a", "kill 5"),
        ])];
        let classify = |_: &FocusWindow| {
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            Some(2),
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(out.candidates, 5);
        assert_eq!(out.deep_scanned, 2, "budget capped the deep scan");
        assert_eq!(
            out.unscanned(),
            3,
            "and three are reported unscanned, not clean"
        );
    }

    #[test]
    fn prototype_examples_cover_selected_categories_only() {
        // Every category must have committed positives to build a prototype
        // from — a category with none would be silently unscannable.
        for cat in Category::ALL {
            let ex = prototype_examples(&[cat]);
            assert!(
                !ex.is_empty(),
                "no fixture positives for {} — its census prototype cannot be built",
                cat.as_str()
            );
            assert!(ex.iter().all(|(c, _)| c == cat.as_str()));
        }
        // Selection scopes: a scam-only selection carries no threat examples.
        let scam_only = prototype_examples(&[Category::ScamFraud]);
        assert!(scam_only.iter().all(|(c, _)| c == "scam-fraud"));
        // And nothing selected means nothing to census against — the caller's
        // "cannot triage" guard has to be able to see this.
        assert!(prototype_examples(&[]).is_empty());
    }

    #[test]
    fn window_chunk_carries_identity_and_context_shape() {
        let mut t = thread(&[
            (1, "a", "hello"),
            (2, "b", "i will kill you"),
            (3, "a", "bye"),
        ]);
        t[1].service = Some("SMS".into());
        let w = triage::context_window(&t, 1, 2);
        let chunk = window_chunk(&w);
        assert_eq!(chunk.items.len(), 3, "whole window rides as context");
        assert_eq!(chunk.thread_identifier.as_deref(), Some("t"));
        assert_eq!(chunk.service.as_deref(), Some("SMS"));
        assert_eq!(
            chunk.items[w.focus].fingerprint, "fp2",
            "per-item identity is the real fingerprint"
        );
    }

    /// The refactor's contract: confirmation is a PHASE after the whole
    /// worklist is classified, never interleaved — that boundary is what lets
    /// the command layer swap the classifier out for the confirmer instead of
    /// holding two multi-GB models resident (and it mirrors the validated
    /// Python reference, which confirms as a separate stage).
    #[test]
    fn confirmation_runs_as_a_phase_after_all_classification() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![
            thread(&[(1, "a", "i will kill you")]),
            thread(&[(2, "a", "kill him too")]),
        ];
        let order = std::cell::RefCell::new(Vec::<&'static str>::new());
        let classify = |w: &FocusWindow| -> Result<FocusOutcome> {
            order.borrow_mut().push("classify");
            let _ = w;
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| {
            order.borrow_mut().push("confirm");
            Ok(true)
        };
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Balanced,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(out.findings, 2);
        let o = order.borrow();
        assert_eq!(o.iter().filter(|s| **s == "classify").count(), 2);
        assert_eq!(o.iter().filter(|s| **s == "confirm").count(), 2);
        let first_confirm = o.iter().position(|s| *s == "confirm").unwrap();
        assert!(
            o[..first_confirm].iter().all(|s| *s == "classify"),
            "every classify call precedes the first confirm: {o:?}"
        );
    }

    #[test]
    fn cancel_between_census_batches_keeps_the_scored_prefix() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        // More than one census batch, so a cancel can land between them.
        let msgs: Vec<(i64, &str, &str)> = (0..300).map(|i| (i as i64, "a", "hi")).collect();
        let threads = vec![thread(&msgs)];
        let cancel = CancelToken::new();
        let cancel2 = cancel.clone();
        // Cancel as soon as the first batch reports done.
        let progress = move |p: TriageProgress| {
            if let TriageProgress::Census { done, .. } = p {
                if done > 0 {
                    cancel2.cancel();
                }
            }
        };
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            panic!("a cancelled census must never reach classification")
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &cancel,
            progress,
        )
        .unwrap();
        assert!(out.cancelled);
        assert_eq!(
            db.census_scored().unwrap().len(),
            256,
            "the completed batch is persisted; the rest resumes next run"
        );
        assert_eq!(
            out.censused, 256,
            "coverage reports what was actually scored, not the whole scope"
        );
    }

    #[test]
    fn cancel_mid_deep_scan_keeps_completed_findings() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![
            thread(&[(1, "a", "i will kill you")]),
            thread(&[(2, "a", "kill him too")]),
        ];
        let cancel = CancelToken::new();
        let cancel2 = cancel.clone();
        let calls = std::cell::Cell::new(0usize);
        let classify = move |_: &FocusWindow| -> Result<FocusOutcome> {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                cancel2.cancel();
            }
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert!(out.cancelled);
        assert_eq!(out.deep_scanned, 1, "the second item was never classified");
        assert_eq!(out.findings, 1, "the completed finding is written");
        assert_eq!(db.list_findings(Some(scan)).unwrap().len(), 1);
    }

    #[test]
    fn cancel_mid_confirm_drops_the_unvetted_tail() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![
            thread(&[(1, "a", "i will kill you")]),
            thread(&[(2, "a", "kill him too")]),
        ];
        let cancel = CancelToken::new();
        let cancel2 = cancel.clone();
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirms = std::cell::Cell::new(0usize);
        let confirm = move |_: &FocusWindow, _: &FocusVerdict| {
            confirms.set(confirms.get() + 1);
            if confirms.get() == 1 {
                cancel2.cancel();
            }
            Ok(true)
        };
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Precise,
            None,
            10,
            embed,
            classify,
            confirm,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert!(out.cancelled);
        assert_eq!(
            out.findings, 1,
            "only the confirmed finding is written; the unvetted one is dropped, not passed through"
        );
    }

    #[test]
    fn findings_carry_the_message_fingerprint_and_service() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let mut t = thread(&[(1, "a", "i will kill you")]);
        t[0].fingerprint = "msg:durable-identity".into();
        t[0].service = Some("iMessage".into());
        let threads = vec![t];
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let rows = db.list_findings(Some(scan)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].fingerprint, "msg:durable-identity",
            "identity is the durable message fingerprint, not a cache row id"
        );
        // FindingRow doesn't surface service; read the stored column directly.
        let svc: Option<String> = db
            .conn()
            .query_row(
                "SELECT service FROM content_findings WHERE id = ?1",
                [rows[0].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(svc.as_deref(), Some("iMessage"));
    }

    /// The census table is global and survives across scans; a scoped scan
    /// must not spend its budget on (or count) another scope's rows.
    #[test]
    fn a_scoped_scan_ignores_out_of_scope_census_rows() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        // A prior, wider scan censused another thread's hot messages.
        db.record_census(
            &[
                CensusRow {
                    source_id: 900,
                    thread_identifier: "other-thread".into(),
                    sender: "x".into(),
                    occurred_at: Some(1),
                    score: 0.99,
                    fingerprint: "fp-other-900".into(),
                },
                CensusRow {
                    source_id: 901,
                    thread_identifier: "other-thread".into(),
                    sender: "x".into(),
                    occurred_at: Some(2),
                    score: 0.98,
                    fingerprint: "fp-other-901".into(),
                },
            ],
            5,
        )
        .unwrap();
        let scan = db.begin_scan("m", (None, None), "t", 1).unwrap();
        // This scan's scope: one thread with one threat.
        let threads = vec![thread(&[(1, "a", "i will kill you")])];
        let classify = |w: &FocusWindow| -> Result<FocusOutcome> {
            assert!(
                w.items[w.focus].text.contains("kill"),
                "an out-of-scope census row reached the classifier"
            );
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        // Budget 1: if the (higher-scoring) out-of-scope rows were counted,
        // they would eat the whole budget and the real threat would be
        // reported as an unscanned tail.
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            Some(1),
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(out.candidates, 1, "candidates count THIS scope only");
        assert_eq!(out.deep_scanned, 1);
        assert_eq!(out.findings, 1);
        assert_eq!(out.unscanned(), 0, "nothing in scope was left unscanned");
    }

    /// The census store outlives re-imports; row ids do not. A stored row whose
    /// fingerprint no longer matches the message its id resolves to must be
    /// re-embedded (not skipped) and must never be deep-scanned as-is.
    #[test]
    fn a_stale_census_row_is_rescored_not_trusted() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        // Pre-import: id 1 was some hot OTHER message.
        db.record_census(
            &[CensusRow {
                source_id: 1,
                thread_identifier: "t".into(),
                sender: "a".into(),
                occurred_at: Some(1),
                score: 0.99,
                fingerprint: "fp-before-reimport".into(),
            }],
            5,
        )
        .unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        // Post-import: id 1 now belongs to harmless chatter.
        let threads = vec![thread(&[(1, "a", "grab milk")])];
        let mut embeds = 0usize;
        let counting_embed = |t: &str| {
            embeds += 1;
            embed(t)
        };
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            panic!("a stale census row was deep-scanned against the wrong message")
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            counting_embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            embeds, 1,
            "the reshuffled message is re-embedded, not skipped"
        );
        assert_eq!(out.findings, 0);
    }

    /// The dominant Stop path: the cancel-watcher kills the server under an
    /// in-flight call, which surfaces as an Err from classify. That must read
    /// as a clean cancel — with completed findings written — never a failure.
    #[test]
    fn a_kill_under_an_in_flight_classify_reads_as_cancelled() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![
            thread(&[(1, "a", "i will kill you")]),
            thread(&[(2, "a", "kill him too")]),
        ];
        let cancel = CancelToken::new();
        let cancel2 = cancel.clone();
        let calls = std::cell::Cell::new(0usize);
        let classify = move |_: &FocusWindow| -> Result<FocusOutcome> {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Ok(fo(vec![FocusVerdict {
                    category: Category::ThreatViolence,
                    severity: 3,
                    rationale: "x".into(),
                }]))
            } else {
                // Stop lands mid-request: the token flips AND the request dies.
                cancel2.cancel();
                Err(crate::Error::Inference("connection reset".into()))
            }
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &cancel,
            |_| {},
        )
        .expect("a Stop is a cancel, not a scan failure");
        assert!(out.cancelled);
        assert_eq!(out.findings, 1, "the completed finding survives the Stop");
    }

    /// A transient failure on one item is retried, then skipped — it must not
    /// discard everything the deep-scan already produced.
    #[test]
    fn one_failing_item_is_skipped_not_fatal() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![
            thread(&[(1, "a", "i will kill you")]),
            thread(&[(2, "a", "kill him too")]),
        ];
        let classify = |w: &FocusWindow| -> Result<FocusOutcome> {
            if w.items[w.focus].source_id == 2 {
                Err(crate::Error::Inference("hiccup".into()))
            } else {
                Ok(fo(vec![FocusVerdict {
                    category: Category::ThreatViolence,
                    severity: 3,
                    rationale: "x".into(),
                }]))
            }
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert!(!out.cancelled);
        assert_eq!(out.deep_scan_failed, 1);
        assert_eq!(out.findings, 1, "the healthy item's finding is kept");
    }

    /// EVERY item failing is the §10.6 harness-bug signature and must fail the
    /// scan loudly — zero findings from a dead harness must never look clean.
    #[test]
    fn all_items_failing_fails_the_scan() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![thread(&[(1, "a", "i will kill you")])];
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            Err(crate::Error::Inference("dead".into()))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        let err = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("harness"),
            "the error must point at the harness, got: {err}"
        );
    }

    /// One transiently-failing confirmation is retried then dropped — it must
    /// not abort the run and discard every already-vetted finding.
    #[test]
    fn one_failing_confirmation_is_dropped_not_fatal() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![
            thread(&[(1, "a", "i will kill you")]),
            thread(&[(2, "a", "kill him too")]),
        ];
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |w: &FocusWindow, _: &FocusVerdict| -> Result<bool> {
            if w.items[w.focus].source_id == 2 {
                Err(crate::Error::Inference("hiccup".into()))
            } else {
                Ok(true)
            }
        };
        let out = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Precise,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(out.confirm_failed, 1);
        assert_eq!(
            out.findings, 1,
            "the vetted finding is written; the failed one is dropped, not fatal"
        );
        assert_eq!(out.unconfirmed, 0, "a failure is not a veto");
    }

    /// Every confirmation failing is the §10.6 harness-bug signature.
    #[test]
    fn all_confirmations_failing_fails_the_scan() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![thread(&[(1, "a", "i will kill you")])];
        let classify = |_: &FocusWindow| -> Result<FocusOutcome> {
            Ok(fo(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }]))
        };
        let confirm = |_: &FocusWindow, _: &FocusVerdict| -> Result<bool> {
            Err(crate::Error::Inference("dead confirmer".into()))
        };
        let err = run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Precise,
            None,
            10,
            embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("harness"),
            "must point at the harness, got: {err}"
        );
    }

    #[test]
    fn a_second_run_reuses_the_census() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![thread(&[(1, "a", "i will kill you"), (2, "a", "hi")])];
        let mut embed_calls = 0;
        let counting_embed = |t: &str| {
            embed_calls += 1;
            embed(t)
        };
        let classify = |_: &FocusWindow| Ok(fo(vec![]));
        let confirm = |_: &FocusWindow, _: &FocusVerdict| Ok(true);
        run_triage(
            &mut db,
            scan,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            counting_embed,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let first = embed_calls;
        assert_eq!(first, 2, "both messages embedded on the first run");

        let scan2 = db.begin_scan("m", (None, None), "all", 2).unwrap();
        let counting_embed2 = |t: &str| {
            embed_calls += 1;
            embed(t)
        };
        run_triage(
            &mut db,
            scan2,
            &threads,
            &threat_proto(),
            ScanMode::Thorough,
            None,
            10,
            counting_embed2,
            classify,
            confirm,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            embed_calls, first,
            "second run re-embeds nothing — census is incremental"
        );
    }
}
