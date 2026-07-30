//! Validate artifact modules against a REAL iOS backup, end to end.
//!
//! Everything else about the modules is proven against fixtures we wrote, which
//! means it proves we parse what we already believed the schema to be. This runs
//! the same code against a real device's encrypted backup: our decryptor, our
//! Manifest index, our module SQL, real rows.
//!
//! It also settles a question fixtures cannot: whether a module's file is
//! **actually in a backup**. Apple's `Domains.plist` says it should be
//! (`tools/data/ios-backup-domains.json`), and this checks the real Manifest
//! agrees.
//!
//! ```text
//! # Josh Hickman's iOS 17 public image, per scripts/fetch-test-image.sh, then:
//! cargo run -p traceloupe-core --example validate_against_real_backup -- \
//!     <backup-dir> [password]
//! ```
//!
//! **Never point this at the owner's own backup** (AGENTS.md). It exists so that
//! is never necessary.

use std::path::PathBuf;

use traceloupe_core::artifacts;
use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::manifest::ManifestIndex;

fn main() {
    let mut args = std::env::args().skip(1);
    let backup_dir = PathBuf::from(
        args.next()
            .expect("usage: validate_against_real_backup <backup-dir> [password]"),
    );
    let password = args.next();

    let work = std::env::temp_dir().join("traceloupe-real-validate");
    std::fs::create_dir_all(&work).expect("work dir");

    // Our own decryptor against a real encrypted backup — the crypto ladder is
    // otherwise only exercised by a fixture we generated ourselves.
    let decryptor = password.as_deref().map(|p| {
        let d = BackupDecryptor::open(&backup_dir, p).expect("open the encrypted backup");
        println!("✓ decrypted the keybag with the supplied password");
        d
    });

    let index = ManifestIndex::open(&backup_dir, decryptor.as_ref(), &work)
        .expect("open + decrypt Manifest.db");
    println!("✓ Manifest.db opened");

    let specs = artifacts::builtin_modules().expect("shipped modules parse");
    println!("\n{} shipped module(s):", specs.len());

    let mut failures = 0;
    for spec in &specs {
        println!("\n── {} ({})", spec.name, spec.id);
        println!("   wants {}:{}", spec.domain, spec.path);

        // The reachability claim, checked against a real backup rather than
        // against Apple's rules alone.
        match index.find(&spec.domain, &spec.path) {
            Ok(Some(e)) => println!("   ✓ present in this backup (fileID {})", &e.file_id[..8]),
            Ok(None) => {
                println!("   ✗ NOT in this backup — the audit says it should be");
                failures += 1;
                continue;
            }
            Err(e) => {
                println!("   ✗ Manifest lookup failed: {e}");
                failures += 1;
                continue;
            }
        }

        match artifacts::run_module(spec, &index, decryptor.as_ref(), &work) {
            Ok(Some(rows)) => {
                println!("   ✓ {} rows", rows.len());
                for row in rows.iter().take(5) {
                    let mut cells: Vec<String> = Vec::new();
                    for c in &spec.columns {
                        let v = row.get(&c.name).cloned().unwrap_or(serde_json::Value::Null);
                        cells.push(format!("{}={}", c.name, v));
                    }
                    println!("       {}", cells.join("  "));
                }
                if rows.is_empty() {
                    println!("   ! the store is present but empty — worth knowing, not a failure");
                }
            }
            Ok(None) => {
                println!("   ✗ run_module found nothing though the Manifest has it");
                failures += 1;
            }
            Err(e) => {
                println!("   ✗ FAILED: {e}");
                failures += 1;
            }
        }
    }

    println!();
    if failures == 0 {
        println!("✓ every shipped module ran against real data");
    } else {
        println!("✗ {failures} module(s) failed against real data");
        std::process::exit(1);
    }
}
