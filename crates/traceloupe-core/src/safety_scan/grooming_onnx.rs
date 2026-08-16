//! The full-pass grooming signal: the PAN12-trained int8 ONNX classifier.
//!
//! Why this exists as its own stage: at ~14 ms per conversation on CPU it can
//! read EVERY conversation in a backup — census-grade cost with
//! published-benchmark accuracy (F0.5 0.958 on PAN12's official test, against
//! 0.9348 published; docs/research/public-data-audit.md). The generative
//! classifier stays for the categories this model does not cover.
//!
//! Windowing follows the measured curve, not intuition: scoring only the first
//! 10 messages of real predatory conversations catches 89%, the first 20
//! catches 96%. We score the first and last window of each conversation and
//! take the max — early grooming and recent escalation are both visible, and
//! cost stays two inferences per conversation.

use std::path::{Path, PathBuf};

use ndarray::Array2;
use ort::session::Session;
use ort::value::TensorRef;
use tokenizers::Tokenizer;

use super::chunker::ChunkItem;

/// Messages per scored window. 10 is the knee of the measured curve.
pub const WINDOW_MESSAGES: usize = 10;
/// Tokenizer truncation, matching training (256 covers a 10-message window).
const MAX_TOKENS: usize = 256;

/// Where the artefacts come from and how they are verified. Mirrors
/// `ModelSpec` deliberately, but ONNX artefacts are release assets of this
/// repo rather than third-party HuggingFace files, and they load in-process
/// via `ort` rather than through the llama-server sidecar.
pub struct OnnxSpec {
    pub filename: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
}

pub const GROOMING_MODEL: OnnxSpec = OnnxSpec {
    filename: "grooming-pan12-modernbert-int8.onnx",
    url: "https://github.com/PeterBlenessy/traceloupe/releases/download/models-v1/grooming-pan12-modernbert-int8.onnx",
    sha256: "5d21078ee94afb5e9ef7fa685d8653a6856618cbc2885f83f78fe498a0a8be23",
    size_bytes: 150_591_876,
};

pub const GROOMING_TOKENIZER: OnnxSpec = OnnxSpec {
    filename: "grooming-tokenizer.json",
    url: "https://github.com/PeterBlenessy/traceloupe/releases/download/models-v1/grooming-tokenizer.json",
    sha256: "9fd55248d51d33976b324fc11592e28071da7d41e0e9401dfb7082e30574b7b1",
    size_bytes: 2_132_967,
};

impl OnnxSpec {
    /// Installed path under `models_dir`, if present with the right size.
    /// Same trade-off as `ModelSpec::installed_at`: integrity is verified at
    /// download time, size-only on the hot path.
    pub fn installed_at(&self, models_dir: &Path) -> Option<PathBuf> {
        let path = models_dir.join(self.filename);
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() == self.size_bytes => Some(path),
            _ => None,
        }
    }
}

pub struct GroomingClassifier {
    session: Session,
    tokenizer: Tokenizer,
}

/// A conversation is rendered exactly as at training time: one line per
/// message, speakers anonymised to `A:`/`B:`/… in order of first appearance.
/// The classifier learned roles-by-position, not identities; leaking real
/// names would move it off its training distribution.
pub fn render_window(messages: &[ChunkItem]) -> String {
    let mut speakers: Vec<&str> = Vec::new();
    let mut out = String::new();
    for m in messages {
        let who = match speakers.iter().position(|s| *s == m.sender.as_str()) {
            Some(i) => i,
            None => {
                speakers.push(m.sender.as_str());
                speakers.len() - 1
            }
        };
        let letter = (b'A' + (who % 26) as u8) as char;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{letter}: {}", m.text));
    }
    out
}

