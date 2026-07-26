//! Every command that returns a collection must be windowed or declared bounded
//! (#65).
//!
//! Two incidents came from IPC shapes chosen without regard for how much data
//! would flow through them — #60 (an event per log record) and #61 (one invoke
//! per finding, ~8000 of them, plus a list that mounted every row). Both were
//! found by hitting them in production rather than in review, because nothing
//! forced the question at the time the command was written.
//!
//! This is that forcing function. A new `#[tauri::command]` returning `Vec<T>`
//! fails this test until its author either takes an `offset`/`limit` pair or
//! writes down, here, why the collection cannot grow. The reason is the point:
//! "it's small" ages badly, and a reader six months later cannot tell a deliberate
//! decision from an oversight.
//!
//! This checks the *shape* of the command surface, not the size of any payload —
//! it parses the source rather than running anything.

use std::collections::BTreeSet;

/// Commands that return a collection with no window, and the reason each one is
/// bounded anyway. Adding a line here is a claim you are making — keep it
/// concrete (what caps it), not reassuring ("should be small").
const BOUNDED: &[(&str, &str)] = &[
    // --- bounded by a UI constant or a fixed set ---
    (
        "count_call_ranges",
        "one count per histogram bucket; the bucket count is a UI constant",
    ),
    ("count_media_ranges", "one count per histogram bucket"),
    ("count_message_ranges", "one count per histogram bucket"),
    ("count_note_ranges", "one count per histogram bucket"),
    (
        "count_safari_bookmark_ranges",
        "one count per histogram bucket",
    ),
    ("count_safari_ranges", "one count per histogram bucket"),
    (
        "list_import_modules",
        "the backend's import catalog — a fixed list in the binary",
    ),
    ("get_reimport_status", "at most one entry per import module"),
    (
        "media_sources",
        "one row per distinct media source; a device has a handful",
    ),
    (
        "message_kinds",
        "one row per distinct message kind; an enum, not data",
    ),
    // --- bounded by the caller's own request ---
    (
        "get_app_icons",
        "one icon per bundle id the caller passed in",
    ),
    (
        "find_shortener_urls",
        "URLs inside one text the caller passed in",
    ),
    // --- bounded by something small on disk ---
    (
        "imported_backup_ids",
        "one id per imported backup; a user has a few",
    ),
    (
        "list_installed_apps",
        "one row per installed app — hundreds at the extreme",
    ),
    (
        "interaction_channels",
        "one row per app the person interacted through",
    ),
    (
        "list_recordings",
        "one row per voice memo; hundreds at the extreme",
    ),
    // --- bounded by time, not by volume ---
    ("health_daily", "one row per DAY in the requested range"),
    (
        "list_health_timezones",
        "one row per timezone change the device recorded",
    ),
    ("workout_route", "GPS points for ONE workout"),
    // --- unbounded in principle, and knowingly not windowed yet ---
    // These are the real backlog. Each is read once into a virtualized view, so
    // the cost is payload size rather than render time (#61's failure mode is
    // covered), but a large backup makes the JSON itself expensive. Windowing
    // them means moving filter/sort/group into SQL, which is a view change, not
    // a command change — tracked in #65.
    // The one that actually hurts: ~350 B/row, so ~3 MB at the 8800 findings
    // seen in practice — and the view re-derives filter, sort and grouping
    // from the whole array on every invalidation.
    (
        "list_content_findings",
        "#65: ~3 MB at 8800 findings; needs filter/sort/group in SQL first",
    ),
    // Measured: 399 B for a heavy group-chat row (8 participants, display
    // name, full snippet) — 78 KB at 200 conversations, 390 KB at 1000,
    // 1.9 MB at 5000. Window it when a real backup approaches the thousands;
    // note that contacts.tsx filters the full list client-side to find one
    // contact's conversations, so windowing needs a query for that first.
    (
        "list_threads",
        "#65: 399 B/row measured; 390 KB at 1000 conversations",
    ),
    (
        "list_contacts",
        "#65: one row per contact; read once into a virtualized list",
    ),
    (
        "list_notes",
        "#65: one row per note; read once into a virtualized list",
    ),
    (
        "list_calls",
        "#65: one row per call; read once into a virtualized list",
    ),
    (
        "list_safari_history",
        "#65: one row per visit; read once into a virtualized list",
    ),
    (
        "list_reminders",
        "#65: one row per reminder; read once into a virtualized list",
    ),
    (
        "list_calendar_events",
        "#65: one row per event; read once into a virtualized list",
    ),
    (
        "list_interactions",
        "#65: one row per CoreDuet interaction; read once into a virtualized list",
    ),
    ("list_workouts", "#65: one row per workout"),
    ("list_sleep", "#65: one row per sleep session"),
    ("list_cycle", "#65: one row per cycle entry"),
    ("list_health_achievements", "#65: one row per achievement"),
    (
        "list_findings",
        "#65: one row per security finding; virtualized in the view",
    ),
    (
        "list_safety_scans",
        "#65: one row per past scan; virtualized in the view",
    ),
    (
        "list_scan_runs",
        "#65: one row per past security run; virtualized in the view",
    ),
];

