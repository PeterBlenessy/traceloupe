//! Dev helper: extract a single app's backup container to a directory so we can
//! inspect real on-disk schemas while writing native app-artifact parsers.
//!
//! The backup password is read from the `TRACELOUPE_BACKUP_PASSWORD` env var so
//! it never appears in argv/process listings (leave empty/unset for a plaintext
//! backup). Nothing here ships in the app — it's a `cargo run --example` tool.
//!
//! Usage:
//!   TRACELOUPE_BACKUP_PASSWORD=… cargo run -p traceloupe-core --example dump_app_files -- \
//!       "<backup_dir>" "<domain>" "<dest_dir>" [relativePath-LIKE-filter]
//!
//! Example (PicCollage container):
//!   … --example dump_app_files -- \
//!       "$HOME/Library/Application Support/MobileSync/Backup/<udid>" \
//!       "AppDomain-com.cardinalblue.PicCollage" "/tmp/piccollage" "%"

use std::path::{Path, PathBuf};

use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::manifest::ManifestIndex;
use traceloupe_core::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: TRACELOUPE_BACKUP_PASSWORD=… dump_app_files \
             <backup_dir> <domain> <dest_dir> [relativePath-LIKE]"
        );
        std::process::exit(2);
    }
    let backup_dir = Path::new(&args[1]);
    let domain = &args[2];
    let dest_dir = PathBuf::from(&args[3]);
    // The manifest `find_prefix` matches `LIKE '<prefix>%'`; default to everything.
    let prefix = args.get(4).map(String::as_str).unwrap_or("");

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

    let entries = if prefix.is_empty() {
        // No prefix filter → list the whole domain. find_prefix with "" LIKE '%'
        // matches all relativePaths under the domain.
        index.find_prefix(domain, "")?
    } else {
        index.find_prefix(domain, prefix)?
    };

    println!("{} files in {domain}", entries.len());
    let mut ok = 0usize;
    for e in &entries {
        if e.relative_path.is_empty() {
            continue; // the domain's root directory entry
        }
        let dest = dest_dir.join(&e.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match index.extract_to(e, decryptor.as_ref(), &dest) {
            Ok(()) => {
                ok += 1;
                println!("  {}", e.relative_path);
            }
            Err(err) => eprintln!("  ! {} — {err}", e.relative_path),
        }
    }
    println!("extracted {ok}/{} → {}", entries.len(), dest_dir.display());
    std::fs::remove_dir_all(&work_dir).ok();
    Ok(())
}
