//! Thin Tauri command layer over traceloupe-core (docs/architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

mod biometric;
mod logging;
mod media;
mod power;
mod safety_scan_cmd;
mod secret;
mod signing;
mod stream;
mod system_watch;
mod theme;

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
fn reopen_decryptor(
    app: &AppHandle,
    cache_path: &Path,
    backup_id: &str,
) -> Option<Arc<BackupDecryptor>> {
    // Sub-phase timings (#40). CAREFUL READING THESE: two of these phases can
    // block on the USER, not on work — a Keychain item with an ACL makes
    // `secret::get` show a password/"Allow" dialog, and `biometric::gate` waits
    // on Touch ID. Their elapsed time is human think-time and is unbounded, so
    // it must never be read as app latency (a ~19.5 s "key load" observed during
    // #40 was exactly this). Phases that measure real work are labelled
    // "compute"; the two that can wait on a person say so.
    let t = std::time::Instant::now();
    // Fetch the stored password first: no key → plaintext backup → None, and the
    // biometric prompt never fires for a plaintext backup.
    let password = secret::get(backup_id)?;
    logging::debug(
        app,
        format!(
            "reopen_decryptor: Keychain read took {} ms",
            t.elapsed().as_millis()
        ),
    );
    let t = std::time::Instant::now();
    if biometric::gate("Unlock this iPhone backup to access its data").is_err() {
        return None; // user cancelled / auth failed → keys stay locked
    }
    logging::debug(
        app,
        format!(
            "reopen_decryptor: biometric gate took {} ms",
            t.elapsed().as_millis()
        ),
    );
    let t = std::time::Instant::now();
    let cache = CacheDb::open(cache_path).ok()?;
    let source_dir = cache.get_meta("source_dir").ok().flatten()?;
    logging::debug(
        app,
        format!(
            "reopen_decryptor: cache open took {} ms",
            t.elapsed().as_millis()
        ),
    );
    let t = std::time::Instant::now();
    let out = BackupDecryptor::open(Path::new(&source_dir), &password)
        .ok()
        .map(Arc::new);
    logging::debug(
        app,
        format!(
            "reopen_decryptor: keybag + key ladder took {} ms",
            t.elapsed().as_millis()
        ),
    );
    out
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
    match reopen_decryptor(app, active_path, &backup_id) {
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
            // Deliberately still an event, not a Channel (#65): the iLEAPP
            // engine is dormant — imports are fully native — and nothing in the
            // UI calls installEngine or listens to this. Converting a stream
            // with no consumer would be busywork; removing the dormant engine
            // surface is a separate call than this plumbing change.
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

/// The last progress event emitted for the in-flight import, plus which backup it
/// belongs to — or None when no import is running.
///
/// An import runs in the Rust process and survives a webview reload; this
/// snapshot is what lets the frontend re-attach afterwards (#72). Without it a
/// reload showed no progress AND re-clicking Import collided with `ImportGate`,
/// erroring while the original import was still writing the cache.
#[derive(Default)]
struct ImportStatus(std::sync::Arc<Mutex<Option<ImportSnapshot>>>);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportSnapshot {
    backup_id: String,
    event: ImportEvent,
}

/// Emit an import progress event AND record it as the re-attach snapshot.
///
/// Both go through one helper so the snapshot cannot drift from what the UI was
/// last told — a snapshot updated at only some emit sites would be worse than
/// none, because a reloaded UI would then show a stale phase.
fn emit_import(app: &AppHandle, backup_id: &str, event: ImportEvent) {
    if let Some(state) = app.try_state::<ImportStatus>() {
        let mut g = state.0.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(ImportSnapshot {
            backup_id: backup_id.to_string(),
            event: event.clone(),
        });
    }
    IMPORT_PROGRESS.send(event);
}

/// The import progress stream. One producer (the import task), one consumer
/// (`import-provider.tsx`, which fans out through React context) — see
/// [`stream`] for why that makes a Channel the right primitive.
static IMPORT_PROGRESS: stream::ProgressStream<ImportEvent> = stream::ProgressStream::new();

/// Subscribe to import progress. Pairs with `get_import_status`: the snapshot
/// re-attaches a freshly mounted UI to an import already running, and this
/// carries what happens next. Either order works — a progress stream ticks
/// again, so a value crossing the two calls self-corrects on the next update.
#[tauri::command]
fn subscribe_import_progress(channel: tauri::ipc::Channel<ImportEvent>) {
    IMPORT_PROGRESS.subscribe(channel);
}

/// Clear the import snapshot. Called on every exit path — success, failure and
/// cancel — so a reloaded UI never shows an import that already finished.
fn clear_import_status(app: &AppHandle) {
    if let Some(state) = app.try_state::<ImportStatus>() {
        *state.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Re-attach to an in-flight import after the frontend lost its state.
#[tauri::command]
fn get_import_status(status: State<'_, ImportStatus>) -> Option<ImportSnapshot> {
    status.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    // Schema-blind media discovery for app chats. Defaults to ON when the
    // caller says nothing, so an older frontend (or a scripted call) keeps the
    // behaviour the setting describes rather than silently losing media.
    discover_media: Option<bool>,
) -> Result<ImportResult, String> {
    let discover_media = discover_media.unwrap_or(true);
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

    // The progress closure runs on a worker and needs the backup id for the
    // re-attach snapshot (#72), so give it its own copy — and a handle to clear
    // that snapshot once the run is over (the original `app` moves into the
    // closure, as `app_for_passive` already accounts for).
    let progress_backup_id = backup_id.clone();
    let app_for_status = app.clone();
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

    // Held for the post-import Passive Check (the pipeline closure moves `app`).
    let app_for_passive = app.clone();
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
            discover_media,
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
                    emit_import(&app, &progress_backup_id, event);
                }
            },
        )
    })
    .await;

    // The run is over (done, error, or cancelled) — clear the shared token so a
    // later cancel_import can't stop a future import, and free it. Same for the
    // re-attach snapshot (#72): this point covers ALL three outcomes, so a
    // reloaded UI can never show an import that has already finished.
    *import_cancel.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    clear_import_status(&app_for_status);

    let outcome = result
        .map_err(|e| format!("import task panicked: {e}"))?
        .map_err(|e| e.to_string())?;

    // Newly imported backup becomes the active one for browsing.
    active.set(outcome.cache_path.clone());
    // The cache was just rebuilt, so re-stamp the user's stars from the durable
    // per-backup file onto it (they'd otherwise all reset to 0).
    if let Ok(cache) = CacheDb::open(&outcome.cache_path) {
        apply_favorites(&cache, &outcome.cache_path);
        apply_marks(&cache, &outcome.cache_path);
    }
    repair_stranded_safety_scans(&app_for_passive, &outcome.cache_path);
    relink_findings_to_cache(&app_for_passive, &outcome.cache_path);

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

    // Passive Check: if the user consented, run the (apps-only by default)
    // detection pass over the just-built cache so a fresh import surfaces
    // findings without a separate scan. Best-effort — never fail an import
    // because detection hiccupped.
    run_passive_check_if_consented(&app_for_passive, &outcome.cache_path).await;

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

/// Run the Passive Check against `cache_path` when the user has consented and
/// enabled it. Used both after an import and by `run_passive_check_now` (the
/// first-launch flow, against an already-imported cache). Best-effort: logs
/// and returns on any error.
async fn run_passive_check_if_consented(app: &AppHandle, cache_path: &Path) {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    let settings = DetectionSettings::load(&data_dir).unwrap_or_default();
    if !settings.passive_active() {
        return;
    }
    let scan_kind = ScanKind::Passive;
    let modules: Vec<&'static str> = match settings.passive_scope {
        traceloupe_core::detection_settings::PassiveScope::AppsOnly => vec!["apps"],
        traceloupe_core::detection_settings::PassiveScope::Full => analyzer::MODULES.to_vec(),
    };
    let snapshot_dir = active_indicators_dir(app);
    let custom_dir = settings.custom_indicator_dir.clone().map(PathBuf::from);
    let cp = cache_path.to_path_buf();
    let app2 = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let db = CacheDb::open(&cp)?;
        let (set, info) = indicators::load_indicators(&snapshot_dir, custom_dir.as_deref())?;
        let feeds_json = serde_json::to_string(&info.feeds).unwrap_or_else(|_| "[]".into());
        analyzer::run_scan(
            &db,
            &set,
            scan_kind,
            &modules,
            // Passive Check does no Tier-B artifact extraction.
            analyzer::ScanInputs::default(),
            &feeds_json,
            info.generated_at_unix(),
            &CancelToken::new(),
            |module, index, total| {
                emit_security_progress(
                    &app2,
                    ScanProgress {
                        module: module.to_string(),
                        index,
                        total,
                    },
                );
            },
        )
    })
    .await;
    match outcome {
        Ok(Ok(o)) => {
            if o.findings > 0 {
                logging::warn(
                    app,
                    format!(
                        "\u{26a0} Passive Check flagged {} item(s) — open Security to review",
                        o.findings
                    ),
                );
            } else {
                logging::info(
                    app,
                    "\u{2713} Passive Check: no known indicators matched".to_string(),
                );
            }
        }
        Ok(Err(e)) => logging::warn(app, format!("Passive Check skipped: {e}")),
        Err(e) => logging::warn(app, format!("Passive Check skipped: {e}")),
    }
}

/// Run the Passive Check now against the active backup (the first-launch
/// consent flow: the user just granted consent and we scan the already-imported
/// cache without waiting for a re-import). No-op if consent isn't granted.
#[tauri::command]
async fn run_passive_check_now(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
) -> Result<Option<ScanSummary>, String> {
    let Ok(cache_path) = active.path() else {
        return Ok(None);
    };
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if !DetectionSettings::load(&data_dir)
        .unwrap_or_default()
        .passive_active()
    {
        return Ok(None);
    }
    run_passive_check_if_consented(&app, &cache_path).await;
    let path = cache_path.clone();
    let run = tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path)?;
        query::latest_scan_run(&cache)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(run.map(|run_id| ScanSummary {
        run_id,
        findings: 0,
        cancelled: false,
    }))
}

