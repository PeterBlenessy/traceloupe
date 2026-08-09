//! Native camera-roll reader for iOS backups (encrypted and unencrypted).
//!
//! Reads `Manifest.db` and enumerates the camera roll, pairing each asset
//! with iOS's pre-rendered JPEG thumbnail from the `Media/PhotoData/Thumbnails/V2`
//! store, so the gallery grid uses ready-made thumbnails (no HEIC decoding) while
//! full images are transcoded on demand.
//!
//! For **unencrypted** backups everything is read raw: thumbnails/originals are
//! served straight from the backup's content-addressed blobs.
//!
//! For **encrypted** backups a [`BackupDecryptor`] supplies the keys. We decrypt
//! `Manifest.db` (and `Photos.sqlite`) to short-lived temp files in the media
//! cache dir, eagerly decrypt the small V2 thumbnails into that cache (so the
//! grid stays instant even after the keys are dropped), and record each full
//! image's wrapped key so the lightbox can decrypt it on demand — the originals
//! are never bulk-decrypted.
//!
//! provenance: reference (own implementation) from the iTunes-backup Manifest
//! and CameraRoll layout; decryption via [`crate::crypto`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::crypto::{self, BackupDecryptor};
use crate::{Error, Result};

/// How much of an asset this backup actually holds.
///
/// A device with iCloud Photos on does not put most of its photo *files* in the
/// backup — the catalogue lists every asset, but the originals stay in iCloud.
/// Measured on a real 95,334-asset library, 10,396 assets had their original and
/// 92,720 had only a thumbnail. Enumerating files alone therefore shows about a
/// tenth of the library and gives no sign that the rest exists, which is what
/// made tens of thousands of hidden photos look like they had been lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The full-resolution file is in the backup.
    Original,
    /// Only iOS's pre-rendered thumbnail is here; the original is in iCloud.
    ThumbnailOnly,
    /// The catalogue knows the asset but the backup holds no pixels at all.
    MetadataOnly,
}

impl Availability {
    /// Stored on the cache row and matched by the UI filter.
    pub fn as_str(self) -> &'static str {
        match self {
            Availability::Original => "original",
            Availability::ThumbnailOnly => "thumbnail",
            Availability::MetadataOnly => "metadata",
        }
    }
}

/// One camera-roll asset resolved to on-disk backup files.
#[derive(Debug, Clone)]
pub struct CameraRollAsset {
    /// e.g. `Media/DCIM/258APPLE/IMG_8998.HEIC`.
    pub relative_path: String,
    /// Full-resolution file in the backup (hashed name). Ciphertext on an
    /// encrypted backup — decrypt with [`Self::decrypt_key`] before serving.
    /// `None` when the original was offloaded to iCloud and never backed up.
    pub full_path: Option<PathBuf>,
    /// What this backup actually holds for the asset.
    pub availability: Availability,
    /// iOS's pre-rendered JPEG thumbnail in the backup, if one exists. Still
    /// CIPHERTEXT on an encrypted backup — decrypt with [`Self::thumb_key`] on
    /// demand. Eagerly decrypting every one used to be affordable because only
    /// assets with an original were enumerated; across a whole iCloud library it
    /// would mean tens of thousands of decryptions and gigabytes written before
    /// the first photo appears.
    pub thumb_path: Option<PathBuf>,
    /// Encrypted backups only: wrapped key + plaintext length for `thumb_path`.
    pub thumb_key: Option<Vec<u8>>,
    pub thumb_size: Option<u64>,
    /// "photo" | "video".
    pub kind: &'static str,
    pub mime: Option<String>,
    /// Capture time (epoch seconds) from Photos.sqlite, if available.
    pub taken_at: Option<i64>,
    /// Encrypted backups only: the class-prefixed wrapped key that decrypts
    /// `full_path` on demand (stored on the cache row). None when the original
    /// is already plaintext.
    pub decrypt_key: Option<Vec<u8>>,
    /// Encrypted backups only: the original's real plaintext length, to trim the
    /// CBC block padding when decrypting on demand. None for plaintext backups.
    pub plain_size: Option<u64>,
}

