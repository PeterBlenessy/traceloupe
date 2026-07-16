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
struct SessionKeys(Mutex<Option<Arc<BackupDecryptor>>>);

impl SessionKeys {
    fn set(&self, decryptor: Option<Arc<BackupDecryptor>>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = decryptor;
    }
    fn get(&self) -> Option<Arc<BackupDecryptor>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    attachment_id: i64,
) -> Result<(), String> {
    let active_path = active.path()?;
    let decryptor = session.get();
    // Reading + full-file AES-decrypting a large attachment must not run on the
    // main thread (it would freeze the UI); do it on a blocking worker.
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&active_path).map_err(|e| e.to_string())?;
        let (local_path, _filename, _mime, decrypt_key, plain_size) =
            query::attachment_blob(&cache, attachment_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "attachment file is not available".to_string())?;

        // Encrypted backup: decrypt to a persistent temp (0600) beside the cache
        // and open that (the external app reads it after this returns, so it isn't
        // auto-deleted — a re-import/forget clears the dir). Plaintext: open direct.
        let to_open = if let Some(key) = decrypt_key {
            let dec = decryptor.ok_or_else(|| "backup keys are not loaded".to_string())?;
            let ciphertext = std::fs::read(&local_path).map_err(|e| e.to_string())?;
            let size = plain_size.and_then(|s| usize::try_from(s).ok());
            let plain = dec
                .decrypt_bytes(&key, &ciphertext, size)
                .map_err(|e| e.to_string())?;
            let dir = active_path
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
    let Ok(Some((local_path, mime, _thumb, decrypt_key, plain_size))) =
        query::note_image_blob(&cache, id)
    else {
        return not_found();
    };

    let thumbs_dir = cache_path
        .parent()
        .map(|p| p.join("note-thumbs"))
        .unwrap_or_else(|| PathBuf::from("note-thumbs"));

    let rendered = if let Some(key) = decrypt_key {
        let Some(dec) = app.state::<SessionKeys>().get() else {
            return not_found(); // encrypted image but no keys this session
        };
        let out = thumbs_dir.join(format!("note-{id}.decrypted"));
        let Some(src) = decrypt_to_cache(&dec, &key, Path::new(&local_path), plain_size, &out)
        else {
            return not_found();
        };
        media::render(&src, &thumbs_dir, id, true, mime.as_deref())
    } else {
        media::render(
            Path::new(&local_path),
            &thumbs_dir,
            id,
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
        let Some(dec) = app.state::<SessionKeys>().get() else {
            return not_found(); // encrypted attachment but no keys this session
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
        let Some(dec) = app.state::<SessionKeys>().get() else {
            return not_found(); // encrypted item but no keys this session
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            health_summary,
            list_interactions,
            message_kinds,
            count_thread_messages,
            get_thread_message_window,
            count_timeline_messages,
            get_timeline_window,
            count_message_ranges,
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