/// Open a previously-imported backup's cache (by id) for browsing, without
/// re-running the engine. Returns false if no cache exists for that id yet.
#[tauri::command]
async fn open_backup(app: AppHandle, backup_id: String) -> bool {
    if !valid_backup_id(&backup_id) {
        return false;
    }
    // Per-phase timings for the open path (#40). Opening felt slow (~4 s) with no
    // way to see where it went; these debug lines make each phase measurable in
    // the dev console instead of guessed at. Kept permanently — a regression here
    // is a UX regression, and the cost is one Instant per open.
    let t_open = std::time::Instant::now();
    let mut t_phase = t_open;
    macro_rules! phase {
        ($name:expr) => {{
            let ms = t_phase.elapsed().as_millis();
            t_phase = std::time::Instant::now();
            logging::debug(&app, format!("open_backup: {} took {} ms", $name, ms));
        }};
    }
    // Serialize against an in-flight import's atomic cache swap, so we never point
    // ActiveBackup at a cache mid-write. Fetched from `app` (not a `State` param)
    // so this command can keep its plain `bool` return.
    let gate = app.state::<ImportGate>();
    let _gate = gate.0.lock().await;
    phase!("import-gate wait");
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
    phase!("clear previous decrypted temps");
    // Point the session at the new backup and hand control back. Browsing only
    // needs the CACHE — keys are for media and native re-imports — so the key
    // rebuild happens in the BACKGROUND (below) instead of blocking the open.
    //
    // Measured before this change: the rebuild was the ENTIRE open cost (~19.5 s
    // observed; every other phase ≤1 ms). Crucially that time is mostly the USER
    // — a Keychain ACL prompt and/or Touch ID — so it is unbounded and can never
    // be optimised away. All the more reason not to sit on it: the first paint
    // doesn't depend on keys, so the open no longer waits for a human to type.
    //
    // `set(None)` also clears the previous backup's keys and its auth_failed
    // flag, so `ensure_session_decryptor` is free to load this backup's keys on
    // demand if something asks before the warm-up finishes.
    app.state::<SessionKeys>().set(None);
    app.state::<ActiveBackup>().set(cache_path.clone());
    // Stamp the user's persisted stars onto the cache this session will query.
    if let Ok(cache) = CacheDb::open(&cache_path) {
        apply_favorites(&cache, &cache_path);
        apply_marks(&cache, &cache_path);
    }
    repair_stranded_safety_scans(&app, &cache_path);
    phase!("repair stranded safety scans");
    // The macro re-arms `t_phase` for a next phase that doesn't exist here; this
    // read keeps that honest for clippy without an allow() over the whole fn.
    let _ = t_phase;

    // Warm the keys in the background. Goes through `ensure_session_decryptor`,
    // NOT `reopen_decryptor` directly, so it shares the one lock that already
    // serialises rebuilds — a media request arriving mid-warm-up blocks briefly
    // and reuses this result instead of deriving a second time (or firing a
    // second Touch ID prompt).
    let app_warm = app.clone();
    let warm_path = cache_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let loaded = ensure_session_decryptor(&app_warm, &warm_path).is_some();
        logging::debug(
            &app_warm,
            format!(
                "open_backup: background key warm-up took {} ms (loaded={loaded}) \
                 — includes any password/Touch ID dialog, i.e. user time, not app latency",
                started.elapsed().as_millis()
            ),
        );
        // Surface a silent key-load failure: if this backup is encrypted but we
        // have no decryptor, full-resolution photos and native re-imports won't
        // work until the keys load. Point at the likely cause — a cancelled or
        // unavailable Touch ID prompt when biometric unlock is on, otherwise the
        // Keychain-ACL/signing issue (a rebuilt dev binary loses access; see
        // docs/reference/signing.md).
        if !loaded {
            if let Ok(Some(src)) = CacheDb::open(&warm_path).and_then(|c| c.get_meta("source_dir"))
            {
                if discovery::read_backup_info(Path::new(&src)).is_encrypted == Some(true) {
                    let msg = if biometric::is_required() {
                        "Backup is encrypted and Touch ID unlock is on, but its keys weren't unlocked \
                         (Touch ID cancelled/failed, or unavailable on this build). Authenticate when \
                         prompted, or turn off Require Touch ID in Settings."
                    } else {
                        "Backup is encrypted but its keys couldn't be loaded from the Keychain — \
                         full-resolution photos and native re-imports are unavailable. Re-import with \
                         the password, or sign the build with a stable identity (docs/reference/signing.md)."
                    };
                    logging::warn(&app_warm, msg);
                }
            }
        }
    });
    logging::debug(
        &app,
        format!("open_backup: total {} ms", t_open.elapsed().as_millis()),
    );
    true
}

/// Point Safety Scan findings back at their source rows after an import (#96).
///
/// `source_id` is a cache row id, and a re-import renumbers every row — so
/// without this a pre-existing finding shows "the source is no longer available"
/// for content that is still there. Findings carry a content fingerprint exactly
/// so the mapping can be rebuilt, which is what this does; anything that truly is
/// gone gets marked stale instead of pointing somewhere wrong.
///
/// Best-effort: never fail an import over it. No analysis DB (never scanned) is
/// the normal case for a first import.
fn relink_findings_to_cache(app: &AppHandle, cache_path: &Path) {
    let Ok(analysis_path) = safety_scan_cmd::analysis_path(cache_path) else {
        return;
    };
    if !analysis_path.exists() {
        return;
    }
    let result = (|| -> Result<_, String> {
        let cache = CacheDb::open(cache_path).map_err(|e| e.to_string())?;
        let analysis = traceloupe_core::analysis::AnalysisDb::open(&analysis_path)
            .map_err(|e| e.to_string())?;
        traceloupe_core::safety_scan::relink::relink_findings(&cache, &analysis)
            .map_err(|e| e.to_string())
    })();
    match result {
        Ok(o) if o.relinked > 0 || o.stale > 0 => logging::info(
            app,
            format!(
                "Safety Scan: relinked {} finding(s) to the re-imported cache, {} no longer present",
                o.relinked, o.stale
            ),
        ),
        Ok(_) => {}
        Err(e) => logging::warn(app, format!("Safety Scan: finding relink failed — {e}")),
    }
}

/// Mark Safety Scan rows stranded 'running' as 'interrupted' the moment a
/// backup becomes active — this process provably has no scan in flight at that
/// point, so the stored state must not claim one is running. Best-effort: no
/// analysis DB (never scanned) is fine, and a failure only delays the repair
/// to the begin-scan backstop.
fn repair_stranded_safety_scans(app: &AppHandle, cache_path: &Path) {
    let Ok(path) = safety_scan_cmd::analysis_path(cache_path) else {
        return;
    };
    if !path.exists() {
        return;
    }
    match traceloupe_core::analysis::AnalysisDb::open(&path)
        .and_then(|db| db.repair_stranded_scans())
    {
        Ok(n) if n > 0 => logging::info(
            app,
            format!("Safety Scan: marked {n} interrupted scan(s) from a previous session"),
        ),
        Ok(_) => {}
        Err(e) => logging::warn(
            app,
            format!("Safety Scan: stranded-scan repair failed: {e}"),
        ),
    }
}

/// Whether a backup is currently open for browsing.
#[tauri::command]
fn has_active_backup(active: State<'_, ActiveBackup>) -> bool {
    active.path().is_ok()
}

/// Close the open backup: clear the active pointer and drop the session's
/// decryption keys. The on-disk cache is untouched (reopening stays instant);
/// only in-session state is cleared, so the UI returns to the picker with
/// nothing open.
#[tauri::command]
fn close_backup(app: AppHandle) {
    app.state::<ActiveBackup>().clear();
    app.state::<SessionKeys>().set(None);
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
/// signature — see docs/reference/signing.md).
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
    // Register this module as in-flight so a reloaded UI (and the activity pill)
    // can see it; the guard deregisters it however this returns.
    if let Some(state) = app.try_state::<ReimportStatus>() {
        state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(module_id.clone());
    }
    let _status_guard = ReimportStatusGuard {
        app: app.clone(),
        module: module_id.clone(),
    };
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
        let app_k = app.clone();
        let rebuilt =
            tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&app_k, &cp, &bid))
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
    // A partial re-import renumbers that module's rows too, and 'messages' and
    // 'notes' are precisely the two sources findings come from — so the links
    // need rebuilding here as well as after a full import (#96).
    if matches!(module_id.as_str(), "messages" | "notes") {
        relink_findings_to_cache(&app, &cache_path);
    }
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

// ---------------------------------------------------------------------------
// Security Check (spyware/stalkerware indicator scan). See
// docs/plans/spyware-analyzer-prd.md and docs/plans/security-check-m1-plan.md.
// ---------------------------------------------------------------------------

use traceloupe_core::analyzer::{self, ScanKind};
use traceloupe_core::detection_settings::DetectionSettings;
use traceloupe_core::indicators::{self, SnapshotInfo};
use traceloupe_core::manifest::ManifestIndex;

/// Cancel token for the scan currently in flight (mirrors [`ImportCancel`]).
#[derive(Default)]
struct ScanCancel(Mutex<Option<CancelToken>>);

/// Serializes scans so two never write findings to the same cache at once.
#[derive(Default)]
struct ScanGate(tauri::async_runtime::Mutex<()>);

/// Progress payload for the `scan://progress` event.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    module: String,
    index: usize,
    total: usize,
}

/// The last `scan://progress` payload, or None when no security scan is running.
///
/// Security scans run in the Rust process and survive a webview reload; this is
/// what lets the UI re-attach afterwards, and what lets the toolbar's activity
/// pill show a scan the user has navigated away from (#72, #73).
#[derive(Default)]
struct SecurityScanStatus(std::sync::Arc<Mutex<Option<ScanProgress>>>);

/// Emit security-scan progress AND record it as the re-attach snapshot. One
/// helper for all four emit sites, so the snapshot cannot drift from what the UI
/// was last told.
fn emit_security_progress(app: &AppHandle, progress: ScanProgress) {
    if let Some(state) = app.try_state::<SecurityScanStatus>() {
        *state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(progress.clone());
    }
    SECURITY_PROGRESS.send(progress);
}

