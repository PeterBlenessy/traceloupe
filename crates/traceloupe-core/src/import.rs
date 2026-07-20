//! Import orchestration (architecture §6): parse a backup natively into a fresh
//! cache DB in one eager, whole-backup pass; every browse afterward is a cache
//! query. Everything TraceLoupe surfaces is now parsed natively — iLEAPP is NOT
//! run (no catalog module carries an iLEAPP key). The sidecar/normalize path is
//! kept, dormant, only so a future long-tail module could opt back in; iLEAPP
//! itself is a development-time reference for schemas we can't inspect directly,
//! never a runtime dependency.

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
    /// Building the searchable indexes from the backup's own databases. `step` is
    /// the ready-to-display label (e.g. "Indexing Messages", "Indexing Photos"),
    /// and `index`/`total` let the UI fill the bar `index/total` across the run
    /// instead of pinning it at one opaque value.
    Indexing {
        step: String,
        index: u32,
        total: u32,
    },
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
    // iLEAPP engine — optional. Every artifact TraceLoupe surfaces is parsed
    // natively now, so a normal import never touches iLEAPP and `cfg` can be None.
    // It's only consulted if a module ever reintroduces an iLEAPP key.
    cfg: Option<&EngineConfig>,
    backup_dir: &Path,
    password: &str,
    cache_path: &Path,
    work_dir: &Path,
    module_ids: &[String],
    cancel: &CancelToken,
    mut on_phase: impl FnMut(ImportPhase),
) -> Result<ImportOutcome> {
    // iLEAPP writes a fresh timestamped output folder each run, so wipe its
    // scratch dir up front — it's transient, not user data.
    let _ = std::fs::remove_dir_all(work_dir);

    // Atomic-swap re-import: build the new cache in a temp file beside the real
    // one, leaving the existing cache LIVE (and browsable) for the whole run. Only
    // on success do we swap it in; a cancel or failure discards the temp and the
    // previous import stays completely intact. The guard removes the temp on any
    // early return (cancel/error).
    let import_cache_path = cache_path.with_file_name("cache.importing.db");
    remove_cache_file(&import_cache_path);
    let mut temp_guard = TempCacheGuard {
        path: &import_cache_path,
        committed: false,
    };

    // Every artifact TraceLoupe surfaces is parsed natively, so no module carries
    // iLEAPP keys and this is always empty — iLEAPP never runs. The branch stays
    // only so a future long-tail module could opt back in (and needs `cfg`).
    let needs_ileapp = !sidecar::resolve_module_keys(module_ids).is_empty();
    let (lava_path, engine_out_dir) = match (needs_ileapp, cfg) {
        (true, Some(cfg)) => {
            let lava = sidecar::run_import(
                cfg,
                backup_dir,
                password,
                work_dir,
                module_ids,
                cancel,
                |p| on_phase(ImportPhase::Parsing(p)),
            )?;
            let dir = lava
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| work_dir.to_path_buf());
            (Some(lava), dir)
        }
        // A module wanted iLEAPP but no engine is available — degrade to native.
        _ => (None, work_dir.to_path_buf()),
    };

    let effective = sidecar::effective_module_ids(module_ids);

    // Total number of indexing steps we'll emit, so the UI can fill the bar
    // `index/total`. Seven always run (Preparing, Calendar, Reminders, Health,
    // Interactions, App Chats, Installed Apps); Safari, TikTok and the camera roll
    // each contribute two. KEEP IN SYNC with the `step!(…)` calls below.
    let index_total: u32 = 7
        + effective.contains(&"messages") as u32
        + effective.contains(&"notes") as u32
        + effective.contains(&"calls") as u32
        + effective.contains(&"safari") as u32 * 2
        + effective.contains(&"contacts") as u32
        + effective.contains(&"tiktok") as u32 * 2
        + effective.contains(&"camera_roll") as u32 * 2
        + effective.contains(&"recordings") as u32;
    let mut step_i: u32 = 0;
    // Emit the next indexing step with a running `index/total`. `$label` is the
    // ready-to-display string (e.g. "Indexing Messages").
    macro_rules! step {
        ($label:expr) => {{
            step_i += 1;
            on_phase(ImportPhase::Indexing {
                step: $label.into(),
                index: step_i,
                total: index_total,
            });
        }};
    }

    step!("Preparing");

    // All writes go to the temp cache; the real one keeps serving the UI.
    let cache = CacheDb::open(&import_cache_path)?;

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
        step!("Indexing Messages");
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
        step!("Indexing Notes");
        import_notes_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        )
    };

    // Phase 2: materialize Calls natively from CallHistory.storedata, and Safari
    // history from History.db (both via the Manifest Index). Same fallback
    // contract as Messages/Notes — any miss/parse failure declines and iLEAPP runs.
    let native_calls = effective.contains(&"calls") && {
        step!("Indexing Call History");
        import_calls_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        )
    };
    let native_safari = effective.contains(&"safari") && {
        step!("Indexing Safari History");
        import_safari_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        )
    };
    // Safari bookmarks / reading list / open tabs (native-only; no iLEAPP path).
    if effective.contains(&"safari") {
        step!("Indexing Safari Bookmarks");
        import_safari_bookmarks_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
            false,
        );
    }
    // Calendar events (native-only; no iLEAPP path). Always attempted — cheap and
    // best-effort, so a missing/unreadable Calendar.sqlitedb is just a warning.
    {
        step!("Indexing Calendar");
        import_calendar_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        );
    }
    // Reminders (native-only; best-effort).
    {
        step!("Indexing Reminders");
        import_reminders_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        );
    }
    // Health workouts + summary (native-only; best-effort).
    {
        step!("Indexing Health");
        import_health_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        );
    }
    // CoreDuet cross-app interaction graph (native-only; best-effort).
    {
        step!("Indexing Interactions");
        import_interactions_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        );
    }

    // Phase 2: self-extract + parse Contacts from AddressBook.sqlitedb, so we no
    // longer depend on iLEAPP to extract it for us.
    let native_contacts = effective.contains(&"contacts") && {
        step!("Indexing Contacts");
        import_contacts_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        )
    };

    // Phase 2: materialize third-party chats (WhatsApp, …) natively from each
    // app's own SQLite DB via the pluggable app-module registry. Returns the
    // service labels handled, so the matching iLEAPP stages are skipped.
    step!("Indexing App Chats");
    let native_app_services = import_app_chats_native(
        backup_dir,
        decryptor.as_ref(),
        &cache,
        work_dir,
        &mut native,
    );

    // TikTok contacts / social graph, natively from AwemeIM.db (the same DB the
    // chat parser reads) into the Contacts view — the last artifact that used to
    // need iLEAPP.
    if effective.contains(&"tiktok") {
        step!("Indexing TikTok Messages");
        import_tiktok_messages_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        );
        step!("Indexing TikTok Contacts");
        import_tiktok_contacts_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut native,
        );
    }

    // Read iLEAPP's output into the cache (each normalizer reports its sub-stage);
    // stages materialized natively above are skipped. Skipped entirely when iLEAPP
    // didn't run (fully-native import).
    let mut report = if let Some(lava) = &lava_path {
        normalize::normalize_lava_with_progress(
            lava,
            &engine_out_dir,
            &cache,
            normalize::NativeSkips {
                messages: native_messages,
                notes: native_notes,
                calls: native_calls,
                safari: native_safari,
                contacts: native_contacts,
                app_services: native_app_services,
            },
            // Dead path (iLEAPP never runs); label without disturbing the counter.
            |s| {
                on_phase(ImportPhase::Indexing {
                    step: format!("Indexing {s}"),
                    index: step_i,
                    total: index_total,
                })
            },
        )?
    } else {
        ImportReport::default()
    };
    report.threads += native.threads;
    report.messages += native.messages;
    report.notes += native.notes;
    report.calls += native.calls;
    report.safari_visits += native.safari_visits;
    report.contacts += native.contacts;
    report.calendar_events += native.calendar_events;
    report.reminders += native.reminders;
    report.workouts += native.workouts;
    report.interactions += native.interactions;
    report.warnings.extend(pre_warnings);
    report.warnings.extend(native.warnings);

    // Camera roll: read the backup's Manifest natively and reference iOS's own
    // thumbnails, so the gallery is fast and full images transcode on demand.
    if effective.contains(&"camera_roll") {
        step!("Indexing Photos");
        // Reuses the decryptor built once above (None for unencrypted backups).
        // The content-addressed `media` dir (decrypted thumbnails, keyed by
        // fileID) is shared across runs and NOT wiped for the atomic swap, so a
        // re-import reuses thumbnails it already decrypted instead of redoing them.
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
        // Enrich the camera-roll rows just inserted with the people detected in
        // each photo (Photos.sqlite face recognition) — powers person search/tags.
        step!("Indexing People in Photos");
        import_photos_metadata_native(
            backup_dir,
            decryptor.as_ref(),
            &cache,
            work_dir,
            &mut report,
        );
    }

    // Voice recordings: read Voice Memos metadata + `.m4a` blobs natively (they
    // decrypt on demand at play time, like the camera roll). No iLEAPP fallback —
    // there's no recordings normalizer — so a failure is just a warning.
    if effective.contains(&"recordings") {
        step!("Indexing Voice Recordings");
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
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            rec.title,
                            rec.folder,
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
    // Safari/Calls (usually the source DB isn't in this backup) is visible instead
    // of silently absent. When the native path actually parsed the source (sms.db /
    // NoteStore.sqlite present), a 0 count means "present but empty", not "absent".
    for id in effective {
        let (label, count, source_present) = match id {
            "messages" => ("Messages", report.messages, native_messages),
            "calls" => ("Call history", report.calls, native_calls),
            "contacts" => ("Contacts", report.contacts, native_contacts),
            "safari" => ("Safari history", report.safari_visits, native_safari),
            "notes" => ("Notes", report.notes, native_notes),
            // camera_roll isn't checked here: media_items also holds message/app
            // attachments, so a 0-count test wouldn't be meaningful.
            _ => continue,
        };
        if count == 0 {
            report.warnings.push(if source_present {
                format!("{label}: none found — the source is present in this backup but empty.")
            } else {
                format!("{label}: nothing found — the source data isn't in this backup.")
            });
        }
    }

    step!("Indexing Installed Apps");
    // Record which apps were on the device + their App Store metadata (name,
    // seller, version, genre, release date) from Info.plist's iTunesMetadata.
    let apps = crate::discovery::installed_apps_meta(backup_dir);
    {
        let conn = cache.conn();
        let tx = conn.unchecked_transaction()?;
        for app in &apps {
            tx.execute(
                "INSERT OR REPLACE INTO installed_apps
                     (bundle_id, name, seller, version, genre, released)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    app.bundle_id,
                    app.name,
                    app.seller,
                    app.version,
                    app.genre,
                    app.released,
                ],
            )?;
        }
        tx.commit()?;
    }

    // Everything landed in the temp cache. Explicitly checkpoint + truncate its
    // WAL so the `.db` is self-contained regardless of close-time behavior (belt
    // and suspenders against a future stray open), then close it and swap it in.
    let _ = cache
        .conn()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    drop(cache);
    swap_cache_into_place(&import_cache_path, cache_path)?;
    temp_guard.committed = true;

    on_phase(ImportPhase::Done(report.clone()));
    Ok(ImportOutcome {
        cache_path: cache_path.to_path_buf(),
        report,
    })
}

