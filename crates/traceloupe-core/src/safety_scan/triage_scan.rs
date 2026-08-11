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
use crate::safety_scan::triage::{self, CensusInput, FocusWindow, ScanMode};
use crate::Result;

/// One verdict a focused classify call returns for the item it judged.
#[derive(Debug, Clone)]
pub struct FocusVerdict {
    pub category: Category,
    pub severity: u8,
    pub rationale: String,
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
}

impl TriageOutcome {
    /// Candidates a budget left unscanned — the tail the report must not call
    /// "clean". Zero means full coverage of everything above the threshold.
    pub fn unscanned(&self) -> usize {
        self.candidates.saturating_sub(self.deep_scanned)
    }
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
/// optional second opinion; it is only called when `mode.confirm()`.
#[allow(clippy::too_many_arguments)]
pub fn run_triage<E, C, F>(
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
) -> Result<TriageOutcome>
where
    E: FnMut(&str) -> Result<Vec<f32>>,
    C: FnMut(&FocusWindow) -> Result<Vec<FocusVerdict>>,
    F: FnMut(&FocusWindow, &FocusVerdict) -> Result<bool>,
{
    let mut out = TriageOutcome::default();

    // --- phase 1: census every message ---
    let flat: Vec<&CensusInput> = threads.iter().flatten().collect();
    let already = analysis.census_scored_ids()?;
    let todo: Vec<CensusInput> = flat
        .iter()
        .filter(|m| !already.contains(&m.source_id))
        .map(|m| (*m).clone())
        .collect();
    let scored = triage::census_messages(&todo, prototypes, &mut embed)?;
    let rows: Vec<CensusRow> = scored
        .iter()
        .map(|s| CensusRow {
            source_id: s.source_id,
            thread_identifier: s.thread_identifier.clone(),
            sender: s.sender.clone(),
            occurred_at: s.occurred_at,
            score: s.score as f64,
        })
        .collect();
    analysis.record_census(&rows, now)?;
    out.censused = flat.len();

    // --- phase 2: rank + budget ---
    let threshold = mode.census_threshold() as f64;
    out.candidates = analysis.triage_candidate_count(threshold)?;
    let worklist = analysis.triage_worklist(threshold, budget)?;

    // Locate a message by id: its thread and index within it, for the window.
    // Built once — a scan touches many work items.
    let mut locate: std::collections::HashMap<i64, (usize, usize)> =
        std::collections::HashMap::new();
    for (ti, thread) in threads.iter().enumerate() {
        for (mi, m) in thread.iter().enumerate() {
            locate.insert(m.source_id, (ti, mi));
        }
    }

    // --- phase 3: focused deep-scan the worklist ---
    let mut findings: Vec<NewFinding> = Vec::new();
    for item in &worklist {
        let Some(&(ti, mi)) = locate.get(&item.source_id) else {
            continue; // a census row whose message is no longer in scope
        };
        out.deep_scanned += 1;
        let window = triage::context_window(&threads[ti], mi, ScanMode::default_radius());
        let judged = &window.items[window.focus];
        for v in classify(&window)? {
            if mode.confirm() && !confirm(&window, &v)? {
                out.unconfirmed += 1;
                continue;
            }
            findings.push(NewFinding {
                source_kind: SourceKind::Message,
                source_id: Some(judged.source_id),
                thread_identifier: Some(judged.thread_identifier.clone()),
                occurred_at: judged.occurred_at,
                fingerprint: format!("triage:{}", judged.source_id),
                category: v.category,
                severity: v.severity,
                rationale: v.rationale,
                service: None,
                sender: Some(judged.sender.clone()),
                content_key: crate::safety_scan::content_key::content_key(&judged.text),
            });
        }
    }
    analysis.replace_findings(scan_id, &findings, now)?;
    out.findings = findings.len();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let classify = |w: &FocusWindow| -> Result<Vec<FocusVerdict>> {
            let judged = &w.items[w.focus];
            Ok(if judged.text.contains("kill") {
                vec![FocusVerdict {
                    category: Category::ThreatViolence,
                    severity: 3,
                    rationale: "threat".into(),
                }]
            } else {
                vec![]
            })
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
            Ok(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }])
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
            Ok(vec![FocusVerdict {
                category: Category::ThreatViolence,
                severity: 3,
                rationale: "x".into(),
            }])
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
    fn a_second_run_reuses_the_census() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let threads = vec![thread(&[(1, "a", "i will kill you"), (2, "a", "hi")])];
        let mut embed_calls = 0;
        let counting_embed = |t: &str| {
            embed_calls += 1;
            embed(t)
        };
        let classify = |_: &FocusWindow| Ok(vec![]);
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
        )
        .unwrap();
        assert_eq!(
            embed_calls, first,
            "second run re-embeds nothing — census is incremental"
        );
    }
}
