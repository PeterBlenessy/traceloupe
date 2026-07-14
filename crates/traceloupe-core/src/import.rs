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
    /// Reading iLEAPP's output into the cache DB; `step` is the sub-stage being
    /// processed (e.g. "Messages", "TikTok messages", "Camera roll") so the UI
    /// can show live progress instead of one opaque "organizing" spinner.
    Normalizing { step: String },
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

    let lava_path = sidecar::run_import(
        cfg,
        backup_dir,
        password,
        work_dir,
        module_ids,
        cancel,
        |p| on_phase(ImportPhase::Parsing(p)),
    )?;

    on_phase(ImportPhase::Normalizing {
        step: "Reading results".into(),
    });
    let engine_out_dir = lava_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| work_dir.to_path_buf());

    let cache = CacheDb::open(cache_path)?;
    let effective = sidecar::effective_module_ids(module_ids);

    // Build the backup decryptor once (encrypted backups) — reused for native
    // Messages and the camera roll. An empty password means an unencrypted
    // backup, so no decryptor is needed. iLEAPP already ran with this password,
    // so a failure here is unexpected; degrade with a warning.
    let mut pre_warnings: Vec<String> = Vec::new();
    let decryptor = if password.is_empty() {
        None
    } else {
        match crate::crypto::BackupDecryptor::open(backup_dir, password) {
            Ok(d) => Some(d),
            Err(e) => {
                pre_warnings.push(format!("Encrypted native reads unavailable: {e}"));
                None
            }
        }
    };

    // Phase 2: materialize Messages natively from the backup's sms.db (via the
    // Manifest Index), skipping iLEAPP's eager sms pass. Falls back to the iLEAPP
    // path when Messages are disabled, sms.db isn't in the backup, or the native
    // parse fails.
    let mut native = ImportReport::default();
    let native_messages = effective.contains(&"messages") && {
        on_phase(ImportPhase::Normalizing {
            step: "Messages".into(),
        });
        import_messages_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        )
    };

    // Phase 2: materialize Notes natively from NoteStore.sqlite (via the Manifest
    // Index), skipping iLEAPP's eager notes pass. Same fallback contract as
    // Messages: any miss/parse failure declines and the iLEAPP path runs.
    let native_notes = effective.contains(&"notes") && {
        on_phase(ImportPhase::Normalizing {
            step: "Notes".into(),
        });
        import_notes_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        )
    };

    // Read iLEAPP's output into the cache (each normalizer reports its sub-stage);
    // the Messages/Notes stages are skipped when handled natively above.
    let mut report = normalize::normalize_lava_with_progress(
        &lava_path,
        &engine_out_dir,
        &cache,
        native_messages,
        native_notes,
        |step| on_phase(ImportPhase::Normalizing { step: step.into() }),
    )?;
    report.threads += native.threads;
    report.messages += native.messages;
    report.notes += native.notes;
    report.warnings.extend(pre_warnings);
    report.warnings.extend(native.warnings);

    on_phase(ImportPhase::Normalizing {
        step: "Camera roll".into(),
    });
    // Camera roll: read the backup's Manifest natively and reference iOS's own
    // thumbnails, so the gallery is fast and full images transcode on demand.
    if effective.contains(&"camera_roll") {
        // Reuses the decryptor built once above (None for unencrypted backups).
        // Decrypted thumbnails and transient decrypted DBs live beside the cache.
        // remove_cache (at import start) already cleared this, so no wipe here.
        let media_cache_dir = cache_path
            .parent()
            .map(|p| p.join("media"))
            .unwrap_or_else(|| work_dir.join("media"));

        match crate::parsers::camera_roll::parse_camera_roll(
            backup_dir,
            decryptor.as_ref(),
            &media_cache_dir,
        ) {
            Ok(assets) => {
                // One transaction for the whole camera roll (can be ~10k rows) —
                // a commit per row is what stalls a large import.
                let conn = cache.conn();
                let tx = conn.unchecked_transaction()?;
                for a in &assets {
                    tx.execute(
                        "INSERT INTO media_items
                            (domain, relative_path, kind, source, mime_type,
                             taken_at, thumb_path, local_path, decrypt_key, plain_size)
                         VALUES ('CameraRollDomain', ?1, ?2, 'Photos', ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            a.relative_path,
                            a.kind,
                            a.mime,
                            a.taken_at,
                            a.thumb_path
                                .as_ref()
                                .map(|p| p.to_string_lossy().into_owned()),
                            a.full_path.to_string_lossy(),
                            a.decrypt_key,
                            a.plain_size,
                        ],
                    )?;
                }
                tx.commit()?;
                report.media_items += assets.len();
            }
            Err(e) => report
                .warnings
                .push(format!("Camera roll: couldn't read the backup ({e}).")),
        }
    }

    // Voice recordings: read Voice Memos metadata + `.m4a` blobs natively (they
    // decrypt on demand at play time, like the camera roll). No iLEAPP fallback —
    // there's no recordings normalizer — so a failure is just a warning.
    if effective.contains(&"recordings") {
        on_phase(ImportPhase::Normalizing {
            step: "Voice recordings".into(),
        });
        match crate::parsers::recordings::parse_recordings(backup_dir, decryptor.as_ref(), work_dir)
        {
            Ok(recordings) => {
                let conn = cache.conn();
                let tx = conn.unchecked_transaction()?;
                for rec in &recordings {
                    tx.execute(
                        "INSERT INTO recordings
                            (title, folder, recorded_at, duration_s, relative_path,
                             local_path, mime_type, decrypt_key, plain_size)
                         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            rec.title,
                            rec.recorded_at,
                            rec.duration_s,
                            rec.relative_path,
                            rec.full_path.to_string_lossy(),
                            rec.mime,
                            rec.decrypt_key,
                            rec.plain_size,
                        ],
                    )?;
                }
                tx.commit()?;
                report.recordings += recordings.len();
            }
            Err(e) => report
                .warnings
                .push(format!("Voice recordings: couldn't read the backup ({e}).")),
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
            report.warnings.push(format!(
                "{label}: nothing found — the source data isn't in this backup."
            ));
        }
    }

    on_phase(ImportPhase::Normalizing {
        step: "Installed apps".into(),
    });
    // Record which apps were on the device (from Info.plist) for the Apps view.
    let apps = crate::discovery::installed_apps(backup_dir);
    {
        let conn = cache.conn();
        let tx = conn.unchecked_transaction()?;
        for bundle_id in &apps {
            tx.execute(
                "INSERT OR IGNORE INTO installed_apps (bundle_id) VALUES (?1)",
                [bundle_id],
            )?;
        }
        tx.commit()?;
    }

    on_phase(ImportPhase::Done(report.clone()));
    Ok(ImportOutcome {
        cache_path: cache_path.to_path_buf(),
        report,
    })
}

