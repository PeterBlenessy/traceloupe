//! Native Photos.sqlite metadata parser (Phase 2): enriches already-imported
//! camera-roll media with what iOS's photo library knows about each asset — the
//! **people** detected in it (face recognition), the **capture date**, **GPS
//! location**, and whether it's a **favorite** — so the Gallery can search,
//! filter, and show them.
//!
//! People come from `ZASSET ← ZDETECTEDFACE.ZASSETFORFACE`,
//! `ZDETECTEDFACE.ZPERSONFORFACE → ZPERSON` (only *named* persons kept). The rest
//! is columns on `ZASSET`. An asset's on-disk path is `Media/<ZDIRECTORY>/
//! <ZFILENAME>`, which is exactly our `media_items` `relative_path` (minus the
//! `Media/` prefix), so we match on that suffix and write the metadata onto the
//! media row.
//!
//! provenance: reference (own implementation) from a real `Photos.sqlite`
//! (iOS 17-era Core Data schema) on a device backup.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::Result;

/// Core Data / CFAbsoluteTime epoch (2001-01-01 UTC) → Unix seconds.
const MAC_EPOCH: i64 = 978_307_200;

/// Per-asset metadata pulled from `ZASSET`.
#[derive(Default)]
struct AssetMeta {
    taken_at: Option<i64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    favorite: bool,
    persons: Option<String>,
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Read Photos.sqlite metadata (people, capture date, GPS, favorite) and write it
/// onto matching `media_items` rows. Returns the number of media rows updated.
/// Missing tables (an unexpected schema) is an error; no matches is simply zero.
pub fn parse_photos_metadata(db_path: &Path, cache: &CacheDb) -> Result<usize> {
    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for t in ["ZASSET", "ZDETECTEDFACE", "ZPERSON"] {
        if !table_exists(&src, t)? {
            return Err(crate::Error::Parse(format!(
                "Photos.sqlite is not a recognized schema (missing {t})"
            )));
        }
    }

    // asset path suffix ("DCIM/100APPLE/IMG_0058.JPG") -> metadata.
    let mut by_suffix: HashMap<String, AssetMeta> = HashMap::new();

    // Base metadata for every asset: date, GPS, favorite.
    {
        let mut stmt = src.prepare(
            "SELECT ZDIRECTORY, ZFILENAME, ZDATECREATED, ZLATITUDE, ZLONGITUDE, ZFAVORITE
             FROM ZASSET
             WHERE ZDIRECTORY IS NOT NULL AND ZFILENAME IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let dir: String = r.get(0)?;
            let file: String = r.get(1)?;
            let taken_at = r
                .get::<_, Option<f64>>(2)?
                .filter(|t| *t > 0.0)
                .map(|t| t as i64 + MAC_EPOCH);
            let lat: Option<f64> = r.get(3)?;
            let lon: Option<f64> = r.get(4)?;
            // iOS stores -180.0 (or an out-of-range value) when there's no fix.
            let (latitude, longitude) = match (lat, lon) {
                (Some(a), Some(o))
                    if (-90.0..=90.0).contains(&a) && (-180.0..180.0).contains(&o) =>
                {
                    (Some(a), Some(o))
                }
                _ => (None, None),
            };
            let favorite = r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0;
            by_suffix.insert(
                format!("{dir}/{file}"),
                AssetMeta {
                    taken_at,
                    latitude,
                    longitude,
                    favorite,
                    persons: None,
                },
            );
        }
    }

    // Named people per asset (a photo can have several).
    {
        let mut names: HashMap<String, BTreeSet<String>> = HashMap::new();
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
            let name: String = r.get::<_, String>(2)?.trim().to_string();
            if !name.is_empty() {
                names
                    .entry(format!("{dir}/{file}"))
                    .or_default()
                    .insert(name);
            }
        }
        for (suffix, set) in names {
            let joined = set.into_iter().collect::<Vec<_>>().join(", ");
            by_suffix.entry(suffix).or_default().persons = Some(joined);
        }
    }

    if by_suffix.is_empty() {
        return Ok(0);
    }

