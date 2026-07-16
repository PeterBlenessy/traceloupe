//! Telegram native chat module.
//!
//! Telegram doesn't store messages in readable SQL — its `db_sqlite` (the
//! "postbox") holds opaque binary blobs. Schema facts (learned from iLEAPP
//! `telegramMesssages.py`, written fresh — provenance reference, §10):
//! - DB: `.../telegram-data/account-*/postbox/db/db_sqlite`.
//! - `t7(key, value)` — one row per message. The KEY is a big-endian
//!   `(peerId i64, namespace i32, timestamp i32, mid i32)`; `peerId` is the
//!   conversation and `timestamp` is the Unix send time. The VALUE is an
//!   "intermediate message" blob: a linear little-endian byte stream carrying
//!   flag-gated fields, the author id, and the UTF-8 text (see [`parse_message`]).
//! - `t2(key, value)` — peer records; `key` is the peer id, `value` is a
//!   "postbox-encoded" keyed object whose `fn`/`ln`/`t`/`un` fields give the
//!   display name (see [`peer_name`]).
//!
//! We surface text messages: conversation grouping, author, timestamp, and
//! direction. Media/attachments are noted (flag) but their payloads aren't
//! decoded.
//!
//! NOTE: unvalidated against a real Telegram backup — the binary layout is
//! exercised only by synthetic fixtures. Kept behind the iLEAPP fallback.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::{Error, Result};

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "telegram",
    service: "Telegram",
    // Telegram peer ids are bare numbers; we always resolve a chat name (or fall
    // back to the numeric id), so 1:1 threads are never inferred as groups here.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%/postbox/db/db_sqlite")?;
    hits.retain(|e| e.relative_path.ends_with("/postbox/db/db_sqlite"));
    Ok(hits)
}

// ---- little-endian byte reader over the message/postbox blobs ----

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(overflow)?;
        let s = self.b.get(self.pos..end).ok_or_else(truncated)?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n)?;
        Ok(())
    }
    /// int32 length-prefixed byte string.
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.i32()?;
        let len = usize::try_from(len).map_err(|_| truncated())?;
        self.take(len)
    }
    /// int32 length-prefixed UTF-8 string.
    fn string(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
    /// uint8 length-prefixed byte string.
    fn short_bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.u8()? as usize;
        self.take(len)
    }
    fn short_string(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.short_bytes()?).into_owned())
    }
    fn at_end(&self) -> bool {
        self.pos >= self.b.len()
    }
}

fn truncated() -> Error {
    Error::Parse("telegram: truncated blob".into())
}
fn overflow() -> Error {
    Error::Parse("telegram: length overflow".into())
}

// MessageDataFlags (uint8) — gate optional header fields.
const DF_GLOBALLY_UNIQUE_ID: u8 = 1 << 0;
const DF_GLOBAL_TAGS: u8 = 1 << 1;
const DF_GROUPING_KEY: u8 = 1 << 2;
const DF_GROUP_INFO: u8 = 1 << 3;
const DF_LOCAL_TAGS: u8 = 1 << 4;
const DF_THREAD_ID: u8 = 1 << 5;
// MessageFlags (uint32).
const MF_INCOMING: u32 = 4;
// FwdInfoFlags (uint8).
const FWD_SOURCE_ID: u8 = 1 << 1;
const FWD_SOURCE_MESSAGE: u8 = 1 << 2;
const FWD_SIGNATURE: u8 = 1 << 3;
const FWD_PSA_TYPE: u8 = 1 << 4;
const FWD_FLAGS: u8 = 1 << 5;

/// The decoded fields of one message we care about.
struct DecodedMessage {
    author_id: Option<i64>,
    text: String,
    incoming: bool,
    has_media: bool,
}

/// Skip the forwarded-info block (present iff its leading flag byte is non-zero).
fn skip_fwd_info(c: &mut Cursor) -> Result<()> {
    let flags = c.u8()?;
    if flags == 0 {
        return Ok(());
    }
    c.skip(8 + 4)?; // authorId i64 + date i32
    if flags & FWD_SOURCE_ID != 0 {
        c.skip(8)?;
    }
    if flags & FWD_SOURCE_MESSAGE != 0 {
        c.skip(8 + 4 + 4)?;
    }
    if flags & FWD_SIGNATURE != 0 {
        c.string()?;
    }
    if flags & FWD_PSA_TYPE != 0 {
        c.string()?;
    }
    if flags & FWD_FLAGS != 0 {
        c.skip(4)?;
    }
    Ok(())
}

