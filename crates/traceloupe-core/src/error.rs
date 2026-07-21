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

    /// Downloading/installing the engine failed (network, disk, or a checksum
    /// mismatch — the last is a hard integrity failure, never retried silently).
    #[error("engine download failed: {0}")]
    EngineDownload(String),

    /// Decrypting an encrypted backup failed: a malformed keybag, a wrong
    /// password (a key unwrap that didn't validate), or an unexpected blob.
    #[error("backup decryption failed: {0}")]
    Decrypt(String),

    /// A native parser could not make sense of a source database (e.g. an
    /// unrecognized schema). Non-fatal at the import layer: the caller falls
    /// back to the iLEAPP path.
    #[error("parse error: {0}")]
    Parse(String),

    /// A caller passed a value outside its documented domain (bad severity,
    /// unknown category slug, wrong-state transition). Analysis-store layer.
    #[error("invalid argument: {0}")]
    Invalid(String),

    /// An indicator feed (STIX2 bundle or Echap YAML) could not be parsed at
    /// all. Individual unrecognized patterns inside a feed are skipped and
    /// reported, not errors.
    #[error("indicator feed {feed}: {message}")]
    IndicatorFeed { feed: String, message: String },
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
