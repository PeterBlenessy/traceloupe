//! TikTok native chat **message** parser.
//!
//! Unlike the other app modules, TikTok spans two databases, so it is driven by a
//! dedicated importer (`import_tiktok_messages_native`) rather than the generic
//! single-file `AppChatModule` registry.
//!
//! Schema facts (learned from iLEAPP `tikTok.py`, written fresh — provenance §10),
//! validated against a real backup:
//! - Messages live in `…/Library/Application Support/ChatFiles/<account_id>/db.sqlite`
//!   — NOT `AwemeIM.db` (which holds only the contact/social graph). `account_id`
//!   is the folder name (the local user's uid), used to tell sent from received.
//! - `TIMMessageORM(localCreatedAt, sender, content, belongingConversationIdentifier)`
//!   — one row per message; `content` is JSON with `$.text` for the body;
//!   `localCreatedAt` is a Unix timestamp (fractional seconds).
//! - Sender display names are resolved from the `AwemeContacts*` tables in
//!   `AwemeIM.db` (a separate DB), passed in as a `uid → (nickname, @handle)` map
//!   built by [`crate::parsers::tiktok_contacts::collect_uid_map`].

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::{col_i64, col_string, AppMessage};
use crate::Result;

/// `uid → (nickname, @handle)` for resolving message senders.
pub type ContactMap = HashMap<String, (Option<String>, Option<String>)>;

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// The local account id is the name of the directory holding the chat `db.sqlite`
/// (`…/ChatFiles/<account_id>/db.sqlite`). Used to tell sent from received.
fn account_id_from_path(rel_path: &str) -> Option<String> {
    let parts: Vec<&str> = rel_path.trim_end_matches('/').split('/').collect();
    // …/<account_id>/db.sqlite → second-to-last component.
    parts
        .len()
        .checked_sub(2)
        .and_then(|i| parts.get(i))
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// TikTok timestamps are Unix; some columns are milliseconds. Normalize to seconds.
fn to_unix_secs(v: i64) -> i64 {
    if v > 100_000_000_000 {
        v / 1000
    } else {
        v
    }
}

/// Turn a message's `content` JSON into `(display body, content kind)`, or `None`
/// to skip it. `kind` is the Messages content-filter bucket.
///
/// TikTok DMs come in many shapes (the numeric `type` column): real text carries
/// `$.text`; other kinds put nothing renderable in `content` (the shared video,
/// sticker image, etc. live on TikTok's servers, not in the backup). Rather than
/// drop those or render blank bubbles, we surface a typed marker classified by the
/// content's shape. Empty control/system messages (`{}`, `"placeholder"`) → `None`.
fn describe_message(content: Option<&str>) -> Option<(String, &'static str)> {
    let j: serde_json::Value = content.and_then(|c| serde_json::from_str(c).ok())?;
    let nonempty = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // Real user text (classified link/text downstream by the inserter — pass
    // "text" here and let a URL body fall into the shared "link" bucket via the
    // generic classifier isn't wired for app kinds, so keep it simple as "text").
    if let Some(t) = nonempty(&j, "text") {
        return Some((t, "text"));
    }
    // System notification ("Message request accepted", "Streak ended", …).
    if let Some(tip) = nonempty(&j, "tips") {
        return Some((tip, "system"));
    }
    // A shared TikTok post (video) carries an `aweme_id` (+ cover thumbnail). Test
    // for a *non-null* value: the envelope may serialize `"aweme_id": null` on a
    // sticker/other message, and `get(..).is_some()` is true for an explicit null.
    if j.get("aweme_id").is_some_and(|v| !v.is_null()) {
        return Some(("📹 Shared a video".to_string(), "shared"));
    }
    // A shared profile card. `aweType` may be stored INTEGER, REAL, or a string.
    let awe_type = j.get("aweType").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    });
    if awe_type == Some(800) {
        return Some(("👤 Shared a profile".to_string(), "shared"));
    }
    // Sticker / GIF / nudge (has an image/sticker id). Non-null, as above.
    if j.get("sticker_id").is_some_and(|v| !v.is_null())
        || j.get("image_id").is_some_and(|v| !v.is_null())
    {
        if j.get("display_name").and_then(|v| v.as_str()) == Some("nudge") {
            return Some(("👋 Nudge".to_string(), "sticker"));
        }
        return Some(("🖼 Sticker".to_string(), "sticker"));
    }
    // Empty control/system message — nothing to show.
    None
}