/// Parse a `t7` message value into the fields we surface. Returns `Ok(None)` for a
/// non-message (`type != 0`) record.
fn parse_message(value: &[u8]) -> Result<Option<DecodedMessage>> {
    let mut c = Cursor::new(value);
    if c.u8()? != 0 {
        return Ok(None); // not an intermediate message
    }
    c.skip(4 + 4)?; // stableId u32 + stableVer u32
    let df = c.u8()?;
    if df & DF_GLOBALLY_UNIQUE_ID != 0 {
        c.skip(8)?;
    }
    if df & DF_GLOBAL_TAGS != 0 {
        c.skip(4)?;
    }
    if df & DF_GROUPING_KEY != 0 {
        c.skip(8)?;
    }
    if df & DF_GROUP_INFO != 0 {
        c.skip(4)?;
    }
    if df & DF_LOCAL_TAGS != 0 {
        c.skip(4)?;
    }
    if df & DF_THREAD_ID != 0 {
        c.skip(8)?;
    }
    let flags = c.u32()?;
    c.skip(4)?; // tags u32
    skip_fwd_info(&mut c)?;
    let author_id = if c.u8()? == 1 { Some(c.i64()?) } else { None };
    let text = c.string()?;
    // attributes[] then embeddedMedia[] — count only, to flag media presence.
    let attr_count = c.i32()?.max(0) as usize;
    for _ in 0..attr_count {
        c.bytes()?;
    }
    let media_count = c.i32().unwrap_or(0).max(0) as usize;
    Ok(Some(DecodedMessage {
        author_id,
        text,
        incoming: flags & MF_INCOMING != 0,
        has_media: media_count > 0,
    }))
}

/// The big-endian `t7` key: (peerId i64, namespace i32, timestamp i32, mid i32).
fn parse_message_key(key: &[u8]) -> Option<(i64, i64)> {
    if key.len() < 20 {
        return None;
    }
    let peer_id = i64::from_be_bytes(key[0..8].try_into().ok()?);
    let timestamp = i32::from_be_bytes(key[12..16].try_into().ok()?) as i64;
    Some((peer_id, timestamp))
}

// ---- minimal PostboxDecoder: enough to pull a peer's display name ----

/// A reader over a postbox keyed-object blob: a flat sequence of
/// `short_str key` + `uint8 valueType` + typed value.
struct PostboxReader<'a> {
    c: Cursor<'a>,
}

// ValueType tags.
const VT_INT32: u8 = 0;
const VT_INT64: u8 = 1;
const VT_BOOL: u8 = 2;
const VT_DOUBLE: u8 = 3;
const VT_STRING: u8 = 4;
const VT_OBJECT: u8 = 5;
const VT_INT32_ARRAY: u8 = 6;
const VT_INT64_ARRAY: u8 = 7;
const VT_OBJECT_ARRAY: u8 = 8;
const VT_OBJECT_DICT: u8 = 9;
const VT_BYTES: u8 = 10;
const VT_NIL: u8 = 11;
const VT_STRING_ARRAY: u8 = 12;
const VT_BYTES_ARRAY: u8 = 13;

