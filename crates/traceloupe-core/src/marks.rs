//! The person's own "unsafe" mark, on anything — not just photos.
//!
//! Marking a photo as unsafe already existed, keyed on its `relative_path` and
//! kept in a per-backup file so it survives the cache being rebuilt. That was
//! the right shape and the wrong scope: a message, a contact and a web visit are
//! every bit as likely to be the thing someone wants to come back to, and a
//! second mechanism per data type is how filters drift apart.
//!
//! So there is one table and one durable file, keyed by **(kind, key)**.
//!
//! # The key has to outlive the cache
//!
//! Row ids do not: the cache is rebuilt from scratch on every import, and
//! `messages.id = 41` means nothing afterwards. The key is therefore derived
//! from the item's own content — the same values the source database will
//! produce again next time — so a mark placed today is still on the same message
//! after a re-import tomorrow.
//!
//! That makes the key a hash of stable fields, not an id, and it is computed in
//! SQL ([`KEY_EXPR`]) so the same definition serves reading, writing and
//! counting instead of three that can disagree.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::cache::CacheDb;
use crate::Result;

/// What can be marked. One arm per data type, so a caller cannot invent a kind
/// the key expression has no definition for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkKind {
    Media,
    Message,
    Contact,
}

impl MarkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MarkKind::Media => "media",
            MarkKind::Message => "message",
            MarkKind::Contact => "contact",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "media" => Some(MarkKind::Media),
            "message" => Some(MarkKind::Message),
            "contact" => Some(MarkKind::Contact),
            _ => None,
        }
    }

    /// The table a row of this kind lives in.
    fn table(self) -> &'static str {
        match self {
            MarkKind::Media => "media_items",
            MarkKind::Message => "messages",
            MarkKind::Contact => "contacts",
        }
    }

    /// SQL producing this row's durable key, given the row aliased as `t`.
    ///
    /// Every part is something the source database will produce identically on
    /// the next import. Nothing here is a cache row id.
    fn key_expr(self) -> &'static str {
        match self {
            // A path in the backup: already stable, already what the old
            // per-photo file used, so existing marks carry over untouched.
            MarkKind::Media => "t.relative_path",
            // No message carries a stable id of its own across every source —
            // iMessage has a GUID but app modules do not — so the identity is
            // the thing itself: which conversation, when, which way, and what it
            // said. Two messages matching all four are indistinguishable in the
            // source too.
            MarkKind::Message => {
                "(SELECT COALESCE(th.identifier, '') FROM threads th WHERE th.id = t.thread_id)
                 || '|' || COALESCE(t.sent_at, 0)
                 || '|' || t.is_from_me
                 || '|' || COALESCE(t.body, '')"
            }
            // A person is identified by who they are, not by their row.
            MarkKind::Contact => {
                "COALESCE(t.first_name, '') || '|' || COALESCE(t.last_name, '')
                 || '|' || COALESCE(t.organization, '')
                 || '|' || COALESCE(t.phones_json, '') || COALESCE(t.emails_json, '')"
            }
        }
    }
}

/// One stored mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub kind: String,
    pub key: String,
}

/// Turn a row id into its durable key, or `None` when there is no such row.
pub fn key_for(cache: &CacheDb, kind: MarkKind, id: i64) -> Result<Option<String>> {
    let sql = format!(
        "SELECT {} FROM {} t WHERE t.id = ?1",
        kind.key_expr(),
        kind.table()
    );
    Ok(cache
        .conn()
        .query_row(&sql, [id], |r| r.get::<_, String>(0))
        .optional()?)
}

/// Add or remove a mark, returning the durable key so the caller can persist it.
///
/// The cache table is the fast half; the caller's file is the durable half. Both
/// are updated, because a mark that only lives in the cache is gone at the next
/// import — which is the one moment someone most wants it back.
pub fn set(cache: &CacheDb, kind: MarkKind, id: i64, on: bool) -> Result<Option<String>> {
    let Some(key) = key_for(cache, kind, id)? else {
        return Ok(None);
    };
    let conn = cache.conn();
    if on {
        conn.execute(
            "INSERT OR IGNORE INTO user_marks (kind, key) VALUES (?1, ?2)",
            rusqlite::params![kind.as_str(), key],
        )?;
    } else {
        conn.execute(
            "DELETE FROM user_marks WHERE kind = ?1 AND key = ?2",
            rusqlite::params![kind.as_str(), key],
        )?;
    }
    Ok(Some(key))
}

