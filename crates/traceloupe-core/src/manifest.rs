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

use std::collections::HashMap;
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
/// Give the manifest at `path` an index on `(domain, relativePath)`, returning
/// the path to use. Best effort: on any failure the caller keeps the original
/// path and merely stays slow, because a missing index is a performance problem
/// and refusing to open the backup over one would be a correctness problem.
///
/// A plaintext backup's Manifest.db belongs to the user and is never written to
/// — it is copied into the work dir first. The copy is reused across the many
/// `ManifestIndex::open` calls one import makes, so the cost is paid once.
fn ensure_indexed(path: &Path, work_dir: &Path) -> Option<PathBuf> {
    const IDX: &str = "CREATE INDEX IF NOT EXISTS idx_files_domain_path
                       ON Files(domain, relativePath)";
    // Ours already (decrypted temp): index in place.
    if path.starts_with(work_dir) {
        let conn = Connection::open(path).ok()?;
        conn.execute_batch(IDX).ok()?;
        return Some(path.to_path_buf());
    }
    let copy = work_dir.join(".manifest-indexed.db");
    let fresh = std::fs::metadata(&copy).ok().and_then(|c| {
        let src = std::fs::metadata(path).ok()?;
        Some(c.len() > 0 && c.modified().ok()? >= src.modified().ok()?)
    });
    if fresh != Some(true) {
        std::fs::create_dir_all(work_dir).ok()?;
        std::fs::copy(path, &copy).ok()?;
    }
    let conn = Connection::open(&copy).ok()?;
    conn.execute_batch(IDX).ok()?;
    Some(copy)
}