/// Removes its temp cache on drop unless the import committed (swapped it in), so
/// a cancel/failure/panic never leaves a stray `cache.importing.db` behind.
struct TempCacheGuard<'a> {
    path: &'a Path,
    committed: bool,
}

impl Drop for TempCacheGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            remove_cache_file(self.path);
        }
    }
}

/// Remove a cache DB file and its WAL/SHM sidecars (not the sibling media dirs).
fn remove_cache_file(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(p);
    }
}

/// Retire the previous cache and move the freshly-built `temp` DB into `final_path`.
///
/// The **rename happens first** (atomic same-directory replace), so if it fails we
/// return `Err` having deleted nothing — the old cache and its derived caches stay
/// intact. Only after a successful rename do we drop stale WAL/SHM sidecars (the
/// temp had none after the checkpoint; any at `final_path` are the retired cache's,
/// now orphaned) and the **id-keyed** derived caches — rendered thumbnails,
/// attachment renders, decrypted-attachment copies — since media_item/attachment
/// ids are reassigned on re-import. Doing the sidecar cleanup right after the
/// rename also shrinks the window in which a concurrent reader could map a stale
/// SHM (harmless — no WAL data is pending on the old cache, and SHM is rebuilt).
/// The content-addressed `media` dir (keyed by fileID) is shared and kept, so a
/// re-import reuses already-decrypted thumbnails instead of redoing them.
fn swap_cache_into_place(temp: &Path, final_path: &Path) -> Result<()> {
    std::fs::rename(temp, final_path).map_err(|e| crate::Error::io(final_path, e))?;
    for base in [temp, final_path] {
        for suffix in ["-wal", "-shm"] {
            let mut p = base.as_os_str().to_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(p);
        }
    }
    if let Some(dir) = final_path.parent() {
        for sub in ["thumbs", "att-thumbs", "att-open"] {
            let _ = std::fs::remove_dir_all(dir.join(sub));
        }
    }
    Ok(())
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
    if let Err(e) = index.extract_db(&entry, decryptor, &sms_db) {
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
    if let Err(e) = index.extract_db(&entry, decryptor, &note_store) {
        let _ = std::fs::remove_file(&note_store);
        report.warnings.push(format!(
            "Native Notes: couldn't read NoteStore.sqlite ({e}); using iLEAPP."
        ));
        return false;
    }
    let img_src = crate::parsers::notes::NoteImageSource {
        index: &index,
        decryptor,
    };
    let ok =
        match crate::parsers::notes::parse_notes(&note_store, cache, report, false, Some(&img_src))
        {
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

/// Materialize the CoreDuet cross-app interaction graph from `interactionC.db`.
/// Native-only and best-effort.
fn import_interactions_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Interactions unavailable ({e})."));
            return;
        }
    };
    let entry = match index.find("HomeDomain", "Library/CoreDuet/People/interactionC.db") {
        Ok(Some(e)) => e,
        Ok(None) => return,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Interactions: Manifest read failed ({e})."));
            return;
        }
    };
    let out = work_dir.join(".interactionC.db");
    if let Err(e) = index.extract_to(&entry, decryptor, &out) {
        let _ = std::fs::remove_file(&out);
        report.warnings.push(format!(
            "Native Interactions: couldn't read interactionC.db ({e})."
        ));
        return;
    }
    if let Err(e) = crate::parsers::coreduet::parse_interactions(&out, cache, report, false) {
        report
            .warnings
            .push(format!("Native Interactions: parse failed ({e})."));
    }
    let _ = std::fs::remove_file(&out);
}

