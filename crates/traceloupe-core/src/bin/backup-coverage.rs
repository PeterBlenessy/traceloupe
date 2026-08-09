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

use std::collections::{HashMap, HashSet};
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

    println!();
    survey_media_dirs(&index, backup_dir)?;

    let _ = std::fs::remove_dir_all(&work_dir);
    Ok(())
}

/// Where image/video files ACTUALLY live, discovered rather than assumed.
///
/// `ASSET_ROOTS` is a hardcoded guess, and a wrong guess is invisible — reading
/// only `DCIM/` silently dropped 42% of one public backup's assets and nothing
/// reported it. So this sweeps every domain, classifies by extension, and groups
/// by directory shape. A directory holding thousands of images that the camera
/// roll does not cover is a gap, and it shows up here as a row marked `MISSED`
/// instead of as a number nobody can explain.
fn survey_media_dirs(index: &ManifestIndex, backup_dir: &Path) -> Result<()> {
    let mut dirs: HashMap<(String, String), i64> = HashMap::new();
    let mut exts: HashMap<String, i64> = HashMap::new();
    // Thumbnails mirror their asset's own path, so the PARENT directory of each
    // thumbnail names one asset. Counting distinct parents answers the question
    // that decides whether offloaded assets can be shown as real pictures or
    // only as placeholders: how many of the library's assets have a thumbnail
    // sitting in this backup, regardless of whether the original came along.
    let mut thumb_assets: HashSet<String> = HashSet::new();
    index.for_each_path(|domain, rel| {
        if let Some(asset) = thumb_asset(&rel) {
            thumb_assets.insert(asset.to_string());
        }
        let Some(ext) = media_ext(&rel) else { return };
        *exts.entry(ext).or_default() += 1;
        *dirs.entry((domain, generalize(&rel))).or_default() += 1;
    })?;

    println!("== thumbnail coverage ==");
    println!(
        "{:>8}  distinct assets that have a thumbnail in this backup",
        thumb_assets.len()
    );
    println!();

    println!("== where image/video files actually live (all domains) ==");
    println!("   'MISSED' = the CAMERA ROLL does not read it. App media (SMS");
    println!("   attachments, WhatsApp, TikTok…) is read by the app parsers");
    println!("   instead, so a MISSED app directory is not necessarily a gap.");
    let mut rows: Vec<_> = dirs.into_iter().collect();
    rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let (mut covered, mut thumbs, mut missed) = (0i64, 0i64, 0i64);
    for ((domain, dir), n) in &rows {
        match coverage(domain, dir) {
            Coverage::Asset => covered += n,
            Coverage::Thumbnail => thumbs += n,
            Coverage::Missed => missed += n,
        }
    }
    for ((domain, dir), n) in rows.iter().take(30) {
        println!("{n:>8}  {}  {domain}  {dir}", coverage(domain, dir).tag());
    }
    if rows.len() > 30 {
        println!("   … and {} more directories", rows.len() - 30);
    }
    println!("{covered:>8}  total read as camera-roll assets");
    println!("{thumbs:>8}  total read as thumbnails");
    println!("{missed:>8}  total NOT read by the camera roll");

    println!();
    probe_missed(index, backup_dir, &rows)?;

    println!();
    println!("== file types found, and whether the camera roll decodes them ==");
    let mut es: Vec<_> = exts.into_iter().collect();
    es.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (ext, n) in &es {
        let tag = if DECODED.contains(&ext.as_str()) {
            "  ok  "
        } else {
            "MISSED"
        };
        println!("{n:>8}  {tag}  .{ext}");
    }
    Ok(())
}

/// The asset a thumbnail belongs to, as its path relative to the thumbnail root.
/// `…/V2/DCIM/100APPLE/IMG_1.HEIC/5005.JPG` names the asset `DCIM/100APPLE/IMG_1.HEIC`
/// — several thumbnail sizes share one parent, so distinct parents count assets,
/// not files.
fn thumb_asset(rel: &str) -> Option<&str> {
    let rest = rel.strip_prefix(THUMB_PREFIX)?;
    let (asset, _size) = rest.rsplit_once('/')?;
    Some(asset)
}

/// How many files to stat per directory before extrapolating. A backup can hold
/// hundreds of thousands; the median is stable long before that.
const PROBE_LIMIT: usize = 4000;

