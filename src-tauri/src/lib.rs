//! Thin Tauri command layer over salvage-core (architecture.md §4).
//! Commands translate core results into serializable responses; no parsing
//! or business logic lives here.

use salvage_core::discovery::{self, BackupInfo};

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
        Some(r) => std::path::PathBuf::from(r),
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![list_backups])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
