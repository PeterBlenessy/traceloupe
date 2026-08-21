//! The deep-scan router (#544): which conversations the expensive model reads
//! FIRST, and which ones it reads at all when a budget bites.
//!
//! Until now that order came from the census — one embedding per message,
//! scored by cosine against hand-written centroids. Measured against real harm
//! inside real traffic (6,000 ordinary chat conversations, harm planted at a
//! 1.1% base rate, plants held out from every model involved), that ordering is
//! barely better than shuffling: it ranks a predator conversation above an
//! ordinary one 62% of the time, and ranks self-harm BELOW ordinary chatter.
//! Two structural reasons, both inherent to the method: a long message
//! mean-pools into a diffuse vector far from centroids built from chat lines,
//! and a conversation scores as the MAX over its messages, so a ten-message
//! thread gets ten tickets in the lottery and a one-message thread gets one.
//! Rebuilding those centroids from real held-out positives made it WORSE
//! (0.514), which rules out better examples as the fix.
//!
//! This model ranks the same haystack at 0.996 (grooming) and 0.996
//! (self-harm), holding 27/27 planted grooming conversations and 20/20
//! self-harm disclosures in the top 5% of the phone, where the census holds
//! 4/27 and 0/20.
//!
//! **Why word counting and not a network.** A ModernBERT-base was trained,
//! quantised and measured on exactly this haystack; it scored the same
//! (0.991/0.792 register-matched, both of them) for 151 MB of download and
//! ~14 ms per conversation. It bought one thing: six fewer ordinary messages
//! near the top of the list. By the rule that held back the civil heads in
//! #527 — a tier must earn its cost — it did not ship. This is 1.9 MB compiled
//! into the binary, with no artefact to download, no checksum to pin and
//! nothing to keep in sync.
//!
//! Two properties this tier deliberately does NOT have:
//!
//! * **No findings.** It decides reading order. Every verdict still comes from
//!   the model that can justify one.
//! * **No threshold.** Everything is expressed as a rank, so the tier cannot
//!   acquire a hidden decision boundary that nobody re-measures.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::chunker::ChunkItem;
use super::grooming_onnx::{render_window, WINDOW_MESSAGES};

/// Share of a phone's threads the router may PROMOTE — threads the census kept
/// nothing from, which would otherwise never be deep-scanned however they rank.
/// 5% is the operating point the experiment measured (27/27 grooming, 20/20
/// self-harm inside it), expressed as a rank so no score can move it.
pub const PROMOTE_TOP_SHARE: f64 = 0.05;

/// The trained weights: `crates/traceloupe-core/fixtures/safety-scan/router-lexical.json`,
/// produced by `harness/export_lexical_router.py` in the training repo. Term
/// weights only — no corpus text, which PAN12's terms forbid redistributing.
const MODEL_JSON: &str = include_str!("../../fixtures/safety-scan/router-lexical.json");

#[derive(Deserialize)]
struct RawTerm {
    t: String,
    idf: f32,
    w: f32,
}

#[derive(Deserialize)]
struct RawModel {
    intercept: f32,
    terms: Vec<RawTerm>,
}

pub struct LexicalRouter {
    /// term -> (idf, weight). Both 1-grams and 2-grams; a 2-gram is its two
    /// tokens joined by one space, as scikit-learn writes them.
    terms: HashMap<String, (f32, f32)>,
    intercept: f32,
}

/// The shipped model, parsed once. ~43k terms; parsing is milliseconds and
/// happens on the first scan, not at startup.
pub fn shipped() -> &'static LexicalRouter {
    static MODEL: OnceLock<LexicalRouter> = OnceLock::new();
    MODEL.get_or_init(|| {
        let raw: RawModel = serde_json::from_str(MODEL_JSON)
            .expect("the router model ships with the binary; a parse failure is a build error");
        LexicalRouter {
            terms: raw.terms.into_iter().map(|t| (t.t, (t.idf, t.w))).collect(),
            intercept: raw.intercept,
        }
    })
}

/// scikit-learn's preprocessing, in order: lowercase, THEN strip accents
/// (NFKD, drop combining marks). The order matters — reversing it changes the
/// tokens for some scripts — and the parity fixture covers it.
fn normalise(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.to_lowercase()
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect()
}

/// `(?u)\b\w\w+\b`: maximal runs of word characters, keeping those of two or
/// more. `\w` is alphanumeric or underscore — after NFKD stripping there are no
/// combining marks left to consider.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            cur.push(c);
        } else if cur.chars().count() >= 2 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.clear();
        }
    }
    if cur.chars().count() >= 2 {
        out.push(cur);
    }
    out
}

