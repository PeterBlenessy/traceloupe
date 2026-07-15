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
    /// In the Hidden album (`ZASSET.ZHIDDEN`).
    hidden: bool,
    /// Media subtype we can classify confidently: "screenshot" | "panorama".
    subtype: Option<&'static str>,
    persons: Option<String>,
    /// Moment place/event title (e.g. "Häljarp", "New Year's Day") — searchable.
    location: Option<String>,
    /// User-created album names this photo belongs to, comma-joined.
    albums: Option<String>,
    /// Pixel dimensions and (for video) duration in seconds.
    width: Option<i64>,
    height: Option<i64>,
    duration_s: Option<f64>,
    /// Original file size in bytes (`ZADDITIONALASSETATTRIBUTES.ZORIGINALFILESIZE`).
    file_size: Option<i64>,
    /// Camera "<make> <model>" and lens model (`ZEXTENDEDATTRIBUTES`).
    camera: Option<String>,
    lens: Option<String>,
    /// Formatted exposure summary, e.g. "ISO 100 · ƒ/1.8 · 1/120s · 26 mm".
    exif: Option<String>,
}

/// Combine a camera make + model into one label, avoiding "Apple Apple …".
fn camera_label(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let make = make.map(str::trim).filter(|s| !s.is_empty());
    let model = model.map(str::trim).filter(|s| !s.is_empty());
    match (make, model) {
        (Some(mk), Some(md)) if !md.to_lowercase().starts_with(&mk.to_lowercase()) => {
            Some(format!("{mk} {md}"))
        }
        (_, Some(md)) => Some(md.to_string()),
        (Some(mk), None) => Some(mk.to_string()),
        (None, None) => None,
    }
}

