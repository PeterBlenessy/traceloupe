//! Native parser for iMessage chat metadata from `sms.db`: group names and
//! participant handles. iLEAPP's `sms` artifact exposes only the group's
//! `chat_identifier` (a `chatNNN…` id), not its name or members, so we read the
//! extracted `sms.db` directly — keyed by `chat.ROWID`, which equals a thread's
//! `identifier` in our cache.
//!
//! provenance: reference (own implementation) from the iMessage `chat.db` schema.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;

use crate::Result;

/// Group name (if any) and participant handles for one chat.
#[derive(Debug, Clone, Default)]
pub struct ChatInfo {
    pub display_name: Option<String>,
    pub participants: Vec<String>,
}

/// Chat metadata keyed by `chat.ROWID`.
pub fn parse_chats(db_path: &Path) -> Result<HashMap<i64, ChatInfo>> {
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut map: HashMap<i64, ChatInfo> = HashMap::new();

    // Group display names (blank ones normalized to None).
    let mut stmt = conn.prepare("SELECT ROWID, display_name FROM chat")?;
    for row in stmt
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .flatten()
    {
        let (rowid, name) = row;
        map.entry(rowid).or_default().display_name = name.filter(|s| !s.trim().is_empty());
    }

    // Participants: chat_handle_join → handle.id (phone/email of each member).
    let mut pstmt = conn.prepare(
        "SELECT chj.chat_id, h.id
         FROM chat_handle_join chj
         JOIN handle h ON h.ROWID = chj.handle_id",
    )?;
    for row in pstmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .flatten()
    {
        let (chat_id, handle) = row;
        let entry = map.entry(chat_id).or_default();
        if !handle.trim().is_empty() && !entry.participants.contains(&handle) {
            entry.participants.push(handle);
        }
    }

    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_group_name_and_members() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sms.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT);
             CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             INSERT INTO chat VALUES (10, 'chat123', 'Bröder');
             INSERT INTO chat VALUES (11, '+15551234567', '');
             INSERT INTO handle VALUES (1, '+15550001111'), (2, '+15550002222'), (3, '+15551234567');
             INSERT INTO chat_handle_join VALUES (10, 1), (10, 2), (11, 3);",
        )
        .unwrap();

        let chats = parse_chats(&db).unwrap();
        let group = &chats[&10];
        assert_eq!(group.display_name.as_deref(), Some("Bröder"));
        assert_eq!(group.participants.len(), 2);
        let direct = &chats[&11];
        assert_eq!(direct.display_name, None); // blank name → None
        assert_eq!(direct.participants, vec!["+15551234567".to_string()]);
    }
}