impl LexicalRouter {
    /// One text's score in [0, 1]. Used only to ORDER, never compared to a
    /// constant.
    ///
    /// tf-idf exactly as scikit-learn computes it for this configuration:
    /// sublinear tf (`1 + ln(count)`), multiplied by the trained idf,
    /// l2-normalised over the terms present, then the logistic link. Terms
    /// outside the vocabulary are dropped BEFORE the norm, which is what
    /// scikit-learn does and is load-bearing for parity.
    pub fn score(&self, text: &str) -> f32 {
        let toks = tokenize(&normalise(text));
        let mut counts: HashMap<&str, f32> = HashMap::new();
        let mut bigrams: Vec<String> = Vec::with_capacity(toks.len().saturating_sub(1));
        for pair in toks.windows(2) {
            bigrams.push(format!("{} {}", pair[0], pair[1]));
        }
        for gram in toks
            .iter()
            .map(String::as_str)
            .chain(bigrams.iter().map(String::as_str))
        {
            if let Some((term, _)) = self.terms.get_key_value(gram) {
                *counts.entry(term.as_str()).or_insert(0.0) += 1.0;
            }
        }
        if counts.is_empty() {
            return sigmoid(self.intercept);
        }
        let mut vec: Vec<(f32, f32)> = Vec::with_capacity(counts.len());
        for (term, count) in &counts {
            let (idf, w) = self.terms[*term];
            vec.push(((1.0 + count.ln()) * idf, w));
        }
        let norm = vec.iter().map(|(v, _)| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return sigmoid(self.intercept);
        }
        let z = vec.iter().map(|(v, w)| (v / norm) * w).sum::<f32>() + self.intercept;
        sigmoid(z)
    }

    /// A thread's score: the higher of its first and last window, matching the
    /// grooming detector's measured windowing — early approach and recent
    /// escalation are both visible.
    ///
    /// Returns the score AND the index of the last message of the window that
    /// produced it, so a promoted thread deep-links to scored evidence rather
    /// than to the top of a years-long conversation.
    pub fn thread_score(&self, messages: &[ChunkItem]) -> (f32, usize) {
        if messages.is_empty() {
            return (0.0, 0);
        }
        let head_end = messages.len().min(WINDOW_MESSAGES);
        let head = self.score(&render_window(&messages[..head_end]));
        let mut best = (head, head_end - 1);
        if messages.len() > WINDOW_MESSAGES {
            let tail = self.score(&render_window(
                &messages[messages.len() - WINDOW_MESSAGES..],
            ));
            if tail > best.0 {
                best = (tail, messages.len() - 1);
            }
        }
        best
    }
}

fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

/// How many threads a phone of `total_threads` may promote. Rank-based, and at
/// least one so a small backup is not rounded down to nothing.
pub fn promotion_budget(total_threads: usize) -> usize {
    if total_threads == 0 {
        return 0;
    }
    ((total_threads as f64 * PROMOTE_TOP_SHARE).round() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_is_a_share_of_the_phone_and_never_zero() {
        assert_eq!(promotion_budget(0), 0, "no threads, nothing to promote");
        assert_eq!(promotion_budget(1), 1, "a tiny backup still gets one");
        assert_eq!(promotion_budget(100), 5);
        assert_eq!(promotion_budget(6_000), 300);
    }

    /// THE guard for this module. Re-implementing scikit-learn's transform is
    /// the kind of thing that fails silently — a different accent rule, a
    /// bigram joined with the wrong separator, tf without the log — and every
    /// score shifts a little while nothing crashes. The fixture carries texts
    /// with the exporter's own probabilities (accents, digits, casing, a
    /// one-character token, an empty string, a long line); this asserts the
    /// Rust arithmetic agrees.
    #[test]
    fn the_rust_transform_matches_scikit_learns() {
        #[derive(serde::Deserialize)]
        struct Case {
            text: String,
            score: f32,
        }
        #[derive(serde::Deserialize)]
        struct Fixture {
            cases: Vec<Case>,
        }
        let raw = include_str!("../../fixtures/safety-scan/router-parity.json");
        let fixture: Fixture = serde_json::from_str(raw).unwrap();
        assert!(
            fixture.cases.len() >= 10,
            "the fixture must cover the edges"
        );
        let router = shipped();
        let mut worst = 0.0f32;
        for case in &fixture.cases {
            let got = router.score(&case.text);
            worst = worst.max((got - case.score).abs());
            assert!(
                (got - case.score).abs() < 1e-3,
                "score drifted on {:?}: rust {got}, python {}",
                case.text.chars().take(48).collect::<String>(),
                case.score
            );
        }
        assert!(worst < 1e-3, "worst drift {worst}");
    }

    /// The tokenizer's own rules, which the parity fixture can only cover
    /// indirectly: two-character minimum, underscores are word characters,
    /// accents are folded, case is ignored.
    #[test]
    fn tokens_follow_scikit_learns_pattern() {
        assert_eq!(tokenize(&normalise("hi a bc")), vec!["hi", "bc"]);
        assert_eq!(tokenize(&normalise("Café RENOVÉ")), vec!["cafe", "renove"]);
        assert_eq!(tokenize(&normalise("a_b c-d 12")), vec!["a_b", "12"]);
        assert!(tokenize(&normalise("¿ x ?")).is_empty());
    }

    /// A thread with no vocabulary at all must not produce a NaN or a panic —
    /// it scores the intercept, like any document with no known terms.
    #[test]
    fn an_unknown_thread_scores_the_intercept_not_nan() {
        let router = shipped();
        let s = router.score("🙂 🙂 🙂");
        assert!(s.is_finite() && (0.0..=1.0).contains(&s), "got {s}");
    }
}
