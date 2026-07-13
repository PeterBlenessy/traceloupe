//! Validate TikTok DM normalization against a real iLEAPP lava DB. Gated on
//! TRACELOUPE_TIKTOK_LAVA (path to a `_lava_artifacts.db` with tiktok_messages).
use traceloupe_core::cache::CacheDb;
use traceloupe_core::normalize::normalize_lava;

#[test]
fn normalizes_real_tiktok_dms() {
    let Ok(lava) = std::env::var("TRACELOUPE_TIKTOK_LAVA") else {
        eprintln!("skipping: set TRACELOUPE_TIKTOK_LAVA to a real lava DB");
        return;
    };
    let lava = std::path::PathBuf::from(lava);
    let out_dir = lava.parent().unwrap();
    let cache = CacheDb::open_in_memory().unwrap();

    let report = normalize_lava(&lava, out_dir, &cache).unwrap();
    eprintln!(
        "TikTok: {} threads, {} messages",
        report.threads, report.messages
    );
    assert!(report.threads > 0, "no TikTok threads");
    assert!(report.messages > 1000, "suspiciously few messages");

    let c = cache.conn();
    // Every TikTok thread should have a positive count and a real last-message time.
    let bad: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM threads
             WHERE service = 'TikTok' AND (message_count = 0 OR last_message_at IS NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad, 0, "threads with no messages/time");

    // Named threads (peer nickname resolved) should be the majority.
    let named: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE service = 'TikTok' AND display_name IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!("named threads: {named}/{}", report.threads);

    // Group chats: labelled "Group chat · N people", never a raw numeric id.
    let groups: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE service = 'TikTok' AND display_name LIKE 'Group chat%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!("group chats: {groups}");
    // No thread name should be a bare numeric id (the group-chat bug).
    let numeric_names: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM threads
             WHERE service = 'TikTok' AND display_name GLOB '[0-9]*'
               AND display_name NOT GLOB '*[^0-9]*'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        numeric_names, 0,
        "a group thread is still named by its raw id"
    );

    // bare-numeric identifier fallback: a null display_name would make the UI
    // show the raw identifier — none should be a bare number for TikTok.
    let numeric_fallback: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM threads
             WHERE service = 'TikTok' AND display_name IS NULL
               AND identifier GLOB '[0-9]*' AND identifier NOT GLOB '*[^0-9]*'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        numeric_fallback, 0,
        "a thread would fall back to a bare-numeric id"
    );
    // A sanity check on timestamps: all within a plausible epoch range.
    let (min_ts, max_ts): (i64, i64) = c
        .query_row(
            "SELECT MIN(sent_at), MAX(sent_at) FROM messages
             WHERE thread_id IN (SELECT id FROM threads WHERE service = 'TikTok')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(min_ts > 1_300_000_000, "min ts too early: {min_ts}");
    assert!(max_ts < 1_900_000_000, "max ts too late: {max_ts}");
    eprintln!("ts range: {min_ts}..{max_ts}");
}
