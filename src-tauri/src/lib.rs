//! Thin Tauri command layer over traceloupe-core (architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

mod logging;
mod media;
mod secret;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Monotonic counter for unique on-demand decrypt temp-file names.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Deletes its path on drop, so a decrypted-plaintext temp file never outlives
/// the request that produced it — even on an early return or a panic mid-render.
struct TempPath(PathBuf);
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Write bytes to a fresh file with owner-only (0600) permissions on Unix, so a
/// decrypted plaintext isn't briefly world-readable at rest.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

use tauri::{AppHandle, Emitter, Manager, State};
use traceloupe_core::cache::CacheDb;
use traceloupe_core::crypto::BackupDecryptor;
use traceloupe_core::discovery::{self, BackupInfo};
use traceloupe_core::engine::{self};
use traceloupe_core::import::{self, ImportPhase};
use traceloupe_core::install;
use traceloupe_core::query::{
    self, Call, Contact, HistoryVisit, MediaItem, Message, Note, Recording, ThreadSummary,
    TimelineMessage,
};
use traceloupe_core::sidecar::CancelToken;

/// The cache DB currently being browsed. Set when an import finishes or a
/// previously-imported backup is opened; read by every artifact query.
#[derive(Default)]
struct ActiveBackup(Mutex<Option<PathBuf>>);

impl ActiveBackup {
    fn set(&self, path: PathBuf) {
        *self.0.lock().unwrap() = Some(path);
    }
    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
    fn path(&self) -> Result<PathBuf, String> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "no backup is open".to_string())
    }
}

/// The active backup's decryptor, for encrypted backups. Holds the unwrapped
/// keys (derived once from the Keychain-stored password) so full-resolution
/// photos can be decrypted on demand by the media protocol. `None` for
/// unencrypted backups. Keys live only in memory for the session.
#[derive(Default)]
struct SessionKeys(Mutex<Option<Arc<BackupDecryptor>>>);

impl SessionKeys {
    fn set(&self, decryptor: Option<Arc<BackupDecryptor>>) {
        *self.0.lock().unwrap() = decryptor;
    }
    fn get(&self) -> Option<Arc<BackupDecryptor>> {
        self.0.lock().unwrap().clone()
    }
}

/// The cancel token of the import currently in flight, so a `cancel_import`
/// command can stop it (killing the iLEAPP subprocess). `None` when idle.
#[derive(Default)]
struct ImportCancel(Mutex<Option<CancelToken>>);

/// Serializes partial re-imports: only one may touch the cache at a time. Two
/// concurrent re-imports would otherwise contend on the single SQLite writer and
/// collide on the shared manifest temp file. A second re-import waits here for the
/// first to finish rather than failing.
#[derive(Default)]
struct ReimportGate(tauri::async_runtime::Mutex<()>);

/// Reconstruct the decryptor for an encrypted backup from its Keychain password
/// and the source dir recorded in its cache. `None` if not encrypted / no key.
fn reopen_decryptor(cache_path: &Path, backup_id: &str) -> Option<Arc<BackupDecryptor>> {
    let password = secret::get(backup_id)?;
    let cache = CacheDb::open(cache_path).ok()?;
    let source_dir = cache.get_meta("source_dir").ok().flatten()?;
    BackupDecryptor::open(Path::new(&source_dir), &password)
        .ok()
        .map(Arc::new)
}

/// Open the active cache DB for a read query.
fn open_active_cache(active: &ActiveBackup) -> Result<CacheDb, String> {
    CacheDb::open(&active.path()?).map_err(|e| e.to_string())
}

/// A backup id is joined into cache/work paths and used as a Keychain account,
/// so it must be a plain identifier — this rejects path separators, `..`, and
/// other tampering. Discovery only ever yields device UDIDs / UUIDs.
fn valid_backup_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Discovery outcome shaped for the UI: distinguishes "no backups" from
/// "macOS denied access" so the frontend can show Full Disk Access guidance.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum DiscoveryResult {
    Ok { backups: Vec<BackupInfo> },
    PermissionDenied { path: String },
    NotFound { path: String },
}

#[tauri::command]
fn list_backups(root: Option<String>) -> Result<DiscoveryResult, String> {
    // No root → scan the default MobileSync location (needs FDA). A root from
    // the folder picker → discover_at, which also accepts a single backup dir.
    let result = match root {
        Some(r) => discovery::discover_at(&PathBuf::from(r)),
        None => {
            let root = discovery::default_backup_root()
                .ok_or_else(|| "cannot resolve home directory".to_string())?;
            discovery::discover_backups(&root)
        }
    };
    match result {
        Ok(backups) => Ok(DiscoveryResult::Ok { backups }),
        Err(traceloupe_core::Error::PermissionDenied { path }) => {
            Ok(DiscoveryResult::PermissionDenied {
                path: path.display().to_string(),
            })
        }
        Err(traceloupe_core::Error::BackupDirNotFound { path }) => Ok(DiscoveryResult::NotFound {
            path: path.display().to_string(),
        }),
        Err(e) => Err(e.to_string()),
    }
}

