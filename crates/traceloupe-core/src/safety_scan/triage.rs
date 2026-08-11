//! Triage scoring and scan modes (#459).
//!
//! The measured pipeline is: cheap embedding census over every message → focused
//! classification of the messages the census ranks highest → optional
//! confirmation. This module holds the parts of that with no I/O: the vector
//! math the census scores with, and the named modes that trade recall against
//! precision and time.
//!
//! End-to-end on realistic chunks with held-out prototypes
//! (docs/validation/safety-scan-validation.md), the pipeline reaches recall 0.94
//! at precision 0.95, against the shipped batch scan's 0.30 / 0.89. The census
//! threshold is a monotonic dial on that; the modes below are the points on it
//! we expose.

/// How aggressively a scan trades recall for precision and time. The UI shows
/// the NAME and a plain description, never the underlying numbers — accuracy
/// figures are a claim to be defended, a posture is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanMode {
    /// Catch as much as possible; accept that the reviewer dismisses more false
    /// positives. Lowest census threshold, no confirmation stage.
    Thorough,
    /// The default. A strong recall/precision balance with confirmation on.
    #[default]
    Balanced,
    /// Fewest false positives; accepts that a little real harm is trimmed by the
    /// confirmation stage. Highest usable threshold, confirmation on.
    Precise,
}

impl ScanMode {
    /// The census keep threshold: a message scoring below this is not deep
    /// scanned. Lower keeps more, raising the recall ceiling at the cost of more
    /// focused-classification work. Values are the measured sweep points
    /// (#459): 0.52 → ceiling 0.96, 0.58 → 0.91, 0.64 → 0.88.
    pub fn census_threshold(self) -> f32 {
        match self {
            ScanMode::Thorough => 0.52,
            ScanMode::Balanced => 0.55,
            ScanMode::Precise => 0.58,
        }
    }

    /// Whether the confirmation stage runs. It lifts precision by a few points
    /// but trims real recall, so Thorough leaves it off — a reviewer chasing
    /// completeness would rather dismiss a false positive than miss a real one.
    pub fn confirm(self) -> bool {
        !matches!(self, ScanMode::Thorough)
    }

    /// Stable id for storage on the scan row and for the settings value.
    pub fn as_str(self) -> &'static str {
        match self {
            ScanMode::Thorough => "thorough",
            ScanMode::Balanced => "balanced",
            ScanMode::Precise => "precise",
        }
    }

    pub fn parse(s: &str) -> Option<ScanMode> {
        match s {
            "thorough" => Some(ScanMode::Thorough),
            "balanced" => Some(ScanMode::Balanced),
            "precise" => Some(ScanMode::Precise),
            _ => None,
        }
    }
}

/// Cosine similarity of two equal-length vectors. Returns 0 for a zero vector
/// rather than NaN, so a degenerate embedding can never rank first.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// The mean of several vectors — a category's prototype, built from the embedded
/// labelled positives. Returns None for an empty set or ragged lengths, which a
/// caller must treat as "no prototype for this category" rather than scoring
/// against garbage.
pub fn centroid(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = vectors.first()?;
    let dim = first.len();
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return None;
    }
    let mut sum = vec![0.0f32; dim];
    for v in vectors {
        for (s, x) in sum.iter_mut().zip(v) {
            *s += x;
        }
    }
    let n = vectors.len() as f32;
    for s in &mut sum {
        *s /= n;
    }
    Some(sum)
}

/// A message's census score: the highest cosine similarity to any prototype.
/// Max, not mean — a message near ONE kind of harm is suspicious even if it
/// looks nothing like the others.
pub fn census_score(message: &[f32], prototypes: &[Vec<f32>]) -> f32 {
    prototypes
        .iter()
        .map(|p| cosine(message, p))
        .fold(0.0f32, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_and_order_by_threshold() {
        for m in [ScanMode::Thorough, ScanMode::Balanced, ScanMode::Precise] {
            assert_eq!(ScanMode::parse(m.as_str()), Some(m));
        }
        assert!(ScanMode::Thorough.census_threshold() < ScanMode::Balanced.census_threshold());
        assert!(ScanMode::Balanced.census_threshold() < ScanMode::Precise.census_threshold());
        // Thorough favours recall: no confirmation trimming.
        assert!(!ScanMode::Thorough.confirm());
        assert!(ScanMode::Precise.confirm());
        assert_eq!(ScanMode::default(), ScanMode::Balanced);
        assert_eq!(ScanMode::parse("nonsense"), None);
    }

    #[test]
    fn cosine_is_one_for_parallel_and_zero_for_orthogonal() {
        assert!((cosine(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // A zero vector is 0, never NaN.
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn centroid_averages_and_rejects_ragged() {
        let c = centroid(&[vec![0.0, 2.0], vec![2.0, 0.0]]).unwrap();
        assert_eq!(c, vec![1.0, 1.0]);
        assert_eq!(centroid(&[]), None);
        assert_eq!(
            centroid(&[vec![1.0], vec![1.0, 2.0]]),
            None,
            "ragged rejected"
        );
    }

    #[test]
    fn census_score_takes_the_nearest_prototype() {
        let threat = vec![1.0, 0.0, 0.0];
        let scam = vec![0.0, 1.0, 0.0];
        // A message identical to the scam prototype scores 1 via scam, even
        // though it is orthogonal to threat.
        let msg = vec![0.0, 1.0, 0.0];
        assert!((census_score(&msg, &[threat, scam]) - 1.0).abs() < 1e-6);
        // No prototypes → 0, never a spurious hit.
        assert_eq!(census_score(&msg, &[]), 0.0);
    }
}
