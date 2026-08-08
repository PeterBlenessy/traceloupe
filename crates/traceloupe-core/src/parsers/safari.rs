//! Native Safari history parser (Phase 2): reads `History.db` directly into the
//! cache `safari_history` table, replacing iLEAPP's `safarihistory` step. Locate +
//! decrypt the DB via the [`crate::manifest::ManifestIndex`], then call
//! [`parse_safari`].
//!
//! `History.db` (at `HomeDomain/Library/Safari/History.db`) is a plain SQLite DB:
//! `history_items` holds the URL + total visit count, `history_visits` holds one
//! row per visit (with the page title and a timestamp). We join them to emit one
//! cache row per visit.
//!
//! Since iOS 17 Safari supports **profiles**, each with its own `History.db`
//! under `Library/Safari/Profiles/<name>/`. Every one is parsed, and the profile
//! name is carried on the row so the default history and a work/personal profile
//! stay distinguishable rather than silently merging.
//!
//! provenance: reference (own implementation) from the reverse-engineered Safari
//! `History.db` schema; the iOS 26 timestamp encoding and the redirect/origin
//! columns cross-checked against iLEAPP's `safariHistory` artifact.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Core Data / CFAbsoluteTime epoch (2001-01-01 UTC) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

/// The profile name recorded for the main (non-profile) `History.db`.
pub const DEFAULT_PROFILE: &str = "Default";

/// Convert a `History.db` timestamp to Unix seconds, tolerating both encodings
/// Safari has used.
///
/// Through iOS 18 `visit_time` is Apple-absolute (CFAbsoluteTime, seconds since
/// 2001-01-01); on iOS 26 it can instead already be a Unix timestamp. Nothing in
/// the row says which, so the magnitude decides: a Cocoa value only exceeds
/// [`MAC_EPOCH`] for dates past **2032-01-14**, while a Unix value only falls
/// below it for dates before 2001. Real browsing history sits between those, so
/// the two ranges do not overlap and the test is unambiguous.
///
/// This heuristic expires in 2032, when genuine Cocoa timestamps start exceeding
/// the threshold and would be misread as Unix. By then iOS 26's encoding will be
/// the only one in the field and this function should collapse to the identity.
fn to_unix(t: f64) -> i64 {
    if t > MAC_EPOCH as f64 {
        t as i64
    } else {
        (t + MAC_EPOCH as f64) as i64
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Whether `table` has `column`.
///
/// Via `PRAGMA table_info`, deliberately: probing with `SELECT "col" FROM t`
/// does *not* work, because SQLite's double-quoted-string misfeature falls back
/// to treating an unresolvable `"col"` as the string literal `'col'` — so the
/// statement prepares fine and every column looks present.
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")) else {
        return false;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return false;
    };
    while let Ok(Some(r)) = rows.next() {
        if r.get::<_, String>(1).is_ok_and(|n| n == column) {
            return true;
        }
    }
    false
}

/// Read a URL column that may be stored as TEXT *or* BLOB.
///
/// SQLite columns are dynamically typed, and a real `History.db` does turn up
/// with `history_items.url` holding a BLOB. `get::<String>` returns
/// `InvalidColumnType` for that row — and because the error propagates out of
/// the row loop it aborts the whole profile, so ONE oddly-stored row cost the
/// entire Safari history ("Invalid column type Blob at index: 0, name: url",
/// #343). A URL is text whichever way it was written; decode it, and skip only
/// the row that genuinely isn't valid UTF-8.
fn url_text(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<String>> {
    Ok(match r.get_ref(i)? {
        rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
            std::str::from_utf8(t).ok().map(str::to_owned)
        }
        _ => None,
    })
}

/// Visit id → URL, so `redirect_source` / `redirect_destination` (which store
/// visit ids) can be shown as the URLs an analyst actually wants to read. Ids
/// are only unique within one database, so this map is rebuilt per file.
fn redirect_urls(src: &Connection) -> Result<HashMap<i64, String>> {
    let mut stmt = src.prepare(
        "SELECT v.id, i.url FROM history_visits v
         JOIN history_items i ON i.id = v.history_item
         WHERE i.url IS NOT NULL",
    )?;
    let mut map = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        if let Some(url) = url_text(r, 1)? {
            map.insert(r.get::<_, i64>(0)?, url);
        }
    }
    Ok(map)
}