/// Format the EXIF exposure fields into a compact, human-readable summary. The
/// values are already in friendly units: ISO integer, aperture as an f-number,
/// shutter as seconds, focal length in mm.
fn exif_summary(
    iso: Option<i64>,
    aperture: Option<f64>,
    shutter: Option<f64>,
    focal: Option<f64>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(iso) = iso.filter(|v| *v > 0) {
        parts.push(format!("ISO {iso}"));
    }
    if let Some(a) = aperture.filter(|v| *v > 0.0) {
        parts.push(format!("ƒ/{a:.1}"));
    }
    if let Some(s) = shutter.filter(|v| *v > 0.0) {
        parts.push(if s < 1.0 {
            format!("1/{}s", (1.0 / s).round() as i64)
        } else {
            format!("{s:.0}s")
        });
    }
    if let Some(f) = focal.filter(|v| *v > 0.0) {
        parts.push(format!("{f:.0} mm"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Find the album↔asset join table (`Z_<n>ASSETS` with a `Z_<n>ALBUMS` column) —
/// the entity number varies by iOS version, so we discover it rather than hardcode
/// it. Returns (table, album_column, asset_column).
fn album_join(conn: &Connection) -> Result<Option<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'Z_[0-9]*ASSETS'",
    )?;
    let tables: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for t in tables {
        let cols: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let album_col = cols.iter().find(|c| c.ends_with("ALBUMS")).cloned();
        let asset_col = cols.iter().find(|c| c.ends_with("ASSETS")).cloned();
        if let (Some(a), Some(s)) = (album_col, asset_col) {
            return Ok(Some((t, a, s)));
        }
    }
    Ok(None)
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

    // asset path suffix ("DCIM/100APPLE/IMG_0058.JPG") -> metadata, and the
    // asset's Core Data Z_PK -> suffix (album/keyword joins are keyed by PK).
    let mut by_suffix: HashMap<String, AssetMeta> = HashMap::new();
    let mut pk_to_suffix: HashMap<i64, String> = HashMap::new();

    // Base metadata for every asset: date, GPS, favorite, and moment place name.
    {
        let mut stmt = src.prepare(
            "SELECT a.Z_PK, a.ZDIRECTORY, a.ZFILENAME, a.ZDATECREATED,
                    a.ZLATITUDE, a.ZLONGITUDE, a.ZFAVORITE, m.ZTITLE,
                    a.ZWIDTH, a.ZHEIGHT, a.ZDURATION, a.ZHIDDEN,
                    a.ZKINDSUBTYPE, a.ZISDETECTEDSCREENSHOT
             FROM ZASSET a
             LEFT JOIN ZMOMENT m ON m.Z_PK = a.ZMOMENT
             WHERE a.ZDIRECTORY IS NOT NULL AND a.ZFILENAME IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let pk: i64 = r.get(0)?;
            let dir: String = r.get(1)?;
            let file: String = r.get(2)?;
            let taken_at = r
                .get::<_, Option<f64>>(3)?
                .filter(|t| *t > 0.0)
                .map(|t| t as i64 + MAC_EPOCH);
            let lat: Option<f64> = r.get(4)?;
            let lon: Option<f64> = r.get(5)?;
            // iOS stores -180.0 (or an out-of-range value) when there's no fix.
            let (latitude, longitude) = match (lat, lon) {
                (Some(a), Some(o))
                    if (-90.0..=90.0).contains(&a) && (-180.0..180.0).contains(&o) =>
                {
                    (Some(a), Some(o))
                }
                _ => (None, None),
            };
            let favorite = r.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0;
            let location = r
                .get::<_, Option<String>>(7)?
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let width = r.get::<_, Option<i64>>(8)?.filter(|v| *v > 0);
            let height = r.get::<_, Option<i64>>(9)?.filter(|v| *v > 0);
            let duration_s = r.get::<_, Option<f64>>(10)?.filter(|v| *v > 0.0);
            let hidden = r.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0;
            // Classify only confidently: screenshot (two corroborating signals) and
            // panorama. The 100-series video subtype codes are ambiguous → leave null.
            let kind_subtype = r.get::<_, Option<i64>>(12)?.unwrap_or(0);
            let is_screenshot = r.get::<_, Option<i64>>(13)?.unwrap_or(0) != 0;
            let subtype = if is_screenshot || kind_subtype == 10 {
                Some("screenshot")
            } else if kind_subtype == 2 {
                Some("panorama")
            } else {
                None
            };
            let suffix = format!("{dir}/{file}");
            pk_to_suffix.insert(pk, suffix.clone());
            by_suffix.insert(
                suffix,
                AssetMeta {
                    taken_at,
                    latitude,
                    longitude,
                    favorite,
                    hidden,
                    subtype,
                    location,
                    width,
                    height,
                    duration_s,
                    ..Default::default()
                },
            );
        }
    }

    // User-created album names (ZKIND = 2) per asset, via the album↔asset join.
    if let Some((join, album_col, asset_col)) = album_join(&src)? {
        let mut albums: HashMap<String, BTreeSet<String>> = HashMap::new();
        let sql = format!(
            "SELECT j.{asset_col}, g.ZTITLE
             FROM {join} j
             JOIN ZGENERICALBUM g ON g.Z_PK = j.{album_col}
             WHERE g.ZKIND = 2 AND g.ZTITLE IS NOT NULL AND g.ZTITLE <> ''"
        );
        let mut stmt = src.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let pk: i64 = r.get(0)?;
            let title: String = r.get::<_, String>(1)?.trim().to_string();
            if title.is_empty() {
                continue;
            }
            if let Some(suffix) = pk_to_suffix.get(&pk) {
                albums.entry(suffix.clone()).or_default().insert(title);
            }
        }
        for (suffix, set) in albums {
            let joined = set.into_iter().collect::<Vec<_>>().join(", ");
            if let Some(meta) = by_suffix.get_mut(&suffix) {
                meta.albums = Some(joined);
            }
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

    // EXIF (camera/lens/exposure), keyed by asset PK → suffix. Guarded because
    // ZEXTENDEDATTRIBUTES is absent on some iOS versions.
    if table_exists(&src, "ZEXTENDEDATTRIBUTES")? {
        let mut stmt = src.prepare(
            "SELECT ZASSET, ZCAMERAMAKE, ZCAMERAMODEL, ZLENSMODEL,
                    ZISO, ZAPERTURE, ZSHUTTERSPEED, ZFOCALLENGTH
             FROM ZEXTENDEDATTRIBUTES WHERE ZASSET IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let pk: i64 = r.get(0)?;
            let camera = camera_label(
                r.get::<_, Option<String>>(1)?.as_deref(),
                r.get::<_, Option<String>>(2)?.as_deref(),
            );
            let lens = r
                .get::<_, Option<String>>(3)?
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let exif = exif_summary(r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?);
            if camera.is_none() && lens.is_none() && exif.is_none() {
                continue;
            }
            if let Some(meta) = pk_to_suffix.get(&pk).and_then(|s| by_suffix.get_mut(s)) {
                meta.camera = camera;
                meta.lens = lens;
                meta.exif = exif;
            }
        }
    }

    // Original file size, from ZADDITIONALASSETATTRIBUTES (separate table, guarded).
    if table_exists(&src, "ZADDITIONALASSETATTRIBUTES")? {
        let mut stmt = src.prepare(
            "SELECT ZASSET, ZORIGINALFILESIZE FROM ZADDITIONALASSETATTRIBUTES
             WHERE ZASSET IS NOT NULL AND ZORIGINALFILESIZE IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let pk: i64 = r.get(0)?;
            let size = r.get::<_, Option<i64>>(1)?.filter(|v| *v > 0);
            if let Some(meta) = pk_to_suffix.get(&pk).and_then(|s| by_suffix.get_mut(s)) {
                meta.file_size = size;
            }
        }
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
                 taken_at = COALESCE(?5, taken_at),
                 location = ?6,
                 albums = ?7,
                 width = COALESCE(?8, width),
                 height = COALESCE(?9, height),
                 duration_s = COALESCE(?10, duration_s),
                 file_size = ?11,
                 camera = ?12,
                 lens = ?13,
                 exif = ?14,
                 hidden = ?15,
                 subtype = ?16
             WHERE id = ?17",
            rusqlite::params![
                meta.persons,
                meta.latitude,
                meta.longitude,
                meta.favorite as i64,
                meta.taken_at,
                meta.location,
                meta.albums,
                meta.width,
                meta.height,
                meta.duration_s,
                meta.file_size,
                meta.camera,
                meta.lens,
                meta.exif,
                meta.hidden as i64,
                meta.subtype,
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
                 ZDATECREATED REAL, ZLATITUDE REAL, ZLONGITUDE REAL, ZFAVORITE INTEGER, ZMOMENT INTEGER,
                 ZWIDTH INTEGER, ZHEIGHT INTEGER, ZDURATION REAL, ZHIDDEN INTEGER,
                 ZKINDSUBTYPE INTEGER, ZISDETECTEDSCREENSHOT INTEGER);
             CREATE TABLE ZPERSON (Z_PK INTEGER PRIMARY KEY, ZFULLNAME TEXT, ZDISPLAYNAME TEXT);
             CREATE TABLE ZDETECTEDFACE (Z_PK INTEGER PRIMARY KEY, ZASSETFORFACE INTEGER, ZPERSONFORFACE INTEGER);
             CREATE TABLE ZMOMENT (Z_PK INTEGER PRIMARY KEY, ZTITLE TEXT);
             CREATE TABLE ZGENERICALBUM (Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER, ZTITLE TEXT);
             CREATE TABLE Z_33ASSETS (Z_33ALBUMS INTEGER, Z_3ASSETS INTEGER);
             CREATE TABLE ZEXTENDEDATTRIBUTES (Z_PK INTEGER PRIMARY KEY, ZASSET INTEGER,
                 ZCAMERAMAKE TEXT, ZCAMERAMODEL TEXT, ZLENSMODEL TEXT,
                 ZISO INTEGER, ZAPERTURE REAL, ZSHUTTERSPEED REAL, ZFOCALLENGTH REAL);
             CREATE TABLE ZADDITIONALASSETATTRIBUTES (Z_PK INTEGER PRIMARY KEY, ZASSET INTEGER,
                 ZORIGINALFILESIZE INTEGER);
             INSERT INTO ZMOMENT VALUES (500, 'Florida');
             -- Asset 1: named people, a real date (721692800 Mac = 1_700_000_000 unix),
             -- a GPS fix, favorited, in the 'Florida' moment, 4032x3024 photo.
             INSERT INTO ZASSET VALUES (1, 'DCIM/100APPLE', 'IMG_0001.HEIC', 721692800.0, 59.33, 18.06, 1, 500, 4032, 3024, 0.0, 0, 0, 0);
             -- Asset 2: no named people, no location (-180 sentinel), not favorited, no moment, hidden, a screenshot.
             INSERT INTO ZASSET VALUES (2, 'DCIM/100APPLE', 'IMG_0002.HEIC', NULL, -180.0, -180.0, 0, NULL, NULL, NULL, NULL, 1, 10, 1);
             -- EXIF + file size for asset 1 (make 'Apple' + model 'iPhone 14 Pro').
             INSERT INTO ZEXTENDEDATTRIBUTES VALUES (1, 1, 'Apple', 'iPhone 14 Pro', 'iPhone 14 Pro back camera', 100, 1.8, 0.008, 26.0);
             INSERT INTO ZADDITIONALASSETATTRIBUTES VALUES (1, 1, 2097152);
             INSERT INTO ZPERSON VALUES (10, 'Alice', NULL);
             INSERT INTO ZPERSON VALUES (11, NULL, 'Bob');
             INSERT INTO ZPERSON VALUES (12, '', '');  -- unnamed cluster, ignored
             INSERT INTO ZDETECTEDFACE VALUES (100, 1, 10);
             INSERT INTO ZDETECTEDFACE VALUES (101, 1, 11);
             INSERT INTO ZDETECTEDFACE VALUES (102, 2, 12);
             -- Album: a user album (ZKIND 2) 'Vacation' containing asset 1; a smart
             -- album (ZKIND 1509) 'Recents' is ignored.
             INSERT INTO ZGENERICALBUM VALUES (20, 2, 'Vacation');
             INSERT INTO ZGENERICALBUM VALUES (21, 1509, 'Recents');
             INSERT INTO Z_33ASSETS VALUES (20, 1);
             INSERT INTO Z_33ASSETS VALUES (21, 1);",
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

        let (location, albums): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT location, albums FROM media_items WHERE relative_path LIKE '%IMG_0001%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(location.as_deref(), Some("Florida"));
        assert_eq!(albums.as_deref(), Some("Vacation"), "smart album excluded");

        // EXIF + dimensions + file size for asset 1.
        let dims: (Option<i64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT width, height, file_size
                 FROM media_items WHERE relative_path LIKE '%IMG_0001%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(dims, (Some(4032), Some(3024), Some(2_097_152)));

        let (camera, lens, exif): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT camera, lens, exif
                 FROM media_items WHERE relative_path LIKE '%IMG_0001%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(camera.as_deref(), Some("Apple iPhone 14 Pro"));
        assert_eq!(lens.as_deref(), Some("iPhone 14 Pro back camera"));
        // ISO 100 · ƒ/1.8 · 1/125s (0.008 = 1/125) · 26 mm.
        assert_eq!(exif.as_deref(), Some("ISO 100 · ƒ/1.8 · 1/125s · 26 mm"));

        // Asset 2: no people, no location (sentinel dropped), not favorite, hidden.
        let (persons2, lat2, fav2, hidden2): (Option<String>, Option<f64>, i64, i64) = conn
            .query_row(
                "SELECT persons, latitude, is_favorite, hidden FROM media_items
                 WHERE relative_path LIKE '%IMG_0002%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(persons2, None);
        assert_eq!(lat2, None);
        assert_eq!(fav2, 0);
        assert_eq!(hidden2, 1, "asset 2 is in the Hidden album");
        let subtype2: Option<String> = conn
            .query_row(
                "SELECT subtype FROM media_items WHERE relative_path LIKE '%IMG_0002%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(subtype2.as_deref(), Some("screenshot"));
    }
}