impl GroomingClassifier {
    pub fn load(model: &Path, tokenizer: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(model))
            .map_err(|e| format!("onnx session: {e}"))?;
        let tokenizer = Tokenizer::from_file(tokenizer).map_err(|e| format!("tokenizer: {e}"))?;
        Ok(Self { session, tokenizer })
    }

    /// Probability-free by design: the head was trained with a decision
    /// threshold at argmax, and the published comparison used argmax, so a
    /// boolean keeps this honest — no invented confidence scale.
    pub fn window_is_predatory(&mut self, rendered: &str) -> Result<bool, String> {
        let mut enc = self
            .tokenizer
            .encode(rendered, true)
            .map_err(|e| format!("encode: {e}"))?;
        enc.truncate(MAX_TOKENS, 0, tokenizers::TruncationDirection::Right);
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
        let row = logits.index_axis(ndarray::Axis(0), 0);
        Ok(row[1] > row[0])
    }

    /// First window and last window, max — per the measured detection curve.
    pub fn conversation_is_predatory(&mut self, messages: &[ChunkItem]) -> Result<bool, String> {
        if messages.is_empty() {
            return Ok(false);
        }
        let head = &messages[..messages.len().min(WINDOW_MESSAGES)];
        if self.window_is_predatory(&render_window(head))? {
            return Ok(true);
        }
        if messages.len() > WINDOW_MESSAGES {
            let tail = &messages[messages.len() - WINDOW_MESSAGES..];
            return self.window_is_predatory(&render_window(tail));
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(sender: &str, text: &str) -> ChunkItem {
        ChunkItem {
            source_id: 0,
            sender: sender.to_string(),
            occurred_at: None,
            text: text.to_string(),
            fingerprint: String::new(),
        }
    }

    #[test]
    fn rendering_anonymises_speakers_in_order_of_appearance() {
        let ms = vec![
            msg("them", "hey"),
            msg("me", "hi"),
            msg("them", "you around?"),
        ];
        assert_eq!(render_window(&ms), "A: hey\nB: hi\nA: you around?");
    }

    #[test]
    fn rendering_is_identity_blind() {
        // Same conversation, different sender labels -> identical rendering.
        let a = vec![msg("them", "hey"), msg("me", "hi")];
        let b = vec![msg("+44 7911 123456", "hey"), msg("owner", "hi")];
        assert_eq!(render_window(&a), render_window(&b));
    }

    /// Parity with the Python reference. Ignored without the artefacts; run
    /// with TRACELOUPE_GROOMING_ONNX and TRACELOUPE_GROOMING_TOKENIZER set.
    #[test]
    fn labels_match_the_python_reference() {
        let (Ok(model), Ok(tok)) = (
            std::env::var("TRACELOUPE_GROOMING_ONNX"),
            std::env::var("TRACELOUPE_GROOMING_TOKENIZER"),
        ) else {
            eprintln!("skipped: set TRACELOUPE_GROOMING_ONNX / TRACELOUPE_GROOMING_TOKENIZER");
            return;
        };
        let mut c = GroomingClassifier::load(Path::new(&model), Path::new(&tok)).unwrap();
        // The same three samples the Python parity check used, same labels.
        let predatory = "A: hey whats up\nB: nothing much, bored\nA: how old are you again\nB: 13 why\nA: youre mature for 13. got any pics? our secret";
        let milk = "A: can you grab milk on the way\nB: sure, 2 pints?\nA: perfect thanks x";
        let meeting = "A: the meeting moved to 3\nB: ok ill update the invite";
        // NOTE: the Python check showed the invented predatory sample is NOT
        // flagged (invented text is off-distribution — the whole lesson of
        // this project). Parity means agreeing with the reference, including
        // on that: false, false, false.
        assert!(!c.window_is_predatory(predatory).unwrap());
        assert!(!c.window_is_predatory(milk).unwrap());
        assert!(!c.window_is_predatory(meeting).unwrap());
        // The positive path needs REAL predatory text, which must never enter
        // the repo (PAN12 access terms). Point TRACELOUPE_GROOMING_SMOKE at a
        // local file holding one rendered real window; the test then asserts
        // the Rust stack flags it, proving the positive path end to end.
        if let Ok(smoke) = std::env::var("TRACELOUPE_GROOMING_SMOKE") {
            let text = std::fs::read_to_string(&smoke).unwrap();
            assert!(
                c.window_is_predatory(text.trim()).unwrap(),
                "the known-predatory smoke window was not flagged"
            );
        }
    }
}
