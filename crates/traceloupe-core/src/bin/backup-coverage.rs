//! `backup-coverage` — why does the app show fewer photos than the phone?
//!
//! Answers that question with COUNTS ONLY. It prints integers: how many assets
//! the photo library's own catalogue knows about, versus how many of those have
//! a file physically present in the backup. No filenames, no dates, no image
//! bytes, nothing that identifies a person or a picture ever leaves this
//! process — so the output is safe to paste into an issue or a chat.
//!
//! The distinction it exists to make: `parse_camera_roll` enumerates the
//! *Manifest* (files that are in the backup) and joins `Photos.sqlite` only for
//! metadata. When iCloud Photos is on, iOS does not put the photo *files* in the
//! device backup — the catalogue still lists every asset, but the originals live
//! in iCloud. So the imported count is bounded by files-on-disk, and no parser
//! change can raise it. This tool makes that gap a measurement instead of a
//! guess.
//!
//! Usage:
//!   backup-coverage <backup_dir> [password]
//!
//! `password` is only needed for an encrypted backup. It is read as an argument
//! for one-shot convenience; it is never logged or written anywhere.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::manifest::ManifestIndex;
use traceloupe_core::{Error, Result};

/// Roots the camera-roll import enumerates, kept in the same order and spelling
/// as `parsers::camera_roll::ASSET_ROOTS` so this reports on what actually runs.
const ASSET_ROOTS: [&str; 2] = ["Media/DCIM/", "Media/PhotoData/CPLAssets/"];
const THUMB_PREFIX: &str = "Media/PhotoData/Thumbnails/V2/";

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: backup-coverage <backup_dir> [password]");
        std::process::exit(2);
    };
    let password = args.next();
    if let Err(e) = run(Path::new(&dir), password.as_deref()) {
        eprintln!("backup-coverage: {e}");
        std::process::exit(1);
    }
}

fn run(backup_dir: &Path, password: Option<&str>) -> Result<()> {
    // Temps (decrypted Manifest/Photos.sqlite) go beside the backup in a temp
    // dir we remove on the way out — never into the app's cache, which this tool
    // must not disturb.
    let work_dir = std::env::temp_dir().join("traceloupe-backup-coverage");
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| Error::Parse(format!("cannot create {}: {e}", work_dir.display())))?;

    let decryptor = match password {
        Some(p) => Some(BackupDecryptor::open(backup_dir, p)?),
        None => None,
    };
    let index = ManifestIndex::open(backup_dir, decryptor.as_ref(), &work_dir)?;

    println!("== files present in the backup (Manifest) ==");
    let manifest = manifest_counts(&index)?;
    for (label, n) in &manifest {
        println!("{label:<38} {n:>8}");
    }

    println!();
    println!("== assets the photo catalogue knows about (Photos.sqlite) ==");
    match catalogue_counts(&index, decryptor.as_ref(), &work_dir) {
        Ok(rows) => {
            for (label, n) in &rows {
                println!("{label:<38} {n:>8}");
            }
        }
        Err(e) => println!("(could not read Photos.sqlite: {e})"),
    }

    let _ = std::fs::remove_dir_all(&work_dir);
    Ok(())
}

/// Count Manifest rows per camera-roll root. These are FILES — the ceiling on
/// what the camera-roll import can ever show.
fn manifest_counts(index: &ManifestIndex) -> Result<Vec<(String, i64)>> {
    let mut out = Vec::new();
    let mut total = 0i64;
    for root in ASSET_ROOTS {
        let n = index.find_prefix("CameraRollDomain", root)?.len() as i64;
        total += n;
        out.push((format!("  {root}"), n));
    }
    out.push(("  (both roots together)".into(), total));
    let thumbs = index.find_prefix("CameraRollDomain", THUMB_PREFIX)?.len() as i64;
    out.push((format!("  {THUMB_PREFIX}"), thumbs));
    Ok(out)
}

/// Count catalogue rows. The catalogue lists every asset in the library whether
/// or not its file came along in the backup, so the gap between this and the
/// Manifest counts above is exactly the iCloud-offloaded population.
fn catalogue_counts(
    index: &ManifestIndex,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
) -> Result<Vec<(String, i64)>> {
    let entry = index
        .find("CameraRollDomain", "Media/PhotoData/Photos.sqlite")?
        .ok_or_else(|| Error::Parse("Photos.sqlite is not in this backup".into()))?;
    let db: PathBuf = work_dir.join(".coverage-photos.sqlite");
    index.extract_db(&entry, decryptor, &db)?;
    let conn = Connection::open(&db)?;

    // iOS 15+ calls it ZASSET; iOS 13/14 ZGENERICASSET. Same probe the real
    // parser does, so this reports on the table the import would actually use.
    let asset = if table_exists(&conn, "ZASSET")? {
        "ZASSET"
    } else if table_exists(&conn, "ZGENERICASSET")? {
        "ZGENERICASSET"
    } else {
        return Err(Error::Parse("no asset table in Photos.sqlite".into()));
    };
    let cols = columns(&conn, asset)?;

    let mut out = Vec::new();
    out.push((
        format!("  {asset} rows (whole library)"),
        count(&conn, &format!("SELECT COUNT(*) FROM {asset}"))?,
    ));
    if cols.contains("ZHIDDEN") {
        out.push((
            "    of which hidden".into(),
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM {asset} WHERE ZHIDDEN = 1"),
            )?,
        ));
    }
    if cols.contains("ZTRASHEDSTATE") {
        out.push((
            "    of which in Recently Deleted".into(),
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM {asset} WHERE ZTRASHEDSTATE = 1"),
            )?,
        ));
    }

    // The decisive number. ZINTERNALRESOURCE describes each asset's resources
    // (original, adjusted, thumbnail); ZLOCALAVAILABILITY = 1 means the bytes
    // were on the device. Assets with no locally-available resource are the ones
    // iCloud holds and the backup cannot contain.
    if table_exists(&conn, "ZINTERNALRESOURCE")? {
        let ir = columns(&conn, "ZINTERNALRESOURCE")?;
        if ir.contains("ZLOCALAVAILABILITY") {
            // The FK column name drifts across iOS versions (ZASSET / ZASSETFORFOO);
            // take whichever one this schema has rather than assuming.
            if let Some(fk) = ir.iter().find(|c| c.starts_with("ZASSET")) {
                out.push((
                    "  assets with a local resource".into(),
                    count(
                        &conn,
                        &format!(
                            "SELECT COUNT(DISTINCT {fk}) FROM ZINTERNALRESOURCE
                             WHERE ZLOCALAVAILABILITY = 1"
                        ),
                    )?,
                ));
                out.push((
                    "  assets with NO local resource".into(),
                    count(
                        &conn,
                        &format!(
                            "SELECT COUNT(*) FROM {asset} a WHERE NOT EXISTS (
                               SELECT 1 FROM ZINTERNALRESOURCE r
                               WHERE r.{fk} = a.Z_PK AND r.ZLOCALAVAILABILITY = 1)"
                        ),
                    )?,
                ));
            }
        }
    }
    let _ = std::fs::remove_file(&db);
    Ok(out)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.flatten().collect())
}

fn count(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}