/// The default Finder/MobileSync backup location, for seeding the folder
/// picker's starting directory. `None` if the home dir can't be resolved.
#[tauri::command]
fn default_backup_root() -> Option<String> {
    discovery::default_backup_root().map(|p| p.display().to_string())
}

/// Open System Settings straight to the Full Disk Access pane. A fixed URL,
/// not one from the frontend, so this can't be used to open arbitrary targets.
/// Uses the absolute path to `open` because a bundle launched from Finder has
/// a minimal PATH that may not include `/usr/bin`.
#[tauri::command]
fn open_full_disk_access_settings() -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Whether an iLEAPP engine is resolvable right now. The UI uses this to decide
/// between offering "import" and "engine not installed" guidance.
#[tauri::command]
fn engine_status(app: AppHandle) -> bool {
    resolve_engine(&app).is_some()
}

/// Engine setup state for the UI: whether one is resolvable now, its pinned
/// version, and whether a downloadable build has been published yet.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineInfo {
    installed: bool,
    version: String,
    can_download: bool,
}

#[tauri::command]
fn engine_info(app: AppHandle) -> EngineInfo {
    let manifest = install::pinned_engine();
    EngineInfo {
        installed: resolve_engine(&app).is_some(),
        version: manifest.version.clone(),
        can_download: manifest.is_published(),
    }
}

/// Download and install the pinned engine into `<app_data>/engine`, streaming
/// progress on `engine://progress`. After it succeeds, `resolve_engine` finds
/// the installed binary and imports work.
#[tauri::command]
async fn install_engine(app: AppHandle) -> Result<(), String> {
    let manifest = install::pinned_engine();
    let install_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("engine");

    tauri::async_runtime::spawn_blocking(move || {
        install::install_engine(&manifest, &install_dir, |p| {
            let ev = match p {
                install::InstallProgress::Downloading { received, total } => {
                    EngineEvent::Downloading {
                        received,
                        total,
                        fraction: if total > 0 {
                            received as f32 / total as f32
                        } else {
                            0.0
                        },
                    }
                }
                install::InstallProgress::Verifying => EngineEvent::Verifying,
                install::InstallProgress::Done => EngineEvent::Done,
            };
            let _ = app.emit("engine://progress", ev);
        })
        .map(|_| ())
    })
    .await
    .map_err(|e| format!("install task panicked: {e}"))?
    .map_err(|e| e.to_string())
}

/// Progress event for engine install, on the `engine://progress` channel.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
enum EngineEvent {
    Downloading {
        received: u64,
        total: u64,
        fraction: f32,
    },
    Verifying,
    Done,
}

/// Resolve the iLEAPP engine from env overrides and the app data dir.
/// - `TRACELOUPE_PYTHON` + `TRACELOUPE_ILEAPP_SOURCE` → run from a source checkout.
/// - `TRACELOUPE_ILEAPP` → an explicit frozen binary.
/// - else `<app_data>/engine/ileapp` (downloaded on first use).
fn resolve_engine(app: &AppHandle) -> Option<traceloupe_core::sidecar::EngineConfig> {
    let source_override = match (
        std::env::var_os("TRACELOUPE_PYTHON"),
        std::env::var_os("TRACELOUPE_ILEAPP_SOURCE"),
    ) {
        (Some(py), Some(src)) => Some((PathBuf::from(py), PathBuf::from(src))),
        _ => None,
    };
    let binary_override = std::env::var_os("TRACELOUPE_ILEAPP").map(PathBuf::from);
    let installed = app
        .path()
        .app_data_dir()
        .map(|d| d.join("engine").join("ileapp"))
        .unwrap_or_else(|_| PathBuf::from("ileapp"));
    engine::resolve_engine(source_override, binary_override, &installed)
}

/// Progress event payload emitted on the `import://progress` channel.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
enum ImportEvent {
    Parsing {
        current: u32,
        total: u32,
        fraction: f32,
        artifact: String,
    },
    Normalizing {
        /// The sub-stage being organized (e.g. "Messages", "Camera roll").
        step: String,
    },
}

/// Outcome returned to the awaiting frontend.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResult {
    cache_path: String,
    threads: usize,
    messages: usize,
    media_items: usize,
    calls: usize,
    safari_visits: usize,
    contacts: usize,
    warnings: Vec<String>,
}

/// Import a backup: run iLEAPP, normalize into a per-backup cache DB, streaming
/// progress on `import://progress`. The password stays in memory only.
///
/// Runs the blocking import on a worker thread so the async runtime is free to
/// deliver the emitted events while it runs.
/// The catalog of importable data types, for the import-selection settings.
#[tauri::command]
fn list_import_modules() -> Vec<traceloupe_core::sidecar::ImportModule> {
    traceloupe_core::sidecar::IMPORT_CATALOG.to_vec()
}

/// Set the dev-console log verbosity at runtime (from Settings).
/// `level` is "off" | "error" | "warn" | "info" | "debug" | "trace".
#[tauri::command]
fn set_log_level(level: String) {
    logging::set_level(&level);
}

