//! Decode an iMessage rich-link payload (`.pluginPayloadAttachment`).
//!
//! When you share a URL in iMessage, the balloon plugin stores the link's rich
//! preview as an NSKeyedArchiver-encoded `LPLinkMetadata` (Apple's Link
//! Presentation framework): the resolved title, the original URL, and often an
//! embedded preview image. [`crate::nska`] resolves the archived object graph;
//! here we pull those fields out — so a link whose *live* page exposes no
//! OpenGraph tags (e.g. an Apple/Google Maps share) still previews from what
//! iMessage already captured, with no network fetch.
//!
//! Standard, app-independent format (LPLinkMetadata), so this is unit-testable on
//! its own against a synthesized archive.

use plist::Value;

use crate::{nska, Result};

/// A decoded rich-link preview. Every field is best-effort — any may be absent.
#[derive(Debug, Clone, Default)]
pub struct RichLink {
    pub url: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    /// The largest embedded image in the archive (the thumbnail), as `(mime, bytes)`.
    pub image: Option<(String, Vec<u8>)>,
}

impl RichLink {
    /// Nothing worth showing was recovered.
    pub fn is_empty(&self) -> bool {
        self.url.is_none() && self.title.is_none() && self.summary.is_none() && self.image.is_none()
    }
}

/// Decode an `LPLinkMetadata` keyed archive. Returns an empty [`RichLink`] (not an
/// error) when the bytes aren't a recognizable archive or carry none of the
/// fields, so callers can treat "no preview" uniformly.
pub fn decode(bytes: &[u8]) -> Result<RichLink> {
    // Path 1 — a keyed archive (binary plist): resolve the object graph and read
    // the LPLinkMetadata fields. Search the whole tree for the dict that carries
    // the link URL rather than assuming a fixed root shape.
    if let Ok(resolved) = nska::resolve(bytes) {
        let meta = find_link_dict(&resolved, 0);
        let rl = RichLink {
            title: meta.and_then(|d| string_field(d, "title")),
            summary: meta.and_then(|d| string_field(d, "summary")),
            // `originalURL` is the URL the user shared; `URL` is the post-redirect
            // canonical one — prefer the former, fall back to the latter.
            url: meta.and_then(|d| url_field(d, "originalURL").or_else(|| url_field(d, "URL"))),
            image: largest_image(&resolved),
        };
        if !rl.is_empty() {
            return Ok(rl);
        }
    }
    // Path 2 — Apple's `streamtyped` (typedstream) format, which iMessage uses for
    // these balloon payloads and which isn't a plist. We don't decode the whole
    // object stream; instead we byte-scan for the embedded thumbnail (stored as
    // raw NSData) and the shared URL. The title isn't reliably locatable this way,
    // and the caller already has the link text from the message body.
    Ok(RichLink {
        url: scan_url(bytes),
        image: scan_embedded_image(bytes),
        ..RichLink::default()
    })
}

/// Find the first `http(s)://…` URL embedded as an ASCII run in the payload.
fn scan_url(bytes: &[u8]) -> Option<String> {
    for scheme in [b"https://".as_slice(), b"http://".as_slice()] {
        if let Some(start) = bytes.windows(scheme.len()).position(|w| w == scheme) {
            let end = bytes[start..]
                .iter()
                .position(|&b| !(0x21..0x7f).contains(&b) || b == b'"')
                .map_or(bytes.len(), |o| start + o);
            if end - start > scheme.len() {
                return std::str::from_utf8(&bytes[start..end]).ok().map(str::to_string);
            }
        }
    }
    None
}

/// Byte-scan for the largest embedded PNG or JPEG (a typedstream stores images as
/// raw NSData, so they appear verbatim in the blob).
fn scan_embedded_image(bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut best: Option<(String, Vec<u8>)> = None;
    let consider = |best: &mut Option<(String, Vec<u8>)>, mime: &str, blob: &[u8]| {
        if best.as_ref().is_none_or(|(_, b)| blob.len() > b.len()) {
            *best = Some((mime.to_string(), blob.to_vec()));
        }
    };
    // PNG: 8-byte signature … IEND chunk (`IEND` + 4-byte CRC).
    const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const PNG_END: &[u8] = b"IEND\xae\x42\x60\x82";
    let mut i = 0;
    while let Some(rel) = bytes[i..].windows(PNG_SIG.len()).position(|w| w == PNG_SIG) {
        let start = i + rel;
        if let Some(erel) = bytes[start..].windows(PNG_END.len()).position(|w| w == PNG_END) {
            let end = start + erel + PNG_END.len();
            consider(&mut best, "image/png", &bytes[start..end]);
            i = end;
        } else {
            break;
        }
    }
    // JPEG: SOI (FF D8 FF) … EOI (FF D9).
    let mut i = 0;
    while let Some(rel) = bytes[i..].windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) {
        let start = i + rel;
        if let Some(erel) = bytes[start + 2..].windows(2).position(|w| w == [0xFF, 0xD9]) {
            let end = start + 2 + erel + 2;
            consider(&mut best, "image/jpeg", &bytes[start..end]);
            i = end;
        } else {
            break;
        }
    }
    best
}

