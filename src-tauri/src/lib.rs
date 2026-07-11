//! Thin Tauri command layer over salvage-core (architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

use std::path::PathBuf;
use std::sync::Mutex;

use salvage_core::cache::CacheDb;
use salvage_core::discovery::{self, BackupInfo};
use salvage_core::engine::{self};
use salvage_core::import::{self, ImportPhase};
use salvage_core::query::{self, Call, Contact, HistoryVisit, MediaItem, Message, ThreadSummary};
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
    let root = match root {
        Some(r) => PathBuf::from(r),
        None => discovery::default_backup_root()
            .ok_or_else(|| "cannot resolve home directory".to_string())?,
    };
    match discovery::discover_backups(&root) {
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

/// Whether an iLEAPP engine is resolvable right now. The UI uses this to decide
/// between offering "import" and "engine not installed" guidance.
#[tauri::command]
fn engine_status(app: AppHandle) -> bool {
    resolve_engine(&app).is_some()
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
#[tauri::command]
async fn import_backup(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    backup_path: String,
    backup_id: String,
    password: String,
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

    // Blocking pipeline on a worker thread; progress is emitted as it runs.
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        import::import_backup(
            &cfg,
            &backup_path,
            &password,
            &cache_path,
            &work_dir,
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
fn open_backup(app: AppHandle, active: State<'_, ActiveBackup>, backup_id: String) -> bool {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return false;
    };
    let cache_path = data_dir.join("caches").join(&backup_id).join("cache.db");
    if cache_path.exists() {
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

#[tauri::command]
fn list_threads(active: State<'_, ActiveBackup>) -> Result<Vec<ThreadSummary>, String> {
    let cache = open_active_cache(&active)?;
    query::list_threads(&cache).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_thread_messages(
    active: State<'_, ActiveBackup>,
    thread_id: i64,
) -> Result<Vec<Message>, String> {
    let cache = open_active_cache(&active)?;
    query::get_messages(&cache, thread_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_calls(active: State<'_, ActiveBackup>) -> Result<Vec<Call>, String> {
    let cache = open_active_cache(&active)?;
    query::list_calls(&cache).map_err(|e| e.to_string())
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
fn list_media(active: State<'_, ActiveBackup>) -> Result<Vec<MediaItem>, String> {
    let cache = open_active_cache(&active)?;
    query::list_media(&cache).map_err(|e| e.to_string())
}

/// Serve a media item's bytes over the `salvage-media://localhost/<id>` scheme.
///
/// Security: the handler takes only a numeric id, looks up the file path
/// recorded for it in the active cache, and serves that. It never accepts a
/// path from the request, so it can't be coerced into reading arbitrary files.
fn media_protocol_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    // Path is "/<id>".
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
    let Ok(Some((local_path, mime))) = query::media_blob(&cache, id) else {
        return not_found();
    };
    let Ok(bytes) = std::fs::read(&local_path) else {
        return not_found();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(
            "Content-Type",
            mime.as_deref().unwrap_or("application/octet-stream"),
        )
        .header("Cache-Control", "no-cache")
        .body(bytes)
        .unwrap()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(ActiveBackup::default())
        .register_uri_scheme_protocol("salvage-media", |ctx, request| {
            let path = request.uri().path().to_string();
            media_protocol_response(ctx.app_handle(), &path)
        })
        .invoke_handler(tauri::generate_handler![
            list_backups,
            engine_status,
            import_backup,
            open_backup,
            has_active_backup,
            list_threads,
            get_thread_messages,
            list_calls,
            list_safari_history,
            list_contacts,
            list_media
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
