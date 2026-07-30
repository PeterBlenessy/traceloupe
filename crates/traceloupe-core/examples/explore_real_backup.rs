//! Explore a REAL iOS backup to find and design the next artifact module.
//!
//! Writing a module means answering three questions about a store, and all three
//! have been answered by hand so far — which is slow and, worse, guessable. This
//! answers them from the real thing:
//!
//! 1. **Is the file in a backup at all?** Not "should it be" per Apple's
//!    `Domains.plist`, but is it in this device's Manifest.
//! 2. **What is its actual schema?** Not what a blog post or iLEAPP's SQL
//!    implies — the `CREATE TABLE` the device really wrote, at its real iOS
//!    version.
//! 3. **Does a candidate query return rows, and which?** So a module ships
//!    having been run, not having been reasoned about.
//!
//! ```text
//! # `list` — what is in the backup, by path pattern (SQL LIKE, % wildcards)
//! cargo run -p traceloupe-core --example explore_real_backup -- \
//!     <backup-dir> <password> list '%voicemail%'
//!
//! # `schema` — every CREATE statement and row count in one store
//! cargo run -p traceloupe-core --example explore_real_backup -- \
//!     <backup-dir> <password> schema HomeDomain Library/Voicemail/voicemail.db
//!
//! # `sql` — run a candidate module query and print what comes back
//! cargo run -p traceloupe-core --example explore_real_backup -- \
//!     <backup-dir> <password> sql HomeDomain Library/Voicemail/voicemail.db \
//!     'SELECT sender, duration FROM voicemail LIMIT 5'
//!
//! # `plist` — the shape of a property list: key paths, types, sample values
//! cargo run -p traceloupe-core --example explore_real_backup -- \
//!     <backup-dir> <password> plist SystemPreferencesDomain \
//!     com.apple.wifi.known-networks.plist
//! ```
//!
//! **Never point this at the owner's own backup** (AGENTS.md). Use Josh
//! Hickman's public image via `scripts/fetch-test-image.sh`. This exists so the
//! owner's data is never the convenient option.

use std::path::PathBuf;

use rusqlite::Connection;
use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::manifest::ManifestIndex;

