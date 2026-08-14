//! Validation harness (plan T10): a hand-labeled fixture set plus a scorer,
//! so a prompt change is measurable rather than vibes.
//!
//! Two layers:
//! - **Deterministic (CI)**: the fixtures parse, cover every Forensic 9
//!   category plus hard negatives, chunk cleanly, and the verdict-validation
//!   pipeline turns a labeled "model output" into exactly the labeled
//!   findings. This gates the code with no model present.
//! - **Live (manual / opt-in)**: [`score_against`] runs a real classifier over
//!   the fixtures and returns per-category precision/recall. The
//!   `eval_against_live_model` test drives a running llama-server when
//!   `TRACELOUPE_EVAL_MODEL` points at a GGUF; it is `#[ignore]` so CI skips
//!   it. See `docs/validation/safety-scan-validation.md`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::analysis::Category;

const CASES_JSON: &str = include_str!("../../fixtures/safety-scan/cases.json");

/// The hand-written expansion of the eval set, one JSONL file per group.
///
/// `cases.json` had 14 coercive-control conversations and 13 harassment ones,
/// which is far too few to compare two classifiers: exact binomial intervals on
/// 13/14 and 12/14 overlap almost completely, and detecting the differences
/// this project cares about needs on the order of 100 per category. Every
/// comparison made before this expansion was reading differences smaller than
/// the measurement error.
///
/// These are hand-written rather than generated, deliberately. The local model
/// available for generation scores 3/8 on relationship-harassment with perfect
/// context, so asking it to invent good examples of that behaviour — and then
/// grading the result — was circular.
const EVAL_JSONL: &[(&str, &str)] = &[
    (
        "coercive-control",
        include_str!("../../fixtures/safety-scan/eval/coercive-control.jsonl"),
    ),
    (
        "relationship-harassment",
        include_str!("../../fixtures/safety-scan/eval/relationship-harassment.jsonl"),
    ),
    (
        "threat-violence",
        include_str!("../../fixtures/safety-scan/eval/threat-violence.jsonl"),
    ),
    (
        "self-harm",
        include_str!("../../fixtures/safety-scan/eval/self-harm.jsonl"),
    ),
    (
        "other-categories",
        include_str!("../../fixtures/safety-scan/eval/other-categories.jsonl"),
    ),
    (
        "negatives",
        include_str!("../../fixtures/safety-scan/eval/negatives.jsonl"),
    ),
];

/// The hand-written TRAINING corpus. Separate from the eval set on purpose, and
/// guarded against overlapping it — see the test below.
const TRAIN_JSONL: &[(&str, &str)] = &[
    ("harassment", include_str!("../../fixtures/safety-scan/train/harassment.jsonl")),
    ("coercive-control", include_str!("../../fixtures/safety-scan/train/coercive-control.jsonl")),
    ("threat-violence", include_str!("../../fixtures/safety-scan/train/threat-violence.jsonl")),
    ("self-harm", include_str!("../../fixtures/safety-scan/train/self-harm.jsonl")),
    ("sexual-grooming", include_str!("../../fixtures/safety-scan/train/sexual-grooming.jsonl")),
    ("hate-scam-drugs", include_str!("../../fixtures/safety-scan/train/hate-scam-drugs.jsonl")),
    ("varied-structure", include_str!("../../fixtures/safety-scan/train/varied-structure.jsonl")),
];

