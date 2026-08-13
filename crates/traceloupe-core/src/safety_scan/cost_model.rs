//! What a triage scan costs, and the most each posture is allowed to cost.
//!
//! The whole economic premise of the triage architecture is SELECTIVITY: the
//! census is cheap, so triage wins only if the census hands the expensive
//! focused classifier a small fraction of the phone. When selectivity slips,
//! nothing breaks and no test fails — the scan simply becomes slower than the
//! batch scan it replaced, and the only symptom is a number in a table that
//! someone has to notice.
//!
//! **Nobody noticed, twice.** The Jigsaw-fitted cuts kept 55% of a real device
//! (#486, ~100 h per 100k against the full read's ~11) and were only caught by
//! a hand measurement; re-derived cuts then had to be corrected again once the
//! census scored against nine prototypes instead of one (#489). This module
//! exists so there is a third detector that is not a person reading a table.
//!
//! Every constant here is measured, not assumed, and carries its provenance.

use super::triage::ScanMode;

/// The batch scan's measured cost — the baseline every posture is judged
/// against, because triage exists to replace it. Measured on Jigsaw fixtures
/// (`docs/validation/safety-scan-validation.md`), so comparisons against it are
/// indicative rather than like-for-like.
pub const FULL_READ_HOURS_PER_100K: f64 = 11.0;

/// Seconds per focused classification call: Gemma 4 E4B Q4_K_M on an M3, with
/// prompt-prefix caching on (#409). This dominates triage's cost — the census
/// itself runs at ~64 msg/s and is nearly free by comparison.
pub const FOCUSED_SECONDS_PER_CALL: f64 = 6.5;

/// Hours to scan 100k messages when the census keeps `selectivity_pct` of them.
///
/// Deliberately ignores census time: it is ~26 min per 100k against tens of
/// hours of classification, and leaving it out makes this a LOWER bound. A
/// guard that under-estimates cost fails late, never early.
pub fn hours_per_100k(selectivity_pct: f64) -> f64 {
    let candidates = 100_000.0 * selectivity_pct / 100.0;
    candidates * FOCUSED_SECONDS_PER_CALL / 3600.0
}

impl ScanMode {
    /// The most this posture may cost per 100k messages before it stops being
    /// the thing its name promises.
    ///
    /// These are bounds, not targets — each follows from what the mode claims
    /// to the user, not from what it happens to measure today:
    ///
    /// - **Thorough** openly trades time for recall (the UI says it takes
    ///   longer than a full read), so it may cost more — but a scan that runs
    ///   for days is not a product. Three times the full read.
    /// - **Balanced** is the default, and a default that costs more than the
    ///   scan it replaced is a regression however good its recall. One times.
    /// - **Precise** is chosen for speed. A third.
    pub fn cost_ceiling_hours_per_100k(self) -> f64 {
        FULL_READ_HOURS_PER_100K
            * match self {
                ScanMode::Thorough => 3.0,
                ScanMode::Balanced => 1.0,
                ScanMode::Precise => 1.0 / 3.0,
            }
    }
}

/// One row of the measured record: what the census actually kept at a mode's
/// shipped threshold.
pub struct MeasuredSelectivity {
    pub mode: ScanMode,
    /// The threshold this was measured at. Must equal the mode's shipped
    /// `census_threshold()` — the coupling is asserted below, so moving a cut
    /// without re-measuring fails CI rather than silently invalidating this.
    pub threshold: f32,
    /// Percent of an ordinary phone's messages the census kept.
    pub selectivity_pct: f64,
}

/// The last measured selectivity of each shipped posture.
///
/// Provenance: `census_recall_vs_cost` against the public iOS 17 DFIR research
/// image (576 in-scope messages), EmbeddingGemma-300M Q8_0, the 85-example
/// prototype corpus, 2026-08-13 (#492). Re-run that harness and update these
/// whenever the corpus, the embedder, or a threshold changes.
pub const MEASURED_SELECTIVITY: [MeasuredSelectivity; 3] = [
    MeasuredSelectivity {
        mode: ScanMode::Thorough,
        threshold: 0.64,
        selectivity_pct: 11.5,
    },
    MeasuredSelectivity {
        mode: ScanMode::Balanced,
        threshold: 0.67,
        selectivity_pct: 2.6,
    },
    MeasuredSelectivity {
        mode: ScanMode::Precise,
        threshold: 0.70,
        selectivity_pct: 0.9,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard #486 asked for: no posture may cost more than its name
    /// promises. This is the cheap half — it runs in CI against the recorded
    /// measurement. The other half runs inside `census_recall_vs_cost`, which
    /// asserts the same ceilings against a FRESH measurement.
    #[test]
    fn no_posture_costs_more_than_it_promises() {
        for m in &MEASURED_SELECTIVITY {
            let hours = hours_per_100k(m.selectivity_pct);
            let ceiling = m.mode.cost_ceiling_hours_per_100k();
            assert!(
                hours <= ceiling,
                "{} keeps {:.1}% of an ordinary phone — {:.1} h per 100k against a \
                 ceiling of {:.1} h ({:.1}× the full read's {FULL_READ_HOURS_PER_100K} h). \
                 The census is not selective enough for this posture to be worth \
                 running; fix the census or retire the claim, do not raise the ceiling.",
                m.mode.as_str(),
                m.selectivity_pct,
                hours,
                ceiling,
                ceiling / FULL_READ_HOURS_PER_100K,
            );
        }
    }

    /// The recorded measurement is only meaningful for the cuts it was taken
    /// at. Moving a threshold silently invalidates every number above, so this
    /// fails until someone re-runs the harness.
    #[test]
    fn the_measured_record_matches_the_shipped_thresholds() {
        for m in &MEASURED_SELECTIVITY {
            assert_eq!(
                m.threshold,
                m.mode.census_threshold(),
                "{} ships at {} but its selectivity was measured at {} — re-run \
                 `census_recall_vs_cost` against a public image and update \
                 MEASURED_SELECTIVITY, because the cost ceiling is now being checked \
                 against a number from a different threshold",
                m.mode.as_str(),
                m.mode.census_threshold(),
                m.threshold,
            );
        }
        for mode in [ScanMode::Thorough, ScanMode::Balanced, ScanMode::Precise] {
            assert!(
                MEASURED_SELECTIVITY.iter().any(|m| m.mode == mode),
                "{} has no recorded selectivity, so its cost is unguarded",
                mode.as_str()
            );
        }
    }

    /// A tighter threshold keeps less and therefore costs less. If this ever
    /// inverts, the modes are mislabelled and the postures do not mean what the
    /// UI says they mean.
    #[test]
    fn tighter_postures_cost_less() {
        let cost = |mode: ScanMode| {
            MEASURED_SELECTIVITY
                .iter()
                .find(|m| m.mode == mode)
                .map(|m| hours_per_100k(m.selectivity_pct))
                .unwrap()
        };
        assert!(cost(ScanMode::Thorough) > cost(ScanMode::Balanced));
        assert!(cost(ScanMode::Balanced) > cost(ScanMode::Precise));
    }

    #[test]
    fn the_cost_model_is_the_measured_one() {
        // 100% selectivity is every message through the classifier: 100k calls
        // at 6.5 s. Anchors the formula against an arithmetic fact, so a unit
        // slip (minutes for seconds) cannot pass.
        assert!((hours_per_100k(100.0) - 100_000.0 * 6.5 / 3600.0).abs() < 1e-9);
        assert!((hours_per_100k(0.0)).abs() < 1e-9);
    }
}
