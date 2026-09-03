//! Bring your own model: pointing the deep scan at an endpoint the user runs.
//!
//! People already run Ollama, LM Studio or vLLM, often with models far larger
//! than anything this app would bundle. Letting them point the scan at one is
//! both a quality option and a licence simplification — nobody is distributing
//! weights, so the model's terms are between the user and whoever published it.
//!
//! **This is the one place message text can leave the machine, so the type
//! makes that impossible to do by accident.** A [`RemoteEndpoint`] cannot be
//! constructed without `acknowledged_sends_text: true`; there is no setter, no
//! default, and no way to flip it after the fact. A caller that forgets consent
//! gets `None` and the scan runs locally, which is the safe failure.
//!
//! Scope, deliberately: only the DEEP SCAN and its confirmer are remoted. The
//! census embedder stays local always — embedding every message remotely would
//! ship the entire device to a third party, which is the opposite of what this
//! product is for. With the router in front (#544) the deep scan reads roughly
//! the top 5% of a phone, so that is the ceiling on what a remote endpoint can
//! ever see.

use std::time::Duration;

use super::client::{Dialect, LlmClient};

/// A user-supplied OpenAI-compatible endpoint. Construction is the consent
/// gate — see the module docs.
#[derive(Debug, Clone)]
pub struct RemoteEndpoint {
    base_url: String,
    model: String,
    api_key: Option<String>,
}

/// Why an endpoint was rejected. Returned to the UI verbatim, so each reads as
/// something a person can act on.
#[derive(Debug, PartialEq, Eq)]
pub enum EndpointError {
    NoConsent,
    EmptyUrl,
    NotHttp,
    EmptyModel,
}

impl EndpointError {
    pub fn message(&self) -> &'static str {
        match self {
            EndpointError::NoConsent => {
                "sending message text to another server needs your explicit confirmation"
            }
            EndpointError::EmptyUrl => "enter the address of your model server",
            EndpointError::NotHttp => "the address must start with http:// or https://",
            EndpointError::EmptyModel => "enter the model name your server expects",
        }
    }
}

impl RemoteEndpoint {
    /// Build an endpoint, or refuse. `acknowledged_sends_text` is the user's
    /// answer to "this sends the messages being scanned to that server" — it is
    /// a required argument rather than a field so it cannot be defaulted, and
    /// the value is never stored, because consent is a decision at scan time,
    /// not a saved preference that outlives the reason it was given.
    pub fn new(
        base_url: &str,
        model: &str,
        api_key: Option<String>,
        acknowledged_sends_text: bool,
    ) -> Result<Self, EndpointError> {
        if !acknowledged_sends_text {
            return Err(EndpointError::NoConsent);
        }
        let url = base_url.trim().trim_end_matches('/');
        if url.is_empty() {
            return Err(EndpointError::EmptyUrl);
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(EndpointError::NotHttp);
        }
        let model = model.trim();
        if model.is_empty() {
            return Err(EndpointError::EmptyModel);
        }
        Ok(Self {
            base_url: url.to_string(),
            model: model.to_string(),
            api_key: api_key.filter(|k| !k.trim().is_empty()),
        })
    }

    /// True when the endpoint is NOT on this machine — the case the UI must
    /// describe honestly. Loopback endpoints (a local Ollama) still leave the
    /// device's data on the device.
    pub fn leaves_this_machine(&self) -> bool {
        let authority = self
            .base_url
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("");
        // IPv6 authorities are bracketed — "[::1]:8080" — so the port cannot be
        // split off on the first colon.
        let host = if let Some(rest) = authority.strip_prefix('[') {
            rest.split(']').next().unwrap_or("")
        } else {
            authority.split(':').next().unwrap_or("")
        };
        !matches!(host, "127.0.0.1" | "localhost" | "::1" | "0.0.0.0")
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn client(&self, timeout: Duration) -> LlmClient {
        LlmClient::new(&self.base_url, &self.model, timeout)
            .with_api_key(self.api_key.clone())
            .with_dialect(Dialect::OpenAi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The consent gate is the type's constructor. This is the test that has to
    /// keep passing for the privacy claim to mean anything.
    #[test]
    fn an_endpoint_cannot_exist_without_consent() {
        assert_eq!(
            RemoteEndpoint::new("https://api.example.com", "m", None, false).unwrap_err(),
            EndpointError::NoConsent
        );
        assert!(RemoteEndpoint::new("https://api.example.com", "m", None, true).is_ok());
    }

    #[test]
    fn addresses_are_checked_before_anything_is_sent() {
        for (url, want) in [
            ("", EndpointError::EmptyUrl),
            ("   ", EndpointError::EmptyUrl),
            ("api.example.com", EndpointError::NotHttp),
            ("ftp://example.com", EndpointError::NotHttp),
        ] {
            assert_eq!(
                RemoteEndpoint::new(url, "m", None, true).unwrap_err(),
                want,
                "url {url:?}"
            );
        }
        assert_eq!(
            RemoteEndpoint::new("https://x.example", "  ", None, true).unwrap_err(),
            EndpointError::EmptyModel
        );
    }

    /// A local Ollama is not "sending your messages to a company", and telling
    /// the user it is would be a false warning that teaches them to ignore
    /// warnings.
    #[test]
    fn a_local_endpoint_is_not_described_as_leaving_the_machine() {
        for local in [
            "http://127.0.0.1:11434",
            "http://localhost:1234",
            "http://[::1]:8080",
        ] {
            let e = RemoteEndpoint::new(local, "m", None, true).unwrap();
            assert!(!e.leaves_this_machine(), "{local} is on this machine");
        }
        let e = RemoteEndpoint::new("https://api.openai.com", "gpt-4o", None, true).unwrap();
        assert!(e.leaves_this_machine());
    }

    /// A blank key is not a key: sending `Authorization: Bearer ` breaks some
    /// servers outright.
    #[test]
    fn a_blank_api_key_is_treated_as_absent() {
        let e = RemoteEndpoint::new("https://x.example", "m", Some("   ".into()), true).unwrap();
        assert!(e.api_key.is_none());
    }

    /// The client an endpoint hands out must speak the OpenAI dialect — if it
    /// defaulted to llama.cpp's, every request would carry a `grammar` field
    /// and a strict server would reject the whole scan.
    #[test]
    fn the_client_speaks_the_third_party_dialect() {
        let e = RemoteEndpoint::new("https://x.example", "m", None, true).unwrap();
        let client = e.client(Duration::from_secs(1));
        assert_eq!(client.dialect(), Dialect::OpenAi);
        assert_eq!(client.model(), "m");
    }

    #[test]
    fn trailing_slashes_do_not_produce_double_slash_urls() {
        let e = RemoteEndpoint::new("https://x.example/", "m", None, true).unwrap();
        assert_eq!(e.base_url(), "https://x.example");
    }
}
