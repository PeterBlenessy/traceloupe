//! Native Safari bookmarks / reading-list / open-tabs parser (Phase 2).
//!
//! Both `Bookmarks.db` (bookmarks + reading list) and `SafariTabs.db` (open tabs)
//! use the same `bookmarks` table: rows form a tree where `type` 1 is a folder and
//! `type` 0 is a leaf (a bookmark, reading-list item, or tab, all carrying a URL).
//! We load the small tree, classify each leaf, and write it to the cache
//! `safari_bookmarks` table with a `kind` the UI can filter on.
//!
//! - **Reading list**: leaves under the `special_id = 3` folder. Their added/
//!   viewed dates and preview text live in the `extra_attributes` binary plist
//!   under `com.apple.ReadingList`.
//! - **Bookmarks**: the remaining leaves, excluding the `special_id = 4`
//!   web-filter allowlist (parental-control seed sites, not user bookmarks).
//! - **Tabs** (`SafariTabs.db`): every leaf; its tab group is the top-level
//!   folder under Root (Local / Private / pinned / recentlyClosed / named group).
//!
//! provenance: reference (own implementation) from the real `Bookmarks.db` /
//! `SafariTabs.db` schema on a device backup.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

const SPECIAL_READING_LIST: i64 = 3;
const SPECIAL_WEB_FILTER: i64 = 4;

/// One row of the `bookmarks` tree, loaded into memory.
struct Node {
    parent: Option<i64>,
    is_folder: bool,
    special_id: i64,
    title: Option<String>,
    url: Option<String>,
    order_index: i64,
    last_modified: Option<f64>,
    extra: Option<Vec<u8>>,
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Load the whole `bookmarks` table (these DBs are small — tens of rows).
fn load_nodes(conn: &Connection) -> Result<HashMap<i64, Node>> {
    let mut stmt = conn.prepare(
        "SELECT id, parent, type, special_id, title, url, order_index,
                last_modified, extra_attributes
         FROM bookmarks WHERE deleted = 0",
    )?;
    let mut map = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        // `id` keys the tree; a row with a NULL id can't be linked, so skip it
        // rather than abort the whole load. A NULL `type` is simply not a folder.
        let Some(id) = r.get::<_, Option<i64>>(0)? else {
            continue;
        };
        map.insert(
            id,
            Node {
                parent: r.get(1)?,
                is_folder: r.get::<_, Option<i64>>(2)?.unwrap_or(0) == 1,
                special_id: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                title: r.get::<_, Option<String>>(4)?.filter(|s| !s.is_empty()),
                url: r.get::<_, Option<String>>(5)?.filter(|s| !s.is_empty()),
                order_index: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                last_modified: r.get(7)?,
                extra: r.get::<_, Option<Vec<u8>>>(8)?,
            },
        );
    }
    Ok(map)
}

/// The result of walking a leaf's ancestors up to (not including) the Root.
#[derive(Default)]
struct Ancestry {
    hits_reading_list: bool,
    hits_web_filter: bool,
    /// The top-level folder under Root — the tab group / top bookmark folder.
    top_group: Option<i64>,
    /// The leaf's immediate parent folder.
    parent: Option<i64>,
}

/// Walk from a leaf's `parent` up to the Root (a node with no parent), noting the
/// special folders passed through and the top-level group folder.
fn ancestry(nodes: &HashMap<i64, Node>, parent: Option<i64>) -> Ancestry {
    let mut a = Ancestry {
        parent,
        ..Default::default()
    };
    let mut cur = parent;
    let mut guard = 0;
    while let Some(id) = cur {
        if guard > 64 {
            break; // cycle guard
        }
        guard += 1;
        let node = match nodes.get(&id) {
            Some(n) => n,
            None => break,
        };
        if node.special_id == SPECIAL_READING_LIST {
            a.hits_reading_list = true;
        }
        if node.special_id == SPECIAL_WEB_FILTER {
            a.hits_web_filter = true;
        }
        // `id` is the top group when its parent is the Root (a parent-less node)
        // or is missing entirely.
        let parent_is_root = match node.parent {
            None => true,
            Some(p) => nodes.get(&p).map(|pn| pn.parent.is_none()).unwrap_or(true),
        };
        if parent_is_root {
            a.top_group = Some(id);
            break;
        }
        cur = node.parent;
    }
    a
}

