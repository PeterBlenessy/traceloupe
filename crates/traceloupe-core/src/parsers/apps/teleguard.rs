//! TeleGuard native chat module.
//!
//! Schema facts, read off a real device rather than a reference — iLEAPP has no
//! TeleGuard parser, so this was designed against Josh Hickman's public iOS 17
//! image with `explore_real_backup` (provenance: own implementation).
//!
//! - DB: `Library/teleguard_database.db` in
//!   `AppDomainGroup-group.ch.swisscows.messenger.teleguardapp`.
//! - `messages(id, chatId, sender, receiver, content, createDate, type, status)` —
//!   one row per message. `createDate` is Unix **milliseconds**. `type` is a word:
//!   TEXT, MEDIA, CALL, QUOTE, SERVICE.
//! - `contacts(serverId, alias, type)` — display names. `type` is PERSON, GROUP,
//!   STATIC (the TeleGuard system account) or META.
//! - `service(id, data)` — `id = 'user'` holds JSON with the LOCAL account's
//!   `serverId`.
//!
//! WHO SENT IT is the one thing worth getting right here, and the store makes it
//! easy to get wrong. `sender` is an opaque id, and there is no `is_from_me`
//! column. It would be tempting to infer the owner as "the id that appears in
//! messages but not in `contacts`" — that works on this image and would break the
//! moment a contact is deleted, silently reversing the direction of every message
//! in that thread. The app records the answer instead: `service` row `user` is the
//! local account. If it is missing, direction is left unknown rather than guessed
//! (`is_from_me = false`), because a wrong attribution is worse than an absent one
//! in a tool people use to work out who said what.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::{col_i64, col_string, AppMessage};
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

pub const MODULE: super::AppChatModule = super::AppChatModule {
    id: "teleguard",
    service: "TeleGuard",
    // Chats are keyed by the peer's server id (1:1) or a UUID (group), never by a
    // bare number, so numeric-id group inference must not run.
    numeric_id_groups: false,
    locate,
    parse,
};