/// Find the LPLinkMetadata-like dict anywhere in the resolved tree: the first one
/// carrying an `originalURL`/`URL` (depth-first). Tolerates the metadata being
/// nested inside a balloon-payload wrapper rather than at the root.
fn find_link_dict(v: &Value, depth: usize) -> Option<&plist::Dictionary> {
    if depth > 64 {
        return None;
    }
    match v {
        Value::Dictionary(d) => {
            if d.contains_key("originalURL") || d.contains_key("URL") {
                return Some(d);
            }
            d.values().find_map(|val| find_link_dict(val, depth + 1))
        }
        Value::Array(a) => a.iter().find_map(|it| find_link_dict(it, depth + 1)),
        _ => None,
    }
}

/// A non-empty, trimmed string property.
fn string_field(d: &plist::Dictionary, key: &str) -> Option<String> {
    let s = d.get(key)?.as_string()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// A URL property: either a plain string, or an NSURL resolved by [`crate::nska`]
/// to `{ "NS.relative": "…", "NS.base": … }` (relative carries the full URL when
/// base is nil, which is the usual case for these payloads).
fn url_field(d: &plist::Dictionary, key: &str) -> Option<String> {
    match d.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Dictionary(u) => u
            .get("NS.relative")
            .and_then(Value::as_string)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// The largest embedded image anywhere in the resolved tree — the rich-link
/// thumbnail. Icons/favicons are smaller, so "largest" reliably picks the main
/// preview over them.
fn largest_image(v: &Value) -> Option<(String, Vec<u8>)> {
    let mut best: Option<(String, Vec<u8>)> = None;
    collect_image(v, &mut best, 0);
    best
}

fn collect_image(v: &Value, best: &mut Option<(String, Vec<u8>)>, depth: usize) {
    if depth > 64 {
        return; // backstop against a pathologically deep resolved tree
    }
    match v {
        Value::Data(bytes) => {
            if let Some(mime) = sniff_image(bytes) {
                if best.as_ref().is_none_or(|(_, b)| bytes.len() > b.len()) {
                    *best = Some((mime.to_string(), bytes.clone()));
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|it| collect_image(it, best, depth + 1)),
        Value::Dictionary(d) => d.values().for_each(|val| collect_image(val, best, depth + 1)),
        _ => {}
    }
}

/// Recognize an embedded preview image by magic bytes.
fn sniff_image(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Uid, Value};
    use std::io::Cursor;

    /// Build an NSKeyedArchiver archive whose root resembles an `LPLinkMetadata`:
    /// `{ title: "Some Place", originalURL: NSURL{NS.relative: "https://…"},
    ///    imageMetadata: { data: <PNG bytes> } }`, encode to bplist, and assert
    /// decode() pulls back the title, URL and embedded image.
    #[test]
    fn decodes_title_url_and_image() {
        // A tiny but valid PNG header + a few bytes so it sniffs as image/png.
        let png: Vec<u8> = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();

        // $objects layout:
        //  0 $null
        //  1 root LPLinkMetadata { title->2, originalURL->3, imageMetadata->6 }
        //  2 "Some Place"
        //  3 NSURL { NS.base->0(null), NS.relative->4 }
        //  4 "https://maps.example/place"
        //  5 "https://maps.example/place" (unused canonical)
        //  6 image holder { data->7 }
        //  7 <PNG data>
        let mut root = plist::Dictionary::new();
        root.insert("title".into(), Value::Uid(Uid::new(2)));
        root.insert("originalURL".into(), Value::Uid(Uid::new(3)));
        root.insert("imageMetadata".into(), Value::Uid(Uid::new(6)));

        let mut nsurl = plist::Dictionary::new();
        nsurl.insert("NS.base".into(), Value::Uid(Uid::new(0)));
        nsurl.insert("NS.relative".into(), Value::Uid(Uid::new(4)));

        let mut img = plist::Dictionary::new();
        img.insert("data".into(), Value::Uid(Uid::new(7)));

        let objects = Value::Array(vec![
            Value::String("$null".into()),
            Value::Dictionary(root),
            Value::String("Some Place".into()),
            Value::Dictionary(nsurl),
            Value::String("https://maps.example/place".into()),
            Value::String("https://maps.example/place".into()),
            Value::Dictionary(img),
            Value::Data(png.clone()),
        ]);
        let mut top = plist::Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));
        let mut archive = plist::Dictionary::new();
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$top".into(), Value::Dictionary(top));
        archive.insert("$objects".into(), objects);

        let mut buf = Vec::new();
        Value::Dictionary(archive)
            .to_writer_binary(Cursor::new(&mut buf))
            .unwrap();

        let rl = decode(&buf).unwrap();
        assert_eq!(rl.title.as_deref(), Some("Some Place"));
        assert_eq!(rl.url.as_deref(), Some("https://maps.example/place"));
        let (mime, bytes) = rl.image.expect("embedded image");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, png);
    }

    /// Non-archive, non-image bytes decode to an empty RichLink, not an error.
    #[test]
    fn non_archive_is_empty() {
        let rl = decode(b"not a plist at all").unwrap_or_default();
        assert!(rl.is_empty());
    }

    /// A `streamtyped` (typedstream) payload — not a plist — still yields the URL
    /// and the embedded PNG via the byte-scan fallback.
    #[test]
    fn scans_typedstream_url_and_image() {
        let png: Vec<u8> = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x00IEND\xae\x42\x60\x82".to_vec();
        let mut blob = b"\x04\x0bstreamtyped\x81\xe8\x03NSObject".to_vec();
        blob.extend_from_slice(b"https://maps.example/place\x00");
        blob.extend_from_slice(&png);
        let rl = decode(&blob).unwrap();
        assert_eq!(rl.url.as_deref(), Some("https://maps.example/place"));
        let (mime, bytes) = rl.image.expect("embedded image");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, png);
    }
}