/// Parse TikTok messages from one `ChatFiles/<account>/db.sqlite`. `rel_path` is the
/// Manifest path (its parent dir names the local account, for direction); `contacts`
/// resolves sender uids to names. A DB without `TIMMessageORM` yields an empty vec.
pub fn parse_tiktok_messages(
    db_path: &Path,
    rel_path: &str,
    contacts: &ContactMap,
) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "TIMMessageORM")? {
        return Ok(Vec::new());
    }
    let account_id = account_id_from_path(rel_path);

    let mut stmt = conn.prepare(
        "SELECT localCreatedAt, sender, content, belongingConversationIdentifier
         FROM TIMMessageORM
         ORDER BY belongingConversationIdentifier, localCreatedAt",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        // `localCreatedAt` is fractional seconds (REAL); col_i64 floors it.
        let timestamp = col_i64(r, 0)?.filter(|t| *t > 1).map(to_unix_secs);
        // `sender` is a uid, stored INTEGER or TEXT. Read `content` and `chat_key`
        // tolerantly too (some builds store `content` as a BLOB or a numeric group
        // id) — a strict `get()` would error out of `next()?` and abort the WHOLE
        // account's messages on a single odd row.
        let sender = col_string(r, 1)?;
        let content = col_string(r, 2)?;
        let chat_key = col_string(r, 3)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());

        // The body/kind are derived from `content` JSON: real text, a system-notice
        // string, or a typed marker for shared videos/stickers/profiles. Empty
        // control messages return None and are skipped.
        let Some((body, kind)) = describe_message(content.as_deref()) else {
            continue;
        };

        let is_from_me = match (&sender, &account_id) {
            (Some(s), Some(a)) => s == a,
            _ => false,
        };
        let (sender_name, sender_handle) = sender
            .as_ref()
            .and_then(|uid| contacts.get(uid))
            .map(|(n, h)| (n.clone(), h.clone()))
            .unwrap_or((None, None));

        out.push(AppMessage {
            attachments: Vec::new(),
            chat_key,
            chat_name: None, // derived from the peer
            timestamp,
            body: Some(body),
            is_from_me,
            sender_name: if is_from_me { None } else { sender_name },
            sender_handle: if is_from_me { None } else { sender_handle },
            sender_id: sender,
            has_attachment: false,
            kind: Some(kind),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_is_the_parent_dir() {
        assert_eq!(
            account_id_from_path("Library/Application Support/ChatFiles/9988/db.sqlite").as_deref(),
            Some("9988")
        );
    }

    fn make_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("db.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE TIMMessageORM (localCreatedAt REAL, sender INTEGER, content TEXT,
                 belongingConversationIdentifier TEXT);
             INSERT INTO TIMMessageORM VALUES (1700000000.5, 200, '{\"text\":\"hi from tiktok\"}', 'conv1');
             INSERT INTO TIMMessageORM VALUES (1700000100.0, 999, '{\"text\":\"sent by me\"}', 'conv1');
             -- Shared video → '📹 Shared a video'.
             INSERT INTO TIMMessageORM VALUES (1700000200.0, 200, '{\"aweme_id\":\"7243968354254425387\"}', 'conv1');
             -- Sticker → '🖼 Sticker'. Carries an explicit null aweme_id, which must
             -- NOT be misread as a shared video (a bare presence test would fail here).
             INSERT INTO TIMMessageORM VALUES (1700000300.0, 200, '{\"sticker_id\":\"abc\",\"aweme_id\":null,\"image_type\":\"gif\"}', 'conv1');
             -- System notice → the $.tips text.
             INSERT INTO TIMMessageORM VALUES (1700000400.0, 200, '{\"tips\":\"Streak ended\"}', 'conv1');
             -- Empty control message → skipped.
             INSERT INTO TIMMessageORM VALUES (1700000500.0, 200, '{}', 'conv1');",
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_messages_direction_and_names() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        // account_id 999 = the local user (from the path); uid 200 → "Robin".
        let mut contacts = ContactMap::new();
        contacts.insert(
            "200".to_string(),
            (Some("Robin".to_string()), Some("@robin_tt".to_string())),
        );
        let msgs = parse_tiktok_messages(&db, "ChatFiles/999/db.sqlite", &contacts).unwrap();
        // 2 texts + video + sticker + system tip = 5; the empty '{}' is skipped.
        assert_eq!(msgs.len(), 5);
        let bodies: Vec<&str> = msgs.iter().filter_map(|m| m.body.as_deref()).collect();
        assert_eq!(
            bodies,
            vec![
                "hi from tiktok",
                "sent by me",
                "📹 Shared a video",
                "🖼 Sticker",
                "Streak ended",
            ]
        );

        let incoming = &msgs[0];
        assert_eq!(incoming.chat_key, "conv1");
        assert_eq!(incoming.sender_name.as_deref(), Some("Robin"));
        assert_eq!(incoming.sender_handle.as_deref(), Some("@robin_tt"));
        assert_eq!(incoming.timestamp, Some(1_700_000_000)); // fractional secs floored
        assert!(!incoming.is_from_me);
        assert!(msgs[1].is_from_me && msgs[1].body.as_deref() == Some("sent by me"));
    }
}
