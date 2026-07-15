//! Dev helper: extract EVERY data file (databases, plists, json, …) from a
//! backup, across all domains, into a directory — a local mirror we can inspect
//! while writing native parsers, so we never have to guess a schema.
//!
//! Media blobs (photos/videos) are skipped by default — they're large and not
//! needed for schema work. Pass `all` as the third argument to include them too.
//!
//! The backup password is read from the `TRACELOUPE_BACKUP_PASSWORD` env var so
//! it never appears in argv/process listings (leave empty/unset for a plaintext
//! backup). Nothing here ships in the app — it's a `cargo run --example` tool.
//!
//! Usage:
//!   TRACELOUPE_BACKUP_PASSWORD=… cargo run -p traceloupe-core --example dump_backup -- \
//!       "<backup_dir>" "<dest_dir>" [all]
//!
//! Files land at `<dest_dir>/<domain>/<relativePath>` (domain kept so apps that
//! share a relativePath like `Library/…` don't collide).

use std::path::{Path, PathBuf};

use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::manifest::ManifestIndex;
use traceloupe_core::Result;

/// File extensions worth extracting for schema inspection (lower-cased). Covers
/// SQLite in its many guises, Core Data, property lists, and app JSON/archives.
const DATA_EXT: &[&str] = &[
    "db", "sqlite", "sqlitedb", "sqlite3", "storedata", "realm", "plist", "json",
    "archive", "strings", "yap", "data", "binarypb",
];

fn is_data_file(relative_path: &str) -> bool {
    match Path::new(relative_path)
        .extension()
        .and_then(|s| s.to_str())
    {
        Some(ext) => DATA_EXT.contains(&ext.to_lowercase().as_str()),
        None => false,
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: TRACELOUPE_BACKUP_PASSWORD=… dump_backup <backup_dir> <dest_dir> [all]"
        );
        std::process::exit(2);
    }
    let backup_dir = Path::new(&args[1]);
    let dest_dir = PathBuf::from(&args[2]);
    let include_all = args.get(3).map(|s| s == "all").unwrap_or(false);

    let password = std::env::var("TRACELOUPE_BACKUP_PASSWORD").unwrap_or_default();
    let decryptor = if password.is_empty() {
        eprintln!("(no password set — treating backup as plaintext)");
        None
    } else {
        Some(BackupDecryptor::open(backup_dir, &password)?)
    };

    let work_dir = dest_dir.join(".work");
    std::fs::create_dir_all(&work_dir).ok();
    let index = ManifestIndex::open(backup_dir, decryptor.as_ref(), &work_dir)?;

    // Every file across every domain (relativePath LIKE '%').
    let entries = index.find_relative_like("%")?;
    println!(
        "{} files in backup; extracting {}",
        entries.len(),
        if include_all { "everything" } else { "data files only (db/plist/json/…)" },
    );

    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for e in &entries {
        if e.relative_path.is_empty() {
            continue; // a domain's root directory entry
        }
        if !include_all && !is_data_file(&e.relative_path) {
            skipped += 1;
            continue;
        }
        let dest = dest_dir.join(&e.domain).join(&e.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match index.extract_to(e, decryptor.as_ref(), &dest) {
            Ok(()) => {
                ok += 1;
                if ok % 250 == 0 {
                    println!("  … {ok} extracted");
                }
            }
            Err(err) => {
                failed += 1;
                eprintln!("  ! {}/{} — {err}", e.domain, e.relative_path);
            }
        }
    }
    println!(
        "extracted {ok} files ({skipped} non-data skipped, {failed} failed) → {}",
        dest_dir.display()
    );
    std::fs::remove_dir_all(&work_dir).ok();
    Ok(())
}
