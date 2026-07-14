//! Manifest Index — the backbone of Phase 2 lazy access.
//!
//! An iOS backup stores every file content-addressed at `<backup>/<id[:2]>/<id>`,
//! with `Manifest.db` mapping each `(domain, relativePath)` to its `fileID` and
//! (for encrypted backups) its wrapped per-file key. This module opens that
//! manifest once — decrypting it for encrypted backups — and resolves paths to
//! files, so a native parser can pull *just* the file(s) a view needs and
//! decrypt them on demand, instead of iLEAPP's eager whole-backup pass.
//!
//! provenance: reference (own implementation) of the iTunes-backup Manifest
//! layout; decryption via [`crate::crypto`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::crypto::BackupDecryptor;
use crate::{Error, Result};

/// Makes each decrypted-manifest temp file name unique, so two `ManifestIndex`
/// instances sharing a `work_dir` (e.g. concurrent parsers) never read/delete
/// each other's file.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// One file recorded in `Manifest.db`.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Content-addressed name; the blob lives at `<backup>/<file_id[:2]>/<file_id>`.
    pub file_id: String,
    pub domain: String,
    pub relative_path: String,
    /// The Manifest `file` column (NSKeyedArchiver metadata carrying the wrapped
    /// key + real size). Empty for plaintext backups / directory entries.
    pub file_blob: Vec<u8>,
}

/// Opens a backup's `Manifest.db` and resolves paths to on-disk files. Cheap to
/// hold; reads a single file only when asked (the lazy primitive).
pub struct ManifestIndex {
    conn: Connection,
    backup_dir: PathBuf,
    /// Decrypted-manifest temp file to clean up on drop (encrypted backups only).
    temp: Option<PathBuf>,
}

impl ManifestIndex {
    /// Open the index. Pass `decryptor` for an encrypted backup — its keys
    /// decrypt `Manifest.db` to a short-lived temp under `work_dir`. Pass `None`
    /// for a plaintext backup (its own `Manifest.db` is read directly).
    pub fn open(
        backup_dir: &Path,
        decryptor: Option<&BackupDecryptor>,
        work_dir: &Path,
    ) -> Result<Self> {
        let (manifest_path, temp) = if let Some(dec) = decryptor {
            std::fs::create_dir_all(work_dir).map_err(|e| Error::io(work_dir, e))?;
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let tmp = work_dir.join(format!(".manifest-index-{seq}.db"));
            // 0600: the decrypted manifest holds the file listing + wrapped keys.
            crate::write_private(&tmp, &dec.decrypt_manifest_db()?)
                .map_err(|e| Error::io(&tmp, e))?;
            (tmp.clone(), Some(tmp))
        } else {
            (backup_dir.join("Manifest.db"), None)
        };
        let conn = Connection::open_with_flags(&manifest_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            conn,
            backup_dir: backup_dir.to_path_buf(),
            temp,
        })
    }

    fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<FileEntry> {
        Ok(FileEntry {
            file_id: row.get(0)?,
            domain: row.get(1)?,
            relative_path: row.get(2)?,
            file_blob: row.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
        })
    }

    /// The file at exactly `domain`/`relative_path`, if present.
    pub fn find(&self, domain: &str, relative_path: &str) -> Result<Option<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT fileID, domain, relativePath, file FROM Files
             WHERE domain = ?1 AND relativePath = ?2 LIMIT 1",
        )?;
        stmt.query_row(rusqlite::params![domain, relative_path], Self::row_to_entry)
            .optional()
            .map_err(Into::into)
    }

    /// Every file under `domain` whose relativePath starts with `prefix`,
    /// ordered by path.
    pub fn find_prefix(&self, domain: &str, prefix: &str) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT fileID, domain, relativePath, file FROM Files
             WHERE domain = ?1 AND relativePath LIKE ?2 || '%'
             ORDER BY relativePath",
        )?;
        let rows = stmt.query_map(rusqlite::params![domain, prefix], Self::row_to_entry)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Every file whose `relativePath` matches a SQL `LIKE` pattern, across ALL
    /// domains, ordered by path. Use when a file's domain varies by iOS version
    /// (e.g. Voice Memos: `AppDomainGroup-group.com.apple.VoiceMemos` vs
    /// `MediaDomain`) so the caller can match on path alone.
    pub fn find_relative_like(&self, pattern: &str) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT fileID, domain, relativePath, file FROM Files
             WHERE relativePath LIKE ?1
             ORDER BY relativePath",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern], Self::row_to_entry)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// The on-disk (possibly ciphertext) path of a backed-up file. `file_id` is
    /// from the untrusted Manifest; a non-hex value (`../..`, `/etc/passwd`) would
    /// escape `backup_dir` via `join`, so anything that isn't a content-addressed
    /// id resolves to a nonexistent in-dir path (a later read simply fails).
    pub fn blob_path(&self, file_id: &str) -> PathBuf {
        if !crate::crypto::is_valid_file_id(file_id) {
            return self.backup_dir.join("__invalid_file_id__");
        }
        self.backup_dir.join(&file_id[..2]).join(file_id)
    }

    /// Read a file's PLAINTEXT bytes into memory, decrypting on the fly for an
    /// encrypted backup. Only the requested file is touched.
    pub fn read_bytes(
        &self,
        entry: &FileEntry,
        decryptor: Option<&BackupDecryptor>,
    ) -> Result<Vec<u8>> {
        match decryptor {
            Some(dec) => dec.decrypt_file(&entry.file_blob, &entry.file_id),
            None => {
                let path = self.blob_path(&entry.file_id);
                std::fs::read(&path).map_err(|e| Error::io(&path, e))
            }
        }
    }

    /// Write a file's plaintext bytes to `dest` (e.g. so rusqlite can open a
    /// decrypted SQLite artifact like `sms.db`).
    pub fn extract_to(
        &self,
        entry: &FileEntry,
        decryptor: Option<&BackupDecryptor>,
        dest: &Path,
    ) -> Result<()> {
        let bytes = self.read_bytes(entry, decryptor)?;
        // 0600: `dest` is decrypted plaintext (sms.db, NoteStore.sqlite, …).
        crate::write_private(dest, &bytes).map_err(|e| Error::io(dest, e))
    }
}