    // Match against media rows by their path suffix (drop the leading "Media/").
    let conn = cache.conn();
    let updates: Vec<(i64, &AssetMeta)> = {
        let mut mstmt = conn.prepare("SELECT id, relative_path FROM media_items")?;
        let mut mrows = mstmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = mrows.next()? {
            let id: i64 = r.get(0)?;
            let rel: String = r.get(1)?;
            let suffix = rel.strip_prefix("Media/").unwrap_or(&rel);
            if let Some(meta) = by_suffix.get(suffix) {
                out.push((id, meta));
            }
        }
        out
    };

    let tx = conn.unchecked_transaction()?;
    for (id, meta) in &updates {
        // COALESCE the date so a genuine Photos date replaces the camera-roll
        // guess, but a missing one keeps whatever's already there.
        tx.execute(
            "UPDATE media_items
             SET persons = ?1,
                 latitude = ?2,
                 longitude = ?3,
                 is_favorite = ?4,
                 taken_at = COALESCE(?5, taken_at)
             WHERE id = ?6",
            rusqlite::params![
                meta.persons,
                meta.latitude,
                meta.longitude,
                meta.favorite as i64,
                meta.taken_at,
                id
            ],
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
            "CREATE TABLE ZASSET (Z_PK INTEGER PRIMARY KEY, ZDIRECTORY TEXT, ZFILENAME TEXT,
                 ZDATECREATED REAL, ZLATITUDE REAL, ZLONGITUDE REAL, ZFAVORITE INTEGER);
             CREATE TABLE ZPERSON (Z_PK INTEGER PRIMARY KEY, ZFULLNAME TEXT, ZDISPLAYNAME TEXT);
             CREATE TABLE ZDETECTEDFACE (Z_PK INTEGER PRIMARY KEY, ZASSETFORFACE INTEGER, ZPERSONFORFACE INTEGER);
             -- Asset 1: named people, a real date (721692800 Mac = 1_700_000_000 unix),
             -- a GPS fix, and favorited.
             INSERT INTO ZASSET VALUES (1, 'DCIM/100APPLE', 'IMG_0001.HEIC', 721692800.0, 59.33, 18.06, 1);
             -- Asset 2: no named people, no location (-180 sentinel), not favorited.
             INSERT INTO ZASSET VALUES (2, 'DCIM/100APPLE', 'IMG_0002.HEIC', NULL, -180.0, -180.0, 0);
             INSERT INTO ZPERSON VALUES (10, 'Alice', NULL);
             INSERT INTO ZPERSON VALUES (11, NULL, 'Bob');
             INSERT INTO ZPERSON VALUES (12, '', '');  -- unnamed cluster, ignored
             INSERT INTO ZDETECTEDFACE VALUES (100, 1, 10);
             INSERT INTO ZDETECTEDFACE VALUES (101, 1, 11);
             INSERT INTO ZDETECTEDFACE VALUES (102, 2, 12);",
        )
        .unwrap();
        db
    }

    #[test]
    fn enriches_media_with_photos_metadata() {
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

        // Both assets exist in ZASSET, so both media rows are updated.
        let n = parse_photos_metadata(&db, &cache).unwrap();
        assert_eq!(n, 2);

        let conn = cache.conn();
        let (persons, lat, lon, fav, taken): (
            Option<String>,
            Option<f64>,
            Option<f64>,
            i64,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT persons, latitude, longitude, is_favorite, taken_at
                 FROM media_items WHERE relative_path LIKE '%IMG_0001%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(persons.as_deref(), Some("Alice, Bob"));
        assert_eq!(lat, Some(59.33));
        assert_eq!(lon, Some(18.06));
        assert_eq!(fav, 1);
        assert_eq!(taken, Some(1_700_000_000));

        // Asset 2: no people, no location (sentinel dropped), not favorite.
        let (persons2, lat2, fav2): (Option<String>, Option<f64>, i64) = conn
            .query_row(
                "SELECT persons, latitude, is_favorite FROM media_items
                 WHERE relative_path LIKE '%IMG_0002%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(persons2, None);
        assert_eq!(lat2, None);
        assert_eq!(fav2, 0);
    }
}
