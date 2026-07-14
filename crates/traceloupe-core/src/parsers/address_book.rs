//! Native parser for iOS `AddressBook.sqlitedb` (Contacts).
//!
//! provenance: reference (own implementation) — schema learned from a real
//! AddressBook.sqlitedb, not ported from iLEAPP.
//!
//! Schema facts:
//! - `ABPerson(ROWID, First, Last, Middle, Organization, …)` — one row/contact.
//! - `ABMultiValue(record_id, property, label, value)` — repeating fields;
//!   `property` 3 = phone, 4 = email; `label` → `ABMultiValueLabel.ROWID`.
//! - `ABMultiValueLabel(value)` — the label text, stored as an iOS magic string
//!   like `_$!<Mobile>!$_`.

use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::Result;

/// A phone number or email address with its (cleaned) label. Round-trips
/// through the cache's JSON columns, hence `Deserialize` too.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LabeledValue {
    pub label: Option<String>,
    pub value: String,
}

/// One parsed contact.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ParsedContact {
    pub id: i64,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub organization: Option<String>,
    pub phones: Vec<LabeledValue>,
    pub emails: Vec<LabeledValue>,
}

const PROP_PHONE: i64 = 3;
const PROP_EMAIL: i64 = 4;

/// Parse all contacts from an AddressBook database, ordered by name.
pub fn parse_address_book(db_path: &Path) -> Result<Vec<ParsedContact>> {
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // One row per person; multivalues joined in. LEFT JOINs so contacts with no
    // phone/email still appear. Ordered so grouping in Rust is a single pass.
    let mut stmt = conn.prepare(
        "SELECT p.ROWID, p.First, p.Last, p.Organization,
                mv.property, mv.value, lbl.value
         FROM ABPerson p
         LEFT JOIN ABMultiValue mv
                ON mv.record_id = p.ROWID AND mv.property IN (?1, ?2)
         LEFT JOIN ABMultiValueLabel lbl ON mv.label = lbl.ROWID
         ORDER BY p.Last IS NULL, p.Last, p.First, p.ROWID, mv.property",
    )?;

    let rows = stmt.query_map([PROP_PHONE, PROP_EMAIL], |r| {
        Ok(Row {
            id: r.get(0)?,
            first: r.get(1)?,
            last: r.get(2)?,
            organization: r.get(3)?,
            property: r.get(4)?,
            value: r.get(5)?,
            label: r.get(6)?,
        })
    })?;

    let mut contacts: Vec<ParsedContact> = Vec::new();
    // Skip a single unreadable row (e.g. non-UTF-8 text) rather than aborting the
    // whole contacts import.
    for row in rows.flatten() {
        // Same person as the last row? (query is grouped by ROWID)
        let contact = match contacts.last_mut() {
            Some(c) if c.id == row.id => c,
            _ => {
                contacts.push(ParsedContact {
                    id: row.id,
                    first_name: row.first,
                    last_name: row.last,
                    organization: row.organization,
                    ..Default::default()
                });
                contacts.last_mut().unwrap()
            }
        };
        if let (Some(prop), Some(value)) = (row.property, row.value) {
            let entry = LabeledValue {
                label: row.label.as_deref().map(clean_label),
                value,
            };
            match prop {
                PROP_PHONE => contact.phones.push(entry),
                PROP_EMAIL => contact.emails.push(entry),
                _ => {}
            }
        }
    }
    Ok(contacts)
}

struct Row {
    id: i64,
    first: Option<String>,
    last: Option<String>,
    organization: Option<String>,
    property: Option<i64>,
    value: Option<String>,
    label: Option<String>,
}

/// Contact photo thumbnails from `AddressBookImages.sqlitedb`, keyed by the
/// ABPerson ROWID (which `ParsedContact.id` also carries, so images line up with
/// contacts). Best-effort: the schema varies across iOS versions, so we try the
/// known tables and skip anything that doesn't fit rather than failing the whole
/// import.
pub fn parse_address_book_images(
    db_path: &Path,
) -> Result<std::collections::HashMap<i64, Vec<u8>>> {
    use std::collections::HashMap;

    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut out: HashMap<i64, Vec<u8>> = HashMap::new();

    // Thumbnails first (format 0, per iLEAPP's addressBook.py), then full-size
    // to cover contacts that only have one. `or_insert` keeps the thumbnail when
    // both exist. We don't break after the first table, so a contact with only a
    // full-size photo still gets one.
    let sources = [
        ("ABThumbnailImage", "AND format = 0"),
        ("ABFullSizeImage", ""),
    ];
    for (table, extra) in sources {
        if !images_table_exists(&conn, table) {
            continue;
        }
        let sql = format!("SELECT record_id, data FROM {table} WHERE data IS NOT NULL {extra}");
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
        else {
            continue;
        };
        for (record_id, data) in rows.flatten() {
            if !data.is_empty() {
                out.entry(record_id).or_insert(data);
            }
        }
    }
    Ok(out)
}

