//! The hate / identity-attack tier: explicit attacks on someone for who they
//! are, found without a model download and without the generative classifier.
//!
//! Trained on UC Berkeley's *Measuring Hate Speech* (CC BY 4.0 — **the product
//! owes this corpus attribution in its notices**), 39,565 real texts rated by
//! ~2 annotators each with the targeted identity recorded. Positives are
//! identity-targeted (race, religion, origin, gender, sexuality, age,
//! disability); negatives are the corpus's clearly-not-hate end.
//!
//! **What it catches, stated exactly.** Explicit hate. At the shipped cut it
//! finds 28% of message-length hate in held-out real data while flagging **0 of
//! 4,827 real personal SMS** and 0 of 30 legitimate-identity-talk checklist
//! items. It does NOT catch implied or coded hate — "people like you should not
//! be allowed in this country" scores 0.38 against a 0.978 cut. That is not a
//! limit of word counting: a ModernBERT trained on the same corpus missed the
//! same six implied lines. The corpus is dominated by explicit hate, so no
//! model trained on it learns the coded kind, and the coverage report must not
//! imply otherwise.
//!
//! **Why not the network.** Pooled over 3 folds ModernBERT caught 95% against
//! the classical baseline's 88-91% — and then lost where it matters. Its scores
//! on ordinary text have a longer tail, so the high-precision cut a findings
//! tier must run at pushes its threshold near 1.0: 20% of message-length hate
//! caught, against this model's 28%, for 150 MB of download. Measured, recorded,
//! rejected — the same rule that held back the civil heads (#527) and the
//! neural router (#545).
//!
//! **Why this one may carry a threshold when the router may not.** The router
//! ships as int8 ONNX, where quantisation moved individual scores by up to
//! 0.95; a cut would have meant something different after export. This ships as
//! plain floats, so the calibrated cut means in Rust exactly what it meant in
//! scikit-learn — and the parity fixture proves it.

use std::sync::OnceLock;

use super::lexical::LexicalModel;

/// Weights and the calibrated cut, produced by `harness/export_hate_tier.py`.
/// Per-term weights only; no corpus text is reproduced.
const MODEL_JSON: &str = include_str!("../../fixtures/safety-scan/hate-lexical.json");

/// Shortest text worth judging. A one-word message carries no context and the
/// vocabulary alone would convict a quoted word.
const MIN_WORDS: usize = 2;

pub struct HateTier {
    model: LexicalModel,
    threshold: f32,
}

pub fn shipped() -> &'static HateTier {
    static TIER: OnceLock<HateTier> = OnceLock::new();
    TIER.get_or_init(|| {
        let model = LexicalModel::from_json(MODEL_JSON);
        let threshold = model
            .threshold()
            .expect("the hate model ships a calibrated threshold");
        HateTier { model, threshold }
    })
}

impl HateTier {
    /// Whether this message is an explicit identity attack.
    ///
    /// Deliberately a boolean: the score is calibrated for one operating point
    /// and nothing downstream should invent a second one from it.
    pub fn is_hate(&self, text: &str) -> bool {
        self.strength(text).is_some()
    }

    /// The score, but ONLY for messages already over the cut — for choosing
    /// which message in a thread to anchor a finding on. A marginal early hit
    /// must not outrank a blatant later one (#540 finding 5). Not a second
    /// threshold: callers may compare these to each other, never to a constant.
    pub fn strength(&self, text: &str) -> Option<f32> {
        if text.split_whitespace().count() < MIN_WORDS {
            return None;
        }
        let s = self.model.score(text);
        (s >= self.threshold).then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard for the transform: the exporter writes texts with
    /// scikit-learn's own probabilities and this asserts the Rust arithmetic
    /// agrees. The fixture's texts are ordinary or hateful-without-slurs,
    /// because it ships in a public repository; a real firing example lives
    /// outside the repo behind TRACELOUPE_HATE_SMOKE, as the grooming
    /// detector's does.
    #[test]
    fn the_rust_transform_matches_scikit_learns() {
        #[derive(serde::Deserialize)]
        struct Case {
            text: String,
            score: f32,
        }
        #[derive(serde::Deserialize)]
        struct Fixture {
            threshold: f32,
            cases: Vec<Case>,
        }
        let raw = include_str!("../../fixtures/safety-scan/hate-parity.json");
        let fixture: Fixture = serde_json::from_str(raw).unwrap();
        let tier = shipped();
        assert!(
            (tier.threshold - fixture.threshold).abs() < 1e-6,
            "the shipped cut and the fixture's disagree"
        );
        for case in &fixture.cases {
            let got = tier.model.score(&case.text);
            assert!(
                (got - case.score).abs() < 1e-3,
                "score drifted on {:?}: rust {got}, python {}",
                case.text.chars().take(48).collect::<String>(),
                case.score
            );
        }
    }

    /// Ordinary talk about identity is not an attack on it. These are the cases
    /// the tier exists to NOT flag — the measured version of this is 0 of 4,827
    /// real personal messages.
    #[test]
    fn ordinary_identity_talk_is_not_flagged() {
        let tier = shipped();
        for text in [
            "picking mum up from the mosque at 2",
            "the polish shop on the corner has the good bread",
            "shes jewish so she cant do friday night, sunday better?",
            "nans dementia is worse, she called me by my mums name again",
            "hey bitches, brunch sunday??",
            "you absolute muppet, you left the tickets at home didnt you",
            "he called me a slur in front of the whole class and nobody helped",
            "queer film night at mine on thursday, bring snacks",
        ] {
            assert!(!tier.is_hate(text), "flagged ordinary traffic: {text:?}");
        }
    }

    /// A single word is never judged: no context, and the vocabulary alone
    /// would convict someone quoting a word rather than using it.
    #[test]
    fn a_one_word_message_is_never_judged() {
        assert!(!shipped().is_hate("idiot"));
        assert!(!shipped().is_hate(""));
    }

    /// Real hate, kept OUTSIDE the repo (same contract as the grooming smoke
    /// test): one line per example, expected to fire.
    #[test]
    fn real_hate_fires_when_the_smoke_file_is_present() {
        let Ok(path) = std::env::var("TRACELOUPE_HATE_SMOKE") else {
            eprintln!("skipped: needs TRACELOUPE_HATE_SMOKE");
            return;
        };
        let raw = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
        let fired = lines.iter().filter(|l| shipped().is_hate(l)).count();
        // The bar is the MEASURED recall, not a hoped-for one. This tier runs
        // at a precision-first cut and finds roughly 28% of message-length
        // hate; on 40 lines that is ~11, and the first draft of this test
        // asserted 50% and failed against the tier working as designed.
        // 15% catches a real regression (a broken vocabulary fires nothing)
        // without pretending to a recall the tier does not have.
        assert!(
            fired * 100 >= lines.len() * 15,
            "only {fired}/{} known-hateful lines fired — expected ~28%, so the \
             tier has regressed",
            lines.len()
        );
    }
}