/// Materialize Apple Health workouts + a sample summary from
/// `healthdb_secure.sqlite`. Native-only and best-effort.
fn import_health_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Health unavailable ({e})."));
            return;
        }
    };
    let entry = match index.find("HealthDomain", "Health/healthdb_secure.sqlite") {
        Ok(Some(e)) => e,
        Ok(None) => return,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Health: Manifest read failed ({e})."));
            return;
        }
    };
    let out = work_dir.join(".healthdb_secure.sqlite");
    if let Err(e) = index.extract_to(&entry, decryptor, &out) {
        let _ = std::fs::remove_file(&out);
        report
            .warnings
            .push(format!("Native Health: couldn't read healthdb ({e})."));
        return;
    }
    if let Err(e) = crate::parsers::health::parse_health(&out, cache, report, false) {
        report
            .warnings
            .push(format!("Native Health: parse failed ({e})."));
    }
    let _ = std::fs::remove_file(&out);
}

/// Materialize Reminders natively from the reminders container. Its Core Data
/// store is `Container_v1/Stores/Data-<UUID>.sqlite` under the reminders domain,
/// and the container has several stores (only one holds the data) — so we try
/// every candidate in that domain and the parser no-ops on the empty ones.
fn import_reminders_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    const DOMAIN: &str = "AppDomainGroup-group.com.apple.reminders";
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Reminders unavailable ({e})."));
            return;
        }
    };
    let candidates = match index.find_relative_like("%Stores/Data-%.sqlite") {
        Ok(v) => v,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Reminders: Manifest read failed ({e})."));
            return;
        }
    };
    for (i, entry) in candidates
        .into_iter()
        .filter(|e| e.domain == DOMAIN)
        .enumerate()
    {
        let out = work_dir.join(format!(".reminders-{i}.sqlite"));
        if index.extract_to(&entry, decryptor, &out).is_err() {
            let _ = std::fs::remove_file(&out);
            continue;
        }
        // Append (the import runs against a fresh cache). The empty container
        // stores no-op; only the one holding ZREMCDREMINDER contributes rows.
        if let Err(e) = crate::parsers::reminders::parse_reminders(&out, cache, report, false) {
            report
                .warnings
                .push(format!("Native Reminders: parse failed ({e})."));
        }
        let _ = std::fs::remove_file(&out);
    }
}