/// A `#[tauri::command]` as it appears in the source.
struct Command {
    name: String,
    params: String,
    ret: String,
}

/// Parse the command surface out of the crate's sources. Deliberately a text
/// scan: it needs to see what an author just typed, and nothing else in the
/// build can report "every registered command" without running Tauri.
fn commands() -> Vec<Command> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("src/ is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    files.sort();
    for path in files {
        let src = std::fs::read_to_string(&path).expect("source is readable");
        let mut at = 0usize;
        while let Some(hit) = src[at..].find("#[tauri::command") {
            let attr_end = at + hit + "#[tauri::command".len();
            at = attr_end;
            // The fn may sit under further attributes; take the next one.
            let Some(fn_rel) = src[attr_end..].find("fn ") else {
                break;
            };
            let fn_start = attr_end + fn_rel + 3;
            let Some(open_rel) = src[fn_start..].find('(') else {
                break;
            };
            let name = src[fn_start..fn_start + open_rel].trim().to_string();
            // Balance parentheses: a parameter type can contain them.
            let mut depth = 0usize;
            let mut i = fn_start + open_rel;
            let bytes = src.as_bytes();
            while i < bytes.len() {
                match bytes[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let params = src[fn_start + open_rel + 1..i].to_string();
            let body = src[i..].find('{').map(|b| i + b).unwrap_or(i);
            out.push(Command {
                name,
                params,
                ret: src[i + 1..body].trim().to_string(),
            });
        }
    }
    assert!(
        out.len() > 50,
        "parsed only {} commands — the scan is broken, not the crate",
        out.len()
    );
    out
}

#[test]
fn every_collection_returning_command_is_windowed_or_declared_bounded() {
    let declared: BTreeSet<&str> = BOUNDED.iter().map(|(n, _)| *n).collect();
    let mut offenders = Vec::new();
    for c in commands() {
        if !c.ret.contains("Vec<") {
            continue;
        }
        // An `offset`/`limit` pair is the windowed shape; that is the fix, so it
        // needs no entry here.
        if c.params.contains("limit") {
            continue;
        }
        if !declared.contains(c.name.as_str()) {
            offenders.push(c.name);
        }
    }
    assert!(
        offenders.is_empty(),
        "these commands return a collection with no window and no declared bound:\n  {}\n\n\
         Take `offset`/`limit` (see get_thread_message_window), or add a line to \
         BOUNDED in this file saying what caps the collection. \"It's small\" is not \
         a bound — say WHAT makes it small. See #65.",
        offenders.join("\n  ")
    );
}

#[test]
fn the_bounded_list_has_no_stale_entries() {
    // An allow-list that outlives the code it excuses is how the next audit ends
    // up trusting a claim about a command that no longer exists — or worse, one
    // that has since been windowed and no longer needs excusing.
    let actual: Vec<_> = commands()
        .into_iter()
        .filter(|c| c.ret.contains("Vec<"))
        .collect();
    let mut stale = Vec::new();
    for (name, _) in BOUNDED {
        match actual.iter().find(|c| c.name == *name) {
            None => stale.push(format!("{name} (no such command any more)")),
            Some(c) if c.params.contains("limit") => {
                stale.push(format!("{name} (now windowed — drop the exemption)"))
            }
            Some(_) => {}
        }
    }
    assert!(
        stale.is_empty(),
        "BOUNDED lists commands that no longer need to be there:\n  {}",
        stale.join("\n  ")
    );
}

#[test]
fn every_bounded_entry_gives_a_reason() {
    // The reason is the whole value of the list. A blank or hand-wavy one turns
    // this test into a rubber stamp.
    let vague = ["small", "few", "not many", "should be", "probably", "fine"];
    let mut bad = Vec::new();
    for (name, reason) in BOUNDED {
        let r = reason.trim();
        if r.len() < 20 {
            bad.push(format!("{name}: too short to be a reason"));
            continue;
        }
        // "hundreds at the extreme" is concrete; "should be small" is not.
        if vague.contains(&r) {
            bad.push(format!("{name}: says nothing about what caps it"));
        }
    }
    assert!(
        bad.is_empty(),
        "weak entries in BOUNDED:\n  {}",
        bad.join("\n  ")
    );
}
