//! Minimal OpenAI-compatible chat client for the local llama-server sidecar
//! (plan T5). Non-streaming — the engine wants whole verdict objects, and
//! chunk latency is dominated by generation either way.
//!
//! Privacy invariants (ADR 0002): requests go only to the configured loopback
//! base URL, and NOTHING from a request or response is ever logged here —
//! errors carry status codes and parse messages, never prompt or completion
//! text.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{Error, Result};

pub struct LlmClient {
    agent: ureq::Agent,
    base_url: String,
    model: String,
    /// Sent as `Authorization: Bearer …` when the server requires `--api-key`.
    api_key: Option<String>,
}

impl LlmClient {
    /// `base_url` like `http://127.0.0.1:8080` (no trailing slash needed).
    pub fn new(base_url: &str, model: &str, timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_read(timeout)
                .timeout_write(timeout)
                .build(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key: None,
        }
    }

    /// Attach the server's per-run bearer token (see `server::generate_api_key`).
    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One classification call: system + user message, constrained by a raw
    /// GBNF `grammar` (temperature 0). Returns the completion content parsed as
    /// JSON. We pass `grammar` rather than a `response_format` JSON schema
    /// because the server's schema→grammar path does NOT enforce `maxItems` and
    /// over-constrains whitespace in a way that suppresses detection — see
    /// `prompt::verdicts_grammar`.
    pub fn chat_json(
        &self,
        system: &str,
        user: &str,
        grammar: &str,
        max_tokens: u32,
    ) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": max_tokens,
            "grammar": grammar,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        let content = self.post_chat(&body)?;
        serde_json::from_str(&content)
            .map_err(|_| Error::Inference("completion content is not valid JSON".into()))
    }

    /// One free-text call (the T6 summary passes) — same privacy rules, no
    /// grammar constraint.
    pub fn chat_text(&self, system: &str, user: &str, max_tokens: u32) -> Result<String> {
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": max_tokens,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        self.post_chat(&body)
    }

    /// Embed one text via `/embedding`, returning its vector. Same privacy
    /// rules as chat: loopback only, nothing logged. Used by the triage census
    /// (#459) to score every message cheaply before any deep classification.
    ///
    /// EmbeddingGemma is trained with task prefixes; the caller passes the
    /// already-prefixed text, because the prefix that matters
    /// ("task: classification | query: ") is a property of what the census is
    /// doing, not of the transport.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let text = self.post_json("/embedding", &json!({ "content": text }))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| Error::Inference("embedding response is not JSON".into()))?;
        // llama-server returns either `{embedding: [...]}` or, with some builds,
        // `[{embedding: [[...]]}]`. Accept both rather than pin one server.
        let arr = v
            .get("embedding")
            .or_else(|| v.get(0).and_then(|x| x.get("embedding")))
            .ok_or_else(|| Error::Inference("no embedding in response".into()))?;
        // The nested form wraps the vector one level deeper.
        let flat = if arr.get(0).map(|x| x.is_array()).unwrap_or(false) {
            arr.get(0).unwrap()
        } else {
            arr
        };
        let out: Vec<f32> = flat
            .as_array()
            .ok_or_else(|| Error::Inference("embedding is not an array".into()))?
            .iter()
            .filter_map(|x| x.as_f64().map(|f| f as f32))
            .collect();
        if out.is_empty() {
            return Err(Error::Inference("empty embedding".into()));
        }
        Ok(out)
    }

    /// One raw `/completion` call — for models whose GGUF ships no chat
    /// template (Llama Guard: the generic template made it CONTINUE the
    /// conversation instead of judging it, journey §10.6 incident 1). The
    /// caller supplies the fully-rendered prompt; same privacy rules as chat.
    pub fn complete(&self, prompt: &str, n_predict: u32) -> Result<String> {
        let body = json!({ "prompt": prompt, "temperature": 0, "n_predict": n_predict });
        let text = self.post_json("/completion", &body)?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| Error::Inference("completion response is not JSON".into()))?;
        v["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| Error::Inference("no content in completion response".into()))
    }

    fn post_chat(&self, body: &Value) -> Result<String> {
        let text = self.post_json("/v1/chat/completions", body)?;
        let envelope: Value = serde_json::from_str(&text)
            .map_err(|_| Error::Inference("response envelope is not JSON".into()))?;
        let content = envelope["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| Error::Inference("no message content in response".into()))?;
        Ok(content.to_string())
    }

    /// The one POST every endpoint goes through: loopback URL, JSON body,
    /// bearer auth, and the PRIVACY-PRESERVING error mapping — errors carry
    /// status codes and error kinds, never the request or response body, which
    /// on these endpoints can echo prompt content (ADR 0002). Keeping this in
    /// one place is what keeps the rule from silently eroding in one of three
    /// hand-copied variants.
    fn post_json(&self, path: &str, body: &Value) -> Result<String> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
        let resp = req.send_string(&body.to_string()).map_err(|e| match e {
            ureq::Error::Status(code, _) => {
                Error::Inference(format!("llama-server returned HTTP {code}"))
            }
            ureq::Error::Transport(t) => Error::Inference(format!("transport: {}", t.kind())),
        })?;
        resp.into_string()
            .map_err(|e| Error::Inference(format!("reading response: {}", e.kind())))
    }

    /// Liveness probe against llama-server's /health.
    pub fn healthy(&self) -> bool {
        self.agent
            .get(&format!("{}/health", self.base_url))
            .call()
            .is_ok()
    }
}