/// Materialize Messages natively: locate `sms.db` via the Manifest Index,
/// decrypt/extract it to a temp file, and parse it into the cache. Returns
/// whether it succeeded (sms.db present + parsed); on any miss or error the
/// caller falls back to the iLEAPP `sms` path. `parse_messages` commits in one
/// transaction, so a failure leaves no partial rows to duplicate.
fn import_messages_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) -> bool {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Messages unavailable ({e}); using iLEAPP."));
            return false;
        }
    };
    let entry = match index.find("HomeDomain", "Library/SMS/sms.db") {
        Ok(Some(e)) => e,
        Ok(None) => return false, // sms.db not in this backup → iLEAPP path
        Err(e) => {
            // A real Manifest read error is worth surfacing, unlike a plain absence.
            report.warnings.push(format!(
                "Native Messages: Manifest read failed ({e}); using iLEAPP."
            ));
            return false;
        }
    };
    let sms_db = work_dir.join(".sms.db");
    if let Err(e) = index.extract_to(&entry, decryptor, &sms_db) {
        let _ = std::fs::remove_file(&sms_db);
        report.warnings.push(format!(
            "Native Messages: couldn't read sms.db ({e}); using iLEAPP."
        ));
        return false;
    }
    let att = crate::parsers::messages::AttachmentSource {
        index: &index,
        decryptor,
    };
    let ok =
        match crate::parsers::messages::parse_messages(&sms_db, cache, report, false, Some(&att)) {
            Ok(()) => true,
            Err(e) => {
                report.warnings.push(format!(
                    "Native Messages: parse failed ({e}); using iLEAPP."
                ));
                false
            }
        };
    let _ = std::fs::remove_file(&sms_db);
    ok
}