/// Whether a given row is marked.
pub fn is_marked(cache: &CacheDb, kind: MarkKind, id: i64) -> Result<bool> {
    let sql = format!(
        "SELECT EXISTS(SELECT 1 FROM user_marks m
                        WHERE m.kind = ?1 AND m.key = ({} FROM {} t WHERE t.id = ?2))",
        format_args!("SELECT {}", kind.key_expr()),
        kind.table()
    );
    let n: i64 = cache
        .conn()
        .query_row(&sql, rusqlite::params![kind.as_str(), id], |r| r.get(0))?;
    Ok(n != 0)
}

/// How many rows of `kind` are marked and still present in this backup.
///
/// Counted through the join rather than by counting the table, so a mark left
/// over from a backup that no longer contains the item does not inflate a badge
/// pointing at nothing.
pub fn count(cache: &CacheDb, kind: MarkKind) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM {} t
          WHERE EXISTS(SELECT 1 FROM user_marks m
                        WHERE m.kind = ?1 AND m.key = {})",
        kind.table(),
        kind.key_expr()
    );
    let n: i64 = cache
        .conn()
        .query_row(&sql, [kind.as_str()], |r| r.get(0))?;
    Ok(n)
}

/// A `WHERE` fragment selecting marked rows of `kind`, for a query whose row is
/// reachable as `alias`.
///
/// Exposed so a list query filters by the same definition the badge counts,
/// rather than a second one that can drift — the two disagreeing is how a filter
/// comes to show three of five marked items.
///
/// The subquery is aliased `um` rather than `m`, because the message queries
/// already use `m` for the message itself and a silent shadow there would make
/// the predicate compare a row to itself.
pub fn marked_predicate(kind: MarkKind, alias: &str) -> String {
    format!(
        "EXISTS(SELECT 1 FROM user_marks um WHERE um.kind = '{}' AND um.key = {})",
        kind.as_str(),
        kind.key_expr().replace("t.", &format!("{alias}."))
    )
}