/// The security-scan progress stream (one producer, one consumer — see
/// [`stream`]).
static SECURITY_PROGRESS: stream::ProgressStream<ScanProgress> = stream::ProgressStream::new();

/// Subscribe to security-scan progress. Paired with `get_security_scan_status`,
/// which re-attaches a reloaded UI to a scan already in flight.
#[tauri::command]
fn subscribe_security_progress(channel: tauri::ipc::Channel<ScanProgress>) {
    SECURITY_PROGRESS.subscribe(channel);
}

/// Clear the security-scan snapshot — every path that ends a scan calls this, so
/// a reloaded UI never shows a scan that already finished.
fn clear_security_status(app: &AppHandle) {
    if let Some(state) = app.try_state::<SecurityScanStatus>() {
        *state.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Re-attach to an in-flight security scan after the frontend lost its state.
#[tauri::command]
fn get_security_scan_status(status: State<'_, SecurityScanStatus>) -> Option<ScanProgress> {
    status.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Which modules are re-importing right now, so the UI can re-attach and the
/// activity pill can list them (#72, #73).
#[derive(Default)]
struct ReimportStatus(std::sync::Arc<Mutex<std::collections::BTreeSet<String>>>);

#[tauri::command]
fn get_reimport_status(status: State<'_, ReimportStatus>) -> Vec<String> {
    status
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// Clears the security-scan snapshot on EVERY exit path — the command has many
/// `?` returns, and a guard covers all of them (plus panics) where scattered
/// clear calls would eventually miss one and strand a finished scan in the UI.
struct SecurityScanStatusGuard(AppHandle);
impl Drop for SecurityScanStatusGuard {
    fn drop(&mut self) {
        clear_security_status(&self.0);
    }
}

/// Same, per re-imported module.
struct ReimportStatusGuard {
    app: AppHandle,
    module: String,
}
impl Drop for ReimportStatusGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<ReimportStatus>() {
            state
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.module);
        }
    }
}

/// Summary returned when a scan finishes.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanSummary {
    run_id: i64,
    findings: usize,
    cancelled: bool,
}

/// The bundled indicator snapshot dir: the packaged app resource, or (dev
/// build) the crate's `resources/indicators`.
fn bundled_indicators_dir(app: &AppHandle) -> PathBuf {
    if let Ok(res) = app.path().resource_dir() {
        let packaged = res.join("indicators");
        if packaged.join("manifest.json").exists() {
            return packaged;
        }
    }
    indicators::bundled_snapshot_dir()
}

/// Where fetched indicator snapshots live (survives re-imports; not the cache).
fn indicators_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|d| d.join("indicators"))
        .map_err(|e| e.to_string())
}

/// The active snapshot dir — a fetched one if present, else the bundle.
fn active_indicators_dir(app: &AppHandle) -> PathBuf {
    let bundled = bundled_indicators_dir(app);
    match app.path().app_data_dir() {
        Ok(data) => indicators::active_snapshot_dir(&data, &bundled),
        Err(_) => bundled,
    }
}

/// `…/caches/<id>/cache.db` → `(backup_id, work_dir)`.
fn backup_layout(cache_path: &Path) -> Result<(String, PathBuf), String> {
    let id_dir = cache_path.parent().ok_or("unexpected cache layout")?;
    let backup_id = id_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("unexpected cache layout")?
        .to_string();
    let data_dir = id_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("unexpected cache layout")?;
    let work_dir = data_dir.join("work").join(&backup_id);
    Ok((backup_id, work_dir))
}

/// Run a Security Check scan over the active backup's cache. `kind` is
/// "explicit" (full modules, may fetch fresh feeds) or "passive" (apps-only by
/// default). Emits `scan://progress`; cancellable via `cancel_scan`.
#[tauri::command]
async fn run_security_scan(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    scan_gate: State<'_, ScanGate>,
    cancel_state: State<'_, ScanCancel>,
    kind: String,
) -> Result<ScanSummary, String> {
    let scan_kind = match kind.as_str() {
        "explicit" => ScanKind::Explicit,
        "passive" => ScanKind::Passive,
        _ => return Err(format!("unknown scan kind '{kind}'")),
    };
    // Snapshot lives for the scan; the guard clears it however this returns.
    let _status_guard = SecurityScanStatusGuard(app.clone());
    let _gate = scan_gate.0.lock().await;
    let cache_path = active.path()?;

    let settings = DetectionSettings::load(&app.path().app_data_dir().map_err(|e| e.to_string())?)
        .unwrap_or_default();

    // Refresh feeds first when the user has opted in (Explicit Scan only).
    let bundled = bundled_indicators_dir(&app);
    if scan_kind == ScanKind::Explicit && settings.may_fetch() {
        if let Ok(dest) = indicators_data_dir(&app) {
            let app2 = app.clone();
            let bundled2 = bundled.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                indicators::fetch_snapshot(&bundled2, &dest, |file, i, n| {
                    emit_security_progress(
                        &app2,
                        ScanProgress {
                            module: format!("updating indicators: {file}"),
                            index: i,
                            total: n,
                        },
                    );
                })
            })
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    let snapshot_dir = active_indicators_dir(&app);

    // For an Explicit Scan, build the manifest file list so the sweep can run.
    // Needs the backup source + (for encrypted) decryption keys, exactly like
    // reimport_module.
    let mut manifest_entries: Option<Vec<(String, String)>> = None;
    let mut tierb_processes: Vec<analyzer::ObservedProcess> = Vec::new();
    let mut tierb_profiles: Vec<analyzer::ObservedProfile> = Vec::new();
    let mut tierb_grants: Vec<analyzer::PermissionGrant> = Vec::new();
    let mut tierb_shortcuts: Vec<analyzer::ObservedShortcut> = Vec::new();
    let mut tierb_webkit: Vec<analyzer::ObservedWebDomain> = Vec::new();
    if scan_kind == ScanKind::Explicit {
        let (backup_id, work_dir) = backup_layout(&cache_path)?;
        let source_dir = {
            let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
            cache.get_meta("source_dir").map_err(|e| e.to_string())?
        };
        if let Some(source_dir) = source_dir {
            let mut decryptor = session.get();
            if decryptor.is_none() {
                let cp = cache_path.clone();
                let bid = backup_id.clone();
                let app_k = app.clone();
                decryptor = tauri::async_runtime::spawn_blocking(move || {
                    reopen_decryptor(&app_k, &cp, &bid)
                })
                .await
                .ok()
                .flatten();
                if let Some(d) = &decryptor {
                    session.set(Some(d.clone()));
                }
            }
            let extracted = tauri::async_runtime::spawn_blocking(move || {
                let idx =
                    ManifestIndex::open(Path::new(&source_dir), decryptor.as_deref(), &work_dir)?;
                let mut out = Vec::new();
                idx.for_each_path(|domain, path| out.push((domain, path)))?;
                // Tier-B process activity: DataUsage.sqlite + OSAnalytics ADDaily.
                // Best-effort — a missing/unreadable file just yields fewer
                // processes, never fails the sweep.
                let mut processes: Vec<analyzer::ObservedProcess> = Vec::new();
                if let Ok(Some(entry)) =
                    idx.find("WirelessDomain", "Library/Databases/DataUsage.sqlite")
                {
                    let dest = work_dir.join(".security-datausage.sqlite");
                    if idx.extract_db(&entry, decryptor.as_deref(), &dest).is_ok() {
                        if let Ok(mut ps) = analyzer::parse_datausage(&dest) {
                            processes.append(&mut ps);
                        }
                        let _ = std::fs::remove_file(&dest);
                    }
                }
                if let Ok(Some(entry)) = idx.find(
                    "HomeDomain",
                    "Library/Preferences/com.apple.osanalytics.addaily.plist",
                ) {
                    if let Ok(bytes) = idx.read_bytes(&entry, decryptor.as_deref()) {
                        if let Ok(mut ps) = analyzer::parse_addaily(&bytes) {
                            processes.append(&mut ps);
                        }
                    }
                }
                // Tier-B configuration profiles: ProfileTruth.plist (installed
                // profiles) + PayloadManifest.plist (hidden set). Best-effort.
                let mut profiles: Vec<analyzer::ObservedProfile> = Vec::new();
                const CP_DOMAIN: &str =
                    "SysSharedContainerDomain-systemgroup.com.apple.configurationprofiles";
                if let Ok(Some(truth_entry)) = idx.find(
                    CP_DOMAIN,
                    "Library/ConfigurationProfiles/ProfileTruth.plist",
                ) {
                    if let Ok(truth) = idx.read_bytes(&truth_entry, decryptor.as_deref()) {
                        let manifest = idx
                            .find(
                                CP_DOMAIN,
                                "Library/ConfigurationProfiles/PayloadManifest.plist",
                            )
                            .ok()
                            .flatten()
                            .and_then(|e| idx.read_bytes(&e, decryptor.as_deref()).ok());
                        if let Ok(mut ps) =
                            analyzer::parse_configuration_profiles(&truth, manifest.as_deref())
                        {
                            profiles.append(&mut ps);
                        }
                    }
                }
                // Tier-B TCC permissions: which apps hold mic/camera/etc. grants.
                let mut grants: Vec<analyzer::PermissionGrant> = Vec::new();
                if let Ok(Some(entry)) = idx.find("HomeDomain", "Library/TCC/TCC.db") {
                    let dest = work_dir.join(".security-tcc.db");
                    if idx.extract_db(&entry, decryptor.as_deref(), &dest).is_ok() {
                        if let Ok(mut gs) = analyzer::parse_tcc(&dest) {
                            grants.append(&mut gs);
                        }
                        let _ = std::fs::remove_file(&dest);
                    }
                }
                // Tier-B Shortcuts: actions can call out to arbitrary URLs.
                let mut shortcuts: Vec<analyzer::ObservedShortcut> = Vec::new();
                if let Ok(Some(entry)) =
                    idx.find("HomeDomain", "Library/Shortcuts/Shortcuts.sqlite")
                {
                    let dest = work_dir.join(".security-shortcuts.sqlite");
                    if idx.extract_db(&entry, decryptor.as_deref(), &dest).is_ok() {
                        if let Ok(mut sc) = analyzer::parse_shortcuts(&dest) {
                            shortcuts.append(&mut sc);
                        }
                        let _ = std::fs::remove_file(&dest);
                    }
                }
                // Tier-B WebKit: domains each app's webview contacted, from the
                // per-app observations.db files (across all app domains).
                let mut webkit: Vec<analyzer::ObservedWebDomain> = Vec::new();
                if let Ok(entries) =
                    idx.find_relative_like("%ResourceLoadStatistics/observations.db")
                {
                    for (i, entry) in entries.iter().enumerate() {
                        let app = entry
                            .domain
                            .split_once('-')
                            .filter(|(p, _)| p.starts_with("AppDomain"))
                            .map(|(_, b)| b.to_string());
                        let dest = work_dir.join(format!(".security-webkit-{i}.db"));
                        if idx.extract_db(entry, decryptor.as_deref(), &dest).is_ok() {
                            if let Ok(domains) = analyzer::parse_webkit_observations(&dest) {
                                for (domain, last_seen) in domains {
                                    webkit.push(analyzer::ObservedWebDomain {
                                        domain,
                                        app: app.clone(),
                                        last_seen,
                                    });
                                }
                            }
                            let _ = std::fs::remove_file(&dest);
                        }
                    }
                }
                Ok::<_, traceloupe_core::Error>((
                    out, processes, profiles, grants, shortcuts, webkit,
                ))
            })
            .await
            .map_err(|e| e.to_string())?;
            match extracted {
                Ok((e, ps, prof, grants, shortcuts, webkit)) => {
                    manifest_entries = Some(e);
                    tierb_processes = ps;
                    tierb_profiles = prof;
                    tierb_grants = grants;
                    tierb_shortcuts = shortcuts;
                    tierb_webkit = webkit;
                }
                Err(e) => logging::warn(
                    &app,
                    format!("Security Check: manifest sweep unavailable: {e}"),
                ),
            }
        }
    }

    let cancel = CancelToken::new();
    *cancel_state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());
    logging::info(
        &app,
        format!("\u{25b6} Security Check ({kind}) started\u{2026}"),
    );
    let started = Instant::now();

    let app_progress = app.clone();
    let cp = cache_path.clone();
    let modules: Vec<&'static str> = scan_kind.default_modules();
    let custom_dir = settings.custom_indicator_dir.clone().map(PathBuf::from);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let db = CacheDb::open(&cp)?;
        let (set, info) = indicators::load_indicators(&snapshot_dir, custom_dir.as_deref())?;
        let feeds_json = serde_json::to_string(&info.feeds).unwrap_or_else(|_| "[]".into());
        let mut sweep = manifest_entries.map(|v| v.into_iter());
        let sweep_ref = sweep
            .as_mut()
            .map(|it| it as &mut dyn Iterator<Item = (String, String)>);
        analyzer::run_scan(
            &db,
            &set,
            scan_kind,
            &modules,
            analyzer::ScanInputs {
                manifest_entries: sweep_ref,
                processes: &tierb_processes,
                profiles: &tierb_profiles,
                grants: &tierb_grants,
                shortcuts: &tierb_shortcuts,
                webkit_domains: &tierb_webkit,
            },
            &feeds_json,
            info.generated_at_unix(),
            &cancel,
            |module, index, total| {
                emit_security_progress(
                    &app_progress,
                    ScanProgress {
                        module: module.to_string(),
                        index,
                        total,
                    },
                );
            },
        )
    })
    .await;

    *cancel_state.0.lock().unwrap_or_else(|e| e.into_inner()) = None;

    let outcome = result.map_err(|e| e.to_string())?.map_err(|e| {
        let e = e.to_string();
        logging::error(&app, format!("\u{2717} Security Check failed: {e}"));
        e
    })?;
    logging::info(
        &app,
        format!(
            "\u{2713} Security Check ({kind}): {} findings in {} ms{}",
            outcome.findings,
            started.elapsed().as_millis(),
            if outcome.cancelled {
                " (cancelled)"
            } else {
                ""
            }
        ),
    );

    Ok(ScanSummary {
        run_id: outcome.run_id,
        findings: outcome.findings,
        cancelled: outcome.cancelled,
    })
}