/// Materialize Notes natively: locate `NoteStore.sqlite` via the Manifest Index,
/// decrypt/extract it to a temp file, and parse it into the cache. Returns
/// whether it succeeded (DB present + parsed); on any miss or error the caller
/// falls back to the iLEAPP `notes` path. `parse_notes` commits in one
/// transaction, so a failure leaves no partial rows to duplicate.
fn import_notes_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) -> bool {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Notes unavailable ({e}); using iLEAPP."));
            return false;
        }
    };
    let entry = match index.find("AppDomainGroup-group.com.apple.notes", "NoteStore.sqlite") {
        Ok(Some(e)) => e,
        Ok(None) => return false, // NoteStore.sqlite not in this backup → iLEAPP path
        Err(e) => {
            report.warnings.push(format!(
                "Native Notes: Manifest read failed ({e}); using iLEAPP."
            ));
            return false;
        }
    };
    let note_store = work_dir.join(".NoteStore.sqlite");
    if let Err(e) = index.extract_to(&entry, decryptor, &note_store) {
        let _ = std::fs::remove_file(&note_store);
        report.warnings.push(format!(
            "Native Notes: couldn't read NoteStore.sqlite ({e}); using iLEAPP."
        ));
        return false;
    }
    let ok = match crate::parsers::notes::parse_notes(&note_store, cache, report, false) {
        Ok(()) => true,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Notes: parse failed ({e}); using iLEAPP."));
            false
        }
    };
    let _ = std::fs::remove_file(&note_store);
    ok
}

/// Remove a SQLite cache DB and its WAL/SHM sidecars, if present.
/// Remove a backup's cache DB and all data derived from it: the WAL/SHM
/// sidecars, and the sibling `media` / `thumbs` / `att-thumbs` directories
/// (decrypted thumbnails and sips-converted JPEGs). Consolidated here so both
/// re-import and "forget backup" clean up everything consistently — a re-import
/// never serves a previous run's stale media, and forgetting leaves nothing.
pub fn remove_cache(cache_path: &Path) {
    let _ = std::fs::remove_file(cache_path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = cache_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(sidecar);
    }
    if let Some(dir) = cache_path.parent() {
        for sub in ["media", "thumbs", "att-thumbs", "att-open"] {
            let _ = std::fs::remove_dir_all(dir.join(sub));
        }
    }
}

/// The natively-parsed data types that can be re-imported on their own — no
/// iLEAPP, so it's fast. The UI offers a "re-import" action only for these.
pub const REIMPORTABLE_NATIVE: &[&str] = &["recordings", "camera_roll", "messages", "notes"];

