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

    /// Context radius for focused classification: messages either side of the
    /// judged one. The end-to-end validation used a 5-message window (#458), so
    /// radius 2 reproduces it. Not per-mode — context need is a property of the
    /// classifier, not the recall/precision posture.
    pub fn default_radius() -> usize {
        2
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

/// EmbeddingGemma's task prefix. Prepended to every text before embedding —
/// measured to matter (recall at a fixed drop rate moved 0.67 → 0.80 with it),
/// because the model is trained with these prefixes and raw text is
/// off-distribution.
pub const EMBED_PREFIX: &str = "task: classification | query: ";

/// A message to score in the census.
#[derive(Debug, Clone)]
pub struct CensusInput {
    pub source_id: i64,
    pub thread_identifier: String,
    pub sender: String,
    pub occurred_at: Option<i64>,
    pub text: String,
    /// The message's durable identity (`chunker::message_fingerprint`).
    /// Findings are keyed on this; cache row ids change on re-import, so a
    /// finding keyed on `source_id` would lose its dismissal at the next
    /// import.
    pub fingerprint: String,
    /// The thread's service (iMessage/SMS/TikTok…), carried onto findings so a
    /// service-scoped scan can count and list its own.
    pub service: Option<String>,
}

/// A scored message, ready to persist.
#[derive(Debug, Clone)]
pub struct ScoredMessage {
    pub source_id: i64,
    pub thread_identifier: String,
    pub sender: String,
    pub occurred_at: Option<i64>,
    pub score: f32,
}

/// Build one prototype vector per category from labelled example texts.
///
/// `examples` is (category, text) pairs — in production, the fixture positives
/// filtered to the SELECTED categories, so a scam-only view ranks by scam
/// prototypes alone. Each text is embedded (through `embed`, prefixed here so
/// callers cannot forget it) and the per-category mean is the prototype.
///
/// A category with no usable examples is simply absent from the result rather
/// than contributing a zero vector that would score everything as mildly
/// suspicious.
pub fn build_prototypes<E>(
    examples: &[(String, String)],
    mut embed: E,
) -> crate::Result<Vec<Vec<f32>>>
where
    E: FnMut(&str) -> crate::Result<Vec<f32>>,
{
    use std::collections::BTreeMap;
    let mut by_cat: BTreeMap<&str, Vec<Vec<f32>>> = BTreeMap::new();
    for (cat, text) in examples {
        let v = embed(&format!("{EMBED_PREFIX}{text}"))?;
        by_cat.entry(cat.as_str()).or_default().push(v);
    }
    Ok(by_cat.values().filter_map(|vs| centroid(vs)).collect())
}

/// Score every message against the prototypes: the census, phase one.
///
/// Returns each message's max similarity to any prototype. `embed` is called
/// once per message (the prefix is applied here). With no prototypes every
/// score is 0 — the caller must treat that as "cannot triage" and fall back to
/// scanning everything, never as "nothing is suspicious".
pub fn census_messages<E>(
    messages: &[CensusInput],
    prototypes: &[Vec<f32>],
    mut embed: E,
) -> crate::Result<Vec<ScoredMessage>>
where
    E: FnMut(&str) -> crate::Result<Vec<f32>>,
{
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        let v = embed(&format!("{EMBED_PREFIX}{}", m.text))?;
        out.push(ScoredMessage {
            source_id: m.source_id,
            thread_identifier: m.thread_identifier.clone(),
            sender: m.sender.clone(),
            occurred_at: m.occurred_at,
            score: census_score(&v, prototypes),
        });
    }
    Ok(out)
}

/// A message plus its neighbours, the unit focused classification judges: the
/// window is context, one item is under review.
#[derive(Debug, Clone)]
pub struct FocusWindow {
    /// The window's messages in order, oldest first.
    pub items: Vec<CensusInput>,
    /// Index within `items` of the message actually being judged.
    pub focus: usize,
}

