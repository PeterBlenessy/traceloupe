//! The loud-category census boost (#525): five per-message heads trained on
//! Civil Comments, int8. Three of them — threat, identity-attack,
//! sexual-explicit — earn a place as worklist signals: at a 2% false-alarm
//! budget they catch 86% / 76% / 93% of their category on the full held-out
//! test split, in exactly the register (loud, single-message) where the
//! embedding census is weakest. Toxicity and insult overlap the census's
//! strength and are deliberately NOT used until measured to add candidates it
//! misses (docs/research/public-data-audit.md).
//!
//! Thresholds are calibrated on THIS quantised artefact's own validation
//! scores. The rule from the quantisation verdict: ranking survives int8,
//! scale does not — a threshold carried across a quantisation boundary is a
//! silent recall change.

use std::path::Path;

use ndarray::Array2;
use ort::session::Session;
use ort::value::TensorRef;
use tokenizers::Tokenizer;

use super::grooming_onnx::OnnxSpec;
use crate::analysis::Category;

pub const CIVIL_HEADS_MODEL: OnnxSpec = OnnxSpec {
    filename: "civil-heads-modernbert-int8.onnx",
    url: "https://github.com/PeterBlenessy/traceloupe/releases/download/models-v2/civil-heads-modernbert-int8.onnx",
    sha256: "3b8f75515b950ca26e6f08a80fa4e63c90118ab7d7eae001f526acd3f849cf93",
    size_bytes: 150_594_192,
};

/// Output head order — fixed by training, not alphabetical.
const HEADS: usize = 5;

/// The three heads that feed the worklist, with their calibrated thresholds at
/// the 2% false-alarm operating point (civil2_int8_thresholds.json).
const ACTIVE: &[(usize, f32, Category)] = &[
    (1, 0.725, Category::ThreatViolence),
    (3, 0.780, Category::HateIdentity),
    (4, 0.805, Category::SexualContent),
];

const MAX_TOKENS: usize = 192;

pub struct CivilHeads {
    session: Session,
    tokenizer: Tokenizer,
}

/// One message the heads flagged, with the category whose threshold it beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadHit {
    pub category: Category,
    pub score: f32,
}

impl CivilHeads {
    pub fn load(model: &Path, tokenizer: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(model))
            .map_err(|e| format!("onnx session: {e}"))?;
        let mut tokenizer =
            Tokenizer::from_file(tokenizer).map_err(|e| format!("tokenizer: {e}"))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                direction: tokenizers::TruncationDirection::Right,
                ..Default::default()
            }))
            .map_err(|e| format!("truncation: {e}"))?;
        Ok(Self { session, tokenizer })
    }

    /// Score a batch of message texts; `Some(hit)` where any active head beat
    /// its calibrated threshold (highest-margin head wins on a tie).
    pub fn score_batch(&mut self, texts: &[&str]) -> Result<Vec<Option<HeadHit>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encs = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| format!("encode: {e}"))?;
        let width = encs.iter().map(|e| e.get_ids().len()).max().unwrap_or(1);
        let n = encs.len();
        let mut ids = vec![0i64; n * width];
        let mut mask = vec![0i64; n * width];
        for (r, e) in encs.iter().enumerate() {
            for (c, (&id, &m)) in e
                .get_ids()
                .iter()
                .zip(e.get_attention_mask().iter())
                .enumerate()
            {
                ids[r * width + c] = id as i64;
                mask[r * width + c] = m as i64;
            }
        }
        let ids = Array2::from_shape_vec((n, width), ids).map_err(|e| e.to_string())?;
        let mask = Array2::from_shape_vec((n, width), mask).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => TensorRef::from_array_view(&ids).map_err(|e| e.to_string())?,
                "attention_mask" => TensorRef::from_array_view(&mask).map_err(|e| e.to_string())?,
            ])
            .map_err(|e| format!("inference: {e}"))?;
        let logits = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| format!("logits: {e}"))?;
        if logits.ndim() != 2 || logits.shape()[1] < HEADS {
            return Err(format!("unexpected logits shape {:?}", logits.shape()));
        }
        let mut out = Vec::with_capacity(n);
        for r in 0..n {
            let mut best: Option<HeadHit> = None;
            for &(head, th, cat) in ACTIVE {
                let p = 1.0 / (1.0 + (-logits[[r, head]]).exp());
                if p >= th && best.is_none_or(|b| p - th > b.score) {
                    best = Some(HeadHit {
                        category: cat,
                        score: p,
                    });
                }
            }
            out.push(best);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity with the calibrated Python reference: loud samples of each
    /// active category flag with the right category; ordinary text does not.
    /// Ignored without the artefacts.
    #[test]
    fn active_heads_flag_their_categories() {
        let (Ok(model), Ok(tok)) = (
            std::env::var("TRACELOUPE_CIVIL_ONNX"),
            std::env::var("TRACELOUPE_GROOMING_TOKENIZER"),
        ) else {
            eprintln!("skipped: set TRACELOUPE_CIVIL_ONNX / TRACELOUPE_GROOMING_TOKENIZER");
            return;
        };
        let mut c = CivilHeads::load(Path::new(&model), Path::new(&tok)).unwrap();
        let texts = [
            "i will come to your house and break both your legs",
            // Real CC0 test-split comment (id. attack 0.8+): invented text
            // reliably scores off-distribution — the project's oldest lesson.
            "Muslim have been killing infidels since 624AD and they will not stop",
            "send me naked pictures right now or else",
            "can you grab milk on the way home",
            "the meeting moved to 3pm, see you there",
        ];
        let hits = c
            .score_batch(&texts.iter().copied().collect::<Vec<_>>())
            .unwrap();
        assert_eq!(hits[0].map(|h| h.category), Some(Category::ThreatViolence));
        assert_eq!(hits[1].map(|h| h.category), Some(Category::HateIdentity));
        assert_eq!(hits[2].map(|h| h.category), Some(Category::SexualContent));
        assert_eq!(hits[3], None, "milk run must not flag");
        assert_eq!(hits[4], None, "meeting must not flag");
    }
}