/// Materialize Calendar events natively from `Calendar.sqlitedb`. Native-only and
/// best-effort: a missing/unreadable DB just logs a warning.
fn import_calendar_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Calendar unavailable ({e})."));
            return;
        }
    };
    let entry = match index.find("HomeDomain", "Library/Calendar/Calendar.sqlitedb") {
        Ok(Some(e)) => e,
        Ok(None) => return, // not in this backup
        Err(e) => {
            report
                .warnings
                .push(format!("Native Calendar: Manifest read failed ({e})."));
            return;
        }
    };
    let out = work_dir.join(".Calendar.sqlitedb");
    if let Err(e) = index.extract_to(&entry, decryptor, &out) {
        let _ = std::fs::remove_file(&out);
        report.warnings.push(format!(
            "Native Calendar: couldn't read Calendar.sqlitedb ({e})."
        ));
        return;
    }
    if let Err(e) = crate::parsers::calendar::parse_calendar(&out, cache, report, false) {
        report
            .warnings
            .push(format!("Native Calendar: parse failed ({e})."));
    }
    let _ = std::fs::remove_file(&out);
}

/// Materialize Calls natively from `CallHistory.storedata`. Returns true when the
/// native path handled Calls (so the iLEAPP `callhistory` stage is skipped); false
/// on any miss/failure, leaving the iLEAPP path to run.
fn import_calls_native(
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
                .push(format!("Native Calls unavailable ({e}); using iLEAPP."));
            return false;
        }
    };
    let entry = match index.find("HomeDomain", "Library/CallHistoryDB/CallHistory.storedata") {
        Ok(Some(e)) => e,
        Ok(None) => return false, // not in this backup → iLEAPP path
        Err(e) => {
            report.warnings.push(format!(
                "Native Calls: Manifest read failed ({e}); using iLEAPP."
            ));
            return false;
        }
    };
    let out = work_dir.join(".CallHistory.storedata");
    if let Err(e) = index.extract_to(&entry, decryptor, &out) {
        let _ = std::fs::remove_file(&out);
        report.warnings.push(format!(
            "Native Calls: couldn't read CallHistory.storedata ({e}); using iLEAPP."
        ));
        return false;
    }
    let ok = match crate::parsers::calls::parse_calls(&out, cache, report, false) {
        Ok(()) => true,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Calls: parse failed ({e}); using iLEAPP."));
            false
        }
    };
    let _ = std::fs::remove_file(&out);
    ok
}

/// Materialize Safari history natively from `History.db`. Returns true when the
/// native path handled Safari (so the iLEAPP `safarihistory` stage is skipped).
fn import_safari_native(
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
                .push(format!("Native Safari unavailable ({e}); using iLEAPP."));
            return false;
        }
    };
    let entry = match index.find("HomeDomain", "Library/Safari/History.db") {
        Ok(Some(e)) => e,
        Ok(None) => return false, // not in this backup → iLEAPP path
        Err(e) => {
            report.warnings.push(format!(
                "Native Safari: Manifest read failed ({e}); using iLEAPP."
            ));
            return false;
        }
    };
    let out = work_dir.join(".History.db");
    // WAL-aware: History.db keeps most/all visits in its `-wal` sidecar.
    if let Err(e) = index.extract_db(&entry, decryptor, &out) {
        let _ = std::fs::remove_file(&out);
        report.warnings.push(format!(
            "Native Safari: couldn't read History.db ({e}); using iLEAPP."
        ));
        return false;
    }
    let ok = match crate::parsers::safari::parse_safari(&out, cache, report, false) {
        Ok(()) => true,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Safari: parse failed ({e}); using iLEAPP."));
            false
        }
    };
    let _ = std::fs::remove_file(&out);
    ok
}