/// Stop the in-flight import (kills the iLEAPP subprocess). No-op when idle.
#[tauri::command]
fn cancel_import(import_cancel: State<'_, ImportCancel>) {
    if let Some(token) = import_cancel.0.lock().unwrap().as_ref() {
        token.cancel();
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects the State params; not a real API.
async fn import_backup(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    import_cancel: State<'_, ImportCancel>,
    backup_path: String,
    backup_id: String,
    password: String,
    modules: Vec<String>,
) -> Result<ImportResult, String> {
    if !valid_backup_id(&backup_id) {
        return Err("invalid backup id".to_string());
    }
    let cfg = resolve_engine(&app).ok_or_else(|| {
        "iLEAPP engine is not installed. Set TRACELOUPE_ILEAPP or install the engine.".to_string()
    })?;

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?;
    let cache_path = data_dir.join("caches").join(&backup_id).join("cache.db");
    let work_dir = data_dir.join("work").join(&backup_id);
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let cancel = CancelToken::new();
    // Expose the token so `cancel_import` can stop this run (kills iLEAPP).
    *import_cancel.0.lock().unwrap() = Some(cancel.clone());
    let backup_path = PathBuf::from(backup_path);
    // Kept for post-import key setup (the originals are moved into the worker).
    let source_dir = backup_path.clone();
    let key_password = password.clone();

    // Blocking pipeline on a worker thread; progress is emitted as it runs.
    let result = tauri::async_runtime::spawn_blocking(move || {
        logging::info(&app, "Import started");
        // Time each phase/step for the dev console (start on entry, elapsed on
        // the next step boundary / completion).
        let import_start = Instant::now();
        let mut step_start = import_start;
        let mut current_step: Option<String> = None;
        import::import_backup(
            &cfg,
            &backup_path,
            &password,
            &cache_path,
            &work_dir,
            &modules,
            &cancel,
            |phase| {
                let event = match &phase {
                    ImportPhase::Parsing(p) => {
                        if current_step.is_none() {
                            logging::info(&app, "\u{25b6} Parsing backup with iLEAPP\u{2026}");
                            current_step = Some("Parsing".into());
                            step_start = Instant::now();
                        }
                        logging::debug(
                            &app,
                            format!("parsing {} ({}/{})", p.artifact, p.current, p.total),
                        );
                        Some(ImportEvent::Parsing {
                            current: p.current,
                            total: p.total,
                            fraction: p.fraction(),
                            artifact: p.artifact.clone(),
                        })
                    }
                    ImportPhase::Normalizing { step } => {
                        if let Some(prev) = current_step.take() {
                            logging::info(
                                &app,
                                format!(
                                    "\u{2713} {prev} ({} ms)",
                                    step_start.elapsed().as_millis()
                                ),
                            );
                        }
                        logging::info(&app, format!("\u{25b6} Organizing {step}"));
                        current_step = Some(step.clone());
                        step_start = Instant::now();
                        Some(ImportEvent::Normalizing { step: step.clone() })
                    }
                    ImportPhase::Done(report) => {
                        if let Some(prev) = current_step.take() {
                            logging::info(
                                &app,
                                format!(
                                    "\u{2713} {prev} ({} ms)",
                                    step_start.elapsed().as_millis()
                                ),
                            );
                        }
                        for w in &report.warnings {
                            logging::warn(&app, w.clone());
                        }
                        logging::info(
                            &app,
                            format!(
                                "Import complete in {} ms ({} messages, {} media, {} contacts)",
                                import_start.elapsed().as_millis(),
                                report.messages,
                                report.media_items,
                                report.contacts
                            ),
                        );
                        None
                    }
                };
                if let Some(event) = event {
                    let _ = app.emit("import://progress", event);
                }
            },
        )
    })
    .await;

    // The run is over (done, error, or cancelled) — clear the shared token so a
    // later cancel_import can't stop a future import, and free it.
    *import_cancel.0.lock().unwrap() = None;

    let outcome = result
        .map_err(|e| format!("import task panicked: {e}"))?
        .map_err(|e| e.to_string())?;

    // Newly imported backup becomes the active one for browsing.
    active.set(outcome.cache_path.clone());

    // Encrypted backup: remember its source dir, stash the password in the
    // Keychain, and hold the decryptor for on-demand media decryption. For an
    // unencrypted backup, clear any stale secret/keys.
    if key_password.is_empty() {
        session.set(None);
        secret::delete(&backup_id);
    } else {
        if let Ok(cache) = CacheDb::open(&outcome.cache_path) {
            let _ = cache.set_meta("source_dir", &source_dir.display().to_string());
        }
        if let Err(e) = secret::store(&backup_id, &key_password) {
            eprintln!("could not store backup password in Keychain: {e}");
        }
        let decryptor = BackupDecryptor::open(&source_dir, &key_password)
            .ok()
            .map(Arc::new);
        session.set(decryptor);
    }

    Ok(ImportResult {
        cache_path: outcome.cache_path.display().to_string(),
        threads: outcome.report.threads,
        messages: outcome.report.messages,
        media_items: outcome.report.media_items,
        calls: outcome.report.calls,
        safari_visits: outcome.report.safari_visits,
        contacts: outcome.report.contacts,
        warnings: outcome.report.warnings,
    })
}

/// Open a previously-imported backup's cache (by id) for browsing, without
/// re-running the engine. Returns false if no cache exists for that id yet.
#[tauri::command]
async fn open_backup(app: AppHandle, backup_id: String) -> bool {
    if !valid_backup_id(&backup_id) {
        return false;
    }
    let Ok(data_dir) = app.path().app_data_dir() else {
        return false;
    };
    let cache_path = data_dir.join("caches").join(&backup_id).join("cache.db");
    if !cache_path.exists() {
        return false;
    }
    // Rebuilding the decryptor reads the Keychain, opens the cache and runs
    // PBKDF2 — all blocking. Keep it off the main thread so selecting a backup
    // never freezes the UI (no-op decryptor for plaintext backups).
    let cp = cache_path.clone();
    let decryptor = tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&cp, &backup_id))
        .await
        .ok()
        .flatten();
    app.state::<SessionKeys>().set(decryptor);
    app.state::<ActiveBackup>().set(cache_path);
    true
}

