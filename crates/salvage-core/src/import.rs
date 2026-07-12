//! Import orchestration (architecture §6): run the iLEAPP sidecar against a
//! backup, then normalize its output into a fresh cache DB. This is the one
//! eager, whole-backup pass; every browse afterward is a cache query.

use std::path::{Path, PathBuf};

use crate::cache::CacheDb;
use crate::normalize::{self, ImportReport};
use crate::sidecar::{self, CancelToken, EngineConfig, Progress};
use crate::Result;

/// Phases of an import, so the UI can show more than a bare percentage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPhase {
    /// iLEAPP is parsing the backup; carries per-artifact progress.
    Parsing(Progress),
    /// Reading iLEAPP's output into the cache DB.
    Normalizing,
    /// Done; carries the final report.
    Done(ImportReport),
}

/// Result of a completed import.
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub cache_path: PathBuf,
    pub report: ImportReport,
}

/// Import `backup_dir` into a cache DB at `cache_path`, using the iLEAPP engine
/// described by `cfg`. `work_dir` holds the engine's (large, transient) output.
/// `on_phase` receives progress updates; `cancel` aborts a running engine.
#[allow(clippy::too_many_arguments)]
pub fn import_backup(
    cfg: &EngineConfig,
    backup_dir: &Path,
    password: &str,
    cache_path: &Path,
    work_dir: &Path,
    module_ids: &[String],
    cancel: &CancelToken,
    mut on_phase: impl FnMut(ImportPhase),
) -> Result<ImportOutcome> {
    // Start from a clean slate so re-importing is idempotent, not additive:
    // iLEAPP writes a new timestamped subfolder each run (they'd pile up and
    // find_lava_db could pick a stale one), and the normalizer appends rows
    // (a leftover cache would duplicate everything). Also frees the previous
    // run's disk before writing the new one.
    let _ = std::fs::remove_dir_all(work_dir);
    remove_cache(cache_path);

    let lava_path = sidecar::run_import(cfg, backup_dir, password, work_dir, module_ids, cancel, |p| {
        on_phase(ImportPhase::Parsing(p))
    })?;

    on_phase(ImportPhase::Normalizing);
    let engine_out_dir = lava_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| work_dir.to_path_buf());

    let cache = CacheDb::open(cache_path)?;
    let mut report = normalize::normalize_lava(&lava_path, &engine_out_dir, &cache)?;

    let effective = sidecar::effective_module_ids(module_ids);

    // Camera roll: read the backup's Manifest natively and reference iOS's own
    // thumbnails, so the gallery is fast and full images transcode on demand.
    if effective.contains(&"camera_roll") {
        match crate::parsers::camera_roll::parse_camera_roll(backup_dir) {
            Ok(assets) => {
                let conn = cache.conn();
                for a in &assets {
                    conn.execute(
                        "INSERT INTO media_items
                            (domain, relative_path, kind, source, mime_type,
                             taken_at, thumb_path, local_path)
                         VALUES ('CameraRollDomain', ?1, ?2, 'Photos', ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            a.relative_path,
                            a.kind,
                            a.mime,
                            a.taken_at,
                            a.thumb_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                            a.full_path.to_string_lossy(),
                        ],
                    )?;
                }
                report.media_items += assets.len();
            }
            Err(e) => report.warnings.push(format!(
                "Camera roll: couldn't read the backup manifest ({e}). \
                 Encrypted backups aren't supported yet."
            )),
        }
    }

    // Diagnostic: flag any enabled data type that produced nothing, so an empty
    // Safari/Calls (usually the source DB isn't in this backup) is visible
    // instead of silently absent.
    for id in effective {
        let (label, count) = match id {
            "messages" => ("Messages", report.messages),
            "calls" => ("Call history", report.calls),
            "contacts" => ("Contacts", report.contacts),
            "safari" => ("Safari history", report.safari_visits),
            "notes" => ("Notes", report.notes),
            // camera_roll isn't checked here: media_items also holds message/app
            // attachments, so a 0-count test wouldn't be meaningful.
            _ => continue,
        };
        if count == 0 {
            report
                .warnings
                .push(format!("{label}: nothing found — the source data isn't in this backup."));
        }
    }

    // Record which apps were on the device (from Info.plist) for the Apps view.
    let apps = crate::discovery::installed_apps(backup_dir);
    for bundle_id in &apps {
        cache.conn().execute(
            "INSERT OR IGNORE INTO installed_apps (bundle_id) VALUES (?1)",
            [bundle_id],
        )?;
    }

    on_phase(ImportPhase::Done(report.clone()));
    Ok(ImportOutcome {
        cache_path: cache_path.to_path_buf(),
        report,
    })
}

