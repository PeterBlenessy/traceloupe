//! LinkedIn native chat module.
//!
//! Schema facts (learned from iLEAPP `LinkedIn.py`, written fresh — provenance
//! reference, §10):
//! - DB: `Documents/msg_database.sqlite`.
//! - `messages(deliveredAt, serializedMessage, conversationUrn)` — one row per
//!   message. `deliveredAt` is Unix **milliseconds**. `serializedMessage` is JSON:
//!   `$.body.text` = message; `$.sender.participantType.member.{firstName,
//!   lastName}.text` = the per-message author; `$.sender.participantType.member.
//!   distance == "SELF"` = sent by the owner.
//! - `conversations(conversationUrn, serializedConversation)` — JSON; the chat
//!   name is derived from the first participant whose `distance != "SELF"`
//!   (`$.conversationParticipants[n].participantType.member.{firstName,lastName}`).
//!
//! Grouping is by `conversationUrn`. NOTE: unvalidated against a real backup —
//! behind the iLEAPP fallback.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::AppMessage;
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "linkedin",
    service: "LinkedIn",
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%/Documents/msg_database.sqlite")?;
    hits.retain(|e| e.relative_path.ends_with("/Documents/msg_database.sqlite"));
    Ok(hits)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// LinkedIn timestamps are Unix milliseconds; normalize to seconds.
fn ms_to_secs(v: i64) -> i64 {
    if v > 100_000_000_000 {
        v / 1000
    } else {
        v
    }
}

