//! The deep-scan router (#544): which conversations the expensive model reads
//! FIRST, and which ones it reads at all when a budget bites.
//!
//! Until now that order came from the census — one embedding per message,
//! scored by cosine against hand-written centroids. Measured against real harm
//! inside real traffic (6,000 ordinary chat conversations, harm planted at a
//! 1.1% base rate, plants held out from every model involved), that ordering is
//! barely better than shuffling: it ranks a predator conversation above an
//! ordinary one 62% of the time, and for self-harm it ranks them BELOW ordinary
//! chatter. Two structural reasons, both inherent to the method: a long message
//! mean-pools into a diffuse vector far from centroids built from chat lines,
//! and a conversation scores as the MAX over its messages, so a ten-message
//! thread gets ten tickets in the lottery and a one-message thread gets one.
//! Rebuilding those centroids from real held-out positives made it worse, which
//! rules out better examples as the fix.
//!
//! This model ranks the same haystack at 0.991 (grooming) and 0.792
//! (self-harm, register-matched). At the top 5% of the phone it holds 27/27
//! planted grooming conversations and 19/20 self-harm disclosures, where the
//! census holds 4/27 and 0/20.
//!
//! Three properties this tier deliberately does NOT have:
//!
//! * **No findings.** It decides reading order. Every verdict still comes from
//!   the model that can justify one.
//! * **No threshold.** int8 quantisation preserves the ORDER (top-decile set
//!   overlap 57/64) while individual scores drift by up to 0.95, so any cut
//!   expressed as a score would mean something different after quantisation.
//!   Everything here is expressed as a rank.
//! * **No scam arm.** Trained with one, 12 of 75 legitimate messages — bank
//!   alerts, parcel notifications — landed in the top 1% of the phone, because
//!   the only public "not smishing" examples are casual personal texts and
//!   legitimate business SMS is therefore off-distribution. Without it: 0 of 75,
//!   and the rules tier already catches 92% of real smishing.

use std::path::Path;

use ndarray::Array2;
use ort::session::Session;
use ort::value::TensorRef;
use tokenizers::Tokenizer;

use super::chunker::ChunkItem;
use super::grooming_onnx::{render_window, OnnxSpec, WINDOW_MESSAGES};

/// Truncation, matching training.
const MAX_TOKENS: usize = 256;

/// Share of a phone's threads the router may PROMOTE — threads the census kept
/// nothing from, which would otherwise never be deep-scanned however they rank.
/// 5% is the operating point the experiment measured (27/27 grooming, 19/20
/// self-harm inside it), expressed as a rank so quantisation cannot move it.
pub const PROMOTE_TOP_SHARE: f64 = 0.05;

pub const ROUTER_MODEL: OnnxSpec = OnnxSpec {
    filename: "router-grooming-selfharm-modernbert-int8.onnx",
    // Filled in when the release asset is published; until then the tier is
    // absent and the scan is byte-identical to today.
    url: "https://github.com/PeterBlenessy/traceloupe/releases/download/models-v2/router-grooming-selfharm-modernbert-int8.onnx",
    sha256: "",
    size_bytes: 0,
};

pub struct DeepScanRouter {
    session: Session,
    tokenizer: Tokenizer,
}

impl DeepScanRouter {
    pub fn load(model: &Path, tokenizer: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(model))
            .map_err(|e| format!("onnx session: {e}"))?;
        let mut tokenizer =
            Tokenizer::from_file(tokenizer).map_err(|e| format!("tokenizer: {e}"))?;
        // Truncate in the tokenizer, not afterwards: HF truncates before adding
        // [CLS]/[SEP], so a post-hoc slice decapitates [SEP] and hands the model
        // a shape it never saw in training (the same trap as #522 finding 5).
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                direction: tokenizers::TruncationDirection::Right,
                ..Default::default()
            }))
            .map_err(|e| format!("truncation: {e}"))?;
        Ok(Self { session, tokenizer })
    }

    /// One window's score in [0, 1]. Used only to ORDER, never compared to a
    /// constant.
    fn window_score(&mut self, rendered: &str) -> Result<f32, String> {
        let enc = self
            .tokenizer
            .encode(rendered, true)
            .map_err(|e| format!("encode: {e}"))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&i| i as i64).collect();
        let n = ids.len();
        let ids = Array2::from_shape_vec((1, n), ids).map_err(|e| e.to_string())?;
        let mask = Array2::from_shape_vec((1, n), mask).map_err(|e| e.to_string())?;
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
        // An unexpected shape is an Err the caller audits and skips, never a
        // panic: a ranking aid must not be able to fail a scan.
        if logits.ndim() != 2 || logits.shape()[1] < 2 {
            return Err(format!("unexpected logits shape {:?}", logits.shape()));
        }
        let row = logits.index_axis(ndarray::Axis(0), 0);
        let (a, b) = (row[0], row[1]);
        let m = a.max(b);
        let (ea, eb) = ((a - m).exp(), (b - m).exp());
        Ok(eb / (ea + eb))
    }

    /// A thread's score: the higher of its first and last window, matching the
    /// grooming detector's measured windowing (early approach and recent
    /// escalation are both visible for two inferences).
    ///
    /// Returns the score AND the index of the last message of the window that
    /// produced it, so a promoted thread deep-links to scored evidence rather
    /// than to the top of a years-long conversation.
    pub fn thread_score(&mut self, messages: &[ChunkItem]) -> Result<(f32, usize), String> {
        if messages.is_empty() {
            return Ok((0.0, 0));
        }
        let head_end = messages.len().min(WINDOW_MESSAGES);
        let head = self.window_score(&render_window(&messages[..head_end]))?;
        let mut best = (head, head_end - 1);
        if messages.len() > WINDOW_MESSAGES {
            let tail = self.window_score(&render_window(
                &messages[messages.len() - WINDOW_MESSAGES..],
            ))?;
            if tail > best.0 {
                best = (tail, messages.len() - 1);
            }
        }
        Ok(best)
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
}
