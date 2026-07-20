//! Perf/AC check against a real cache DB (plan T4: full Tier A scan < 60 s).
//! Ignored by default; run with a copy-safe env pointer:
//!
//!   TRACELOUPE_REAL_CACHE="$HOME/Library/Application Support/se.addable.traceloupe.dev/caches/<udid>/cache.db" \
//!     cargo test -p traceloupe-core --test scan_real_cache -- --ignored --nocapture
//!
//! The cache is copied to a temp dir first — a scan writes scan_runs/findings
//! and must never touch the live cache from a test.

use traceloupe_core::analyzer::{run_scan, ScanKind, MODULES};
use traceloupe_core::cache::CacheDb;
use traceloupe_core::indicators::{bundled_snapshot_dir, load_snapshot_dir};
use traceloupe_core::sidecar::CancelToken;

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
        None,
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
