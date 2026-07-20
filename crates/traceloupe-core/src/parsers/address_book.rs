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
    pub middle_name: Option<String>,
    pub nickname: Option<String>,
    pub organization: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    /// Birthday as a Unix timestamp (from Core Data's 2001 epoch), or None.
    pub birthday_at: Option<i64>,
    pub note: Option<String>,
    pub phones: Vec<LabeledValue>,
    pub emails: Vec<LabeledValue>,
    /// Postal addresses, each formatted to one line with its (cleaned) label.
    pub addresses: Vec<LabeledValue>,
    /// Related people: label = relationship (Mother / custom), value = name.
    pub related: Vec<LabeledValue>,
    /// Names of the address-book groups this contact belongs to.
    pub groups: Vec<String>,
    /// Social / IM profiles: label = service (Snapchat/…), value = username.
    pub social: Vec<LabeledValue>,
}

const PROP_PHONE: i64 = 3;
const PROP_EMAIL: i64 = 4;
const PROP_ADDRESS: i64 = 5;
const PROP_RELATED: i64 = 23;
const PROP_SOCIAL: i64 = 46;
/// Core Data epoch (2001-01-01) → Unix, for the `Birthday` timestamp column.
const MAC_EPOCH: i64 = 978_307_200;

/// Parse all contacts from an AddressBook database, ordered by name.
pub fn parse_address_book(db_path: &Path) -> Result<Vec<ParsedContact>> {
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // One row per person; multivalues joined in. LEFT JOINs so contacts with no
    // phone/email still appear. Ordered so grouping in Rust is a single pass.
    let mut stmt = conn.prepare(
        "SELECT p.ROWID, p.First, p.Last, p.Organization,
                mv.property, mv.value, lbl.value,
                p.Middle, p.Nickname, p.JobTitle, p.Department, p.Birthday, p.Note
         FROM ABPerson p
         LEFT JOIN ABMultiValue mv
                ON mv.record_id = p.ROWID AND mv.property IN (?1, ?2, ?3)
         LEFT JOIN ABMultiValueLabel lbl ON mv.label = lbl.ROWID
         ORDER BY p.Last IS NULL, p.Last, p.First, p.ROWID, mv.property",
    )?;

    let rows = stmt.query_map([PROP_PHONE, PROP_EMAIL, PROP_RELATED], |r| {
        // Birthday is a Core Data timestamp stored as a TEXT float; parse + shift
        // to Unix. None if absent or unparseable.
        let birthday_at = r
            .get::<_, Option<String>>(11)?
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|t| (t + MAC_EPOCH as f64) as i64);
        Ok(Row {
            id: r.get(0)?,
            first: r.get(1)?,
            last: r.get(2)?,
            organization: r.get(3)?,
            property: r.get(4)?,
            value: r.get(5)?,
            label: r.get(6)?,
            middle: r.get(7)?,
            nickname: r.get(8)?,
            job_title: r.get(9)?,
            department: r.get(10)?,
            birthday_at,
            note: r.get(12)?,
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
                    middle_name: row.middle,
                    nickname: row.nickname,
                    organization: row.organization,
                    job_title: row.job_title,
                    department: row.department,
                    birthday_at: row.birthday_at,
                    note: row.note,
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
                PROP_RELATED => contact.related.push(entry),
                _ => {}
            }
        }
    }

    // Structured postal addresses (property 5) live in the entry tables, not the
    // multivalue `value`; attach them by record_id.
    let mut addrs = parse_addresses(&conn)?;
    for c in &mut contacts {
        if let Some(a) = addrs.remove(&c.id) {
            c.addresses = a;
        }
    }
    // Group memberships, attached by record_id.
    let mut groups = parse_groups(&conn)?;
    for c in &mut contacts {
        if let Some(g) = groups.remove(&c.id) {
            c.groups = g;
        }
    }
    // Social / IM profiles (property 46), attached by record_id.
    let mut social = parse_social(&conn)?;
    for c in &mut contacts {
        if let Some(s) = social.remove(&c.id) {
            c.social = s;
        }
    }
    Ok(contacts)
}

