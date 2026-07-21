//! traceloupe-core: UI-agnostic core for the local iOS backup browser.
//!
//! This crate has no Tauri or UI dependencies. It exposes the use-cases the
//! shell calls over IPC: backup discovery, import orchestration, and cached
//! artifact queries. See architecture.md §5.

pub mod analysis;
pub mod analyzer;
pub mod cache;
pub mod crypto;
pub mod detection_settings;
pub mod discovery;
pub mod engine;
mod error;
pub mod import;
pub mod indicators;
pub mod install;
pub mod manifest;
pub mod normalize;
pub mod nska;
pub mod parsers;
pub mod query;
pub mod safety_scan;
pub mod sidecar;

pub use error::{Error, Result};

/// Write bytes to a file with owner-only (0600) permissions on Unix, so decrypted
/// plaintext (a Manifest, a transient DB) isn't left world-readable at rest. The
/// file is created fresh (truncating any existing content).
pub(crate) fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
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