fn usage() -> ! {
    eprintln!(
        "usage: explore_real_backup <backup-dir> <password|-> <command> ...\n\
         \n\
         \x20 list   <like-pattern>                  paths matching a SQL LIKE pattern\n\
         \x20 schema <domain> <relative-path>        CREATE statements + row counts\n\
         \x20 sql    <domain> <relative-path> <sql>  run a query, print the rows\n\
         \x20 plist  <domain> <relative-path>        key paths, types, sample values\n\
         \x20 raw    <domain> <relative-path>        what kind of file it is, and its head\n\
         \n\
         Pass - as the password for an unencrypted backup."
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        usage();
    }
    let backup_dir = PathBuf::from(&args[0]);
    // `-` rather than an absent argument, so a forgotten password is a usage
    // error instead of silently exploring as if the backup were plaintext.
    let password = if args[1] == "-" {
        None
    } else {
        Some(args[1].clone())
    };
    let command = args[2].as_str();

    let work = std::env::temp_dir().join("traceloupe-explore");
    std::fs::create_dir_all(&work).expect("work dir");

    let decryptor = password
        .as_deref()
        .map(|p| BackupDecryptor::open(&backup_dir, p).expect("open the encrypted backup"));
    let index = ManifestIndex::open(&backup_dir, decryptor.as_ref(), &work)
        .expect("open + decrypt Manifest.db");

    match command {
        "list" => {
            let pattern = args.get(3).map(String::as_str).unwrap_or_else(|| usage());
            let hits = index
                .find_relative_like(pattern)
                .expect("query the manifest");
            println!("{} path(s) matching {pattern}\n", hits.len());
            for e in &hits {
                println!("  {:<28} {}", e.domain, e.relative_path);
            }
            if hits.is_empty() {
                println!("  (nothing — this store is NOT in this backup)");
            }
        }
        // The first bytes of any file, whatever it is. Needed because "what kind of
        // file is this even" is the first question about a candidate, and `schema`
        // and `plist` both assume an answer.
        "raw" => {
            let (domain, path) = match (args.get(3), args.get(4)) {
                (Some(d), Some(p)) => (d.as_str(), p.as_str()),
                _ => usage(),
            };
            let entry = index
                .find(domain, path)
                .expect("query the manifest")
                .unwrap_or_else(|| {
                    eprintln!("NOT IN THIS BACKUP: {domain}:{path}");
                    std::process::exit(1);
                });
            let bytes = index
                .read_bytes(&entry, decryptor.as_ref())
                .expect("decrypt the file");
            let kind = if bytes.starts_with(b"SQLite format 3") {
                "SQLite"
            } else if bytes.starts_with(b"bplist00") {
                "binary plist"
            } else if bytes.starts_with(b"<?xml") {
                "XML"
            } else if bytes.first().is_some_and(|b| *b == b'{' || *b == b'[') {
                "JSON (probably)"
            } else {
                "unrecognised"
            };
            println!(
                "── {domain}:{path}\n{} bytes, looks like: {kind}\n",
                bytes.len()
            );
            let head: String = String::from_utf8_lossy(&bytes[..bytes.len().min(600)])
                .chars()
                .map(|c| if c.is_control() && c != '\n' { '.' } else { c })
                .collect();
            println!("{head}");
        }
        "plist" => {
            let (domain, path) = match (args.get(3), args.get(4)) {
                (Some(d), Some(p)) => (d.as_str(), p.as_str()),
                _ => usage(),
            };
            let entry = index
                .find(domain, path)
                .expect("query the manifest")
                .unwrap_or_else(|| {
                    eprintln!("NOT IN THIS BACKUP: {domain}:{path}");
                    std::process::exit(1);
                });
            let bytes = index
                .read_bytes(&entry, decryptor.as_ref())
                .expect("decrypt the plist");
            // Resolved the same way the runner resolves it, or the explorer would
            // show `$objects`/`$top` for an archive while a module sees the real
            // tree — and an author would design against the wrong shape.
            let root = traceloupe_core::nska::resolve(&bytes).expect("parse the property list");
            println!("── {domain}:{path}\n");
            // Paths rather than a pretty-print: a module declares a key PATH, so
            // that is what needs reading off. Arrays collapse to one representative
            // element with its index shown as [n], because 400 identical shapes
            // teach nothing that the first does not.
            dump_plist(&root, &mut Vec::new(), 0);
        }
        "schema" | "sql" => {
            let (domain, path) = match (args.get(3), args.get(4)) {
                (Some(d), Some(p)) => (d.as_str(), p.as_str()),
                _ => usage(),
            };
            let entry = index
                .find(domain, path)
                .expect("query the manifest")
                .unwrap_or_else(|| {
                    // The single most important negative result: a store that is
                    // simply not in a backup, however good its schema looks in an
                    // FFS image.
                    eprintln!("NOT IN THIS BACKUP: {domain}:{path}");
                    eprintln!("Try `list` with a pattern to see what is.");
                    std::process::exit(1);
                });

            let dest = work.join("explore.sqlite");
            // The sidecars too: `extract_db` writes `-wal`/`-shm` and only tries
            // to checkpoint them away, discarding the result. Removing just the
            // main file could leave a previous run's WAL beside a fresh store.
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(work.join(format!("explore.sqlite{suffix}")));
            }
            index
                .extract_db(&entry, decryptor.as_ref(), &dest)
                .expect("decrypt + extract the store");
            let conn = Connection::open_with_flags(
                &dest,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .expect("open the extracted store read-only");

            if command == "schema" {
                dump_schema(&conn, domain, path);
            } else {
                let sql = args.get(5).map(String::as_str).unwrap_or_else(|| usage());
                run_sql(&conn, sql);
            }
        }
        _ => usage(),
    }
}

/// How deep to descend. Deep enough for real artifacts, shallow enough that a
/// pathological plist cannot fill the terminal.
const MAX_DEPTH: usize = 6;

fn dump_plist(v: &plist::Value, at: &mut Vec<String>, depth: usize) {
    let here = if at.is_empty() {
        "(root)".to_string()
    } else {
        at.join(" / ")
    };
    match v {
        plist::Value::Dictionary(d) => {
            println!(
                "{:indent$}{here}  <dict, {} keys>",
                "",
                d.len(),
                indent = depth * 2
            );
            if depth >= MAX_DEPTH {
                // Silence here looked exactly like an empty container.
                println!(
                    "{:indent$}  … (truncated at depth {MAX_DEPTH})",
                    "",
                    indent = depth * 2
                );
                return;
            }
            for (k, val) in d.iter() {
                at.push(k.clone());
                dump_plist(val, at, depth + 1);
                at.pop();
            }
        }
        plist::Value::Array(a) => {
            // Heterogeneous arrays are ordinary in Apple's plists, so say when the
            // later elements are NOT like the first. Showing element 0 alone and
            // saying nothing hides exactly the key a module author would miss.
            let extra = union_of_keys_beyond_first(a);
            let note = if extra.is_empty() {
                String::new()
            } else {
                format!("  (elements differ; also seen: {})", extra.join(", "))
            };
            println!(
                "{:indent$}{here}  <array, {} items>{note}",
                "",
                a.len(),
                indent = depth * 2
            );
            if depth >= MAX_DEPTH {
                println!(
                    "{:indent$}  … (truncated at depth {MAX_DEPTH})",
                    "",
                    indent = depth * 2
                );
                return;
            }
            // Element 0 as the representative shape, indexed the way a module's
            // path indexes it — `rows = ["items", "0"]`, not "[0]".
            if let Some(first) = a.first() {
                at.push("0".into());
                dump_plist(first, at, depth + 1);
                at.pop();
            }
        }
        scalar => {
            let (kind, sample) = describe(scalar);
            println!(
                "{:indent$}{here}  <{kind}> {sample}",
                "",
                indent = depth * 2
            );
        }
    }
}