/// Cancel a scan in flight (no-op if none running).
#[tauri::command]
fn cancel_scan(cancel_state: State<'_, ScanCancel>) {
    if let Some(token) = cancel_state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        token.cancel();
    }
}

#[tauri::command]
async fn list_scan_runs(active: State<'_, ActiveBackup>) -> Result<Vec<query::ScanRun>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path)?;
        query::list_scan_runs(&cache)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// The most recent completed run's id (so the UI can open it by default).
#[tauri::command]
async fn latest_scan_run(active: State<'_, ActiveBackup>) -> Result<Option<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path)?;
        query::latest_scan_run(&cache)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_findings(
    active: State<'_, ActiveBackup>,
    run_id: i64,
    min_severity: Option<String>,
    module: Option<String>,
) -> Result<Vec<query::Finding>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path)?;
        query::list_findings(&cache, run_id, min_severity.as_deref(), module.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Load info about the active indicator snapshot (feed counts + freshness),
/// including any custom indicator folder the user configured.
#[tauri::command]
async fn get_indicator_info(app: AppHandle) -> Result<SnapshotInfo, String> {
    let dir = active_indicators_dir(&app);
    let custom_dir = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|d| DetectionSettings::load(&d).ok())
        .and_then(|s| s.custom_indicator_dir)
        .map(PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || {
        indicators::load_indicators(&dir, custom_dir.as_deref()).map(|(_, info)| info)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Fetch fresh indicator feeds now (user-initiated "Update indicators").
#[tauri::command]
async fn update_indicators(
    app: AppHandle,
    scan_gate: State<'_, ScanGate>,
) -> Result<SnapshotInfo, String> {
    // Refuse while a scan holds the gate: the run loaded its indicators at
    // start, and swapping the feed directory underneath it would leave the
    // run's stamped feed counts describing files that no longer exist.
    let Ok(_gate) = scan_gate.0.try_lock() else {
        return Err("A scan is running — wait for it to finish, then update.".into());
    };
    let bundled = bundled_indicators_dir(&app);
    let dest = indicators_data_dir(&app)?;
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        indicators::fetch_snapshot(&bundled, &dest, |file, i, n| {
            emit_security_progress(
                &app2,
                ScanProgress {
                    module: format!("updating indicators: {file}"),
                    index: i,
                    total: n,
                },
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Write a CSV report of a scan run to `path` (chosen via a save dialog on the
/// frontend). Returns the number of bytes written.
#[tauri::command]
async fn export_scan_report(
    active: State<'_, ActiveBackup>,
    run_id: i64,
    path: String,
) -> Result<u64, String> {
    let cache_path = active.path()?;
    let version = env!("CARGO_PKG_VERSION").to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        let csv =
            analyzer::export_report_csv(&cache, run_id, &version).map_err(|e| e.to_string())?;
        std::fs::write(&path, &csv).map_err(|e| format!("writing {path}: {e}"))?;
        Ok::<u64, String>(csv.len() as u64)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_detection_settings(app: AppHandle) -> Result<DetectionSettings, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    DetectionSettings::load(&data).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_detection_settings(app: AppHandle, settings: DetectionSettings) -> Result<(), String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    settings.save(&data).map_err(|e| e.to_string())
}

// --- Opt-in shortened-URL de-shortener (ADR 0001 exception) ----------------
//
// Resolving a shortened link contacts a remote host with a URL from the backup
// — the sole sanctioned exception to "nothing leaves the machine". It is a
// deliberate, per-link, user-approved action (never automatic, never during a
// Passive Check). Resolution only ever connects to allowlisted shortener hosts
// and reveals the destination from the redirect `Location` WITHOUT connecting
// to it, so the final (possibly attacker-controlled) target is never contacted.

const DESHORTEN_META_KEY: &str = "security_deshorten_auto_approve";

/// Whether the user has opted out of the per-use approval prompt *for this
/// backup* (stored in the backup's own cache; never global). Resets on
/// re-import and clears when the backup is forgotten.
#[tauri::command]
async fn deshorten_auto_approve_get(active: State<'_, ActiveBackup>) -> Result<bool, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path)?;
        Ok::<bool, traceloupe_core::Error>(
            cache.get_meta(DESHORTEN_META_KEY)?.as_deref() == Some("1"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn deshorten_auto_approve_set(
    active: State<'_, ActiveBackup>,
    enabled: bool,
) -> Result<(), String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path)?;
        cache.set_meta(DESHORTEN_META_KEY, if enabled { "1" } else { "0" })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Find known shortener URLs in text (a finding's context/value), so the UI can
/// offer to expand them. Pure and local — no network.
#[tauri::command]
fn find_shortener_urls(text: String) -> Vec<String> {
    traceloupe_core::shorteners::find_shortener_urls(&text)
}

/// Reveal a shortened URL's destination. The input must be a known shortener;
/// only shortener hosts are ever contacted, and the revealed target is read
/// from the redirect Location without being visited. SSRF-guarded by the same
/// `PublicOnlyResolver` as link previews.
#[tauri::command]
async fn expand_short_url(url: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || resolve_short_url(&url))
        .await
        .map_err(|e| e.to_string())?
}

fn resolve_short_url(input: &str) -> Result<String, String> {
    use traceloupe_core::shorteners::is_shortener_host;
    // Refuse anything that isn't a known shortener — we never fetch arbitrary
    // hosts, only the allowlisted resolvers the user approved.
    let start_host = url_host(input).ok_or("malformed URL")?;
    if !is_shortener_host(&start_host) {
        return Err("not a recognized shortened link".into());
    }
    let agent = ureq::builder()
        .redirects(0)
        .resolver(PublicOnlyResolver)
        .timeout(std::time::Duration::from_secs(8))
        .build();

    let mut current = input.to_string();
    for _hop in 0..6 {
        let lower = current.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err("unsupported URL scheme".into());
        }
        let host = url_host(&current).ok_or("malformed URL")?;
        // Only ever contact shortener hosts; a non-shortener is the revealed
        // destination and must be returned without a request.
        if !is_shortener_host(&host) {
            return Ok(current);
        }
        if !host_is_public(&host) {
            return Err("refusing to fetch a private or loopback host".into());
        }
        let resp = match agent
            .get(&current)
            .set("User-Agent", "Mozilla/5.0 TraceLoupe/deshorten")
            .call()
        {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) if (300..400).contains(&code) => r,
            Err(e) => return Err(format!("request failed: {e}")),
        };
        if (300..400).contains(&resp.status()) {
            let loc = resp.header("Location").ok_or("redirect without Location")?;
            let next = absolutize(&current, loc);
            // If it points off the shortener, that's the answer — return it
            // without visiting. If it's another shortener, follow that hop.
            let next_host = url_host(&next).ok_or("malformed redirect target")?;
            if !is_shortener_host(&next_host) {
                return Ok(next);
            }
            current = next;
            continue;
        }
        // A shortener that returns 2xx (no redirect) reveals nothing to follow.
        return Err("the link did not redirect to a destination".into());
    }
    Err("too many redirects".into())
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

/// Why each module ended up empty or not, from the import that built this
/// cache (#288).
///
/// Read by the views' empty states, so "we could not read your call history"
/// is distinguishable from "your backup contains none" long after the toast
/// that reported it at import time has gone.
#[tauri::command]
async fn module_status(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<traceloupe_core::normalize::ModuleStatus>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        cache.module_status().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Prepare an app's database for raw inspection: locate it, decrypt it out to a
/// temp file, and hand back the path.
///
/// The extracted copy is a plaintext copy of evidence, so it lives in the
/// backup's work dir (already treated as sensitive scratch) and is replaced
/// rather than accumulated — one file per database, reused across paging and
/// searching so a scroll does not decrypt the same store fifty times.
async fn stage_raw_db(
    app: &AppHandle,
    active: &State<'_, ActiveBackup>,
    session: &State<'_, SessionKeys>,
    relative_path: String,
) -> Result<std::path::PathBuf, String> {
    let cache_path = active.path()?;
    let (backup_id, work_dir) = backup_layout(&cache_path)?;
    let source_dir = {
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        cache
            .get_meta("source_dir")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "this backup's source folder is not recorded".to_string())?
    };
    let mut decryptor = session.get();
    if decryptor.is_none() {
        let cp = cache_path.clone();
        let bid = backup_id.clone();
        let app_k = app.clone();
        decryptor =
            tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&app_k, &cp, &bid))
                .await
                .ok()
                .flatten();
        if let Some(d) = &decryptor {
            session.set(Some(d.clone()));
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let idx = ManifestIndex::open(Path::new(&source_dir), decryptor.as_deref(), &work_dir)
            .map_err(|e| e.to_string())?;
        let entry = idx
            .find_relative_like(&relative_path)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|e| e.relative_path == relative_path)
            .ok_or_else(|| format!("{relative_path} is not in this backup"))?;
        // Named from a hash of the path so paging reuses one file per database
        // instead of leaving a trail of decrypted copies.
        let mut h: u64 = 1469598103934665603;
        for b in relative_path.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        let dest = work_dir.join(format!(".rawdb-{h:016x}.sqlite"));
        if !dest.exists() {
            idx.extract_db(&entry, decryptor.as_deref(), &dest)
                .map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(dest)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Which slice of the timeline to read.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineQuery {
    kinds: Vec<String>,
    sources: Vec<String>,
    lo: Option<i64>,
    hi: Option<i64>,
    search: Option<String>,
    offset: i64,
    limit: i64,
    desc: bool,
}

/// How many timeline events match, for the virtualizer.
#[tauri::command]
async fn count_timeline_events(
    active: State<'_, ActiveBackup>,
    args: TimelineQuery,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        traceloupe_core::timeline::count_events(
            &cache,
            &traceloupe_core::timeline::EventFilter {
                kinds: &args.kinds,
                sources: &args.sources,
                lo: args.lo,
                hi: args.hi,
                search: args.search.as_deref(),
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A window of the timeline.
#[tauri::command]
async fn get_timeline_events(
    active: State<'_, ActiveBackup>,
    args: TimelineQuery,
) -> Result<Vec<traceloupe_core::timeline::TimelineEvent>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        traceloupe_core::timeline::get_events(
            &cache,
            &traceloupe_core::timeline::EventFilter {
                kinds: &args.kinds,
                sources: &args.sources,
                lo: args.lo,
                hi: args.hi,
                search: args.search.as_deref(),
            },
            args.offset,
            args.limit,
            args.desc,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Kind and source counts, so the filter offers only what this backup holds.
#[tauri::command]
async fn timeline_facets(active: State<'_, ActiveBackup>) -> Result<TimelineFacets, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        let (kinds, sources) =
            traceloupe_core::timeline::facets(&cache).map_err(|e| e.to_string())?;
        Ok(TimelineFacets { kinds, sources })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineFacets {
    kinds: Vec<traceloupe_core::timeline::TimelineFacet>,
    sources: Vec<traceloupe_core::timeline::TimelineFacet>,
}

/// Every SQLite database this app has in the backup.
#[tauri::command]
async fn raw_databases(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    bundle_id: String,
) -> Result<Vec<traceloupe_core::rawdb::RawDatabase>, String> {
    let cache_path = active.path()?;
    let (backup_id, work_dir) = backup_layout(&cache_path)?;
    let source_dir = {
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        cache
            .get_meta("source_dir")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "this backup's source folder is not recorded".to_string())?
    };
    let mut decryptor = session.get();
    if decryptor.is_none() {
        let cp = cache_path.clone();
        let bid = backup_id.clone();
        let app_k = app.clone();
        decryptor =
            tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&app_k, &cp, &bid))
                .await
                .ok()
                .flatten();
        if let Some(d) = &decryptor {
            session.set(Some(d.clone()));
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let idx = ManifestIndex::open(Path::new(&source_dir), decryptor.as_deref(), &work_dir)
            .map_err(|e| e.to_string())?;
        traceloupe_core::rawdb::databases_for_app(&idx, &bundle_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The tables in one of those databases, with row counts.
#[tauri::command]
async fn raw_tables(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    relative_path: String,
) -> Result<Vec<traceloupe_core::rawdb::RawTable>, String> {
    let db = stage_raw_db(&app, &active, &session, relative_path).await?;
    tauri::async_runtime::spawn_blocking(move || {
        traceloupe_core::rawdb::tables(&db).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Which page of which table to read. One struct rather than five loose
/// parameters, so the command reads the way the caller writes it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRowsQuery {
    relative_path: String,
    table: String,
    offset: i64,
    limit: i64,
    search: Option<String>,
}

/// A page of one table, optionally filtered.
#[tauri::command]
async fn raw_rows(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    args: RawRowsQuery,
) -> Result<traceloupe_core::rawdb::RawRows, String> {
    let RawRowsQuery {
        relative_path,
        table,
        offset,
        limit,
        search,
    } = args;
    let db = stage_raw_db(&app, &active, &session, relative_path).await?;
    tauri::async_runtime::spawn_blocking(move || {
        traceloupe_core::rawdb::rows(&db, &table, offset, limit, search.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One home-dashboard tile. Carries its own label, route and icon, so the view
/// renders whatever arrives without knowing which modules exist (#157).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModuleMetricDto {
    id: String,
    label: String,
    route: String,
    icon: String,
    count: i64,
    first_at: Option<i64>,
    last_at: Option<i64>,
    series: Vec<i64>,
    facets: Vec<FacetDto>,
}

/// What is inside a tile — a service, a channel, a Health category.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FacetDto {
    label: String,
    count: i64,
}

/// The home dashboard's tiles: every kind of data this backup actually yielded,
/// with its count, the period it covers and a sparkline of when it clusters.
///
/// Deliberately NOT part of `device_info`: #40 measures "the backup is open" as
/// the moment the home view has its data, and these aggregates would land on
/// exactly that number. The view paints its device header first and fills the
/// tiles in behind it.
#[tauri::command]
async fn module_metrics(active: State<'_, ActiveBackup>) -> Result<Vec<ModuleMetricDto>, String> {
    let path = active.path()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(i64::MAX);
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        let metrics = traceloupe_core::dashboard::module_metrics(cache.conn(), now)
            .map_err(|e| e.to_string())?;
        Ok(metrics
            .into_iter()
            .map(|m| ModuleMetricDto {
                id: m.id,
                label: m.label,
                route: m.route,
                icon: m.icon,
                count: m.count,
                first_at: m.first_at,
                last_at: m.last_at,
                series: m.series,
                facets: m
                    .facets
                    .into_iter()
                    .map(|f| FacetDto {
                        label: f.label,
                        count: f.count,
                    })
                    .collect(),
            })
            .collect())
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
    search: Option<String>,
    unsafe_only: Option<bool>,
    ranges: Vec<query::TimeRange>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_messages(
            &cache,
            thread_id,
            kind.as_deref(),
            search.as_deref(),
            unsafe_only.unwrap_or(false),
            &ranges,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: thread + window + kind + dir + search + mark.
async fn get_thread_message_window(
    active: State<'_, ActiveBackup>,
    thread_id: i64,
    offset: i64,
    limit: i64,
    kind: Option<String>,
    desc: bool,
    search: Option<String>,
    unsafe_only: Option<bool>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<Message>, String> {
    // Async + spawn_blocking: a synchronous command runs on the main thread and
    // would freeze the whole native UI. Only the requested window is read, so
    // the frontend can lazily load a thread as it scrolls.
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_message_window(
            &cache,
            thread_id,
            offset,
            limit,
            kind.as_deref(),
            desc,
            search.as_deref(),
            unsafe_only.unwrap_or(false),
            &ranges,
        )
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
    unsafe_only: Option<bool>,
    ranges: Vec<query::TimeRange>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_all_messages(
            &cache,
            service.as_deref(),
            search.as_deref(),
            kind.as_deref(),
            unsafe_only.unwrap_or(false),
            &ranges,
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
    unsafe_only: Option<bool>,
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
            unsafe_only.unwrap_or(false),
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
async fn media_date_bounds(active: State<'_, ActiveBackup>) -> Result<Option<(i64, i64)>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::media_date_bounds(&cache).map_err(|e| e.to_string())
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
    unsafe_only: Option<bool>,
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
            unsafe_only.unwrap_or(false),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_note_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_note_ranges(&cache, &ranges).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: time range + service + search + paging + dir.
async fn get_range_window(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
    offset: i64,
    limit: i64,
    service: Option<String>,
    search: Option<String>,
    kind: Option<String>,
    desc: bool,
    unsafe_only: Option<bool>,
) -> Result<Vec<TimelineMessage>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_range_window(
            &cache,
            &ranges,
            offset,
            limit,
            service.as_deref(),
            search.as_deref(),
            kind.as_deref(),
            desc,
            unsafe_only.unwrap_or(false),
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

/// The plaintext bytes of one gallery item, plus its real filename. Decrypts an
/// encrypted backup's blob on demand (same path the media protocol uses); errors
/// with a readable message when the keys aren't loaded.
fn media_plain_bytes(
    app: &AppHandle,
    active_path: &Path,
    id: i64,
) -> Result<(Vec<u8>, String, Option<String>, PathBuf), String> {
    let cache = CacheDb::open(active_path).map_err(|e| e.to_string())?;
    let (local_path, mime, _thumb, decrypt_key, plain_size) = query::media_blob(&cache, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "this item's file is not available in the backup".to_string())?;
    let plain = if let Some(key) = decrypt_key {
        let dec = ensure_session_decryptor(app, active_path)
            .ok_or_else(|| "backup keys are not loaded (unlock the backup first)".to_string())?;
        let ciphertext = std::fs::read(&local_path).map_err(|e| e.to_string())?;
        let size = plain_size.and_then(|s| usize::try_from(s).ok());
        dec.decrypt_bytes(&key, &ciphertext, size)
            .map_err(|e| e.to_string())?
    } else {
        std::fs::read(&local_path).map_err(|e| e.to_string())?
    };
    let filename = local_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("item")
        .to_string();
    Ok((plain, filename, mime, PathBuf::from(&local_path)))
}

/// Save a photo/video to a user-chosen path. `as_jpeg` transcodes an image to a
/// full-resolution JPEG (for HEIC → something every viewer opens); otherwise the
/// ORIGINAL bytes are written, byte-for-byte, which is what a forensic export
/// wants.
#[tauri::command]
async fn save_media(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    id: i64,
    path: String,
    as_jpeg: bool,
) -> Result<(), String> {
    let active_path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let (plain, _filename, mime, orig_path) = media_plain_bytes(&app, &active_path, id)?;
        let bytes = if as_jpeg {
            // Decrypted temp so sips can read it; the RAII guard removes it.
            let thumbs = active_path
                .parent()
                .map(|p| p.join("thumbs"))
                .unwrap_or_else(|| PathBuf::from("thumbs"));
            std::fs::create_dir_all(&thumbs).map_err(|e| e.to_string())?;
            let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Keep the original extension so sips recognises the input format.
            let ext = orig_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin");
            let tmp = thumbs.join(format!("save-{id}.{seq}.{ext}"));
            write_private(&tmp, &plain).map_err(|e| e.to_string())?;
            let _guard = TempPath(tmp.clone());
            media::render_full_jpeg(&tmp, &thumbs, id).ok_or_else(|| {
                format!(
                    "couldn't convert this {} to JPEG",
                    mime.as_deref().unwrap_or("image")
                )
            })?
        } else {
            plain
        };
        std::fs::write(&path, &bytes).map_err(|e| format!("writing {path}: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Write a photo/video's ORIGINAL bytes to a temp under the cache dir and reveal
/// it in Finder (`open -R`), so the user can drag/copy it into any folder. The
/// bytes carry the item's real filename (hence extension), and the temp is
/// cleared on backup close/switch like the other decrypted exports.
#[tauri::command]
async fn reveal_media(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    id: i64,
) -> Result<(), String> {
    let active_path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let (plain, filename, _mime, _orig) = media_plain_bytes(&app, &active_path, id)?;
        let dir = active_path
            .parent()
            .map(|p| p.join("att-open"))
            .ok_or_else(|| "unexpected cache layout".to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let base = filename.rsplit(['/', '\\']).next().unwrap_or("item");
        let dest = dir.join(format!("{id}-{base}"));
        write_private(&dest, &plain).map_err(|e| e.to_string())?;
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(&dest)
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
async fn list_cycle(active: State<'_, ActiveBackup>) -> Result<Vec<query::CycleEntry>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_cycle(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_health_achievements(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::HealthAchievement>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_health_achievements(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_health_timezones(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::HealthTimezone>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_health_timezones(&cache).map_err(|e| e.to_string())
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

/// Whether the stored artifact rows came from the module set installed now.
///
/// The view needs this to avoid claiming a backup "contained none" when the
/// truth is that no module has run against it — which is what a cache imported
/// before the modules existed looks like.
#[tauri::command]
async fn artifacts_extraction_state(
    active: State<'_, ActiveBackup>,
) -> Result<traceloupe_core::artifacts::ExtractionState, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        traceloupe_core::artifacts::extraction_state(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Run the artifact modules against the already-open backup.
///
/// Deliberately explicit rather than automatic on view open: extraction needs
/// the decryptor, and rebuilding that can block on a Touch ID prompt. A prompt
/// must not appear because someone clicked a sidebar entry.
#[tauri::command]
async fn extract_artifacts(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    gate: State<'_, ImportGate>,
) -> Result<Vec<String>, String> {
    let _gate = gate.0.lock().await;
    let cache_path = active.path()?;
    let id_dir = cache_path
        .parent()
        .ok_or_else(|| "unexpected cache layout".to_string())?;
    let backup_id = id_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "unexpected cache layout".to_string())?
        .to_string();
    let data_dir = id_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "unexpected cache layout".to_string())?;
    let work_dir = data_dir.join("work").join(&backup_id);

    let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
    let source_dir = cache
        .get_meta("source_dir")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "this backup's source path isn't recorded; re-import fully once".to_string()
        })?;
    drop(cache);

    // Same key-rebuild path as a single-module re-import: the session may not
    // hold the keys, and reopen_decryptor can block on Touch ID, so it runs off
    // the async executor.
    let mut decryptor = session.get();
    if decryptor.is_none() {
        let cp = cache_path.clone();
        let bid = backup_id.clone();
        let app_k = app.clone();
        let rebuilt =
            tauri::async_runtime::spawn_blocking(move || reopen_decryptor(&app_k, &cp, &bid))
                .await
                .ok()
                .flatten();
        if let Some(d) = rebuilt {
            session.set(Some(d.clone()));
            decryptor = Some(d);
        }
    }

    logging::info(&app, "\u{25b6} Extracting artifacts\u{2026}".to_string());
    let src = PathBuf::from(&source_dir);
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        Ok(import::extract_artifacts_now(
            &src,
            decryptor.as_deref(),
            &cache,
            &work_dir,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Which artifacts this backup yielded, and their shape. The UI renders
/// whatever arrives — it knows no artifact by name, which is the whole point of
/// the declarative modules.
#[tauri::command]
async fn list_artifacts(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<traceloupe_core::artifacts::ArtifactSummary>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        traceloupe_core::artifacts::list_artifacts(cache.conn()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A page of one artifact's rows.
#[tauri::command]
async fn get_artifact_rows(
    active: State<'_, ActiveBackup>,
    artifact_id: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<traceloupe_core::artifacts::ArtifactRow>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        traceloupe_core::artifacts::read_rows(cache.conn(), &artifact_id, offset, limit)
            .map_err(|e| e.to_string())
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
async fn list_installed_apps(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::InstalledApp>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_installed_apps(&cache).map_err(|e| e.to_string())
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
fn safari_search_sort(field: &str, desc: bool) -> query::Sort {
    let col = match field {
        "term" => "term COLLATE NOCASE",
        "engine" => "engine COLLATE NOCASE",
        _ => "searched_at",
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
    sources: Vec<String>,
    ranges: Vec<query::TimeRange>,
    search: Option<String>,
    favorites_only: bool,
    hidden_only: bool,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_media(
            &cache,
            &sources,
            &ranges,
            search.as_deref(),
            favorites_only,
            hidden_only,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_media_ranges(
    active: State<'_, ActiveBackup>,
    sources: Vec<String>,
    ranges: Vec<query::TimeRange>,
    search: Option<String>,
    favorites_only: bool,
    hidden_only: bool,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_media_ranges(
            &cache,
            &sources,
            &ranges,
            search.as_deref(),
            favorites_only,
            hidden_only,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: source + time range + search + paging + sort + favorites.
async fn get_media_window(
    active: State<'_, ActiveBackup>,
    sources: Vec<String>,
    ranges: Vec<query::TimeRange>,
    search: Option<String>,
    offset: i64,
    limit: i64,
    sort_by: String,
    desc: bool,
    favorites_only: bool,
    hidden_only: bool,
) -> Result<Vec<MediaItem>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_media_window(
            &cache,
            &sources,
            &ranges,
            search.as_deref(),
            offset,
            limit,
            media_sort(&sort_by, desc),
            favorites_only,
            hidden_only,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Path to a backup's durable star file — a sibling of the cache DB, so a
/// re-import (which rebuilds the cache) leaves it untouched.
/// Where the person's own marks live, durably.
///
/// Beside the cache rather than inside it: the cache is deleted and rebuilt by
/// every import, and a mark that only lived there would vanish at the one moment
/// someone most wants it back.
fn marks_file(cache_path: &Path) -> PathBuf {
    cache_path.with_file_name("marks.json")
}

fn read_marks(cache_path: &Path) -> Vec<traceloupe_core::marks::Mark> {
    let mut marks: Vec<traceloupe_core::marks::Mark> = std::fs::read(marks_file(cache_path))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // Carry the photo stars written before marks existed. They were a bare list
    // of relative paths; the same value is a media key, so they migrate as-is
    // rather than being lost to a rename.
    for path in read_favorites(cache_path) {
        let m = traceloupe_core::marks::Mark {
            kind: "media".into(),
            key: path,
        };
        if !marks.contains(&m) {
            marks.push(m);
        }
    }
    marks
}

fn write_marks(cache_path: &Path, marks: &[traceloupe_core::marks::Mark]) {
    if let Ok(json) = serde_json::to_vec(marks) {
        let _ = std::fs::write(marks_file(cache_path), json);
    }
}

/// Re-apply the persisted marks onto a freshly opened/imported cache.
fn apply_marks(cache: &CacheDb, cache_path: &Path) {
    let _ = traceloupe_core::marks::apply(cache, &read_marks(cache_path));
}

/// Mark or unmark one item as unsafe.
#[tauri::command]
async fn set_item_mark(
    active: State<'_, ActiveBackup>,
    kind: String,
    id: i64,
    marked: bool,
) -> Result<(), String> {
    let path = active.path()?;
    let kind = traceloupe_core::marks::MarkKind::parse(&kind)
        .ok_or_else(|| format!("unknown mark kind '{kind}'"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        if traceloupe_core::marks::set(&cache, kind, id, marked)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Ok(()); // no such row
        }
        // Persist the whole set: cheap at this size, and it keeps the file a
        // straight mirror of the table rather than a log that can drift.
        let all = traceloupe_core::marks::all(&cache).map_err(|e| e.to_string())?;
        write_marks(&path, &all);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The row ids marked in this backup, for badging rows without a column on
/// every query.
#[tauri::command]
async fn marked_ids(active: State<'_, ActiveBackup>, kind: String) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    let kind = traceloupe_core::marks::MarkKind::parse(&kind)
        .ok_or_else(|| format!("unknown mark kind '{kind}'"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        traceloupe_core::marks::marked_ids(&cache, kind).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The conversations with at least one message in the period.
#[tauri::command]
async fn threads_in_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::threads_in_ranges(&cache, &ranges).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How many items of each kind are marked, for the filter badges.
#[tauri::command]
async fn mark_counts(active: State<'_, ActiveBackup>) -> Result<MarkCounts, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        use traceloupe_core::marks::MarkKind;
        Ok(MarkCounts {
            media: traceloupe_core::marks::count(&cache, MarkKind::Media).unwrap_or(0),
            message: traceloupe_core::marks::count(&cache, MarkKind::Message).unwrap_or(0),
            contact: traceloupe_core::marks::count(&cache, MarkKind::Contact).unwrap_or(0),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MarkCounts {
    media: i64,
    message: i64,
    contact: i64,
}

fn favorites_file(cache_path: &Path) -> PathBuf {
    cache_path.with_file_name("favorites.json")
}

fn read_favorites(cache_path: &Path) -> Vec<String> {
    std::fs::read(favorites_file(cache_path))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
        .unwrap_or_default()
}

/// Re-apply the persisted stars onto a freshly opened/imported cache. Best
/// effort: a missing or unreadable file just means "no stars yet."
fn apply_favorites(cache: &CacheDb, cache_path: &Path) {
    let paths = read_favorites(cache_path);
    let _ = query::apply_user_favorites(cache, &paths);
}

#[tauri::command]
async fn set_media_favorite(
    active: State<'_, ActiveBackup>,
    id: i64,
    favorite: bool,
) -> Result<(), String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        // Flip the cache column and learn the row's stable relative_path.
        let Some(rel) =
            query::set_user_favorite(&cache, id, favorite).map_err(|e| e.to_string())?
        else {
            return Ok(()); // no such media row
        };
        // Persist to the durable per-backup file so the star survives re-import.
        let mut paths = read_favorites(&path);
        if favorite {
            if !paths.contains(&rel) {
                paths.push(rel);
            }
        } else {
            paths.retain(|p| p != &rel);
        }
        let json = serde_json::to_vec_pretty(&paths).map_err(|e| e.to_string())?;
        write_private(&favorites_file(&path), &json).map_err(|e| e.to_string())?;
        Ok(())
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
    // Addresses whose CONTACT NAME matched the search, resolved by the client
    // (#279). See `query::call_addresses` for why the client does it.
    addresses: Option<Vec<String>>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_calls(
            &cache,
            search.as_deref(),
            query::TimeRange { lo, hi },
            addresses.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every distinct peer address in the call log, so the client can resolve them
/// to contact names and search by name (#279).
#[tauri::command]
async fn call_addresses(active: State<'_, ActiveBackup>) -> Result<Vec<String>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::call_addresses(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_call_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
    search: Option<String>,
    addresses: Option<Vec<String>>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_call_ranges(&cache, &ranges, search.as_deref(), addresses.as_deref())
            .map_err(|e| e.to_string())
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
    addresses: Option<Vec<String>>,
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
            addresses.as_deref(),
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
async fn message_deletion_evidence(
    active: State<'_, ActiveBackup>,
) -> Result<query::DeletionEvidence, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::message_deletion_evidence(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_devices_used(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::DeviceUse>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_devices_used(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_device_os_history(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<query::DeviceUse>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::list_device_os_history(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_safari_searches(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari_searches(&cache, search.as_deref(), query::TimeRange { lo, hi })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_safari_search_ranges(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_safari_search_ranges(&cache, search.as_deref(), &ranges)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn get_safari_searches_window(
    active: State<'_, ActiveBackup>,
    search: Option<String>,
    lo: Option<i64>,
    hi: Option<i64>,
    offset: i64,
    limit: i64,
    sort_by: String,
    desc: bool,
) -> Result<Vec<query::WebSearch>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_safari_searches_window(
            &cache,
            search.as_deref(),
            query::TimeRange { lo, hi },
            offset,
            limit,
            safari_search_sort(&sort_by, desc),
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
/// `Cache-Control` for a media response, and WHY IT IS NOT ALWAYS `no-cache`.
///
/// On a custom URI scheme there is no revalidation mechanism, so `no-cache` means
/// WebKit re-invokes the scheme HANDLER every time it needs the bytes again —
/// including on an ordinary repaint. Hovering a gallery tile (the badge/mark
/// opacity reveals) repaints the scroll layer, so every visible thumbnail was
/// re-fetched and re-decoded at once: the "hovering one photo makes the ones
/// below blink" report.
///
/// A versioned URL is what makes a long-lived cache safe. `k=<n>` is minted per
/// view mount by `useMediaCacheKey`, so anything that should invalidate —
/// switching backups, re-importing, leaving and returning to the view — remounts
/// the view and changes the key, and the old URLs are simply never requested
/// again. An UNVERSIONED url keeps `no-cache`, because nothing could bust it.
fn media_cache_control(query_str: Option<&str>) -> &'static str {
    if query_str.is_some_and(|q| q.contains("k=")) {
        "private, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn media_protocol_response(
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

    // Path is "/<id>"; the query may carry "thumb=1".
    let Some(id) = path.trim_start_matches('/').parse::<i64>().ok() else {
        return not_found();
    };
    let want_thumb = query_str.is_some_and(|q| q.contains("thumb"));
    // A downscaled full-screen preview (see media::render_preview): far faster to
    // load than the original, which is what stops a fast next/prev from leaving
    // black frames.
    let want_preview = query_str.is_some_and(|q| q.contains("preview"));

    let cache_ctl = media_cache_control(query_str);

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
    // even without the keys. Videos use this for the grid tile AND the lightbox
    // poster.
    if want_thumb {
        if let Some(tp) = thumb_path {
            if let Ok(bytes) = std::fs::read(&tp) {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "image/jpeg")
                    .header("Cache-Control", cache_ctl)
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

    // VIDEO: stream it (Range-seekable), never buffer the whole file. `<video>`
    // in WKWebView needs `206`/`Accept-Ranges` to start playing at all — without
    // it a 100 MB+ clip must fully download (and, for encrypted backups, fully
    // decrypt into memory) before the first frame, which is why large videos
    // "never start." The message-attachment and audio handlers already do this;
    // this brings the gallery's own scheme up to the same behaviour.
    //
    // Detection uses the ORIGINAL path (`local_path`), whose extension survives
    // even when an encrypted backup's on-disk source is an extension-less temp.
    if !want_thumb && media::is_video(std::path::Path::new(&local_path), mime.as_deref()) {
        // Resolve a stable plaintext source. For encrypted backups decrypt ONCE
        // to a reused temp (`decrypt_to_cache`), because the webview fires many
        // Range requests while scrubbing and re-decrypting per request would
        // thrash disk/OOM. `clear_decrypted_temps` removes it on close/switch.
        let source_path: PathBuf = if let Some(key) = decrypt_key {
            let Some(dec) = ensure_session_decryptor(app, &cache_path) else {
                return not_found();
            };
            let out = thumbs_dir.join(format!("media-{id}.decrypted"));
            let Some(p) = decrypt_to_cache(&dec, &key, Path::new(&local_path), plain_size, &out)
            else {
                return not_found();
            };
            p
        } else {
            PathBuf::from(&local_path)
        };

        let content_type =
            media::video_content_type(std::path::Path::new(&local_path), mime.as_deref());
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
                .header("Cache-Control", cache_ctl)
                .body(buf)
                .unwrap();
        }

        let Ok(bytes) = std::fs::read(&source_path) else {
            return not_found();
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Accept-Ranges", "bytes")
            .header("Cache-Control", cache_ctl)
            .body(bytes)
            .unwrap();
    }

    // Encrypted original: decrypt it (using the session keys) to a temp file that
    // `media::render` / sips can read, then discard the plaintext. The rendered
    // JPEG is still cached by id, so repeat views don't re-decrypt via sips.
    // ALREADY RENDERED? Serve it and touch nothing else.
    //
    // `render` caches its output, but on an encrypted backup we used to decrypt
    // the whole original into memory and write a temp BEFORE calling it — only
    // for `render` to return the cached JPEG immediately. So every repaint
    // re-decrypted every visible HEIC, which is why the flicker was HEIC-only
    // (other formats are served as original bytes and never take this path).
    if let Some(r) = media::cached(
        &thumbs_dir,
        id,
        media::render_suffix(want_thumb, want_preview),
    ) {
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", r.content_type)
            .header("Cache-Control", cache_ctl)
            .body(r.bytes)
            .unwrap();
    }

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
        if want_preview {
            media::render_preview(&tmp, &thumbs_dir, id, mime.as_deref())
        } else {
            media::render(&tmp, &thumbs_dir, id, want_thumb, mime.as_deref())
        }
    } else if want_preview {
        media::render_preview(
            std::path::Path::new(&local_path),
            &thumbs_dir,
            id,
            mime.as_deref(),
        )
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
        .header("Cache-Control", cache_ctl)
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

/// One app's fetched App Store artwork, as a self-contained data: URI.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppIcon {
    bundle_id: String,
    data_uri: String,
}

/// Fetch real App Store icons for `bundle_ids` via Apple's public iTunes lookup
/// API, caching each as a data: URI on disk so it's a one-time cost. Opt-in
/// (Settings → Apps): this is the only feature that sends data off-device — it
/// tells Apple which apps a backup contains — so the caller gates it on the
/// user's setting. Best-effort: apps with no store match are silently skipped
/// (and negatively cached so they aren't retried every visit).
#[tauri::command]
async fn get_app_icons(app: AppHandle, bundle_ids: Vec<String>) -> Result<Vec<AppIcon>, String> {
    let cache_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("app-icons");
    let _ = std::fs::create_dir_all(&cache_dir);
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        // Cap per call so a huge app list can't spin up an unbounded fetch storm;
        // the disk cache means subsequent visits resolve instantly anyway.
        for id in bundle_ids.iter().take(120) {
            if let Some(data_uri) = app_icon_cached_or_fetch(&cache_dir, id) {
                out.push(AppIcon {
                    bundle_id: id.clone(),
                    data_uri,
                });
            }
        }
        out
    })
    .await
    .map_err(|e| e.to_string())
}

/// A cached data: URI for `bundle_id`, or a fresh fetch from the iTunes lookup
/// API. A cached empty file is a negative result (no store match) — respected
/// so we don't re-hit Apple for it every time.
fn app_icon_cached_or_fetch(cache_dir: &std::path::Path, bundle_id: &str) -> Option<String> {
    // Reject anything that isn't a plain bundle id, both to build a safe cache
    // filename and to keep it out of the query string unescaped.
    if bundle_id.is_empty()
        || !bundle_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    let cache_file = cache_dir.join(format!("{bundle_id}.datauri"));
    if let Ok(s) = std::fs::read_to_string(&cache_file) {
        return if s.is_empty() { None } else { Some(s) };
    }

    let url = format!("https://itunes.apple.com/lookup?bundleId={bundle_id}");
    let Ok((_final, bytes)) = safe_http_get(&url, 1024 * 1024, None) else {
        // Network/transient error: don't negatively cache — retry next time.
        return None;
    };
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let art = parsed["results"]
        .get(0)
        .and_then(|r| {
            r.get("artworkUrl100")
                .or_else(|| r.get("artworkUrl60"))
                .or_else(|| r.get("artworkUrl512"))
        })
        .and_then(|v| v.as_str());
    let Some(art) = art else {
        // Definitive "no store match": negatively cache so we skip it next time.
        let _ = std::fs::write(&cache_file, b"");
        return None;
    };
    let uri = proxy_image(art)?;
    let _ = std::fs::write(&cache_file, &uri);
    Some(uri)
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
        .manage(ScanCancel::default())
        .manage(ScanGate::default())
        .manage(safety_scan_cmd::SafetyScanCancel::default())
        .manage(safety_scan_cmd::SafetyScanGate::default())
        .manage(safety_scan_cmd::SafetyDownloadCancel::default())
        .manage(safety_scan_cmd::SafetyDownloadGate::default())
        .manage(safety_scan_cmd::SafetyDownloadStatus::default())
        .manage(safety_scan_cmd::SafetyScanStatus::default())
        .manage(ImportStatus::default())
        .manage(SecurityScanStatus::default())
        .manage(ReimportStatus::default())
        // Asynchronous protocols: the handlers decrypt bytes and shell out to
        // `sips` to render/downscale images. On the *synchronous* scheme that
        // runs on the main thread, so scrolling a timeline or gallery full of
        // thumbnails/avatars froze the whole UI. Answer each request on a
        // blocking worker instead and hand the bytes back via the responder.
        .register_asynchronous_uri_scheme_protocol("traceloupe-media", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            let query = request.uri().query().map(str::to_string);
            // Videos are Range-served (see media_protocol_response); the webview
            // sends `Range` while scrubbing and expects `206`.
            let range = request
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(media_protocol_response(
                    &app,
                    &path,
                    query.as_deref(),
                    range.as_deref(),
                ));
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
            system_watch::subscribe_system_changes,
            system_watch::get_system_text_scale,
            system_watch::get_system_locale,
            system_watch::get_full_keyboard_access,
            system_watch::get_accessibility_prefs,
            theme::get_system_selection_color,
            subscribe_import_progress,
            subscribe_security_progress,
            engine_info,
            install_engine,
            list_import_modules,
            set_log_level,
            cancel_import,
            import_backup,
            open_backup,
            has_active_backup,
            close_backup,
            set_biometric_required,
            app_signing_status,
            reimport_module,
            forget_backup,
            imported_backup_ids,
            list_threads,
            device_info,
            module_status,
            set_item_mark,
            marked_ids,
            threads_in_ranges,
            mark_counts,
            count_timeline_events,
            get_timeline_events,
            timeline_facets,
            raw_databases,
            raw_tables,
            raw_rows,
            module_metrics,
            list_calendar_events,
            list_reminders,
            list_workouts,
            workout_route,
            health_daily,
            list_sleep,
            list_health_timezones,
            list_health_achievements,
            list_cycle,
            health_summary,
            message_kinds,
            count_thread_messages,
            get_thread_message_window,
            thread_message_index,
            recover_attachment_media,
            count_timeline_messages,
            get_timeline_window,
            count_message_ranges,
            count_note_ranges,
            message_date_bounds,
            media_date_bounds,
            get_range_window,
            open_attachment,
            list_calls,
            list_notes,
            unlock_note,
            list_recordings,
            list_safari_history,
            list_contacts,
            list_installed_apps,
            get_app_icons,
            media_sources,
            count_media,
            count_media_ranges,
            get_media_window,
            set_media_favorite,
            save_media,
            reveal_media,
            count_calls,
            count_call_ranges,
            call_addresses,
            get_calls_window,
            count_safari,
            count_safari_ranges,
            message_deletion_evidence,
            list_devices_used,
            list_device_os_history,
            count_safari_searches,
            count_safari_search_ranges,
            get_safari_searches_window,
            count_safari_bookmarks,
            count_safari_bookmark_ranges,
            get_safari_bookmarks_window,
            get_safari_window,
            run_security_scan,
            cancel_scan,
            list_scan_runs,
            latest_scan_run,
            list_findings,
            get_indicator_info,
            update_indicators,
            get_detection_settings,
            set_detection_settings,
            run_passive_check_now,
            export_scan_report,
            find_shortener_urls,
            expand_short_url,
            deshorten_auto_approve_get,
            deshorten_auto_approve_set,
            safety_scan_cmd::get_safety_scan_model_status,
            safety_scan_cmd::safety_scan_health_check,
            safety_scan_cmd::download_safety_scan_model,
            safety_scan_cmd::get_safety_scan_download_status,
            safety_scan_cmd::cancel_safety_scan_model_download,
            safety_scan_cmd::run_safety_scan,
            safety_scan_cmd::cancel_safety_scan,
            safety_scan_cmd::list_content_findings,
            safety_scan_cmd::content_finding_rank,
            safety_scan_cmd::count_content_findings,
            safety_scan_cmd::content_finding_analytics,
            safety_scan_cmd::subscribe_safety_scan_progress,
            safety_scan_cmd::subscribe_safety_model_progress,
            safety_scan_cmd::content_finding_snippet,
            safety_scan_cmd::safety_scan_finding_marks,
            safety_scan_cmd::dismiss_content_finding,
            safety_scan_cmd::mark_content_finding_seen,
            safety_scan_cmd::add_safety_suppression,
            safety_scan_cmd::list_safety_suppressions,
            safety_scan_cmd::remove_safety_suppression,
            logging::subscribe_logs,
            logging::set_file_logging,
            logging::log_file_path,
            logging::reveal_log_file,
            get_import_status,
            get_security_scan_status,
            get_reimport_status,
            safety_scan_cmd::get_safety_scan_status,
            safety_scan_cmd::get_safety_scan_report,
            safety_scan_cmd::generate_thread_summary,
            safety_scan_cmd::list_safety_scans,
            safety_scan_cmd::delete_safety_scan,
            theme::get_system_accent_color,
            list_artifacts,
            get_artifact_rows,
            artifacts_extraction_state,
            extract_artifacts
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
    fn versioned_media_urls_are_cacheable_unversioned_are_not() {
        // A VERSIONED url may be cached hard: `k=` changes whenever the view
        // remounts, which is the only thing that should invalidate it. Without
        // this, `no-cache` forces the scheme handler to re-run on every repaint
        // — which is what made hovering one tile blank the ones below it.
        assert!(media_cache_control(Some("thumb=1&k=7")).contains("immutable"));
        assert!(media_cache_control(Some("preview=1&k=12")).contains("max-age"));
        // UNVERSIONED: nothing could bust a stale entry, so it must not be
        // cached — a re-import would otherwise keep serving the old bytes.
        assert_eq!(media_cache_control(Some("thumb=1")), "no-cache");
        assert_eq!(media_cache_control(None), "no-cache");
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

    #[test]
    fn deshorten_refuses_non_shortener_without_network() {
        // A non-shortener host is rejected before any request is made.
        let err = resolve_short_url("https://example.com/whatever").unwrap_err();
        assert!(err.contains("not a recognized shortened link"));
        // Malformed input, likewise no network.
        assert!(resolve_short_url("not a url").is_err());
    }

    #[test]
    fn deshorten_rejects_unsupported_scheme_on_shortener() {
        // Host is a shortener, but the scheme isn't http(s) → rejected in-loop,
        // still no outbound request.
        let err = resolve_short_url("ftp://bit.ly/abc").unwrap_err();
        assert!(err.contains("unsupported URL scheme"));
    }
}
