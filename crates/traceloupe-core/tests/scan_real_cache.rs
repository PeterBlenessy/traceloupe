//! Perf/AC check against a real cache DB (plan T4: full Tier A scan < 60 s).
//! Ignored by default; run with a copy-safe env pointer:
//!
//!   TRACELOUPE_REAL_CACHE="$HOME/Library/Application Support/se.addable.traceloupe.dev/caches/<udid>/cache.db" \
//!     cargo test -p traceloupe-core --test scan_real_cache -- --ignored --nocapture
//!
//! The cache is copied to a temp dir first — a scan writes scan_runs/findings
//! and must never touch the live cache from a test.

use traceloupe_core::analyzer::{
    parse_addaily, parse_configuration_profiles, parse_datausage, parse_tcc, run_scan, ScanInputs,
    ScanKind, MODULES,
};
use traceloupe_core::cache::CacheDb;
use traceloupe_core::indicators::{bundled_snapshot_dir, load_snapshot_dir};
use traceloupe_core::sidecar::CancelToken;

/// Verify Tier-B process extraction against the real decrypted dev mirror
/// (~/.traceloupe-dev/backup-mirror). Ignored; run with:
///   cargo test -p traceloupe-core --test scan_real_cache real_mirror -- --ignored --nocapture
#[test]
#[ignore = "needs the decrypted dev mirror at ~/.traceloupe-dev/backup-mirror"]
fn process_extraction_on_real_mirror() {
    let home = std::env::var("HOME").unwrap();
    let mirror = format!("{home}/.traceloupe-dev/backup-mirror");
    let datausage = format!("{mirror}/WirelessDomain/Library/Databases/DataUsage.sqlite");
    let addaily =
        format!("{mirror}/HomeDomain/Library/Preferences/com.apple.osanalytics.addaily.plist");

    let mut processes = parse_datausage(std::path::Path::new(&datausage)).expect("datausage");
    let du = processes.len();
    let bytes = std::fs::read(&addaily).expect("addaily readable");
    let add = parse_addaily(&bytes).expect("addaily");
    processes.extend(add.iter().cloned());
    eprintln!(
        "extracted {} processes ({du} DataUsage + {} OSAnalytics)",
        processes.len(),
        add.len()
    );
    assert!(
        processes.len() > 50,
        "expected many processes on a real device"
    );

    // Configuration profiles from ProfileTruth + PayloadManifest.
    let cp_dir = format!(
        "{mirror}/SysSharedContainerDomain-systemgroup.com.apple.configurationprofiles/Library/ConfigurationProfiles"
    );
    let truth = std::fs::read(format!("{cp_dir}/ProfileTruth.plist")).expect("ProfileTruth");
    let manifest = std::fs::read(format!("{cp_dir}/PayloadManifest.plist")).ok();
    let profiles = parse_configuration_profiles(&truth, manifest.as_deref()).expect("profiles");
    eprintln!("extracted {} configuration profiles:", profiles.len());
    for p in &profiles {
        eprintln!(
            "  {} (org={:?} hidden={} caps={:?})",
            p.display_name, p.organization, p.hidden, p.capabilities
        );
    }

    // TCC granted permissions.
    let tcc = format!("{mirror}/HomeDomain/Library/TCC/TCC.db");
    let grants = parse_tcc(std::path::Path::new(&tcc)).expect("tcc");
    let clients: std::collections::HashSet<&str> =
        grants.iter().map(|g| g.client.as_str()).collect();
    eprintln!(
        "extracted {} permission grants across {} clients",
        grants.len(),
        clients.len()
    );

    // Scan them against the bundled indicators (cache-less: in-memory db).
    let (set, _) = load_snapshot_dir(&bundled_snapshot_dir()).unwrap();
    let db = CacheDb::open_in_memory().unwrap();
    let outcome = run_scan(
        &db,
        &set,
        ScanKind::Explicit,
        &["process_names", "profiles", "tcc"],
        ScanInputs {
            manifest_entries: None,
            processes: &processes,
            profiles: &profiles,
            grants: &grants,
        },
        "[]",
        &CancelToken::new(),
        |_, _, _| {},
    )
    .unwrap();
    eprintln!("process-name findings: {}", outcome.findings);
    let mut stmt = db
        .conn()
        .prepare("SELECT severity, malware, matched_value, context FROM findings")
        .unwrap();
    for row in stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .flatten()
    {
        eprintln!("  {row:?}");
    }
}

#[test]
#[ignore = "needs TRACELOUPE_REAL_CACHE pointing at a real cache.db"]
fn full_tier_a_scan_under_60s() {
    let Ok(src) = std::env::var("TRACELOUPE_REAL_CACHE") else {
        panic!("set TRACELOUPE_REAL_CACHE");
    };
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("cache.db");
    std::fs::copy(&src, &copy).unwrap();

    let db = CacheDb::open(&copy).unwrap();
    let (set, info) = load_snapshot_dir(&bundled_snapshot_dir()).unwrap();
    eprintln!(
        "snapshot {} feeds, {} indicators (generated {})",
        info.feeds.len(),
        set.len(),
        info.generated_at
    );

    let start = std::time::Instant::now();
    let outcome = run_scan(
        &db,
        &set,
        ScanKind::Explicit,
        MODULES,
        ScanInputs::default(),
        "[]",
        &CancelToken::new(),
        |m, i, n| eprintln!("  [{}/{}] {m} ({:?} elapsed)", i + 1, n, start.elapsed()),
    )
    .unwrap();
    let elapsed = start.elapsed();
    eprintln!(
        "scan finished in {elapsed:?}: {} findings",
        outcome.findings
    );
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT severity, module, malware, matched_value, context FROM findings WHERE run_id = ?1")
        .unwrap();
    let rows = stmt
        .query_map([outcome.run_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .unwrap();
    for row in rows.flatten() {
        eprintln!("  finding: {row:?}");
    }

    // Findings on a personal device are expected to be rare but not
    // impossible (e.g. a security researcher's browsing history). The AC here
    // is time, not cleanliness.
    assert!(
        elapsed.as_secs() < 60,
        "Tier A scan took {elapsed:?}, budget is 60s"
    );
}