/// The exclusive upper bound for a prefix range: the prefix with its last byte
/// incremented. `"abc"` → `"abd"`, so `>= "abc" AND < "abd"` is exactly
/// "starts with abc". Falls back to an unbounded high sentinel when the last
/// byte cannot be incremented, which keeps the query correct rather than empty.
fn prefix_upper_bound(prefix: &str) -> String {
    let mut b = prefix.as_bytes().to_vec();
    while let Some(last) = b.pop() {
        if last < 0xFF {
            b.push(last + 1);
            return String::from_utf8_lossy(&b).into_owned();
        }
    }
    // Empty prefix (or all-0xFF): match everything from here up.
    format!("{prefix}\u{10FFFF}")
}

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
        // An iOS Manifest.db declares `Files(fileID PRIMARY KEY, domain,
        // relativePath, …)` and NOTHING else — no index on the two columns every
        // lookup filters by. So each `find`/`find_prefix` scans the whole table,
        // and a caller resolving per item (note images, app attachments) pays one
        // scan per item. That is what made import look hung rather than slow.
        //
        // Build the index once. The decrypted temp is ours to write to; the
        // backup's own Manifest.db is NOT, so for a plaintext backup we index a
        // copy in the work dir instead of touching the user's file.
        let manifest_path = ensure_indexed(&manifest_path, work_dir).unwrap_or(manifest_path);
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
        // A RANGE, not `LIKE ?||'%'`. SQLite will not use an index for LIKE
        // (it is case-insensitive by default, so the optimisation is off), which
        // left every prefix search scanning the whole table even once the index
        // existed — measured at 47 s for 2,000 searches over 300,000 files. A
        // half-open range on the same column is an index seek.
        let mut stmt = self.conn.prepare(
            "SELECT fileID, domain, relativePath, file FROM Files
             WHERE domain = ?1 AND relativePath >= ?2 AND relativePath < ?3
             ORDER BY relativePath",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![domain, prefix, prefix_upper_bound(prefix)],
            Self::row_to_entry,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// One entry by its content-addressed id. `fileID` is the table's primary
    /// key, so this is an index seek — unlike a basename `LIKE`, which cannot
    /// use an index at all.
    pub fn find_by_file_id(&self, file_id: &str) -> Result<Option<FileEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT fileID, domain, relativePath, file FROM Files WHERE fileID = ?1")?;
        let mut rows = stmt.query_map([file_id], Self::row_to_entry)?;
        Ok(match rows.next() {
            Some(r) => Some(r?),
            None => None,
        })
    }

    /// Basename → the file that owns it, but ONLY where exactly one file does.
    ///
    /// The attachment fallback needs "find the single file with this name".
    /// Doing that as `LIKE '%/name'` per attachment is a full Manifest scan
    /// each time — on a large backup that is hundreds of thousands of rows
    /// times thousands of attachments, which is why import appeared to hang.
    /// This pays for ONE scan and answers every lookup from memory.
    ///
    /// Ambiguous basenames map to `None` and stay that way: attaching the
    /// wrong image to a message is a silent, confident error, and worse than
    /// the missing file it would replace.
    pub fn unique_basenames(&self) -> Result<HashMap<String, Option<String>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT fileID, relativePath FROM Files WHERE relativePath != ''")?;
        let mut rows = stmt.query([])?;
        let mut map: HashMap<String, Option<String>> = HashMap::new();
        while let Some(r) = rows.next()? {
            let file_id: String = r.get(0)?;
            let rel: String = r.get(1)?;
            let Some(base) = rel.rsplit('/').next() else {
                continue;
            };
            if base.is_empty() {
                continue;
            }
            match map.get_mut(base) {
                None => {
                    map.insert(base.to_string(), Some(file_id));
                }
                // Seen before → ambiguous. Drop the id; never resurrect it.
                Some(slot) => *slot = None,
            }
        }
        Ok(map)
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

    /// Every `(domain, relativePath)` in the backup, for the Security Check
    /// manifest sweep. Streams via the callback so the whole file list (tens of
    /// thousands of rows) is never held in memory at once.
    pub fn for_each_path(&self, mut f: impl FnMut(String, String)) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT domain, relativePath FROM Files WHERE relativePath != ''")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            f(row.get(0)?, row.get(1)?);
        }
        Ok(())
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

    /// Like [`extract_to`], but for a **WAL-mode SQLite database**: also extract
    /// the `-wal`/`-shm` sidecars (iOS backs them up as separate Manifest entries)
    /// and fold the WAL into the main file, so a read-only open sees every
    /// committed row. Without this, data still in the WAL at backup time is
    /// silently dropped — e.g. all of Safari `History.db`'s visits lived in its WAL.
    /// Sidecars are best-effort; the checkpoint leaves a self-contained DB.
    pub fn extract_db(
        &self,
        entry: &FileEntry,
        decryptor: Option<&BackupDecryptor>,
        dest: &Path,
    ) -> Result<()> {
        self.extract_to(entry, decryptor, dest)?;
        for suffix in ["-wal", "-shm"] {
            let rel = format!("{}{suffix}", entry.relative_path);
            if let Ok(Some(side)) = self.find(&entry.domain, &rel) {
                let side_dest = std::path::PathBuf::from(format!("{}{suffix}", dest.display()));
                let _ = self.extract_to(&side, decryptor, &side_dest);
            }
        }
        // Checkpoint: switching journal_mode folds the WAL into the main DB and
        // removes the sidecars, leaving a plain file for the read-only parser.
        if let Ok(conn) = Connection::open(dest) {
            let _ = conn.pragma_update(None, "journal_mode", "DELETE");
        }
        Ok(())
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

#[cfg(test)]
mod lookup_bench {
    use super::*;

    /// An iOS `Manifest.db` ships no index on `(domain, relativePath)`, so every
    /// lookup was a full scan — and a caller resolving per item (note images,
    /// app attachments) paid one scan per item. Compares the OLD shape (no
    /// index, `LIKE ?||'%'`) against what `ManifestIndex` now does.
    #[test]
    #[ignore = "measurement, not an assertion — run with --ignored --nocapture"]
    fn measure_lookup_old_shape_vs_indexed_range() {
        const FILES: usize = 300_000;
        const LOOKUPS: usize = 2_000;
        const DOMAIN: &str = "AppDomainGroup-group.com.apple.notes";

        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().join("Manifest.db");
        let conn = Connection::open(&raw).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
        )
        .unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            let mut ins = tx
                .prepare("INSERT INTO Files VALUES (?1,?2,?3,1,NULL)")
                .unwrap();
            for i in 0..FILES {
                ins.execute(rusqlite::params![
                    format!("{i:08x}"),
                    if i % 3 == 0 { DOMAIN } else { "MediaDomain" },
                    format!("Accounts/A1/Media/{i:06}/img_{i:06}.png")
                ])
                .unwrap();
            }
            drop(ins);
            tx.commit().unwrap();
        }

        // OLD: no index, LIKE prefix — exactly what shipped before.
        let t = std::time::Instant::now();
        {
            let mut st = conn
                .prepare(
                    "SELECT fileID FROM Files
                     WHERE domain = ?1 AND relativePath LIKE ?2 || '%'",
                )
                .unwrap();
            for i in 0..LOOKUPS {
                let _: Vec<String> = st
                    .query_map(
                        rusqlite::params![DOMAIN, format!("Accounts/A1/Media/{:06}/", i * 3)],
                        |r| r.get(0),
                    )
                    .unwrap()
                    .flatten()
                    .collect();
            }
        }
        let old = t.elapsed();
        drop(conn);

        // NEW: ManifestIndex builds the index on open and searches by range.
        let t = std::time::Instant::now();
        let index = ManifestIndex::open(tmp.path(), None, &tmp.path().join("w")).unwrap();
        let open_cost = t.elapsed();
        let t = std::time::Instant::now();
        for i in 0..LOOKUPS {
            let _ = index
                .find_prefix(DOMAIN, &format!("Accounts/A1/Media/{:06}/", i * 3))
                .unwrap();
        }
        let new = t.elapsed();

        println!("{FILES} files, {LOOKUPS} prefix lookups");
        println!("  old (no index, LIKE)      : {old:?}");
        println!("  new (indexed copy, range) : {open_cost:?} open + {new:?} lookups");
        println!(
            "  speedup: {:.0}x",
            old.as_secs_f64() / (open_cost + new).as_secs_f64()
        );
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    /// `find_prefix` became a half-open RANGE so it can use an index. The bound
    /// has to be exact: too wide and a sibling directory leaks in, too narrow
    /// and the last file of a directory disappears.
    #[test]
    fn a_prefix_range_takes_the_directory_and_nothing_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = Connection::open(tmp.path().join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('a','D','Media/DCIM/a.heic',1,NULL);
             INSERT INTO Files VALUES ('z','D','Media/DCIM/zzz.heic',1,NULL);
             -- Sorts immediately after 'Media/DCIM/' and must NOT be included.
             INSERT INTO Files VALUES ('s','D','Media/DCIM0/other.heic',1,NULL);
             INSERT INTO Files VALUES ('o','D','Media/Other/x.heic',1,NULL);",
        )
        .unwrap();
        let index = ManifestIndex::open(tmp.path(), None, &tmp.path().join("w")).unwrap();
        let got: Vec<String> = index
            .find_prefix("D", "Media/DCIM/")
            .unwrap()
            .into_iter()
            .map(|e| e.relative_path)
            .collect();
        assert_eq!(got, vec!["Media/DCIM/a.heic", "Media/DCIM/zzz.heic"]);
    }

    #[test]
    fn the_upper_bound_increments_the_last_byte() {
        assert_eq!(prefix_upper_bound("abc"), "abd");
        assert_eq!(prefix_upper_bound("Media/DCIM/"), "Media/DCIM0");
    }
}

