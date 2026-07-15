//! Native Photos.sqlite metadata parser (Phase 2): enriches already-imported
//! camera-roll media with the **people** detected in each photo, so the Gallery
//! can search and show face-recognition person tags.
//!
//! The join is `ZASSET ← ZDETECTEDFACE.ZASSETFORFACE`,
//! `ZDETECTEDFACE.ZPERSONFORFACE → ZPERSON`; only *named* persons (a non-empty
//! `ZFULLNAME`/`ZDISPLAYNAME`) are kept. An asset's on-disk path is
//! `Media/<ZDIRECTORY>/<ZFILENAME>`, which is exactly our `media_items`
//! `relative_path` (minus the `Media/` prefix), so we match on that suffix and
//! write the comma-joined names onto the media row.
//!
//! provenance: reference (own implementation) from a real `Photos.sqlite`
//! (iOS 17-era Core Data schema) on a device backup.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::Result;

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Read Photos.sqlite person tags and write them onto matching `media_items`
/// rows (`persons` column). Returns the number of media rows tagged. Missing
/// tables (an unexpected schema) is an error; no matches is simply zero.
pub fn parse_photos_persons(db_path: &Path, cache: &CacheDb) -> Result<usize> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for t in ["ZASSET", "ZDETECTEDFACE", "ZPERSON"] {
        if !table_exists(&src, t)? {
            return Err(crate::Error::Parse(format!(
                "Photos.sqlite is not a recognized schema (missing {t})"
            )));
        }
    }

    // asset path suffix ("DCIM/100APPLE/IMG_0058.JPG") -> set of person names.
    let mut by_suffix: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut stmt = src.prepare(
        "SELECT a.ZDIRECTORY, a.ZFILENAME,
                COALESCE(NULLIF(p.ZFULLNAME, ''), NULLIF(p.ZDISPLAYNAME, '')) AS name
         FROM ZDETECTEDFACE f
         JOIN ZASSET  a ON a.Z_PK = f.ZASSETFORFACE
         JOIN ZPERSON p ON p.Z_PK = f.ZPERSONFORFACE
         WHERE name IS NOT NULL AND a.ZDIRECTORY IS NOT NULL AND a.ZFILENAME IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let dir: String = r.get(0)?;
        let file: String = r.get(1)?;
        let name: String = r.get(2)?;
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        by_suffix
            .entry(format!("{dir}/{file}"))
            .or_default()
            .insert(name);
    }
    if by_suffix.is_empty() {
        return Ok(0);
    }

    // Match against media rows by their path suffix (drop the leading "Media/").
    let conn = cache.conn();
    let updates: Vec<(i64, String)> = {
        let mut mstmt = conn.prepare("SELECT id, relative_path FROM media_items")?;
        let mut mrows = mstmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = mrows.next()? {
            let id: i64 = r.get(0)?;
            let rel: String = r.get(1)?;
            let suffix = rel.strip_prefix("Media/").unwrap_or(&rel);
            if let Some(names) = by_suffix.get(suffix) {
                out.push((id, names.iter().cloned().collect::<Vec<_>>().join(", ")));
            }
        }
        out
    };

    let tx = conn.unchecked_transaction()?;
    for (id, persons) in &updates {
        tx.execute(
            "UPDATE media_items SET persons = ?1 WHERE id = ?2",
            rusqlite::params![persons, id],
        )?;
    }
    tx.commit()?;
    Ok(updates.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_photos_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("Photos.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZASSET (Z_PK INTEGER PRIMARY KEY, ZDIRECTORY TEXT, ZFILENAME TEXT);
             CREATE TABLE ZPERSON (Z_PK INTEGER PRIMARY KEY, ZFULLNAME TEXT, ZDISPLAYNAME TEXT);
             CREATE TABLE ZDETECTEDFACE (Z_PK INTEGER PRIMARY KEY, ZASSETFORFACE INTEGER, ZPERSONFORFACE INTEGER);
             INSERT INTO ZASSET VALUES (1, 'DCIM/100APPLE', 'IMG_0001.HEIC');
             INSERT INTO ZASSET VALUES (2, 'DCIM/100APPLE', 'IMG_0002.HEIC');
             INSERT INTO ZPERSON VALUES (10, 'Alice', NULL);
             INSERT INTO ZPERSON VALUES (11, NULL, 'Bob');
             INSERT INTO ZPERSON VALUES (12, '', '');  -- unnamed cluster, ignored
             -- Asset 1 has Alice + Bob; asset 2 has only an unnamed cluster.
             INSERT INTO ZDETECTEDFACE VALUES (100, 1, 10);
             INSERT INTO ZDETECTEDFACE VALUES (101, 1, 11);
             INSERT INTO ZDETECTEDFACE VALUES (102, 2, 12);",
        )
        .unwrap();
        db
    }

    #[test]
    fn tags_media_with_named_persons() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_photos_db(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        cache
            .conn()
            .execute_batch(
                "INSERT INTO media_items (relative_path, kind) VALUES ('Media/DCIM/100APPLE/IMG_0001.HEIC', 'photo');
                 INSERT INTO media_items (relative_path, kind) VALUES ('Media/DCIM/100APPLE/IMG_0002.HEIC', 'photo');",
            )
            .unwrap();

        let n = parse_photos_persons(&db, &cache).unwrap();
        assert_eq!(n, 1, "only the asset with named persons is tagged");

        let persons: Option<String> = cache
            .conn()
            .query_row(
                "SELECT persons FROM media_items WHERE relative_path LIKE '%IMG_0001%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(persons.as_deref(), Some("Alice, Bob"));

        let none: Option<String> = cache
            .conn()
            .query_row(
                "SELECT persons FROM media_items WHERE relative_path LIKE '%IMG_0002%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(none, None, "unnamed-only asset stays untagged");
    }
}
