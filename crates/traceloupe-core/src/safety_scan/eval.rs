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
//!   it. See `docs/safety-scan-validation.md`.

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
    /// Count of hard-negative (expect-nothing) cases — the clean-rate divisor.
    pub negatives_scored: usize,
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
            "\nhard-negative clean rate: {:.2} ({} false alarms of {} negatives)\n",
            self.negative_clean_rate(),
            self.false_alarms.len(),
            self.negatives_scored
        ));
        if !self.false_alarms.is_empty() {
            out.push_str(&format!("false alarms: {}\n", self.false_alarms.join(", ")));
        }
        out
    }
}

/// Score a classifier over every fixture. `classify` returns the set of
/// categories the classifier flagged for a case's messages. Pure function of
/// the classifier — the live test and the golden test share it.
pub fn score_against(
    fixtures: &Fixtures,
    mut classify: impl FnMut(&Case) -> BTreeSet<Category>,
) -> ScoreReport {
    let mut report = ScoreReport::default();
    for case in &fixtures.cases {
        let expected = case.expected_categories();
        let predicted = classify(case);
        for cat in Category::ALL {
            let e = expected.contains(&cat);
            let p = predicted.contains(&cat);
            let s = report.per_category.entry(cat).or_default();
            match (e, p) {
                (true, true) => s.tp += 1,
                (false, true) => s.fp += 1,
                (true, false) => s.fn_ += 1,
                (false, false) => {}
            }
        }
        if expected.is_empty() {
            report.negatives_scored += 1;
            if !predicted.is_empty() {
                report.false_alarms.push(case.id.clone());
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
        assert!(f.cases.len() >= 15, "want a meaningful fixture set");
        let positives = f.cases.iter().filter(|c| c.kind == "positive").count();
        let negatives = f.cases.iter().filter(|c| c.kind == "negative").count();
        assert!(positives >= 10, "want >=10 positive cases");
        assert!(negatives >= 5, "want >=5 hard negatives");
    }

    #[test]
    fn every_category_has_at_least_one_positive() {
        let f = load_fixtures();
        let covered: BTreeSet<Category> = f
            .cases
            .iter()
            .flat_map(|c| c.expected_categories())
            .collect();
        for cat in Category::ALL {
            assert!(covered.contains(&cat), "no fixture covers {}", cat.as_str());
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
                gpu_layers: -1,
                sandbox: true,
                scratch_dir: std::env::temp_dir().join("traceloupe-eval-scratch"),
            },
        )
        .expect("spawn llama-server");
        server
            .wait_healthy(Duration::from_secs(180))
            .expect("model load");
        let client = LlmClient::new(server.base_url(), "eval", Duration::from_secs(300));
        let schema = prompt::verdicts_schema();

        let fixtures = load_fixtures();
        let report = score_against(&fixtures, |case| {
            // Build a single chunk from the case's messages, classify it, and
            // collapse verdicts to the set of categories seen.
            let items: Vec<ChunkItem> = case
                .messages
                .iter()
                .enumerate()
                .map(|(i, m)| ChunkItem {
                    source_id: i as i64,
                    sender: if m.sender == "me" {
                        "me".into()
                    } else {
                        "them".into()
                    },
                    occurred_at: Some(1000 + i as i64),
                    text: m.text.clone(),
                    fingerprint: format!("{}:{i}", case.id),
                })
                .collect();
            let chunk = Chunk {
                key: case.id.clone(),
                fingerprint: case.id.clone(),
                kind: crate::analysis::SourceKind::Message,
                thread_identifier: Some(case.id.clone()),
                label: None,
                items,
            };
            let user = prompt::render_chunk(&chunk);
            match client.chat_json(prompt::SYSTEM_PROMPT, &user, &schema, 1200) {
                Ok(output) => engine::verdicts_to_findings_for_eval(&chunk, &output)
                    .into_iter()
                    .map(|f| f.category)
                    .collect(),
                Err(e) => {
                    eprintln!("{}: {e}", case.id);
                    BTreeSet::new()
                }
            }
        });
        server.shutdown();
        println!("\n=== Safety Scan live eval ===\n{}", report.table());
        // A release gate could assert per-category recall/precision floors
        // here; left as a print so a human reviews the numbers first.
    }
}