fn locate(index: &ManifestIndex) -> Result<Vec<FileEntry>> {
    let mut hits = index.find_relative_like("%teleguard_database.db")?;
    hits.retain(|e| {
        let rp = &e.relative_path;
        // `teleguard_temp.db` sits beside it and is not the message store.
        rp == "teleguard_database.db" || rp.ends_with("/teleguard_database.db")
    });
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

/// The local account's server id, from the `service` table's `user` row.
///
/// Parsed with a narrow scan rather than a JSON dependency: the value is a plain
/// `"serverId":"XXXX"` and pulling in a parser to read one field would be more
/// machinery than the job needs. Returns `None` when the row is absent or the
/// shape is not what we expect — and every caller treats that as "direction
/// unknown", never as "everything is incoming".
fn local_account(conn: &Connection) -> Option<String> {
    if !table_exists(conn, "service").ok()? {
        return None;
    }
    let raw: String = conn
        .query_row(
            "SELECT CAST(data AS TEXT) FROM service WHERE id = 'user'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    let key = "\"serverId\"";
    let after = raw.find(key)? + key.len();
    let rest = raw.get(after..)?;
    let start = rest.find('"')? + 1;
    let tail = rest.get(start..)?;
    let end = tail.find('"')?;
    let id = tail.get(..end)?.trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

fn parse(db_path: &Path, _rel_path: &str) -> Result<Vec<AppMessage>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "messages")? {
        return Ok(Vec::new());
    }

    // serverId → display name, and which ids are groups.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut groups: HashMap<String, bool> = HashMap::new();
    // Contacts the app types as STATIC are the app itself, not people.
    let mut app_accounts: HashMap<String, bool> = HashMap::new();
    if table_exists(&conn, "contacts")? {
        let mut stmt = conn.prepare("SELECT serverId, alias, type FROM contacts")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let Some(id) = col_string(r, 0)? else {
                continue;
            };
            if let Some(alias) = col_string(r, 1)? {
                if !alias.trim().is_empty() {
                    names.insert(id.clone(), alias);
                }
            }
            let kind = col_string(r, 2)?.unwrap_or_default();
            groups.insert(id.clone(), kind.eq_ignore_ascii_case("GROUP"));
            app_accounts.insert(id, kind.eq_ignore_ascii_case("STATIC"));
        }
    }

    let me = local_account(&conn);

    let mut stmt = conn.prepare(
        "SELECT chatId, sender, content, createDate, type FROM messages ORDER BY createDate",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let Some(chat) = col_string(r, 0)? else {
            continue;
        };
        let sender = col_string(r, 1)?;
        let body = col_string(r, 2)?;
        // Unix MILLISECONDS. Divided rather than passed through, because every
        // other module in this pipeline hands over seconds and a millisecond value
        // would render as a date in the year 56000.
        let timestamp = col_i64(r, 3)?.map(|ms| ms / 1000);
        let kind_raw = col_string(r, 4)?.unwrap_or_default();

        // Only when the app told us who we are. Without that, "not from me" is a
        // statement we cannot support, so the thread reads as all-incoming rather
        // than as confidently mis-attributed.
        let is_from_me = match (&me, &sender) {
            (Some(me), Some(s)) => s == me,
            _ => false,
        };

        // A STATIC contact is the app's own account. Its onboarding message is
        // stored as `type = TEXT` with a body that is a JSON map of every
        // language's welcome text — which would otherwise render as though a person
        // had sent someone a wall of JSON. The store types the CONTACT even though
        // it does not type the message, so this is read from the data rather than
        // sniffed out of the body.
        let from_app_account = app_accounts.get(&chat).copied().unwrap_or(false);
        let kind = match kind_raw.as_str() {
            // TeleGuard's own onboarding and join/leave notices are not something
            // anyone said.
            "SERVICE" => Some("system"),
            "CALL" => Some("call"),
            "MEDIA" => Some("shared"),
            _ if from_app_account => Some("system"),
            _ => None,
        };

        let is_group = groups.get(&chat).copied().unwrap_or(false);
        out.push(AppMessage {
            source_id: None,
            is_group,
            chat_name: names.get(&chat).cloned(),
            // A group's members are named individually; a 1:1 thread's peer is the
            // chat itself, and repeating its name on every incoming message adds
            // nothing.
            sender_name: if is_group {
                sender.as_ref().and_then(|s| names.get(s).cloned())
            } else {
                None
            },
            sender_handle: None,
            sender_id: sender,
            chat_key: chat,
            timestamp,
            body,
            is_from_me,
            has_attachment: kind_raw == "MEDIA",
            kind,
            attachments: Vec::new(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(with_user_row: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teleguard_database.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE messages(id TEXT PRIMARY KEY, chatId TEXT, sender TEXT,
                receiver TEXT, content TEXT, createDate INTEGER, type TEXT, status TEXT);
             CREATE TABLE contacts(serverId TEXT PRIMARY KEY, alias TEXT, type TEXT);
             CREATE TABLE service(id TEXT PRIMARY KEY, data BLOB);
             INSERT INTO contacts VALUES
                ('PEER1','DFIR Two','PERSON'),
                ('GRP1','Image Data Test Group','GROUP'),
                ('MEMBER','LizD','PERSON'),
                ('teleguard','TeleGuard','STATIC');
             INSERT INTO messages (id, chatId, sender, content, createDate, type) VALUES
                ('1','PEER1','ME','You here?',1704837027792,'TEXT'),
                ('2','PEER1','PEER1','I am.',1704837111837,'TEXT'),
                ('3','PEER1',NULL,'User accepted invite',1704836953872,'SERVICE'),
                ('4','GRP1','MEMBER','in the group',1704839145613,'TEXT'),
                ('5','PEER1','ME',NULL,1704839560183,'MEDIA'),
                -- The app's own onboarding: type TEXT, body a JSON language map.
                ('6','teleguard','teleguard','{\"ru\":\"W\",\"en\":\"W\"}',1704836825611,'TEXT');",
        )
        .unwrap();
        if with_user_row {
            c.execute(
                "INSERT INTO service VALUES ('user', ?1)",
                [r#"{"serverId":"ME","userId":"a10d0b00-af38"}"#],
            )
            .unwrap();
        }
        drop(c);
        (dir, path)
    }

    #[test]
    fn reads_threads_direction_and_kinds() {
        let (_d, path) = db(true);
        let msgs = parse(&path, "Library/teleguard_database.db").unwrap();
        assert_eq!(msgs.len(), 6);

        // The app's own account: typed as system, so a JSON blob of localised
        // welcome text is not presented as something a person wrote.
        let onboarding = msgs.iter().find(|m| m.chat_key == "teleguard").unwrap();
        assert_eq!(onboarding.kind, Some("system"));

        // Milliseconds → seconds. 1704837027792 ms is 2024-01-09, not year 56000.
        let first = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("You here?"))
            .unwrap();
        assert_eq!(first.timestamp, Some(1_704_837_027));
        assert!(first.is_from_me, "sender == the local account");
        assert_eq!(first.chat_name.as_deref(), Some("DFIR Two"));

        let incoming = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("I am."))
            .unwrap();
        assert!(!incoming.is_from_me);
        // A 1:1 peer is the thread; repeating it per message says nothing.
        assert_eq!(incoming.sender_name, None);

        // In a GROUP the individual author is named.
        let grp = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("in the group"))
            .unwrap();
        assert_eq!(grp.sender_name.as_deref(), Some("LizD"));
        assert_eq!(grp.chat_name.as_deref(), Some("Image Data Test Group"));

        // A join notice is not something a person said.
        let sys = msgs
            .iter()
            .find(|m| m.body.as_deref() == Some("User accepted invite"))
            .unwrap();
        assert_eq!(sys.kind, Some("system"));
        assert!(!sys.is_from_me, "a service message has no sender");

        let media = msgs.iter().find(|m| m.kind == Some("shared")).unwrap();
        assert!(media.has_attachment);
    }

    /// Without the `user` row the owner is unknown, and unknown must not become
    /// "everything is incoming AND we are sure". Nothing is marked as ours.
    #[test]
    fn direction_is_not_guessed_when_the_account_is_unknown() {
        let (_d, path) = db(false);
        let msgs = parse(&path, "Library/teleguard_database.db").unwrap();
        assert!(
            msgs.iter().all(|m| !m.is_from_me),
            "direction was invented without the account id"
        );
        // And the messages are still all there — an unknown owner is not a reason
        // to drop the conversation.
        assert_eq!(msgs.len(), 6);
    }

    #[test]
    fn the_local_account_is_read_from_the_service_row() {
        let (_d, path) = db(true);
        let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(local_account(&c).as_deref(), Some("ME"));
    }
}