/// Social / instant-messaging profiles (property 46), keyed by ABPerson ROWID.
/// Like postal addresses, each profile's fields live in `ABMultiValueEntry`
/// (`service`, `username`, `url`, …); we surface `service` as the label and
/// `username` as the value. Returns an empty map if the entry tables are
/// absent (older schema).
fn parse_social(conn: &Connection) -> Result<std::collections::HashMap<i64, Vec<LabeledValue>>> {
    use std::collections::HashMap;

    let mut out: HashMap<i64, Vec<LabeledValue>> = HashMap::new();
    for ((rec, _uid), (_label, fields)) in group_multivalue_entries(conn, PROP_SOCIAL)? {
        // The handle is the point; skip a profile that has only metadata.
        if let Some(username) = fields
            .get("username")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            out.entry(rec).or_default().push(LabeledValue {
                // `service` is a plain name here, but clean_label is harmless
                // and keeps social labels consistent with every other parser.
                label: fields.get("service").map(|s| clean_label(s.trim())),
                value: username.to_string(),
            });
        }
    }
    Ok(out)
}

/// Address-book group names keyed by member ABPerson ROWID (`ABGroup` ⋈
/// `ABGroupMembers`). Empty map when the tables are absent (older schema).
fn parse_groups(conn: &Connection) -> Result<std::collections::HashMap<i64, Vec<String>>> {
    let mut out: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    let has_groups = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='ABGroup'
             AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='ABGroupMembers')",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_groups {
        return Ok(out);
    }
    // member_type 0 = person; other kinds (e.g. CardDAV-synced nested groups)
    // carry an ABGroup ROWID in member_id, which must not be matched against
    // ABPerson ROWIDs. Filter when the column exists (older schemas may lack it).
    let has_member_type = conn
        .prepare("PRAGMA table_info(ABGroupMembers)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .any(|c| c == "member_type");
    let sql = format!(
        "SELECT gm.member_id, g.Name
         FROM ABGroupMembers gm
         JOIN ABGroup g ON g.ROWID = gm.group_id
         WHERE g.Name IS NOT NULL AND g.Name <> ''{}
         ORDER BY g.Name, gm.member_id",
        if has_member_type {
            " AND gm.member_type = 0"
        } else {
            ""
        }
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let member: i64 = r.get(0)?;
        let name: String = r.get(1)?;
        out.entry(member).or_default().push(name);
    }
    Ok(out)
}

/// Postal addresses (property 5), formatted one-line, keyed by ABPerson ROWID.
/// Each address's components live in `ABMultiValueEntry(parent_id, key, value)`
/// with `key` → `ABMultiValueEntryKey` ("Street"/"City"/"State"/"ZIP"/"Country"…).
/// Returns an empty map if the entry tables are absent (older schema).
fn parse_addresses(conn: &Connection) -> Result<std::collections::HashMap<i64, Vec<LabeledValue>>> {
    use std::collections::HashMap;

    let mut out: HashMap<i64, Vec<LabeledValue>> = HashMap::new();
    for ((rec, _uid), (label, fields)) in group_multivalue_entries(conn, PROP_ADDRESS)? {
        if let Some(value) = format_address(&fields) {
            out.entry(rec).or_default().push(LabeledValue {
                label: label.as_deref().map(clean_label),
                value,
            });
        }
    }
    Ok(out)
}

/// One multivalue's optional label (from `ABMultiValueLabel`) + its key→value
/// field map (from `ABMultiValueEntry`), keyed by `(record_id, UID)`.
type MultiValueGroup = (Option<String>, std::collections::HashMap<String, String>);

