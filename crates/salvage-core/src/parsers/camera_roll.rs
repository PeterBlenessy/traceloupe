//! Native camera-roll reader for iOS backups (encrypted and unencrypted).
//!
//! Reads `Manifest.db` and enumerates the DCIM camera roll, pairing each asset
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

/// One camera-roll asset resolved to on-disk backup files.
#[derive(Debug, Clone)]
pub struct CameraRollAsset {
    /// e.g. `Media/DCIM/258APPLE/IMG_8998.HEIC`.
    pub relative_path: String,
    /// Full-resolution file in the backup (hashed name). Ciphertext on an
    /// encrypted backup — decrypt with [`Self::decrypt_key`] before serving.
    pub full_path: PathBuf,
    /// Pre-rendered JPEG thumbnail, ready to serve (decrypted into the cache on
    /// an encrypted backup), if one exists.
    pub thumb_path: Option<PathBuf>,
    /// "photo" | "video".
    pub kind: &'static str,
    pub mime: Option<String>,
    /// Capture time (epoch seconds) from Photos.sqlite, if available.
    pub taken_at: Option<i64>,
    /// Encrypted backups only: the class-prefixed wrapped key that decrypts
    /// `full_path` on demand (stored on the cache row). None when the original
    /// is already plaintext.
    pub decrypt_key: Option<Vec<u8>>,
}

const THUMB_PREFIX: &str = "Media/PhotoData/Thumbnails/V2/DCIM/";
const DCIM_PREFIX: &str = "Media/DCIM/";

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
        std::fs::write(&manifest_temp, dec.decrypt_manifest_db()?)
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

    // A backed-up file lives at `<backup>/<first two hex>/<fileID>`.
    let file_path = |file_id: &str| -> PathBuf {
        backup_dir
            .join(&file_id[..2.min(file_id.len())])
            .join(file_id)
    };

    // Thumbnails keyed by "<album>/<original filename>" (e.g. "258APPLE/IMG_8998.HEIC").
    // A relative path looks like `.../V2/DCIM/258APPLE/IMG_8998.HEIC/5005.JPG`.
    let mut thumbs: HashMap<String, (String, Vec<u8>)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT fileID, relativePath, file FROM Files
             WHERE domain = 'CameraRollDomain'
               AND relativePath LIKE 'Media/PhotoData/Thumbnails/V2/DCIM/%.JPG'",
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
                // rest = "258APPLE/IMG_8998.HEIC/5005.JPG" → key drops the size file.
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

    let mut stmt = conn.prepare(
        "SELECT fileID, relativePath, file FROM Files
         WHERE domain = 'CameraRollDomain' AND relativePath LIKE 'Media/DCIM/%'
         ORDER BY relativePath",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
        ))
    })?;

    let mut assets = Vec::new();
    for (file_id, rel, blob) in rows.flatten() {
        let Some((kind, mime)) = classify(&rel) else {
            continue; // skip directories, .AAE sidecars, etc.
        };
        let key = rel.strip_prefix(DCIM_PREFIX).unwrap_or(&rel).to_string();
        let asset_meta = meta.get(&key);
        if asset_meta.is_some_and(|m| m.trashed) {
            continue; // recently-deleted assets
        }

        // Resolve the thumbnail to a servable plaintext path (decrypt to the
        // cache for encrypted backups, raw path otherwise).
        let thumb_path = match thumbs.get(&key) {
            None => None,
            Some((tid, tblob)) => Some(resolve_thumb(
                decryptor,
                media_cache_dir,
                &file_path(tid),
                tid,
                tblob,
            )?),
        };

        // Encrypted backups: keep the wrapped key so the original decrypts on
        // demand. Plaintext backups serve the original directly.
        let decrypt_key = match decryptor {
            Some(_) => Some(crypto::file_key_field(&blob)?.0),
            None => None,
        };

        assets.push(CameraRollAsset {
            full_path: file_path(&file_id),
            thumb_path,
            kind,
            mime: Some(mime.to_string()),
            taken_at: asset_meta.and_then(|m| m.taken_at),
            decrypt_key,
            relative_path: rel,
        });
    }
    Ok(assets)
}

