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
