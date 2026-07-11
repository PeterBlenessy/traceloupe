use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The backup directory exists but macOS denied access — the Full Disk
    /// Access case. The UI shows grant-FDA guidance for this variant.
    #[error("permission denied reading {path}")]
    PermissionDenied { path: PathBuf },

    #[error("backup directory not found: {path}")]
    BackupDirNotFound { path: PathBuf },

    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("malformed plist at {path}: {source}")]
    Plist { path: PathBuf, source: plist::Error },

    #[error("cache database error: {0}")]
    Cache(#[from] rusqlite::Error),

    /// The iLEAPP sidecar binary could not be located or spawned.
    #[error("iLEAPP engine not available: {0}")]
    EngineNotFound(String),

    /// The sidecar ran but exited non-zero; carries a short tail of its log.
    #[error("iLEAPP engine failed (exit {code}): {detail}")]
    EngineFailed { code: i32, detail: String },

    /// The import was cancelled by the caller before completion.
    #[error("import cancelled")]
    Cancelled,

    /// The sidecar produced no `_lava_artifacts.db` where one was expected.
    #[error("no engine output found under {path}")]
    NoEngineOutput { path: PathBuf },
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        let path = path.into();
        if source.kind() == std::io::ErrorKind::PermissionDenied {
            Error::PermissionDenied { path }
        } else {
            Error::Io { path, source }
        }
    }
}
