//! Native Voice Memos reader for iOS backups (encrypted and unencrypted).
//!
//! The Voice Memos app records metadata in a Core Data DB (`CloudRecordings.db`
//! on modern iOS, `Recordings.db` on older) and stores each recording as an
//! `.m4a` under `Recordings/`. We locate both through the
//! [`crate::manifest::ManifestIndex`]: read title/date/duration/path from the DB,
//! then pair each recording with its audio blob (recording the wrapped key so an
//! encrypted `.m4a` decrypts on demand at play time — never bulk-decrypted).
//!
//! When the DB is absent or unreadable we still surface the audio: every `.m4a`
//! under `Recordings/` becomes an untitled recording (filename as the label), so
//! recordings show up even without metadata.
//!
//! provenance: reference (own implementation) from the Voice Memos Core Data
//! schema and the iTunes-backup layout; decryption via [`crate::crypto`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::crypto::{self, BackupDecryptor};
use crate::manifest::{FileEntry, ManifestIndex};
use crate::Result;

/// One voice memo resolved to its on-disk audio blob.
#[derive(Debug, Clone)]
pub struct RecordingAsset {
    /// User label, or None for an auto-named memo.
    pub title: Option<String>,
    pub recorded_at: Option<i64>,
    pub duration_s: Option<f64>,
    /// e.g. `Recordings/20240101 090000.m4a`.
    pub relative_path: String,
    /// The `.m4a` blob in the backup (ciphertext on an encrypted backup —
    /// decrypt with [`Self::decrypt_key`] before serving).
    pub full_path: PathBuf,
    pub mime: &'static str,
    /// Encrypted backups only: the wrapped key that decrypts `full_path` on
    /// demand. None when the audio is already plaintext.
    pub decrypt_key: Option<Vec<u8>>,
    /// Encrypted backups only: the real plaintext length, to trim CBC padding.
    pub plain_size: Option<u64>,
}

/// Domains a Voice Memos DB / audio can live in, newest layout first.
const DOMAINS: &[&str] = &["AppDomainGroup-group.com.apple.VoiceMemos", "MediaDomain"];
/// Candidate metadata DB relative paths within a domain.
const DB_PATHS: &[&str] = &["Recordings/CloudRecordings.db", "Recordings/Recordings.db"];

/// Enumerate voice memos. Pass `decryptor` for an encrypted backup, `None` for a
/// plaintext one. `work_dir` holds the transient decrypted metadata DB. Returns
/// an error only if the Manifest itself can't be opened; a missing Voice Memos DB
/// is not an error (returns whatever audio is present, possibly empty).
pub fn parse_recordings(
    backup_dir: &Path,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
) -> Result<Vec<RecordingAsset>> {
    let index = ManifestIndex::open(backup_dir, decryptor, work_dir)?;

    // All `.m4a` blobs under Recordings/, keyed by filename, across candidate
    // domains. This is the source of truth for what's actually in the backup.
    let mut audio: HashMap<String, FileEntry> = HashMap::new();
    for domain in DOMAINS {
        for entry in index.find_prefix(domain, "Recordings/")? {
            if entry.relative_path.to_ascii_lowercase().ends_with(".m4a") {
                if let Some(name) = basename(&entry.relative_path) {
                    audio.entry(name.to_string()).or_insert(entry);
                }
            }
        }
    }
    if audio.is_empty() {
        return Ok(Vec::new());
    }

    // Metadata from the first Voice Memos DB we can find + read. Keyed by the
    // audio filename so we can join it to the blobs above.
    let meta = read_metadata(&index, decryptor, work_dir).unwrap_or_default();

    // Build one asset per audio file, enriching with metadata where present. A
    // recording whose audio was evicted to iCloud (metadata but no blob) is
    // dropped — there's nothing to play.
    let mut assets = Vec::new();
    for (name, entry) in &audio {
        let m = meta.get(name);
        let (decrypt_key, plain_size) = match decryptor {
            Some(_) => {
                let (k, s) = crypto::file_key_field(&entry.file_blob)?;
                (Some(k), s)
            }
            None => (None, None),
        };
        assets.push(RecordingAsset {
            title: m.and_then(|m| m.title.clone()),
            recorded_at: m.and_then(|m| m.recorded_at),
            duration_s: m.and_then(|m| m.duration_s),
            full_path: index.blob_path(&entry.file_id),
            mime: "audio/mp4",
            decrypt_key,
            plain_size,
            relative_path: entry.relative_path.clone(),
        });
    }
    // Most-recent first; undated (metadata-less) recordings sort to the end.
    assets.sort_by_key(|a| std::cmp::Reverse(a.recorded_at));
    Ok(assets)
}

struct RecordingMeta {
    title: Option<String>,
    recorded_at: Option<i64>,
    duration_s: Option<f64>,
}

/// Read the Voice Memos metadata DB (if any), returning a map keyed by audio
/// filename. Best-effort: any miss/parse failure yields an empty map, and the
/// caller still surfaces the raw audio.
fn read_metadata(
    index: &ManifestIndex,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
) -> Option<HashMap<String, RecordingMeta>> {
    // First DB found across the candidate (domain, path) pairs.
    let db_entry = DOMAINS
        .iter()
        .flat_map(|d| DB_PATHS.iter().map(move |p| (*d, *p)))
        .find_map(|(d, p)| index.find(d, p).ok().flatten())?;

    let db_temp = work_dir.join(".recordings.db");
    index.extract_to(&db_entry, decryptor, &db_temp).ok()?;
    let result = read_recording_rows(&db_temp);
    let _ = std::fs::remove_file(&db_temp);
    result.ok()
}