/// Extract + parse Safari `Bookmarks.db` (bookmarks + reading list) and
/// `SafariTabs.db` (open tabs) into the cache. Native-only — iLEAPP doesn't feed
/// these — so it just does nothing (bar a warning) when a file is absent or
/// unreadable, rather than falling back.
fn import_safari_bookmarks_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
    replace: bool,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native Safari bookmarks unavailable ({e})."));
            return;
        }
    };
    // (relativePath, temp filename, which store) — parsed in turn.
    let stores = [
        ("Library/Safari/Bookmarks.db", ".Bookmarks.db", "bookmarks"),
        ("Library/Safari/SafariTabs.db", ".SafariTabs.db", "tabs"),
    ];
    for (rel, tmp, which) in stores {
        let entry = match index.find("HomeDomain", rel) {
            Ok(Some(e)) => e,
            Ok(None) => continue, // not in this backup
            Err(e) => {
                report.warnings.push(format!(
                    "Native Safari {which}: Manifest read failed ({e})."
                ));
                continue;
            }
        };
        let out = work_dir.join(tmp);
        if let Err(e) = index.extract_to(&entry, decryptor, &out) {
            let _ = std::fs::remove_file(&out);
            report
                .warnings
                .push(format!("Native Safari {which}: extract failed ({e})."));
            continue;
        }
        let res = if which == "bookmarks" {
            crate::parsers::safari_bookmarks::parse_safari_bookmarks(&out, cache, report, replace)
        } else {
            crate::parsers::safari_bookmarks::parse_safari_tabs(&out, cache, report, replace)
        };
        if let Err(e) = res {
            report
                .warnings
                .push(format!("Native Safari {which}: parse failed ({e})."));
        }
        let _ = std::fs::remove_file(&out);
    }
}

/// Extract `entry` to `out`, plus its `-wal`/`-shm` siblings (looked up in `all`
/// by relative path) so a read-only SQLite open replays an un-checkpointed WAL —
/// otherwise recently-written rows still sitting in the WAL are lost. Returns the
/// temp files created (the main DB first) for cleanup; empty if the main extract
/// failed. `all` must include the sidecar entries (query with a trailing `%`).
fn extract_with_wal(
    index: &crate::manifest::ManifestIndex,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    entry: &crate::manifest::FileEntry,
    all: &[crate::manifest::FileEntry],
    out: &Path,
) -> Vec<PathBuf> {
    if index.extract_to(entry, decryptor, out).is_err() {
        let _ = std::fs::remove_file(out);
        return Vec::new();
    }
    let mut temps = vec![out.to_path_buf()];
    for suf in ["-wal", "-shm"] {
        let sib_rel = format!("{}{suf}", entry.relative_path);
        if let Some(sib) = all.iter().find(|e| e.relative_path == sib_rel) {
            let sc = PathBuf::from(format!("{}{suf}", out.display()));
            if index.extract_to(sib, decryptor, &sc).is_ok() {
                temps.push(sc);
            } else {
                let _ = std::fs::remove_file(&sc);
            }
        }
    }
    temps
}

/// Locate + extract `AwemeIM.db` and parse TikTok contacts (social graph) into
/// the cache `contacts` table (source 'TikTok'). Native-only, best-effort.
fn import_tiktok_contacts_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(_) => return,
    };
    // TikTok keeps a per-account `AwemeIM-<accountid>.db`, so match every
    // `AwemeIM*.db` (trailing `%` also catches the `-wal`/`-shm` sidecars) and read
    // them all, bringing each DB's WAL so a recently-added contact isn't missed.
    let (mains, temps) = extract_aweme_dbs(&index, decryptor, work_dir);
    if mains.is_empty() {
        return; // no TikTok in this backup
    }
    if let Err(e) =
        crate::parsers::tiktok_contacts::parse_tiktok_contacts(&mains, cache, report, false)
    {
        report
            .warnings
            .push(format!("TikTok contacts: parse failed ({e})."));
    }
    for t in &temps {
        let _ = std::fs::remove_file(t);
    }
}

