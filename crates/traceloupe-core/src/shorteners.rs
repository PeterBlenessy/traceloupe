//! URL-shortener recognition for the Security Check's opt-in de-shortener.
//!
//! De-shortening resolves a shortened link to reveal its true destination.
//! That contacts a remote host with a URL taken from the backup, the sole
//! sanctioned exception to the "nothing leaves the machine" promise (ADR 0001),
//! so it is a deliberate, per-link, user-approved action. This module does
//! recognition only. The network resolution lives in the shell layer behind the
//! consent gate, and only ever contacts hosts on this allowlist.

/// Known URL-shortener hosts. Kept deliberately to well-known services: the
/// de-shortener will only make requests to these, never to arbitrary hosts.
pub const SHORTENER_HOSTS: &[&str] = &[
    "bit.ly",
    "t.co",
    "tinyurl.com",
    "goo.gl",
    "ow.ly",
    "buff.ly",
    "is.gd",
    "v.gd",
    "cutt.ly",
    "rebrand.ly",
    "t.ly",
    "rb.gy",
    "shorturl.at",
    "tiny.cc",
    "bl.ink",
    "lnkd.in",
    "trib.al",
    "fb.me",
    "amzn.to",
    "youtu.be",
    "wa.me",
    "spoti.fi",
    "apple.co",
    "vm.tiktok.com",
    "vt.tiktok.com",
    "s.id",
    "x.co",
    "po.st",
    "ift.tt",
    "dlvr.it",
    "snip.ly",
    "u.to",
    "chilp.it",
    "shorte.st",
    "adf.ly",
    "qr.ae",
    "clck.ru",
    "l.ead.me",
];

/// Whether `host` is a known shortener (exact match or a subdomain of one).
pub fn is_shortener_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    SHORTENER_HOSTS
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// Extract the host of an `http(s)` URL (lowercased, no port/userinfo).
fn url_host(url: &str) -> Option<String> {
    let lower = url.trim();
    let rest = lower.split_once("://").map(|(_, r)| r)?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Find distinct shortener URLs in free text (e.g. a finding's context snippet).
/// Only `http(s)` URLs whose host is on the allowlist are returned.
pub fn find_shortener_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || "<>\"'(){}[]".contains(c)) {
        let candidate = raw.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let lower = candidate.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            continue;
        }
        if url_host(candidate).is_some_and(|h| is_shortener_host(&h))
            && !out.contains(&candidate.to_string())
        {
            out.push(candidate.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_shortener_hosts() {
        assert!(is_shortener_host("bit.ly"));
        assert!(is_shortener_host("BIT.LY"));
        assert!(is_shortener_host("vm.tiktok.com"));
        // subdomain of a shortener
        assert!(is_shortener_host("cdn.bit.ly"));
        // not shorteners
        assert!(!is_shortener_host("example.com"));
        assert!(!is_shortener_host("notbit.ly.evil.com"));
    }

    #[test]
    fn extracts_shortener_urls_from_text() {
        let text = "check https://bit.ly/abc123 and http://tinyurl.com/xyz. \
                    ignore https://example.com/normal and bit.ly-without-scheme";
        let urls = find_shortener_urls(text);
        assert_eq!(urls.len(), 2);
        assert!(urls.iter().any(|u| u == "https://bit.ly/abc123"));
        assert!(urls.iter().any(|u| u == "http://tinyurl.com/xyz"));
        // trailing punctuation trimmed; non-shortener + scheme-less excluded
        assert!(urls.iter().all(|u| !u.ends_with('.')));
    }

    #[test]
    fn no_urls_in_plain_text() {
        assert!(find_shortener_urls("just some words, no links").is_empty());
    }
}
