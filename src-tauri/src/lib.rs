//! Thin Tauri command layer over traceloupe-core (architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

mod biometric;
mod logging;
mod media;
mod secret;
mod signing;

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

/// Decrypt an encrypted backup blob to a stable cached plaintext file (0600),
/// reused across requests (e.g. `<video>`/`<audio>` Range seeks) instead of
/// re-decrypting the whole file — and re-writing a whole temp — on every request.
///
/// The write goes to a unique temp then atomically renames into `out`, so
/// concurrent callers for the same id can never observe a half-written file. An
/// existing `out` whose size matches the expected plaintext size is reused as-is.
/// The plaintext lives under the cache dir, so `forget_backup` (and a backup
/// switch) clear it; it never outlives the backup being open.
fn decrypt_to_cache(
    dec: &BackupDecryptor,
    key: &[u8],
    ciphertext_path: &Path,
    plain_size: Option<i64>,
    out: &Path,
) -> Option<PathBuf> {
    let want = plain_size.and_then(|s| u64::try_from(s).ok());
    if let Ok(meta) = std::fs::metadata(out) {
        // Reuse only when the size matches (guards a truncated/partial leftover).
        if want.is_none_or(|w| meta.len() == w) {
            return Some(out.to_path_buf());
        }
    }
    let ciphertext = std::fs::read(ciphertext_path).ok()?;
    let size = plain_size.and_then(|s| usize::try_from(s).ok());
    let plain = dec.decrypt_bytes(key, &ciphertext, size).ok()?;
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = out.with_extension(format!("{seq}.partial"));
    if write_private(&tmp, &plain).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    if std::fs::rename(&tmp, out).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(out.to_path_buf())
}

/// Remove a backup's decrypted-plaintext temp files — the on-demand decrypted
/// originals (`*.decrypted`) and the externally-opened attachments (`att-open/`)
/// — without touching the parsed cache DB or the (already-decrypted-by-design)
/// rendered thumbnails. Called when a backup is closed/switched so full-plaintext
/// originals don't linger past the session that produced them.
fn clear_decrypted_temps(cache_dir: &Path) {
    let _ = std::fs::remove_dir_all(cache_dir.join("att-open"));
    for sub in ["att-thumbs", "thumbs", "note-thumbs"] {
        if let Ok(entries) = std::fs::read_dir(cache_dir.join(sub)) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "decrypted") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
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
    self, Call, Contact, HistoryVisit, MediaItem, Message, Note, Recording, SafariBookmark,
    ThreadSummary, TimelineMessage,
};
use traceloupe_core::sidecar::CancelToken;

/// The cache DB currently being browsed. Set when an import finishes or a
/// previously-imported backup is opened; read by every artifact query.
#[derive(Default)]
struct ActiveBackup(Mutex<Option<PathBuf>>);

impl ActiveBackup {
    fn set(&self, path: PathBuf) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    }
    fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    fn path(&self) -> Result<PathBuf, String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "no backup is open".to_string())
    }
}

/// The active backup's decryptor, for encrypted backups. Holds the unwrapped
/// keys (derived once from the Keychain-stored password) so full-resolution
/// photos can be decrypted on demand by the media protocol. `None` for
/// unencrypted backups. Keys live only in memory for the session.
#[derive(Default)]
struct SessionState {
    decryptor: Option<Arc<BackupDecryptor>>,
    /// Set once a biometric / Keychain unlock was cancelled or failed this session,
    /// so on-demand media loads stop re-prompting Touch ID for every single item
    /// (a photo grid would otherwise fire one prompt per tile). Cleared whenever
    /// keys are (re)set — a fresh import or an explicit reload — which is the user
    /// signalling they want to unlock again.
    auth_failed: bool,
}

#[derive(Default)]
struct SessionKeys(Mutex<SessionState>);

impl SessionKeys {
    fn set(&self, decryptor: Option<Arc<BackupDecryptor>>) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.decryptor = decryptor;
        g.auth_failed = false;
    }
    fn get(&self) -> Option<Arc<BackupDecryptor>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .decryptor
            .clone()
    }
}

/// The cancel token of the import currently in flight, so a `cancel_import`
/// command can stop it (killing the iLEAPP subprocess). `None` when idle.
#[derive(Default)]
struct ImportCancel(Mutex<Option<CancelToken>>);

/// Serializes every cache-writing import for a backup — full imports AND partial
/// re-imports. Only one may touch a backup's cache/media/temp files at a time.
/// Without this, a full import's atomic swap (renaming a fresh cache over the live
/// one) racing a re-import's in-place writes would silently drop the re-import's
/// rows, and two full imports would collide on the shared `cache.importing.db`
/// temp. Waiters queue rather than fail.
#[derive(Default)]
struct ImportGate(tauri::async_runtime::Mutex<()>);

/// Reconstruct the decryptor for an encrypted backup from its Keychain password
/// and the source dir recorded in its cache. `None` if not encrypted / no key, or
/// if the biometric gate (when enabled) isn't satisfied. Blocks on the Touch ID
/// prompt when biometric unlock is on, so call it off the async executor.
fn reopen_decryptor(cache_path: &Path, backup_id: &str) -> Option<Arc<BackupDecryptor>> {
    // Fetch the stored password first: no key → plaintext backup → None, and the
    // biometric prompt never fires for a plaintext backup.
    let password = secret::get(backup_id)?;
    if biometric::gate("Unlock this iPhone backup to access its data").is_err() {
        return None; // user cancelled / auth failed → keys stay locked
    }
    let cache = CacheDb::open(cache_path).ok()?;
    let source_dir = cache.get_meta("source_dir").ok().flatten()?;
    BackupDecryptor::open(Path::new(&source_dir), &password)
        .ok()
        .map(Arc::new)
}

