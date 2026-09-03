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
//! Measured through this code path on the real haystack
//! (`router_recall_vs_cost` in eval.rs, ignored without the data): **6,047
//! threads ranked in 0.14 s — 23 µs per thread**, holding 27/27 grooming and
//! 20/20 self-harm in the top 5%. The network it replaced needed ~14 ms per
//! conversation, roughly 600x more, for the same ranking.
//!
//! Two properties this tier deliberately does NOT have:
//!
//! * **No findings.** It decides reading order. Every verdict still comes from
//!   the model that can justify one.
//! * **No threshold.** Everything is expressed as a rank, so the tier cannot
//!   acquire a hidden decision boundary that nobody re-measures.

use std::sync::OnceLock;

use super::chunker::ChunkItem;
use super::grooming_onnx::{render_window, WINDOW_MESSAGES};
use super::lexical::LexicalModel;

/// Share of a phone's threads the router may PROMOTE — threads the census kept
/// nothing from, which would otherwise never be deep-scanned however they rank.
/// 5% is the operating point the experiment measured (27/27 grooming, 20/20
/// self-harm inside it), expressed as a rank so no score can move it.
pub const PROMOTE_TOP_SHARE: f64 = 0.05;

/// The trained weights: `crates/traceloupe-core/fixtures/safety-scan/router-lexical.json`,
/// produced by `harness/export_lexical_router.py` in the training repo. Term
/// weights only — no corpus text, which PAN12's terms forbid redistributing.
const MODEL_JSON: &str = include_str!("../../fixtures/safety-scan/router-lexical.json");

pub struct LexicalRouter {
    model: LexicalModel,
}

/// The shipped model, parsed once. ~43k terms; parsing is milliseconds and
/// happens on the first scan, not at startup.
pub fn shipped() -> &'static LexicalRouter {
    static MODEL: OnceLock<LexicalRouter> = OnceLock::new();
    MODEL.get_or_init(|| LexicalRouter {
        model: LexicalModel::from_json(MODEL_JSON),
    })
}

impl LexicalRouter {
    /// One text's score in [0, 1]. Used only to ORDER, never compared to a
    /// constant — see `lexical::LexicalModel` for the transform itself.
    pub fn score(&self, text: &str) -> f32 {
        self.model.score(text)
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

    /// A thread with no vocabulary at all must not produce a NaN or a panic —
    /// it scores the intercept, like any document with no known terms.
    #[test]
    fn an_unknown_thread_scores_the_intercept_not_nan() {
        let router = shipped();
        let s = router.score("🙂 🙂 🙂");
        assert!(s.is_finite() && (0.0..=1.0).contains(&s), "got {s}");
    }
}