/// Query the recordings table (schema/column names vary by iOS version).
fn read_recording_rows(db: &Path) -> Result<HashMap<String, RecordingMeta>> {
    // Core Data time is seconds since 2001-01-01 UTC.
    const MAC_EPOCH: f64 = 978_307_200.0;
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // Modern schema uses ZCLOUDRECORDING; older uses ZRECORDING.
    let table = ["ZCLOUDRECORDING", "ZRECORDING"]
        .into_iter()
        .find(|t| table_exists(&conn, t))
        .ok_or_else(|| crate::Error::Parse("no recordings table".into()))?;
    let cols = table_columns(&conn, table)?;

    let path = pick(&cols, &["ZPATH"]);
    let title = pick(&cols, &["ZCUSTOMLABEL", "ZTITLE", "ZLABEL"]);
    let date = pick(&cols, &["ZDATE", "ZRECORDINGDATE", "ZCREATIONDATE"]);
    let duration = pick(&cols, &["ZDURATION"]);
    // ZPATH is required to join to audio; without it there's nothing to key on.
    let Some(path) = path else {
        return Ok(HashMap::new());
    };

    let sql = format!(
        "SELECT {path}, {title}, {date}, {duration} FROM {table} WHERE {path} IS NOT NULL",
        title = title.as_deref().unwrap_or("NULL"),
        date = date.as_deref().unwrap_or("NULL"),
        duration = duration.as_deref().unwrap_or("NULL"),
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut map = HashMap::new();
    while let Some(r) = rows.next()? {
        let zpath: String = r.get(0)?;
        let Some(name) = basename(&zpath).map(str::to_string) else {
            continue;
        };
        let title: Option<String> = r
            .get::<_, Option<String>>(1)?
            .filter(|s| !s.trim().is_empty());
        let recorded_at = r
            .get::<_, Option<f64>>(2)?
            .filter(|d| *d > 0.0)
            .map(|d| (d + MAC_EPOCH) as i64);
        let duration_s = r.get::<_, Option<f64>>(3)?.filter(|d| *d > 0.0);
        map.insert(
            name,
            RecordingMeta {
                title,
                recorded_at,
                duration_s,
            },
        );
    }
    Ok(map)
}

/// The trailing filename component of a `/`-separated path.
fn basename(path: &str) -> Option<&str> {
    path.rsplit('/').next().filter(|s| !s.is_empty())
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        set.insert(r.get::<_, String>(1)?.to_uppercase());
    }
    Ok(set)
}

/// First candidate column present in `cols`, else None.
fn pick(cols: &HashSet<String>, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|c| cols.contains(**c))
        .map(|c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plaintext backup: Manifest.db with a CloudRecordings.db blob and two
    /// `.m4a` blobs, one of which the DB has metadata for.
    fn make_backup(dir: &Path) {
        let voicememos = "AppDomainGroup-group.com.apple.VoiceMemos";
        let db_id = "aa00000000000000000000000000000000000001";
        let m4a1_id = "bb00000000000000000000000000000000000002";
        let m4a2_id = "cc00000000000000000000000000000000000003";

        Connection::open(dir.join("Manifest.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
                 INSERT INTO Files VALUES ('{db_id}','{voicememos}','Recordings/CloudRecordings.db',1,NULL);
                 INSERT INTO Files VALUES ('{m4a1_id}','{voicememos}','Recordings/20240101 090000.m4a',1,NULL);
                 INSERT INTO Files VALUES ('{m4a2_id}','{voicememos}','Recordings/Untitled.m4a',1,NULL);"
            ))
            .unwrap();

        // The audio blobs on disk.
        for id in [m4a1_id, m4a2_id] {
            let sub = dir.join(&id[..2]);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join(id), b"m4a-bytes").unwrap();
        }

        // CloudRecordings.db with metadata for the first recording only.
        let sub = dir.join(&db_id[..2]);
        std::fs::create_dir_all(&sub).unwrap();
        Connection::open(sub.join(db_id))
            .unwrap()
            .execute_batch(
                "CREATE TABLE ZCLOUDRECORDING (Z_PK INTEGER PRIMARY KEY, ZCUSTOMLABEL TEXT, ZDATE REAL, ZDURATION REAL, ZPATH TEXT);
                 INSERT INTO ZCLOUDRECORDING VALUES (1, 'Morning idea', 721692800.0, 42.5, '20240101 090000.m4a');",
            )
            .unwrap();
    }

    #[test]
    fn parses_recordings_with_and_without_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path());
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        let mut assets = parse_recordings(tmp.path(), None, &work).unwrap();
        assert_eq!(assets.len(), 2);

        // Dated recording sorts first.
        let titled = &assets[0];
        assert_eq!(titled.title.as_deref(), Some("Morning idea"));
        assert_eq!(titled.recorded_at, Some(1_700_000_000)); // 721692800 + 2001-epoch
        assert_eq!(titled.duration_s, Some(42.5));
        assert_eq!(titled.mime, "audio/mp4");
        assert_eq!(titled.decrypt_key, None); // plaintext backup
        assert!(titled.relative_path.ends_with("20240101 090000.m4a"));

        // The metadata-less .m4a still surfaces, untitled.
        assets.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        let untitled = assets
            .iter()
            .find(|a| a.relative_path.ends_with("Untitled.m4a"))
            .unwrap();
        assert_eq!(untitled.title, None);
        assert_eq!(untitled.recorded_at, None);
    }

    #[test]
    fn no_recordings_domain_yields_empty() {
        let tmp = tempfile::tempdir().unwrap();
        Connection::open(tmp.path().join("Manifest.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
            )
            .unwrap();
        let assets = parse_recordings(tmp.path(), None, tmp.path()).unwrap();
        assert!(assets.is_empty());
    }
}