/// Extract a peer's display name from a `t2` value. The record wraps the peer
/// under an Object at key `_`; the peer's own fields include `fn`/`ln` (person
/// first/last), `t` (group/channel title), and `un` (username).
fn peer_name(blob: &[u8]) -> Option<String> {
    let fields = PostboxReader::new(blob).root_object_fields()?;
    let name = match (fields.get("fn"), fields.get("ln"), fields.get("t")) {
        (Some(fnm), ln, _) => {
            let mut s = fnm.clone();
            if let Some(l) = ln {
                if !l.is_empty() {
                    s.push(' ');
                    s.push_str(l);
                }
            }
            s
        }
        (None, _, Some(t)) => t.clone(),
        _ => fields.get("un").cloned()?,
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

impl<'a> PostboxReader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { c: Cursor::new(b) }
    }

    /// Find the Object stored under key `_` and return its string fields.
    fn root_object_fields(&mut self) -> Option<HashMap<String, String>> {
        while !self.c.at_end() {
            let key = self.c.short_string().ok()?;
            let vt = self.c.u8().ok()?;
            if key == "_" && vt == VT_OBJECT {
                // Object = int32 typeHash + int32 dataLen + data.
                self.c.skip(4).ok()?;
                let data = self.c.bytes().ok()?;
                return Some(Self::new(data).string_fields());
            }
            self.skip_value(vt).ok()?;
        }
        None
    }

    /// Collect the top-level String fields of this blob (ignore non-strings).
    fn string_fields(&mut self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        while !self.c.at_end() {
            let Ok(key) = self.c.short_string() else {
                break;
            };
            let Ok(vt) = self.c.u8() else { break };
            if vt == VT_STRING {
                if let Ok(s) = self.c.string() {
                    out.insert(key, s);
                }
            } else if self.skip_value(vt).is_err() {
                break;
            }
        }
        out
    }

    /// Advance past one value of the given type without interpreting it.
    fn skip_value(&mut self, vt: u8) -> Result<()> {
        match vt {
            VT_INT32 => self.c.skip(4),
            VT_INT64 | VT_DOUBLE => self.c.skip(8),
            VT_BOOL => self.c.skip(1),
            VT_STRING | VT_BYTES => self.c.bytes().map(|_| ()),
            VT_NIL => Ok(()),
            VT_OBJECT => {
                self.c.skip(4)?; // typeHash
                self.c.bytes().map(|_| ()) // dataLen + data
            }
            VT_INT32_ARRAY => {
                let n = self.c.i32()?.max(0) as usize;
                self.c.skip(n.saturating_mul(4))
            }
            VT_INT64_ARRAY => {
                let n = self.c.i32()?.max(0) as usize;
                self.c.skip(n.saturating_mul(8))
            }
            VT_STRING_ARRAY | VT_BYTES_ARRAY => {
                let n = self.c.i32()?.max(0) as usize;
                for _ in 0..n {
                    self.c.bytes()?;
                }
                Ok(())
            }
            VT_OBJECT_ARRAY => {
                let n = self.c.i32()?.max(0) as usize;
                for _ in 0..n {
                    self.c.skip(4)?;
                    self.c.bytes()?;
                }
                Ok(())
            }
            VT_OBJECT_DICT => {
                let n = self.c.i32()?.max(0) as usize;
                for _ in 0..n {
                    // key object + value object
                    self.c.skip(4)?;
                    self.c.bytes()?;
                    self.c.skip(4)?;
                    self.c.bytes()?;
                }
                Ok(())
            }
            _ => Err(Error::Parse(format!("telegram: unknown value type {vt}"))),
        }
    }
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // The postbox has t7 (messages) and t2 (peers). Absence → not this schema.
    let has_t7: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='t7'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !has_t7 {
        return Ok(Vec::new());
    }

    // Resolve peer names lazily, caching by id.
    let mut peer_cache: HashMap<i64, Option<String>> = HashMap::new();
    let mut peer_name = |id: i64| -> Option<String> {
        if let Some(cached) = peer_cache.get(&id) {
            return cached.clone();
        }
        let name = conn
            .query_row("SELECT value FROM t2 WHERE key = ?1 LIMIT 1", [id], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .ok()
            .and_then(|blob| peer_name(&blob));
        peer_cache.insert(id, name.clone());
        name
    };

    let mut stmt = conn.prepare("SELECT key, value FROM t7")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let key: Vec<u8> = r.get(0)?;
        let value: Vec<u8> = r.get(1)?;
        let Some((peer_id, timestamp)) = parse_message_key(&key) else {
            continue;
        };
        // A malformed message blob shouldn't abort the whole DB — skip the row.
        let Ok(Some(msg)) = parse_message(&value) else {
            continue;
        };

        let chat_name = peer_name(peer_id);
        // Sender: the author peer's name (falls back to the peer for a 1:1).
        let sender_name = msg
            .author_id
            .and_then(&mut peer_name)
            .or_else(|| chat_name.clone());

        out.push(AppMessage {
            attachments: Vec::new(),
            chat_key: peer_id.to_string(),
            chat_name,
            timestamp: Some(timestamp),
            body: if msg.text.is_empty() {
                None
            } else {
                Some(msg.text)
            },
            is_from_me: !msg.incoming,
            sender_name: if msg.incoming { sender_name } else { None },
            sender_handle: None,
            sender_id: msg.author_id.map(|a| a.to_string()),
            has_attachment: msg.has_media,
            kind: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDb;
    use crate::normalize::ImportReport;

    // --- helpers to encode the exact binary formats the parser expects ---

    fn le_i32(v: i32, out: &mut Vec<u8>) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn lp_string(s: &str, out: &mut Vec<u8>) {
        le_i32(s.len() as i32, out);
        out.extend_from_slice(s.as_bytes());
    }

    /// A minimal `t7` message value: type 0, no data flags, given incoming flag,
    /// author id, and text; zero attributes/media.
    fn message_value(incoming: bool, author_id: i64, text: &str) -> Vec<u8> {
        let mut v = Vec::new();
        v.push(0u8); // type
        le_i32(0, &mut v); // stableId
        le_i32(0, &mut v); // stableVer
        v.push(0u8); // dataFlags: none
        let flags: u32 = if incoming { MF_INCOMING } else { 0 };
        v.extend_from_slice(&flags.to_le_bytes());
        le_i32(0, &mut v); // tags
        v.push(0u8); // fwd_info flags = 0 (absent)
        v.push(1u8); // hasAuthorId
        v.extend_from_slice(&author_id.to_le_bytes());
        lp_string(text, &mut v);
        le_i32(0, &mut v); // attributesCount
        le_i32(0, &mut v); // embeddedMediaCount
        v
    }

    fn message_key(peer_id: i64, timestamp: i32, mid: i32) -> Vec<u8> {
        let mut k = Vec::new();
        k.extend_from_slice(&peer_id.to_be_bytes()); // big-endian!
        k.extend_from_slice(&0i32.to_be_bytes()); // namespace
        k.extend_from_slice(&timestamp.to_be_bytes());
        k.extend_from_slice(&mid.to_be_bytes());
        k
    }

    fn short_str(s: &str, out: &mut Vec<u8>) {
        out.push(s.len() as u8);
        out.extend_from_slice(s.as_bytes());
    }

    /// A `t2` peer value: `_` → Object{ fn: first, un: username }.
    fn peer_value(first: &str, username: &str) -> Vec<u8> {
        // inner object data: fields fn (String) and un (String)
        let mut inner = Vec::new();
        short_str("fn", &mut inner);
        inner.push(VT_STRING);
        lp_string(first, &mut inner);
        short_str("un", &mut inner);
        inner.push(VT_STRING);
        lp_string(username, &mut inner);

        let mut v = Vec::new();
        short_str("_", &mut v);
        v.push(VT_OBJECT);
        le_i32(0, &mut v); // typeHash (ignored)
        le_i32(inner.len() as i32, &mut v);
        v.extend_from_slice(&inner);
        v
    }

    fn make_postbox(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("db_sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE t2 (key INTEGER PRIMARY KEY, value BLOB);
             CREATE TABLE t7 (key BLOB PRIMARY KEY, value BLOB);",
        )
        .unwrap();
        // Peer 500 = "Nadia (@nadia)", the conversation + the incoming author.
        conn.execute(
            "INSERT INTO t2 (key, value) VALUES (500, ?1)",
            [peer_value("Nadia", "nadia")],
        )
        .unwrap();
        // Incoming from 500 in chat 500, then an outgoing reply.
        conn.execute(
            "INSERT INTO t7 (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                message_key(500, 1_700_000_000, 1),
                message_value(true, 500, "privet")
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO t7 (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                message_key(500, 1_700_000_100, 2),
                message_value(false, 999, "hi Nadia")
            ],
        )
        .unwrap();
        db
    }

    #[test]
    fn parse_message_round_trips() {
        let v = message_value(true, 500, "hello");
        let m = parse_message(&v).unwrap().unwrap();
        assert!(m.incoming);
        assert_eq!(m.author_id, Some(500));
        assert_eq!(m.text, "hello");
        assert!(!m.has_media);
    }

    #[test]
    fn peer_name_decodes() {
        let v = peer_value("Nadia", "nadia");
        assert_eq!(peer_name(&v).as_deref(), Some("Nadia"));
    }

    #[test]
    fn parses_and_inserts_telegram_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_postbox(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 2);

        // Grouped under peer 500, named "Nadia".
        let incoming = msgs.iter().find(|m| !m.is_from_me).unwrap();
        assert_eq!(incoming.chat_key, "500");
        assert_eq!(incoming.chat_name.as_deref(), Some("Nadia"));
        assert_eq!(incoming.body.as_deref(), Some("privet"));
        assert_eq!(incoming.timestamp, Some(1_700_000_000));
        assert!(msgs
            .iter()
            .any(|m| m.is_from_me && m.body.as_deref() == Some("hi Nadia")));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "Telegram", false, msgs, &mut report)
            .unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 2);
        let name: String = cache
            .conn()
            .query_row("SELECT display_name FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Nadia");
    }

    #[test]
    fn truncated_blob_is_an_error_not_a_panic() {
        // A message value cut short must return Err, not panic.
        let mut v = message_value(true, 500, "hi");
        v.truncate(6);
        assert!(parse_message(&v).is_err() || parse_message(&v).unwrap().is_none());
    }
}