#[cfg(test)]
mod basename_tests {
    use super::*;

    /// The fallback exists to rescue attachments stored outside the expected
    /// directory, but it must REFUSE when a basename is shared: attaching the
    /// wrong image to a message is a silent, confident error, and worse than the
    /// missing file it replaces. Speeding the lookup up must not relax that.
    #[test]
    fn a_shared_basename_is_refused_not_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = Connection::open(tmp.path().join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
             INSERT INTO Files VALUES ('a1','MediaDomain','Media/one/IMG_0001.HEIC',1,NULL);
             INSERT INTO Files VALUES ('b2','MediaDomain','Media/two/IMG_0001.HEIC',1,NULL);
             INSERT INTO Files VALUES ('c3','MediaDomain','Media/three/UNIQUE.HEIC',1,NULL);",
        )
        .unwrap();
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let map = index.unique_basenames().unwrap();

        assert_eq!(
            map.get("IMG_0001.HEIC"),
            Some(&None),
            "a basename owned by two files must resolve to nothing"
        );
        assert_eq!(map.get("UNIQUE.HEIC"), Some(&Some("c3".to_string())));
        assert_eq!(map.get("NEVER.HEIC"), None);

        // And the id it hands back must actually resolve to that file.
        let e = index.find_by_file_id("c3").unwrap().unwrap();
        assert_eq!(e.relative_path, "Media/three/UNIQUE.HEIC");
    }
}

#[cfg(test)]
mod basename_bench {
    use super::*;

    /// The attachment fallback used to run a `LIKE '%/name'` per attachment —
    /// a full Manifest scan each time. This measures both shapes at a scale a
    /// real device reaches, because "it felt slow" is not a diagnosis.
    #[test]
    #[ignore = "measurement, not an assertion — run with --ignored --nocapture"]
    fn measure_basename_fallback_shapes() {
        const FILES: usize = 300_000;
        const LOOKUPS: usize = 3_000;

        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("Manifest.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
        )
        .unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            let mut ins = tx
                .prepare("INSERT INTO Files VALUES (?1,'MediaDomain',?2,1,NULL)")
                .unwrap();
            for i in 0..FILES {
                ins.execute(rusqlite::params![
                    format!("{i:08x}"),
                    format!("Media/dir{}/file_{i:06}.dat", i % 997)
                ])
                .unwrap();
            }
            drop(ins);
            tx.commit().unwrap();
        }
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();

        let t = std::time::Instant::now();
        for i in 0..LOOKUPS {
            let base = format!("file_{:06}.dat", i * 7);
            let _ = index.find_relative_like(&format!("%/{base}")).unwrap();
        }
        let old = t.elapsed();

        let t = std::time::Instant::now();
        let map = index.unique_basenames().unwrap();
        let build = t.elapsed();
        let t = std::time::Instant::now();
        for i in 0..LOOKUPS {
            let base = format!("file_{:06}.dat", i * 7);
            if let Some(Some(id)) = map.get(&base) {
                let _ = index.find_by_file_id(id).unwrap();
            }
        }
        let new = t.elapsed();

        println!("{FILES} files, {LOOKUPS} attachment lookups");
        println!("  per-attachment LIKE scan : {old:?}");
        println!("  one scan + hash + seek   : {build:?} build + {new:?} lookups");
        println!(
            "  speedup on the lookup phase: {:.0}x",
            old.as_secs_f64() / (build + new).as_secs_f64()
        );
    }
}