/// Extract every `AwemeIM*.db` (with its WAL sidecars) to temp files. Returns
/// `(main db paths, all temp paths incl. sidecars)`; caller parses the mains and
/// removes all temps. Shared by the TikTok contacts + messages importers.
fn extract_aweme_dbs(
    index: &crate::manifest::ManifestIndex,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    work_dir: &Path,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let hits = index
        .find_relative_like("%AwemeIM%.db%")
        .unwrap_or_default();
    let mut mains = Vec::new();
    let mut all_temps = Vec::new();
    let is_main = |e: &crate::manifest::FileEntry| {
        let base = e
            .relative_path
            .rsplit('/')
            .next()
            .unwrap_or(&e.relative_path);
        base.starts_with("AwemeIM") && base.ends_with(".db")
    };
    let main_entries: Vec<_> = hits.iter().filter(|e| is_main(e)).collect();
    for (i, entry) in main_entries.iter().enumerate() {
        let out = work_dir.join(format!(".tt-aweme-{i}.db"));
        let temps = extract_with_wal(index, decryptor, entry, &hits, &out);
        if let Some(main) = temps.first() {
            mains.push(main.clone());
        }
        all_temps.extend(temps);
    }
    (mains, all_temps)
}

/// Materialize TikTok chats natively. TikTok is special: messages live in
/// per-account `…/ChatFiles/<account>/db.sqlite` (`TIMMessageORM`), while sender
/// names live in the `AwemeContacts*` tables of `AwemeIM.db` — two DBs the generic
/// single-file app-module API can't join, so it gets a dedicated importer. Best-
/// effort: a backup without TikTok, or an unreadable DB, just yields nothing.
fn import_tiktok_messages_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(_) => return,
    };

    // 1. Build the sender-name map from every `AwemeIM*.db` (with WAL sidecars, so
    //    a recently-added contact resolves); collect, then drop the temps.
    let uid_map = {
        let (mains, temps) = extract_aweme_dbs(&index, decryptor, work_dir);
        let map = crate::parsers::tiktok_contacts::collect_uid_map(&mains);
        for t in &temps {
            let _ = std::fs::remove_file(t);
        }
        map
    };

    // 2. Locate the per-account chat databases (`ChatFiles/<account>/db.sqlite`).
    let all_hits = match index.find_relative_like("%ChatFiles%db.sqlite%") {
        Ok(h) => h,
        Err(_) => return,
    };
    let mains: Vec<_> = all_hits
        .iter()
        .filter(|e| {
            let base = e
                .relative_path
                .rsplit('/')
                .next()
                .unwrap_or(&e.relative_path);
            base == "db.sqlite" && e.relative_path.contains("ChatFiles")
        })
        .cloned()
        .collect();
    if mains.is_empty() {
        return; // no TikTok chats in this backup
    }

    // 3. Parse each chat DB and insert per-account (conversations don't span
    //    accounts, so per-file grouping is correct and bounds memory).
    for (i, entry) in mains.iter().enumerate() {
        // Bring the -wal/-shm so SQLite replays uncommitted messages.
        let out = work_dir.join(format!(".tt-chat-{i}.db"));
        let temps = extract_with_wal(&index, decryptor, entry, &all_hits, &out);
        let Some(base) = temps.first() else {
            report
                .warnings
                .push("TikTok messages: couldn't read a chat DB.".into());
            continue;
        };

        match crate::parsers::apps::tiktok::parse_tiktok_messages(
            base,
            &entry.relative_path,
            &uid_map,
        ) {
            Ok(msgs) if !msgs.is_empty() => {
                let resolve = app_media_resolver(&index, decryptor);
                if let Err(e) = crate::parsers::apps::insert_app_conversation_with_media(
                    cache, "TikTok", true, msgs, report, &resolve,
                ) {
                    report
                        .warnings
                        .push(format!("TikTok messages: insert failed ({e})."));
                }
            }
            Ok(_) => {}
            Err(e) => report
                .warnings
                .push(format!("TikTok messages: parse failed ({e}).")),
        }
        for t in &temps {
            let _ = std::fs::remove_file(t);
        }
    }
}

/// Extract Photos.sqlite and tag camera-roll `media_items` with the people
/// detected in each photo. Native-only, best-effort: a missing/unreadable
/// Photos.sqlite (or no named people) just leaves media untagged.
fn import_photos_metadata_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(_) => return,
    };
    let entry = match index.find("CameraRollDomain", "Media/PhotoData/Photos.sqlite") {
        Ok(Some(e)) => e,
        _ => return, // not in this backup
    };
    let out = work_dir.join(".Photos.sqlite");
    if let Err(e) = index.extract_to(&entry, decryptor, &out) {
        let _ = std::fs::remove_file(&out);
        report
            .warnings
            .push(format!("Photo people: couldn't read Photos.sqlite ({e})."));
        return;
    }
    if let Err(e) = crate::parsers::photos_meta::parse_photos_metadata(&out, cache) {
        report
            .warnings
            .push(format!("Photo people: parse failed ({e})."));
    }
    let _ = std::fs::remove_file(&out);
}