/// `firstName [lastName]` from a `…member` JSON object, trimmed; None if empty.
fn member_name(member: &Value) -> Option<String> {
    let first = member
        .pointer("/firstName/text")
        .and_then(Value::as_str)
        .unwrap_or("");
    let last = member
        .pointer("/lastName/text")
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = format!("{first} {last}");
    let name = name.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// The chat name for a conversation: the first participant who isn't the owner
/// (`distance != "SELF"`).
fn conversation_name(conv: &Value) -> Option<String> {
    let parts = conv.get("conversationParticipants")?.as_array()?;
    for p in parts {
        let member = p.pointer("/participantType/member")?;
        let is_self = member.pointer("/distance").and_then(Value::as_str) == Some("SELF");
        if !is_self {
            if let Some(name) = member_name(member) {
                return Some(name);
            }
        }
    }
    None
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "messages")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT m.conversationUrn, m.deliveredAt, m.serializedMessage, c.serializedConversation
         FROM messages m
         LEFT JOIN conversations c ON c.conversationUrn = m.conversationUrn
         ORDER BY m.conversationUrn, m.deliveredAt",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let chat_key: String = super::col_string(r, 0)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        let timestamp = super::col_i64(r, 1)?.filter(|t| *t > 0).map(ms_to_secs);
        let msg_json: Option<String> = super::col_string(r, 2)?;
        let conv_json: Option<String> = super::col_string(r, 3)?;

        let msg = msg_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let (body, is_from_me, sender_name) = match &msg {
            Some(m) => {
                let body = m
                    .pointer("/body/text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|s| !s.is_empty());
                let sender = m.pointer("/sender/participantType/member");
                let is_from_me = sender
                    .and_then(|s| s.pointer("/distance"))
                    .and_then(Value::as_str)
                    == Some("SELF");
                let sender_name = sender.and_then(member_name);
                (body, is_from_me, sender_name)
            }
            None => (None, false, None),
        };

        let chat_name = conv_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .as_ref()
            .and_then(conversation_name);

        out.push(AppMessage {
            chat_key,
            chat_name,
            timestamp,
            body,
            is_from_me,
            sender_name: if is_from_me {
                None
            } else {
                sender_name.clone()
            },
            sender_handle: None,
            // No stable member id in the extracted JSON; the name drives group
            // detection (LinkedIn is mostly 1:1, where chat_name wins anyway).
            sender_id: if is_from_me { None } else { sender_name },
            has_attachment: false,
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

    fn msg_json(first: &str, last: &str, distance: &str, body: &str) -> String {
        format!(
            r#"{{"body":{{"text":"{body}"}},"sender":{{"participantType":{{"member":{{"firstName":{{"text":"{first}"}},"lastName":{{"text":"{last}"}},"distance":"{distance}"}}}}}}}}"#
        )
    }

    fn conv_json() -> String {
        // participant 0 = SELF (owner), participant 1 = the peer.
        r#"{"conversationParticipants":[
             {"participantType":{"member":{"firstName":{"text":"Me"},"lastName":{"text":""},"distance":"SELF"}}},
             {"participantType":{"member":{"firstName":{"text":"Dana"},"lastName":{"text":"Ng"},"distance":"DISTANCE_1"}}}
           ]}"#
        .to_string()
    }

    fn make_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("msg_database.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE conversations (conversationUrn TEXT, serializedConversation TEXT);
             CREATE TABLE messages (conversationUrn TEXT, deliveredAt INTEGER, serializedMessage TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversations (conversationUrn, serializedConversation) VALUES ('urn:1', ?1)",
            [conv_json()],
        )
        .unwrap();
        // Incoming from Dana (ms = 1_700_000_000_000), then an outgoing SELF reply.
        conn.execute(
            "INSERT INTO messages (conversationUrn, deliveredAt, serializedMessage) VALUES ('urn:1', 1700000000000, ?1)",
            [msg_json("Dana", "Ng", "DISTANCE_1", "hi there")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (conversationUrn, deliveredAt, serializedMessage) VALUES ('urn:1', 1700000100000, ?1)",
            [msg_json("Me", "", "SELF", "hello Dana")],
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_linkedin_conversation() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_db(tmp.path());
        let msgs = parse(&db, "").unwrap();
        assert_eq!(msgs.len(), 2);

        let incoming = msgs.iter().find(|m| !m.is_from_me).unwrap();
        assert_eq!(incoming.chat_key, "urn:1");
        assert_eq!(incoming.chat_name.as_deref(), Some("Dana Ng"));
        assert_eq!(incoming.body.as_deref(), Some("hi there"));
        assert_eq!(incoming.timestamp, Some(1_700_000_000)); // ms → s
        assert_eq!(incoming.sender_name.as_deref(), Some("Dana Ng"));
        assert!(msgs
            .iter()
            .any(|m| m.is_from_me && m.body.as_deref() == Some("hello Dana")));

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "LinkedIn", false, msgs, &mut report)
            .unwrap();
        assert_eq!(report.threads, 1);
        assert_eq!(report.messages, 2);
        let name: String = cache
            .conn()
            .query_row("SELECT display_name FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Dana Ng");
    }

    /// Edge cases the review flagged: a malformed-JSON row must not abort the
    /// parse; a message with no `conversations` row still parses (peer derived
    /// from the sender); a 3-party group attributes each author per-message.
    #[test]
    fn tolerates_bad_json_and_handles_group() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("msg_database.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE conversations (conversationUrn TEXT, serializedConversation TEXT);
             CREATE TABLE messages (conversationUrn TEXT, deliveredAt INTEGER, serializedMessage TEXT);",
        )
        .unwrap();
        // A group conversation (urn:g) with no `conversations` row → chat_name None.
        conn.execute(
            "INSERT INTO messages (conversationUrn, deliveredAt, serializedMessage) VALUES ('urn:g', 1700000000000, ?1)",
            [msg_json("Alex", "R", "DISTANCE_1", "kickoff at 10")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (conversationUrn, deliveredAt, serializedMessage) VALUES ('urn:g', 1700000100000, ?1)",
            [msg_json("Bina", "S", "DISTANCE_2", "works for me")],
        )
        .unwrap();
        // A malformed-JSON row must not abort the whole parse.
        conn.execute(
            "INSERT INTO messages (conversationUrn, deliveredAt, serializedMessage) VALUES ('urn:g', 1700000200000, '{not valid json')",
            [],
        )
        .unwrap();
        drop(conn);

        let msgs = parse(&db, "").unwrap();
        assert_eq!(
            msgs.len(),
            3,
            "the bad-JSON row is still emitted, not aborted"
        );
        // Per-author attribution in the group.
        assert_eq!(
            msgs.iter()
                .find(|m| m.body.as_deref() == Some("kickoff at 10"))
                .unwrap()
                .sender_name
                .as_deref(),
            Some("Alex R")
        );
        assert_eq!(
            msgs.iter()
                .find(|m| m.body.as_deref() == Some("works for me"))
                .unwrap()
                .sender_name
                .as_deref(),
            Some("Bina S")
        );
        // The malformed row has no body but didn't break anything.
        assert!(msgs.iter().any(|m| m.body.is_none()));

        // With no conversations row, the framework labels the group by its two
        // distinct senders.
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        super::super::insert_app_conversation(&cache, "LinkedIn", false, msgs, &mut report)
            .unwrap();
        let title: String = cache
            .conn()
            .query_row(
                "SELECT display_name FROM threads WHERE identifier = 'urn:g'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(title.starts_with("Group chat"), "got: {title}");
    }
}
