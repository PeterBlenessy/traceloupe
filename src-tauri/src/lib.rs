//! Thin Tauri command layer over salvage-core (architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

use std::path::PathBuf;

use salvage_core::discovery::{self, BackupInfo};
use salvage_core::engine::{self};
use salvage_core::import::{self, ImportPhase};
use salvage_core::sidecar::CancelToken;
use tauri::{AppHandle, Emitter, Manager};

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

    Ok(ImportResult {
        cache_path: outcome.cache_path.display().to_string(),
        threads: outcome.report.threads,
        messages: outcome.report.messages,
        media_items: outcome.report.media_items,
        warnings: outcome.report.warnings,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_backups,
            engine_status,
            import_backup
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