fn images_table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

/// Strip iOS's `_$!<Mobile>!$_` wrapper to `Mobile`; pass other labels through.
fn clean_label(raw: &str) -> String {
    raw.strip_prefix("_$!<")
        .and_then(|s| s.strip_suffix(">!$_"))
        .unwrap_or(raw)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE ABPerson (ROWID INTEGER PRIMARY KEY, First TEXT, Last TEXT, Middle TEXT, Organization TEXT);
             CREATE TABLE ABMultiValueLabel (value TEXT);
             CREATE TABLE ABMultiValue (UID INTEGER PRIMARY KEY, record_id INTEGER, property INTEGER, label INTEGER, value TEXT);
             INSERT INTO ABMultiValueLabel (rowid, value) VALUES (1, '_$!<Mobile>!$_'), (2, '_$!<Home>!$_');
             INSERT INTO ABPerson (ROWID, First, Last, Organization) VALUES (1, 'Alex', 'Rivera', NULL);
             INSERT INTO ABPerson (ROWID, First, Last, Organization) VALUES (2, NULL, NULL, 'Bella Vista Pizza');
             INSERT INTO ABMultiValue (record_id, property, label, value) VALUES (1, 3, 1, '+15551234567');
             INSERT INTO ABMultiValue (record_id, property, label, value) VALUES (1, 4, 2, 'alex@example.com');
             INSERT INTO ABMultiValue (record_id, property, label, value) VALUES (2, 3, 1, '+15550001111');",
        )
        .unwrap();
    }

    #[test]
    fn parses_contacts_with_phones_and_emails() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("AddressBook.sqlitedb");
        make_db(&db);

        let contacts = parse_address_book(&db).unwrap();
        assert_eq!(contacts.len(), 2);

        let alex = contacts
            .iter()
            .find(|c| c.first_name.as_deref() == Some("Alex"))
            .unwrap();
        assert_eq!(alex.last_name.as_deref(), Some("Rivera"));
        assert_eq!(alex.phones.len(), 1);
        assert_eq!(alex.phones[0].value, "+15551234567");
        assert_eq!(alex.phones[0].label.as_deref(), Some("Mobile")); // magic string cleaned
        assert_eq!(alex.emails.len(), 1);
        assert_eq!(alex.emails[0].value, "alex@example.com");

        // Org-only contact with no name.
        let pizza = contacts.iter().find(|c| c.organization.is_some()).unwrap();
        assert!(pizza.first_name.is_none() && pizza.last_name.is_none());
        assert_eq!(pizza.phones.len(), 1);
        assert!(pizza.emails.is_empty());
    }

    #[test]
    fn parses_contact_thumbnails_by_record_id() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("AddressBookImages.sqlitedb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ABThumbnailImage (record_id INTEGER, format INTEGER, data BLOB);
             CREATE TABLE ABFullSizeImage (record_id INTEGER, data BLOB);
             INSERT INTO ABThumbnailImage (record_id, format, data) VALUES (1, 0, x'FFD8FF01');
             INSERT INTO ABThumbnailImage (record_id, format, data) VALUES (3, 1, x'DEAD');
             INSERT INTO ABFullSizeImage (record_id, data) VALUES (2, x'89504E47');",
        )
        .unwrap();

        let images = parse_address_book_images(&db).unwrap();
        // record 1: format-0 thumbnail; record 2: full-size fallback.
        assert_eq!(images.len(), 2);
        assert_eq!(images.get(&1).unwrap(), &vec![0xFF, 0xD8, 0xFF, 0x01]);
        assert_eq!(images.get(&2).unwrap(), &vec![0x89, 0x50, 0x4E, 0x47]);
        // record 3's thumbnail is format 1 (not 0) and has no full-size → skipped.
        assert!(!images.contains_key(&3));
    }

    #[test]
    fn empty_addressbook_yields_no_contacts() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("AddressBook.sqlitedb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ABPerson (ROWID INTEGER PRIMARY KEY, First TEXT, Last TEXT, Middle TEXT, Organization TEXT);
             CREATE TABLE ABMultiValueLabel (value TEXT);
             CREATE TABLE ABMultiValue (UID INTEGER PRIMARY KEY, record_id INTEGER, property INTEGER, label INTEGER, value TEXT);",
        )
        .unwrap();
        assert!(parse_address_book(&db).unwrap().is_empty());
    }
}