/// Group the `ABMultiValueEntry` rows of every multivalue with the given
/// `property` by `(record_id, UID)`. Shared by the postal-address and
/// social-profile parsers — both are multi-field multivalues that differ only
/// in which keys they read. Ordered (BTreeMap) so output is deterministic;
/// empty when the entry tables are absent (older schema).
fn group_multivalue_entries(
    conn: &Connection,
    property: i64,
) -> Result<std::collections::BTreeMap<(i64, i64), MultiValueGroup>> {
    use std::collections::{BTreeMap, HashMap};

    let mut groups: BTreeMap<(i64, i64), MultiValueGroup> = BTreeMap::new();
    let has_entries = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='ABMultiValueEntry'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_entries {
        return Ok(groups);
    }

    let mut stmt = conn.prepare(
        "SELECT mv.record_id, mv.UID, lbl.value, k.value, e.value
         FROM ABMultiValue mv
         JOIN ABMultiValueEntry e ON e.parent_id = mv.UID
         JOIN ABMultiValueEntryKey k ON e.key = k.ROWID
         LEFT JOIN ABMultiValueLabel lbl ON mv.label = lbl.ROWID
         WHERE mv.property = ?1
         ORDER BY mv.record_id, mv.UID",
    )?;
    let mut rows = stmt.query([property])?;
    while let Some(r) = rows.next()? {
        let rec: i64 = r.get(0)?;
        let uid: i64 = r.get(1)?;
        let label: Option<String> = r.get(2)?;
        let key: Option<String> = r.get(3)?;
        let value: Option<String> = r.get(4)?;
        let entry = groups
            .entry((rec, uid))
            .or_insert_with(|| (label, HashMap::new()));
        if let (Some(k), Some(v)) = (key, value) {
            if !v.trim().is_empty() {
                entry.1.insert(k, v);
            }
        }
    }
    Ok(groups)
}

