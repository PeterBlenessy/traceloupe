//! Thin Tauri command layer over salvage-core (architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

mod media;
mod secret;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use salvage_core::cache::CacheDb;
use salvage_core::crypto::BackupDecryptor;
use salvage_core::discovery::{self, BackupInfo};
use salvage_core::engine::{self};
use salvage_core::import::{self, ImportPhase};
use salvage_core::install;
use salvage_core::query::{
    self, Call, Contact, HistoryVisit, MediaItem, Message, Note, ThreadSummary, TimelineMessage,
};
use salvage_core::sidecar::CancelToken;
use tauri::{AppHandle, Emitter, Manager, State};

/// The cache DB currently being browsed. Set when an import finishes or a
/// previously-imported backup is opened; read by every artifact query.
#[derive(Default)]
struct ActiveBackup(Mutex<Option<PathBuf>>);

impl ActiveBackup {
    fn set(&self, path: PathBuf) {
        *self.0.lock().unwrap() = Some(path);
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
        Err(salvage_core::Error::PermissionDenied { path }) => {
            Ok(DiscoveryResult::PermissionDenied {
                path: path.display().to_string(),
            })
        }
        Err(salvage_core::Error::BackupDirNotFound { path }) => Ok(DiscoveryResult::NotFound {
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
/// - `SALVAGE_PYTHON` + `SALVAGE_ILEAPP_SOURCE` → run from a source checkout.
/// - `SALVAGE_ILEAPP` → an explicit frozen binary.
/// - else `<app_data>/engine/ileapp` (downloaded on first use).
fn resolve_engine(app: &AppHandle) -> Option<salvage_core::sidecar::EngineConfig> {
    let source_override = match (
        std::env::var_os("SALVAGE_PYTHON"),
        std::env::var_os("SALVAGE_ILEAPP_SOURCE"),
    ) {
        (Some(py), Some(src)) => Some((PathBuf::from(py), PathBuf::from(src))),
        _ => None,
    };
    let binary_override = std::env::var_os("SALVAGE_ILEAPP").map(PathBuf::from);
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
    Normalizing,
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
fn list_import_modules() -> Vec<salvage_core::sidecar::ImportModule> {
    salvage_core::sidecar::IMPORT_CATALOG.to_vec()
}

#[tauri::command]
async fn import_backup(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    backup_path: String,
    backup_id: String,
    password: String,
    modules: Vec<String>,
) -> Result<ImportResult, String> {
    let cfg = resolve_engine(&app).ok_or_else(|| {
        "iLEAPP engine is not installed. Set SALVAGE_ILEAPP or install the engine.".to_string()
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
    let backup_path = PathBuf::from(backup_path);
    // Kept for post-import key setup (the originals are moved into the worker).
    let source_dir = backup_path.clone();
    let key_password = password.clone();

    // Blocking pipeline on a worker thread; progress is emitted as it runs.
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        import::import_backup(
            &cfg,
            &backup_path,
            &password,
            &cache_path,
            &work_dir,
            &modules,
            &cancel,
            |phase| {
                let event = match phase {
                    ImportPhase::Parsing(p) => Some(ImportEvent::Parsing {
                        current: p.current,
                        total: p.total,
                        fraction: p.fraction(),
                        artifact: p.artifact,
                    }),
                    ImportPhase::Normalizing => Some(ImportEvent::Normalizing),
                    ImportPhase::Done(_) => None,
                };
                if let Some(event) = event {
                    let _ = app.emit("import://progress", event);
                }
            },
        )
    })
    .await
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
fn open_backup(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    session: State<'_, SessionKeys>,
    backup_id: String,
) -> bool {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return false;
    };
    let cache_path = data_dir.join("caches").join(&backup_id).join("cache.db");
    if cache_path.exists() {
        // Rebuild the decryptor for an encrypted backup (no-op for plaintext).
        session.set(reopen_decryptor(&cache_path, &backup_id));
        active.set(cache_path);
        true
    } else {
        false
    }
}

/// Whether a backup is currently open for browsing.
#[tauri::command]
fn has_active_backup(active: State<'_, ActiveBackup>) -> bool {
    active.path().is_ok()
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
fn list_threads(active: State<'_, ActiveBackup>) -> Result<Vec<ThreadSummary>, String> {
    let cache = open_active_cache(&active)?;
    query::list_threads(&cache).map_err(|e| e.to_string())
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
async fn count_timeline_messages(active: State<'_, ActiveBackup>) -> Result<i64, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_all_messages(&cache).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_timeline_window(
    active: State<'_, ActiveBackup>,
    offset: i64,
    limit: i64,
) -> Result<Vec<TimelineMessage>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_timeline_window(&cache, offset, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn count_message_ranges(
    active: State<'_, ActiveBackup>,
    ranges: Vec<query::TimeRange>,
) -> Result<Vec<i64>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::count_message_ranges(&cache, &ranges).map_err(|e| e.to_string())
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
) -> Result<Vec<TimelineMessage>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_range_window(&cache, query::TimeRange { lo, hi }, offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a message attachment's file with the OS default app (for documents and
/// anything not rendered inline).
#[tauri::command]
fn open_attachment(active: State<'_, ActiveBackup>, attachment_id: i64) -> Result<(), String> {
    let cache = open_active_cache(&active)?;
    let (path, _) = query::attachment_blob(&cache, attachment_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "attachment file is not available".to_string())?;
    std::process::Command::new("/usr/bin/open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_calls(active: State<'_, ActiveBackup>) -> Result<Vec<Call>, String> {
    let cache = open_active_cache(&active)?;
    query::list_calls(&cache).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_notes(active: State<'_, ActiveBackup>) -> Result<Vec<Note>, String> {
    let cache = open_active_cache(&active)?;
    query::list_notes(&cache).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_safari_history(active: State<'_, ActiveBackup>) -> Result<Vec<HistoryVisit>, String> {
    let cache = open_active_cache(&active)?;
    query::list_safari_history(&cache).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_contacts(active: State<'_, ActiveBackup>) -> Result<Vec<Contact>, String> {
    let cache = open_active_cache(&active)?;
    query::list_contacts(&cache).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_installed_apps(active: State<'_, ActiveBackup>) -> Result<Vec<String>, String> {
    let cache = open_active_cache(&active)?;
    query::list_installed_apps(&cache).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_media(active: State<'_, ActiveBackup>) -> Result<Vec<MediaItem>, String> {
    let cache = open_active_cache(&active)?;
    query::list_media(&cache).map_err(|e| e.to_string())
}

/// (source label, count) pairs for the gallery's source filter.
#[tauri::command]
fn media_sources(active: State<'_, ActiveBackup>) -> Result<Vec<(String, i64)>, String> {
    let cache = open_active_cache(&active)?;
    query::media_sources(&cache).map_err(|e| e.to_string())
}

// Windowed, filterable list commands (async + spawn_blocking) so the UI can
// lazily load huge lists a slice at a time — the same pattern as messages.

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
) -> Result<Vec<MediaItem>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_media_window(&cache, source.as_deref(), offset, limit).map_err(|e| e.to_string())
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
) -> Result<Vec<Call>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_calls_window(&cache, search.as_deref(), offset, limit).map_err(|e| e.to_string())
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
) -> Result<Vec<HistoryVisit>, String> {
    let path = active.path()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = CacheDb::open(&path).map_err(|e| e.to_string())?;
        query::get_safari_window(&cache, search.as_deref(), offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Serve a media item over the `salvage-media://localhost/<id>` scheme
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
    let Ok(Some((local_path, mime, thumb_path, decrypt_key))) = query::media_blob(&cache, id)
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
        let Ok(plain) = dec.decrypt_bytes(&key, &ciphertext, None) else {
            return not_found();
        };
        let _ = std::fs::create_dir_all(&thumbs_dir);
        let tmp = thumbs_dir.join(format!("{id}.decrypted"));
        if std::fs::write(&tmp, &plain).is_err() {
            return not_found();
        }
        let out = media::render(&tmp, &thumbs_dir, id, want_thumb, mime.as_deref());
        let _ = std::fs::remove_file(&tmp);
        out
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

/// Serve a contact's photo over `salvage-avatar://localhost/<contactId>`.
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

/// Serve a message attachment over `salvage-attachment://localhost/<id>`
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
    let Ok(Some((local_path, mime))) = query::attachment_blob(&cache, id) else {
        return not_found();
    };

    let is_image = mime.as_deref().is_some_and(|m| m.starts_with("image/"))
        || local_path.to_ascii_lowercase().ends_with(".heic");

    if is_image {
        // Its own thumbs dir so attachment ids can't collide with media ids.
        let thumbs_dir = cache_path
            .parent()
            .map(|p| p.join("att-thumbs"))
            .unwrap_or_else(|| PathBuf::from("att-thumbs"));
        let Some(rendered) = media::render(
            std::path::Path::new(&local_path),
            &thumbs_dir,
            id,
            want_thumb,
            mime.as_deref(),
        ) else {
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
    // (and without reading the whole file into memory each time).
    let content_type = mime.unwrap_or_else(|| "application/octet-stream".to_string());
    let Ok(meta) = std::fs::metadata(&local_path) else {
        return not_found();
    };
    let total = meta.len();

    if let Some((start, end)) = range.and_then(|r| parse_byte_range(r, total)) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&local_path) else {
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

    let Ok(bytes) = std::fs::read(&local_path) else {
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
        .register_uri_scheme_protocol("salvage-media", |ctx, request| {
            let path = request.uri().path().to_string();
            let query = request.uri().query().map(str::to_string);
            media_protocol_response(ctx.app_handle(), &path, query.as_deref())
        })
        .register_uri_scheme_protocol("salvage-avatar", |ctx, request| {
            let path = request.uri().path().to_string();
            avatar_protocol_response(ctx.app_handle(), &path)
        })
        .register_uri_scheme_protocol("salvage-attachment", |ctx, request| {
            let path = request.uri().path().to_string();
            let query = request.uri().query().map(str::to_string);
            let range = request
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            attachment_protocol_response(
                ctx.app_handle(),
                &path,
                query.as_deref(),
                range.as_deref(),
            )
        })
        .invoke_handler(tauri::generate_handler![
            list_backups,
            default_backup_root,
            open_full_disk_access_settings,
            engine_status,
            engine_info,
            install_engine,
            list_import_modules,
            import_backup,
            open_backup,
            has_active_backup,
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