/// Re-run one native data type into an existing cache, replacing just that type's
/// rows. Unlike [`import_backup`] this skips iLEAPP entirely, so it's cheap — for
/// refreshing a single view or picking up a parser fix without a full re-import.
///
/// The decrypt + parse runs to completion BEFORE any existing rows are deleted,
/// so a failure (e.g. a file that won't decrypt) leaves the current data intact.
pub fn reimport_module(
    module_id: &str,
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache_path: &Path,
    work_dir: &Path,
) -> Result<ImportReport> {
    let _ = std::fs::create_dir_all(work_dir);
    let cache = CacheDb::open(cache_path)?;
    let mut report = ImportReport::default();

    match module_id {
        "recordings" => {
            let recs =
                crate::parsers::recordings::parse_recordings(backup_dir, decryptor, work_dir)?;
            let conn = cache.conn();
            let tx = conn.unchecked_transaction()?;
            tx.execute("DELETE FROM recordings", [])?;
            for rec in &recs {
                tx.execute(
                    "INSERT INTO recordings
                        (title, folder, recorded_at, duration_s, relative_path,
                         local_path, mime_type, decrypt_key, plain_size)
                     VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        rec.title,
                        rec.recorded_at,
                        rec.duration_s,
                        rec.relative_path,
                        rec.full_path.to_string_lossy(),
                        rec.mime,
                        rec.decrypt_key,
                        rec.plain_size,
                    ],
                )?;
            }
            tx.commit()?;
            report.recordings = recs.len();
        }
        "camera_roll" => {
            let media_cache_dir = cache_path
                .parent()
                .map(|p| p.join("media"))
                .unwrap_or_else(|| work_dir.join("media"));
            let assets = crate::parsers::camera_roll::parse_camera_roll(
                backup_dir,
                decryptor,
                &media_cache_dir,
            )?;
            let conn = cache.conn();
            let tx = conn.unchecked_transaction()?;
            // Only the camera roll (source 'Photos'); message/app attachments in
            // media_items are left alone.
            tx.execute("DELETE FROM media_items WHERE source = 'Photos'", [])?;
            for a in &assets {
                tx.execute(
                    "INSERT INTO media_items
                        (domain, relative_path, kind, source, mime_type,
                         taken_at, thumb_path, local_path, decrypt_key, plain_size)
                     VALUES ('CameraRollDomain', ?1, ?2, 'Photos', ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        a.relative_path,
                        a.kind,
                        a.mime,
                        a.taken_at,
                        a.thumb_path
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned()),
                        a.full_path.to_string_lossy(),
                        a.decrypt_key,
                        a.plain_size,
                    ],
                )?;
            }
            tx.commit()?;
            report.media_items = assets.len();
        }
        "messages" => {
            // Extract sms.db first — the decrypt happens here, so a failure aborts
            // before we delete anything. Only then swap the iMessage/SMS rows
            // (app-chat threads — TikTok/WhatsApp/Telegram — are left untouched).
            let index = crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir)?;
            let entry = index
                .find("HomeDomain", "Library/SMS/sms.db")?
                .ok_or_else(|| crate::Error::Parse("sms.db is not in this backup".into()))?;
            let sms_db = work_dir.join(".reimport-sms.db");
            if let Err(e) = index.extract_to(&entry, decryptor, &sms_db) {
                let _ = std::fs::remove_file(&sms_db);
                return Err(e);
            }
            let att = crate::parsers::messages::AttachmentSource {
                index: &index,
                decryptor,
            };
            // replace=true does the delete + re-insert atomically (see parse_messages).
            let r = crate::parsers::messages::parse_messages(
                &sms_db,
                &cache,
                &mut report,
                true,
                Some(&att),
            );
            let _ = std::fs::remove_file(&sms_db);
            r?;
        }
        "notes" => {
            let index = crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir)?;
            let entry = index
                .find("AppDomainGroup-group.com.apple.notes", "NoteStore.sqlite")?
                .ok_or_else(|| {
                    crate::Error::Parse("NoteStore.sqlite is not in this backup".into())
                })?;
            let note_db = work_dir.join(".reimport-notes.db");
            if let Err(e) = index.extract_to(&entry, decryptor, &note_db) {
                let _ = std::fs::remove_file(&note_db);
                return Err(e);
            }
            // replace=true clears + re-inserts atomically (see parse_notes).
            let r = crate::parsers::notes::parse_notes(&note_db, &cache, &mut report, true);
            let _ = std::fs::remove_file(&note_db);
            r?;
        }
        other => {
            return Err(crate::Error::Parse(format!(
                "'{other}' cannot be re-imported on its own"
            )))
        }
    }
    Ok(report)
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
        assert!(phases
            .iter()
            .any(|p| matches!(p, ImportPhase::Normalizing { .. })));
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

    /// A plaintext backup whose Manifest.db points at an `sms.db` blob: the
    /// native path resolves it via the Manifest Index and parses it into the
    /// cache, no iLEAPP involved.
    #[test]
    fn native_messages_from_a_plaintext_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();

        // sms.db stored content-addressed at <backup>/<id[:2]>/<id>.
        let sms_id = "ab00000000000000000000000000000000000099";
        let sub = backup.join(&sms_id[..2]);
        std::fs::create_dir_all(&sub).unwrap();
        let sms = Connection::open(sub.join(sms_id)).unwrap();
        sms.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT, service_name TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111');
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat_handle_join VALUES (10,1);
             INSERT INTO message VALUES (100,'hi',0,721692800000000000,1,0);
             INSERT INTO chat_message_join VALUES (10,100);",
        )
        .unwrap();
        drop(sms);

        // Manifest.db resolving HomeDomain/Library/SMS/sms.db → that blob.
        Connection::open(backup.join("Manifest.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
                 INSERT INTO Files VALUES ('{sms_id}','HomeDomain','Library/SMS/sms.db',1,NULL);"
            ))
            .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut report = ImportReport::default();

        let ok = import_messages_native(&backup, None, &cache, &work, &mut report);
        assert!(
            ok,
            "native path should succeed; warnings: {:?}",
            report.warnings
        );
        assert_eq!(report.messages, 1);
        assert_eq!(report.threads, 1);
        let n: i64 = cache
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    /// Native Messages resolves an attachment to its backup file and writes an
    /// `attachments` row with the servable path.
    #[test]
    fn native_messages_resolves_attachments() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();

        let sms_id = "ab00000000000000000000000000000000000042";
        let att_id = "cd00000000000000000000000000000000000043";
        for (id, bytes) in [(sms_id, None), (att_id, Some(b"jpeg".as_slice()))] {
            let sub = backup.join(&id[..2]);
            std::fs::create_dir_all(&sub).unwrap();
            if let Some(b) = bytes {
                std::fs::write(sub.join(id), b).unwrap();
            }
        }
        let sms = Connection::open(backup.join(&sms_id[..2]).join(sms_id)).unwrap();
        sms.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT, service_name TEXT);
             CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             CREATE TABLE attachment (ROWID INTEGER PRIMARY KEY, filename TEXT, transfer_name TEXT, mime_type TEXT);
             CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111');
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat_handle_join VALUES (10,1);
             -- an attachment-only message (NULL text, has_attachments=1).
             INSERT INTO message VALUES (100,NULL,0,721692800000000000,1,1);
             INSERT INTO chat_message_join VALUES (10,100);
             INSERT INTO attachment VALUES (5,'~/Library/SMS/Attachments/ab/00/GUID/pic.jpg','pic.jpg','image/jpeg');
             INSERT INTO message_attachment_join VALUES (100,5);",
        )
        .unwrap();
        drop(sms);

        Connection::open(backup.join("Manifest.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
                 INSERT INTO Files VALUES ('{sms_id}','HomeDomain','Library/SMS/sms.db',1,NULL);
                 INSERT INTO Files VALUES ('{att_id}','MediaDomain','Library/SMS/Attachments/ab/00/GUID/pic.jpg',1,NULL);"
            ))
            .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut report = ImportReport::default();

        assert!(import_messages_native(
            &backup,
            None,
            &cache,
            &work,
            &mut report
        ));

        let (filename, mime, local_path): (Option<String>, Option<String>, String) = cache
            .conn()
            .query_row(
                "SELECT filename, mime_type, local_path FROM attachments",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(filename.as_deref(), Some("pic.jpg"));
        assert_eq!(mime.as_deref(), Some("image/jpeg"));
        // Resolved to the content-addressed blob (plaintext backup → no key).
        assert!(local_path.ends_with(&format!("{}/{att_id}", &att_id[..2])));
    }

    /// No sms.db in the backup → the native path declines (returns false) so the
    /// caller falls back to the iLEAPP `sms` stage, writing nothing itself.
    #[test]
    fn native_messages_absent_sms_db_declines() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        Connection::open(backup.join("Manifest.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE Files (fileID TEXT, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
            )
            .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        let ok = import_messages_native(&backup, None, &cache, tmp.path(), &mut report);
        assert!(!ok);
        assert_eq!(report.messages, 0);
    }

    /// A plaintext backup whose Manifest.db points at a `NoteStore.sqlite` blob:
    /// the native path resolves it and parses the note into the cache. (Body
    /// protobuf decoding is covered by parsers::notes; here a NULL-body note is
    /// enough to exercise the manifest → parse → cache wiring.)
    #[test]
    fn native_notes_from_a_plaintext_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();

        let note_id = "cd00000000000000000000000000000000000077";
        let sub = backup.join(&note_id[..2]);
        std::fs::create_dir_all(&sub).unwrap();
        let ns = Connection::open(sub.join(note_id)).unwrap();
        ns.execute_batch(
            "CREATE TABLE ZICNOTEDATA (Z_PK INTEGER PRIMARY KEY, ZNOTE INTEGER, ZDATA BLOB);
             CREATE TABLE ZICCLOUDSYNCINGOBJECT (
                Z_PK INTEGER PRIMARY KEY, ZTITLE1 TEXT, ZTITLE2 TEXT, ZSNIPPET TEXT,
                ZFOLDER INTEGER, ZNOTEDATA INTEGER,
                ZCREATIONDATE1 REAL, ZMODIFICATIONDATE1 REAL, ZMARKEDFORDELETION INTEGER);
             INSERT INTO ZICCLOUDSYNCINGOBJECT (Z_PK, ZTITLE2) VALUES (1, 'Notes');
             INSERT INTO ZICNOTEDATA (Z_PK, ZNOTE, ZDATA) VALUES (5, 10, NULL);
             INSERT INTO ZICCLOUDSYNCINGOBJECT
                (Z_PK, ZTITLE1, ZSNIPPET, ZFOLDER, ZNOTEDATA, ZCREATIONDATE1, ZMODIFICATIONDATE1)
             VALUES (10, 'Reminder', 'call the plumber', 1, 5, 721692800.0, 721692900.0);",
        )
        .unwrap();
        drop(ns);

        Connection::open(backup.join("Manifest.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
                 INSERT INTO Files VALUES ('{note_id}','AppDomainGroup-group.com.apple.notes','NoteStore.sqlite',1,NULL);"
            ))
            .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut report = ImportReport::default();

        let ok = import_notes_native(&backup, None, &cache, &work, &mut report);
        assert!(
            ok,
            "native notes should succeed; warnings: {:?}",
            report.warnings
        );
        assert_eq!(report.notes, 1);
        let (folder, title): (Option<String>, Option<String>) = cache
            .conn()
            .query_row("SELECT folder, title FROM notes", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(folder.as_deref(), Some("Notes"));
        assert_eq!(title.as_deref(), Some("Reminder"));
    }

    /// Re-importing a native type replaces its rows rather than appending: two
    /// runs leave the same count, and the row count reflects the backup, not the
    /// number of runs.
    #[test]
    fn reimport_recordings_replaces_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();

        let m4a_id = "ee00000000000000000000000000000000000009";
        let sub = backup.join(&m4a_id[..2]);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(m4a_id), b"m4a").unwrap();
        Connection::open(backup.join("Manifest.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
                 INSERT INTO Files VALUES ('{m4a_id}','MediaDomain','Recordings/memo.m4a',1,NULL);"
            ))
            .unwrap();

        let cache_path = tmp.path().join("cache.db");
        let work = tmp.path().join("work");

        for _ in 0..2 {
            let report = reimport_module("recordings", &backup, None, &cache_path, &work).unwrap();
            assert_eq!(report.recordings, 1);
        }
        // Still one row after two runs — the DELETE ran, no duplication.
        let cache = CacheDb::open(&cache_path).unwrap();
        let n: i64 = cache
            .conn()
            .query_row("SELECT COUNT(*) FROM recordings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
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