/// Size up the biggest MISSED directories, because a count alone cannot tell a
/// pile of full-resolution originals from a pile of derivatives — and only the
/// former is a photo we are failing to show. Median bytes settles it: originals
/// run to megabytes, derivatives and caches to tens of kilobytes.
///
/// Sizes come from the on-disk blob rather than the Manifest's `Size` field. For
/// an encrypted backup that overstates by up to one 16-byte AES block, which is
/// irrelevant at this resolution and avoids decrypting anything.
fn probe_missed(
    index: &ManifestIndex,
    backup_dir: &Path,
    rows: &[((String, String), i64)],
) -> Result<()> {
    println!("== what the biggest MISSED directories actually contain ==");
    println!("   median size says whether these are originals or derivatives.");
    let mut shown = 0;
    for ((domain, dir), _) in rows.iter() {
        if coverage(domain, dir) != Coverage::Missed {
            continue;
        }
        if shown >= 10 {
            break;
        }
        // Generalized dirs carry `*` where shard names were collapsed; the literal
        // prefix up to the first `*` is what the Manifest can actually match.
        let prefix = dir.split('*').next().unwrap_or(dir);
        let entries = index.find_prefix(domain, prefix)?;
        let mut sizes: Vec<u64> = Vec::new();
        let mut capped = false;
        for e in entries.iter() {
            if media_ext(&e.relative_path).is_none() {
                continue;
            }
            if sizes.len() >= PROBE_LIMIT {
                capped = true;
                break;
            }
            if e.file_id.len() >= 2 {
                let p = backup_dir.join(&e.file_id[..2]).join(&e.file_id);
                if let Ok(m) = std::fs::metadata(&p) {
                    sizes.push(m.len());
                }
            }
        }
        if sizes.is_empty() {
            continue;
        }
        sizes.sort_unstable();
        let median = sizes[sizes.len() / 2];
        let total: u64 = sizes.iter().sum();
        // Say so when the sample was capped — a silent cap reads as "measured
        // everything" and would make the totals quietly wrong.
        let note = if capped {
            format!(" (median from first {PROBE_LIMIT})")
        } else {
            String::new()
        };
        println!(
            "  {:>7} files  median {:>8}  sampled {:>9}  {domain}  {dir}{note}",
            sizes.len(),
            human(median),
            human(total),
        );
        shown += 1;
    }
    Ok(())
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// What the camera-roll import does with a directory's media.
#[derive(PartialEq, Debug)]
enum Coverage {
    /// Enumerated as assets — these become rows in Photos.
    Asset,
    /// Read, but only as thumbnails paired to an asset. Not a gap: calling it
    /// one would bury the real gaps under a guaranteed false alarm.
    Thumbnail,
    /// Not read at all. This is the row worth acting on.
    Missed,
}

impl Coverage {
    fn tag(&self) -> &'static str {
        match self {
            Coverage::Asset => "  ok  ",
            Coverage::Thumbnail => " thumb",
            Coverage::Missed => "MISSED",
        }
    }
}

fn coverage(domain: &str, dir: &str) -> Coverage {
    if domain != "CameraRollDomain" {
        return Coverage::Missed;
    }
    if dir.starts_with(THUMB_PREFIX.trim_end_matches('/')) {
        return Coverage::Thumbnail;
    }
    if ASSET_ROOTS
        .iter()
        .any(|r| dir.starts_with(r.trim_end_matches('/')))
    {
        return Coverage::Asset;
    }
    Coverage::Missed
}

/// Extensions the camera-roll import can actually turn into a thumbnail today
/// (mirrors `camera_roll::classify`). Anything outside this set that shows up in
/// quantity is a decoder gap.
const DECODED: [&str; 10] = [
    "heic", "heif", "jpg", "jpeg", "png", "gif", "mov", "mp4", "m4v", "avif",
];

/// Media extensions worth counting — deliberately WIDER than what we decode, so
/// formats we cannot yet render still surface as a gap rather than vanishing.
fn media_ext(rel: &str) -> Option<String> {
    let lower = rel.to_ascii_lowercase();
    let ext = lower.rsplit('.').next()?;
    const MEDIA: [&str; 22] = [
        "heic", "heif", "avif", "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "dng",
        "cr2", "nef", "arw", "raf", "mov", "mp4", "m4v", "avi", "3gp", "webm",
    ];
    MEDIA.contains(&ext).then(|| ext.to_string())
}

