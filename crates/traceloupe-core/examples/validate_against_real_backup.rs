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

    report_uncatalogued_apps(&index);

    let specs = artifacts::builtin_modules().expect("shipped modules parse");
    println!("\n{} shipped module(s):", specs.len());

    let mut failures = 0;
    let mut absent = 0;
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
                // ABSENT, not BROKEN. Whether this is a defect depends on the
                // other devices in the corpus, which one run cannot see -- so
                // the marker is emitted for the corpus runner to judge, and the
                // exit code below still fails a single run, because on its own
                // "the audit says it should be here" is all we know.
                println!("   ✗ NOT in this backup — the audit says it should be");
                println!("MARKER absent {}", spec.id);
                absent += 1;
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
                println!("MARKER broken {}", spec.id);
                failures += 1;
            }
            Err(e) => {
                println!("   ✗ FAILED: {e}");
                println!("MARKER broken {}", spec.id);
                failures += 1;
            }
        }
    }

    println!();
    if failures == 0 && absent == 0 {
        println!("✓ every shipped module ran against real data");
    } else {
        if absent > 0 {
            println!("! {absent} module(s) read a store this device does not have");
        }
        if failures > 0 {
            println!("✗ {failures} module(s) failed against real data");
        }
        std::process::exit(1);
    }
}

/// Apps installed on this device that the catalogue says nothing about.
///
/// The check that would have caught a real bug rather than luck catching it: the
/// catalogue listed Telegram under `org.telegram.messenger` and imo under
/// `com.imo.imoim` — both ANDROID package names. On a real iPhone we imported
/// their chats and then showed the apps as unsupported, and nothing anywhere
/// noticed, because only a real device knows what a real bundle id looks like.
///
/// It cannot run in CI (there is no device there), so it runs where the evidence
/// is. It reports rather than fails: an app we say nothing about is usually just
/// an app we do not support yet, and only a human can tell that from a wrong id.
fn report_uncatalogued_apps(index: &ManifestIndex) {
    let catalog = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/apps.ts"),
    )
    .unwrap_or_default();

    let mut installed: Vec<String> = Vec::new();
    let _ = index.for_each_path(|domain, _| {
        // `AppDomain-<bundle>`, `AppDomainGroup-group.<bundle>` and
        // `AppDomainPlugin-<bundle>` all carry an app identifier.
        if let Some((prefix, rest)) = domain.split_once('-') {
            if prefix == "AppDomain" && !rest.starts_with("com.apple") {
                installed.push(rest.to_string());
            }
        }
    });
    installed.sort();
    installed.dedup();

    let missing: Vec<&String> = installed
        .iter()
        .filter(|b| !catalog.contains(&format!("\"{b}\"")))
        .collect();

    println!(
        "\n── app catalogue: {} of {} installed third-party apps are catalogued",
        installed.len() - missing.len(),
        installed.len()
    );
    if !missing.is_empty() {
        println!("   not in src/lib/apps.ts — each is either unsupported, or supported under");
        println!("   the WRONG bundle id (which is how Telegram and imo were invisible):");
        for b in missing.iter().take(40) {
            println!("     {b}");
        }
        if missing.len() > 40 {
            println!("     … and {} more", missing.len() - 40);
        }
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
    // ABSENCE IS A FACT ABOUT A DEVICE; only absence EVERYWHERE is a defect.
    //
    // The iPhone 11 at iOS 16.1.2 has no `ACXRemoteAppList.plist` and no
    // `com.apple.MobileBackup.plist`; the same phone at 17.3 has both. Counting
    // that as two module failures makes the corpus report get worse every time
    // an older device is added, which is the opposite of what a corpus is for --
    // and a validator that always fails is one nobody runs.
    //
    // So a module absent HERE but present on some other device is noted. A
    // module absent from EVERY backup has a path that is wrong, and fails.
    let mut broken: Vec<String> = Vec::new();
    let mut absent_in: std::collections::BTreeMap<String, usize> = Default::default();
    for dir in &backups {
        let pw = password_for(&catalog, dir);
        println!("════ {}", dir.display());
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(dir)
            .args(pw.iter())
            .output()
            .expect("re-run self for one backup");
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            // Markers are consumed here, never shown -- they are plumbing.
            if let Some(id) = line.strip_prefix("MARKER absent ") {
                *absent_in.entry(id.to_string()).or_default() += 1;
            } else if let Some(id) = line.strip_prefix("MARKER broken ") {
                broken.push(format!("{id} ({})", dir.display()));
            } else {
                println!("{line}");
            }
        }
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        println!();
    }

    let everywhere: Vec<&String> = absent_in
        .iter()
        .filter(|(_, n)| **n == backups.len())
        .map(|(id, _)| id)
        .collect();
    let somewhere: Vec<&String> = absent_in
        .iter()
        .filter(|(_, n)| **n < backups.len())
        .map(|(id, _)| id)
        .collect();

    if !somewhere.is_empty() {
        println!(
            "! not on every device, which is a fact about the devices: {}",
            somewhere
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if broken.is_empty() && everywhere.is_empty() {
        println!(
            "✓ every shipped module ran against all {} backup(s)",
            backups.len()
        );
        return;
    }
    if !everywhere.is_empty() {
        eprintln!(
            "✗ absent from EVERY backup, so the path is wrong: {}",
            everywhere
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !broken.is_empty() {
        eprintln!("✗ present but unreadable: {}", broken.join(", "));
    }
    std::process::exit(1);
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