/// Whether a backup is currently open for browsing.
#[tauri::command]
fn has_active_backup(active: State<'_, ActiveBackup>) -> bool {
    active.path().is_ok()
}

/// Counts refreshed by a partial re-import (only the relevant one is non-zero).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReimportResult {
    module: String,
    recordings: usize,
    media_items: usize,
    messages: usize,
    threads: usize,
    notes: usize,
    warnings: Vec<String>,
}

/// Re-import a single natively-parsed data type into the open backup's cache,
/// replacing just that type's rows — no iLEAPP, so it's fast. Paths are derived
/// from the active cache (`…/caches/<id>/cache.db`) and the original backup dir
/// recorded in its `source_dir` meta; the decrypt keys come from the session.
#[tauri::command]
async fn reimport_module(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    gate: State<'_, ReimportGate>,
    module_id: String,
) -> Result<ReimportResult, String> {
    if !import::REIMPORTABLE_NATIVE.contains(&module_id.as_str()) {
        return Err(format!("'{module_id}' can't be re-imported on its own"));
    }
    let label = reimport_label(&module_id);
    // Serialize re-imports: a second one waits here until the first finishes, so
    // they never contend on the cache writer or the shared manifest temp file.
    logging::info(&app, format!("\u{25b6} Re-importing {label}\u{2026}"));
    let started = Instant::now();
    let _gate = gate.0.lock().await;
    let cache_path = active.path()?;
    // …/caches/<id>/cache.db → id dir → caches dir → data dir → …/work/<id>
    let id_dir = cache_path
        .parent()
        .ok_or_else(|| "unexpected cache layout".to_string())?;
    let backup_id = id_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "unexpected cache layout".to_string())?;
    let data_dir = id_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "unexpected cache layout".to_string())?;
    let work_dir = data_dir.join("work").join(backup_id);

    // The original backup dir (may be offline now) is recorded in the cache.
    let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
    let source_dir = cache
        .get_meta("source_dir")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "this backup's source path isn't recorded; re-import fully once".to_string()
        })?;
    drop(cache);

    let decryptor = session.get();
    let module = module_id.clone();
    let cp = cache_path.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        import::reimport_module(
            &module,
            Path::new(&source_dir),
            decryptor.as_deref(),
            &cp,
            &work_dir,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        let e = e.to_string();
        logging::error(&app, format!("\u{2717} Re-import {label} failed: {e}"));
        e
    })?;

    let count = reimport_count(&module_id, &report);
    logging::info(
        &app,
        format!(
            "\u{2713} Re-imported {label}: {count} in {} ms",
            started.elapsed().as_millis()
        ),
    );
    for w in &report.warnings {
        logging::warn(&app, w.clone());
    }

    Ok(ReimportResult {
        module: module_id,
        recordings: report.recordings,
        media_items: report.media_items,
        messages: report.messages,
        threads: report.threads,
        notes: report.notes,
        warnings: report.warnings,
    })
}

/// Human label for a re-importable module id (for logs).
fn reimport_label(module_id: &str) -> &'static str {
    match module_id {
        "recordings" => "voice recordings",
        "camera_roll" => "camera roll",
        "messages" => "messages",
        "notes" => "notes",
        _ => "data",
    }
}

/// A human count line for a completed re-import (only the relevant field is set).
fn reimport_count(module_id: &str, r: &traceloupe_core::normalize::ImportReport) -> String {
    match module_id {
        "recordings" => format!("{} recordings", r.recordings),
        "camera_roll" => format!("{} photos & videos", r.media_items),
        "messages" => format!("{} messages in {} threads", r.messages, r.threads),
        "notes" => format!("{} notes", r.notes),
        _ => String::new(),
    }
}

