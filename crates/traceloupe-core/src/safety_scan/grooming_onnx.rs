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
        // One LINE per message, as in training. Real iMessage bodies contain
        // newlines; unescaped, one message renders as several apparent turns,
        // and a body containing "\nB: ..." would inject a fabricated turn.
        let flat: String = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push_str(&format!("{letter}: {flat}"));
    }
    out
}

impl GroomingClassifier {
    pub fn load(model: &Path, tokenizer: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(model))
            .map_err(|e| format!("onnx session: {e}"))?;
        let mut tokenizer =
            Tokenizer::from_file(tokenizer).map_err(|e| format!("tokenizer: {e}"))?;
        // Truncation must live in the tokenizer, not as a post-hoc slice: HF
        // truncates BEFORE adding [CLS]/[SEP], so trained inputs always end
        // with [SEP]. Slicing afterwards decapitates [SEP] on any window over
        // MAX_TOKENS — a shape the model never saw (review of #522, finding 5).
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                direction: tokenizers::TruncationDirection::Right,
                ..Default::default()
            }))
            .map_err(|e| format!("truncation: {e}"))?;
        Ok(Self { session, tokenizer })
    }

    /// Probability-free by design: the head was trained with a decision
    /// threshold at argmax, and the published comparison used argmax, so a
    /// boolean keeps this honest — no invented confidence scale.
    pub fn window_is_predatory(&mut self, rendered: &str) -> Result<bool, String> {
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
        // A net must never sink the boat: an unexpected output shape is an
        // Err (audited, thread skipped), not a panic that unwinds the scan.
        if logits.ndim() != 2 || logits.shape()[1] < 2 {
            return Err(format!("unexpected logits shape {:?}", logits.shape()));
        }
        let row = logits.index_axis(ndarray::Axis(0), 0);
        Ok(row[1] > row[0])
    }

    /// First window and last window, max — per the measured detection curve.
    /// Returns the index (within `messages`) of the LAST message of the window
    /// that fired: the finding anchors on scored evidence, so a hit in the
    /// tail of a years-long thread must not deep-link to its beginning
    /// (review of #522, finding 2).
    pub fn conversation_predatory_at(
        &mut self,
        messages: &[ChunkItem],
    ) -> Result<Option<usize>, String> {
        if messages.is_empty() {
            return Ok(None);
        }
        let head_end = messages.len().min(WINDOW_MESSAGES);
        if self.window_is_predatory(&render_window(&messages[..head_end]))? {
            return Ok(Some(head_end - 1));
        }
        if messages.len() > WINDOW_MESSAGES
            && self.window_is_predatory(&render_window(
                &messages[messages.len() - WINDOW_MESSAGES..],
            ))?
        {
            return Ok(Some(messages.len() - 1));
        }
        Ok(None)
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

    /// Review of #522, finding 2: a hit in the TAIL window must anchor there.
    /// Uses the real model via the env artefacts plus the out-of-repo smoke
    /// window planted at the END of a long ordinary thread.
    #[test]
    fn a_tail_window_hit_anchors_on_the_tail() {
        let (Ok(model), Ok(tok), Ok(smoke)) = (
            std::env::var("TRACELOUPE_GROOMING_ONNX"),
            std::env::var("TRACELOUPE_GROOMING_TOKENIZER"),
            std::env::var("TRACELOUPE_GROOMING_SMOKE"),
        ) else {
            eprintln!("skipped: needs the artefact env vars");
            return;
        };
        let mut c = GroomingClassifier::load(Path::new(&model), Path::new(&tok)).unwrap();
        let mut msgs: Vec<ChunkItem> = (0..30)
            .map(|i| {
                msg(
                    if i % 2 == 0 { "them" } else { "me" },
                    "see you at the quiz",
                )
            })
            .collect();
        for line in std::fs::read_to_string(&smoke).unwrap().trim().lines() {
            let (who, body) = line.split_once(": ").unwrap_or(("A", line));
            msgs.push(msg(if who == "A" { "them" } else { "me" }, body));
        }
        let hit = c.conversation_predatory_at(&msgs).unwrap();
        assert_eq!(
            hit,
            Some(msgs.len() - 1),
            "the anchor must be in the tail window, not message 9 of the head"
        );
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
