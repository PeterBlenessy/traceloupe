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

use std::path::{Path, PathBuf};

use traceloupe_core::artifacts;
use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::manifest::ManifestIndex;

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    // No argument: validate against EVERY backup in the local corpus. The point of
    // a corpus is that a module is checked on more than one device, and a
    // validator that has to be pointed at one device at a time quietly becomes a
    // validator that is only ever run against the newest.
    let Some(first) = first else {
        return validate_corpus();
    };
    let backup_dir = PathBuf::from(first);
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
        // `artifacts::locate`, not a lookup of our own: a module's `path` may be a
        // PATTERN, and an exact `find` would report a store the runner reads
        // perfectly well as missing.
        match artifacts::locate(&index, spec) {
            Ok(found) if !found.is_empty() => {
                println!(
                    "   ✓ present in this backup ({})",
                    found
                        .iter()
                        .map(|e| format!("{} {}", &e.file_id[..8], e.relative_path))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Ok(_) => {
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

/// Every unpacked backup under the corpus directory, validated in turn.
///
/// Found by SHAPE — a directory holding a `Manifest.db` — rather than from a
/// recorded list, so a backup someone unpacked by hand counts, and a list that has
/// gone stale can never claim a device we do not have. Passwords come from
/// `tools/data/dfir-images.json`, matched by the image id in the path.
fn validate_corpus() {
    let root = std::env::var("TRACELOUPE_TEST_IMAGES").unwrap_or_else(|_| {
        format!(
            "{}/Development/traceloupe-test-images",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let root = PathBuf::from(root);
    if !root.is_dir() {
        eprintln!(
            "no corpus at {} — run scripts/fetch-test-image.sh --list",
            root.display()
        );
        std::process::exit(2);
    }

    let mut backups = Vec::new();
    find_backups(&root, 0, &mut backups);
    backups.sort();
    if backups.is_empty() {
        eprintln!(
            "no unpacked backups under {} — run scripts/fetch-test-image.sh <id>",
            root.display()
        );
        std::process::exit(2);
    }

    let catalog = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/data/dfir-images.json"),
    )
    .unwrap_or_default();

    println!(
        "corpus: {} backup(s) under {}\n",
        backups.len(),
        root.display()
    );
    let mut failed = 0;
    for dir in &backups {
        let pw = password_for(&catalog, dir);
        println!("════ {}", dir.display());
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(dir)
            .args(pw.iter())
            .status()
            .expect("re-run self for one backup");
        if !status.success() {
            failed += 1;
        }
        println!();
    }
    if failed > 0 {
        eprintln!("✗ {failed} of {} backup(s) failed", backups.len());
        std::process::exit(1);
    }
    println!(
        "✓ every shipped module ran against all {} backup(s)",
        backups.len()
    );
}

fn find_backups(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            // `extracted/` holds loose plists pulled out for inspection, not a
            // backup — it has a Manifest.db and no blobs, so it would "validate"
            // as a backup containing nothing.
            if p.file_name().is_some_and(|n| n == "extracted") {
                continue;
            }
            if p.join("Manifest.db").is_file() {
                out.push(p);
            } else {
                find_backups(&p, depth + 1, out);
            }
        }
    }
}

/// The backup password for whichever catalogued image this path belongs to.
fn password_for(catalog: &str, dir: &Path) -> Option<String> {
    let path = dir.to_string_lossy().to_lowercase();
    // Crude on purpose: matching an id against the path is enough to tell ios17
    // from ios16, and a JSON dependency for four fields is not worth it.
    for block in catalog.split("{\n") {
        // `continue`, not `?`: a block without an id is the readme or a stray
        // brace, and returning on the first one meant the password was never
        // found — which silently ran the validator unauthenticated against an
        // ENCRYPTED backup, so all 15 modules "failed against real data".
        let Some(id) = field_of(block, "id") else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let compact = path.replace(['_', '-', ' '], "");
        if compact.contains(&id.to_lowercase()) {
            return field_of(block, "backup_password").filter(|p| !p.is_empty());
        }
    }
    None
}

fn field_of(block: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\": \"");
    let at = block.find(&needle)? + needle.len();
    let rest = &block[at..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