/// Where the camera roll's files live, relative to `Media/`.
///
/// `DCIM/` is only half of it. An iCloud Photo Library keeps its assets under
/// `PhotoData/CPLAssets/group<N>/` instead, and a device with iCloud Photos on
/// puts most of the roll there — including hidden items, screenshots and screen
/// recordings. Reading only `DCIM/` silently dropped every one of them: measured
/// on the public iOS 17 backup, 216 of the 519 assets whose files are present
/// (42%) were never imported, and the gallery gave no sign that anything was
/// missing. Both roots are read; `ZDIRECTORY` already names whichever applies.
const ASSET_ROOTS: [&str; 2] = ["Media/DCIM/", "Media/PhotoData/CPLAssets/"];
/// Thumbnails mirror the asset's own path under this prefix, e.g.
/// `…/V2/DCIM/258APPLE/IMG_8998.HEIC/5005.JPG` and
/// `…/V2/PhotoData/CPLAssets/group1/IMG_0042.HEIC/5005.JPG`.
const THUMB_PREFIX: &str = "Media/PhotoData/Thumbnails/V2/";
const MEDIA_PREFIX: &str = "Media/";

/// Enumerate camera-roll assets. Pass `decryptor` for an encrypted backup (its
/// keys decrypt Manifest.db, thumbnails, and Photos.sqlite); pass `None` for a
/// plaintext backup. `media_cache_dir` holds decrypted thumbnails plus transient
/// decrypted copies of Manifest.db/Photos.sqlite (encrypted backups only).
/// Returns an error if the manifest can't be read (e.g. an encrypted backup with
/// no decryptor).
pub fn parse_camera_roll(
    backup_dir: &Path,
    decryptor: Option<&BackupDecryptor>,
    media_cache_dir: &Path,
) -> Result<Vec<CameraRollAsset>> {
    // Point rusqlite at a plaintext Manifest.db: the backup's own for unencrypted
    // backups, a decrypted temp copy for encrypted ones.
    let manifest_temp = media_cache_dir.join(".manifest.db");
    let manifest_path = if let Some(dec) = decryptor {
        std::fs::create_dir_all(media_cache_dir).map_err(|e| Error::io(media_cache_dir, e))?;
        crate::write_private(&manifest_temp, &dec.decrypt_manifest_db()?)
            .map_err(|e| Error::io(&manifest_temp, e))?;
        manifest_temp.clone()
    } else {
        backup_dir.join("Manifest.db")
    };

    let result = enumerate(backup_dir, decryptor, media_cache_dir, &manifest_path);

    // Clean up the transient decrypted DBs (the decrypted thumbnails stay).
    if decryptor.is_some() {
        let _ = std::fs::remove_file(&manifest_temp);
        let _ = std::fs::remove_file(media_cache_dir.join(".photos.sqlite"));
    }
    result
}