/// Forget an imported backup: delete its cache DB and all derived caches
/// (media/thumbs), its work dir, and its stored password. Does not touch the
/// original backup on disk. Re-importing recreates everything.
#[tauri::command]
async fn forget_backup(app: AppHandle, backup_id: String) -> Result<(), String> {
    if !valid_backup_id(&backup_id) {
        return Err("invalid backup id".to_string());
    }
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let cache_dir = data_dir.join("caches").join(&backup_id);
    // If this backup is currently open, close it first so we don't delete under a
    // live handle and its keys don't linger in the session.
    let active = app.state::<ActiveBackup>();
    if active.path().is_ok_and(|p| p.starts_with(&cache_dir)) {
        active.clear();
        app.state::<SessionKeys>().set(None);
    }
    let work_dir = data_dir.join("work").join(&backup_id);
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(&cache_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
        secret::delete(&backup_id);
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Backup ids that have already been parsed (a cache exists) — the UI shows
/// these as "open instantly" rather than needing a first-time read.
#[tauri::command]
fn imported_backup_ids(app: AppHandle) -> Vec<String> {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(data_dir.join("caches")) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().join("cache.db").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

#[tauri::command]
async fn list_threads(active: State<'_, ActiveBackup>) -> Result<Vec<ThreadSummary>, String> {
    // Async + spawn_blocking: this scans every thread (with a per-thread snippet
    // subquery) and must not run on the main thread, or opening a backup with
    // thousands of conversations freezes the whole UI.
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_threads(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_thread_messages(
    active: State<'_, ActiveBackup>,
    thread_id: i64,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_messages(&cache, thread_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_thread_message_window(
    active: State<'_, ActiveBackup>,
    thread_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<Message>, String> {
    // Async + spawn_blocking: a synchronous command runs on the main thread and
    // would freeze the whole native UI. Only the requested window is read, so
    // the frontend can lazily load a thread as it scrolls.
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_message_window(&cache, thread_id, offset, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_timeline_messages(
    active: State<'_, ActiveBackup>,
    service: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_all_messages(&cache, service.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_timeline_window(
    active: State<'_, ActiveBackup>,
    offset: i64,
    limit: i64,
    service: Option<String>,
) -> Result<Vec<TimelineMessage>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_timeline_window(&cache, offset, limit, service.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_message_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
    service: Option<String>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_message_ranges(&cache, &ranges, service.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_range_window(
    active: State<'_, ActiveBackup>,
    lo: Option<i64>,
    hi: Option<i64>,
    offset: i64,
    limit: i64,
    service: Option<String>,
) -> Result<Vec<TimelineMessage>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_range_window(
            &cache,
            query::TimeRange { lo, hi },
            offset,
            limit,
            service.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a message attachment's file with the OS default app (for documents and
/// anything not rendered inline).
#[tauri::command]
fn open_attachment(
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    attachment_id: i64,
) -> Result<(), String> {
    let cache = open_active_cache(&active)?;
    let (local_path, _mime, decrypt_key, plain_size) =
        query::attachment_blob(&cache, attachment_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "attachment file is not available".to_string())?;

    // Encrypted backup: decrypt to a persistent temp beside the cache and open
    // that (the external app needs to read it after this returns, so it isn't
    // auto-deleted — a re-import/forget clears the dir). Plaintext: open directly.
    let to_open = if let Some(key) = decrypt_key {
        let dec = session
            .get()
            .ok_or_else(|| "backup keys are not loaded".to_string())?;
        let ciphertext = std::fs::read(&local_path).map_err(|e| e.to_string())?;
        let size = plain_size.and_then(|s| usize::try_from(s).ok());
        let plain = dec
            .decrypt_bytes(&key, &ciphertext, size)
            .map_err(|e| e.to_string())?;
        let dir = active
            .path()?
            .parent()
            .map(|p| p.join("att-open"))
            .ok_or_else(|| "unexpected cache layout".to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name = std::path::Path::new(&local_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("attachment-{attachment_id}"));
        let dest = dir.join(format!("{attachment_id}-{name}"));
        write_private(&dest, &plain).map_err(|e| e.to_string())?;
        dest
    } else {
        PathBuf::from(&local_path)
    };

    std::process::Command::new("/usr/bin/open")
        .arg(&to_open)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn list_calls(active: State<'_, ActiveBackup>) -> Result<Vec<Call>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_calls(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_notes(active: State<'_, ActiveBackup>) -> Result<Vec<Note>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_notes(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_recordings(active: State<'_, ActiveBackup>) -> Result<Vec<Recording>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_recordings(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_safari_history(active: State<'_, ActiveBackup>) -> Result<Vec<HistoryVisit>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_safari_history(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_contacts(active: State<'_, ActiveBackup>) -> Result<Vec<Contact>, String> {
    // Async + spawn_blocking: the address book can hold tens of thousands of
    // contacts (e.g. TikTok), so this must stay off the main thread.
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_contacts(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_installed_apps(active: State<'_, ActiveBackup>) -> Result<Vec<String>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_installed_apps(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_media(active: State<'_, ActiveBackup>) -> Result<Vec<MediaItem>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_media(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// (source label, count) pairs for the gallery's source filter.
#[tauri::command]
async fn media_sources(active: State<'_, ActiveBackup>) -> Result<Vec<(String, i64)>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::media_sources(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// Windowed, filterable list commands (async + spawn_blocking) so the UI can
// lazily load huge lists a slice at a time — the same pattern as messages.

// These map a client-supplied sort *field name* to an allowlisted SQL column so
// nothing untrusted is ever interpolated into a query. Unknown fields fall back
// to each list's default (date/most-recent).
fn calls_sort(field: &str, desc: bool) -> query::Sort {
    let col = match field {
        "name" => "address COLLATE NOCASE",
        "duration" => "duration_s",
        _ => "occurred_at",
    };
    query::Sort::new(col, desc)
}
fn safari_sort(field: &str, desc: bool) -> query::Sort {
    let col = match field {
        "title" => "title COLLATE NOCASE",
        "visits" => "visit_count",
        _ => "visited_at",
    };
    query::Sort::new(col, desc)
}
fn media_sort(field: &str, desc: bool) -> query::Sort {
    let col = match field {
        "source" => "source COLLATE NOCASE",
        _ => "taken_at",
    };
    query::Sort::new(col, desc)
}

#[tauri::command]
async fn count_media(
    active: State<'_, ActiveBackup>,
    source: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_media(&cache, source.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_media_window(
    active: State<'_, ActiveBackup>,
    source: Option<String>,
    offset: i64,
    limit: i64,
    sort_by: String,
    desc: bool,
) -> Result<Vec<MediaItem>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_media_window(
            &cache,
            source.as_deref(),
            offset,
            limit,
            media_sort(&sort_by, desc),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_calls(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_calls(&cache, search.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_calls_window(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    offset: i64,
    limit: i64,
    sort_by: String,
    desc: bool,
) -> Result<Vec<Call>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_calls_window(
            &cache,
            search.as_deref(),
            offset,
            limit,
            calls_sort(&sort_by, desc),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_safari(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari(&cache, search.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_safari_window(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    offset: i64,
    limit: i64,
    sort_by: String,
    desc: bool,
) -> Result<Vec<HistoryVisit>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_safari_window(
            &cache,
            search.as_deref(),
            offset,
            limit,
            safari_sort(&sort_by, desc),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Serve a media item over the `traceloupe-media://localhost/<id>` scheme
/// (append `?thumb=1` for a downscaled thumbnail).
///
/// Security: the handler takes only a numeric id, looks up the file path
/// recorded for it in the active cache, and serves that. It never accepts a
/// path from the request, so it can't be coerced into reading arbitrary files.
///
/// HEIC (the format most iOS photos use) is transcoded to JPEG so the webview
/// can render it; thumbnails are downscaled JPEGs. Both are cached (see media).
fn media_protocol_response(
    app: &AppHandle,
    path: &str,
    query_str: Option<&str>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    // Path is "/<id>"; the query may carry "thumb=1".
    let Some(id) = path.trim_start_matches('/').parse::<i64>().ok() else {
        return not_found();
    };
    let want_thumb = query_str.is_some_and(|q| q.contains("thumb"));

    let active = app.state::<ActiveBackup>();
    let Ok(cache_path) = active.path() else {
        return not_found();
    };
    let Ok(cache) = CacheDb::open(&cache_path) else {
        return not_found();
    };
    let Ok(Some((local_path, mime, thumb_path, decrypt_key, plain_size))) =
        query::media_blob(&cache, id)
    else {
        return not_found();
    };

    // Camera-roll items carry iOS's pre-rendered JPEG thumbnail — serve it
    // directly for grid requests (no HEIC decode at all). On encrypted backups
    // this thumbnail was decrypted into the cache at import, so the grid works
    // even without the keys.
    if want_thumb {
        if let Some(tp) = thumb_path {
            if let Ok(bytes) = std::fs::read(&tp) {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "image/jpeg")
                    .header("Cache-Control", "no-cache")
                    .body(bytes)
                    .unwrap();
            }
        }
    }

    // Converted thumbnails/full-JPEGs are cached alongside the backup's cache DB.
    let thumbs_dir = cache_path
        .parent()
        .map(|p| p.join("thumbs"))
        .unwrap_or_else(|| PathBuf::from("thumbs"));

    // Encrypted original: decrypt it (using the session keys) to a temp file that
    // `media::render` / sips can read, then discard the plaintext. The rendered
    // JPEG is still cached by id, so repeat views don't re-decrypt via sips.
    let rendered = if let Some(key) = decrypt_key {
        let Some(dec) = app.state::<SessionKeys>().get() else {
            return not_found(); // encrypted item but no keys this session
        };
        let Ok(ciphertext) = std::fs::read(&local_path) else {
            return not_found();
        };
        let size = plain_size.and_then(|s| usize::try_from(s).ok());
        let Ok(plain) = dec.decrypt_bytes(&key, &ciphertext, size) else {
            return not_found();
        };
        let _ = std::fs::create_dir_all(&thumbs_dir);
        // Unique per request so concurrent webview requests for the same id
        // (grid + lightbox, or strict-mode double-invokes) never clobber each
        // other's temp file mid-render.
        let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = thumbs_dir.join(format!("{id}.{seq}.decrypted"));
        if write_private(&tmp, &plain).is_err() {
            return not_found();
        }
        // RAII: the plaintext temp is removed when this guard drops, no matter how
        // we leave the block.
        let _tmp = TempPath(tmp.clone());
        media::render(&tmp, &thumbs_dir, id, want_thumb, mime.as_deref())
    } else {
        media::render(
            std::path::Path::new(&local_path),
            &thumbs_dir,
            id,
            want_thumb,
            mime.as_deref(),
        )
    };

    let Some(rendered) = rendered else {
        return not_found();
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", rendered.content_type)
        .header("Cache-Control", "no-cache")
        .body(rendered.bytes)
        .unwrap()
}

/// Serve a contact's photo over `traceloupe-avatar://localhost/<contactId>`.
///
/// Like the media handler, it takes only a numeric id and reads the bytes stored
/// for that contact in the active cache — never a path from the request.
fn avatar_protocol_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    let Some(id) = path.trim_start_matches('/').parse::<i64>().ok() else {
        return not_found();
    };
    let active = app.state::<ActiveBackup>();
    let Ok(cache_path) = active.path() else {
        return not_found();
    };
    let Ok(cache) = CacheDb::open(&cache_path) else {
        return not_found();
    };
    let Ok(Some(bytes)) = query::contact_image(&cache, id) else {
        return not_found();
    };

    let content_type = guess_image_mime(&bytes);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-cache")
        .body(bytes)
        .unwrap()
}

/// Sniff a bitmap's magic bytes; contact thumbnails are usually JPEG/PNG.
fn guess_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// Serve a message attachment over `traceloupe-attachment://localhost/<id>`
/// (`?thumb=1` for an image thumbnail). Images are transcoded/downscaled like
/// gallery media; audio/video are served as raw bytes with their stored mime.
fn attachment_protocol_response(
    app: &AppHandle,
    path: &str,
    query_str: Option<&str>,
    range: Option<&str>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    let Some(id) = path.trim_start_matches('/').parse::<i64>().ok() else {
        return not_found();
    };
    let want_thumb = query_str.is_some_and(|q| q.contains("thumb"));

    let active = app.state::<ActiveBackup>();
    let Ok(cache_path) = active.path() else {
        return not_found();
    };
    let Ok(cache) = CacheDb::open(&cache_path) else {
        return not_found();
    };
    let Ok(Some((local_path, mime, decrypt_key, plain_size))) = query::attachment_blob(&cache, id)
    else {
        return not_found();
    };

    // Its own thumbs/temp dir so attachment ids can't collide with media ids.
    let att_dir = cache_path
        .parent()
        .map(|p| p.join("att-thumbs"))
        .unwrap_or_else(|| PathBuf::from("att-thumbs"));

    // Resolve to a plaintext source: the backup file directly, or (encrypted
    // backup) a short-lived decrypted temp removed when `_tmp` drops.
    let (source_path, _tmp): (PathBuf, Option<TempPath>) = if let Some(key) = decrypt_key {
        let Some(dec) = app.state::<SessionKeys>().get() else {
            return not_found(); // encrypted attachment but no keys this session
        };
        let Ok(ciphertext) = std::fs::read(&local_path) else {
            return not_found();
        };
        let size = plain_size.and_then(|s| usize::try_from(s).ok());
        let Ok(plain) = dec.decrypt_bytes(&key, &ciphertext, size) else {
            return not_found();
        };
        let _ = std::fs::create_dir_all(&att_dir);
        let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = att_dir.join(format!("att-{id}.{seq}.decrypted"));
        if write_private(&tmp, &plain).is_err() {
            return not_found();
        }
        (tmp.clone(), Some(TempPath(tmp)))
    } else {
        (PathBuf::from(&local_path), None)
    };

    let is_image = mime.as_deref().is_some_and(|m| m.starts_with("image/"))
        || source_path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".heic");

    if is_image {
        let Some(rendered) = media::render(&source_path, &att_dir, id, want_thumb, mime.as_deref())
        else {
            return not_found();
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", rendered.content_type)
            .header("Cache-Control", "no-cache")
            .body(rendered.bytes)
            .unwrap();
    }

    // Audio/video and anything else served inline: raw bytes with stored mime.
    // Honor Range requests so <video>/<audio> can seek without re-downloading
    // (and without reading the whole file into memory each time). The stored MIME
    // is from the backup, so validate it before it becomes a response header.
    let content_type = media::safe_content_type(mime.as_deref());
    let Ok(meta) = std::fs::metadata(&source_path) else {
        return not_found();
    };
    let total = meta.len();

    if let Some((start, end)) = range.and_then(|r| parse_byte_range(r, total)) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&source_path) else {
            return not_found();
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return not_found();
        }
        let mut buf = vec![0u8; (end - start + 1) as usize];
        if file.read_exact(&mut buf).is_err() {
            return not_found();
        }
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header("Content-Type", content_type)
            .header("Accept-Ranges", "bytes")
            .header("Content-Range", format!("bytes {start}-{end}/{total}"))
            .header("Cache-Control", "no-cache")
            .body(buf)
            .unwrap();
    }

    let Ok(bytes) = std::fs::read(&source_path) else {
        return not_found();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Accept-Ranges", "bytes")
        .header("Cache-Control", "no-cache")
        .body(bytes)
        .unwrap()
}

/// Serve a voice recording over `traceloupe-audio://localhost/<id>`.
///
/// Like the media handler, it takes only a numeric id and reads the file recorded
/// for it in the active cache — never a path from the request. On an encrypted
/// backup the `.m4a` is decrypted with the session keys into a buffer (audio
/// files are small), then served; `Range` requests are honored against that
/// buffer so `<audio>` can seek.
fn audio_protocol_response(
    app: &AppHandle,
    path: &str,
    range: Option<&str>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    let Some(id) = path.trim_start_matches('/').parse::<i64>().ok() else {
        return not_found();
    };

    let active = app.state::<ActiveBackup>();
    let Ok(cache_path) = active.path() else {
        return not_found();
    };
    let Ok(cache) = CacheDb::open(&cache_path) else {
        return not_found();
    };
    let Ok(Some((local_path, mime, decrypt_key, plain_size))) = query::recording_blob(&cache, id)
    else {
        return not_found();
    };

    // Materialize the plaintext bytes: decrypt an encrypted original with the
    // session keys, or read a plaintext one straight off disk.
    let bytes = if let Some(key) = decrypt_key {
        let Some(dec) = app.state::<SessionKeys>().get() else {
            return not_found(); // encrypted item but no keys this session
        };
        let Ok(ciphertext) = std::fs::read(&local_path) else {
            return not_found();
        };
        let size = plain_size.and_then(|s| usize::try_from(s).ok());
        let Ok(plain) = dec.decrypt_bytes(&key, &ciphertext, size) else {
            return not_found();
        };
        plain
    } else {
        let Ok(raw) = std::fs::read(&local_path) else {
            return not_found();
        };
        raw
    };

    let content_type = media::safe_content_type(mime.as_deref());
    let total = bytes.len() as u64;

    if let Some((start, end)) = range.and_then(|r| parse_byte_range(r, total)) {
        let slice = bytes[start as usize..=end as usize].to_vec();
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header("Content-Type", content_type)
            .header("Accept-Ranges", "bytes")
            .header("Content-Range", format!("bytes {start}-{end}/{total}"))
            .header("Cache-Control", "no-cache")
            .body(slice)
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Accept-Ranges", "bytes")
        .header("Cache-Control", "no-cache")
        .body(bytes)
        .unwrap()
}

/// Parse a single-range `Range: bytes=start-end` header into an inclusive
/// `[start, end]` clamped to `total`. Supports `start-`, `start-end`, and
/// `-suffix`. Returns None for unsatisfiable or multi-range requests.
fn parse_byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end: u64 = if b.is_empty() {
            total - 1
        } else {
            b.parse::<u64>().ok()?.min(total - 1)
        };
        (start, end)
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(ActiveBackup::default())
        .manage(SessionKeys::default())
        .manage(ImportCancel::default())
        .manage(ReimportGate::default())
        // Asynchronous protocols: the handlers decrypt bytes and shell out to
        // `sips` to render/downscale images. On the *synchronous* scheme that
        // runs on the main thread, so scrolling a timeline or gallery full of
        // thumbnails/avatars froze the whole UI. Answer each request on a
        // blocking worker instead and hand the bytes back via the responder.
        .register_asynchronous_uri_scheme_protocol("traceloupe-media", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            let query = request.uri().query().map(str::to_string);
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(media_protocol_response(&app, &path, query.as_deref()));
            });
        })
        .register_asynchronous_uri_scheme_protocol(
            "traceloupe-avatar",
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let path = request.uri().path().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(avatar_protocol_response(&app, &path));
                });
            },
        )
        .register_asynchronous_uri_scheme_protocol(
            "traceloupe-attachment",
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let path = request.uri().path().to_string();
                let query = request.uri().query().map(str::to_string);
                let range = request
                    .headers()
                    .get("range")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(attachment_protocol_response(
                        &app,
                        &path,
                        query.as_deref(),
                        range.as_deref(),
                    ));
                });
            },
        )
        .register_asynchronous_uri_scheme_protocol("traceloupe-audio", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            let range = request
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(audio_protocol_response(&app, &path, range.as_deref()));
            });
        })
        .invoke_handler(tauri::generate_handler![
            list_backups,
            default_backup_root,
            open_full_disk_access_settings,
            engine_status,
            engine_info,
            install_engine,
            list_import_modules,
            set_log_level,
            cancel_import,
            import_backup,
            open_backup,
            has_active_backup,
            reimport_module,
            forget_backup,
            imported_backup_ids,
            list_threads,
            count_thread_messages,
            get_thread_message_window,
            count_timeline_messages,
            get_timeline_window,
            count_message_ranges,
            get_range_window,
            open_attachment,
            list_calls,
            list_notes,
            list_recordings,
            list_safari_history,
            list_contacts,
            list_installed_apps,
            list_media,
            media_sources,
            count_media,
            get_media_window,
            count_calls,
            get_calls_window,
            count_safari,
            get_safari_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