/// plist `Date` → Unix seconds.
fn plist_date_unix(v: &plist::Value) -> Option<i64> {
    let d = v.as_date()?;
    let st: SystemTime = d.into();
    st.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Pull (date_added, date_viewed, preview_text) from a reading-list item's
/// `extra_attributes` binary plist. Any failure degrades to None, not an error.
fn reading_list_meta(blob: &[u8]) -> (Option<i64>, Option<i64>, Option<String>) {
    let Ok(val) = plist::Value::from_reader(Cursor::new(blob)) else {
        return (None, None, None);
    };
    let dict = val
        .as_dictionary()
        .and_then(|d| d.get("com.apple.ReadingList"))
        .and_then(|v| v.as_dictionary());
    let Some(dict) = dict else {
        return (None, None, None);
    };
    let added = dict.get("DateAdded").and_then(plist_date_unix);
    let viewed = dict.get("DateLastViewed").and_then(plist_date_unix);
    let preview = dict
        .get("PreviewText")
        .and_then(|v| v.as_string())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    (added, viewed, preview)
}

/// A tab group's display name, tidying Safari's internal folder names.
fn tab_group_name(title: Option<&str>) -> Option<String> {
    let t = title?.trim();
    Some(match t {
        "pinned" => "Pinned".into(),
        "privatePinned" => "Pinned (Private)".into(),
        "recentlyClosed" => "Recently Closed".into(),
        "" => return None,
        other => other.to_string(),
    })
}

/// Parse `Bookmarks.db` into the cache `safari_bookmarks` table — bookmarks and
/// reading-list items. With `replace`, clears those two kinds first (leaving tabs
/// untouched) in the same transaction.
pub fn parse_safari_bookmarks(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&src, "bookmarks")? {
        return Err(crate::Error::Parse(
            "Bookmarks.db is not a recognized Safari schema".into(),
        ));
    }
    let nodes = load_nodes(&src)?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute(
            "DELETE FROM safari_bookmarks WHERE kind IN ('bookmark', 'reading_list')",
            [],
        )?;
    }
    let mut inserted = 0usize;
    for node in nodes.values() {
        if node.is_folder {
            continue;
        }
        let Some(url) = &node.url else { continue };
        let anc = ancestry(&nodes, node.parent);
        if anc.hits_web_filter {
            continue; // parental-control allowlist, not a user bookmark
        }
        let (kind, folder, date_added, date_viewed, preview) = if anc.hits_reading_list {
            let (added, viewed, preview) = node
                .extra
                .as_deref()
                .map(reading_list_meta)
                .unwrap_or((None, None, None));
            (
                "reading_list",
                None,
                added.or_else(|| node.last_modified.map(|t| t as i64)),
                viewed,
                preview,
            )
        } else {
            // A bookmark: show the containing folder, unless it's the Bookmarks
            // Bar (special_id 1) or the Root itself (parent-less) — both read as
            // "no folder".
            let folder = anc
                .parent
                .and_then(|p| nodes.get(&p))
                .filter(|p| p.special_id != 1 && p.parent.is_some())
                .and_then(|p| p.title.clone());
            (
                "bookmark",
                folder,
                node.last_modified.map(|t| t as i64),
                None,
                None,
            )
        };
        tx.execute(
            "INSERT INTO safari_bookmarks
                (kind, title, url, folder, date_added, date_viewed, preview_text, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                kind,
                node.title,
                url,
                folder,
                date_added,
                date_viewed,
                preview,
                node.order_index
            ],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.safari_bookmarks += inserted;
    Ok(())
}

/// Parse `SafariTabs.db` into the cache `safari_bookmarks` table as `kind = 'tab'`
/// rows, each tagged with its tab group. With `replace`, clears existing tabs
/// first (leaving bookmarks/reading list untouched).
pub fn parse_safari_tabs(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&src, "bookmarks")? {
        return Err(crate::Error::Parse(
            "SafariTabs.db is not a recognized Safari schema".into(),
        ));
    }
    let nodes = load_nodes(&src)?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM safari_bookmarks WHERE kind = 'tab'", [])?;
    }
    let mut inserted = 0usize;
    for node in nodes.values() {
        if node.is_folder {
            continue;
        }
        let Some(url) = &node.url else { continue };
        let anc = ancestry(&nodes, node.parent);
        let group = anc
            .top_group
            .and_then(|g| nodes.get(&g))
            .and_then(|g| tab_group_name(g.title.as_deref()));
        tx.execute(
            "INSERT INTO safari_bookmarks
                (kind, title, url, folder, date_added, date_viewed, preview_text, position)
             VALUES ('tab', ?1, ?2, ?3, ?4, NULL, NULL, ?5)",
            rusqlite::params![
                node.title,
                url,
                group,
                node.last_modified.map(|t| t as i64),
                node.order_index
            ],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.safari_bookmarks += inserted;
    Ok(())
}

/// Parse `BrowserState.db` — the device's **local** open tabs — into
/// `safari_bookmarks` as `kind = 'tab'`. This is richer and more complete than
/// the iCloud-synced `SafariTabs.db` (per-tab last-viewed time, private-browsing
/// flag), so when it's present it **replaces** any tabs already parsed. Runs
/// after `parse_safari_tabs` in the import; best-effort on an unknown schema.
pub fn parse_browser_tabs(
    db_path: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
) -> Result<()> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&src, "tabs")? {
        // Not a BrowserState schema — leave whatever SafariTabs.db provided.
        return Ok(());
    }
    // `last_viewed_time` is CFAbsoluteTime (seconds since 2001). Some rows carry a
    // corrupt far-future sentinel, so accept only plausible values (< ~2032).
    const MAC_EPOCH: f64 = 978_307_200.0;
    // `addr` prefers a non-empty user_visible_url, falls back to url, and is NULL
    // when both are empty/absent (outer NULLIF) so the WHERE skips url-less rows.
    let mut stmt = src.prepare(
        "SELECT title, NULLIF(COALESCE(NULLIF(user_visible_url, ''), url), '') AS addr,
                order_index, last_viewed_time, private_browsing
         FROM tabs
         WHERE NULLIF(COALESCE(NULLIF(user_visible_url, ''), url), '') IS NOT NULL
         ORDER BY private_browsing, order_index",
    )?;

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    // BrowserState is authoritative for open tabs — always clear existing tab rows
    // first so the local and iCloud sources can't stack. Discount the rows we
    // remove from the running report so a re-parse doesn't double-count tabs.
    let removed = tx.execute("DELETE FROM safari_bookmarks WHERE kind = 'tab'", [])?;
    report.safari_bookmarks = report.safari_bookmarks.saturating_sub(removed);
    let mut inserted = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let title: Option<String> = r.get(0)?;
        let url: String = r.get(1)?;
        let position: Option<i64> = r.get(2)?;
        let date_viewed = r
            .get::<_, Option<f64>>(3)?
            .filter(|t| *t > 0.0 && *t < 1_000_000_000.0)
            .map(|t| (t + MAC_EPOCH) as i64);
        let private = r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0;
        tx.execute(
            "INSERT INTO safari_bookmarks
                (kind, title, url, folder, date_added, date_viewed, preview_text, position, private)
             VALUES ('tab', ?1, ?2, NULL, NULL, ?3, NULL, ?4, ?5)",
            rusqlite::params![title, url, date_viewed, position, private as i64],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    report.safari_bookmarks += inserted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `bookmarks`-format DB mirroring the real Safari layout: Root,
    /// Bookmarks Bar, a Reading List folder (special_id 3) with one item carrying
    /// a `com.apple.ReadingList` plist, a web-filter folder (special_id 4) to be
    /// excluded, and a couple of bookmarks.
    fn make_bookmarks_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("Bookmarks.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE bookmarks (id INTEGER PRIMARY KEY, special_id INTEGER DEFAULT 0,
                 parent INTEGER, type INTEGER, title TEXT, url TEXT, num_children INTEGER,
                 order_index INTEGER, last_modified REAL, deleted INTEGER DEFAULT 0,
                 extra_attributes BLOB);
             INSERT INTO bookmarks (id,special_id,parent,type,title,order_index) VALUES (0,0,NULL,1,'Root',0);
             INSERT INTO bookmarks (id,special_id,parent,type,title,order_index) VALUES (1,1,0,1,'BookmarksBar',0);
             INSERT INTO bookmarks (id,special_id,parent,type,title,order_index) VALUES (2,3,0,1,'com.apple.ReadingList',1);
             INSERT INTO bookmarks (id,special_id,parent,type,title,order_index) VALUES (3,4,0,1,'com.apple.WebFilterWhiteList',2);
             INSERT INTO bookmarks (id,parent,type,title,url,order_index,last_modified) VALUES (5,1,0,'Apple','https://apple.com',0,1700000000.0);
             INSERT INTO bookmarks (id,parent,type,title,url,order_index) VALUES (6,3,0,'KidSite','https://disney.com',0);",
        )
        .unwrap();
        // Reading-list item with a com.apple.ReadingList plist (DateAdded + preview).
        let plist = b"<?xml version=\"1.0\"?><!DOCTYPE plist><plist version=\"1.0\"><dict>\
            <key>com.apple.ReadingList</key><dict>\
            <key>DateAdded</key><date>2023-01-01T00:00:00Z</date>\
            <key>PreviewText</key><string>a preview</string></dict></dict></plist>";
        conn.execute(
            "INSERT INTO bookmarks (id,parent,type,title,url,order_index,extra_attributes)
             VALUES (7,2,0,'Read me','https://example.com',0,?1)",
            rusqlite::params![&plist[..]],
        )
        .unwrap();
        db
    }

    #[test]
    fn classifies_bookmarks_and_reading_list() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_bookmarks_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_safari_bookmarks(&db, &cache, &mut report, false).unwrap();

        // One bookmark (Apple) + one reading-list item; the web-filter kid site is
        // excluded.
        assert_eq!(report.safari_bookmarks, 2);
        let conn = cache.conn();
        let bookmarks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM safari_bookmarks WHERE kind='bookmark'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bookmarks, 1, "Apple only; KidSite excluded");

        let (title, preview, added): (String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT title, preview_text, date_added FROM safari_bookmarks
                 WHERE kind='reading_list'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "Read me");
        assert_eq!(preview.as_deref(), Some("a preview"));
        // 2023-01-01T00:00:00Z.
        assert_eq!(added, Some(1_672_531_200));
    }

    #[test]
    fn browser_state_replaces_tabs_with_local_open_tabs() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("BrowserState.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE tabs (id INTEGER PRIMARY KEY, title TEXT, url TEXT,
                 user_visible_url TEXT, order_index INTEGER, last_viewed_time REAL,
                 private_browsing INTEGER, browser_window_uuid TEXT);
             -- a normal tab with a real last-viewed (721692800 CFAbsolute → 1_700_000_000 unix)
             INSERT INTO tabs VALUES (1,'Example','https://example.com/x','https://example.com',0,721692800.0,0,'w1');
             -- a private tab, no user_visible_url (falls back to url), corrupt far-future last-viewed → dropped
             INSERT INTO tabs VALUES (2,'Secret',NULL,'https://private.example',1,1783839264.0,1,'w2');
             -- a row with no url at all → skipped
             INSERT INTO tabs VALUES (3,'Blank',NULL,'',2,0,0,'w1');",
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        // A stale iCloud tab already in the table — must be replaced by BrowserState.
        cache
            .conn()
            .execute(
                "INSERT INTO safari_bookmarks (kind, title, url) VALUES ('tab','Old iCloud tab','https://old.example')",
                [],
            )
            .unwrap();

        let mut report = ImportReport::default();
        parse_browser_tabs(&db, &cache, &mut report).unwrap();
        assert_eq!(
            report.safari_bookmarks, 2,
            "two valid tabs; the url-less row skipped"
        );

        let c = cache.conn();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM safari_bookmarks WHERE kind='tab'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "the stale iCloud tab was replaced");

        // Normal tab: user_visible_url preferred, real last-viewed kept.
        let (url, viewed, private): (String, Option<i64>, i64) = c
            .query_row(
                "SELECT url, date_viewed, private FROM safari_bookmarks WHERE title='Example'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            url, "https://example.com",
            "user_visible_url preferred over url"
        );
        assert_eq!(viewed, Some(1_700_000_000));
        assert_eq!(private, 0);

        // Private tab: flagged, url fallback, corrupt future date dropped.
        let (url2, viewed2, private2): (String, Option<i64>, i64) = c
            .query_row(
                "SELECT url, date_viewed, private FROM safari_bookmarks WHERE title='Secret'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(url2, "https://private.example");
        assert_eq!(viewed2, None, "far-future sentinel dropped");
        assert_eq!(private2, 1);
    }
}
