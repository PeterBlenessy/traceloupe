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
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One classification call: system + user message, grammar-constrained by
    /// `response_format`, temperature 0. Returns the completion content parsed
    /// as JSON.
    pub fn chat_json(
        &self,
        system: &str,
        user: &str,
        response_format: &Value,
        max_tokens: u32,
    ) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": max_tokens,
            "response_format": response_format,
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

    fn post_chat(&self, body: &Value) -> Result<String> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| match e {
                // Never include the response body: on this endpoint it can
                // echo prompt content.
                ureq::Error::Status(code, _) => {
                    Error::Inference(format!("llama-server returned HTTP {code}"))
                }
                ureq::Error::Transport(t) => Error::Inference(format!("transport: {}", t.kind())),
            })?;
        let text = resp
            .into_string()
            .map_err(|e| Error::Inference(format!("reading response: {}", e.kind())))?;
        let envelope: Value = serde_json::from_str(&text)
            .map_err(|_| Error::Inference("response envelope is not JSON".into()))?;
        let content = envelope["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| Error::Inference("no message content in response".into()))?;
        Ok(content.to_string())
    }

    /// Liveness probe against llama-server's /health.
    pub fn healthy(&self) -> bool {
        self.agent
            .get(&format!("{}/health", self.base_url))
            .call()
            .is_ok()
    }
}
