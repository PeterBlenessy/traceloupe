//! Native camera-roll reader for UNENCRYPTED iOS backups.
//!
//! Reads `Manifest.db` directly (no iLEAPP) and enumerates the DCIM camera roll,
//! pairing each asset with iOS's pre-rendered JPEG thumbnail from the
//! `Media/PhotoData/Thumbnails/V2` store. So the gallery grid can use ready-made
//! thumbnails (no HEIC decoding) while full images are transcoded on demand.
//!
//! provenance: reference (own implementation) from the iTunes-backup Manifest
//! and CameraRoll layout. Encrypted backups (whose Manifest.db is itself
//! encrypted) aren't supported here yet — that needs the Phase-2 decryptor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::Result;

/// One camera-roll asset resolved to on-disk backup files.
#[derive(Debug, Clone)]
pub struct CameraRollAsset {
    /// e.g. `Media/DCIM/258APPLE/IMG_8998.HEIC`.
    pub relative_path: String,
    /// Full-resolution file in the backup (hashed name).
    pub full_path: PathBuf,
    /// Pre-rendered JPEG thumbnail in the backup, if one exists.
    pub thumb_path: Option<PathBuf>,
    /// "photo" | "video".
    pub kind: &'static str,
    pub mime: Option<String>,
}

const THUMB_PREFIX: &str = "Media/PhotoData/Thumbnails/V2/DCIM/";
const DCIM_PREFIX: &str = "Media/DCIM/";

/// Enumerate camera-roll assets from an unencrypted backup's `Manifest.db`.
/// Returns an error if the manifest can't be read (e.g. an encrypted backup).
pub fn parse_camera_roll(backup_dir: &Path) -> Result<Vec<CameraRollAsset>> {
    let manifest = backup_dir.join("Manifest.db");
    let conn =
        Connection::open_with_flags(&manifest, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let backup_dir = backup_dir.to_path_buf();
    // A backed-up file lives at `<backup>/<first two hex>/<fileID>`.
    let file_path = |file_id: &str| -> PathBuf {
        backup_dir.join(&file_id[..2.min(file_id.len())]).join(file_id)
    };

    // Thumbnails keyed by "<album>/<original filename>" (e.g. "258APPLE/IMG_8998.HEIC").
    // A relative path looks like `.../V2/DCIM/258APPLE/IMG_8998.HEIC/5005.JPG`.
    let mut thumbs: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT fileID, relativePath FROM Files
             WHERE domain = 'CameraRollDomain'
               AND relativePath LIKE 'Media/PhotoData/Thumbnails/V2/DCIM/%.JPG'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for (file_id, rel) in rows.flatten() {
            if let Some(rest) = rel.strip_prefix(THUMB_PREFIX) {
                // rest = "258APPLE/IMG_8998.HEIC/5005.JPG" → key drops the size file.
                if let Some(idx) = rest.rfind('/') {
                    thumbs.entry(rest[..idx].to_string()).or_insert(file_id);
                }
            }
        }
    }

    let mut stmt = conn.prepare(
        "SELECT fileID, relativePath FROM Files
         WHERE domain = 'CameraRollDomain' AND relativePath LIKE 'Media/DCIM/%'
         ORDER BY relativePath",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;

    let mut assets = Vec::new();
    for (file_id, rel) in rows.flatten() {
        let Some((kind, mime)) = classify(&rel) else {
            continue; // skip directories, .AAE sidecars, etc.
        };
        let key = rel.strip_prefix(DCIM_PREFIX).unwrap_or(&rel).to_string();
        let thumb_path = thumbs.get(&key).map(|tid| file_path(tid));
        assets.push(CameraRollAsset {
            full_path: file_path(&file_id),
            thumb_path,
            kind,
            mime: Some(mime.to_string()),
            relative_path: rel,
        });
    }
    Ok(assets)
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
    fn pairs_dcim_assets_with_thumbnails() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path();
        let conn = Connection::open(backup.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('aa11', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_8998.HEIC', 1, NULL);
             INSERT INTO Files VALUES ('bb22', 'CameraRollDomain', 'Media/PhotoData/Thumbnails/V2/DCIM/258APPLE/IMG_8998.HEIC/5005.JPG', 1, NULL);
             INSERT INTO Files VALUES ('cc33', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_9001.MOV', 1, NULL);
             INSERT INTO Files VALUES ('dd44', 'CameraRollDomain', 'Media/DCIM/258APPLE/IMG_8998.AAE', 1, NULL);",
        )
        .unwrap();

        let assets = parse_camera_roll(backup).unwrap();
        assert_eq!(assets.len(), 2); // .AAE sidecar skipped

        let photo = assets.iter().find(|a| a.kind == "photo").unwrap();
        assert!(photo.relative_path.ends_with("IMG_8998.HEIC"));
        assert_eq!(photo.full_path, backup.join("aa").join("aa11"));
        assert_eq!(photo.thumb_path, Some(backup.join("bb").join("bb22")));
        assert_eq!(photo.mime.as_deref(), Some("image/heic"));

        let video = assets.iter().find(|a| a.kind == "video").unwrap();
        assert!(video.thumb_path.is_none()); // no thumb entry for the video
    }
}
