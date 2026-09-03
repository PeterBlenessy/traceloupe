//! Minimal OpenAI-compatible chat client for the local llama-server sidecar
//! (plan T5). Non-streaming — the engine wants whole verdict objects, and
//! chunk latency is dominated by generation either way.
//!
//! Privacy invariants (ADR 0002): NOTHING from a request or response is ever
//! logged here — errors carry status codes and parse messages, never prompt or
//! completion text.
//!
//! **The base URL is no longer loopback by construction.** Users may point the
//! scan at their own model — Ollama, LM Studio, vLLM, or a hosted API — which
//! means message text can leave the machine. That is a deliberate,
//! opt-in-only change: the command layer refuses to build a remote client
//! without explicit consent, and this module simply honours whatever base URL
//! it is given. Two consequences live here:
//!
//! * **Dialect.** The local sidecar is llama.cpp and takes a GBNF `grammar`
//!   field; no third-party server does. OpenAI-compatible endpoints take
//!   `response_format` instead, and reject unknown fields outright. So the
//!   request body branches on which kind of server is on the other end.
//! * **What leaves.** Only the deep scan is remoted. The census embedder stays
//!   local always — embedding every message remotely would send the whole phone
//!   to a third party, which is the opposite of the product's purpose. With the
//!   router in front (#544), the deep scan sees roughly the top 5% of a device.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{Error, Result};

/// Which server is on the other end, and therefore how structured output is
/// requested. Not cosmetic: llama.cpp needs `grammar`, and OpenAI-compatible
/// servers return HTTP 400 for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// The bundled llama-server. GBNF grammar, and `cache_prompt` to reuse the
    /// ~1000-token system prefix across focused calls.
    #[default]
    LlamaCpp,
    /// Anything speaking the OpenAI chat API: Ollama, LM Studio, vLLM, hosted
    /// providers. Structured output via `response_format`, no llama.cpp extras.
    OpenAi,
}

pub struct LlmClient {
    agent: ureq::Agent,
    base_url: String,
    model: String,
    /// Sent as `Authorization: Bearer …` when the server requires `--api-key`.
    api_key: Option<String>,
    dialect: Dialect,
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
            dialect: Dialect::default(),
        }
    }

    /// Speak to a third-party OpenAI-compatible server rather than the bundled
    /// llama-server.
    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
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
        let body = self.chat_json_body(system, user, grammar, max_tokens);
        let content = self.post_chat(&body)?;
        let content = strip_code_fence(&content);
        serde_json::from_str(content)
            .map_err(|_| Error::Inference("completion content is not valid JSON".into()))
    }

    /// The exact request body [`Self::chat_json`] sends — shared with the
    /// prompt-cache measurement harness so a benchmark can never drift into
    /// measuring a request production does not make.
    ///
    /// `cache_prompt` is sent EXPLICITLY rather than trusting the server
    /// default: sequential focused calls share the ~1000-token system-prompt
    /// prefix, and the measured saving (~86% of prompt eval; see the
    /// validation doc) must be a property of the request, not of whatever a
    /// future llama-server build defaults to.
    pub(crate) fn chat_json_body(
        &self,
        system: &str,
        user: &str,
        grammar: &str,
        max_tokens: u32,
    ) -> Value {
        let mut body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": max_tokens,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        match self.dialect {
            Dialect::LlamaCpp => {
                body["grammar"] = json!(grammar);
                body["cache_prompt"] = json!(true);
            }
            Dialect::OpenAi => {
                // `grammar` and `cache_prompt` are llama.cpp extensions and a
                // strict server rejects the whole request for them. The nearest
                // portable constraint is JSON mode; the shape is then enforced
                // by parsing, and a model that ignores it fails the same way a
                // malformed local response does — one skipped chunk, audited.
                body["response_format"] = json!({ "type": "json_object" });
            }
        }
        body
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
        let body = json!({ "prompt": prompt, "temperature": 0, "n_predict": n_predict, "cache_prompt": true });
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

/// Some OpenAI-compatible servers wrap the completion in a ```json fence even
/// in JSON mode. Stripping it is the difference between "works with the user's
/// Ollama" and "skips every chunk"; on unfenced content this is a no-op.
fn strip_code_fence(content: &str) -> &str {
    let t = content.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim_start_matches('\n')
        .strip_suffix("```")
        .unwrap_or(rest)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_llama_cpp_body_carries_its_grammar_and_the_openai_body_does_not() {
        let local = LlmClient::new("http://127.0.0.1:1", "m", Duration::from_secs(1));
        let body = local.chat_json_body("s", "u", "root ::= \"x\"", 64);
        assert!(body["grammar"].is_string(), "llama.cpp needs the grammar");
        assert_eq!(body["cache_prompt"], json!(true));
        assert!(body["response_format"].is_null());

        let remote = LlmClient::new("https://example.invalid", "m", Duration::from_secs(1))
            .with_dialect(Dialect::OpenAi);
        let body = remote.chat_json_body("s", "u", "root ::= \"x\"", 64);
        assert!(
            body["grammar"].is_null() && body["cache_prompt"].is_null(),
            "llama.cpp extensions must never reach a third-party server: a \
             strict one rejects the whole request"
        );
        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert_eq!(body["temperature"], json!(0));
    }

    #[test]
    fn fenced_json_from_a_third_party_server_still_parses() {
        assert_eq!(strip_code_fence("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("  {\"a\":1}  "), "{\"a\":1}");
    }
}