/// The row ids of `kind` that are marked, in this backup.
///
/// Ids rather than a column on every row: a mark is advisory chrome shown next
/// to an item, and threading a `marked` field through every SELECT that builds
/// a message or a contact would touch a dozen queries to carry one boolean.
/// The set is small — it is what a person marked by hand.
pub fn marked_ids(cache: &CacheDb, kind: MarkKind) -> Result<Vec<i64>> {
    let sql = format!(
        "SELECT t.id FROM {} t
          WHERE EXISTS(SELECT 1 FROM user_marks m
                        WHERE m.kind = ?1 AND m.key = {})
          ORDER BY t.id",
        kind.table(),
        kind.key_expr()
    );
    let conn = cache.conn();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([kind.as_str()], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every mark in the cache, for persisting to the durable file.
pub fn all(cache: &CacheDb) -> Result<Vec<Mark>> {
    let conn = cache.conn();
    let mut stmt = conn.prepare("SELECT kind, key FROM user_marks ORDER BY kind, key")?;
    let rows = stmt.query_map([], |r| {
        Ok(Mark {
            kind: r.get(0)?,
            key: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Replace the cache's marks with `marks` — called after an import rebuilt it.
pub fn apply(cache: &CacheDb, marks: &[Mark]) -> Result<()> {
    let conn = cache.conn();
    conn.execute("DELETE FROM user_marks", [])?;
    for m in marks {
        conn.execute(
            "INSERT OR IGNORE INTO user_marks (kind, key) VALUES (?1, ?2)",
            rusqlite::params![m.kind, m.key],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> CacheDb {
        let c = CacheDb::open_in_memory().unwrap();
        {
            let conn = c.conn();
            conn.execute_batch(
                "INSERT INTO threads (id, identifier, display_name, service)
                     VALUES (1, 'chat-guid-1', 'Sam', 'iMessage');
                 INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at)
                     VALUES (1, 1, 'Sam', 0, 'meet me at six', 1000),
                            (2, 1, NULL, 1, 'ok', 1010);
                 INSERT INTO contacts (id, first_name, last_name, organization)
                     VALUES (1, 'Robin', 'Vega', 'Acme');
                 INSERT INTO media_items (id, relative_path, kind)
                     VALUES (1, 'Media/DCIM/100APPLE/IMG_1.HEIC', 'photo');",
            )
            .unwrap();
        }
        c
    }

    #[test]
    fn a_mark_can_be_placed_and_removed_on_any_kind() {
        let c = seeded();
        for (kind, id) in [
            (MarkKind::Media, 1),
            (MarkKind::Message, 1),
            (MarkKind::Contact, 1),
        ] {
            assert!(!is_marked(&c, kind, id).unwrap());
            set(&c, kind, id, true).unwrap();
            assert!(
                is_marked(&c, kind, id).unwrap(),
                "{kind:?} should be marked"
            );
            assert_eq!(count(&c, kind).unwrap(), 1);
            set(&c, kind, id, false).unwrap();
            assert!(!is_marked(&c, kind, id).unwrap());
            assert_eq!(count(&c, kind).unwrap(), 0);
        }
    }

    /// The whole point of deriving the key from content: a re-import rebuilds
    /// the cache with different row ids, and the mark has to land on the same
    /// message anyway.
    #[test]
    fn a_mark_survives_the_cache_being_rebuilt_with_new_row_ids() {
        let first = seeded();
        set(&first, MarkKind::Message, 1, true).unwrap();
        set(&first, MarkKind::Contact, 1, true).unwrap();
        let saved = all(&first).unwrap();
        assert_eq!(saved.len(), 2);

        // A fresh import: same content, different ids and a different thread row.
        let second = CacheDb::open_in_memory().unwrap();
        {
            let conn = second.conn();
            conn.execute_batch(
                "INSERT INTO threads (id, identifier, service)
                     VALUES (77, 'chat-guid-1', 'iMessage');
                 INSERT INTO messages (id, thread_id, sender, is_from_me, body, sent_at)
                     VALUES (500, 77, 'Sam', 0, 'meet me at six', 1000);
                 INSERT INTO contacts (id, first_name, last_name, organization)
                     VALUES (900, 'Robin', 'Vega', 'Acme');",
            )
            .unwrap();
        }
        apply(&second, &saved).unwrap();

        assert!(
            is_marked(&second, MarkKind::Message, 500).unwrap(),
            "the same message, at a new row id, must still be marked"
        );
        assert!(is_marked(&second, MarkKind::Contact, 900).unwrap());
    }

    /// Marking one message must not mark its neighbour in the same conversation.
    #[test]
    fn marks_do_not_leak_between_rows() {
        let c = seeded();
        set(&c, MarkKind::Message, 1, true).unwrap();
        assert!(is_marked(&c, MarkKind::Message, 1).unwrap());
        assert!(!is_marked(&c, MarkKind::Message, 2).unwrap());
    }

    /// Kinds are separate namespaces: media #1 and message #1 are not the same
    /// thing, and a key collision between them would mark both.
    #[test]
    fn kinds_do_not_collide() {
        let c = seeded();
        set(&c, MarkKind::Media, 1, true).unwrap();
        assert!(!is_marked(&c, MarkKind::Message, 1).unwrap());
        assert_eq!(count(&c, MarkKind::Message).unwrap(), 0);
    }

    /// A mark for something this backup no longer has must not inflate a badge
    /// that points at nothing.
    #[test]
    fn a_mark_for_a_missing_item_is_not_counted() {
        let c = seeded();
        apply(
            &c,
            &[Mark {
                kind: "contact".into(),
                key: "Ghost|Person||".into(),
            }],
        )
        .unwrap();
        assert_eq!(count(&c, MarkKind::Contact).unwrap(), 0);
    }

    #[test]
    fn marked_ids_reports_exactly_what_was_marked() {
        let c = seeded();
        set(&c, MarkKind::Message, 2, true).unwrap();
        assert_eq!(marked_ids(&c, MarkKind::Message).unwrap(), vec![2]);
        assert!(marked_ids(&c, MarkKind::Contact).unwrap().is_empty());
    }

    #[test]
    fn setting_a_mark_on_a_missing_row_is_a_no_op() {
        let c = seeded();
        assert_eq!(set(&c, MarkKind::Message, 9999, true).unwrap(), None);
        assert_eq!(count(&c, MarkKind::Message).unwrap(), 0);
    }
}