impl Drop for ManifestIndex {
    fn drop(&mut self) {
        if let Some(t) = &self.temp {
            let _ = std::fs::remove_file(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal plaintext backup: a `Manifest.db` with a `Files` table and
    /// one content-addressed blob on disk.
    fn make_backup(dir: &Path) {
        let manifest = dir.join("Manifest.db");
        let conn = Connection::open(&manifest).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('ab00000000000000000000000000000000000001','HomeDomain','Library/SMS/sms.db',1,NULL);
             INSERT INTO Files VALUES ('ab00000000000000000000000000000000000002','HomeDomain','Library/SMS/Attachments/x.jpg',1,NULL);",
        )
        .unwrap();
        for (id, bytes) in [
            (
                "ab00000000000000000000000000000000000001",
                b"sqlite-bytes-here".as_slice(),
            ),
            (
                "ab00000000000000000000000000000000000002",
                b"jpg-bytes".as_slice(),
            ),
        ] {
            let sub = dir.join(&id[..2]);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join(id), bytes).unwrap();
        }
    }

    #[test]
    fn resolves_and_reads_files_from_a_plaintext_backup() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path());
        let idx = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();

        // Exact lookup + read.
        let sms = idx
            .find("HomeDomain", "Library/SMS/sms.db")
            .unwrap()
            .expect("sms.db present");
        assert_eq!(sms.file_id, "ab00000000000000000000000000000000000001");
        assert_eq!(idx.read_bytes(&sms, None).unwrap(), b"sqlite-bytes-here");

        // Prefix lookup returns both files under the SMS library.
        let under = idx.find_prefix("HomeDomain", "Library/SMS/").unwrap();
        assert_eq!(under.len(), 2);

        // Extract to a file (what a native SQLite parser does).
        let out = tmp.path().join("out.db");
        idx.extract_to(&sms, None, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"sqlite-bytes-here");

        // Misses.
        assert!(idx.find("HomeDomain", "nope").unwrap().is_none());
        assert!(idx
            .find("WrongDomain", "Library/SMS/sms.db")
            .unwrap()
            .is_none());
    }
}
