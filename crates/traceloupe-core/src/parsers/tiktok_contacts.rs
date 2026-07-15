//! Native TikTok contacts / social-graph parser (Phase 2). Replaces iLEAPP's
//! `tiktok_contacts`, so a default import needs no iLEAPP at all.
//!
//! The user's TikTok social graph (the people the messaging feature syncs) lives
//! in `AwemeIM.db` in versioned `AwemeContactsV<n>` tables (`uid, nickname,
//! customid/customID, latestchattimestamp`). We read every such table across all
//! account DBs, dedup by `uid`, and write each person into the cache `contacts`
//! table tagged `source = 'TikTok'` — matching how the iLEAPP path normalized
//! them (nickname → name, `@customID` → the organization/subtitle). Surfaced in
//! the Contacts view behind a "TikTok" source pill so the (large) graph never
//! buries the device address book.
//!
//! We deliberately DO NOT read the `TTKIMContactBaseUser<n>` tables: those are the
//! IM SDK's user-info cache (video authors, commenters, anyone the app rendered —
//! tens of thousands of rows that are not the user's contacts). iLEAPP's contacts
//! artifact reads only `AwemeContacts*`, and matching it keeps the graph to the
//! real social graph.
//!
//! provenance: reference (own implementation, diffed against iLEAPP `tikTok.py`)
//! from a real `AwemeIM.db`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Every social-graph table in the DB (its exact name is version-specific, e.g.
/// `AwemeContactsV5`). Excludes `TTKIMContactBaseUser*` (the IM user cache) — see
/// the module doc.
fn contact_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
         WHERE type='table' AND name LIKE 'AwemeContacts%'",
    )?;
    let tables = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tables)
}

/// A column of `table` whose name equals `wanted` case-insensitively (the custom
/// id is `customid` in old schemas, `customID` in new ones).
fn find_col(conn: &Connection, table: &str, wanted: &str) -> Option<String> {
    let ident = table.replace('"', "\"\"");
    let mut stmt = conn
        .prepare(&format!("SELECT name FROM pragma_table_info(\"{ident}\")"))
        .ok()?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .flatten()
        .collect();
    cols.into_iter().find(|c| c.eq_ignore_ascii_case(wanted))
}

/// Collect (uid, nickname, @handle) from one `AwemeIM*.db`, deduping by uid
/// against `seen` (shared across the account DBs).
fn collect_from_db(
    db_path: &Path,
    seen: &mut HashSet<String>,
    out: &mut Vec<(Option<String>, Option<String>)>,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for table in contact_tables(&src)? {
        let (Some(uid_c), Some(nick_c)) = (
            find_col(&src, &table, "uid"),
            find_col(&src, &table, "nickname"),
        ) else {
            continue;
        };
        let custom_c = find_col(&src, &table, "customid"); // customid / customID
        let ident = table.replace('"', "\"\"");
        let custom_sel = custom_c
            .as_deref()
            .map(|c| format!("\"{c}\""))
            .unwrap_or_else(|| "NULL".into());
        let sql = format!(
            "SELECT \"{uid_c}\", \"{nick_c}\", {custom_sel} FROM \"{ident}\"
             WHERE \"{uid_c}\" IS NOT NULL"
        );
        let Ok(mut stmt) = src.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |r| {
            // uid may be TEXT or INTEGER.
            let uid = match r.get_ref(0)? {
                rusqlite::types::ValueRef::Integer(i) => Some(i.to_string()),
                rusqlite::types::ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
                _ => None,
            };
            let nickname: Option<String> = r.get(1)?;
            let custom: Option<String> = r.get(2)?;
            Ok((uid, nickname, custom))
        }) else {
            continue;
        };
        for row in rows.flatten() {
            let (Some(uid), nickname, custom) = row else {
                continue;
            };
            if !seen.insert(uid) {
                continue;
            }
            let nickname = nickname.filter(|s| !s.trim().is_empty());
            let handle = custom
                .filter(|s| !s.trim().is_empty())
                .map(|h| format!("@{h}"));
            if nickname.is_none() && handle.is_none() {
                continue;
            }
            out.push((nickname, handle));
        }
    }
    Ok(())
}