fn enumerate(
    backup_dir: &Path,
    decryptor: Option<&BackupDecryptor>,
    media_cache_dir: &Path,
    manifest_path: &Path,
) -> Result<Vec<CameraRollAsset>> {
    let conn = Connection::open_with_flags(manifest_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // A backed-up file lives at `<backup>/<first two hex>/<fileID>`. Reject
    // non-hex ids from the untrusted Manifest so they can't `join` out of the dir.
    let file_path = |file_id: &str| -> PathBuf {
        if !crypto::is_valid_file_id(file_id) {
            return backup_dir.join("__invalid_file_id__");
        }
        backup_dir.join(&file_id[..2]).join(file_id)
    };

    // Thumbnails keyed by the asset's path relative to `Media/`
    // (e.g. "DCIM/258APPLE/IMG_8998.HEIC", "PhotoData/CPLAssets/group1/IMG_1.HEIC"),
    // which is the same key the Manifest and Photos.sqlite sides build below.
    let mut thumbs: HashMap<String, (String, Vec<u8>)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT fileID, relativePath, file FROM Files
             WHERE domain = 'CameraRollDomain'
               AND relativePath LIKE 'Media/PhotoData/Thumbnails/V2/%.JPG'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
            ))
        })?;
        for (file_id, rel, blob) in rows.flatten() {
            if let Some(rest) = rel.strip_prefix(THUMB_PREFIX) {
                // rest = "DCIM/258APPLE/IMG_8998.HEIC/5005.JPG" → key drops the size file.
                if let Some(idx) = rest.rfind('/') {
                    thumbs
                        .entry(rest[..idx].to_string())
                        .or_insert((file_id, blob));
                }
            }
        }
    }

    // Capture dates + trashed flag from Photos.sqlite (best effort — the gallery
    // still works without it, just without real dates / trash filtering).
    let meta =
        load_photos_metadata(&conn, backup_dir, decryptor, media_cache_dir).unwrap_or_default();

    // Built from ASSET_ROOTS so the roots can't drift from their documentation.
    let roots = ASSET_ROOTS
        .iter()
        .map(|r| format!("relativePath LIKE '{r}%'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut stmt = conn.prepare(&format!(
        "SELECT fileID, relativePath, file FROM Files
         WHERE domain = 'CameraRollDomain' AND ({roots})
         ORDER BY relativePath"
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
        ))
    })?;

    // Files whose original is present, keyed like the catalogue and the
    // thumbnails. BTreeMap so the output order stays deterministic now that it
    // no longer comes from the Manifest's ORDER BY.
    let mut files: std::collections::BTreeMap<String, (String, String, Vec<u8>)> =
        std::collections::BTreeMap::new();
    for (file_id, rel, blob) in rows.flatten() {
        if classify(&rel).is_none() {
            continue; // skip directories, .AAE sidecars, etc.
        }
        let key = rel.strip_prefix(MEDIA_PREFIX).unwrap_or(&rel).to_string();
        files.insert(key, (file_id, rel, blob));
    }

    // THE UNION, not either side alone. The catalogue holds assets whose files
    // stayed in iCloud; the file set holds things the catalogue never lists —
    // notably the `.MOV` half of a Live Photo, which has no ZASSET row of its
    // own. Enumerating either side alone silently drops the other's population.
    let keys: std::collections::BTreeSet<&String> = files.keys().chain(meta.keys()).collect();

    let mut assets = Vec::new();
    for key in keys {
        let key = key.clone();
        let present = files.get(&key);
        // A catalogue-only asset has no Manifest row, so its path is derived from
        // the key the catalogue itself gave us.
        let rel = match present {
            Some((_, rel, _)) => rel.clone(),
            None => format!("{MEDIA_PREFIX}{key}"),
        };
        let Some((kind, mime)) = classify(&rel) else {
            continue;
        };
        // A Live Photo is TWO files on disk — `IMG_0001.HEIC` and its paired
        // `IMG_0001.MOV` — but only ONE asset row (the still). So the `.MOV`
        // component finds no metadata and would show no capture date. Borrow the
        // still's date: same basename, a photo extension. Only for the paired
        // component; a standalone video keeps its own asset row and date.
        let asset_meta = meta.get(&key).or_else(|| {
            let (stem, ext) = key.rsplit_once('.')?;
            if !ext.eq_ignore_ascii_case("mov") {
                return None;
            }
            ["HEIC", "heic", "JPG", "jpg", "HEIF", "heif", "PNG", "png"]
                .iter()
                .find_map(|e| meta.get(&format!("{stem}.{e}")))
        });
        // Recently-deleted (trashed) assets are ingested too and badged later by
        // photos_meta (ZTRASHEDSTATE) — surfaced, not excluded, for forensics.

        // Point at the thumbnail's backup blob and carry its key; the media
        // handler decrypts on first request and caches the result. Doing it here
        // instead would decrypt the whole library up front.
        let (thumb_path, thumb_key, thumb_size) = match thumbs.get(&key) {
            None => (None, None, None),
            Some((tid, tblob)) => {
                let (k, s) = match decryptor {
                    Some(_) => match crypto::file_key_field(tblob) {
                        Ok((k, s)) => (Some(k), s),
                        // A thumbnail we cannot unwrap is not worth losing the
                        // asset over — it still has its metadata and maybe its
                        // original.
                        Err(_) => (None, None),
                    },
                    None => (None, None),
                };
                (Some(file_path(tid)), k, s)
            }
        };

        // Encrypted backups: keep the wrapped key + real size so the original
        // decrypts (and trims) on demand. Plaintext backups serve it directly.
        // An asset with a missing/malformed `file` blob keeps its metadata and
        // thumbnail rather than vanishing — dropping it is how a photo silently
        // disappears from the gallery.
        let (full_path, decrypt_key, plain_size) = match present {
            None => (None, None, None),
            Some((file_id, _, blob)) => match decryptor {
                Some(_) => match crypto::file_key_field(blob) {
                    Ok((enc_key, size)) => (Some(file_path(file_id)), Some(enc_key), size),
                    Err(_) => (None, None, None),
                },
                None => (Some(file_path(file_id)), None, None),
            },
        };

        let availability = match (&full_path, &thumb_path) {
            (Some(_), _) => Availability::Original,
            (None, Some(_)) => Availability::ThumbnailOnly,
            (None, None) => Availability::MetadataOnly,
        };

        assets.push(CameraRollAsset {
            full_path,
            availability,
            thumb_path,
            thumb_key,
            thumb_size,
            kind,
            mime: Some(mime.to_string()),
            taken_at: asset_meta.and_then(|m| m.taken_at),
            decrypt_key,
            plain_size,
            relative_path: rel,
        });
    }
    Ok(assets)
}