/// Resolve a thumbnail to a plaintext path the media protocol can serve raw. For
/// plaintext backups that's the backup blob itself; for encrypted backups we
/// decrypt it once into `<media_cache_dir>/thumbs/<fileID>.JPG` and reuse it.
fn resolve_thumb(
    decryptor: Option<&BackupDecryptor>,
    media_cache_dir: &Path,
    raw_path: &Path,
    file_id: &str,
    blob: &[u8],
) -> Result<PathBuf> {
    let Some(dec) = decryptor else {
        return Ok(raw_path.to_path_buf());
    };
    let dir = media_cache_dir.join("thumbs");
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let dest = dir.join(format!("{file_id}.JPG"));
    if !dest.exists() {
        let plain = dec.decrypt_file(blob, file_id)?;
        std::fs::write(&dest, plain).map_err(|e| Error::io(&dest, e))?;
    }
    Ok(dest)
}

struct AssetMeta {
    taken_at: Option<i64>,
    trashed: bool,
}

/// Per-asset capture date + trashed flag from the backup's `Photos.sqlite`,
/// keyed by "<album>/<filename>" (e.g. "258APPLE/IMG_8998.HEIC"). Best-effort:
/// schema varies by iOS version, so any failure yields an empty map.
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

    // Open Photos.sqlite `immutable` — it's WAL-mode with no sidecars in the
    // backup, so this reads the main file directly (ignoring the missing WAL).
    let conn = match decryptor {
        None => {
            let photos = backup_dir
                .join(&file_id[..2.min(file_id.len())])
                .join(&file_id);
            Connection::open_with_flags(
                immutable_uri(&photos),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?
        }
        Some(dec) => {
            let dest = media_cache_dir.join(".photos.sqlite");
            let plain = dec.decrypt_file(blob.as_deref().unwrap_or_default(), &file_id)?;
            std::fs::write(&dest, plain).map_err(|e| Error::io(&dest, e))?;
            Connection::open_with_flags(
                immutable_uri(&dest),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?
        }
    };

    read_photos_metadata(&conn)
}

/// Query ZASSET on an open Photos.sqlite for capture dates + trashed flags.
fn read_photos_metadata(conn: &Connection) -> Result<HashMap<String, AssetMeta>> {
    // ZDATECREATED is a Core Data timestamp (seconds since 2001-01-01).
    const COCOA_EPOCH_OFFSET: f64 = 978_307_200.0;
    let mut stmt = conn.prepare(
        "SELECT ZDIRECTORY, ZFILENAME, ZDATECREATED, ZTRASHEDSTATE
         FROM ZASSET WHERE ZDIRECTORY LIKE 'DCIM/%'",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<f64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
        ))
    })?;

    let mut map = HashMap::new();
    for (dir, fname, date, trashed) in rows.flatten() {
        let key = format!("{}/{}", dir.strip_prefix("DCIM/").unwrap_or(&dir), fname);
        map.insert(
            key,
            AssetMeta {
                taken_at: date.map(|d| (d + COCOA_EPOCH_OFFSET) as i64),
                trashed: trashed.unwrap_or(0) != 0,
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
        // .AAE sidecar skipped; trashed IMG_7777 excluded.
        assert_eq!(assets.len(), 2);
        assert!(assets.iter().all(|a| !a.relative_path.contains("IMG_7777")));

        let photo = assets.iter().find(|a| a.kind == "photo").unwrap();
        assert!(photo.relative_path.ends_with("IMG_8998.HEIC"));
        assert_eq!(photo.full_path, backup.join("aa").join("aa11"));
        assert_eq!(photo.thumb_path, Some(backup.join("bb").join("bb22")));
        assert_eq!(photo.mime.as_deref(), Some("image/heic"));
        assert_eq!(photo.decrypt_key, None); // plaintext backup
                                             // 700000000 (Cocoa) + 978307200 = 1678307200 (Unix).
        assert_eq!(photo.taken_at, Some(1_678_307_200));

        let video = assets.iter().find(|a| a.kind == "video").unwrap();
        assert!(video.thumb_path.is_none()); // no thumb entry for the video
        assert_eq!(video.taken_at, None); // not in ZASSET
    }
}