/// Self-extract + parse Contacts from `AddressBook.sqlitedb` (photos from the
/// sibling `AddressBookImages.sqlitedb`). Returns true when Contacts were handled
/// natively (so the iLEAPP contacts stage is skipped). The address-book insert
/// only touches device contacts, leaving any third-party rows untouched.
fn import_contacts_native(
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
                .push(format!("Native Contacts unavailable ({e}); using iLEAPP."));
            return false;
        }
    };
    let entry = match index.find("HomeDomain", "Library/AddressBook/AddressBook.sqlitedb") {
        Ok(Some(e)) => e,
        Ok(None) => return false, // not in this backup → iLEAPP path
        Err(e) => {
            report.warnings.push(format!(
                "Native Contacts: Manifest read failed ({e}); using iLEAPP."
            ));
            return false;
        }
    };
    let ab = work_dir.join(".AddressBook.sqlitedb");
    if let Err(e) = index.extract_to(&entry, decryptor, &ab) {
        let _ = std::fs::remove_file(&ab);
        report.warnings.push(format!(
            "Native Contacts: couldn't read AddressBook.sqlitedb ({e}); using iLEAPP."
        ));
        return false;
    }

    // Photos are optional — a missing/odd images DB just means no avatars.
    let images = match index.find(
        "HomeDomain",
        "Library/AddressBook/AddressBookImages.sqlitedb",
    ) {
        Ok(Some(ie)) => {
            let ip = work_dir.join(".AddressBookImages.sqlitedb");
            let m = if index.extract_to(&ie, decryptor, &ip).is_ok() {
                crate::parsers::address_book::parse_address_book_images(&ip).unwrap_or_default()
            } else {
                Default::default()
            };
            let _ = std::fs::remove_file(&ip);
            m
        }
        _ => Default::default(),
    };

    let contacts = match crate::parsers::address_book::parse_address_book(&ab) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&ab);
            report.warnings.push(format!(
                "Native Contacts: parse failed ({e}); using iLEAPP."
            ));
            return false;
        }
    };
    let _ = std::fs::remove_file(&ab);

    match crate::parsers::address_book::insert_contacts(cache, &contacts, &images, false) {
        Ok(n) => {
            report.contacts += n;
            true
        }
        Err(e) => {
            report.warnings.push(format!(
                "Native Contacts: insert failed ({e}); using iLEAPP."
            ));
            false
        }
    }
}

/// Materialize third-party chats natively by driving the app-module registry:
/// each module locates its own DB in the Manifest, we extract + parse it, and the
/// shared inserter writes the threads/messages. Returns the service labels handled
/// natively (so the equivalent iLEAPP stages are skipped). An app whose DB isn't in
/// the backup, or that fails to parse, is silently left to the iLEAPP path.
/// Build a resolver that maps an app attachment to its backup blob. App media
/// files have unique (usually UUID) names, so a Manifest `LIKE '%<basename>'`
/// lookup finds them regardless of the app-specific directory layout. Returns
/// `None` when the file isn't in the backup (evicted / not backed up).
fn app_media_resolver<'a>(
    index: &'a crate::manifest::ManifestIndex,
    decryptor: Option<&'a crate::crypto::BackupDecryptor>,
) -> impl Fn(&crate::parsers::apps::AppAttachment) -> Option<crate::parsers::apps::ResolvedMedia> + 'a
{
    move |att| {
        let basename = att
            .path
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())?;
        let entry = index
            .find_relative_like(&format!("%{basename}"))
            .ok()?
            .into_iter()
            .next()?;
        let path = index
            .blob_path(&entry.file_id)
            .to_string_lossy()
            .into_owned();
        let (key, size) = match decryptor {
            Some(_) => crate::crypto::file_key_field(&entry.file_blob)
                .map(|(k, s)| (Some(k), s))
                .unwrap_or((None, None)),
            None => (None, None),
        };
        Some((path, key, size))
    }
}

fn import_app_chats_native(
    backup_dir: &Path,
    decryptor: Option<&crate::crypto::BackupDecryptor>,
    cache: &CacheDb,
    work_dir: &Path,
    report: &mut ImportReport,
) -> Vec<&'static str> {
    let index = match crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir) {
        Ok(i) => i,
        Err(e) => {
            report
                .warnings
                .push(format!("Native app chats unavailable ({e}); using iLEAPP."));
            return Vec::new();
        }
    };
    let mut handled = Vec::new();
    for m in crate::parsers::apps::APP_CHAT_MODULES {
        let entries = match (m.locate)(&index) {
            Ok(e) => e,
            Err(e) => {
                report
                    .warnings
                    .push(format!("Native {}: Manifest read failed ({e}).", m.service));
                continue;
            }
        };
        if entries.is_empty() {
            continue; // app not in this backup → iLEAPP (or nothing)
        }
        // Some apps (Messenger) have several candidate DBs; parse each and combine.
        let mut msgs = Vec::new();
        let mut parsed_any = false;
        for (i, entry) in entries.iter().enumerate() {
            let out = work_dir.join(format!(".app-{}-{i}.sqlite", m.id));
            if let Err(e) = index.extract_to(entry, decryptor, &out) {
                let _ = std::fs::remove_file(&out);
                report
                    .warnings
                    .push(format!("Native {}: couldn't read a DB ({e}).", m.service));
                continue;
            }
            match (m.parse)(&out, &entry.relative_path) {
                Ok(mut parsed) => {
                    msgs.append(&mut parsed);
                    parsed_any = true;
                }
                Err(e) => report
                    .warnings
                    .push(format!("Native {}: parse failed ({e}).", m.service)),
            }
            let _ = std::fs::remove_file(&out);
        }
        // Only claim the app (and skip iLEAPP for it) when the native path actually
        // produced messages. A located-but-unrecognized DB (schema drift) parses to
        // an empty stream — indistinguishable from a genuinely empty store — so we
        // let iLEAPP run rather than risk silently dropping messages it could parse.
        // (Worst case here is a redundant, empty iLEAPP pass for a truly-empty app.)
        if !parsed_any {
            report.warnings.push(format!(
                "Native {}: no DB could be read; using iLEAPP.",
                m.service
            ));
            continue;
        }
        if msgs.is_empty() {
            continue; // recognized nothing / empty — leave the service to iLEAPP
        }
        let resolve = app_media_resolver(&index, decryptor);
        match crate::parsers::apps::insert_app_conversation_with_media(
            cache,
            m.service,
            m.numeric_id_groups,
            msgs,
            report,
            &resolve,
        ) {
            Ok(()) => handled.push(m.service),
            Err(e) => report.warnings.push(format!(
                "Native {}: insert failed ({e}); using iLEAPP.",
                m.service
            )),
        }
    }
    handled
}