/// Remove a SQLite cache DB and its WAL/SHM sidecars, if present.
fn remove_cache(cache_path: &Path) {
    let _ = std::fs::remove_file(cache_path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = cache_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(sidecar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// End-to-end orchestration with a fake engine (a shell script that writes
    /// a minimal lava DB), so it needs no real iLEAPP. Confirms the phases fire
    /// in order and the cache ends up populated.
    #[cfg(unix)]
    #[test]
    fn import_runs_engine_then_normalizes() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();

        // Fake engine: emits one progress line, then writes a lava DB with one
        // sms row into its output subfolder.
        let script = tmp.path().join("fake_ileapp.sh");
        {
            let mut f = std::fs::File::create(&script).unwrap();
            writeln!(
                f,
                r#"#!/bin/sh
out=""
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2;; *) shift;; esac; done
echo "[1/1] sms [sms] artifact started"
sub="$out/iLEAPP_Output_test"
mkdir -p "$sub"
sqlite3 "$sub/_lava_artifacts.db" "CREATE TABLE sms (message_timestamp INTEGER, read_timestamp INTEGER, message TEXT, service TEXT, message_direction TEXT, message_sent TEXT, message_delivered TEXT, message_read TEXT, account TEXT, account_login TEXT, chat_contact_id TEXT, attachment_name TEXT, attachment_file TEXT, attachment_timestamp INTEGER, attachment_mimetype TEXT, attachment_size_bytes TEXT, message_row_id TEXT, chat_id TEXT, from_me TEXT); INSERT INTO sms (message_timestamp, message, chat_contact_id, chat_id, from_me) VALUES (1717840800, 'hi', '+15551234567', '1', '0');"
"#
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Skip if sqlite3 CLI isn't available on this machine.
        if Command::sqlite3_missing() {
            eprintln!("skipping: sqlite3 CLI not found");
            return;
        }

        let cfg = EngineConfig::frozen(&script);
        let cache_path = tmp.path().join("cache.db");
        let work_dir = tmp.path().join("work");
        let mut phases = Vec::new();

        let outcome = import_backup(
            &cfg,
            tmp.path(),
            "pw",
            &cache_path,
            &work_dir,
            &[],
            &CancelToken::new(),
            |ph| phases.push(ph),
        )
        .unwrap();

        assert_eq!(outcome.report.messages, 1);
        assert_eq!(outcome.report.threads, 1);
        assert!(matches!(phases[0], ImportPhase::Parsing(_)));
        assert!(matches!(phases[phases.len() - 2], ImportPhase::Normalizing));
        assert!(matches!(phases[phases.len() - 1], ImportPhase::Done(_)));

        let n: i64 = Connection::open(&cache_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[cfg(unix)]
    #[test]
    fn reimport_is_idempotent() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        if Command::sqlite3_missing() {
            eprintln!("skipping: sqlite3 CLI not found");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fake_ileapp.sh");
        {
            let mut f = std::fs::File::create(&script).unwrap();
            writeln!(
                f,
                r#"#!/bin/sh
out=""
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2;; *) shift;; esac; done
sub="$out/iLEAPP_Output_test"
mkdir -p "$sub"
sqlite3 "$sub/_lava_artifacts.db" "CREATE TABLE sms (message_timestamp INTEGER, read_timestamp INTEGER, message TEXT, service TEXT, message_direction TEXT, message_sent TEXT, message_delivered TEXT, message_read TEXT, account TEXT, account_login TEXT, chat_contact_id TEXT, attachment_name TEXT, attachment_file TEXT, attachment_timestamp INTEGER, attachment_mimetype TEXT, attachment_size_bytes TEXT, message_row_id TEXT, chat_id TEXT, from_me TEXT); INSERT INTO sms (message_timestamp, message, chat_contact_id, chat_id, from_me) VALUES (1717840800, 'hi', '+15551234567', '1', '0');"
"#
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let cfg = EngineConfig::frozen(&script);
        let cache_path = tmp.path().join("cache.db");
        let work_dir = tmp.path().join("work");
        let run = || {
            import_backup(
                &cfg,
                tmp.path(),
                "pw",
                &cache_path,
                &work_dir,
                &[],
                &CancelToken::new(),
                |_| {},
            )
            .unwrap()
        };

        // Import the same backup twice into the same paths.
        assert_eq!(run().report.messages, 1);
        assert_eq!(run().report.messages, 1);

        // The cache must hold one message, not two — re-import replaced, not
        // appended. And the work dir holds a single engine output.
        let n: i64 = Connection::open(&cache_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "re-import must not duplicate rows");
        let outputs = std::fs::read_dir(&work_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("iLEAPP_Output_")
            })
            .count();
        assert_eq!(outputs, 1, "stale engine outputs must not accumulate");
    }

    // Small helper so the test can gracefully skip without sqlite3.
    struct Command;
    impl Command {
        fn sqlite3_missing() -> bool {
            std::process::Command::new("sqlite3")
                .arg("-version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_err()
        }
    }
}
