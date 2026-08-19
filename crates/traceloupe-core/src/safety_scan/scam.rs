//! The scam/smishing tier (#539): a compact classical classifier, embedded.
//!
//! Measured before built. Hand-written structural rules caught 46% of real
//! held-out smishing; this model catches **92% at 2.2% false alarms** on the
//! same split (UCI + Mendeley, both CC BY 4.0, deduplicated by text first —
//! Mendeley includes UCI, and without dedup 55% of test scam appears verbatim
//! in train, which inflated an early run to a meaningless 98%).
//!
//! Scam is lexically distinctive — the reason bag-of-words spam filters have
//! worked since the 1990s — and it is the opposite of coercive control, where
//! the words are ordinary and only the pattern betrays it. Hence: words here,
//! arithmetic there.
//!
//! No ML runtime, no download: the whole detector is 536 KB of weights, a
//! hashmap lookup and a dot product. Per message it costs microseconds.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const MODEL_JSON: &str = include_str!("../../fixtures/safety-scan/scam-model.json");

#[derive(Deserialize)]
struct RawModel {
    threshold: f64,
    intercept: f64,
    /// term -> (logistic coefficient, idf)
    features: HashMap<String, (f64, f64)>,
}

pub struct ScamModel {
    threshold: f64,
    intercept: f64,
    features: HashMap<String, (f64, f64)>,
}

static MODEL: OnceLock<ScamModel> = OnceLock::new();

pub fn model() -> &'static ScamModel {
    MODEL.get_or_init(|| {
        let raw: RawModel = serde_json::from_str(MODEL_JSON).expect("scam-model.json is valid");
        ScamModel {
            threshold: raw.threshold,
            intercept: raw.intercept,
            features: raw.features,
        }
    })
}

/// Tokenisation must match the trainer's (scikit-learn's default analyzer):
/// lowercase, then word tokens of 2+ alphanumeric/underscore characters,
/// plus adjacent bigrams. A drift here is a silent accuracy change, so the
/// parity test pins it against the Python scores.
fn tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let words: Vec<String> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_string)
        .collect();
    let mut out = words.clone();
    for pair in words.windows(2) {
        out.push(format!("{} {}", pair[0], pair[1]));
    }
    out
}

impl ScamModel {
    /// The scam probability for one message.
    pub fn score(&self, text: &str) -> f64 {
        let toks = tokens(text);
        if toks.is_empty() {
            return 0.0;
        }
        // Raw term counts -> tf-idf with L2 normalisation, as the trainer does.
        let mut counts: HashMap<&str, f64> = HashMap::new();
        for t in &toks {
            if self.features.contains_key(t) {
                *counts.entry(t.as_str()).or_default() += 1.0;
            }
        }
        let mut norm = 0.0;
        for (t, c) in &counts {
            let (_, idf) = self.features[*t];
            norm += (c * idf).powi(2);
        }
        // A message with no known feature carries no evidence either way.
        if norm == 0.0 {
            return 0.0;
        }
        let norm = norm.sqrt();
        let mut z = self.intercept;
        for (t, c) in &counts {
            let (coef, idf) = self.features[*t];
            z += coef * (c * idf) / norm;
        }
        1.0 / (1.0 + (-z).exp())
    }

    pub fn is_scam(&self, text: &str) -> bool {
        self.score(text) >= self.threshold
    }

    pub fn threshold(&self) -> f64 {
        self.threshold
    }
}

/// Structural signals, kept from the rule work — NOT the detector (they caught
/// 46% where the model catches 92%), but the readable reason a finding exists.
/// Measured lift over ordinary SMS in parentheses.
pub fn explain(text: &str) -> Vec<&'static str> {
    let t = text.to_lowercase();
    let mut out = Vec::new();
    if t.contains("http") || t.contains("www.") || t.contains("bit.ly") {
        out.push("contains a link");
    }
    if regex_lite_premium(&t) {
        out.push("asks you to call a premium-rate number");
    }
    for (needle, label) in [
        ("won", "claims you have won something"),
        ("prize", "claims you have won something"),
        ("refund", "offers a refund"),
        ("urgent", "presses for immediate action"),
        ("expires", "presses for immediate action"),
        ("verify", "asks you to verify account details"),
        ("password", "asks for credentials"),
        ("parcel", "references a delivery"),
    ] {
        if t.contains(needle) && !out.contains(&label) {
            out.push(label);
        }
    }
    out
}

/// `09`/`08` premium prefixes followed by 8 digits — no regex crate needed.
fn regex_lite_premium(lower: &str) -> bool {
    let b = lower.as_bytes();
    b.windows(10).any(|w| {
        (w.starts_with(b"09") || w.starts_with(b"08")) && w.iter().all(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_loads_and_carries_its_measurement() {
        let m = model();
        assert!(m.threshold > 0.0 && m.threshold < 1.0);
        assert!(
            m.features.len() > 5_000,
            "the exported vocabulary looks truncated: {}",
            m.features.len()
        );
    }

    #[test]
    fn ordinary_messages_do_not_flag() {
        let m = model();
        for t in [
            "can you grab milk on the way home",
            "the meeting moved to 3pm, see you there",
            "running about 20 minutes late, sorry",
            "happy birthday!! hope you have a lovely day x",
        ] {
            assert!(!m.is_scam(t), "false alarm on {t:?} (score {})", m.score(t));
        }
    }

    /// Rust must reproduce the Python model's DECISIONS on real held-out
    /// messages, or the tokenisation has drifted and the measured 92%/2.2% no
    /// longer describes what ships. Fixture lives outside the repo (real SMS
    /// text is not committed); point TRACELOUPE_SCAM_PARITY at it.
    #[test]
    fn rust_scoring_matches_the_python_model() {
        let Ok(path) = std::env::var("TRACELOUPE_SCAM_PARITY") else {
            eprintln!("skipped: set TRACELOUPE_SCAM_PARITY");
            return;
        };
        #[derive(serde::Deserialize)]
        struct Case {
            text: String,
            python_score: f64,
        }
        let cases: Vec<Case> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let m = model();
        let (mut agree, mut worst) = (0usize, 0.0f64);
        for c in &cases {
            let ours = m.score(&c.text);
            worst = worst.max((ours - c.python_score).abs());
            if (ours >= m.threshold()) == (c.python_score >= m.threshold()) {
                agree += 1;
            }
        }
        assert_eq!(
            agree,
            cases.len(),
            "decision parity broken on {} of {} held-out messages (worst score gap {worst:.4})",
            cases.len() - agree,
            cases.len()
        );
    }

    #[test]
    fn explanations_only_name_what_is_present() {
        let e = explain("Your parcel is held, verify at http://x.co");
        assert!(e.contains(&"contains a link"));
        assert!(e.contains(&"references a delivery"));
        assert!(!e.contains(&"claims you have won something"));
        assert!(explain("see you at 8").is_empty());
    }
}
