//! Download-on-first-use of the iLEAPP engine (architecture §9).
//!
//! The engine is not bundled. On first import the app fetches a **pinned**
//! engine binary, verifies its SHA-256 against a checksum baked into the app,
//! stores it under the app data dir, and runs it thereafter. The pin matters
//! because `_lava_artifacts.db` is not a stable API — the normalizer targets a
//! specific engine version (see docs/spike-ileapp.md).
//!
//! This module is transport/format agnostic and fully testable: it downloads
//! from any URL, streams to disk while hashing, and only commits the file if
//! the digest matches. A mismatch is a hard failure — a corrupt or tampered
//! download is never executed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// A pinned engine artifact to download and verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineManifest {
    /// Human-readable version, shown to the user before downloading.
    pub version: String,
    /// Where to fetch the binary from.
    pub url: String,
    /// Lowercase hex SHA-256 the download must match.
    pub sha256: String,
    /// Expected size in bytes, for the progress bar (0 if unknown).
    pub size: u64,
}

impl EngineManifest {
    /// Whether a downloadable engine has actually been published yet. Until we
    /// host a re-frozen build, the pinned URL is empty and the app falls back to
    /// the "engine not installed" guidance / local dev engine.
    pub fn is_published(&self) -> bool {
        !self.url.is_empty() && !self.sha256.is_empty()
    }
}

/// The engine TraceLoupe installs on first use.
///
/// TODO(publish): fill `url`/`sha256`/`size` once a re-frozen iLEAPP is hosted
/// as a release asset. Upstream's own macOS binary is broken (Pillow), so this
/// must point at *our* build. Until then `is_published()` is false and the app
/// uses the local dev engine (`pnpm setup:engine`).
pub fn pinned_engine() -> EngineManifest {
    EngineManifest {
        version: "iLEAPP v2026.1.0".to_string(),
        url: String::new(),
        sha256: String::new(),
        size: 0,
    }
}

/// Progress of an engine install, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallProgress {
    /// `received`/`total` bytes downloaded so far (`total` 0 if unknown).
    Downloading {
        received: u64,
        total: u64,
    },
    /// Download complete; verifying the checksum.
    Verifying,
    Done,
}

/// Download the manifest's binary into `install_dir`, verify its SHA-256, make
/// it executable, and return the installed path (`<install_dir>/ileapp`).
///
/// Streams to a temporary file so a partial/failed download never leaves a
/// runnable binary behind; the file is only renamed into place after the
/// checksum matches.
pub fn install_engine(
    manifest: &EngineManifest,
    install_dir: &Path,
    on_progress: impl FnMut(InstallProgress),
) -> Result<PathBuf> {
    let agent = ureq::AgentBuilder::new().build();
    install_engine_with(&agent, manifest, install_dir, on_progress)
}

/// Testable core: takes the `ureq` agent so tests can point it at a local server.
pub(crate) fn install_engine_with(
    agent: &ureq::Agent,
    manifest: &EngineManifest,
    install_dir: &Path,
    mut on_progress: impl FnMut(InstallProgress),
) -> Result<PathBuf> {
    if !manifest.is_published() {
        return Err(Error::EngineDownload(
            "no engine has been published to download yet".into(),
        ));
    }
    std::fs::create_dir_all(install_dir).map_err(|e| Error::EngineDownload(e.to_string()))?;
    let tmp = install_dir.join("ileapp.download");
    let final_path = install_dir.join("ileapp");

    let resp = agent
        .get(&manifest.url)
        .call()
        .map_err(|e| Error::EngineDownload(format!("request failed: {e}")))?;

    // Prefer the server's Content-Length; fall back to the manifest's size.
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(manifest.size);

    // Cap the download so a compromised/MITM'd host can't stream unbounded bytes
    // to fill the disk before the checksum is ever checked (verification is at
    // EOF). Allow a generous margin over the manifest's declared size.
    let max_bytes = manifest.size.saturating_mul(2).max(256 * 1024 * 1024);

    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&tmp).map_err(|e| Error::EngineDownload(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut received: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::EngineDownload(e.to_string()))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > max_bytes {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::EngineDownload(
                "download exceeded the expected size; aborting".to_string(),
            ));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| Error::EngineDownload(e.to_string()))?;
        on_progress(InstallProgress::Downloading { received, total });
    }
    file.flush()
        .map_err(|e| Error::EngineDownload(e.to_string()))?;
    drop(file);

    on_progress(InstallProgress::Verifying);
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&manifest.sha256) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::EngineDownload(format!(
            "checksum mismatch: expected {}, got {digest}",
            manifest.sha256
        )));
    }

    make_executable(&tmp)?;
    std::fs::rename(&tmp, &final_path).map_err(|e| Error::EngineDownload(e.to_string()))?;
    on_progress(InstallProgress::Done);
    Ok(final_path)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| Error::EngineDownload(e.to_string()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| Error::EngineDownload(e.to_string()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// Serve `body` over HTTP on a throwaway port, once, and return the URL.
    /// Minimal by hand so the test needs no HTTP-server dependency.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/ileapp", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request line/headers.
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
                stream.flush().unwrap();
            }
        });
        url
    }

    fn manifest_for(url: String, body: &[u8]) -> EngineManifest {
        EngineManifest {
            version: "test".into(),
            url,
            sha256: hex::encode(Sha256::digest(body)),
            size: body.len() as u64,
        }
    }

    #[test]
    fn downloads_verifies_and_installs_executable() {
        let body = b"#!/bin/sh\necho fake ileapp\n".to_vec();
        let url = serve_once(body.clone());
        let manifest = manifest_for(url, &body);
        let tmp = tempfile::tempdir().unwrap();

        let mut saw_verify = false;
        let mut last_received = 0u64;
        let path = install_engine(&manifest, tmp.path(), |p| match p {
            InstallProgress::Downloading { received, .. } => last_received = received,
            InstallProgress::Verifying => saw_verify = true,
            InstallProgress::Done => {}
        })
        .unwrap();

        assert_eq!(path, tmp.path().join("ileapp"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(last_received, body.len() as u64);
        assert!(saw_verify);
        // No leftover partial file.
        assert!(!tmp.path().join("ileapp.download").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "installed binary must be executable");
        }
    }

    #[test]
    fn checksum_mismatch_is_rejected_and_leaves_nothing() {
        let body = b"real bytes".to_vec();
        let url = serve_once(body.clone());
        // Manifest claims a different (wrong) digest.
        let mut manifest = manifest_for(url, &body);
        manifest.sha256 = "0".repeat(64);
        let tmp = tempfile::tempdir().unwrap();

        let err = install_engine(&manifest, tmp.path(), |_| {}).unwrap_err();
        assert!(matches!(err, Error::EngineDownload(m) if m.contains("checksum mismatch")));
        // Neither the temp download nor the final binary survive a bad checksum.
        assert!(!tmp.path().join("ileapp.download").exists());
        assert!(!tmp.path().join("ileapp").exists());
    }

    #[test]
    fn unpublished_manifest_does_not_download() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = pinned_engine(); // url/sha empty until we publish
        assert!(!manifest.is_published());
        let err = install_engine(&manifest, tmp.path(), |_| {}).unwrap_err();
        assert!(matches!(err, Error::EngineDownload(_)));
    }
}