/// Format an address's components into one line: "Street, City, State ZIP, Country".
fn format_address(fields: &std::collections::HashMap<String, String>) -> Option<String> {
    let get = |k: &str| fields.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    // "City, State ZIP" — comma between city and state, space before the ZIP.
    let mut locality = String::new();
    if let Some(c) = get("City") {
        locality.push_str(c);
    }
    if let Some(s) = get("State") {
        if !locality.is_empty() {
            locality.push_str(", ");
        }
        locality.push_str(s);
    }
    if let Some(z) = get("ZIP") {
        if !locality.is_empty() {
            locality.push(' ');
        }
        locality.push_str(z);
    }
    let locality = (!locality.is_empty()).then_some(locality);
    let parts: Vec<&str> = [get("Street"), locality.as_deref(), get("Country")]
        .into_iter()
        .flatten()
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

struct Row {
    id: i64,
    first: Option<String>,
    last: Option<String>,
    organization: Option<String>,
    property: Option<i64>,
    value: Option<String>,
    label: Option<String>,
    middle: Option<String>,
    nickname: Option<String>,
    job_title: Option<String>,
    department: Option<String>,
    birthday_at: Option<i64>,
    note: Option<String>,
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

/// Insert parsed contacts (with their optional photos) into the cache `contacts`
/// table, returning the number inserted. With `replace = true` the device
/// address-book rows are cleared first — but third-party contacts (e.g. TikTok),
/// which carry their own `source`, are left intact. Shared by the iLEAPP-extracted
/// path and the native self-extracting path.
pub fn insert_contacts(
    cache: &crate::cache::CacheDb,
    contacts: &[ParsedContact],
    images: &std::collections::HashMap<i64, Vec<u8>>,
    replace: bool,
) -> Result<usize> {
    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM contacts WHERE source = 'Address Book'", [])?;
    }
    for c in contacts {
        tx.execute(
            "INSERT INTO contacts
                (first_name, last_name, organization, phones_json, emails_json, image,
                 middle_name, nickname, job_title, department, birthday_at, note, addresses_json,
                 related_json, groups_json, social_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                c.first_name,
                c.last_name,
                c.organization,
                serde_json::to_string(&c.phones).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&c.emails).unwrap_or_else(|_| "[]".into()),
                images.get(&c.id),
                c.middle_name,
                c.nickname,
                c.job_title,
                c.department,
                c.birthday_at,
                c.note,
                serde_json::to_string(&c.addresses).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&c.related).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&c.groups).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&c.social).unwrap_or_else(|_| "[]".into()),
            ],
        )?;
    }
    tx.commit()?;
    Ok(contacts.len())
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
            "CREATE TABLE ABPerson (ROWID INTEGER PRIMARY KEY, First TEXT, Last TEXT, Middle TEXT,
                 Organization TEXT, Nickname TEXT, JobTitle TEXT, Department TEXT, Birthday TEXT, Note TEXT);
             CREATE TABLE ABMultiValueLabel (value TEXT);
             CREATE TABLE ABMultiValue (UID INTEGER PRIMARY KEY, record_id INTEGER, property INTEGER, label INTEGER, value TEXT);
             CREATE TABLE ABMultiValueEntry (parent_id INTEGER, key INTEGER, value TEXT);
             CREATE TABLE ABMultiValueEntryKey (ROWID INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO ABMultiValueLabel (rowid, value) VALUES (1, '_$!<Mobile>!$_'), (2, '_$!<Home>!$_');
             INSERT INTO ABMultiValueEntryKey (ROWID, value) VALUES (1,'Street'),(2,'City'),(3,'State'),(4,'ZIP'),(5,'Country');
             -- Birthday 700000000.0 Core Data = 700000000 + 978307200 = 1678307200 Unix.
             INSERT INTO ABPerson (ROWID, First, Last, JobTitle, Birthday, Note)
                 VALUES (1, 'Alex', 'Rivera', 'Engineer', '700000000.0', 'met at the conference');
             INSERT INTO ABPerson (ROWID, First, Last, Organization) VALUES (2, NULL, NULL, 'Bella Vista Pizza');
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (10, 1, 3, 1, '+15551234567');
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (11, 1, 4, 2, 'alex@example.com');
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (12, 2, 3, 1, '+15550001111');
             -- A Home address (property 5) for Alex, split across entry rows.
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (13, 1, 5, 2, NULL);
             INSERT INTO ABMultiValueEntry (parent_id, key, value) VALUES (13,1,'1 Market St'),(13,2,'Springfield'),(13,3,'CA'),(13,4,'90001'),(13,5,'USA');
             -- A social profile (property 46) for Alex: service + username entry rows.
             INSERT INTO ABMultiValueEntryKey (ROWID, value) VALUES (6,'service'),(7,'username');
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (16, 1, 46, NULL, NULL);
             INSERT INTO ABMultiValueEntry (parent_id, key, value) VALUES (16,6,'Snapchat'),(16,7,'alex_r');
             -- Related names (property 23): a magic label and a custom one.
             INSERT INTO ABMultiValueLabel (rowid, value) VALUES (3, '_$!<Mother>!$_'), (4, 'Bestie');
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (14, 1, 23, 3, 'Maria Rivera');
             INSERT INTO ABMultiValue (UID, record_id, property, label, value) VALUES (15, 1, 23, 4, 'Sam Chen');
             -- Groups: Alex in two, one unnamed group ignored.
             CREATE TABLE ABGroup (ROWID INTEGER PRIMARY KEY, Name TEXT);
             CREATE TABLE ABGroupMembers (UID INTEGER PRIMARY KEY, group_id INTEGER, member_type INTEGER, member_id INTEGER);
             INSERT INTO ABGroup (ROWID, Name) VALUES (1, 'Family'), (2, 'Climbing'), (3, NULL);
             -- Row 5 is a non-person member (member_type 1, e.g. a nested
             -- subgroup) whose member_id collides with contact 2's ABPerson
             -- ROWID — it must NOT tag that contact with 'Family'.
             INSERT INTO ABGroupMembers (UID, group_id, member_type, member_id)
                 VALUES (1, 1, 0, 1), (2, 2, 0, 1), (3, 3, 0, 1), (4, 2, 0, 2), (5, 1, 1, 2);",
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
        assert_eq!(alex.job_title.as_deref(), Some("Engineer"));
        assert_eq!(alex.note.as_deref(), Some("met at the conference"));
        assert_eq!(alex.birthday_at, Some(1_678_307_200)); // 700000000 + MAC_EPOCH
        assert_eq!(alex.addresses.len(), 1);
        assert_eq!(alex.addresses[0].label.as_deref(), Some("Home"));
        assert_eq!(
            alex.addresses[0].value,
            "1 Market St, Springfield, CA 90001, USA"
        );
        // Related names: magic label cleaned, custom label passed through.
        assert_eq!(alex.related.len(), 2);
        assert_eq!(alex.related[0].label.as_deref(), Some("Mother"));
        assert_eq!(alex.related[0].value, "Maria Rivera");
        assert_eq!(alex.related[1].label.as_deref(), Some("Bestie"));
        assert_eq!(alex.related[1].value, "Sam Chen");
        // Groups sorted by name; the unnamed group is dropped.
        assert_eq!(
            alex.groups,
            vec!["Climbing".to_string(), "Family".to_string()]
        );
        // Social profile: service label + username value.
        assert_eq!(alex.social.len(), 1);
        assert_eq!(alex.social[0].label.as_deref(), Some("Snapchat"));
        assert_eq!(alex.social[0].value, "alex_r");

        // Org-only contact with no name.
        let pizza = contacts.iter().find(|c| c.organization.is_some()).unwrap();
        assert!(pizza.first_name.is_none() && pizza.last_name.is_none());
        assert_eq!(pizza.phones.len(), 1);
        assert!(pizza.emails.is_empty());
        // Only the person membership (Climbing); the colliding member_type-1
        // row must not add 'Family'.
        assert_eq!(pizza.groups, vec!["Climbing".to_string()]);
    }

    #[test]
    fn insert_contacts_replace_preserves_third_party_rows() {
        use crate::cache::CacheDb;
        use std::collections::HashMap;

        let cache = CacheDb::open_in_memory().unwrap();
        // A third-party contact (e.g. from TikTok) already in the cache.
        cache
            .conn()
            .execute(
                "INSERT INTO contacts (first_name, phones_json, emails_json, source)
                 VALUES ('Nyx', '[]', '[]', 'TikTok')",
                [],
            )
            .unwrap();

        let people = vec![ParsedContact {
            id: 1,
            first_name: Some("Alex".into()),
            ..Default::default()
        }];
        let images: HashMap<i64, Vec<u8>> = HashMap::new();

        insert_contacts(&cache, &people, &images, false).unwrap();
        // A replace re-import: device rows cleared + re-inserted, TikTok row kept.
        let n = insert_contacts(&cache, &people, &images, true).unwrap();
        assert_eq!(n, 1);

        let device: i64 = cache
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM contacts WHERE source = 'Address Book'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let tiktok: i64 = cache
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM contacts WHERE source = 'TikTok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(device, 1, "replace should not accumulate device contacts");
        assert_eq!(tiktok, 1, "third-party contacts must survive a replace");
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
            "CREATE TABLE ABPerson (ROWID INTEGER PRIMARY KEY, First TEXT, Last TEXT, Middle TEXT,
                 Organization TEXT, Nickname TEXT, JobTitle TEXT, Department TEXT, Birthday TEXT, Note TEXT);
             CREATE TABLE ABMultiValueLabel (value TEXT);
             CREATE TABLE ABMultiValue (UID INTEGER PRIMARY KEY, record_id INTEGER, property INTEGER, label INTEGER, value TEXT);",
        )
        .unwrap();
        assert!(parse_address_book(&db).unwrap().is_empty());
    }
}
