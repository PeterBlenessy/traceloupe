//! Native TikTok contacts / social-graph parser (Phase 2). Replaces iLEAPP's
//! `tiktok_contacts`, so a default import needs no iLEAPP at all.
//!
//! The people a user interacts with on TikTok live in `AwemeIM.db` (the same DB
//! the chat parser reads) in a versioned user table — older builds name it
//! `AwemeContactsV<n>` (columns `uid, nickname, customid`), newer ones
//! `TTKIMContactBaseUserV<n>` (`uid, nickname, customID`). We read every such
//! table, dedup by `uid`, and write each person into the cache `contacts` table
//! tagged `source = 'TikTok'` — matching how the iLEAPP path normalized them
//! (nickname → name, `@customID` → the organization/subtitle).
//!
//! provenance: reference (own implementation) from a real `AwemeIM.db`.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Every user/contact table in the DB (its exact name is version-specific).
fn contact_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
         WHERE type='table'
           AND (name LIKE 'AwemeContacts%' OR name LIKE 'TTKIMContactBaseUser%')",
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

/// Parse TikTok contacts from `AwemeIM.db` into the cache `contacts` table
/// (`source = 'TikTok'`). With `replace`, clears existing TikTok contacts first.
/// Returns the number of contacts inserted.
pub fn parse_tiktok_contacts(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<usize> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // Gather (uid, nickname, @handle) across every version table, dedup by uid.
    let mut people: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
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
            if !seen.insert(uid.clone()) {
                continue;
            }
            let nickname = nickname.filter(|s| !s.trim().is_empty());
            let handle = custom
                .filter(|s| !s.trim().is_empty())
                .map(|h| format!("@{h}"));
            if nickname.is_none() && handle.is_none() {
                continue;
            }
            people.push((uid, nickname, handle));
        }
    }

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM contacts WHERE source = 'TikTok'", [])?;
    }
    for (_uid, nickname, handle) in &people {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_schema_versions_deduped() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("AwemeIM.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE AwemeContactsV5 (uid TEXT, nickname TEXT, customid TEXT);
             CREATE TABLE TTKIMContactBaseUserV18 (uid TEXT, nickname TEXT, customID TEXT);
             INSERT INTO AwemeContactsV5 VALUES ('200', 'Robin', 'robin_tt');
             -- Same uid 200 in the newer table → deduped; a new uid 201 added.
             INSERT INTO TTKIMContactBaseUserV18 VALUES ('200', 'Robin', 'robin_tt');
             INSERT INTO TTKIMContactBaseUserV18 VALUES ('201', 'Sam', 'sammy');
             -- No name and no handle → skipped.
             INSERT INTO TTKIMContactBaseUserV18 VALUES ('202', NULL, NULL);",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        let n = parse_tiktok_contacts(&db, &cache, &mut report, false).unwrap();
        assert_eq!(n, 2, "uid 200 deduped, 202 (empty) skipped");
        assert_eq!(report.contacts, 2);

        let (name, org, source): (String, String, String) = cache
            .conn()
            .query_row(
                "SELECT first_name, organization, source FROM contacts
                 WHERE organization LIKE '@%' ORDER BY first_name LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Robin");
        assert_eq!(org, "@robin_tt");
        assert_eq!(source, "TikTok");
    }
}