/// Build the context window for the message at `focus_idx` in a thread's
/// ordered `messages`.
///
/// Focused classification needs the harmful message SEEN IN CONTEXT — a threat
/// reads differently after "stop messaging me" than after a joke. The window is
/// `radius` messages either side, clamped at the thread's ends, so a message
/// near the start or end simply has a shorter, off-centre window rather than
/// being padded with unrelated content.
///
/// `focus` in the returned window is the judged message's new index, which
/// shifts when the start is clamped — the caller judges THAT index, so it must
/// be right or the verdict clamp (engine) rejects a correct finding.
pub fn context_window(messages: &[CensusInput], focus_idx: usize, radius: usize) -> FocusWindow {
    let start = focus_idx.saturating_sub(radius);
    let end = (focus_idx + radius + 1).min(messages.len());
    FocusWindow {
        items: messages[start..end].to_vec(),
        focus: focus_idx - start,
    }
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

    // A deterministic fake embedder: maps a keyword to a one-hot axis, so tests
    // can reason about similarity without a model. "threat"->x, "scam"->y,
    // anything else->z (orthogonal to both).
    fn fake_embed(text: &str) -> crate::Result<Vec<f32>> {
        let t = text.to_lowercase();
        Ok(if t.contains("threat") || t.contains("kill") {
            vec![1.0, 0.0, 0.0]
        } else if t.contains("scam") || t.contains("gift card") {
            vec![0.0, 1.0, 0.0]
        } else {
            vec![0.0, 0.0, 1.0]
        })
    }

    #[test]
    fn prototypes_are_per_category_and_selection_scopes_them() {
        // Two threat examples, two scam — the SELECTED categories.
        let ex = vec![
            ("threat-violence".to_string(), "i will kill you".to_string()),
            ("threat-violence".to_string(), "a threat here".to_string()),
            (
                "scam-fraud".to_string(),
                "send a gift card scam".to_string(),
            ),
        ];
        let protos = build_prototypes(&ex, fake_embed).unwrap();
        assert_eq!(protos.len(), 2, "one prototype per category");

        // A scam message scores high against this set…
        let scam = census_score(&fake_embed("gift card please").unwrap(), &protos);
        assert!(scam > 0.9);

        // …but if the reviewer selected ONLY scam, the threat prototype is gone,
        // and a threat message no longer ranks — the scan does not read for a
        // category nobody asked about (#460).
        let scam_only: Vec<_> = ex
            .iter()
            .filter(|(c, _)| c == "scam-fraud")
            .cloned()
            .collect();
        let scam_protos = build_prototypes(&scam_only, fake_embed).unwrap();
        assert_eq!(scam_protos.len(), 1);
        let threat = census_score(&fake_embed("kill you").unwrap(), &scam_protos);
        assert!(
            threat.abs() < 1e-6,
            "a threat is invisible to a scam-only census"
        );
    }

    #[test]
    fn census_scores_a_backup_and_ranks_harm_above_chatter() {
        let protos = build_prototypes(
            &[("threat-violence".to_string(), "i will kill you".to_string())],
            fake_embed,
        )
        .unwrap();
        let msgs = vec![
            CensusInput {
                source_id: 1,
                thread_identifier: "t".into(),
                sender: "a".into(),
                occurred_at: Some(1),
                text: "i will kill you".into(),
                fingerprint: "fp".into(),
                service: None,
            },
            CensusInput {
                source_id: 2,
                thread_identifier: "t".into(),
                sender: "a".into(),
                occurred_at: Some(2),
                text: "grab milk please".into(),
                fingerprint: "fp".into(),
                service: None,
            },
        ];
        let scored = census_messages(&msgs, &protos, fake_embed).unwrap();
        assert!(scored[0].score > scored[1].score);
        assert!(scored[0].score > 0.9 && scored[1].score < 0.1);
        // Metadata is carried through untouched — the census row needs it.
        assert_eq!(scored[0].source_id, 1);
        assert_eq!(scored[0].sender, "a");
    }

    /// No prototypes must score 0, never a spurious hit — the caller treats
    /// this as "cannot triage, scan everything", not "all clean".
    fn msgs(n: usize) -> Vec<CensusInput> {
        (0..n)
            .map(|i| CensusInput {
                source_id: i as i64,
                thread_identifier: "t".into(),
                sender: "s".into(),
                occurred_at: Some(1000 + i as i64),
                text: format!("m{i}"),
                fingerprint: "fp".into(),
                service: None,
            })
            .collect()
    }

    #[test]
    fn a_centred_window_puts_the_focus_in_the_middle() {
        let w = context_window(&msgs(10), 5, 2);
        assert_eq!(w.items.len(), 5, "radius 2 each side");
        assert_eq!(
            w.items[w.focus].source_id, 5,
            "focus points at the judged msg"
        );
        assert_eq!(w.focus, 2, "centred");
    }

    #[test]
    fn a_window_at_the_start_is_shorter_and_off_centre_not_padded() {
        let w = context_window(&msgs(10), 0, 2);
        assert_eq!(w.items.len(), 3, "no messages before index 0");
        assert_eq!(w.focus, 0);
        assert_eq!(w.items[w.focus].source_id, 0);
    }

    #[test]
    fn a_window_at_the_end_clamps() {
        let m = msgs(10);
        let w = context_window(&m, 9, 3);
        assert_eq!(w.items.last().unwrap().source_id, 9);
        assert_eq!(w.items[w.focus].source_id, 9, "focus still the judged msg");
        // Never runs past the end.
        assert!(w.items.len() <= 4);
    }

    #[test]
    fn the_focus_index_survives_a_clamped_start() {
        // radius 4 but focus at 1: start clamps to 0, so focus shifts to 1.
        let w = context_window(&msgs(10), 1, 4);
        assert_eq!(
            w.items[w.focus].source_id, 1,
            "the clamp must not misplace focus"
        );
    }

    #[test]
    fn an_empty_prototype_set_scores_zero() {
        let msgs = vec![CensusInput {
            source_id: 1,
            thread_identifier: "t".into(),
            sender: "a".into(),
            occurred_at: None,
            text: "i will kill you".into(),
            fingerprint: "fp".into(),
            service: None,
        }];
        let scored = census_messages(&msgs, &[], fake_embed).unwrap();
        assert_eq!(scored[0].score, 0.0);
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
