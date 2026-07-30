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