/// Collapse a path to its directory shape: drop the filename, keep at most four
/// components, and replace shard/bucket names (`100APPLE`, `group1`, hex fanout)
/// with `*`. Without this the report is thousands of near-identical rows and the
/// real outlier directory hides among them.
fn generalize(rel: &str) -> String {
    let dir = match rel.rsplit_once('/') {
        Some((d, _)) => d,
        None => return "(root)".into(),
    };
    dir.split('/')
        .take(4)
        .map(|c| {
            let shard = (c.chars().any(|ch| ch.is_ascii_digit())
                && c.chars()
                    .all(|ch| ch.is_ascii_digit() || ch.is_ascii_uppercase()))
                || c.strip_prefix("group")
                    .is_some_and(|r| r.parse::<u32>().is_ok())
                || (c.len() >= 8 && c.chars().all(|ch| ch.is_ascii_hexdigit()));
            if shard && c != "V2" {
                "*"
            } else {
                c
            }
        })
        .collect::<Vec<_>>()
        .join("/")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Shard directories must collapse, or the survey is thousands of one-row
    /// entries and the outlier directory it exists to reveal is invisible.
    #[test]
    fn collapses_shard_directories_but_keeps_meaningful_names() {
        assert_eq!(generalize("Media/DCIM/100APPLE/IMG_1.HEIC"), "Media/DCIM/*");
        assert_eq!(
            generalize("Media/PhotoData/CPLAssets/group1/IMG_1.HEIC"),
            "Media/PhotoData/CPLAssets/*"
        );
        // `V2` has a digit but names a real directory — collapsing it would merge
        // thumbnails into whatever else lives under PhotoData.
        assert_eq!(
            generalize("Media/PhotoData/Thumbnails/V2/DCIM/1.JPG"),
            "Media/PhotoData/Thumbnails/V2"
        );
    }

    /// Thumbnails are READ, just not as assets. Reporting them as a gap would
    /// cry wolf on every single backup and train the reader to skip the section.
    #[test]
    fn thumbnails_are_not_reported_as_a_gap() {
        assert_eq!(
            coverage("CameraRollDomain", "Media/PhotoData/Thumbnails/V2"),
            Coverage::Thumbnail
        );
        assert_eq!(
            coverage("CameraRollDomain", "Media/DCIM/*"),
            Coverage::Asset
        );
        // My Photo Stream is a real iOS location the camera roll does not read.
        assert_eq!(
            coverage("CameraRollDomain", "Media/PhotoStreamsData/*/*"),
            Coverage::Missed
        );
        assert_eq!(
            coverage("MediaDomain", "Media/Recordings"),
            Coverage::Missed
        );
    }

    /// The survey counts formats we cannot yet decode on purpose — a RAW or WebP
    /// pile we silently skip is exactly the kind of gap this tool is for.
    #[test]
    fn counts_media_we_cannot_decode() {
        assert_eq!(media_ext("a/b/RAW_1.DNG").as_deref(), Some("dng"));
        assert_eq!(media_ext("a/b/x.webp").as_deref(), Some("webp"));
        assert!(!DECODED.contains(&"dng"));
        assert_eq!(media_ext("a/b/notes.sqlite"), None);
    }

    /// Several thumbnail sizes share one parent directory, so counting FILES
    /// would inflate library coverage severalfold. Distinct parents count assets.
    #[test]
    fn thumbnails_count_assets_not_files() {
        let a = "Media/PhotoData/Thumbnails/V2/DCIM/100APPLE/IMG_1.HEIC/5005.JPG";
        let b = "Media/PhotoData/Thumbnails/V2/DCIM/100APPLE/IMG_1.HEIC/5003.JPG";
        assert_eq!(thumb_asset(a), Some("DCIM/100APPLE/IMG_1.HEIC"));
        assert_eq!(thumb_asset(a), thumb_asset(b));
        assert_eq!(thumb_asset("Media/DCIM/100APPLE/IMG_1.HEIC"), None);
    }

    /// Byte counts are the whole point of the size probe; a wrong unit boundary
    /// would turn a 900 KB derivative into something that reads like an original.
    #[test]
    fn human_sizes_do_not_slip_a_unit() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1024 * 1024), "1.0 MB");
        assert_eq!(human(3 * 1024 * 1024 / 2), "1.5 MB");
    }
}
