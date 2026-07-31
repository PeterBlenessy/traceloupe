//! Safari web searches, from the two places iOS records them.
//!
//! **Visited searches** are recovered from `History.db`: a search-engine result
//! page is an ordinary history entry whose URL carries the query, so the term is
//! extracted from the URL rather than stored anywhere as text.
//!
//! **Typed searches** come from `com.apple.mobilesafari.plist`'s
//! `RecentWebSearches` — what was typed into the search field, with its own date.
//! These are not the same set: a typed search that was never opened leaves no
//! history entry, and a result page reached from a link leaves no typed entry.
//! Both land in `safari_searches`, tagged by `source`, so neither is mistaken for
//! the other.
//!
//! provenance: reference (own implementation); the two sources and the
//! `RecentWebSearches` shape cross-checked against iLEAPP's `safariWebsearch`
//! and `safariRecentWebSearches` artifacts.

use std::path::Path;

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// A search term recovered from a URL, with the engine it was run against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlSearch {
    pub engine: String,
    pub term: String,
}

/// Search engines whose result URLs we recognise, and the query parameter each
/// puts the term in.
///
/// Matching on the host — rather than iLEAPP's `url LIKE '%search?q=%'` — is what
/// makes DuckDuckGo (`duckduckgo.com/?q=`, no `search` segment) and Yahoo (`?p=`,
/// not `?q=`) reachable at all; both are invisible to a `search?q=` filter.
const ENGINES: &[(&str, &str)] = &[
    ("google.", "q"),
    ("bing.", "q"),
    ("duckduckgo.", "q"),
    ("ecosia.", "q"),
    ("search.brave.", "q"),
    ("startpage.", "q"),
    ("qwant.", "q"),
    ("mojeek.", "q"),
    ("yahoo.", "p"),
    ("baidu.", "wd"),
    ("yandex.", "text"),
];

/// The host portion of a URL, lowercased and without a leading `www.`.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|h| !h.is_empty())?
        // Strip userinfo and port.
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

/// Percent- and plus-decode one query-string value.
///
/// Returns None on a malformed escape or on bytes that are not UTF-8, so a
/// mangled URL yields no search rather than mojibake presented as a search term.
fn decode_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                if i + 2 >= b.len() {
                    return None;
                }
                let hi = (b[i + 1] as char).to_digit(16)?;
                let lo = (b[i + 2] as char).to_digit(16)?;
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Value of `name` in a URL's query string, still encoded.
fn raw_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let after = url.split_once('?')?.1;
    let query = after.split('#').next().unwrap_or(after);
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// The search term a URL carries, if it is a search-engine result page.
///
/// A URL only counts as a search when its host is a recognised engine, or when
/// its path contains a `search` segment and it has a `q`. Without that second
/// condition every URL with a stray `q=` parameter — tracking tokens, pagination
/// — would be reported as something the user searched for.
pub fn search_in_url(url: &str) -> Option<UrlSearch> {
    let host = host_of(url)?;
    let (engine, param) = match ENGINES.iter().find(|(h, _)| host.contains(h)) {
        Some(&(_, param)) => (host.clone(), param),
        // Unknown host: only a path that says "search" qualifies, and only via `q`.
        None => {
            let path = url
                .split_once("://")
                .map(|(_, r)| r)
                .unwrap_or(url)
                .split(['?', '#'])
                .next()
                .unwrap_or("");
            if !path
                .split('/')
                .any(|seg| seg.eq_ignore_ascii_case("search"))
            {
                return None;
            }
            (host.clone(), "q")
        }
    };
    let term = decode_component(raw_param(url, param)?)?;
    let term = term.trim();
    (!term.is_empty()).then(|| UrlSearch {
        engine,
        term: term.to_string(),
    })
}

/// Record one search in the cache. `source` is `visited` (recovered from a
/// history URL) or `typed` (from `RecentWebSearches`).
pub(crate) fn insert_search(
    tx: &rusqlite::Connection,
    term: &str,
    searched_at: Option<i64>,
    source: &str,
    engine: Option<&str>,
    url: Option<&str>,
    profile: Option<&str>,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO safari_searches (term, searched_at, source, engine, url, profile)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![term, searched_at, source, engine, url, profile],
    )?;
    Ok(())
}