/// Parse a Safari `History.db` into the cache `safari_history` table (one row per
/// visit). With `replace = true` the table is cleared first, in the same
/// transaction as the re-insert, so a partial re-import is atomic.
///
/// `profile` names the Safari profile this database belongs to — [`DEFAULT_PROFILE`]
/// for the main history, otherwise the directory name under `Safari/Profiles/`.
pub fn parse_safari(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
    profile: &str,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&src, "history_items")? || !table_exists(&src, "history_visits")? {
        return Err(crate::Error::Parse(
            "History.db is not a recognized Safari schema".into(),
        ));
    }

    // `origin` and the redirect columns are absent on old schemas (pre-iOS 13);
    // select them only when present so an old database still parses rather than
    // failing whole. NULL stands in for "this schema didn't record it".
    let has_origin = column_exists(&src, "history_visits", "origin");
    let has_redirects = column_exists(&src, "history_visits", "redirect_source")
        && column_exists(&src, "history_visits", "redirect_destination");
    let origin_sql = if has_origin { "v.origin" } else { "NULL" };
    let (rs_sql, rd_sql) = if has_redirects {
        ("v.redirect_source", "v.redirect_destination")
    } else {
        ("NULL", "NULL")
    };

    let redirects = if has_redirects {
        redirect_urls(&src)?
    } else {
        HashMap::new()
    };

    // One row per visit: the item's URL + total visit_count, the visit's title,
    // time, sync origin and redirect chain.
    let mut stmt = src.prepare(&format!(
        "SELECT i.url, v.title, v.visit_time, i.visit_count,
                {origin_sql}, {rs_sql}, {rd_sql}
         FROM history_visits v
         JOIN history_items i ON i.id = v.history_item
         WHERE i.url IS NOT NULL
         ORDER BY v.visit_time DESC",
    ))?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM safari_history", [])?;
        // The searches recovered from these URLs are derived from the rows being
        // replaced, so they go with them. Typed searches come from a different
        // file and survive.
        tx.execute("DELETE FROM safari_searches WHERE source = 'visited'", [])?;
    }
    let mut inserted: usize = 0;
    let mut searches: usize = 0;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let Some(url) = url_text(r, 0)? else {
            continue;
        };
        let title: Option<String> = r
            .get::<_, Option<String>>(1)?
            .filter(|s| !s.trim().is_empty());
        let visited_at = r
            .get::<_, Option<f64>>(2)?
            .filter(|t| *t > 0.0)
            .map(to_unix);
        let visit_count: Option<i64> = r.get(3)?;
        // origin: 0 = this device, 1 = an iCloud-synced device. Anything else is
        // unknown rather than "local", so it stays false but is not asserted.
        let synced = r.get::<_, Option<i64>>(4)?.is_some_and(|o| o == 1);
        let redirect_source = r
            .get::<_, Option<i64>>(5)?
            .and_then(|id| redirects.get(&id).cloned());
        let redirect_destination = r
            .get::<_, Option<i64>>(6)?
            .and_then(|id| redirects.get(&id).cloned());
        tx.execute(
            "INSERT INTO safari_history
                (url, title, visited_at, visit_count, deleted,
                 profile, synced, redirect_source, redirect_destination)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                url,
                title,
                visited_at,
                visit_count,
                profile,
                synced as i64,
                redirect_source,
                redirect_destination
            ],
        )?;
        inserted += 1;

        // A search-engine result page is an ordinary history row whose URL
        // carries the query; nothing stores the term as text, so recover it here
        // while the URL is in hand.
        if let Some(s) = crate::parsers::safari_search::search_in_url(&url) {
            crate::parsers::safari_search::insert_search(
                &tx,
                &s.term,
                visited_at,
                "visited",
                Some(&s.engine),
                Some(&url),
                Some(profile),
            )?;
            searches += 1;
        }
    }

    // Deleted-history tombstones: URLs Safari recorded as removed from history.
    // Surface them flagged (deleted = 1), keyed by their deletion time, so an
    // analyst sees what was browsed-then-cleared. Guarded — the table is optional.
    if table_exists(&src, "history_tombstones")? {
        let mut tstmt =
            src.prepare("SELECT url, end_time FROM history_tombstones WHERE url IS NOT NULL")?;
        let mut trows = tstmt.query([])?;
        while let Some(r) = trows.next()? {
            let Some(url) = url_text(r, 0)? else {
                continue;
            };
            let deleted_at = r
                .get::<_, Option<f64>>(1)?
                .filter(|t| *t > 0.0)
                .map(to_unix);
            tx.execute(
                "INSERT INTO safari_history
                    (url, title, visited_at, visit_count, deleted,
                     profile, synced, redirect_source, redirect_destination)
                 VALUES (?1, NULL, ?2, NULL, 1, ?3, 0, NULL, NULL)",
                rusqlite::params![url, deleted_at, profile],
            )?;
            inserted += 1;
        }
    }

    tx.commit()?;
    // Count only committed rows — a mid-loop error rolls back, adding nothing.
    report.safari_visits += inserted;
    report.safari_searches += searches;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A History.db where one `url` is stored as a BLOB — which real ones are.
    fn make_blob_url_history_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("HistoryBlob.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
             CREATE TABLE history_visits (id INTEGER PRIMARY KEY, history_item INTEGER,
                 title TEXT, visit_time REAL, origin INTEGER,
                 redirect_source INTEGER, redirect_destination INTEGER);
             -- BLOB first, so a row-loop abort takes the TEXT rows down with it.
             INSERT INTO history_items (id, url, visit_count)
                VALUES (1, CAST('https://blob.example' AS BLOB), 2);
             INSERT INTO history_items (id, url, visit_count)
                VALUES (2, 'https://text.example', 1);
             INSERT INTO history_visits (id, history_item, title, visit_time, origin)
                VALUES (10, 1, 'Blob', 721692800.0, 0);
             INSERT INTO history_visits (id, history_item, title, visit_time, origin)
                VALUES (11, 2, 'Text', 721692700.0, 0);",
        )
        .unwrap();
        db
    }

    /// One BLOB-stored URL used to abort the entire profile, so a whole
    /// device's Safari history vanished behind "Invalid column type Blob at
    /// index: 0, name: url" (#343). A URL is text however it was written.
    #[test]
    fn a_blob_stored_url_does_not_take_the_whole_history_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_blob_url_history_db(dir.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_safari(&db, &cache, &mut report, false, DEFAULT_PROFILE).unwrap();

        let conn = cache.conn();
        let mut stmt = conn
            .prepare("SELECT url FROM safari_history ORDER BY url")
            .unwrap();
        let urls: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            urls,
            vec![
                "https://blob.example".to_string(),
                "https://text.example".to_string()
            ],
            "both the blob-stored and the text-stored URL should survive"
        );
    }

    fn make_history_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("History.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
             CREATE TABLE history_visits (id INTEGER PRIMARY KEY, history_item INTEGER,
                 title TEXT, visit_time REAL, origin INTEGER,
                 redirect_source INTEGER, redirect_destination INTEGER);
             INSERT INTO history_items (id, url, visit_count) VALUES (1, 'https://example.com', 3);
             -- Two visits of the same item; Mac-time 721692800 = unix 1_700_000_000.
             INSERT INTO history_visits (id, history_item, title, visit_time, origin)
                VALUES (10, 1, 'Example Domain', 721692800.0, 0);
             INSERT INTO history_visits (id, history_item, title, visit_time, origin)
                VALUES (11, 1, 'Example Domain', 721692500.0, 1);
             -- An item with no url is ignored.
             INSERT INTO history_items (id, url, visit_count) VALUES (2, NULL, 1);
             INSERT INTO history_visits (id, history_item, title, visit_time)
                VALUES (12, 2, NULL, 721692400.0);
             -- A deleted-history tombstone.
             CREATE TABLE history_tombstones (id INTEGER PRIMARY KEY, start_time REAL, end_time REAL, url TEXT, generation INTEGER);
             INSERT INTO history_tombstones (id, start_time, end_time, url)
                VALUES (1, 721692000.0, 721692000.0, 'https://deleted.example');",
        )
        .unwrap();
        db
    }

    fn visited_at_for(cache: &CacheDb, url: &str) -> i64 {
        cache
            .conn()
            .query_row(
                "SELECT visited_at FROM safari_history WHERE url = ?1",
                [url],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn parses_safari_visits() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_history_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        parse_safari(&db, &cache, &mut report, false, DEFAULT_PROFILE).unwrap();
        assert_eq!(
            report.safari_visits, 3,
            "one row per visit (url-less skipped) + one tombstone"
        );

        // The tombstone is stored flagged deleted, with no visit count.
        let (deleted_count, tomb_deleted): (i64, i64) = cache
            .conn()
            .query_row(
                "SELECT COUNT(*), MAX(deleted) FROM safari_history WHERE url = 'https://deleted.example'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(deleted_count, 1);
        assert_eq!(tomb_deleted, 1, "tombstone flagged deleted");

        let c = cache.conn();
        let (url, title, visited, count): (String, String, i64, i64) = c
            .query_row(
                "SELECT url, title, visited_at, visit_count
                 FROM safari_history ORDER BY visited_at DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(url, "https://example.com");
        assert_eq!(title, "Example Domain");
        assert_eq!(visited, 1_700_000_000);
        assert_eq!(count, 3);
    }

    /// origin = 1 marks a visit that happened on another iCloud-synced device,
    /// not this one — an attribution difference, so it must not read as local.
    #[test]
    fn records_icloud_synced_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_history_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        parse_safari(
            &db,
            &cache,
            &mut ImportReport::default(),
            false,
            DEFAULT_PROFILE,
        )
        .unwrap();

        let synced: Vec<i64> = cache
            .conn()
            .prepare("SELECT synced FROM safari_history WHERE deleted = 0 ORDER BY visited_at DESC")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(synced, vec![0, 1], "origin 0 = local, origin 1 = synced");
    }

    /// The profile name rides on every row, including tombstones, so a work and a
    /// personal profile stay distinguishable instead of merging into one list.
    #[test]
    fn carries_the_profile_name() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_history_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        parse_safari(&db, &cache, &mut ImportReport::default(), false, "Work").unwrap();

        let (n, distinct): (i64, i64) = cache
            .conn()
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT profile) FROM safari_history WHERE profile = 'Work'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n, 3, "visits and tombstones both carry the profile");
        assert_eq!(distinct, 1);
    }

    /// Redirect columns hold visit ids, which are meaningless to a reader; they
    /// are resolved to the URLs those visits landed on.
    #[test]
    fn resolves_redirects_to_urls() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("History.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
             CREATE TABLE history_visits (id INTEGER PRIMARY KEY, history_item INTEGER,
                 title TEXT, visit_time REAL, origin INTEGER,
                 redirect_source INTEGER, redirect_destination INTEGER);
             INSERT INTO history_items VALUES (1, 'https://t.co/abc', 1);
             INSERT INTO history_items VALUES (2, 'https://example.com/landing', 1);
             INSERT INTO history_visits VALUES (10, 1, 'shortlink', 721692800.0, 0, NULL, 11);
             INSERT INTO history_visits VALUES (11, 2, 'Landing', 721692801.0, 0, 10, NULL);",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        parse_safari(
            &db,
            &cache,
            &mut ImportReport::default(),
            false,
            DEFAULT_PROFILE,
        )
        .unwrap();

        let dest: String = cache
            .conn()
            .query_row(
                "SELECT redirect_destination FROM safari_history WHERE url = 'https://t.co/abc'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dest, "https://example.com/landing");
        let src: String = cache
            .conn()
            .query_row(
                "SELECT redirect_source FROM safari_history WHERE url LIKE '%landing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "https://t.co/abc");
    }

    /// iOS 26 writes `visit_time` as a Unix timestamp instead of Apple-absolute.
    /// Adding the 2001 epoch to it unconditionally — what we used to do — dated
    /// every visit ~31 years into the future.
    #[test]
    fn reads_ios26_unix_timestamps_without_double_offsetting() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("History.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
             CREATE TABLE history_visits (id INTEGER PRIMARY KEY, history_item INTEGER,
                 title TEXT, visit_time REAL, origin INTEGER,
                 redirect_source INTEGER, redirect_destination INTEGER);
             CREATE TABLE history_tombstones (id INTEGER PRIMARY KEY, start_time REAL,
                 end_time REAL, url TEXT, generation INTEGER);
             INSERT INTO history_items VALUES (1, 'https://example.com', 1);
             -- Already Unix seconds, as iOS 26 stores them.
             INSERT INTO history_visits VALUES (10, 1, 'Example', 1700000000.0, 0, NULL, NULL);
             INSERT INTO history_tombstones (id, end_time, url)
                VALUES (1, 1700000000.0, 'https://deleted.example');",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        parse_safari(
            &db,
            &cache,
            &mut ImportReport::default(),
            false,
            DEFAULT_PROFILE,
        )
        .unwrap();

        assert_eq!(
            visited_at_for(&cache, "https://example.com"),
            1_700_000_000,
            "a Unix visit_time must be stored as-is, not offset again"
        );
        assert_eq!(
            visited_at_for(&cache, "https://deleted.example"),
            1_700_000_000,
            "tombstones use the same encoding as visits"
        );
    }

    /// Both encodings must round-trip, so an iOS 18 backup keeps working while an
    /// iOS 26 one starts working.
    #[test]
    fn to_unix_handles_both_encodings() {
        assert_eq!(to_unix(721_692_800.0), 1_700_000_000, "Cocoa → unix");
        assert_eq!(to_unix(1_700_000_000.0), 1_700_000_000, "unix → unchanged");
    }

    /// A pre-iOS 13 schema has neither `origin` nor the redirect columns. It must
    /// still parse — losing those fields, not the history.
    #[test]
    fn parses_a_schema_without_origin_or_redirects() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("History.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
             CREATE TABLE history_visits (id INTEGER PRIMARY KEY, history_item INTEGER,
                 title TEXT, visit_time REAL);
             INSERT INTO history_items VALUES (1, 'https://old.example', 2);
             INSERT INTO history_visits VALUES (10, 1, 'Old', 721692800.0);",
        )
        .unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_safari(&db, &cache, &mut report, false, DEFAULT_PROFILE).unwrap();

        assert_eq!(report.safari_visits, 1);
        let (synced, rs): (i64, Option<String>) = cache
            .conn()
            .query_row(
                "SELECT synced, redirect_source FROM safari_history",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(synced, 0);
        assert_eq!(rs, None, "absent columns read as unknown, not fabricated");
    }
}