struct AssetMeta {
    taken_at: Option<i64>,
}

/// Per-asset capture date from the backup's `Photos.sqlite`, keyed by
/// "<album>/<filename>" (e.g. "258APPLE/IMG_8998.HEIC"). Best-effort: schema
/// varies by iOS version, so any failure yields an empty map. (The trashed flag
/// and the rest of the ZASSET metadata are applied later by `photos_meta`.)
fn load_photos_metadata(
    manifest: &Connection,
    backup_dir: &Path,
    decryptor: Option<&BackupDecryptor>,
    media_cache_dir: &Path,
) -> Result<HashMap<String, AssetMeta>> {
    let (file_id, blob): (String, Option<Vec<u8>>) = manifest.query_row(
        "SELECT fileID, file FROM Files
         WHERE domain = 'CameraRollDomain' AND relativePath = 'Media/PhotoData/Photos.sqlite'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // The fileID is from the untrusted Manifest; reject a non-hex value so the
    // plaintext branch below can't `join` its way out of backup_dir (matches the
    // guard on every other blob-path construction).
    if !crypto::is_valid_file_id(&file_id) {
        return Ok(HashMap::new());
    }

    // Open Photos.sqlite `immutable` — it's WAL-mode with no sidecars in the
    // backup, so this reads the main file directly (ignoring the missing WAL).
    let conn = match decryptor {
        None => {
            let photos = backup_dir.join(&file_id[..2]).join(&file_id);
            Connection::open_with_flags(
                immutable_uri(&photos),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?
        }
        Some(dec) => {
            let dest = media_cache_dir.join(".photos.sqlite");
            let plain = dec.decrypt_file(blob.as_deref().unwrap_or_default(), &file_id)?;
            crate::write_private(&dest, &plain).map_err(|e| Error::io(&dest, e))?;
            Connection::open_with_flags(
                immutable_uri(&dest),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?
        }
    };

    read_photos_metadata(&conn)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Query the asset table on an open Photos.sqlite for capture dates.
fn read_photos_metadata(conn: &Connection) -> Result<HashMap<String, AssetMeta>> {
    // ZDATECREATED is a Core Data timestamp (seconds since 2001-01-01).
    const COCOA_EPOCH_OFFSET: f64 = 978_307_200.0;
    // The asset table is `ZGENERICASSET` on iOS 13/14 and `ZASSET` from iOS 15.
    // Querying the wrong one just errors (→ empty map → no dates), which is
    // exactly what made every photo on an older backup show no capture date.
    let asset_table = if table_exists(conn, "ZASSET")? {
        "ZASSET"
    } else {
        "ZGENERICASSET"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT ZDIRECTORY, ZFILENAME, ZDATECREATED
         FROM {asset_table} WHERE ZDIRECTORY IS NOT NULL AND ZFILENAME IS NOT NULL",
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<f64>>(2)?,
        ))
    })?;

    let mut map = HashMap::new();
    for (dir, fname, date) in rows.flatten() {
        let key = format!("{}/{}", dir.trim_end_matches('/'), fname);
        map.insert(
            key,
            AssetMeta {
                taken_at: date.map(|d| (d + COCOA_EPOCH_OFFSET) as i64),
            },
        );
    }
    Ok(map)
}

/// Build a percent-encoded `file:…?immutable=1` SQLite URI for `path`.
fn immutable_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'-' | b'_' | b'~' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// Classify a DCIM file by extension into (kind, mime); None for non-media.
fn classify(rel: &str) -> Option<(&'static str, &'static str)> {
    let lower = rel.to_ascii_lowercase();
    let ext = lower.rsplit('.').next()?;
    Some(match ext {
        "heic" | "heif" => ("photo", "image/heic"),
        "jpg" | "jpeg" => ("photo", "image/jpeg"),
        "png" => ("photo", "image/png"),
        "gif" => ("photo", "image/gif"),
        "mov" => ("video", "video/quicktime"),
        "mp4" | "m4v" => ("video", "video/mp4"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the inversion. With iCloud Photos on, the catalogue
    /// lists tens of thousands of assets whose ORIGINALS were never backed up —
    /// on a real 95,334-asset library only 10,396 had one. Enumerating files
    /// alone showed a tenth of the library and gave no sign the rest existed,
    /// which is what made a user's hidden photos look permanently lost. They are
    /// recoverable as thumbnails, and must be emitted.
    #[test]
    fn emits_offloaded_assets_that_have_only_a_thumbnail() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path();
        let conn = Connection::open(backup.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             -- One asset with its original present.
             INSERT INTO Files VALUES ('aa11', 'CameraRollDomain', 'Media/DCIM/100APPLE/IMG_0001.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('bb22', 'CameraRollDomain', 'Media/PhotoData/Thumbnails/V2/DCIM/100APPLE/IMG_0001.HEIC/5005.JPG', 1, NULL);
             -- One offloaded: thumbnail only, NO original in the backup.
             INSERT INTO Files VALUES ('cc33', 'CameraRollDomain', 'Media/PhotoData/Thumbnails/V2/DCIM/100APPLE/IMG_0002.HEIC/5005.JPG', 1, NULL);
             -- The Live Photo video half, which has no catalogue row of its own
             -- and would be dropped by a catalogue-only enumeration.
             INSERT INTO Files VALUES ('ee55', 'CameraRollDomain', 'Media/DCIM/100APPLE/IMG_0001.MOV', 1, NULL);
             INSERT INTO Files VALUES ('ff55aa', 'CameraRollDomain', 'Media/PhotoData/Photos.sqlite', 1, NULL);",
        )
        .unwrap();
        let photos = backup.join("ff").join("ff55aa");
        std::fs::create_dir_all(photos.parent().unwrap()).unwrap();
        let ph = Connection::open(&photos).unwrap();
        ph.execute_batch(
            "CREATE TABLE ZASSET (ZDIRECTORY TEXT, ZFILENAME TEXT, ZDATECREATED REAL, ZTRASHEDSTATE INTEGER);
             INSERT INTO ZASSET VALUES ('DCIM/100APPLE', 'IMG_0001.HEIC', 700000000.0, 0);
             INSERT INTO ZASSET VALUES ('DCIM/100APPLE', 'IMG_0002.HEIC', 700000500.0, 0);
             -- Catalogued but nothing local at all: neither original nor thumbnail.
             INSERT INTO ZASSET VALUES ('DCIM/100APPLE', 'IMG_0003.HEIC', 700000900.0, 0);",
        )
        .unwrap();

        let assets = parse_camera_roll(backup, None, &backup.join("_cache")).unwrap();
        let by = |n: &str| {
            assets
                .iter()
                .find(|a| a.relative_path.ends_with(n))
                .unwrap_or_else(|| {
                    panic!(
                        "{n} missing from {:?}",
                        assets.iter().map(|a| &a.relative_path).collect::<Vec<_>>()
                    )
                })
        };

        assert_eq!(by("IMG_0001.HEIC").availability, Availability::Original);
        assert!(by("IMG_0001.HEIC").full_path.is_some());

        // The offloaded one: present, showable, and honest about what it is.
        let off = by("IMG_0002.HEIC");
        assert_eq!(off.availability, Availability::ThumbnailOnly);
        assert!(off.full_path.is_none(), "there is no original to point at");
        assert!(
            off.thumb_path.is_some(),
            "its thumbnail is what makes it viewable"
        );
        assert_eq!(off.taken_at, Some(700_000_500 + 978_307_200));

        assert_eq!(by("IMG_0003.HEIC").availability, Availability::MetadataOnly);

        // The union, not the catalogue alone — the .MOV half has no ZASSET row.
        assert_eq!(by("IMG_0001.MOV").availability, Availability::Original);
        assert_eq!(assets.len(), 4);
    }

    /// An iCloud Photo Library keeps the roll under `PhotoData/CPLAssets/`, not
    /// `DCIM/`. Reading only `DCIM/` dropped those assets entirely and gave no
    /// sign of it — on the public iOS 17 backup that was 216 of 519 present
    /// assets (42%), and on a device with iCloud Photos on it is most of the
    /// roll, hidden screenshots and screen recordings included.
    #[test]
    fn reads_icloud_library_assets_and_not_only_dcim() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path();
        let conn = Connection::open(backup.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('aa11', 'CameraRollDomain', 'Media/DCIM/100APPLE/IMG_0001.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('bb22', 'CameraRollDomain', 'Media/PhotoData/Thumbnails/V2/DCIM/100APPLE/IMG_0001.HEIC/5005.JPG', 1, NULL);
             INSERT INTO Files VALUES ('cc33', 'CameraRollDomain', 'Media/PhotoData/CPLAssets/group42/IMG_0099.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('dd44', 'CameraRollDomain', 'Media/PhotoData/Thumbnails/V2/PhotoData/CPLAssets/group42/IMG_0099.HEIC/5005.JPG', 1, NULL);
             INSERT INTO Files VALUES ('ff55aa', 'CameraRollDomain', 'Media/PhotoData/Photos.sqlite', 1, NULL);",
        )
        .unwrap();
        let photos = backup.join("ff").join("ff55aa");
        std::fs::create_dir_all(photos.parent().unwrap()).unwrap();
        let ph = Connection::open(&photos).unwrap();
        ph.execute_batch(
            "CREATE TABLE ZASSET (ZDIRECTORY TEXT, ZFILENAME TEXT, ZDATECREATED REAL, ZTRASHEDSTATE INTEGER);
             INSERT INTO ZASSET VALUES ('DCIM/100APPLE', 'IMG_0001.HEIC', 700000000.0, 0);
             INSERT INTO ZASSET VALUES ('PhotoData/CPLAssets/group42', 'IMG_0099.HEIC', 700000500.0, 0);",
        )
        .unwrap();

        let assets = parse_camera_roll(backup, None, &backup.join("_cache")).unwrap();
        let paths: Vec<&str> = assets.iter().map(|a| a.relative_path.as_str()).collect();
        assert!(
            paths.contains(&"Media/PhotoData/CPLAssets/group42/IMG_0099.HEIC"),
            "the iCloud-library asset must be imported, got {paths:?}"
        );
        assert_eq!(assets.len(), 2, "both roots, got {paths:?}");

        // Its thumbnail and its capture date have to resolve too — an asset that
        // appears with neither is barely better than one that does not appear.
        let cpl = assets
            .iter()
            .find(|a| a.relative_path.contains("CPLAssets"))
            .unwrap();
        assert!(
            cpl.thumb_path.is_some(),
            "iCloud-library thumbnail unresolved"
        );
        assert_eq!(
            cpl.taken_at,
            Some(700_000_500 + 978_307_200),
            "iCloud-library capture date unresolved"
        );
    }

    #[test]
    fn pairs_dcim_assets_with_thumbnails_and_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path();
        let conn = Connection::open(backup.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('aa11', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_8998.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('bb22', 'CameraRollDomain', 'Media/PhotoData/Thumbnails/V2/DCIM/258APPLE/IMG_8998.HEIC/5005.JPG', 1, NULL);
             INSERT INTO Files VALUES ('cc33', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_9001.MOV', 1, NULL);
             INSERT INTO Files VALUES ('dd44', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_8998.AAE', 1, NULL);
             INSERT INTO Files VALUES ('ee7777', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_7777.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('ff55aa', 'CameraRollDomain', 'Media/PhotoData/Photos.sqlite', 1, NULL);",
        )
        .unwrap();

        // Photos.sqlite for capture dates + trashed filtering.
        let photos = backup.join("ff").join("ff55aa");
        std::fs::create_dir_all(photos.parent().unwrap()).unwrap();
        let ph = Connection::open(&photos).unwrap();
        ph.execute_batch(
            "CREATE TABLE ZASSET (ZDIRECTORY TEXT, ZFILENAME TEXT, ZDATECREATED REAL, ZTRASHEDSTATE INTEGER);
             INSERT INTO ZASSET VALUES ('DCIM/258APPLE', 'IMG_8998.HEIC', 700000000.0, 0);
             INSERT INTO ZASSET VALUES ('DCIM/258APPLE', 'IMG_7777.HEIC', 700000100.0, 1);",
        )
        .unwrap();

        // Unencrypted: no decryptor, cache dir unused.
        let assets = parse_camera_roll(backup, None, &backup.join("_cache")).unwrap();
        // .AAE sidecar skipped; trashed IMG_7777 is ingested now (badged later by
        // photos_meta), not excluded — so photo, video, and the trashed photo.
        assert_eq!(assets.len(), 3);
        assert!(assets.iter().any(|a| a.relative_path.contains("IMG_7777")));

        let photo = assets
            .iter()
            .find(|a| a.relative_path.ends_with("IMG_8998.HEIC"))
            .unwrap();
        assert_eq!(photo.kind, "photo");
        assert_eq!(photo.full_path, Some(backup.join("aa").join("aa11")));
        assert_eq!(photo.thumb_path, Some(backup.join("bb").join("bb22")));
        assert_eq!(photo.mime.as_deref(), Some("image/heic"));
        assert_eq!(photo.decrypt_key, None); // plaintext backup
        assert_eq!(photo.plain_size, None);
        // 700000000 (Cocoa) + 978307200 = 1678307200 (Unix).
        assert_eq!(photo.taken_at, Some(1_678_307_200));

        let video = assets.iter().find(|a| a.kind == "video").unwrap();
        assert!(video.thumb_path.is_none()); // no thumb entry for the video
        assert_eq!(video.taken_at, None); // not in ZASSET, and no sibling still
    }

    #[test]
    fn older_ios_uses_zgenericasset_and_live_photo_movs_borrow_the_stills_date() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path();
        let conn = Connection::open(backup.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('aa11', 'CameraRollDomain', 'Media/DCIM/100APPLE/IMG_0001.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('bb22', 'CameraRollDomain', 'Media/DCIM/100APPLE/IMG_0001.MOV', 1, NULL);
             INSERT INTO Files VALUES ('cc33', 'CameraRollDomain', 'Media/DCIM/100APPLE/IMG_0002.MOV', 1, NULL);
             INSERT INTO Files VALUES ('ff55aa', 'CameraRollDomain', 'Media/PhotoData/Photos.sqlite', 1, NULL);",
        )
        .unwrap();

        // iOS 13/14 keep assets in ZGENERICASSET, not ZASSET.
        let photos = backup.join("ff").join("ff55aa");
        std::fs::create_dir_all(photos.parent().unwrap()).unwrap();
        let ph = Connection::open(&photos).unwrap();
        ph.execute_batch(
            "CREATE TABLE ZGENERICASSET (ZDIRECTORY TEXT, ZFILENAME TEXT, ZDATECREATED REAL, ZTRASHEDSTATE INTEGER);
             INSERT INTO ZGENERICASSET VALUES ('DCIM/100APPLE', 'IMG_0001.HEIC', 700000000.0, 0);",
        )
        .unwrap();

        let assets = parse_camera_roll(backup, None, &backup.join("_cache")).unwrap();

        // The still is dated from ZGENERICASSET (the older-schema fix).
        let still = assets
            .iter()
            .find(|a| a.relative_path.ends_with("IMG_0001.HEIC"))
            .unwrap();
        assert_eq!(still.taken_at, Some(1_678_307_200));

        // Its paired Live Photo .MOV — no asset row of its own — borrows the
        // still's date rather than showing nothing.
        let live_mov = assets
            .iter()
            .find(|a| a.relative_path.ends_with("IMG_0001.MOV"))
            .unwrap();
        assert_eq!(live_mov.taken_at, Some(1_678_307_200));

        // A standalone video with no sibling still keeps no date — we don't
        // invent one.
        let lone_mov = assets
            .iter()
            .find(|a| a.relative_path.ends_with("IMG_0002.MOV"))
            .unwrap();
        assert_eq!(lone_mov.taken_at, None);
    }
}
