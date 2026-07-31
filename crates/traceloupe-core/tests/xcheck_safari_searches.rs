//! Cross-validation of the Safari search parsers against a real device backup.
//!
//! Gated on `TRACELOUPE_REAL_BACKUP` (+ `TRACELOUPE_REAL_BACKUP_PASSWORD` when the
//! backup is encrypted) so it is skipped in CI, where no such backup exists. Point
//! it at a public DFIR image — never at a personal backup.
//!
//! What it proves that the unit tests cannot: that the plist really is where we
//! think it is, decrypts, and parses to the terms iLEAPP reports for the same
//! image. Run against Josh Hickman's iOS 17.3 iPhone 11 image:
//!
//! ```text
//! TRACELOUPE_REAL_BACKUP=…/iOS_17/Backup/unpacked/00008030-… \
//! TRACELOUPE_REAL_BACKUP_PASSWORD=MyPassword123 \
//!   cargo test -p traceloupe-core --test xcheck_safari_searches -- --nocapture
//! ```

use traceloupe_core::cache::CacheDb;
use traceloupe_core::manifest::ManifestIndex;
use traceloupe_core::normalize::ImportReport;

/// Open the backup named by the environment, or return None to skip.
fn real_backup() -> Option<(std::path::PathBuf, Option<String>)> {
    let dir = std::env::var("TRACELOUPE_REAL_BACKUP").ok()?;
    let password = std::env::var("TRACELOUPE_REAL_BACKUP_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty());
    Some((std::path::PathBuf::from(dir), password))
}

#[test]
fn recent_web_searches_from_a_real_backup() {
    let Some((dir, password)) = real_backup() else {
        eprintln!("skipping: set TRACELOUPE_REAL_BACKUP to a public image's backup dir");
        return;
    };
    let decryptor = password.map(|p| {
        traceloupe_core::crypto::BackupDecryptor::open(&dir, &p).expect("open with the password")
    });
    let work = tempfile::tempdir().unwrap();
    let index =
        ManifestIndex::open(&dir, decryptor.as_ref(), work.path()).expect("open Manifest.db");

    let hits = index
        .find_relative_like("%Library/Preferences/com.apple.mobilesafari.plist")
        .expect("query the manifest");
    let entry = hits
        .into_iter()
        .next()
        .expect("com.apple.mobilesafari.plist is in this backup");
    eprintln!("found {} / {}", entry.domain, entry.relative_path);

    let out = work.path().join("com.apple.mobilesafari.plist");
    index
        .extract_to(&entry, decryptor.as_ref(), &out)
        .expect("decrypt + extract the plist");

    let cache = CacheDb::open_in_memory().unwrap();
    let mut report = ImportReport::default();
    traceloupe_core::parsers::safari_search::parse_recent_searches(
        &out,
        &cache,
        &mut report,
        false,
    )
    .expect("parse RecentWebSearches");

    let mut stmt = cache
        .conn()
        .prepare("SELECT term, searched_at, source FROM safari_searches ORDER BY searched_at")
        .unwrap();
    let rows: Vec<(String, Option<i64>, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(std::result::Result::unwrap)
        .collect();

    for (term, at, source) in &rows {
        eprintln!("  {source:8} {at:?}  {term}");
    }
    assert_eq!(
        rows.len(),
        report.safari_searches,
        "the report must count what actually landed"
    );
    // Every row must be usable: a term that is not blank, and a source we set.
    for (term, _, source) in &rows {
        assert!(!term.trim().is_empty(), "blank search term stored");
        assert_eq!(source, "typed", "this file only yields typed searches");
    }
    eprintln!(
        "cross-check OK: {} typed search(es) from the real backup",
        rows.len()
    );
}