/// Keys present on later array elements but not on the first — the ones a reader
/// looking only at element 0 would never know about.
fn union_of_keys_beyond_first(a: &[plist::Value]) -> Vec<String> {
    let first: std::collections::BTreeSet<String> = match a.first() {
        Some(plist::Value::Dictionary(d)) => d.keys().cloned().collect(),
        _ => return Vec::new(),
    };
    let mut extra = std::collections::BTreeSet::new();
    for v in a.iter().skip(1) {
        if let plist::Value::Dictionary(d) = v {
            for k in d.keys() {
                if !first.contains(k) {
                    extra.insert(k.clone());
                }
            }
        }
    }
    extra.into_iter().take(12).collect()
}

fn describe(v: &plist::Value) -> (&'static str, String) {
    match v {
        plist::Value::String(s) => (
            "string",
            format!("{:?}", s.chars().take(60).collect::<String>()),
        ),
        plist::Value::Integer(i) => ("integer", i.to_string()),
        plist::Value::Real(f) => ("real", f.to_string()),
        plist::Value::Boolean(b) => ("bool", b.to_string()),
        plist::Value::Date(d) => ("date", format!("{d:?}")),
        // Two things a byte count does not give: whether the bytes are UTF-8
        // (which decides whether a `text` column shows the string or nothing), and
        // whether this is an EMBEDDED binary plist, where the densest subtrees in
        // Apple's stores hide.
        plist::Value::Data(d) => (
            "data",
            if d.starts_with(b"bplist00") {
                format!("<{} bytes — EMBEDDED binary plist>", d.len())
            } else {
                match std::str::from_utf8(d) {
                    Ok(t)
                        if !t.is_empty()
                            && !t
                                .chars()
                                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) =>
                    {
                        format!(
                            "<{} bytes, UTF-8> {:?}",
                            d.len(),
                            t.chars().take(60).collect::<String>()
                        )
                    }
                    _ => format!("<{} bytes, not text>", d.len()),
                }
            },
        ),
        plist::Value::Uid(u) => ("uid", format!("{}", u.get())),
        _ => ("?", String::new()),
    }
}

fn dump_schema(conn: &Connection, domain: &str, path: &str) {
    println!("── {domain}:{path}\n");
    let mut stmt = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
             ORDER BY type != 'table', name",
        )
        .expect("read sqlite_master");
    let mut rows = stmt.query([]).expect("query sqlite_master");
    let mut tables = Vec::new();
    while let Some(r) = rows.next().expect("next row") {
        let kind: String = r.get(0).unwrap_or_default();
        let name: String = r.get(1).unwrap_or_default();
        let sql: String = r.get(2).unwrap_or_default();
        println!("{sql};\n");
        if kind == "table" {
            tables.push(name);
        }
    }

    // Row counts, because an empty table is the difference between a module that
    // works and a module that only compiles.
    println!("row counts:");
    for t in &tables {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{t}\""), [], |r| r.get(0))
            .unwrap_or(-1);
        println!("  {n:>8}  {t}");
    }
}

fn run_sql(conn: &Connection, sql: &str) {
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SQL did not prepare: {e}");
            std::process::exit(1);
        }
    };
    let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    println!("{}", cols.join(" | "));

    let mut rows = stmt.query([]).expect("run the query");
    let mut n = 0;
    while let Some(r) = rows.next().expect("next row") {
        let cells: Vec<String> = (0..cols.len())
            .map(|i| match r.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Null) => "NULL".into(),
                Ok(rusqlite::types::ValueRef::Integer(v)) => v.to_string(),
                Ok(rusqlite::types::ValueRef::Real(v)) => v.to_string(),
                Ok(rusqlite::types::ValueRef::Text(v)) => {
                    String::from_utf8_lossy(v).chars().take(60).collect()
                }
                Ok(rusqlite::types::ValueRef::Blob(v)) => format!("<{} bytes>", v.len()),
                Err(_) => "?".into(),
            })
            .collect();
        println!("{}", cells.join(" | "));
        n += 1;
    }
    println!("\n{n} row(s)");
}