/// Parse `com.apple.mobilesafari.plist`'s `RecentWebSearches` into
/// `safari_searches` as `source = 'typed'`.
///
/// Absent or malformed keys are skipped rather than failing the file: this plist
/// holds hundreds of unrelated Safari preferences, and one odd entry should not
/// cost the rest.
pub fn parse_recent_searches(
    plist_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let value = plist::Value::from_file(plist_path)
        .map_err(|e| crate::Error::Parse(format!("com.apple.mobilesafari.plist: {e}")))?;
    let entries = value
        .as_dictionary()
        .and_then(|d| d.get("RecentWebSearches"))
        .and_then(|v| v.as_array());

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM safari_searches WHERE source = 'typed'", [])?;
    }
    let mut n = 0usize;
    for entry in entries.into_iter().flatten() {
        let Some(dict) = entry.as_dictionary() else {
            continue;
        };
        let Some(term) = dict
            .get("SearchString")
            .and_then(|v| v.as_string())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        // The plist stores these as real dates (UTC), not Cocoa reals.
        let searched_at = dict.get("Date").and_then(|v| v.as_date()).map(|d| {
            let t: std::time::SystemTime = d.into();
            match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(dur) => dur.as_secs() as i64,
                Err(e) => -(e.duration().as_secs() as i64),
            }
        });
        insert_search(&tx, term, searched_at, "typed", None, None, None)?;
        n += 1;
    }
    tx.commit()?;
    report.safari_searches += n;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_of(url: &str) -> Option<String> {
        search_in_url(url).map(|s| s.term)
    }

    #[test]
    fn extracts_terms_from_the_common_engines() {
        assert_eq!(
            term_of("https://www.google.com/search?q=mission+peak&oq=x"),
            Some("mission peak".into())
        );
        assert_eq!(
            term_of("https://www.bing.com/search?q=hello%20world"),
            Some("hello world".into())
        );
        // DuckDuckGo has no `search` path segment — invisible to a
        // `LIKE '%search?q=%'` filter, which is why the host list exists.
        assert_eq!(
            term_of("https://duckduckgo.com/?q=tor+browser&ia=web"),
            Some("tor browser".into())
        );
        // Yahoo uses `p`, not `q`.
        assert_eq!(
            term_of("https://search.yahoo.com/search?p=weather"),
            Some("weather".into())
        );
    }

    /// Percent-encoded UTF-8 must come back as text, not as escapes.
    #[test]
    fn decodes_non_ascii_terms() {
        assert_eq!(
            term_of("https://www.google.com/search?q=caf%C3%A9+m%C3%BCnchen"),
            Some("café münchen".into())
        );
    }

    /// A stray `q=` on a non-search URL is not something the user searched for.
    #[test]
    fn ignores_q_parameters_that_are_not_searches() {
        assert_eq!(term_of("https://example.com/page?q=123"), None);
        assert_eq!(
            term_of("https://cdn.example.com/asset.js?q=cachebust"),
            None
        );
        // …but a path that really is a search still counts on an unknown host.
        assert_eq!(
            term_of("https://forum.example.com/search?q=rust"),
            Some("rust".into())
        );
    }

    #[test]
    fn ignores_empty_and_malformed_terms() {
        assert_eq!(term_of("https://www.google.com/search?q="), None);
        assert_eq!(term_of("https://www.google.com/search?q=%zz"), None);
        assert_eq!(term_of("https://www.google.com/search?q=%E0"), None);
        assert_eq!(term_of("https://www.google.com/search"), None);
        assert_eq!(term_of("https://www.google.com/search?q=%20%20"), None);
    }

    #[test]
    fn reports_the_engine_host_without_www() {
        let s = search_in_url("https://www.google.co.uk/search?q=x").unwrap();
        assert_eq!(s.engine, "google.co.uk");
    }

    #[test]
    fn parses_recent_web_searches_from_the_plist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("com.apple.mobilesafari.plist");
        // Mirrors the real iOS 17.3 shape: an array of {SearchString, Date}.
        let mut entry = plist::Dictionary::new();
        entry.insert("SearchString".into(), "digitalcorpora".into());
        entry.insert(
            "Date".into(),
            plist::Value::Date(
                (std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_705_019_864)).into(),
            ),
        );
        // A junk entry alongside the good one: it is skipped, not fatal.
        let mut junk = plist::Dictionary::new();
        junk.insert("SearchString".into(), "   ".into());

        let mut root = plist::Dictionary::new();
        root.insert(
            "RecentWebSearches".into(),
            plist::Value::Array(vec![
                plist::Value::Dictionary(entry),
                plist::Value::Dictionary(junk),
                plist::Value::String("not a dict".into()),
            ]),
        );
        root.insert("UnrelatedPreference".into(), true.into());
        plist::Value::Dictionary(root)
            .to_file_binary(&path)
            .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_recent_searches(&path, &cache, &mut report, false).unwrap();

        assert_eq!(report.safari_searches, 1, "only the well-formed entry");
        let (term, at, source): (String, i64, String) = cache
            .conn()
            .query_row(
                "SELECT term, searched_at, source FROM safari_searches",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(term, "digitalcorpora");
        assert_eq!(at, 1_705_019_864);
        assert_eq!(source, "typed");
    }

    /// A plist with no `RecentWebSearches` at all is a normal state, not an error.
    #[test]
    fn tolerates_a_plist_without_the_key() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("com.apple.mobilesafari.plist");
        let mut root = plist::Dictionary::new();
        root.insert("SomethingElse".into(), 1.into());
        plist::Value::Dictionary(root)
            .to_file_binary(&path)
            .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_recent_searches(&path, &cache, &mut report, false).unwrap();
        assert_eq!(report.safari_searches, 0);
    }
}