/// Parse TikTok contacts from one or more `AwemeIM*.db` account DBs into the cache
/// `contacts` table (`source = 'TikTok'`), deduped by uid across the files. With
/// `replace`, clears existing TikTok contacts first. Returns the count inserted.
///
/// TikTok keeps a `AwemeIM-<accountid>.db` per logged-in account (the plain
/// `AwemeIM.db` is an empty template), so the caller passes every one it finds.
pub fn parse_tiktok_contacts(
    db_paths: &[PathBuf],
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<usize> {
    let mut people: Vec<(Option<String>, Option<String>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for db in db_paths {
        // A single unreadable/corrupt account DB shouldn't abort the rest.
        let _ = collect_from_db(db, &mut seen, &mut people);
    }

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM contacts WHERE source = 'TikTok'", [])?;
    }
    for (nickname, handle) in &people {
        tx.execute(
            "INSERT INTO contacts
                 (first_name, last_name, organization, phones_json, emails_json, image, source)
             VALUES (?1, NULL, ?2, '[]', '[]', NULL, 'TikTok')",
            rusqlite::params![nickname, handle],
        )?;
    }
    tx.commit()?;
    report.contacts += people.len();
    Ok(people.len())
}

/// `uid → (nickname, @handle)` for every person across the `AwemeContacts*` tables
/// of all account DBs — used to resolve TikTok *message* senders (which store only
/// a uid; iLEAPP performs the same join). First non-empty value per uid wins.
pub fn collect_uid_map(db_paths: &[PathBuf]) -> HashMap<String, (Option<String>, Option<String>)> {
    let mut map: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for db in db_paths {
        let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        let Ok(tables) = contact_tables(&conn) else {
            continue;
        };
        for table in tables {
            let (Some(uid_c), Some(nick_c)) = (
                find_col(&conn, &table, "uid"),
                find_col(&conn, &table, "nickname"),
            ) else {
                continue;
            };
            let custom_c = find_col(&conn, &table, "customid");
            let ident = table.replace('"', "\"\"");
            let custom_sel = custom_c
                .as_deref()
                .map(|c| format!("\"{c}\""))
                .unwrap_or_else(|| "NULL".into());
            let sql = format!(
                "SELECT \"{uid_c}\", \"{nick_c}\", {custom_sel} FROM \"{ident}\"
                 WHERE \"{uid_c}\" IS NOT NULL"
            );
            let Ok(mut stmt) = conn.prepare(&sql) else {
                continue;
            };
            let Ok(rows) = stmt.query_map([], |r| {
                let uid = match r.get_ref(0)? {
                    rusqlite::types::ValueRef::Integer(i) => Some(i.to_string()),
                    rusqlite::types::ValueRef::Text(t) => {
                        Some(String::from_utf8_lossy(t).into_owned())
                    }
                    _ => None,
                };
                let nickname: Option<String> = r.get(1)?;
                let custom: Option<String> = r.get(2)?;
                Ok((uid, nickname, custom))
            }) else {
                continue;
            };
            for row in rows.flatten() {
                let (Some(uid), nickname, custom) = row else {
                    continue;
                };
                let nickname = nickname.filter(|s| !s.trim().is_empty());
                let handle = custom
                    .filter(|s| !s.trim().is_empty())
                    .map(|h| format!("@{h}"));
                map.entry(uid).or_insert((nickname, handle));
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_schema_versions_deduped() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("AwemeIM.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            // Old (`customid`) and new (`customID`) column spellings coexist across
            // versioned tables; a `TTKIMContactBaseUser*` cache table must be ignored.
            "CREATE TABLE AwemeContactsV5 (uid TEXT, nickname TEXT, customid TEXT);
             CREATE TABLE AwemeContactsV6 (uid TEXT, nickname TEXT, customID TEXT);
             CREATE TABLE TTKIMContactBaseUserV18 (uid TEXT, nickname TEXT, customID TEXT);
             INSERT INTO AwemeContactsV5 VALUES ('200', 'Robin', 'robin_tt');
             -- Same uid 200 in a newer table → deduped; a new uid 201 added.
             INSERT INTO AwemeContactsV6 VALUES ('200', 'Robin', 'robin_tt');
             INSERT INTO AwemeContactsV6 VALUES ('201', 'Sam', 'sammy');
             -- No name and no handle → skipped.
             INSERT INTO AwemeContactsV6 VALUES ('202', NULL, NULL);
             -- IM user cache — must NOT be read (would add noise uid 900).
             INSERT INTO TTKIMContactBaseUserV18 VALUES ('900', 'Noise', 'noise_x');",
        )
        .unwrap();

        // A second account DB reuses uid 200 (deduped across files) and adds 300.
        let db2 = tmp.path().join("AwemeIM-999.db");
        let c2 = Connection::open(&db2).unwrap();
        c2.execute_batch(
            "CREATE TABLE AwemeContactsV6 (uid TEXT, nickname TEXT, customID TEXT);
             INSERT INTO AwemeContactsV6 VALUES ('200', 'Robin', 'robin_tt');
             INSERT INTO AwemeContactsV6 VALUES ('300', 'Kai', 'kai_x');",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        let n = parse_tiktok_contacts(&[db, db2], &cache, &mut report, false).unwrap();
        assert_eq!(
            n, 3,
            "uids 200 (x3) deduped, 202 empty skipped, 201 + 300 added, TTKIM uid 900 ignored"
        );
        assert_eq!(report.contacts, 3);

        let (name, org, source): (String, String, String) = cache
            .conn()
            .query_row(
                "SELECT first_name, organization, source FROM contacts
                 WHERE first_name = 'Robin'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Robin");
        assert_eq!(org, "@robin_tt");
        assert_eq!(source, "TikTok");
    }
}