/// The natively-parsed data types that can be re-imported on their own — no
/// iLEAPP, so it's fast. The UI offers a "re-import" action only for these.
pub const REIMPORTABLE_NATIVE: &[&str] = &[
    "recordings",
    "camera_roll",
    "messages",
    "notes",
    "calls",
    "safari",
];

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
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        rec.title,
                        rec.folder,
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
            // Re-tag the fresh rows with photo people (the re-insert above cleared
            // the persons column).
            import_photos_metadata_native(backup_dir, decryptor, &cache, work_dir, &mut report);
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
            if let Err(e) = index.extract_db(&entry, decryptor, &sms_db) {
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
            if let Err(e) = index.extract_db(&entry, decryptor, &note_db) {
                let _ = std::fs::remove_file(&note_db);
                return Err(e);
            }
            // replace=true clears + re-inserts atomically (see parse_notes).
            let img_src = crate::parsers::notes::NoteImageSource {
                index: &index,
                decryptor,
            };
            let r = crate::parsers::notes::parse_notes(
                &note_db,
                &cache,
                &mut report,
                true,
                Some(&img_src),
            );
            let _ = std::fs::remove_file(&note_db);
            r?;
        }
        "calls" => {
            let index = crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir)?;
            let entry = index
                .find("HomeDomain", "Library/CallHistoryDB/CallHistory.storedata")?
                .ok_or_else(|| {
                    crate::Error::Parse("CallHistory.storedata is not in this backup".into())
                })?;
            let out = work_dir.join(".reimport-CallHistory.storedata");
            if let Err(e) = index.extract_to(&entry, decryptor, &out) {
                let _ = std::fs::remove_file(&out);
                return Err(e);
            }
            let r = crate::parsers::calls::parse_calls(&out, &cache, &mut report, true);
            let _ = std::fs::remove_file(&out);
            r?;
        }
        "safari" => {
            let index = crate::manifest::ManifestIndex::open(backup_dir, decryptor, work_dir)?;
            let entry = index
                .find("HomeDomain", "Library/Safari/History.db")?
                .ok_or_else(|| crate::Error::Parse("History.db is not in this backup".into()))?;
            let out = work_dir.join(".reimport-History.db");
            if let Err(e) = index.extract_db(&entry, decryptor, &out) {
                let _ = std::fs::remove_file(&out);
                return Err(e);
            }
            let r = crate::parsers::safari::parse_safari(&out, &cache, &mut report, true);
            let _ = std::fs::remove_file(&out);
            r?;
            // Refresh bookmarks / reading list / tabs alongside history.
            import_safari_bookmarks_native(
                backup_dir,
                decryptor,
                &cache,
                work_dir,
                &mut report,
                true,
            );
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
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER, date_read INTEGER, date_delivered INTEGER, guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER, associated_message_emoji TEXT, thread_originator_guid TEXT, attributedBody BLOB, date_edited INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111');
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat_handle_join VALUES (10,1);
             INSERT INTO message VALUES (100,'hi',0,721692800000000000,1,0,0,0,'G100',NULL,0,NULL,NULL,NULL,0);
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
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, is_from_me INTEGER, date INTEGER, handle_id INTEGER, cache_has_attachments INTEGER, date_read INTEGER, date_delivered INTEGER, guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER, associated_message_emoji TEXT, thread_originator_guid TEXT, attributedBody BLOB, date_edited INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             CREATE TABLE attachment (ROWID INTEGER PRIMARY KEY, filename TEXT, transfer_name TEXT, mime_type TEXT);
             CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
             INSERT INTO handle VALUES (1,'+15550001111');
             INSERT INTO chat VALUES (10,'+15550001111',NULL,'iMessage');
             INSERT INTO chat_handle_join VALUES (10,1);
             -- an attachment-only message (NULL text, has_attachments=1).
             INSERT INTO message VALUES (100,NULL,0,721692800000000000,1,1,0,0,'G100',NULL,0,NULL,NULL,NULL,0);
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
}