/// The session decryptor for the currently-open encrypted backup, lazily rebuilt
/// from the Keychain password (prompting Touch ID if enabled) when it isn't
/// already loaded — so an on-demand decrypt (opening an attachment, serving media)
/// recovers when the keys didn't auto-load this session, instead of dead-ending on
/// "backup keys are not loaded". Blocks on Touch ID, so call off the async
/// executor. Returns None only for a plaintext backup or a genuine key failure.
fn ensure_session_decryptor(app: &AppHandle, active_path: &Path) -> Option<Arc<BackupDecryptor>> {
    let session = app.state::<SessionKeys>();
    // Hold the lock across the (possibly Touch-ID-prompting) rebuild so two
    // concurrent opens don't each prompt / re-derive — the first sets it, the rest
    // block briefly then reuse. Safe: this runs on a blocking worker, not across
    // an await.
    let mut guard = session.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = guard.decryptor.as_ref() {
        return Some(d.clone());
    }
    // A prior unlock this session was cancelled/failed — stay locked rather than
    // firing a fresh Touch ID prompt for every on-demand media load. Cleared by an
    // explicit re-set (import / reload) via SessionKeys::set.
    if guard.auth_failed {
        return None;
    }
    let backup_id = active_path.parent()?.file_name()?.to_str()?.to_owned();
    match reopen_decryptor(active_path, &backup_id) {
        Some(d) => {
            guard.decryptor = Some(d.clone());
            Some(d)
        }
        None => {
            guard.auth_failed = true;
            None
        }
    }
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
async fn list_backups(root: Option<String>) -> Result<DiscoveryResult, String> {
    // A full MobileSync scan touches the disk; keep it off the main thread so the
    // UI never freezes while discovering backups.
    tauri::async_runtime::spawn_blocking(move || {
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
            Err(traceloupe_core::Error::BackupDirNotFound { path }) => {
                Ok(DiscoveryResult::NotFound {
                    path: path.display().to_string(),
                })
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
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
    Indexing {
        /// Ready-to-display label for the current step (e.g. "Indexing Messages").
        step: String,
        /// 1-based step number and total, so the UI fills the bar `index/total`.
        index: u32,
        total: u32,
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
    if let Some(token) = import_cancel
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
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
    gate: State<'_, ImportGate>,
    backup_path: String,
    backup_id: String,
    password: String,
    modules: Vec<String>,
) -> Result<ImportResult, String> {
    if !valid_backup_id(&backup_id) {
        return Err("invalid backup id".to_string());
    }
    // Serialize against re-imports and any other import: only one writer touches a
    // backup's cache/temp at a time (held for the whole run).
    let _gate = gate.0.lock().await;
    // The engine is optional: TraceLoupe parses everything it surfaces natively,
    // so a missing iLEAPP is fine (import runs fully native). It's only used if a
    // future module reintroduces an iLEAPP key.
    let cfg = resolve_engine(&app);

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
    *import_cancel.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());
    let backup_path = PathBuf::from(backup_path);
    // Kept for post-import key setup (the originals are moved into the worker).
    let source_dir = backup_path.clone();
    // Hold the password only in zeroized buffers, so every copy is wiped from
    // memory on drop rather than lingering in a freed String allocation.
    let password = zeroize::Zeroizing::new(password);
    let key_password = zeroize::Zeroizing::new(password.to_string());

    // Blocking pipeline on a worker thread; progress is emitted as it runs.
    let result = tauri::async_runtime::spawn_blocking(move || {
        logging::info(&app, "Import started");
        // Time each phase/step for the dev console (start on entry, elapsed on
        // the next step boundary / completion).
        let import_start = Instant::now();
        let mut step_start = import_start;
        let mut current_step: Option<String> = None;
        import::import_backup(
            cfg.as_ref(),
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
                    ImportPhase::Indexing { step, index, total } => {
                        if let Some(prev) = current_step.take() {
                            logging::info(
                                &app,
                                format!(
                                    "\u{2713} {prev} ({} ms)",
                                    step_start.elapsed().as_millis()
                                ),
                            );
                        }
                        logging::info(&app, format!("\u{25b6} {step} ({index}/{total})"));
                        current_step = Some(step.clone());
                        step_start = Instant::now();
                        Some(ImportEvent::Indexing {
                            step: step.clone(),
                            index: *index,
                            total: *total,
                        })
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
    *import_cancel.0.lock().unwrap_or_else(|e| e.into_inner()) = None;

    let outcome = result
        .map_err(|e| format!("import task panicked: {e}"))?
        .map_err(|e| e.to_string())?;

    // Newly imported backup becomes the active one for browsing.
    active.set(outcome.cache_path.clone());

    // Remember the source dir for every backup — a partial re-import needs it to
    // locate the backup's files (encrypted or not).
    if let Ok(cache) = CacheDb::open(&outcome.cache_path) {
        let _ = cache.set_meta("source_dir", &source_dir.display().to_string());
    }
    // Encrypted backup: stash the password in the Keychain and hold the decryptor
    // for on-demand media decryption. Unencrypted: clear any stale secret/keys.
    if key_password.is_empty() {
        session.set(None);
        secret::delete(&backup_id);
    } else {
        if let Err(e) = secret::store(&backup_id, &key_password) {
            eprintln!("could not store backup password in Keychain: {e}");
        }
        // Deriving the keys is PBKDF2 (several hundred ms) — keep it off the async
        // executor, like reopen_decryptor does.
        let sd = source_dir.clone();
        let pw = key_password.clone();
        let decryptor = tauri::async_runtime::spawn_blocking(move || {
            BackupDecryptor::open(&sd, &pw).ok().map(Arc::new)
        })
        .await
        .ok()
        .flatten();
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
    // Serialize against an in-flight import's atomic cache swap, so we never point
    // ActiveBackup at a cache mid-write. Fetched from `app` (not a `State` param)
    // so this command can keep its plain `bool` return.
    let gate = app.state::<ImportGate>();
    let _gate = gate.0.lock().await;
    let Ok(data_dir) = app.path().app_data_dir() else {
        return false;
    };
    let cache_path = data_dir.join("caches").join(&backup_id).join("cache.db");
    if !cache_path.exists() {
        return false;
    }
    // Switching away from another backup: drop its decrypted-plaintext temps so
    // full-plaintext originals don't linger once it's no longer the open one.
    if let Ok(prev) = app.state::<ActiveBackup>().path() {
        if !prev.starts_with(data_dir.join("caches").join(&backup_id)) {
            if let Some(prev_dir) = prev.parent() {
                clear_decrypted_temps(prev_dir);
            }
        }
    }
    // Rebuilding the decryptor reads the Keychain, opens the cache and runs
    // PBKDF2 — all blocking. Keep it off the main thread so selecting a backup
    // never freezes the UI (no-op decryptor for plaintext backups).
    let cp = cache_path.clone();
    let decryptor = tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&cp, &backup_id))
        .await
        .ok()
        .flatten();
    // Surface a silent key-load failure: if this backup is encrypted but we have
    // no decryptor, full-resolution photos and native re-imports won't work until
    // the keys load. Point at the likely cause — a cancelled/unavailable Touch ID
    // prompt when biometric unlock is on, otherwise the Keychain-ACL/signing issue
    // (a rebuilt dev binary loses access; see docs/signing.md).
    if decryptor.is_none() {
        if let Ok(Some(src)) = CacheDb::open(&cache_path).and_then(|c| c.get_meta("source_dir")) {
            if discovery::read_backup_info(Path::new(&src)).is_encrypted == Some(true) {
                let msg = if biometric::is_required() {
                    "Backup is encrypted and Touch ID unlock is on, but its keys weren't unlocked \
                     (Touch ID cancelled/failed, or unavailable on this build). Authenticate when \
                     prompted, or turn off Require Touch ID in Settings."
                } else {
                    "Backup is encrypted but its keys couldn't be loaded from the Keychain — \
                     full-resolution photos and native re-imports are unavailable. Re-import with \
                     the password, or sign the build with a stable identity (docs/signing.md)."
                };
                logging::warn(&app, msg);
            }
        }
    }
    app.state::<SessionKeys>().set(decryptor);
    app.state::<ActiveBackup>().set(cache_path);
    true
}

/// Whether a backup is currently open for browsing.
#[tauri::command]
fn has_active_backup(active: State<'_, ActiveBackup>) -> bool {
    active.path().is_ok()
}

/// Turn the Touch ID gate for backup keys on/off (persisted by the frontend and
/// re-applied at startup). When on, reconstructing an encrypted backup's decryptor
/// prompts for Touch ID first.
#[tauri::command]
fn set_biometric_required(enabled: bool) {
    biometric::set_required(enabled);
}

/// The running app's code-signing status. The UI uses it to decide whether Touch
/// ID / stable Keychain persistence can work (they need a real, non-adhoc
/// signature — see docs/signing.md).
#[tauri::command]
async fn app_signing_status() -> signing::SigningStatus {
    tauri::async_runtime::spawn_blocking(signing::status)
        .await
        .unwrap_or(signing::SigningStatus {
            signed: false,
            adhoc: false,
            identity: None,
        })
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
    calls: usize,
    safari_visits: usize,
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
    gate: State<'_, ImportGate>,
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

    // Decryption keys. The session may not hold them (e.g. the backup was
    // reopened in a session where the Keychain read didn't yield a live
    // decryptor); rebuild from the Keychain if we can and cache it back. Off the
    // async executor — reopen_decryptor may block on a Touch ID prompt.
    let mut decryptor = session.get();
    if decryptor.is_none() {
        let cp = cache_path.clone();
        let bid = backup_id.to_string();
        let rebuilt = tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&cp, &bid))
            .await
            .ok()
            .flatten();
        if let Some(d) = rebuilt {
            session.set(Some(d.clone()));
            decryptor = Some(d);
        }
    }
    // An encrypted backup with no keys would open its Manifest as plaintext and
    // fail with a cryptic "file is not a database" — give an actionable error.
    if decryptor.is_none()
        && discovery::read_backup_info(Path::new(&source_dir)).is_encrypted == Some(true)
    {
        logging::error(
            &app,
            format!("\u{2717} Re-import {label}: backup keys aren't loaded"),
        );
        return Err(
            "This backup is encrypted, but its decryption keys aren't loaded. Reopen the \
             backup (allow Keychain access when prompted) or re-import it with its password, \
             then try again."
                .to_string(),
        );
    }

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
        calls: report.calls,
        safari_visits: report.safari_visits,
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
        "calls" => "call history",
        "safari" => "Safari history",
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
        "calls" => format!("{} calls", r.calls),
        "safari" => format!("{} Safari visits", r.safari_visits),
        _ => String::new(),
    }
}

/// Forget an imported backup: delete its cache DB and all derived caches
/// (media/thumbs), its work dir, and its stored password. Does not touch the
/// original backup on disk. Re-importing recreates everything.
#[tauri::command]
async fn forget_backup(
    app: AppHandle,
    gate: State<'_, ImportGate>,
    backup_id: String,
) -> Result<(), String> {
    if !valid_backup_id(&backup_id) {
        return Err("invalid backup id".to_string());
    }
    // Serialize against imports/re-imports so we don't delete a cache dir while an
    // import is writing it (which could resurrect a half-written cache or fail the
    // import mid-write).
    let _gate = gate.0.lock().await;
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

/// Device + backup metadata for the active backup (name, model, iOS version,
/// serial, last-backup date, encryption). Re-reads the source backup's Info.plist
/// via the `source_dir` stored in the cache; None if that isn't recorded.
#[tauri::command]
async fn device_info(active: State<'_, ActiveBackup>) -> Result<Option<BackupInfo>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        let Some(source_dir) = cache.get_meta("source_dir").map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        Ok(Some(discovery::read_backup_info(Path::new(&source_dir))))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Distinct content kinds present (with counts) for the message content filter.
/// `thread_id` scopes to a conversation; otherwise all messages in `service`.
#[tauri::command]
async fn message_kinds(
    active: State<'_, ActiveBackup>,
    thread_id: Option<i64>,
    service: Option<String>,
) -> Result<Vec<(String, i64)>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::message_kinds(&cache, thread_id, service.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_thread_messages(
    active: State<'_, ActiveBackup>,
    thread_id: i64,
    kind: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_messages(&cache, thread_id, kind.as_deref()).map_err(|e| e.to_string())
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
    kind: Option<String>,
    desc: bool,
) -> Result<Vec<Message>, String> {
    // Async + spawn_blocking: a synchronous command runs on the main thread and
    // would freeze the whole native UI. Only the requested window is read, so
    // the frontend can lazily load a thread as it scrolls.
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_message_window(&cache, thread_id, offset, limit, kind.as_deref(), desc)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn thread_message_index(
    active: State<'_, ActiveBackup>,
    thread_id: i64,
    message_id: i64,
    kind: Option<String>,
    desc: bool,
) -> Result<Option<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::message_row_index(&cache, thread_id, message_id, kind.as_deref(), desc)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A camera-roll item matched to a missing message attachment by file name.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveredMedia {
    id: i64,
    kind: String,
}

/// Find a Photos (camera-roll) item that matches a missing message attachment by
/// file name, so the offloaded-to-iCloud attachment can be shown from Photos
/// instead. Best-effort — the UI gates it behind a setting and labels it.
#[tauri::command]
async fn recover_attachment_media(
    active: State<'_, ActiveBackup>,
    attachment_id: i64,
) -> Result<Option<RecoveredMedia>, String> {
    let path = active.path()?;
    let found = tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::recover_attachment_media(&cache, attachment_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(found.map(|(id, kind)| RecoveredMedia { id, kind }))
}

#[tauri::command]
async fn count_timeline_messages(
    active: State<'_, ActiveBackup>,
    service: Option<String>,
    search: Option<String>,
    kind: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_all_messages(
            &cache,
            service.as_deref(),
            search.as_deref(),
            kind.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: paging + service + search + kind + dir.
async fn get_timeline_window(
    active: State<'_, ActiveBackup>,
    offset: i64,
    limit: i64,
    service: Option<String>,
    search: Option<String>,
    kind: Option<String>,
    desc: bool,
) -> Result<Vec<TimelineMessage>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_timeline_window(
            &cache,
            offset,
            limit,
            service.as_deref(),
            search.as_deref(),
            kind.as_deref(),
            desc,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The earliest/latest dated message (Unix seconds), for the Timeline's per-year
/// quick filters. `None` when there are no dated messages.
#[tauri::command]
async fn message_date_bounds(
    active: State<'_, ActiveBackup>,
) -> Result<Option<(i64, i64)>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::message_date_bounds(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_message_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
    service: Option<String>,
    search: Option<String>,
    kind: Option<String>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_message_ranges(
            &cache,
            &ranges,
            service.as_deref(),
            search.as_deref(),
            kind.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: time range + service + search + paging + dir.
async fn get_range_window(
    active: State<'_, ActiveBackup>,
    lo: Option<i64>,
    hi: Option<i64>,
    offset: i64,
    limit: i64,
    service: Option<String>,
    search: Option<String>,
    kind: Option<String>,
    desc: bool,
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
            search.as_deref(),
            kind.as_deref(),
            desc,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a message attachment's file with the OS default app (for documents and
/// anything not rendered inline).
#[tauri::command]
async fn open_attachment(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    attachment_id: i64,
) -> Result<(), String> {
    let active_path = active.path()?;
    // Reading + full-file AES-decrypting a large attachment (and possibly a Touch
    // ID prompt to reload keys) must not run on the main thread; do it on a
    // blocking worker.
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&active_path).map_err(|e| e.to_string())?;
        let (local_path, filename, _mime, decrypt_key, plain_size) =
            query::attachment_blob(&cache, attachment_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "attachment file is not available".to_string())?;

        // Materialize to a temp named with the attachment's REAL filename (so its
        // extension is present) and open THAT — the `local_path` is the backup's
        // content-addressed blob (a hex file-id with no extension), so opening it
        // directly makes macOS fall back to TextEdit and show binary garbage.
        // Encrypted → decrypt first; plaintext → copy. The temp lives under the
        // cache dir (0600), cleared on re-import/forget/backup-switch.
        let plain = if let Some(key) = decrypt_key {
            let dec = ensure_session_decryptor(&app, &active_path).ok_or_else(|| {
                "backup keys are not loaded (unlock the backup, or re-import if this \
                 is a rebuilt dev binary)"
                    .to_string()
            })?;
            let ciphertext = std::fs::read(&local_path).map_err(|e| e.to_string())?;
            let size = plain_size.and_then(|s| usize::try_from(s).ok());
            dec.decrypt_bytes(&key, &ciphertext, size)
                .map_err(|e| e.to_string())?
        } else {
            std::fs::read(&local_path).map_err(|e| e.to_string())?
        };
        let dir = active_path
            .parent()
            .map(|p| p.join("att-open"))
            .ok_or_else(|| "unexpected cache layout".to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Sanitize the display name to a bare filename so it can't escape att-open.
        let base = filename
            .as_deref()
            .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f).replace('\0', ""))
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty() && f != "." && f != "..")
            .unwrap_or_else(|| format!("attachment-{attachment_id}"));
        let dest = dir.join(format!("{attachment_id}-{base}"));
        write_private(&dest, &plain).map_err(|e| e.to_string())?;

        // The filename (hence extension) comes from the backup, so a sender could
        // pick a type whose default handler runs the file's contents (.html/.webloc
        // from a file:// origin, scripts, etc.). Reveal those in Finder instead of
        // launching their handler; open ordinary media/documents directly.
        let ext = base
            .rsplit('.')
            .next()
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        const REVEAL_ONLY: &[&str] = &[
            "html", "htm", "xhtml", "shtml", "svg", "webloc", "fileloc", "url", "desktop",
            "command", "sh", "bash", "zsh", "csh", "terminal", "scpt", "app", "pkg", "mpkg", "dmg",
            "action", "workflow", "shortcut", "jar",
        ];
        let mut cmd = std::process::Command::new("/usr/bin/open");
        if REVEAL_ONLY.contains(&ext.as_str()) {
            cmd.arg("-R"); // reveal in Finder; let the user decide
        }
        cmd.arg(&dest).spawn().map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
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

/// Decrypt a password-protected note's body on demand. The plaintext is returned
/// to the UI but never stored. Runs off the async executor (PBKDF2 is CPU-heavy).
#[tauri::command]
async fn unlock_note(
    active: State<'_, ActiveBackup>,
    note_id: i64,
    password: String,
) -> Result<String, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        let (salt, iter, iv, tag, enc, wrapped) = query::note_crypto(&cache, note_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "This note isn't locked, or its encrypted data is missing.".to_string()
            })?;
        let iterations = u32::try_from(iter).unwrap_or(0);
        traceloupe_core::parsers::notes::decrypt_locked_note(
            &password, &salt, iterations, &iv, &tag, &enc, &wrapped,
        )
        .ok_or_else(|| "Wrong password.".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_calendar_events(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::CalendarEvent>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_calendar_events(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_interactions(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::Interaction>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_interactions(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_workouts(active: State<'_, ActiveBackup>) -> Result<Vec<query::Workout>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_workouts(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn workout_route(
    active: State<'_, ActiveBackup>,
    workout_id: i64,
) -> Result<Vec<query::RoutePoint>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::workout_route(&cache, workout_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn health_daily(active: State<'_, ActiveBackup>) -> Result<Vec<query::HealthDay>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::health_daily(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_sleep(active: State<'_, ActiveBackup>) -> Result<Vec<query::SleepSession>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_sleep(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn health_summary(active: State<'_, ActiveBackup>) -> Result<query::HealthSummary, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::health_summary(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_reminders(active: State<'_, ActiveBackup>) -> Result<Vec<query::Reminder>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_reminders(&cache).map_err(|e| e.to_string())
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
fn safari_bookmark_sort(field: &str, desc: bool) -> query::Sort {
    let col = match field {
        "title" => "title COLLATE NOCASE",
        "folder" => "folder COLLATE NOCASE",
        _ => "date_added",
    };
    query::Sort::new(col, desc)
}

#[tauri::command]
async fn count_media(
    active: State<'_, ActiveBackup>,
    source: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
    search: Option<String>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_media(
            &cache,
            source.as_deref(),
            query::TimeRange { lo, hi },
            search.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_media_ranges(
    active: State<'_, ActiveBackup>,
    source: Option<String>,
    ranges: Vec<query::TimeRange>,
    search: Option<String>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_media_ranges(&cache, source.as_deref(), &ranges, search.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: source + time range + search + paging + sort.
async fn get_media_window(
    active: State<'_, ActiveBackup>,
    source: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
    search: Option<String>,
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
            query::TimeRange { lo, hi },
            search.as_deref(),
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
    lo: Option<i64>,
    hi: Option<i64>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_calls(&cache, search.as_deref(), query::TimeRange { lo, hi })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_call_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
    search: Option<String>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_call_ranges(&cache, &ranges, search.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: search + range + paging + sort.
async fn get_calls_window(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
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
            query::TimeRange { lo, hi },
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
    lo: Option<i64>,
    hi: Option<i64>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari(&cache, search.as_deref(), query::TimeRange { lo, hi })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_safari_ranges(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari_ranges(&cache, search.as_deref(), &ranges).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: search + time range + paging + sort.
async fn get_safari_window(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
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
            query::TimeRange { lo, hi },
            offset,
            limit,
            safari_sort(&sort_by, desc),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_safari_bookmarks(
    active: State<'_, ActiveBackup>,
    kind: String,
    search: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari_bookmarks(
            &cache,
            &kind,
            search.as_deref(),
            query::TimeRange { lo, hi },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_safari_bookmark_ranges(
    active: State<'_, ActiveBackup>,
    kind: String,
    search: Option<String>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari_bookmark_ranges(&cache, &kind, search.as_deref(), &ranges)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: kind + search + time range + paging + sort.
async fn get_safari_bookmarks_window(
    active: State<'_, ActiveBackup>,
    kind: String,
    search: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
    offset: i64,
    limit: i64,
    sort_by: String,
    desc: bool,
) -> Result<Vec<SafariBookmark>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_safari_bookmarks_window(
            &cache,
            &kind,
            search.as_deref(),
            query::TimeRange { lo, hi },
            offset,
            limit,
            safari_bookmark_sort(&sort_by, desc),
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
        let Some(dec) = ensure_session_decryptor(app, &cache_path) else {
            return not_found(); // encrypted item, and keys couldn't be loaded (no stored password)
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

/// Serve a note's first-image thumbnail over `traceloupe-note-image://localhost/<noteId>`.
///
/// Takes only a numeric note id and resolves the image's backup blob from the
/// active cache — never a path from the request. The blob is decrypted on demand
/// (encrypted backups) and rendered to a downscaled JPEG thumbnail via `sips`.
fn note_image_protocol_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    // "/<id>" serves the note's first image (list thumbnail); "/<id>/<index>"
    // serves the index-th image from note_media (the detail gallery).
    let mut parts = path.trim_start_matches('/').split('/');
    let Some(id) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
        return not_found();
    };
    let index = parts.next().and_then(|s| s.parse::<i64>().ok());
    let active = app.state::<ActiveBackup>();
    let Ok(cache_path) = active.path() else {
        return not_found();
    };
    let Ok(cache) = CacheDb::open(&cache_path) else {
        return not_found();
    };
    let blob = match index {
        Some(i) => query::note_media_blob(&cache, id, i),
        None => query::note_image_blob(&cache, id),
    };
    let Ok(Some((local_path, mime, _thumb, decrypt_key, plain_size))) = blob else {
        return not_found();
    };
    // A cache key unique per (note, index) so rendered/decrypted files don't clash.
    let key = match index {
        Some(i) => id.wrapping_mul(100_000).wrapping_add(i),
        None => id,
    };

    let thumbs_dir = cache_path
        .parent()
        .map(|p| p.join("note-thumbs"))
        .unwrap_or_else(|| PathBuf::from("note-thumbs"));

    let rendered = if let Some(wrapped) = decrypt_key {
        let Some(dec) = ensure_session_decryptor(app, &cache_path) else {
            return not_found(); // encrypted image, and keys couldn't be loaded (no stored password)
        };
        let out = thumbs_dir.join(format!("note-{key}.decrypted"));
        let Some(src) = decrypt_to_cache(&dec, &wrapped, Path::new(&local_path), plain_size, &out)
        else {
            return not_found();
        };
        media::render(&src, &thumbs_dir, key, true, mime.as_deref())
    } else {
        media::render(
            Path::new(&local_path),
            &thumbs_dir,
            key,
            true,
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
    let Ok(Some((local_path, filename, mime, decrypt_key, plain_size))) =
        query::attachment_blob(&cache, id)
    else {
        return not_found();
    };

    // Its own thumbs/temp dir so attachment ids can't collide with media ids.
    let att_dir = cache_path
        .parent()
        .map(|p| p.join("att-thumbs"))
        .unwrap_or_else(|| PathBuf::from("att-thumbs"));

    // Resolve to a plaintext source: the backup file directly, or (encrypted
    // backup) a decrypted temp cached by id. Caching matters for media: the
    // webview issues many `Range` requests while scrubbing a video, and
    // re-decrypting the whole file (and re-writing a whole temp) per request is an
    // OOM/disk-thrash path. `clear_decrypted_temps` removes these on close/forget.
    let source_path: PathBuf = if let Some(key) = decrypt_key {
        let Some(dec) = ensure_session_decryptor(app, &cache_path) else {
            return not_found(); // encrypted attachment, and keys couldn't be loaded (no stored password)
        };
        let out = att_dir.join(format!("att-{id}.decrypted"));
        let Some(p) = decrypt_to_cache(&dec, &key, Path::new(&local_path), plain_size, &out) else {
            return not_found();
        };
        p
    } else {
        PathBuf::from(&local_path)
    };

    // Detect an image by MIME, else by the ORIGINAL filename's extension — an
    // encrypted backup's on-disk source is a `.decrypted` temp with no meaningful
    // extension, and sms.db often stores a NULL mime for image attachments, so
    // MIME-only detection would serve them as octet-stream (won't render).
    let is_image = mime.as_deref().is_some_and(|m| m.starts_with("image/"))
        || media::has_image_extension(filename.as_deref());

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

    // Audio/video served inline (Range-seekable); anything else (html/svg/js/…)
    // is forced to a download type so an attacker-supplied attachment can't run as
    // a document in the custom-scheme origin. The stored MIME is untrusted, so it's
    // validated for header-safety inside the helper.
    let content_type = media::inline_media_content_type(mime.as_deref());
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

    // Resolve to a plaintext source path: the file directly, or (encrypted) a
    // decrypt-once temp cached by id — so a memo's Range seeks don't re-decrypt the
    // whole `.m4a` each time. Cleared on close/forget by `clear_decrypted_temps`.
    let cache_dir = cache_path
        .parent()
        .map(|p| p.join("att-thumbs"))
        .unwrap_or_else(|| PathBuf::from("att-thumbs"));
    let source_path: PathBuf = if let Some(key) = decrypt_key {
        let Some(dec) = ensure_session_decryptor(app, &cache_path) else {
            return not_found(); // encrypted item, and keys couldn't be loaded (no stored password)
        };
        let out = cache_dir.join(format!("audio-{id}.decrypted"));
        let Some(p) = decrypt_to_cache(&dec, &key, Path::new(&local_path), plain_size, &out) else {
            return not_found();
        };
        p
    } else {
        PathBuf::from(&local_path)
    };

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

/// An OpenGraph link preview. All fields best-effort.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkPreview {
    url: String,
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
    site_name: Option<String>,
}

/// Fetch a URL's OpenGraph/title metadata for a link preview. **Opt-in**: the UI
/// only calls this when the user enables link previews — it makes an outbound
/// request to the linked site. http/https only, short timeout, HTML capped, and
/// private/loopback/link-local hosts are refused (SSRF guard). The preview image
/// is fetched here and returned as a `data:` URL so the webview never contacts
/// the image host directly (no IP leak beyond this backend request).
#[tauri::command]
async fn fetch_link_preview(app: AppHandle, url: String) -> Result<LinkPreview, String> {
    let result = tauri::async_runtime::spawn_blocking({
        let url = url.clone();
        move || {
            // TikTok serves no OpenGraph to server-side fetchers (a JS shell), so
            // scraping yields only a bare <title>. Its oEmbed endpoint returns the
            // caption, author and a thumbnail — use it (it also resolves
            // vm.tiktok.com short links itself). Fall through to scraping if it
            // comes back empty.
            if url_host(&url).is_some_and(|h| h == "tiktok.com" || h.ends_with(".tiktok.com")) {
                if let Ok(p) = tiktok_oembed(&url) {
                    if p.title.is_some() || p.image.is_some() {
                        return Ok(p);
                    }
                }
            }
            // 2 MB cap: big pages (e.g. YouTube ~1.2 MB) put their OpenGraph tags
            // well past 512 KB — byte ~662 KB on a watch page — so a smaller cap
            // truncates before the meta tags and yields no preview.
            let (final_url, body) = safe_http_get(&url, 2 * 1024 * 1024, Some("html"))?;
            let html = String::from_utf8_lossy(&body);
            let image = meta_content(&html, "og:image")
                .map(|i| absolutize(&final_url, &i))
                .and_then(|i| proxy_image(&i));
            Ok::<LinkPreview, String>(LinkPreview {
                title: meta_content(&html, "og:title").or_else(|| html_title(&html)),
                description: meta_content(&html, "og:description"),
                site_name: meta_content(&html, "og:site_name"),
                image,
                url,
            })
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    // Structural diagnostic (no content): whether each field came back, or why not.
    match &result {
        Ok(p) => logging::debug(
            &app,
            format!(
                "link-preview {}: title={} image={} desc={}",
                url,
                p.title.is_some(),
                p.image.is_some(),
                p.description.is_some()
            ),
        ),
        Err(e) => logging::debug(&app, format!("link-preview {url}: failed: {e}")),
    }
    result
}

/// A link preview for a TikTok URL via its public oEmbed endpoint (TikTok serves
/// no OpenGraph to bots). Returns the caption as the title, the creator as the
/// description, and the video thumbnail (proxied to a `data:` URL). oEmbed
/// resolves `vm.tiktok.com` short links itself, so any TikTok URL works.
fn tiktok_oembed(url: &str) -> Result<LinkPreview, String> {
    let endpoint = format!("https://www.tiktok.com/oembed?url={}", percent_encode(url));
    let (_final, body) = safe_http_get(&endpoint, 256 * 1024, None)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let image = field("thumbnail_url").and_then(|t| proxy_image(&t));
    Ok(LinkPreview {
        title: field("title"),
        description: field("author_name"),
        site_name: Some("TikTok".into()),
        image,
        url: url.to_string(),
    })
}

/// Percent-encode a string for use as a URL query value (encode everything but
/// the RFC 3986 unreserved set).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A hardened GET for opt-in previews: http/https only; refuses private/loopback/
/// link-local hosts (resolving names, so a public-looking host that maps to a
/// private IP is caught too — SSRF guard); follows at most a few redirects,
/// re-validating each hop; caps the body; optionally requires an html/image
/// content-type. Returns the final URL and the (capped) body bytes.
/// A ureq resolver that only ever yields globally-routable addresses. ureq
/// connects to exactly the addresses its resolver returns, and re-runs the
/// resolver on every redirect hop (each is a fresh connection) — so validating
/// *here*, rather than in a separate pre-check, is what closes the DNS-rebind
/// TOCTOU: the address we vet is the address ureq dials, with no second lookup in
/// between. An all-private (or empty) result becomes a connection error, so the
/// fetch fails closed. This matters because preview URLs come from third-party
/// messages in a backup that may be of a compromised phone — i.e. attacker-
/// controlled input that a naive resolve-then-connect check can be rebound past.
/// TLS still validates the certificate against the original hostname (SNI is set
/// from the URL, not the pinned IP), so pinning the IP doesn't weaken cert checks.
struct PublicOnlyResolver;

impl ureq::Resolver for PublicOnlyResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
        use std::net::ToSocketAddrs;
        let addrs: Vec<std::net::SocketAddr> = netloc
            .to_socket_addrs()?
            .filter(|a| ip_is_global(a.ip()))
            .collect();
        if addrs.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "refusing to connect to a private or non-global address",
            ));
        }
        Ok(addrs)
    }
}

fn safe_http_get(url: &str, cap: u64, want: Option<&str>) -> Result<(String, Vec<u8>), String> {
    use std::io::Read;
    let agent = ureq::builder()
        .redirects(0)
        // The authoritative SSRF guard: every address ureq connects to is vetted
        // by this resolver, closing the rebind window the host_is_public pre-check
        // below can't (it resolves separately, then ureq resolves again).
        .resolver(PublicOnlyResolver)
        .timeout(std::time::Duration::from_secs(8))
        .build();
    let mut current = url.to_string();
    for _hop in 0..5 {
        let lower = current.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err("unsupported URL scheme".into());
        }
        // Cheap first-line reject: hostname literals (localhost/.local/.internal)
        // and hosts that statically resolve to a private/non-global address. This
        // is NOT the TOCTOU-safe layer on its own — it resolves separately from the
        // connect — but it gives a clear error and short-circuits the obvious cases.
        // The real rebind-proof guard is PublicOnlyResolver on the agent above,
        // which vets the exact address ureq connects to.
        let host = url_host(&current).ok_or("malformed URL")?;
        if !host_is_public(&host) {
            return Err("refusing to fetch a private or loopback host".into());
        }
        match agent
            .get(&current)
            // A crawler-style UA (not a full browser one): sites like Spotify and
            // Instagram serve OpenGraph tags to crawlers but a JS app-shell or a
            // login wall to browsers, so impersonating a browser would *lose*
            // those previews. Some sites (e.g. newbalance.se) hard-block any
            // server fetch regardless — those fall back to the domain card.
            .set("User-Agent", "Mozilla/5.0 TraceLoupe/link-preview")
            .call()
        {
            Ok(resp) => {
                // With `redirects(0)`, ureq returns a 3xx as `Ok` (not `Err`), so
                // we must follow the Location ourselves — otherwise the
                // content-type check below runs against the redirect response
                // (often `application/binary`, e.g. m.youtube.com) and wrongly
                // rejects it. Each hop's host is re-validated on the next
                // iteration (SSRF guard).
                if (300..400).contains(&resp.status()) {
                    let loc = resp.header("Location").ok_or("redirect without Location")?;
                    current = absolutize(&current, loc);
                    continue;
                }
                if let Some(kind) = want {
                    let ct = resp
                        .header("Content-Type")
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let ok = match kind {
                        "html" => ct.is_empty() || ct.contains("text/html") || ct.contains("xhtml"),
                        "image" => ct.starts_with("image/"),
                        _ => true,
                    };
                    if !ok {
                        return Err(format!("unexpected content-type: {ct}"));
                    }
                }
                let mut buf = Vec::new();
                resp.into_reader()
                    .take(cap)
                    .read_to_end(&mut buf)
                    .map_err(|e| e.to_string())?;
                return Ok((current, buf));
            }
            // Belt-and-suspenders: if a build of ureq surfaces a 3xx as an error
            // instead, follow it the same way.
            Err(ureq::Error::Status(code, resp)) if (300..400).contains(&code) => {
                let loc = resp.header("Location").ok_or("redirect without Location")?;
                current = absolutize(&current, loc);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("too many redirects".into())
}

/// The host of an http(s) URL (no port, no userinfo; IPv6 literal unwrapped).
fn url_host(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let authority = after.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?; // strip userinfo
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next()?.to_string() // IPv6 literal
    } else {
        authority.split(':').next()?.to_string()
    };
    (!host.is_empty()).then_some(host)
}

/// Whether a host is safe to fetch for a preview — not loopback/private/link-local.
/// Resolves the name so a public-looking host that maps to a private IP is caught.
fn host_is_public(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty()
        || h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".internal")
    {
        return false;
    }
    use std::net::ToSocketAddrs;
    match (h.as_str(), 80u16).to_socket_addrs() {
        Ok(addrs) => {
            let mut resolved = false;
            for a in addrs {
                resolved = true;
                if !ip_is_global(a.ip()) {
                    return false;
                }
            }
            resolved
        }
        Err(_) => false, // can't resolve → don't fetch
    }
}

/// A conservative "is this a globally-routable IP" check (`IpAddr::is_global` is
/// still unstable, so hand-roll the non-global ranges we care about).
fn ip_is_global(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64)) // 100.64/10 CGNAT
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (s[0] & 0xffc0) == 0xfe80) // fe80::/10 link-local
        }
    }
}

/// Fetch an image via the SSRF-safe GET and return it as a `data:` URL, so the
/// webview never contacts the image host. None on any failure (never falls back
/// to the raw URL, which would leak the user's IP).
fn proxy_image(url: &str) -> Option<String> {
    let (_final, bytes) = safe_http_get(url, 2 * 1024 * 1024, Some("image")).ok()?;
    let mime = sniff_image_mime(&bytes)?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

/// Recognize a preview image by magic bytes (only these are embedded).
fn sniff_image_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The `content` of the first `<meta property|name="key">` tag (either attribute
/// order), HTML-unescaped. Best-effort string scan (no HTML parser dependency).
fn meta_content(html: &str, key: &str) -> Option<String> {
    for tag in html.split("<meta").skip(1) {
        let end = match tag.find('>') {
            Some(e) => e,
            None => continue,
        };
        let attrs = &tag[..end];
        let key_matches = attr_val(attrs, "property").as_deref() == Some(key)
            || attr_val(attrs, "name").as_deref() == Some(key);
        if key_matches {
            if let Some(c) = attr_val(attrs, "content") {
                let c = html_unescape(c.trim());
                if !c.is_empty() {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// The value of attribute `name` in a tag's attribute string (case-insensitive
/// name, single- or double-quoted value).
fn attr_val(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let i = from + rel;
        let boundary = i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
        let after = &attrs[i + name.len()..];
        let after_eq = after.trim_start();
        if boundary {
            if let Some(rest) = after_eq.strip_prefix('=') {
                let rest = rest.trim_start();
                let quote = rest.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let body = &rest[1..];
                    if let Some(endq) = body.find(quote) {
                        return Some(body[..endq].to_string());
                    }
                }
            }
        }
        from = i + name.len();
    }
    None
}

/// `<title>…</title>` text, if present.
fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let gt = lower[start..].find('>')? + start + 1;
    let end = lower[gt..].find("</title>")? + gt;
    let t = html_unescape(html[gt..end].trim());
    (!t.is_empty()).then_some(t)
}

/// Minimal HTML entity unescaping for preview text.
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

/// Resolve a possibly-relative image URL against the page URL.
fn absolutize(base: &str, img: &str) -> String {
    if img.starts_with("http://") || img.starts_with("https://") {
        return img.to_string();
    }
    if let Some(rest) = img.strip_prefix("//") {
        let scheme = base.split(':').next().unwrap_or("https");
        return format!("{scheme}://{rest}");
    }
    // Origin = scheme://host (up to the third '/').
    let origin: String = {
        let after_scheme = base.find("://").map(|i| i + 3).unwrap_or(0);
        let host_end = base[after_scheme..]
            .find('/')
            .map(|i| after_scheme + i)
            .unwrap_or(base.len());
        base[..host_end].to_string()
    };
    if let Some(path) = img.strip_prefix('/') {
        format!("{origin}/{path}")
    } else {
        format!("{origin}/{img}")
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Restore/save the window's size & position across launches.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(ActiveBackup::default())
        .manage(SessionKeys::default())
        .manage(ImportCancel::default())
        .manage(ImportGate::default())
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
        .register_asynchronous_uri_scheme_protocol(
            "traceloupe-note-image",
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let path = request.uri().path().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(note_image_protocol_response(&app, &path));
                });
            },
        )
        .invoke_handler(tauri::generate_handler![
            list_backups,
            default_backup_root,
            open_full_disk_access_settings,
            fetch_link_preview,
            engine_status,
            engine_info,
            install_engine,
            list_import_modules,
            set_log_level,
            cancel_import,
            import_backup,
            open_backup,
            has_active_backup,
            set_biometric_required,
            app_signing_status,
            reimport_module,
            forget_backup,
            imported_backup_ids,
            list_threads,
            device_info,
            list_calendar_events,
            list_reminders,
            list_workouts,
            workout_route,
            health_daily,
            list_sleep,
            health_summary,
            list_interactions,
            message_kinds,
            count_thread_messages,
            get_thread_message_window,
            thread_message_index,
            recover_attachment_media,
            count_timeline_messages,
            get_timeline_window,
            count_message_ranges,
            message_date_bounds,
            get_range_window,
            open_attachment,
            list_calls,
            list_notes,
            unlock_note,
            list_recordings,
            list_safari_history,
            list_contacts,
            list_installed_apps,
            list_media,
            media_sources,
            count_media,
            count_media_ranges,
            get_media_window,
            count_calls,
            count_call_ranges,
            get_calls_window,
            count_safari,
            count_safari_ranges,
            count_safari_bookmarks,
            count_safari_bookmark_ranges,
            get_safari_bookmarks_window,
            get_safari_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::Resolver;

    #[test]
    fn ip_is_global_rejects_private_and_special_ranges() {
        let g = |s: &str| ip_is_global(s.parse().unwrap());
        // Non-global: loopback, RFC1918, link-local, CGNAT, metadata, unspecified.
        assert!(!g("127.0.0.1"));
        assert!(!g("10.0.0.1"));
        assert!(!g("192.168.1.1"));
        assert!(!g("172.16.0.1"));
        assert!(!g("169.254.169.254")); // link-local / cloud metadata
        assert!(!g("100.64.0.1")); // CGNAT
        assert!(!g("0.0.0.0"));
        assert!(!g("::1"));
        assert!(!g("fe80::1")); // link-local
        assert!(!g("fc00::1")); // unique-local
                                // Global: public v4/v6.
        assert!(g("8.8.8.8"));
        assert!(g("1.1.1.1"));
        assert!(g("2606:4700:4700::1111"));
    }

    #[test]
    fn resolver_rejects_private_literal_and_accepts_public() {
        // Literal IPs need no DNS, so this is hermetic. The resolver is the
        // rebind-proof layer: it must drop private addresses even when handed
        // one directly (the exact address ureq would otherwise dial).
        assert!(PublicOnlyResolver.resolve("127.0.0.1:80").is_err());
        assert!(PublicOnlyResolver.resolve("169.254.169.254:80").is_err());
        assert!(PublicOnlyResolver.resolve("192.168.0.1:443").is_err());

        let ok = PublicOnlyResolver.resolve("8.8.8.8:80").unwrap();
        assert_eq!(ok, vec!["8.8.8.8:80".parse().unwrap()]);
    }
}