/// Every message in the training corpus, for the contamination guard.
pub fn training_corpus_lines() -> Vec<String> {
    let mut out = Vec::new();
    for (group, body) in TRAIN_JSONL {
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("train/{group}.jsonl: {e}"));
            if let Some(msgs) = v.get("messages").and_then(|m| m.as_array()) {
                for m in msgs {
                    if let Some(t) = m.get("text").and_then(|t| t.as_str()) {
                        out.push(t.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Every eval case: the original `cases.json` plus the hand-written expansion.
///
/// This is what a measurement should score against. `load_fixtures()` remains
/// the narrower sealed set for the guards that were written against it.
pub fn load_all_eval_cases() -> Vec<Case> {
    let mut out = load_fixtures().cases;
    for (group, body) in EVAL_JSONL {
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let case: Case =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("eval/{group}.jsonl: {e}"));
            out.push(case);
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct Fixtures {
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
pub struct Case {
    pub id: String,
    pub kind: String, // "positive" | "negative"
    pub messages: Vec<FixtureMessage>,
    pub expect: Vec<Expectation>,
}

#[derive(Debug, Deserialize)]
pub struct FixtureMessage {
    pub sender: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct Expectation {
    pub category: String,
    #[serde(rename = "minSeverity")]
    pub min_severity: u8,
}

impl Case {
    /// Categories this case must produce (empty for hard negatives).
    pub fn expected_categories(&self) -> BTreeSet<Category> {
        self.expect
            .iter()
            .filter_map(|e| Category::parse(&e.category))
            .collect()
    }

    /// The severity each expected category must reach, so a "concerning"
    /// verdict cannot satisfy a fixture that demands "serious or imminent".
    pub fn min_severity(&self, cat: Category) -> u8 {
        self.expect
            .iter()
            .filter(|e| Category::parse(&e.category) == Some(cat))
            .map(|e| e.min_severity)
            .max()
            .unwrap_or(1)
    }

    /// A negative that no classifier can fail: every message is refused before
    /// a verdict survives validation. Counting these in the clean rate flatters
    /// it, because they cannot go wrong.
    pub fn is_structurally_clean(&self) -> bool {
        self.expect.is_empty()
            && self
                .messages
                .iter()
                .all(|m| crate::safety_scan::trivial::is_contentless(&m.text))
    }
}

/// One verdict as the ENGINE produces it: a category, a severity, and the item
/// it was attached to. Scoring on the category alone hid two things the app
/// depends on — the severity floor that decides whether a reviewer ever sees
/// the finding, and which message it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub category: Category,
    pub severity: u8,
    /// Index of the message the verdict was attached to, when known.
    pub item: Option<usize>,
}

/// The severity below which the app hides a finding by default (#450). The
/// eval applies it, or it reports detections no reviewer will ever see.
pub const DEFAULT_FLOOR: u8 = 2;

/// `cases.json` is the **sealed evaluation set**. Nothing a model learns from —
/// census prototypes, prompt examples, and above all a fine-tune's training
/// corpus — may contain its text, or every number measured against it is
/// train-on-test.
///
/// This project has hit that contamination three times: leave-one-out was
/// needed because the centroids were built from the eval fixtures (#489); the
/// corpus split that removed the need for it was itself guarded by an
/// equality check that passed while three prototype lines sat *inside* fixture
/// positives (#492); and a measurement was retracted for it before that. The
/// prototype corpus is checked with this function. **Any future training
/// corpus must be checked with it too** — the discipline only survives if
/// re-using it is easier than re-deriving it.
///
/// Returns the offending (fixture text, corpus line) pair, or `None` when the
/// corpus is clean.
pub fn overlaps_sealed_fixtures(corpus_lines: &[String]) -> Option<(String, String)> {
    /// A shared PHRASE, not a shared word. Matching raw substrings flagged the
    /// fixture "thanks ❤️" against any corpus line containing "thanks", so
    /// compare word sequences and require the shared run to be long enough to
    /// be text someone copied rather than vocabulary two sentences happen to
    /// share. Equality still fails at any length.
    const MIN_SHARED_WORDS: usize = 4;
    fn words(s: &str) -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }
    fn leaks(a: &[String], b: &[String]) -> bool {
        if a == b {
            return true;
        }
        let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
        short.len() >= MIN_SHARED_WORDS && long.windows(short.len()).any(|w| w == short)
    }
    let corpus: Vec<(Vec<String>, &String)> = corpus_lines
        .iter()
        .map(|l| (words(l), l))
        .filter(|(w, _)| !w.is_empty())
        .collect();
    for case in &load_all_eval_cases() {
        for m in &case.messages {
            let f = words(&m.text);
            if f.is_empty() {
                continue;
            }
            for (c, raw) in &corpus {
                if leaks(&f, c) {
                    return Some((m.text.clone(), (*raw).clone()));
                }
            }
        }
    }
    None
}

pub fn load_fixtures() -> Fixtures {
    serde_json::from_str(CASES_JSON).expect("cases.json is valid")
}

/// One category's confusion counts across all cases.
#[derive(Debug, Default, Clone, Copy)]
pub struct CategoryScore {
    pub tp: u32,
    pub fp: u32,
    pub fn_: u32,
}

impl CategoryScore {
    pub fn precision(&self) -> f64 {
        let d = self.tp + self.fp;
        if d == 0 {
            1.0
        } else {
            self.tp as f64 / d as f64
        }
    }
    pub fn recall(&self) -> f64 {
        let d = self.tp + self.fn_;
        if d == 0 {
            1.0
        } else {
            self.tp as f64 / d as f64
        }
    }
    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ScoreReport {
    pub per_category: BTreeMap<Category, CategoryScore>,
    /// Hard-negative cases that were (wrongly) flagged with any category.
    pub false_alarms: Vec<String>,
    pub cases_scored: usize,
    /// SEMANTIC hard negatives only — the ones a classifier could get wrong.
    /// Structurally-clean cases (all-emoji, refused before any verdict) are
    /// excluded, because including cases that cannot fail inflates the rate.
    pub negatives_scored: usize,
    /// Structurally-clean negatives, counted apart so the exclusion is visible.
    pub structural_negatives: usize,
    /// Detections that the app's severity floor HIDES: the right category at a
    /// severity the default view does not show. The eval used to count these as
    /// successes, so its recall described a product nobody uses.
    pub hidden_by_floor: Vec<String>,
    /// Right category, severity below what the fixture demands — a
    /// miscalibration rather than a miss, and worth seeing separately.
    pub under_severity: Vec<String>,
}

impl ScoreReport {
    /// Fraction of hard negatives that stayed clean (no category flagged).
    /// Divides by negatives only — a diluted "of all cases" rate would flatter
    /// the model.
    pub fn negative_clean_rate(&self) -> f64 {
        if self.negatives_scored == 0 {
            return 1.0;
        }
        (self.negatives_scored - self.false_alarms.len()) as f64 / self.negatives_scored as f64
    }

    pub fn table(&self) -> String {
        let mut out = String::from("category               precision  recall   f1\n");
        for (cat, s) in &self.per_category {
            out.push_str(&format!(
                "{:<22} {:>8.2} {:>8.2} {:>7.2}\n",
                cat.as_str(),
                s.precision(),
                s.recall(),
                s.f1()
            ));
        }
        out.push_str(&format!(
            "\nhard-negative clean rate: {:.2} ({} false alarms of {} SEMANTIC negatives; \
             {} structurally-clean cases excluded)\n",
            self.negative_clean_rate(),
            self.false_alarms.len(),
            self.negatives_scored,
            self.structural_negatives
        ));
        if !self.false_alarms.is_empty() {
            out.push_str(&format!("false alarms: {}\n", self.false_alarms.join(", ")));
        }
        if !self.hidden_by_floor.is_empty() {
            out.push_str(&format!(
                "found but HIDDEN by the severity floor ({}): {}\n",
                self.hidden_by_floor.len(),
                self.hidden_by_floor.join(", ")
            ));
        }
        if !self.under_severity.is_empty() {
            out.push_str(&format!(
                "under-severity ({}): {}\n",
                self.under_severity.len(),
                self.under_severity.join(", ")
            ));
        }
        out
    }
}

/// Score a classifier over every fixture. `classify` returns the set of
/// categories the classifier flagged for a case's messages. Pure function of
/// the classifier — the live test and the golden test share it.
pub fn score_against(
    fixtures: &Fixtures,
    classify: impl FnMut(&Case) -> BTreeSet<Category>,
) -> ScoreReport {
    // Category-only classifiers (the golden tests) come in through here; they
    // assert nothing about severity, so every verdict is given the floor.
    let mut c = classify;
    score_verdicts(fixtures, |case| {
        c(case)
            .into_iter()
            .map(|category| Verdict {
                category,
                severity: DEFAULT_FLOOR,
                item: None,
            })
            .collect()
    })
}

/// Score a classifier over every fixture, on what the APP would show.
///
/// Three things this does that scoring bare categories did not:
///
/// - **The severity floor is applied.** A verdict below [`DEFAULT_FLOOR`] is
///   not a detection, because the default view hides it. Those cases are
///   reported in `hidden_by_floor` rather than silently counted as successes.
/// - **Severity is compared to the fixture's `minSeverity`.** "Concerning" does
///   not satisfy a case that demands "serious or imminent"; that lands in
///   `under_severity`.
/// - **Structurally-clean negatives are excluded from the clean rate**, since a
///   case that cannot be failed does not measure anything.
pub fn score_verdicts(
    fixtures: &Fixtures,
    mut classify: impl FnMut(&Case) -> Vec<Verdict>,
) -> ScoreReport {
    let mut report = ScoreReport::default();
    for case in &fixtures.cases {
        let expected = case.expected_categories();
        let all = classify(case);
        // What a reviewer would actually be shown.
        let shown: BTreeSet<Category> = all
            .iter()
            .filter(|v| v.severity >= DEFAULT_FLOOR)
            .map(|v| v.category)
            .collect();
        let any: BTreeSet<Category> = all.iter().map(|v| v.category).collect();

        for cat in Category::ALL {
            let e = expected.contains(&cat);
            let p = shown.contains(&cat);
            let s = report.per_category.entry(cat).or_default();
            match (e, p) {
                (true, true) => s.tp += 1,
                (false, true) => s.fp += 1,
                (true, false) => s.fn_ += 1,
                (false, false) => {}
            }
            if e && !p && any.contains(&cat) {
                // Found, then hidden. Distinct from never finding it, and the
                // distinction decides whether the floor or the model is at
                // fault.
                report
                    .hidden_by_floor
                    .push(format!("{}:{}", case.id, cat.as_str()));
            }
            if e && p {
                let want = case.min_severity(cat);
                let got = all
                    .iter()
                    .filter(|v| v.category == cat)
                    .map(|v| v.severity)
                    .max()
                    .unwrap_or(0);
                if got < want {
                    report.under_severity.push(format!(
                        "{}:{} ({got} < {want})",
                        case.id,
                        cat.as_str()
                    ));
                }
            }
        }

        if expected.is_empty() {
            if case.is_structurally_clean() {
                report.structural_negatives += 1;
            } else {
                report.negatives_scored += 1;
                if !shown.is_empty() {
                    report.false_alarms.push(case.id.clone());
                }
            }
        }
        report.cases_scored += 1;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One spawn for every live `#[ignore]` test: the sandboxed server, the
    /// health wait, and a per-process scratch dir (a FIXED name in the shared
    /// temp root can already belong to another uid, and per-test names defeat
    /// the Metal shader cache). ServerConfig changes land here once instead of
    /// in hand-copied literals per test.
    ///
    /// Run live tests ONE AT A TIME (`cargo test <name> -- --ignored`): a bare
    /// `-- --ignored` loads several multi-GB models concurrently — an OOM risk
    /// on the 24 GB reference machine, and timings measured under GPU
    /// contention are not comparable to the recorded numbers.
    /// A cheap identity for a corpus of texts, so two harnesses can prove they
    /// are looking at the same messages instead of assuming it.
    fn bed_fingerprint(bed: &[String]) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for t in bed {
            h.update(t.as_bytes());
            h.update([1u8]);
        }
        format!("{:x}", h.finalize())[..12].to_string()
    }

    #[allow(dead_code)]
    fn spawn_live_server(
        model: &str,
        embedding: bool,
        ctx_size: u32,
    ) -> crate::safety_scan::server::LlamaServer {
        use crate::safety_scan::server::{pick_port, resolve_binary, LlamaServer, ServerConfig};
        let mut s = LlamaServer::spawn(
            &ServerConfig {
                binary: resolve_binary().expect("sidecar binary"),
                model_path: std::path::PathBuf::from(model),
                port: pick_port().unwrap(),
                ctx_size,
                parallel: 1,
                api_key: None,
                gpu_layers: -1,
                sandbox: true,
                embedding,
                scratch_dir: std::env::temp_dir()
                    .join(format!("traceloupe-eval-scratch-{}", std::process::id())),
            },
            None,
        )
        .expect("spawn");
        s.wait_healthy(std::time::Duration::from_secs(180))
            .expect("healthy");
        s
    }

    #[test]
    fn fixtures_parse_and_are_substantial() {
        let f = load_fixtures();
        assert!(f.cases.len() >= 60, "want a meaningful fixture set");
        let positives = f.cases.iter().filter(|c| c.kind == "positive").count();
        let negatives = f.cases.iter().filter(|c| c.kind == "negative").count();
        assert!(positives >= 10, "want >=10 positive cases");
        // Hard negatives are what the classifier actually fails, so they are
        // not a token few: they carry the clean rate the release gate turns on.
        assert!(negatives >= 20, "want >=20 hard negatives");
    }

    /// Not "at least one" — at least [`MIN_PER_CATEGORY`]. With one example a
    /// category's precision and recall can only be 0.00 or 1.00, so a single
    /// case swings the whole cell and the table reads far more precisely than
    /// it measures. That is how "harassment recall is 0.00" got reported before
    /// anyone could tell a model property from one unlucky sentence.
    #[test]
    fn every_category_has_enough_positives_to_mean_something() {
        const MIN_PER_CATEGORY: usize = 5;
        let f = load_fixtures();
        let mut per: BTreeMap<Category, usize> = BTreeMap::new();
        for c in &f.cases {
            for cat in c.expected_categories() {
                *per.entry(cat).or_default() += 1;
            }
        }
        for cat in Category::ALL {
            let n = per.get(&cat).copied().unwrap_or(0);
            assert!(
                n >= MIN_PER_CATEGORY,
                "{} has {n} positives, want at least {MIN_PER_CATEGORY}",
                cat.as_str()
            );
        }
    }

    #[test]
    fn expectations_and_kinds_are_consistent() {
        let f = load_fixtures();
        for c in &f.cases {
            assert!(
                c.kind == "positive" || c.kind == "negative",
                "{}: bad kind {}",
                c.id,
                c.kind
            );
            if c.kind == "negative" {
                assert!(
                    c.expect.is_empty(),
                    "{}: negative must expect nothing",
                    c.id
                );
            } else {
                assert!(
                    !c.expect.is_empty(),
                    "{}: positive must expect something",
                    c.id
                );
            }
            for e in &c.expect {
                assert!(
                    Category::parse(&e.category).is_some(),
                    "{}: bad category {}",
                    c.id,
                    e.category
                );
                assert!(
                    (1..=3).contains(&e.min_severity),
                    "{}: severity out of range",
                    c.id
                );
            }
            assert!(!c.messages.is_empty(), "{}: no messages", c.id);
        }
    }

    /// The emoji hard negatives are clean *by construction*, not by the
    /// model's good judgement: every message in them is refused before it can
    /// become a finding, so no classifier — however badly it behaves — can
    /// flag one. Editing one of these to contain words breaks that guarantee,
    /// and this test is what says so.
    ///
    /// The counters exist because a loop that silently matches nothing reports
    /// success identically to one that checked everything.
    #[test]
    fn the_emoji_negatives_are_unflaggable_by_construction() {
        use crate::safety_scan::trivial;
        let f = load_fixtures();
        const EMOJI_NEGATIVES: &[&str] = &["neg-emoji-affection", "neg-emoji-run"];

        let mut cases_seen = 0;
        let mut messages_checked = 0;
        for c in f
            .cases
            .iter()
            .filter(|c| EMOJI_NEGATIVES.contains(&c.id.as_str()))
        {
            cases_seen += 1;
            for m in &c.messages {
                assert!(
                    trivial::is_contentless(&m.text),
                    "{}: {:?} could still be flagged",
                    c.id,
                    m.text
                );
                messages_checked += 1;
            }
        }
        assert_eq!(cases_seen, EMOJI_NEGATIVES.len(), "a fixture went missing");
        assert!(
            messages_checked >= 7,
            "expected to check every message, saw {messages_checked}"
        );
    }

    /// The filter must not become an escape hatch. A threat that happens to
    /// carry an emoji, and a bare weapon emoji, both stay classifiable.
    #[test]
    fn an_emoji_does_not_make_a_threat_unflaggable() {
        use crate::safety_scan::trivial;
        let f = load_fixtures();
        let c = f
            .cases
            .iter()
            .find(|c| c.id == "threat-with-emoji")
            .expect("threat-with-emoji fixture");
        assert!(!c.expect.is_empty(), "it is a positive case");
        for m in &c.messages {
            assert!(
                !trivial::is_contentless(&m.text),
                "{:?} was filtered out of a positive case",
                m.text
            );
        }
    }

    /// A classifier that finds everything but calls it all "concerning" scores
    /// ZERO, because the app hides that tier. The old scorer gave it full
    /// marks — it counted detections no reviewer would ever be shown.
    #[test]
    fn detections_below_the_floor_are_not_detections() {
        let f = load_fixtures();
        let report = score_verdicts(&f, |c| {
            c.expected_categories()
                .into_iter()
                .map(|category| Verdict {
                    category,
                    severity: 1,
                    item: None,
                })
                .collect()
        });
        for (cat, s) in &report.per_category {
            assert_eq!(s.recall(), 0.0, "{} counted a hidden finding", cat.as_str());
        }
        assert!(
            !report.hidden_by_floor.is_empty(),
            "and it says WHICH were found then hidden"
        );
    }

    /// "Concerning" must not satisfy a fixture demanding "serious or imminent".
    #[test]
    fn a_severity_below_the_fixture_is_reported_as_miscalibrated() {
        let f = load_fixtures();
        let report = score_verdicts(&f, |c| {
            c.expected_categories()
                .into_iter()
                .map(|category| Verdict {
                    category,
                    severity: 2,
                    item: None,
                })
                .collect()
        });
        let want3 = f
            .cases
            .iter()
            .filter(|c| c.expect.iter().any(|e| e.min_severity == 3))
            .count();
        assert!(want3 > 0, "fixtures must contain severity-3 expectations");
        assert_eq!(
            report.under_severity.len(),
            want3,
            "every severity-3 case flagged at 2 is reported"
        );
    }

    /// The clean rate must not count cases that cannot fail. The emoji
    /// negatives are refused before any verdict exists, so including them
    /// inflates the number the release gate turns on.
    #[test]
    fn structurally_clean_negatives_are_excluded_from_the_clean_rate() {
        let f = load_fixtures();
        let report = score_verdicts(&f, |_| Vec::new());
        assert!(
            report.structural_negatives >= 2,
            "the emoji negatives are structural, found {}",
            report.structural_negatives
        );
        let semantic = f
            .cases
            .iter()
            .filter(|c| c.expect.is_empty() && !c.is_structurally_clean())
            .count();
        assert_eq!(report.negatives_scored, semantic);
    }

    /// The golden path with NO model: a perfect classifier (labels → itself)
    /// scores 1.0 everywhere and raises no false alarm. This guards `score_against`
    /// and proves the fixtures are internally consistent.
    #[test]
    fn perfect_classifier_scores_perfectly() {
        let f = load_fixtures();
        let report = score_against(&f, |c| c.expected_categories());
        for (cat, s) in &report.per_category {
            assert!(
                (s.precision() - 1.0).abs() < 1e-9,
                "{}: precision {}",
                cat.as_str(),
                s.precision()
            );
            assert!(
                (s.recall() - 1.0).abs() < 1e-9,
                "{}: recall {}",
                cat.as_str(),
                s.recall()
            );
        }
        assert!(report.false_alarms.is_empty());
        assert!((report.negative_clean_rate() - 1.0).abs() < 1e-9);
        assert!(report.negatives_scored >= 5);
    }

    /// A cry-wolf classifier that flags harassment on everything tanks
    /// precision and lights up every hard negative as a false alarm —
    /// confirming the scorer actually penalizes over-flagging.
    #[test]
    fn overflagging_classifier_is_penalized() {
        let f = load_fixtures();
        let report = score_against(&f, |_| {
            let mut s = BTreeSet::new();
            s.insert(Category::HarassmentBullying);
            s
        });
        let h = report.per_category[&Category::HarassmentBullying];
        assert!(h.precision() < 0.5, "over-flagging must hurt precision");
        assert!(
            !report.false_alarms.is_empty(),
            "hard negatives must register as false alarms"
        );
    }

    /// Live end-to-end eval against a real model. Ignored by default (needs a
    /// multi-GB GGUF); run with:
    ///   TRACELOUPE_EVAL_MODEL=/path/model.gguf \
    ///   TRACELOUPE_LLAMA_SERVER=/path/llama-server \
    ///   cargo test -p traceloupe-core eval_against_live_model -- --ignored --nocapture
    #[test]
    #[ignore = "requires a local GGUF + llama-server (set TRACELOUPE_EVAL_MODEL)"]
    fn eval_against_live_model() {
        use crate::safety_scan::chunker::{Chunk, ChunkItem};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::{engine, prompt};
        use std::path::PathBuf;
        use std::time::Duration;

        let Ok(model) = std::env::var("TRACELOUPE_EVAL_MODEL") else {
            eprintln!("set TRACELOUPE_EVAL_MODEL to run the live eval");
            return;
        };
        let model = PathBuf::from(model);
        let binary = crate::safety_scan::server::resolve_binary()
            .expect("set TRACELOUPE_LLAMA_SERVER or bundle a sidecar");
        let port = crate::safety_scan::server::pick_port().unwrap();
        let mut server = crate::safety_scan::server::LlamaServer::spawn(
            &crate::safety_scan::server::ServerConfig {
                binary,
                model_path: model,
                port,
                ctx_size: 8192,
                parallel: 1,
                api_key: None,
                gpu_layers: -1,
                embedding: false,
                sandbox: true,
                scratch_dir: std::env::temp_dir().join("traceloupe-eval-scratch"),
            },
            None,
        )
        .expect("spawn llama-server");
        server
            .wait_healthy(Duration::from_secs(180))
            .expect("model load");
        let client = LlmClient::new(server.base_url(), "eval", Duration::from_secs(300));

        // Production sends WINDOW-message chunks; a fixture case is 2-4
        // messages. Scoring only the short shape measures an input the app
        // never produces — the same dilution that turned a prefilter's 52%
        // into 15%. TRACELOUPE_EVAL_CHUNKED=1 buries each case in ordinary
        // conversation to the real window size and scores that instead.
        let chunked = std::env::var("TRACELOUPE_EVAL_CHUNKED").is_ok();
        // WINDOW is a choice, not a constant of nature. Sweeping it is the
        // point: if a smaller window detects more, the app should change.
        let window: usize = std::env::var("TRACELOUPE_EVAL_WINDOW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(crate::safety_scan::chunker::WINDOW);
        const BED: &[&str] = &[
            "are you still coming over later",
            "yeah should be there around seven",
            "no rush, dinner is not until eight",
            "did you pick up the thing for your mum",
            "it is in the car so i do not forget",
            "how did the meeting go this morning",
            "long, three hours for something that was an email",
            "that sounds about right honestly",
            "at least it is done now",
            "want me to bring wine",
            "red or white, i never remember",
            "just get whatever, it does not matter",
            "the train was delayed again",
            "did you see the score last night",
            "only the highlights, was working late",
            "i will send you the clip",
            "how is your sister after the move",
            "settling in, the flat is smaller than she hoped",
            "at least it is closer to work",
            "booked the tickets for saturday",
            "cannot wait honestly",
            "finished that book you lent me",
            "what did you make of the ending",
            "started running again this morning",
            "that is more than i manage",
        ];

        let fixtures = load_fixtures();
        let report = score_verdicts(&fixtures, |case| {
            // Build a single chunk from the case's messages, classify it, and
            // collapse verdicts to the set of categories seen.
            // The case's own messages, optionally bedded into ordinary
            // conversation so the model sees a production-shaped chunk.
            let texts: Vec<(String, String)> = if chunked {
                // Bed the case in ordinary conversation, centred, to `window`
                // messages. Centring matters: a case at the very start or end
                // of the window is a different test from one surrounded.
                let room = window.saturating_sub(case.messages.len());
                let before = room / 2;
                let mut v: Vec<(String, String)> = Vec::new();
                for (i, t) in BED.iter().take(before).enumerate() {
                    v.push((if i % 2 == 0 { "them" } else { "me" }.into(), (*t).into()));
                }
                for m in &case.messages {
                    v.push((m.sender.clone(), m.text.clone()));
                }
                for (i, t) in BED.iter().skip(before).enumerate() {
                    if v.len() >= window {
                        break;
                    }
                    v.push((if i % 2 == 0 { "them" } else { "me" }.into(), (*t).into()));
                }
                v.truncate(window.max(case.messages.len()));
                v
            } else {
                case.messages
                    .iter()
                    .map(|m| (m.sender.clone(), m.text.clone()))
                    .collect()
            };
            let items: Vec<ChunkItem> = texts
                .iter()
                .enumerate()
                .map(|(i, (sender, text))| ChunkItem {
                    source_id: i as i64,
                    sender: if sender == "me" {
                        "me".into()
                    } else {
                        "them".into()
                    },
                    occurred_at: Some(1000 + i as i64),
                    text: text.clone(),
                    fingerprint: format!("{}:{i}", case.id),
                })
                .collect();
            let chunk = Chunk {
                key: case.id.clone(),
                fingerprint: case.id.clone(),
                kind: crate::analysis::SourceKind::Message,
                thread_identifier: Some(case.id.clone()),
                label: None,
                service: None,
                items,
            };
            let user = prompt::render_chunk(&chunk);
            let grammar = prompt::verdicts_grammar(chunk.items.len());
            match client.chat_json(prompt::SYSTEM_PROMPT, &user, &grammar, 1200) {
                // Severity and item index come through now: the floor decides
                // what a reviewer sees, and the item decides where the app
                // deep-links to.
                Ok(output) => engine::verdicts_to_findings_for_eval(&chunk, &output)
                    .into_iter()
                    .map(|f| Verdict {
                        category: f.category,
                        severity: f.severity,
                        item: f.source_id.map(|i| i as usize),
                    })
                    .collect(),
                Err(e) => {
                    eprintln!("{}: {e}", case.id);
                    Vec::new()
                }
            }
        });
        server.shutdown();
        println!(
            "\n=== Safety Scan live eval ({}) ===\n{}",
            if chunked {
                "production 25-message chunks"
            } else {
                "short fixture cases"
            },
            report.table()
        );
        // A release gate could assert per-category recall/precision floors
        // here; left as a print so a human reviews the numbers first.
    }

    /// The triage census path, end to end: the sandboxed sidecar serves
    /// embeddings, and `embed()` returns a vector that puts a threat nearer a
    /// threat than a grocery run. Ignored like the other live tests.
    ///
    ///   TRACELOUPE_EMBED_MODEL=~/.../embeddinggemma-300M-Q8_0.gguf \
    ///   TRACELOUPE_LLAMA_SERVER=~/.../llama-server \
    ///   cargo test -p traceloupe-core embed_discriminates -- --ignored --nocapture
    #[test]
    #[ignore = "requires the embedding GGUF (set TRACELOUPE_EMBED_MODEL)"]
    fn embed_discriminates() {
        use crate::safety_scan::client::LlmClient;
        use std::path::PathBuf;
        use std::time::Duration;
        let Ok(model) = std::env::var("TRACELOUPE_EMBED_MODEL") else {
            eprintln!("set TRACELOUPE_EMBED_MODEL");
            return;
        };
        let binary = crate::safety_scan::server::resolve_binary().expect("sidecar");
        let port = crate::safety_scan::server::pick_port().unwrap();
        let mut server = crate::safety_scan::server::LlamaServer::spawn(
            &crate::safety_scan::server::ServerConfig {
                binary,
                model_path: PathBuf::from(model),
                port,
                ctx_size: 2048,
                parallel: 1,
                api_key: None,
                gpu_layers: -1,
                sandbox: true,
                embedding: true,
                scratch_dir: std::env::temp_dir().join("traceloupe-embed-scratch"),
            },
            None,
        )
        .expect("spawn");
        server.wait_healthy(Duration::from_secs(120)).expect("load");
        let c = LlmClient::new(server.base_url(), "embed", Duration::from_secs(60));
        let pfx = "task: classification | query: ";
        let threat = c
            .embed(&format!(
                "{pfx}i know where you live and im going to make you regret this"
            ))
            .unwrap();
        let threat2 = c
            .embed(&format!(
                "{pfx}you will pay for what you did, i will find you"
            ))
            .unwrap();
        let grocery = c
            .embed(&format!("{pfx}can you grab milk on the way home"))
            .unwrap();
        server.shutdown();

        let cos = |a: &[f32], b: &[f32]| {
            let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            d / (na * nb)
        };
        let same = cos(&threat, &threat2);
        let diff = cos(&threat, &grocery);
        println!("threat~threat {same:.3}  threat~grocery {diff:.3}");
        assert_eq!(threat.len(), 768, "EmbeddingGemma is 768-dim");
        assert!(
            same > diff + 0.05,
            "two threats must be nearer than a threat and a grocery run"
        );
    }

    /// The whole triage data path from a REAL cache store: a seeded fixture
    /// `CacheDb` → `census_threads` (the production reader, with its
    /// fingerprints, senders and services) → prototypes from the committed
    /// fixture positives → `run_triage` with live sidecars. This is the half
    /// of journey §8's re-measure item the corpus-parity test cannot cover —
    /// it proves the message store and the pipeline agree end to end, not just
    /// that the pipeline works on hand-built inputs.
    ///
    /// Synthetic messages only; no backup, real or otherwise, is involved.
    ///
    ///   TRACELOUPE_EMBED_MODEL=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
    ///   TRACELOUPE_EVAL_MODEL=~/.../gemma-4-E4B-it-Q4_K_M.gguf \
    ///   cargo test -p traceloupe-core triage_from_a_fixture_cache -- --ignored --nocapture
    #[test]
    #[ignore = "requires both GGUFs (set TRACELOUPE_EVAL_MODEL, TRACELOUPE_EMBED_MODEL)"]
    fn triage_from_a_fixture_cache() {
        use crate::analysis::{AnalysisDb, Category};
        use crate::safety_scan::chunker::{self, message_fingerprint, ScanSources, TimeRange};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{self, FocusWindow, ScanMode};
        use crate::safety_scan::triage_scan::{self, FocusVerdict};
        use crate::sidecar::CancelToken;
        use std::cell::RefCell;
        use std::time::Duration;

        let (Ok(embed_model), Ok(class_model)) = (
            std::env::var("TRACELOUPE_EMBED_MODEL"),
            std::env::var("TRACELOUPE_EVAL_MODEL"),
        ) else {
            eprintln!("set TRACELOUPE_EMBED_MODEL and TRACELOUPE_EVAL_MODEL");
            return;
        };

        // Three conversations of generated chatter with one planted threat —
        // the sentence the dilution work proved a batch scan misses at
        // production shape (validation doc), which makes it the right canary.
        const THREAT: &str = "i know where you live and im going to make you regret this";
        let cache = chunker::tests::cache_with(&[
            (
                "family-chat",
                1_000,
                "can you grab milk on the way home",
                false,
            ),
            ("family-chat", 1_010, "sure, anything else", true),
            (
                "family-chat",
                1_020,
                "maybe bread if the good one is in",
                false,
            ),
            ("family-chat", 1_030, "will do", true),
            ("family-chat", 1_040, "trains are delayed again", false),
            ("family-chat", 1_050, "classic. see you around seven", true),
            ("hostile", 2_000, "you were at the game last night", false),
            ("hostile", 2_010, "yeah great match", true),
            ("hostile", 2_020, THREAT, false),
            ("hostile", 2_030, "what are you talking about", true),
            ("hostile", 2_040, "you know exactly what you did", false),
            ("work", 3_000, "standup moved to ten tomorrow", false),
            ("work", 3_010, "thanks, i'll update the invite", true),
            ("work", 3_020, "deploy went out clean", false),
            ("work", 3_030, "great, closing the ticket", true),
        ]);

        // The production reader over the real store: fingerprints, senders and
        // services come from the cache rows, exactly as run_triage_scan reads
        // them.
        // Mirror the production command's scope handling exactly: it forces
        // notes off before computing the slug it stores on the scan row, so
        // this fixture certifies the same recorded scope the triage path
        // actually produces.
        let scan_sources = ScanSources {
            notes: false,
            ..Default::default()
        };
        let threads = chunker::census_threads(&cache, TimeRange::default(), &scan_sources).unwrap();
        let total: usize = threads.iter().map(|t| t.len()).sum();
        assert_eq!(total, 15, "the reader sees every seeded message");

        let spawn = spawn_live_server;

        // Prototypes exactly as the command builds them: fixture positives,
        // all categories.
        let server = spawn(&embed_model, true, 2048);
        let ec = LlmClient::new(server.base_url(), "embed", Duration::from_secs(300));
        let examples = triage_scan::prototype_examples(&Category::ALL);
        let prototypes = triage::build_prototypes(&examples, |t| ec.embed(t)).expect("prototypes");

        let slot = RefCell::new((server, ec, false));
        let embed = |t: &str| slot.borrow().1.embed(t);
        let classify = |w: &FocusWindow| {
            {
                let mut s = slot.borrow_mut();
                if !s.2 {
                    s.0.shutdown();
                    let srv = spawn(&class_model, false, 8192);
                    let c = LlmClient::new(srv.base_url(), "eval", Duration::from_secs(300));
                    *s = (srv, c, true);
                }
            }
            triage_scan::classify_focused(&slot.borrow().1, w)
        };

        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("fixture-e2e", (None, None), &scan_sources.slug(), 1)
            .unwrap();
        let out = triage_scan::run_triage(
            &mut db,
            scan,
            &threads,
            &prototypes,
            ScanMode::Thorough,
            ScanMode::Thorough.census_threshold(),
            None,
            1,
            embed,
            classify,
            |_: &FocusWindow, _: &FocusVerdict| Ok(true),
            &CancelToken::new(),
            |_| {},
        )
        .expect("run_triage");
        // NOT shut down yet: the optional confirm phase below reuses the
        // resident classifier — tearing it down here made every classify call
        // in run 2 fail, which the §10.6 all-failed guard turned into a loud
        // error (working as designed; the teardown moved, not the guard).

        println!(
            "fixture e2e: censused {} candidates {} deep-scanned {} findings {} unscanned {}",
            out.censused,
            out.candidates,
            out.deep_scanned,
            out.findings,
            out.unscanned()
        );
        assert_eq!(out.censused, 15);
        assert_eq!(out.unscanned(), 0, "no budget — every candidate read");

        // The planted threat is found, and its finding carries the DURABLE
        // identity and metadata straight from the cache row — the contract
        // dismissals and deep-links depend on.
        let findings = db.list_findings(Some(scan)).unwrap();
        let threat_fp = message_fingerprint("hostile", Some(2_020), "them", THREAT);
        let hit = findings
            .iter()
            // Fingerprint AND category: one message can yield verdicts in
            // several categories (sharing the fingerprint), and list_findings
            // orders by severity — fingerprint alone could return a sibling
            // category and fail a run that actually succeeded.
            .find(|f| f.fingerprint == threat_fp && f.category == Category::ThreatViolence)
            .unwrap_or_else(|| {
                panic!(
                    "the planted threat was not found; findings: {:?}",
                    findings
                        .iter()
                        .map(|f| (&f.thread_identifier, &f.category))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(hit.thread_identifier.as_deref(), Some("hostile"));
        assert_eq!(hit.sender.as_deref(), Some("them"));
        let svc: Option<String> = db
            .conn()
            .query_row(
                "SELECT service FROM content_findings WHERE id = ?1",
                [hit.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            svc.as_deref(),
            Some("SMS"),
            "service flows from the thread row"
        );

        // ---- optional: the CONFIRM phase from the same store ----
        // Set TRIAGE_GUARD_MODEL to also run a Precise-mode scan whose
        // confirm closure is the real guard::confirm_focused — store-to-
        // findings coverage of phase 4 (second swap, veto accounting), which
        // Thorough skips by design. The census is incremental: nothing is
        // re-embedded.
        if let Ok(guard_model) = std::env::var("TRIAGE_GUARD_MODEL") {
            let scan2 = db
                .begin_scan("fixture-e2e-precise", (None, None), &scan_sources.slug(), 2)
                .unwrap();
            let classify2 = |w: &FocusWindow| {
                // The classifier is already resident from the Thorough run.
                triage_scan::classify_focused(&slot.borrow().1, w)
            };
            let confirm2 = |w: &FocusWindow, _: &FocusVerdict| {
                {
                    let mut sl = slot.borrow_mut();
                    if sl.2 {
                        // Still the classifier: swap to Guard on the first
                        // confirm (run_triage's phase batching guarantees the
                        // classify phase is over). The bool doubles as the
                        // "swap once" latch.
                        sl.0.shutdown();
                        let srv = spawn(&guard_model, false, 16384);
                        let c = LlmClient::new(srv.base_url(), "guard", Duration::from_secs(300));
                        *sl = (srv, c, false);
                    }
                }
                crate::safety_scan::guard::confirm_focused(&slot.borrow().1, w)
            };
            let out2 = triage_scan::run_triage(
                &mut db,
                scan2,
                &threads,
                &prototypes,
                ScanMode::Precise,
                ScanMode::Precise.census_threshold(),
                None,
                2,
                |_: &str| -> crate::Result<Vec<f32>> {
                    panic!("the incremental census must leave zero embed work")
                },
                classify2,
                confirm2,
                &CancelToken::new(),
                |_| {},
            )
            .expect("run_triage precise");
            println!(
                "fixture e2e confirm: findings {} unconfirmed {} confirm_failed {}",
                out2.findings, out2.unconfirmed, out2.confirm_failed
            );
            assert_eq!(
                out2.confirm_failed, 0,
                "the confirmer answered every finding"
            );
            let confirmed = db.list_findings(Some(scan2)).unwrap();
            assert!(
                confirmed
                    .iter()
                    .any(|f| f.fingerprint == threat_fp && f.category == Category::ThreatViolence),
                "the planted threat must survive Guard confirmation from the store"
            );
        }
        slot.borrow_mut().0.shutdown();
    }

    /// A long message must embed, not kill the scan (#485).
    ///
    /// llama-server cannot split a pooled embedding across physical batches and
    /// answers HTTP 500 above one — default 512 tokens. The census feeds it
    /// whole messages, so before the sidecar was started with its batches sized
    /// to the context, the first long message in a real conversation aborted
    /// the entire triage run mid-census. This drives the PRODUCTION spawn path,
    /// so it fails if those flags are ever dropped.
    ///
    ///   TRACELOUPE_EMBED_MODEL=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
    ///   cargo test -p traceloupe-core a_long_message_still_embeds -- --ignored --nocapture
    #[test]
    #[ignore = "requires the embedding GGUF (set TRACELOUPE_EMBED_MODEL)"]
    fn a_long_message_still_embeds() {
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{EMBED_MAX_BYTES, EMBED_PREFIX};
        use std::time::Duration;

        let Ok(model) = std::env::var("TRACELOUPE_EMBED_MODEL") else {
            eprintln!("set TRACELOUPE_EMBED_MODEL");
            return;
        };
        let mut server = spawn_live_server(&model, true, 2048);
        let c = LlmClient::new(server.base_url(), "embed", Duration::from_secs(120));

        // DENSE text at the cap, not prose: measured, prose embeds fine at
        // 4,000 chars while dense ASCII fails between 2,000 and 3,000, so a
        // prose probe would pass even with the cap set wrongly and prove
        // nothing. This is the worst case the census can actually be handed.
        let dense = "aB3$x9-Zq7#Lm2/Kp5!Wn8&Rt4".repeat(EMBED_MAX_BYTES / 26 + 1);
        let text: String = dense.chars().take(EMBED_MAX_BYTES).collect();
        let v = c.embed(&format!("{EMBED_PREFIX}{text}"));
        server.shutdown();
        let v = v.unwrap_or_else(|e| {
            panic!(
                "a {}-char message failed to embed ({e}) — the sidecar's physical batch is \
                 too small again, and one long message will kill a whole scan (#485)",
                text.chars().count()
            )
        });
        assert_eq!(v.len(), 768, "EmbeddingGemma is 768-dim");
    }

    /// The pipeline against a REAL device's data: import a public DFIR research
    /// image (Joshua Hickman / Digital Corpora — published for unrestricted
    /// research use, and the corpus this repo validates parsers against), then
    /// census and deep-scan its actual message history.
    ///
    /// Every other measurement so far ran on either the Jigsaw corpus (5-message
    /// synthetic chunks) or a 15-message fixture. This is the first time the
    /// triage pipeline meets a device's real conversation volume and shape, so
    /// it answers two questions nothing else can: how the census threshold
    /// behaves on real chatter, and what a whole-backup scan actually costs.
    ///
    /// The owner's own backup is off-limits (AGENTS.md); this test takes a path
    /// and is meant for the public images under
    /// `scripts/fetch-test-image.sh --list`.
    ///
    ///   TRIAGE_REAL_BACKUP=/path/to/unpacked/<udid> \
    ///   TRIAGE_REAL_PASSWORD=MyPassword123 \
    ///   TRACELOUPE_EMBED_MODEL=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
    ///   TRACELOUPE_EVAL_MODEL=~/.../gemma-4-E4B-it-Q4_K_M.gguf \
    ///   cargo test -p traceloupe-core triage_on_a_public_research_image -- --ignored --nocapture
    #[test]
    #[ignore = "requires a public DFIR image + both GGUFs (set TRIAGE_REAL_BACKUP)"]
    fn triage_on_a_public_research_image() {
        use crate::analysis::{AnalysisDb, Category};
        use crate::cache::CacheDb;
        use crate::safety_scan::chunker::{self, ScanSources, TimeRange};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{self, FocusWindow, ScanMode};
        use crate::safety_scan::triage_scan::{self, FocusVerdict, TriageProgress};
        use crate::sidecar::CancelToken;
        use std::cell::RefCell;
        use std::time::{Duration, Instant};

        let (Ok(backup), Ok(embed_model), Ok(class_model)) = (
            std::env::var("TRIAGE_REAL_BACKUP"),
            std::env::var("TRACELOUPE_EMBED_MODEL"),
            std::env::var("TRACELOUPE_EVAL_MODEL"),
        ) else {
            eprintln!("set TRIAGE_REAL_BACKUP, TRACELOUPE_EMBED_MODEL, TRACELOUPE_EVAL_MODEL");
            return;
        };
        let password = std::env::var("TRIAGE_REAL_PASSWORD").unwrap_or_default();
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");

        // --- import, exactly as the app does (native parsers, no iLEAPP) ---
        let t0 = Instant::now();
        let outcome = crate::import::import_backup(
            None,
            std::path::Path::new(&backup),
            &password,
            &cache_path,
            &dir.path().join("work"),
            &["messages".to_string()],
            false,
            false,
            &CancelToken::new(),
            |_| {},
        )
        .expect("import the public image");
        let import_s = t0.elapsed().as_secs_f64();
        let cache = CacheDb::open(&cache_path).unwrap();
        let threads =
            chunker::census_threads(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
        let total: usize = threads.iter().map(|t| t.len()).sum();
        let _ = &outcome.cache_path;
        println!(
            "imported in {import_s:.0}s — {} threads, {total} messages in scope",
            threads.len()
        );
        assert!(total > 0, "the image has readable messages");

        // --- census + deep-scan with live sidecars ---
        let server = spawn_live_server(&embed_model, true, 2048);
        let ec = LlmClient::new(server.base_url(), "embed", Duration::from_secs(300));
        let prototypes =
            triage::build_prototypes(&triage_scan::prototype_examples(&Category::ALL), |t| {
                ec.embed(t)
            })
            .expect("prototypes");

        let slot = RefCell::new((server, ec, false));
        // DIAGNOSTIC: report the shape of anything the embedder rejects, so a
        // failure names its cause instead of a bare HTTP code.
        let embed = |t: &str| {
            let r = slot.borrow().1.embed(t);
            if r.is_err() {
                let chars = t.chars().count();
                let bytes = t.len();
                let ascii = t.chars().filter(|c| c.is_ascii()).count();
                eprintln!(
                    "EMBED FAILED: chars={chars} bytes={bytes} ascii={ascii}                      ({}% non-ascii, {:.2} bytes/char)",
                    100 - (100 * ascii / chars.max(1)),
                    bytes as f64 / chars.max(1) as f64
                );
            }
            r
        };
        let classify = |w: &FocusWindow| {
            {
                let mut s = slot.borrow_mut();
                if !s.2 {
                    s.0.shutdown();
                    let srv = spawn_live_server(&class_model, false, 8192);
                    let c = LlmClient::new(srv.base_url(), "eval", Duration::from_secs(300));
                    *s = (srv, c, true);
                }
            }
            let r = triage_scan::classify_focused(&slot.borrow().1, w);
            if r.is_err() {
                let total: usize = w.items.iter().map(|i| i.text.chars().count()).sum();
                eprintln!(
                    "CLASSIFY FAILED: window of {} items, {total} chars",
                    w.items.len()
                );
            }
            r
        };

        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("real-image", (None, None), "messages", 1)
            .unwrap();
        let census_start = Instant::now();
        let mut census_done_at = None;
        let t1 = Instant::now();
        let out = triage_scan::run_triage(
            &mut db,
            scan,
            &threads,
            &prototypes,
            ScanMode::Thorough,
            ScanMode::Thorough.census_threshold(),
            // A budget keeps this a bounded measurement rather than an
            // overnight run; the unscanned tail is the point of the coverage
            // line and is reported below.
            Some(
                std::env::var("TRIAGE_REAL_BUDGET")
                    .ok()
                    .and_then(|b| b.parse().ok())
                    .unwrap_or(40),
            ),
            1,
            embed,
            classify,
            |_: &FocusWindow, _: &FocusVerdict| Ok(true),
            &CancelToken::new(),
            |p: TriageProgress| match p {
                TriageProgress::Census { done, total } => {
                    if done > 0 && done == total {
                        census_done_at = Some(census_start.elapsed().as_secs_f64());
                    }
                    if done % 2000 == 0 && done > 0 {
                        println!("  census {done}/{total}");
                    }
                }
                TriageProgress::DeepScan { done, total, .. } => {
                    if done % 10 == 0 && done > 0 {
                        println!("  deep-scan {done}/{total}");
                    }
                }
                TriageProgress::Confirm { .. } => {}
            },
        )
        .expect("run_triage on real data");
        let scan_s = t1.elapsed().as_secs_f64();
        slot.borrow_mut().0.shutdown();

        let census_s = census_done_at.unwrap_or(scan_s);
        println!(
            "REAL IMAGE: censused {} in {:.0}s ({:.0} msg/s) · candidates {} ({:.2}% of scope) \
             · deep-scanned {} · findings {} · unscanned {} · total {:.0}s",
            out.censused,
            census_s,
            out.censused as f64 / census_s.max(0.001),
            out.candidates,
            100.0 * out.candidates as f64 / out.censused.max(1) as f64,
            out.deep_scanned,
            out.findings,
            out.unscanned(),
            scan_s
        );
        // No quality assertion: this image's messages carry no ground truth, so
        // the value here is the SHAPE — volume, selectivity, throughput — not a
        // recall number. What must hold is that the pipeline ran end to end on
        // real data and the census actually discriminated.
        assert_eq!(out.censused, total, "every in-scope message was scored");
        assert!(
            out.candidates < out.censused,
            "the census must select a SUBSET — flagging everything would mean no triage at all"
        );
    }

    /// Why does the census keep 55% of a real backup when the threshold was
    /// tuned to keep 18%? (#486)
    ///
    /// The suspicion: 0.52 was calibrated against a corpus scored with ONE
    /// prototype, while production builds NINE — one per Forensic 9 category —
    /// and `census_score` takes the MAX over them. More prototypes means more
    /// chances to clear the same bar.
    ///
    /// Embeddings do not depend on the prototype set, so this embeds each
    /// message ONCE and then scores it against every prototype subset offline:
    /// the whole selectivity curve for the price of one census pass. It prints
    /// a table and asserts only the mechanism, not a target — choosing
    /// thresholds is a product decision, and this is the evidence for it.
    ///
    ///   TRIAGE_REAL_BACKUP=/path/to/unpacked/<udid> TRIAGE_REAL_PASSWORD=… \
    ///   TRACELOUPE_EMBED_MODEL=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
    ///   cargo test -p traceloupe-core census_selectivity_by_prototype_count -- --ignored --nocapture
    #[test]
    #[ignore = "requires a public DFIR image + the embedding GGUF"]
    fn census_selectivity_by_prototype_count() {
        use crate::analysis::Category;
        use crate::cache::CacheDb;
        use crate::safety_scan::chunker::{self, ScanSources, TimeRange};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{self, census_score, EMBED_PREFIX};
        use crate::safety_scan::triage_scan;
        use crate::sidecar::CancelToken;
        use std::time::Duration;

        let (Ok(backup), Ok(embed_model)) = (
            std::env::var("TRIAGE_REAL_BACKUP"),
            std::env::var("TRACELOUPE_EMBED_MODEL"),
        ) else {
            eprintln!("set TRIAGE_REAL_BACKUP and TRACELOUPE_EMBED_MODEL");
            return;
        };
        let password = std::env::var("TRIAGE_REAL_PASSWORD").unwrap_or_default();
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        crate::import::import_backup(
            None,
            std::path::Path::new(&backup),
            &password,
            &cache_path,
            &dir.path().join("work"),
            &["messages".to_string()],
            false,
            false,
            &CancelToken::new(),
            |_| {},
        )
        .expect("import the public image");
        let cache = CacheDb::open(&cache_path).unwrap();
        let threads =
            chunker::census_threads(&cache, TimeRange::default(), &ScanSources::default()).unwrap();
        let msgs: Vec<_> = threads.into_iter().flatten().collect();

        let mut server = spawn_live_server(&embed_model, true, 2048);
        let c = LlmClient::new(server.base_url(), "embed", Duration::from_secs(300));

        // One prototype per category, kept SEPARATE so subsets can be scored.
        let per_category: Vec<(Category, Vec<f32>)> = Category::ALL
            .iter()
            .filter_map(|cat| {
                let ex = triage_scan::prototype_examples(&[*cat]);
                triage::build_prototypes(&ex, |t| c.embed(t))
                    .ok()
                    .and_then(|p| p.into_iter().next())
                    .map(|v| (*cat, v))
            })
            .collect();
        assert_eq!(
            per_category.len(),
            Category::ALL.len(),
            "a prototype per category"
        );

        // Embed every message once; scoring against subsets is then free.
        let vectors: Vec<Vec<f32>> = msgs
            .iter()
            .filter_map(|m| {
                let capped: String = m.text.chars().take(400).collect();
                c.embed(&format!("{EMBED_PREFIX}{capped}")).ok()
            })
            .collect();
        server.shutdown();
        let n = vectors.len();
        assert!(n > 0, "the image has embeddable messages");

        let keep_rate = |protos: &[Vec<f32>], th: f32| -> f64 {
            let kept = vectors
                .iter()
                .filter(|v| census_score(v, protos) >= th)
                .count();
            100.0 * kept as f64 / n as f64
        };

        println!("\n=== census selectivity on a real device ({n} messages) ===");
        println!("prototypes                       0.52     0.55     0.58     0.64");
        for k in [1usize, 3, 5, 9] {
            let subset: Vec<Vec<f32>> = per_category
                .iter()
                .take(k)
                .map(|(_, v)| v.clone())
                .collect();
            println!(
                "{:>2} categories                    {:>5.1}%   {:>5.1}%   {:>5.1}%   {:>5.1}%",
                k,
                keep_rate(&subset, 0.52),
                keep_rate(&subset, 0.55),
                keep_rate(&subset, 0.58),
                keep_rate(&subset, 0.64),
            );
        }
        println!("\nper-category keep rate at 0.52 (which prototypes are loose):");
        for (cat, v) in &per_category {
            println!(
                "  {:<24} {:>5.1}%",
                cat.as_str(),
                keep_rate(std::slice::from_ref(v), 0.52)
            );
        }
        // What thresholds SHOULD be is a product call; what must be true is the
        // mechanism — scoring against more prototypes cannot select less.
        let one: Vec<Vec<f32>> = per_category
            .iter()
            .take(1)
            .map(|(_, v)| v.clone())
            .collect();
        let all: Vec<Vec<f32>> = per_category.iter().map(|(_, v)| v.clone()).collect();
        assert!(
            keep_rate(&all, 0.52) >= keep_rate(&one, 0.52),
            "max-over-prototypes is monotonic in the prototype set"
        );
    }

    /// Derive the census thresholds from a recall-vs-cost curve on a REAL
    /// message distribution (#489, #491).
    ///
    /// The prototypes come from the census's own corpus and the ground truth
    /// from the eval fixtures, which are disjoint by construction (guarded in
    /// `triage_scan`). That disjointness is what makes this honest: an earlier
    /// version built centroids from the very fixtures it scored and reported
    /// recall 1.000 at 0.52 where the held-out oracle measured 0.96. With two
    /// corpora there is nothing to hold out — the ground truth was never in
    /// the centroids.
    ///
    /// The bed is the public image's own conversation, because ordinary
    /// chatter is exactly what the census must reject.
    ///
    ///   TRIAGE_REAL_BACKUP=… TRIAGE_REAL_PASSWORD=… \
    ///   TRACELOUPE_EMBED_MODEL=… \
    ///   cargo test -p traceloupe-core census_recall_vs_cost -- --ignored --nocapture
    /// Does giving each category its OWN threshold beat one global cut? (#486)
    ///
    /// The census scores a message against nine centroids and keeps the MAX,
    /// then compares it to one number. That number is therefore read at
    /// whatever cut suits the loosest centroid, which is the shape of the
    /// per-category spread that has survived every corpus revision: at the
    /// Thorough cut `hate-identity` recalls 0.82 and `scam-fraud` 0.33. The
    /// open question on #486 is whether that is a corpus problem or a scoring
    /// problem.
    ///
    /// The per-category scheme here is QUANTILE calibration: each category's
    /// threshold is set so it keeps the same share of ordinary chatter as
    /// every other, and a message is a candidate if ANY category clears its
    /// own bar. That is the cheapest honest version of "normalise per
    /// category" — it needs no per-category ground truth, only the bed, so it
    /// could actually ship.
    ///
    /// Both schemes are scored on the same messages with the same embeddings,
    /// and compared at MATCHED COST, because a scheme that keeps more of the
    /// phone will always recall more.
    ///
    ///   TRIAGE_REAL_BACKUP=… TRIAGE_REAL_PASSWORD=… TRACELOUPE_EMBED_MODEL=… \
    ///   cargo test -p traceloupe-core per_category_cuts -- --ignored --nocapture
    #[test]
    #[ignore = "requires a public DFIR image + the embedding GGUF"]
    fn per_category_cuts_vs_one_global_cut() {
        use crate::analysis::Category;
        use crate::cache::CacheDb;
        use crate::safety_scan::chunker::{self, ScanSources, TimeRange};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::cost_model::hours_per_100k;
        use crate::safety_scan::triage::{
            build_prototypes, cap_for_embedding, cosine, EMBED_PREFIX,
        };
        use crate::safety_scan::triage_scan::prototype_examples;
        use crate::sidecar::CancelToken;
        use std::time::Duration;

        let (Ok(backup), Ok(embed_model)) = (
            std::env::var("TRIAGE_REAL_BACKUP"),
            std::env::var("TRACELOUPE_EMBED_MODEL"),
        ) else {
            eprintln!("set TRIAGE_REAL_BACKUP and TRACELOUPE_EMBED_MODEL");
            return;
        };
        let password = std::env::var("TRIAGE_REAL_PASSWORD").unwrap_or_default();
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        crate::import::import_backup(
            None,
            std::path::Path::new(&backup),
            &password,
            &cache_path,
            &dir.path().join("work"),
            &["messages".to_string()],
            false,
            false,
            &CancelToken::new(),
            |_| {},
        )
        .expect("import the public image");
        let cache = CacheDb::open(&cache_path).unwrap();
        let bed: Vec<String> =
            chunker::census_threads(&cache, TimeRange::default(), &ScanSources::default())
                .unwrap()
                .into_iter()
                .flatten()
                .map(|m| m.text)
                .collect();
        assert!(
            bed.len() > 100,
            "the image supplies a real conversational bed"
        );
        println!(
            "bed identity: n={} sha={}",
            bed.len(),
            bed_fingerprint(&bed)
        );

        let server = spawn_live_server(&embed_model, true, 2048);
        let c = LlmClient::new(server.base_url(), "embed", Duration::from_secs(300));
        let embed_one = |t: &str| c.embed(&format!("{EMBED_PREFIX}{}", cap_for_embedding(t)));

        // One centroid per category, LABELLED — the shipped path throws the
        // labels away by taking a max, which is precisely what is under test.
        let cats: Vec<Category> = Category::ALL.to_vec();
        let centroids: Vec<Vec<f32>> = cats
            .iter()
            .map(|cat| {
                let ex = prototype_examples(&[*cat]);
                // RAW `c.embed`, not `embed_one`: build_prototypes applies the
                // task prefix and the byte cap itself. Passing embed_one here
                // prefixed every example TWICE, which shifted every centroid
                // and made this experiment's curve look one grid step looser
                // than the shipped one on an identical bed.
                let built = build_prototypes(&ex, |t| c.embed(t)).expect("build centroid");
                assert_eq!(built.len(), 1, "one category must yield one centroid");
                built.into_iter().next().unwrap()
            })
            .collect();

        let scores = |v: &[f32]| -> Vec<f32> { centroids.iter().map(|p| cosine(v, p)).collect() };

        // CROSS-CHECK against the shipped scoring path before trusting a
        // single number below. This experiment rebuilds the centroids one
        // category at a time so it can keep the labels the production path
        // throws away — if that rebuild is not bit-for-bit the same scoring,
        // every comparison here is against a curve the product does not have.
        //
        // Note what this does and does not catch. It compares two constructions
        // INSIDE this test, so a mistake made identically in both (as
        // double-prefixing originally was) passes it. What caught that was this
        // harness's global curve disagreeing with `census_recall_vs_cost` on an
        // identical bed — which is the real reason to keep two harnesses that
        // compute the same column.
        {
            use crate::safety_scan::triage::census_score;
            let shipped = build_prototypes(&prototype_examples(&Category::ALL), |t| c.embed(t))
                .expect("shipped prototypes");
            assert_eq!(
                shipped.len(),
                centroids.len(),
                "the shipped path built {} centroids, this experiment {}",
                shipped.len(),
                centroids.len()
            );
            // Across the WHOLE bed, not a few probes: a handful of strings can
            // agree while the curves built from 576 differ, which is exactly
            // the disagreement this check exists to settle.
            let mut worst = 0.0f32;
            let mut worst_at = String::new();
            for t in &bed {
                let Ok(v) = embed_one(t) else { continue };
                let mine = centroids
                    .iter()
                    .map(|p| cosine(&v, p))
                    .fold(0.0f32, f32::max);
                let theirs = census_score(&v, &shipped);
                if (mine - theirs).abs() > worst {
                    worst = (mine - theirs).abs();
                    worst_at = t.chars().take(60).collect();
                }
            }
            println!("  scoring cross-check: worst |experiment - production| = {worst:.6}");
            assert!(
                worst < 1e-5,
                "scoring diverged by {worst:.6} (worst at {worst_at:?}) — every comparison \
                 below would be against a curve the product does not have"
            );
        }
        let bed_scores: Vec<Vec<f32>> = bed
            .iter()
            .filter_map(|t| embed_one(t).ok())
            .map(|v| scores(&v))
            .collect();

        // Embed each positive ONCE and carry its categories alongside the
        // score row. Embedding per (case × category) double-counts every
        // multi-category case in the denominator every number below is read
        // off — the same defect a review found in `census_recall_vs_cost`.
        let fixtures = load_fixtures();
        let mut planted_scores: Vec<(Vec<Category>, Vec<f32>)> = Vec::new();
        for case in fixtures.cases.iter().filter(|c| c.kind == "positive") {
            let cs: Vec<Category> = case.expected_categories().into_iter().collect();
            for m in &case.messages {
                if let Ok(v) = embed_one(&m.text) {
                    planted_scores.push((cs.clone(), scores(&v)));
                }
            }
        }
        assert!(!planted_scores.is_empty() && !bed_scores.is_empty());
        let max_of = |s: &[f32]| s.iter().copied().fold(0.0f32, f32::max);

        // Per-category cut at quantile q of that category's BED scores: each
        // category is allowed to keep the same fraction of ordinary chatter.
        let cut_at = |cat_ix: usize, q: f64| -> f32 {
            let mut col: Vec<f32> = bed_scores.iter().map(|s| s[cat_ix]).collect();
            col.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let idx = ((col.len() - 1) as f64 * q).round() as usize;
            col[idx]
        };

        println!("\n=== per-category cuts vs one global cut ===");
        println!(
            "bed {} messages · {} planted positives · {} categories",
            bed_scores.len(),
            planted_scores.len(),
            cats.len()
        );

        // The global curve on a FINE grid, so "matched cost" means the same
        // cost and not the nearest point of a coarse one. Comparing a scheme
        // that keeps 13.9% of the phone against one that keeps 11.5% and
        // calling the difference a win is how a costlier setting gets sold as
        // a better one.
        let curve = |t: f32| -> (f64, f64) {
            let recall = planted_scores
                .iter()
                .filter(|(_, s)| max_of(s) >= t)
                .count() as f64
                / planted_scores.len() as f64;
            let sel = 100.0 * bed_scores.iter().filter(|s| max_of(s) >= t).count() as f64
                / bed_scores.len() as f64;
            (recall, sel)
        };
        let fine: Vec<(f32, f64, f64)> = (0..=160)
            .map(|i| 0.40 + i as f32 * 0.0025)
            .map(|t| {
                let (r, sel) = curve(t);
                (t, r, sel)
            })
            .collect();
        // The global threshold that keeps the same share of the phone.
        let global_at = |target_sel: f64| -> (f32, f64, f64) {
            *fine
                .iter()
                .min_by(|a, b| {
                    (a.2 - target_sel)
                        .abs()
                        .partial_cmp(&(b.2 - target_sel).abs())
                        .unwrap()
                })
                .unwrap()
        };

        println!("\nglobal cut (shipped):");
        println!("  cut    recall  selectivity   h/100k");
        for t in [0.55f32, 0.58, 0.61, 0.64, 0.67, 0.70] {
            let (recall, sel) = curve(t);
            println!(
                "  {t:.2}   {recall:.3}   {sel:>5.1}%      {:>5.1}",
                hours_per_100k(sel)
            );
        }

        println!("\nper-category cuts (quantile-calibrated on the bed):");
        println!("  q        recall  selectivity   h/100k   global at the SAME cost");
        for q in [0.80f64, 0.90, 0.95, 0.975, 0.99, 0.995] {
            let cuts: Vec<f32> = (0..cats.len()).map(|i| cut_at(i, q)).collect();
            let keeps = |s: &[f32]| s.iter().zip(&cuts).any(|(v, t)| v >= t);
            let recall = planted_scores.iter().filter(|(_, s)| keeps(s)).count() as f64
                / planted_scores.len() as f64;
            let sel = 100.0 * bed_scores.iter().filter(|s| keeps(s)).count() as f64
                / bed_scores.len() as f64;
            let (gt, gr, gs) = global_at(sel);
            println!(
                "  {q:.3}    {recall:.3}   {sel:>5.1}%      {:>5.1}    {gt:.4} → {gr:.3} at {gs:.1}%   ({:+.3})",
                hours_per_100k(sel),
                recall - gr,
            );
        }

        // TARGETED: lower ONE category's bar, leave the other eight at the
        // shipped cut.
        //
        // #495 rejected a UNIFORM per-category scheme — every category
        // quantile-calibrated to keep an equal share of chatter — because at
        // matched cost it was a wash. This is a different question, and one
        // #495 had no way to ask: #504 measured that with full context the
        // focused stage gets coercive-control 13/14, while the census passes
        // only 0.62 of it. Recall the census drops HERE is recoverable value,
        // because the judge behind the gate is nearly perfect. That is not true
        // of a category whose judge would miss it anyway, which is why moving
        // every cut at once averaged out to nothing.
        let cc_ix = cats
            .iter()
            .position(|c| *c == Category::CoerciveControl)
            .expect("coercive-control is in the catalogue");
        let cc_positives: Vec<&Vec<f32>> = planted_scores
            .iter()
            .filter(|(cs, _)| cs.contains(&Category::CoerciveControl))
            .map(|(_, s)| s)
            .collect();
        for (posture, shipped) in [("Thorough", 0.64f32), ("Balanced", 0.675)] {
            let (base_r, base_sel) = curve(shipped);
            let base_cc = cc_positives.iter().filter(|s| max_of(s) >= shipped).count() as f64
                / cc_positives.len() as f64;
            println!(
                "\ntargeted coercive-control gate, {posture} (others stay at {shipped:.3}):\n  \
                 baseline: overall {base_r:.3}, coercive-control {base_cc:.3}, {base_sel:.1}%, \
                 {:.1} h",
                hours_per_100k(base_sel)
            );
            println!("  cc cut   overall  cc recall  selectivity   h/100k   vs baseline");
            for cc_cut in [
                shipped,
                shipped - 0.015,
                shipped - 0.03,
                shipped - 0.045,
                shipped - 0.06,
            ] {
                let keeps = |s: &[f32]| {
                    s.iter()
                        .enumerate()
                        .any(|(i, v)| *v >= if i == cc_ix { cc_cut } else { shipped })
                };
                let overall = planted_scores.iter().filter(|(_, s)| keeps(s)).count() as f64
                    / planted_scores.len() as f64;
                let cc = cc_positives.iter().filter(|s| keeps(s)).count() as f64
                    / cc_positives.len() as f64;
                let sel = 100.0 * bed_scores.iter().filter(|s| keeps(s)).count() as f64
                    / bed_scores.len() as f64;
                // The comparison that decides it: a GLOBAL cut bought to the
                // same cost. Lowering one category's bar also admits other
                // categories' positives that happen to score on that centroid,
                // so "overall recall went up" is not evidence the targeting
                // worked — only beating an equally expensive global cut is.
                let (gt, gr, _) = global_at(sel);
                let gcc = cc_positives.iter().filter(|s| max_of(s) >= gt).count() as f64
                    / cc_positives.len() as f64;
                println!(
                    "  {cc_cut:.3}    {overall:.3}    {cc:.3}      {sel:>5.1}%      {:>5.1}   \
                     global {gt:.3} -> {gr:.3}/{gcc:.3}  ({:+.3} overall, {:+.3} cc)",
                    hours_per_100k(sel),
                    overall - gr,
                    cc - gcc,
                );
            }
        }

        // Per-category recall at one representative operating point of each
        // scheme, so the SPREAD — the thing #486 is actually about — is visible.
        let q = 0.975f64;
        let cuts: Vec<f32> = (0..cats.len()).map(|i| cut_at(i, q)).collect();
        let keeps = |s: &[f32]| s.iter().zip(&cuts).any(|(v, t)| v >= t);
        let sel_at_q =
            100.0 * bed_scores.iter().filter(|s| keeps(s)).count() as f64 / bed_scores.len() as f64;
        let (gt, _, gs) = global_at(sel_at_q);
        println!(
            "\nper-category recall (its own positives) at MATCHED cost \
             — per-category q={q:.3} ({sel_at_q:.1}%) vs global {gt:.4} ({gs:.1}%):"
        );
        println!("  category                 per-cat   global   its cut");
        for (i, cat) in cats.iter().enumerate() {
            let of_cat: Vec<&Vec<f32>> = planted_scores
                .iter()
                .filter(|(cs, _)| cs.contains(cat))
                .map(|(_, s)| s)
                .collect();
            if of_cat.is_empty() {
                continue;
            }
            let per = of_cat.iter().filter(|s| keeps(s)).count() as f64 / of_cat.len() as f64;
            let glob =
                of_cat.iter().filter(|s| max_of(s) >= gt).count() as f64 / of_cat.len() as f64;
            println!(
                "  {:<22}   {per:.2}     {glob:.2}    {:.3}",
                cat.as_str(),
                cuts[i]
            );
        }
        drop(server);
    }

    #[test]
    #[ignore = "requires a public DFIR image + the embedding GGUF"]
    fn census_recall_vs_cost() {
        use crate::analysis::Category;
        use crate::cache::CacheDb;
        use crate::safety_scan::chunker::{self, ScanSources, TimeRange};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{self, cap_for_embedding, census_score, EMBED_PREFIX};
        use crate::safety_scan::triage_scan;
        use crate::sidecar::CancelToken;
        use std::time::Duration;

        let (Ok(backup), Ok(embed_model)) = (
            std::env::var("TRIAGE_REAL_BACKUP"),
            std::env::var("TRACELOUPE_EMBED_MODEL"),
        ) else {
            eprintln!("set TRIAGE_REAL_BACKUP and TRACELOUPE_EMBED_MODEL");
            return;
        };
        let password = std::env::var("TRIAGE_REAL_PASSWORD").unwrap_or_default();
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        crate::import::import_backup(
            None,
            std::path::Path::new(&backup),
            &password,
            &cache_path,
            &dir.path().join("work"),
            &["messages".to_string()],
            false,
            false,
            &CancelToken::new(),
            |_| {},
        )
        .expect("import the public image");
        let cache = CacheDb::open(&cache_path).unwrap();
        let bed: Vec<String> =
            chunker::census_threads(&cache, TimeRange::default(), &ScanSources::default())
                .unwrap()
                .into_iter()
                .flatten()
                .map(|m| m.text)
                .collect();
        assert!(
            bed.len() > 100,
            "the image supplies a real conversational bed"
        );
        println!(
            "bed identity: n={} sha={}",
            bed.len(),
            bed_fingerprint(&bed)
        );

        let mut server = spawn_live_server(&embed_model, true, 2048);
        let c = LlmClient::new(server.base_url(), "embed", Duration::from_secs(300));
        let embed_one = |t: &str| c.embed(&format!("{EMBED_PREFIX}{}", cap_for_embedding(t)));

        // THE PRODUCTION PATH: same corpus, same builder, same centroids the
        // shipped census scores against. A measurement that builds its own
        // prototypes measures its own prototypes.
        let prototypes =
            triage::build_prototypes(&triage_scan::prototype_examples(&Category::ALL), |t| {
                c.embed(t)
            })
            .expect("prototypes");
        assert_eq!(
            prototypes.len(),
            Category::ALL.len(),
            "one centroid per category"
        );

        // Ground truth: the eval fixtures' positives, per category.
        let fixtures = load_fixtures();
        let positives: Vec<_> = fixtures
            .cases
            .iter()
            .filter(|c| c.kind == "positive")
            .collect();
        assert!(
            !positives.is_empty(),
            "fixture positives are the ground truth — without them the recall column is 0/0"
        );
        let mut planted: Vec<(Vec<Category>, Vec<f32>)> = Vec::new();
        let mut embed_failures = 0usize;
        for case in positives {
            // ONCE per message, with its categories fanned out over the stored
            // vector. Embedding per (case × category) counted multi-category
            // cases twice in the denominator the thresholds are read off.
            let cats: Vec<Category> = case.expected_categories().into_iter().collect();
            for m in &case.messages {
                match embed_one(&m.text) {
                    Ok(v) => planted.push((cats.clone(), v)),
                    Err(_) => embed_failures += 1,
                }
            }
        }
        let mut bed_vecs = Vec::new();
        for t in &bed {
            match embed_one(t) {
                Ok(v) => bed_vecs.push(v),
                Err(_) => embed_failures += 1,
            }
        }
        server.shutdown();
        assert_eq!(
            embed_failures, 0,
            "{embed_failures} embeddings failed — the curve would be computed from a \
             truncated sample and would look plausible anyway"
        );

        use crate::safety_scan::cost_model::{
            hours_per_100k, FULL_READ_HOURS_PER_100K, MEASURED_SELECTIVITY,
        };
        println!(
            "\n=== census recall vs cost (production prototypes, disjoint ground truth) ===\n\
             bed {} real messages · {} planted positive messages\n",
            bed_vecs.len(),
            planted.len()
        );
        {
            let mut m: Vec<f32> = bed_vecs
                .iter()
                .map(|v| census_score(v, &prototypes))
                .collect();
            m.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "  bed max-score: mean {:.4} median {:.4} p90 {:.4}",
                m.iter().sum::<f32>() / m.len() as f32,
                m[m.len() / 2],
                m[(m.len() as f64 * 0.9) as usize]
            );
        }
        println!("threshold   recall   selectivity   100k cost");
        for th in [0.46f32, 0.49, 0.52, 0.55, 0.58, 0.61, 0.64, 0.67, 0.70] {
            let kept = planted
                .iter()
                .filter(|(_, v)| census_score(v, &prototypes) >= th)
                .count();
            let recall = kept as f64 / planted.len() as f64;
            let kept_bed = bed_vecs
                .iter()
                .filter(|v| census_score(v, &prototypes) >= th)
                .count();
            let sel = 100.0 * kept_bed as f64 / bed_vecs.len() as f64;
            let hours = hours_per_100k(sel);
            println!("  {th:.2}      {recall:.3}      {sel:>5.1}%      {hours:>5.1} h");
        }
        println!(
            "\n(full read for reference: 0.30 recall, ~{FULL_READ_HOURS_PER_100K} h per 100k — \
             measured on Jigsaw fixtures, so the comparison is indicative, not like for like)"
        );

        // THE GUARD (#486). Printing the cost was never enough: the census kept
        // 55% of a real phone for weeks, and the only reason anyone found out
        // was a person reading this table. Now a posture that stops being
        // affordable fails the run that measured it.
        //
        // It also re-checks the CHECKED-IN record against what was just
        // measured, because the CI-side guard in `cost_model` can only be as
        // honest as those constants.
        // A FINE sweep around the shipped cuts. The coarse grid above is the
        // historical comparison and should not move; re-deriving a posture's
        // cut needs resolution the coarse grid does not have — #501 shipped a
        // corpus change that pushed Balanced from 2.6% to 3.6% keep-rate, and
        // "somewhere between 0.67 and 0.70" is not a threshold.
        //
        // The reference points printed alongside are what each posture PROMISES
        // the user, so a cut is chosen against the claim rather than by eye:
        // Balanced says "about as much as a full read in roughly half the
        // time", i.e. ~5.5 h per 100k.
        println!(
            "\nfine sweep for re-deriving a cut (target: Balanced ~5.5 h = half the full read):"
        );
        println!("  cut     recall  selectivity   h/100k");
        {
            let mut th = 0.640f32;
            while th <= 0.7201 {
                let recall = planted
                    .iter()
                    .filter(|(_, v)| census_score(v, &prototypes) >= th)
                    .count() as f64
                    / planted.len() as f64;
                let sel = 100.0
                    * bed_vecs
                        .iter()
                        .filter(|v| census_score(v, &prototypes) >= th)
                        .count() as f64
                    / bed_vecs.len() as f64;
                let hours = hours_per_100k(sel);
                let mark = if (hours - 5.5).abs() < 0.6 {
                    "  <- ~half a full read"
                } else {
                    ""
                };
                println!("  {th:.3}   {recall:.3}   {sel:>5.1}%      {hours:>5.1}{mark}");
                th += 0.005;
            }
        }

        println!("\nselectivity guard (measured now vs the checked-in record):");
        let mut drifted: Vec<String> = Vec::new();
        let mut over_ceiling: Vec<String> = Vec::new();
        for m in &MEASURED_SELECTIVITY {
            let kept_bed = bed_vecs
                .iter()
                .filter(|v| census_score(v, &prototypes) >= m.threshold)
                .count();
            let sel = 100.0 * kept_bed as f64 / bed_vecs.len() as f64;
            let hours = hours_per_100k(sel);
            let ceiling = m.mode.cost_ceiling_hours_per_100k();
            println!(
                "  {:<9} @{:.3}  {sel:>5.1}% ({:.1}% recorded)  {hours:>5.1} h / ceiling {ceiling:>5.1} h",
                m.mode.as_str(),
                m.threshold,
                m.selectivity_pct,
            );
            if hours > ceiling {
                over_ceiling.push(format!(
                    "{} keeps {sel:.1}% of this device — {hours:.1} h per 100k against a \
                     ceiling of {ceiling:.1} h. Triage's whole premise is that the census is \
                     selective enough to make depth affordable; at this rate the posture costs \
                     more than the {FULL_READ_HOURS_PER_100K} h full read it replaces. Fix the \
                     census, do not raise the ceiling.",
                    m.mode.as_str(),
                ));
            }
            // 0.1 pp sits deliberately between the two scales that matter: the
            // recorded constants are rounded to one decimal, so they can differ
            // from a true measurement by up to 0.05 pp for no reason at all,
            // while one message of this 576-message bed is 0.174 pp. So this
            // tolerates rounding and nothing else — a single message changing
            // side trips it, which is the sensitivity we want from a record
            // whose only job is to describe what ships.
            if (sel - m.selectivity_pct).abs() > 0.1 {
                drifted.push(format!(
                    "{} @{:.2}: measured {sel:.1}%, recorded {:.1}%",
                    m.mode.as_str(),
                    m.threshold,
                    m.selectivity_pct
                ));
            }
        }
        println!("\nper-category recall at the shipped cuts:");
        println!("category                  0.64   0.67   0.70");
        for cat in Category::ALL {
            let of_cat: Vec<_> = planted.iter().filter(|(cs, _)| cs.contains(&cat)).collect();
            if of_cat.is_empty() {
                continue;
            }
            let r = |th: f32| {
                of_cat
                    .iter()
                    .filter(|(_, v)| census_score(v, &prototypes) >= th)
                    .count() as f64
                    / of_cat.len() as f64
            };
            println!(
                "  {:<24} {:.2}   {:.2}   {:.2}",
                cat.as_str(),
                r(0.64),
                r(0.67),
                r(0.70)
            );
        }
        assert!(!bed_vecs.is_empty() && !planted.is_empty());

        // BOTH assertions land here, after every diagnostic has printed.
        // They used to fire mid-way, which aborted the run before the
        // per-category table — so the guard withheld exactly the numbers you
        // need to decide whether the drift it just reported is an improvement
        // worth recording or a regression worth reverting.
        assert!(
            over_ceiling.is_empty(),
            "a posture now costs more than its name promises:\n  {}",
            over_ceiling.join("\n  ")
        );
        assert!(
            drifted.is_empty(),
            "cost_model::MEASURED_SELECTIVITY no longer matches this device:\n  {}\n\
             Update those constants — the CI-side cost guard is checking a stale number, \
             which is exactly how a selectivity regression stays invisible.",
            drifted.join("\n  ")
        );
    }

    /// Can the FOCUSED stage see what the census structurally cannot? (#503)
    ///
    /// #503 established that relationship-harassment and coercive-control are
    /// conversational patterns whose individual messages are ordinary — "im
    /// downstairs, let me in" is harassment only because of a prior refusal —
    /// and that a per-message census therefore cannot separate them from
    /// everyday logistics without keeping the logistics too.
    ///
    /// That bounds the CENSUS. It says nothing about the pipeline, because the
    /// focused stage reads a conversational window and could in principle
    /// recover exactly what the census misses. If it does, triage's answer for
    /// these categories is "rank generously, judge in context". If it does not,
    /// the limit is the taxonomy meeting a per-message model at all, and only
    /// the fine-tune is left.
    ///
    /// So: hand the focused classifier each pattern-category fixture as a whole
    /// conversation, with every message in the window, and ask whether it finds
    /// the category the fixture is labelled with. This is the stage's CEILING —
    /// perfect context, no census gate, no budget — so a miss here is a real
    /// limit rather than a tuning problem.
    ///
    ///   TRACELOUPE_EVAL_MODEL=…/gemma-4-E4B-it-Q4_K_M.gguf \
    ///   cargo test -p traceloupe-core focused_stage_on_pattern_categories -- --ignored --nocapture
    #[test]
    #[ignore = "requires the classifier GGUF (set TRACELOUPE_EVAL_MODEL)"]
    fn focused_stage_on_pattern_categories() {
        use crate::analysis::Category;
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{CensusInput, FocusWindow};
        use crate::safety_scan::triage_scan;
        use std::time::Duration;

        let Ok(model) = std::env::var("TRACELOUPE_EVAL_MODEL") else {
            eprintln!("set TRACELOUPE_EVAL_MODEL");
            return;
        };
        let server = spawn_live_server(&model, false, 8192);
        let c = LlmClient::new(server.base_url(), "eval", Duration::from_secs(300));

        // The two categories #503 named, plus threat-violence as a CONTROL:
        // its harm is in the words, so if the harness is sound it should score
        // well here and the pattern categories should not.
        let targets = [
            Category::CoerciveControl,
            Category::HarassmentBullying,
            Category::ThreatViolence,
        ];
        // Scored against the FULL eval set (cases.json + the hand-written
        // expansion), because the original 14 coercive-control conversations
        // could not resolve the differences this test exists to report.
        let cases_all = load_all_eval_cases();
        let fixtures = Fixtures { cases: cases_all };
        println!("\n=== focused stage, whole conversation, no census gate ===");
        println!("category                 hit   n   missed cases");
        for cat in targets {
            let cases: Vec<_> = fixtures
                .cases
                .iter()
                .filter(|c| c.kind == "positive" && c.expected_categories().contains(&cat))
                .collect();
            if cases.is_empty() {
                continue;
            }
            let mut hit = 0usize;
            let mut missed: Vec<&str> = Vec::new();
            for case in &cases {
                let items: Vec<CensusInput> = case
                    .messages
                    .iter()
                    .enumerate()
                    .map(|(i, m)| CensusInput {
                        source_id: i as i64,
                        thread_identifier: case.id.clone(),
                        sender: m.sender.clone(),
                        occurred_at: Some(i as i64 * 60),
                        text: m.text.clone(),
                        fingerprint: format!("{}-{i}", case.id),
                        service: Some("iMessage".into()),
                    })
                    .collect();
                // Judge the LAST message of the exchange: the pattern is
                // complete by then, which is the most favourable reading.
                let focus = items.len() - 1;
                let w = FocusWindow { items, focus };
                match triage_scan::classify_focused(&c, &w) {
                    Ok(out) => {
                        if out.verdicts.iter().any(|v| v.category == cat) {
                            hit += 1;
                        } else {
                            missed.push(&case.id);
                        }
                    }
                    Err(e) => {
                        missed.push(&case.id);
                        eprintln!("  classify failed on {}: {e}", case.id);
                    }
                }
            }
            println!(
                "  {:<22} {hit:>3}  {:>3}   {}",
                cat.as_str(),
                cases.len(),
                if missed.is_empty() {
                    "-".to_string()
                } else {
                    missed.join(", ")
                }
            );
        }
        drop(server);
    }

    /// #409's premise, measured: focused mode re-sends the system prompt per
    /// message — but the pinned llama-server (b10075) defaults `cache_prompt`
    /// to true, so sequential focused calls on one slot should reuse the
    /// shared system-prompt prefix and only pay for the divergent tail. This
    /// prints per-call prompt-eval counts and times with the default cache and
    /// with `cache_prompt: false`, so #409 can be closed (or re-scoped) from a
    /// measurement rather than an assumption. A measurement harness, like
    /// `measure_scan_throughput`: it prints, it does not assert thresholds.
    ///
    ///   TRACELOUPE_EVAL_MODEL=~/.../gemma-4-E4B-it-Q4_K_M.gguf \
    ///   cargo test -p traceloupe-core measure_focused_prompt_cache -- --ignored --nocapture
    #[test]
    #[ignore = "requires the classifier GGUF (set TRACELOUPE_EVAL_MODEL)"]
    fn measure_focused_prompt_cache() {
        use crate::analysis::SourceKind;
        use crate::safety_scan::chunker::{Chunk, ChunkItem};
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::prompt;
        use std::time::Duration;

        let Ok(class_model) = std::env::var("TRACELOUPE_EVAL_MODEL") else {
            eprintln!("set TRACELOUPE_EVAL_MODEL");
            return;
        };
        let mut server = spawn_live_server(&class_model, false, 8192);

        // Six distinct focused windows: same system prompt (the shared
        // prefix), different conversations (the divergent tail).
        let window = |seed: usize| -> Chunk {
            let items = (0..5)
                .map(|i| ChunkItem {
                    source_id: (seed * 10 + i) as i64,
                    sender: if i % 2 == 0 { "them" } else { "me" }.into(),
                    occurred_at: None,
                    text: format!(
                        "message {i} of conversation {seed} about the plans for saturday"
                    ),
                    fingerprint: format!("fp{seed}:{i}"),
                })
                .collect();
            Chunk {
                key: format!("bench:{seed}"),
                fingerprint: String::new(),
                kind: SourceKind::Message,
                thread_identifier: Some(format!("bench-{seed}")),
                label: None,
                service: None,
                items,
            }
        };

        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(300))
            .build();
        // The EXACT production request body (LlmClient::chat_json_body — the
        // same prompt fns, grammar, token budget and explicit cache_prompt the
        // scan sends), so this harness can never drift into measuring a
        // request production does not make. Only the counterfactual arm
        // mutates the body, and only to flip the knob under measurement.
        let client = LlmClient::new(server.base_url(), "eval", Duration::from_secs(300));
        let call = |seed: usize, cache: bool| -> (u64, f64) {
            let chunk = window(seed);
            let mut body = client.chat_json_body(
                prompt::SYSTEM_PROMPT,
                &prompt::render_focused(&chunk, 2),
                &prompt::verdicts_grammar(chunk.items.len()),
                600,
            );
            body["cache_prompt"] = serde_json::Value::Bool(cache);
            let resp: serde_json::Value = serde_json::from_str(
                &agent
                    .post(&format!("{}/v1/chat/completions", server.base_url()))
                    .set("Content-Type", "application/json")
                    .send_string(&body.to_string())
                    .expect("request")
                    .into_string()
                    .expect("read"),
            )
            .expect("json");
            // Loud, not lenient: this harness's only output IS these two
            // numbers, and a server build that drops/moves the (non-standard)
            // timings block would otherwise print zeros that read as a perfect
            // cache hit.
            let t = resp
                .get("timings")
                .expect("timings block in llama-server response");
            (
                t["prompt_n"].as_u64().expect("timings.prompt_n"),
                t["prompt_ms"].as_f64().expect("timings.prompt_ms"),
            )
        };

        println!("cache_prompt=true (what production sends):");
        for seed in 0..6 {
            let (n, ms) = call(seed, true);
            println!("  call {seed}: prompt_n={n} prompt_ms={ms:.0}");
        }
        println!("cache_prompt=false (the counterfactual):");
        for seed in 6..12 {
            let (n, ms) = call(seed, false);
            println!("  call {seed}: prompt_n={n} prompt_ms={ms:.0}");
        }
        server.shutdown();
    }

    /// The census cuts the ORACLE's numbers were recorded at. Pinned here
    /// rather than read off `ScanMode`, because the shipped postures are tuned
    /// to a real distribution (#489) while these bands come from the Jigsaw
    /// corpus: reading the enum made a constant change look like a pipeline
    /// regression, and asserting the enum matched them made the test
    /// impossible to pass at all.
    const ORACLE_CENSUS_CUT: f32 = 0.52;
    const ORACLE_CONFIRM_CUT: f32 = 0.58;

    /// The wired Rust triage pipeline against the recorded reference run.
    ///
    /// The Python oracle (`tools/validate-triage-pipeline.py`) produced the
    /// validated numbers (docs/validation/safety-scan-validation.md,
    /// 2026-08-12). This drives the MERGED `run_triage` — the production
    /// census/rank/window/classify path, real sidecars, real prompt and
    /// grammar — over the oracle's exact seeded corpus, and asserts the
    /// stage-level numbers land where the oracle's did (chunk-level, at the
    /// Thorough threshold 0.52, no confirmation stage):
    ///
    ///   census ceiling 0.9625 · focused recall 0.9625 · precision 0.74
    ///
    /// Corpus first (Jigsaw text stays in /tmp — CC-BY-SA, never vendored):
    ///   TRIAGE_DUMP_CHUNKS=/tmp/triage-chunks.json TRIAGE_GEMMA=- TRIAGE_EMBED=- \
    ///   TRIAGE_JIGSAW=/tmp/public-sets/jigsaw.csv TRIAGE_GRAMMARS=/tmp/grammars.json \
    ///     python3 tools/validate-triage-pipeline.py
    /// Then (~15 min: ~800 embeddings + ~110 focused calls):
    ///   TRIAGE_CHUNKS=/tmp/triage-chunks.json \
    ///   TRACELOUPE_EMBED_MODEL=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
    ///   TRACELOUPE_EVAL_MODEL=~/.../gemma-4-E4B-it-Q4_K_M.gguf \
    ///   cargo test -p traceloupe-core triage_pipeline_matches_reference -- --ignored --nocapture
    ///
    /// Optionally set TRIAGE_GUARD_MODEL=/path/Llama-Guard-3-8B.Q4_K_M.gguf to
    /// also run the CONFIRMATION stage as a second, Precise-mode scan (adds
    /// ~10 min) and assert it against the oracle's recorded guard-stage
    /// reference at 0.58.
    #[test]
    #[ignore = "requires the corpus dump + both GGUFs (set TRIAGE_CHUNKS, TRACELOUPE_EVAL_MODEL, TRACELOUPE_EMBED_MODEL)"]
    fn triage_pipeline_matches_reference() {
        use crate::analysis::AnalysisDb;
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::triage::{self, CensusInput, FocusWindow, ScanMode};
        use crate::safety_scan::triage_scan::{self, TriageProgress};
        use crate::sidecar::CancelToken;
        use std::cell::RefCell;
        use std::collections::BTreeSet;
        use std::time::Duration;

        let (Ok(chunks_path), Ok(embed_model), Ok(class_model)) = (
            std::env::var("TRIAGE_CHUNKS"),
            std::env::var("TRACELOUPE_EMBED_MODEL"),
            std::env::var("TRACELOUPE_EVAL_MODEL"),
        ) else {
            eprintln!("set TRIAGE_CHUNKS, TRACELOUPE_EMBED_MODEL and TRACELOUPE_EVAL_MODEL");
            return;
        };

        #[derive(Deserialize)]
        struct DumpChunk {
            msgs: Vec<String>,
            real: bool,
        }
        #[derive(Deserialize)]
        struct Dump {
            prototypes: Vec<String>,
            chunks: Vec<DumpChunk>,
        }
        let dump: Dump =
            serde_json::from_str(&std::fs::read_to_string(&chunks_path).expect("dump readable"))
                .expect("dump parses");
        let n_real = dump.chunks.iter().filter(|c| c.real).count();
        assert!(n_real > 0, "the dump has labelled threats");

        // One thread per oracle chunk, senders exactly as the oracle rendered
        // them ('them' for even indexes, 'me' for odd). source_id encodes
        // (chunk, index) so findings map back to chunks for scoring.
        let threads: Vec<Vec<CensusInput>> = dump
            .chunks
            .iter()
            .enumerate()
            .map(|(ci, c)| {
                c.msgs
                    .iter()
                    .enumerate()
                    .map(|(i, text)| CensusInput {
                        source_id: (ci * 100 + i) as i64,
                        thread_identifier: format!("c{ci}"),
                        sender: if i % 2 == 1 {
                            "me".into()
                        } else {
                            "them".into()
                        },
                        occurred_at: None,
                        text: text.clone(),
                        fingerprint: format!("parity:{ci}:{i}"),
                        service: None,
                    })
                    .collect()
            })
            .collect();

        let spawn = spawn_live_server;

        // Phase A: the embedder — prototypes from the oracle's HELD-OUT threat
        // texts (single category, same centroid math either way).
        let server = spawn(&embed_model, true, 2048);
        let ec = LlmClient::new(server.base_url(), "embed", Duration::from_secs(300));
        let examples: Vec<(String, String)> = dump
            .prototypes
            .iter()
            .map(|t| ("threat-violence".to_string(), t.clone()))
            .collect();
        let prototypes = triage::build_prototypes(&examples, |t| ec.embed(t)).expect("prototypes");

        // The same lazy phase-boundary swaps the command performs: embedder →
        // classifier on the first focused call, classifier → confirmer on the
        // first confirmation. Role: 0 = embedder, 1 = classifier, 2 = confirmer.
        let guard_model = std::env::var("TRIAGE_GUARD_MODEL").ok();
        let slot = RefCell::new((server, ec, 0u8));
        let ensure = |role: u8| {
            let mut s = slot.borrow_mut();
            if s.2 != role {
                s.0.shutdown();
                let (model, ctx): (&str, u32) = if role == 2 {
                    (guard_model.as_deref().expect("guard model env"), 16384)
                } else {
                    (&class_model, 8192)
                };
                let srv = spawn(model, false, ctx);
                let c = LlmClient::new(srv.base_url(), "eval", Duration::from_secs(300));
                *s = (srv, c, role);
            }
        };
        let embed = |t: &str| slot.borrow().1.embed(t);
        let classify = |w: &FocusWindow| {
            ensure(1);
            triage_scan::classify_focused(&slot.borrow().1, w)
        };

        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("parity", (None, None), "all", 1).unwrap();
        let outcome = triage_scan::run_triage(
            &mut db,
            scan,
            &threads,
            &prototypes,
            // No confirmation stage — the oracle's stages 1+2. The mode is
            // only consulted for confirm(); the CUT is pinned to 0.52 below,
            // because that is where the oracle's numbers were recorded.
            ScanMode::Thorough,
            ORACLE_CENSUS_CUT,
            None,
            1,
            embed,
            classify,
            |_: &FocusWindow, _| Ok(true),
            &CancelToken::new(),
            |p: TriageProgress| {
                if let TriageProgress::DeepScan { done, total, .. } = p {
                    if done % 20 == 0 {
                        eprintln!("deep-scan {done}/{total}");
                    }
                }
            },
        )
        .expect("run_triage");

        // Chunk-level scoring, same as the reference numbers were computed.
        let kept: BTreeSet<usize> = db
            .conn()
            .prepare("SELECT DISTINCT thread_identifier FROM census WHERE score >= 0.52")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap()[1..].parse::<usize>().unwrap())
            .collect();
        let kept_real = kept.iter().filter(|c| dump.chunks[**c].real).count();
        let ceiling = kept_real as f64 / n_real as f64;

        let findings = db.list_findings(Some(scan)).unwrap();
        let flagged: BTreeSet<usize> = findings
            .iter()
            .filter_map(|f| f.source_id)
            .map(|id| (id / 100) as usize)
            .collect();
        let tp = flagged.iter().filter(|c| dump.chunks[**c].real).count();
        let recall = tp as f64 / n_real as f64;
        let precision = if flagged.is_empty() {
            0.0
        } else {
            tp as f64 / flagged.len() as f64
        };

        println!(
            "wired pipeline @0.52: census ceiling {ceiling:.3} (ref 0.962) · \
             chunk recall {recall:.3} (ref 0.962) · precision {precision:.3} (ref 0.740) · \
             censused {} candidates {} deep-scanned {} findings {}",
            outcome.censused, outcome.candidates, outcome.deep_scanned, outcome.findings
        );

        // A model that produces nothing is a harness bug until proven
        // otherwise (journey §10.6): zero findings fails loudly rather than
        // being scored as a precision of convenience.
        assert!(
            !findings.is_empty(),
            "no findings at all — suspect the harness (grammar/prompt), not the model"
        );
        assert!(
            (ceiling - 0.9625).abs() <= 0.04,
            "census ceiling {ceiling:.3} drifted from the reference 0.9625"
        );
        assert!(
            recall >= 0.90,
            "chunk-level focused recall {recall:.3} below the reference band (ref 0.9625)"
        );
        assert!(
            (0.64..=0.86).contains(&precision),
            "chunk-level focused precision {precision:.3} outside the reference band (ref 0.740)"
        );

        // ---- optional: the CONFIRMATION stage (set TRIAGE_GUARD_MODEL) ----
        // Precise (threshold 0.58, confirm on) matches an oracle sweep point
        // exactly; its recorded guard-stage reference, chunk-level:
        // recall 0.8125, precision 0.9701 (from the 2026-08-12 sweep's stage
        // cache). The census is incremental, so this second run re-embeds
        // nothing and needs no embedder — prototypes are reused from phase A.
        if guard_model.is_some() {
            // begin_scan REUSES the row for an identical scope, so a DB query
            // by scan id would return run 1's findings merged in. The
            // confirmation stage is scored from the confirm closure's own
            // kept-set instead — the decision stream itself, immune to
            // scan-row and replace-findings semantics.
            let scan2 = db
                .begin_scan("parity-precise", (None, None), "messages", 2)
                .unwrap();
            let kept_chunks = RefCell::new(BTreeSet::<usize>::new());
            let classify2 = |w: &FocusWindow| {
                ensure(1);
                triage_scan::classify_focused(&slot.borrow().1, w)
            };
            let confirm2 = |w: &FocusWindow, _: &triage_scan::FocusVerdict| {
                ensure(2);
                let keep = crate::safety_scan::guard::confirm_focused(&slot.borrow().1, w)?;
                if keep {
                    kept_chunks
                        .borrow_mut()
                        .insert((w.items[w.focus].source_id / 100) as usize);
                }
                Ok(keep)
            };
            let out2 = triage_scan::run_triage(
                &mut db,
                scan2,
                &threads,
                &prototypes,
                ScanMode::Precise,
                ORACLE_CONFIRM_CUT, // the oracle's confirm sweep point, not the shipped mode's
                None,
                2,
                |_: &str| -> crate::Result<Vec<f32>> {
                    // Run 2 reuses run 1's census (same db, same corpus, same
                    // fingerprints) and every post-phase-A server is spawned
                    // WITHOUT --embedding — so an embed call here means the
                    // incremental-census invariant broke. Fail loudly at the
                    // cause instead of as an opaque mid-run HTTP error.
                    panic!("run 2 must not embed — the incremental census should have zero work")
                },
                classify2,
                confirm2,
                &CancelToken::new(),
                |p: TriageProgress| match p {
                    TriageProgress::Census { total, .. } => {
                        assert_eq!(total, 0, "run 2 re-embedded — census not incremental");
                    }
                    TriageProgress::Confirm { done, total } => {
                        if done % 20 == 0 {
                            eprintln!("confirm {done}/{total}");
                        }
                    }
                    TriageProgress::DeepScan { .. } => {}
                },
            )
            .expect("run_triage precise");
            let _ = scan2;
            let cflag = kept_chunks.borrow().clone();
            let ctp = cflag.iter().filter(|c| dump.chunks[**c].real).count();
            let crec = ctp as f64 / n_real as f64;
            let cprec = if cflag.is_empty() {
                0.0
            } else {
                ctp as f64 / cflag.len() as f64
            };
            println!(
                "wired pipeline @0.58+confirm: chunk recall {crec:.3} (ref 0.813) · precision {cprec:.3} (ref 0.970) · findings {} unconfirmed {}",
                out2.findings, out2.unconfirmed
            );
            assert!(
                out2.unconfirmed > 0 || out2.findings == 0,
                "the confirmer vetoed nothing at all — suspect the harness (a Guard driven through a chat template answers nothing useful)"
            );
            assert!(
                crec >= 0.74,
                "confirmed chunk-level recall {crec:.3} below the reference band (ref 0.8125)"
            );
            assert!(
                cprec >= 0.90,
                "confirmed chunk-level precision {cprec:.3} below the reference band (ref 0.9701)"
            );
        }
        slot.borrow_mut().0.shutdown();
    }

    /// The other half of a baseline (#407): how fast a scan actually goes.
    ///
    /// The eval above measures quality over 2–4 message cases; a real chunk is
    /// [`WINDOW`] messages, so its numbers say nothing about throughput. This
    /// times FULL-SIZE chunks through the same client, prompt and grammar the
    /// engine uses, and reports chunks per minute — the unit every speed
    /// proposal in #397 is argued in.
    ///
    /// No backup is involved, real or fixture: the messages are generated here.
    /// A scan's cost is dominated by prefill over chunk text of a given size,
    /// and synthetic text of that size measures it without touching anyone's
    /// data.
    ///
    ///   TRACELOUPE_EVAL_MODEL=/path/model.gguf \
    ///   TRACELOUPE_LLAMA_SERVER=/path/llama-server \
    ///   TRACELOUPE_BENCH_CHUNKS=8 TRACELOUPE_BENCH_PARALLEL=1 \
    ///   cargo test -p traceloupe-core measure_scan_throughput -- --ignored --nocapture
    /// Dump the PRODUCTION verdict grammars to `$TRACELOUPE_GRAMMAR_OUT`
    /// (default /tmp/grammars.json) as `{ "<n>": "<gbnf>", ... }` for item
    /// counts 1..=30. The external triage validation harness
    /// (`tools/validate-triage-pipeline.py`) sends these verbatim so it can
    /// never drift into reimplementing GBNF — a mistake that produced false
    /// "recall 0.00" results three times during the rebuild (see
    /// docs/safety-scan-journey.md §10.6).
    #[test]
    #[ignore = "writes the grammar file the validation harness needs"]
    fn dump_grammars() {
        let out = std::env::var("TRACELOUPE_GRAMMAR_OUT")
            .unwrap_or_else(|_| "/tmp/grammars.json".to_string());
        let mut s = String::from("{");
        for n in 1..=30usize {
            if n > 1 {
                s.push(',');
            }
            s.push_str(&format!(
                "\"{n}\":{}",
                serde_json::to_string(&crate::safety_scan::prompt::verdicts_grammar(n)).unwrap()
            ));
        }
        s.push('}');
        std::fs::write(&out, s).unwrap();
        eprintln!("wrote grammars to {out}");
    }

    #[test]
    #[ignore = "requires a local GGUF + llama-server (set TRACELOUPE_EVAL_MODEL)"]
    fn measure_scan_throughput() {
        use crate::safety_scan::chunker::{Chunk, ChunkItem};
        // Sweepable: the window is a configuration choice, and its cost has to
        // be measured at the size we might actually ship.
        let window: usize = std::env::var("TRACELOUPE_BENCH_WINDOW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(crate::safety_scan::chunker::WINDOW);
        #[allow(non_snake_case)]
        let WINDOW = window;
        use crate::safety_scan::client::LlmClient;
        use crate::safety_scan::{engine, prompt};
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let Ok(model) = std::env::var("TRACELOUPE_EVAL_MODEL") else {
            eprintln!("set TRACELOUPE_EVAL_MODEL to measure throughput");
            return;
        };
        let chunks_to_run: usize = std::env::var("TRACELOUPE_BENCH_CHUNKS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        let parallel: u32 = std::env::var("TRACELOUPE_BENCH_PARALLEL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        // A SECOND mundane corpus, deliberately unlike the first. The domestic
        // one below is a couple coordinating an evening, which contains
        // check-ins ("text me if you are late") that a coercive-control
        // classifier could reasonably latch onto — so measuring false alarms
        // only against it risks reporting a property of the fixture as a
        // property of the model. This one has no relationship in it at all.
        const WORK_LINES: &[&str] = &[
            "the deploy finished, staging looks fine",
            "great, i will run through the smoke tests after lunch",
            "did the migration script take long in the end",
            "about forty minutes, mostly the index rebuild",
            "worth doing it out of hours next time then",
            "agreed. i will put it in the runbook",
            "any word from the vendor about the api limits",
            "they raised it to ten thousand a day, should be plenty",
            "good. that was the last blocker for the release",
            "i will draft the notes this afternoon",
            "send them over and i will review before we publish",
            "will do. anything you want called out specifically",
            "the caching change, people have been asking for it",
            "makes sense. i will lead with that",
            "thanks. how is the new starter settling in",
            "well, picked up the codebase faster than i expected",
            "good to hear. pair them on the next ticket",
            "planning to. they asked for something backend heavy",
            "plenty of that around at the moment",
            "there always is. i will sort it in standup tomorrow",
            "sounds good. i am off at four today, dentist",
            "no problem, i will cover the release call",
            "appreciate it. talk tomorrow",
            "see you tomorrow",
            "bye",
        ];

        // Ordinary conversation, deliberately unremarkable: a sweep's cost is
        // paid on chunks that produce nothing, and that is what we are timing.
        const LINES: &[&str] = &[
            "are you still coming over later or did something come up",
            "yeah should be there around seven, traffic depending",
            "no rush, dinner is not until eight anyway",
            "did you remember to pick up the thing for your mum",
            "i did, it is in the car so i do not forget it again",
            "ha, like last time. she still brings that up you know",
            "i know, i know. i will never live it down",
            "anyway how did the meeting go this morning",
            "long. three hours for something that was an email",
            "that sounds about right for them honestly",
            "at least it is done. i can actually get work done now",
            "want me to bring anything, wine or something",
            "wine would be lovely if you are passing the shop",
            "will do. red or white, i can never remember",
            "red please, the one we had at christmas if they have it",
            "no idea what that was but i will ask someone",
            "just get whatever, honestly it does not matter",
            "famous last words, you will judge me at the table",
            "i would never. out loud, anyway",
            "see you at seven then. text me if you are late",
            "always do. do not start without me this time",
            "no promises, you know how the kids get when hungry",
            "fair enough. feed them, save me a plate",
            "deal. see you soon",
            "see you soon",
        ];

        let binary = crate::safety_scan::server::resolve_binary()
            .expect("set TRACELOUPE_LLAMA_SERVER or bundle a sidecar");
        let port = crate::safety_scan::server::pick_port().unwrap();
        let mut server = crate::safety_scan::server::LlamaServer::spawn(
            &crate::safety_scan::server::ServerConfig {
                binary,
                model_path: PathBuf::from(&model),
                port,
                // Matches production: total context is divided across slots.
                ctx_size: 8192 * parallel,
                parallel,
                api_key: None,
                gpu_layers: -1,
                embedding: false,
                sandbox: true,
                scratch_dir: std::env::temp_dir().join("traceloupe-bench-scratch"),
            },
            None,
        )
        .expect("spawn llama-server");
        let load_started = Instant::now();
        server
            .wait_healthy(Duration::from_secs(300))
            .expect("model load");
        let load_secs = load_started.elapsed().as_secs_f64();
        let client = LlmClient::new(server.base_url(), "bench", Duration::from_secs(300));

        // Which corpus: "work" has no relationship in it, "domestic" does.
        let corpus: &[&str] = match std::env::var("TRACELOUPE_BENCH_CORPUS").as_deref() {
            Ok("work") => WORK_LINES,
            _ => LINES,
        };
        let chunk_of = |n: usize| Chunk {
            key: format!("bench-{n}"),
            fingerprint: format!("bench-{n}"),
            kind: crate::analysis::SourceKind::Message,
            thread_identifier: Some("+15550000000".into()),
            label: None,
            service: Some("iMessage".into()),
            items: (0..WINDOW)
                .map(|i| ChunkItem {
                    source_id: (n * WINDOW + i) as i64,
                    sender: if i % 2 == 0 {
                        "them".into()
                    } else {
                        "me".into()
                    },
                    occurred_at: Some(1_700_000_000 + i as i64),
                    // Vary per chunk so no two prompts are identical — an
                    // identical prompt would be served from the prefix cache
                    // and report a speed no real scan ever sees.
                    text: format!("{} ({n})", corpus[i % corpus.len()]),
                    fingerprint: format!("bench-{n}-{i}"),
                })
                .collect(),
        };

        // One warm-up, uncounted: the first call pays for graph setup and Metal
        // shader compilation, which a 5000-chunk scan pays once and would be
        // pure distortion spread over eight.
        let warm = chunk_of(999);
        let _ = client.chat_json(
            prompt::SYSTEM_PROMPT,
            &prompt::render_chunk(&warm),
            &prompt::verdicts_grammar(warm.items.len()),
            1200,
        );

        let mut prompt_chars = 0usize;
        let started = Instant::now();
        let mut failures = 0usize;
        // Every one of these chunks is mundane conversation. A verdict on any
        // of them is a false alarm, and counting them here measures the thing
        // the user actually complains about, in the most direct form there is.
        let mut findings = 0usize;
        // Severity and category of every false alarm. If the noise is
        // concentrated at severity 1, a floor removes it for free; if it is
        // spread across 2 and 3, only the model or the prompt can.
        let mut by_severity = [0usize; 4];
        let mut by_category: std::collections::BTreeMap<&'static str, usize> = Default::default();
        for n in 0..chunks_to_run {
            let chunk = chunk_of(n);
            let user = prompt::render_chunk(&chunk);
            prompt_chars += prompt::SYSTEM_PROMPT.len() + user.len();
            match client.chat_json(
                prompt::SYSTEM_PROMPT,
                &user,
                &prompt::verdicts_grammar(chunk.items.len()),
                1200,
            ) {
                Ok(out) => {
                    for f in engine::verdicts_to_findings_for_eval(&chunk, &out) {
                        findings += 1;
                        by_severity[(f.severity as usize).min(3)] += 1;
                        *by_category.entry(f.category.as_str()).or_default() += 1;
                    }
                }
                Err(e) => {
                    eprintln!("chunk {n}: {e}");
                    failures += 1;
                }
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        server.shutdown();

        let per_chunk = elapsed / chunks_to_run as f64;
        println!("\n=== Safety Scan throughput ===");
        println!("model             {model}");
        println!("model load        {load_secs:.1}s");
        println!("chunks            {chunks_to_run} x {WINDOW} messages, parallel={parallel}");
        println!("failures          {failures}");
        println!("elapsed           {elapsed:.1}s");
        println!("per chunk         {per_chunk:.2}s");
        println!("throughput        {:.1} chunks/min", 60.0 / per_chunk);
        println!(
            "prompt size       ~{} chars/chunk",
            prompt_chars / chunks_to_run
        );
        println!(
            "false alarms      {findings} on {} mundane messages ({:.1} per chunk)",
            chunks_to_run * WINDOW,
            findings as f64 / chunks_to_run as f64
        );
        println!(
            "  by severity     1:{}  2:{}  3:{}",
            by_severity[1], by_severity[2], by_severity[3]
        );
        for (cat, n) in &by_category {
            println!("  {cat:<24} {n}");
        }
        // What this means for a real backup, stated so nobody has to redo the
        // arithmetic: stride is WINDOW - OVERLAP.
        for messages in [10_000usize, 100_000] {
            let chunks = messages / (WINDOW - crate::safety_scan::chunker::OVERLAP);
            println!(
                "{messages:>7} messages  ~{chunks} chunks  ~{:.0} min",
                chunks as f64 * per_chunk / 60.0
            );
        }
        assert!(
            failures * 4 <= chunks_to_run,
            "{failures} of {chunks_to_run} chunks failed — too many for the timing to mean anything"
        );
    }
}
