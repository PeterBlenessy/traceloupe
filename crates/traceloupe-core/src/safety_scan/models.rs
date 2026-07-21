//! Model provisioning (plan T2): the two-entry hardcoded catalog — Gemma 4
//! E4B (default) and E2B (low-RAM fallback) — with pinned sha256 checksums,
//! verified streaming download, and the RAM check that picks the default tier.
//!
//! Checksums/sizes are the Hugging Face LFS oids for the unsloth GGUF repos,
//! captured 2026-07-21. Bumping a model version means changing filename, size,
//! AND sha256 together; the download hard-fails on any mismatch.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::install::InstallProgress;
use crate::sidecar::CancelToken;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Stable id stored in `scans.model` and settings.
    pub id: &'static str,
    pub display_name: &'static str,
    pub repo: &'static str,
    pub filename: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    /// Below this much total system RAM, this model is not recommended.
    pub ram_floor_bytes: u64,
    /// Context size the sidecar is started with. Chunk prompts are ~2-3k
    /// tokens; 8k leaves headroom without the RAM cost of the full 128k.
    pub ctx_size: u32,
}

impl ModelSpec {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}",
            self.repo, self.filename
        )
    }

    /// The installed path under `models_dir`, if present with the right size.
    /// (Integrity is guaranteed at download time; a full 5 GB re-hash on every
    /// launch is not worth it.)
    pub fn installed_at(&self, models_dir: &Path) -> Option<PathBuf> {
        let path = models_dir.join(self.filename);
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() == self.size_bytes => Some(path),
            _ => None,
        }
    }
}

const GIB: u64 = 1024 * 1024 * 1024;

/// The whole catalog. Deliberately two entries (grill decision): no model
/// picker sprawl, just the default and the low-RAM fallback.
pub const CATALOG: [ModelSpec; 2] = [
    ModelSpec {
        id: "gemma-4-E4B-it-Q4_K_M",
        display_name: "Gemma 4 E4B (recommended)",
        repo: "unsloth/gemma-4-E4B-it-GGUF",
        filename: "gemma-4-E4B-it-Q4_K_M.gguf",
        sha256: "85a896a047553e842f25297ee5b031d64ff30147d9c4af17b1e4b394cd1fab87",
        size_bytes: 4_977_171_584,
        ram_floor_bytes: 12 * GIB,
        ctx_size: 8192,
    },
    ModelSpec {
        id: "gemma-4-E2B-it-Q4_K_M",
        display_name: "Gemma 4 E2B (for 8 GB Macs)",
        repo: "unsloth/gemma-4-E2B-it-GGUF",
        filename: "gemma-4-E2B-it-Q4_K_M.gguf",
        sha256: "740185b21d22ceb83a11c3aa62ad5842ef32c70f6096d756bbee85a1e4ec34b8",
        size_bytes: 3_106_738_272,
        ram_floor_bytes: 6 * GIB,
        ctx_size: 8192,
    },
];

pub fn spec_by_id(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|s| s.id == id)
}

/// Total physical RAM. macOS: sysctl hw.memsize; elsewhere 0 (unknown).
pub fn total_ram_bytes() -> u64 {
    if cfg!(target_os = "macos") {
        Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    } else {
        0
    }
}

/// The tier to propose: E4B when the machine clears its RAM floor (or RAM is
/// unknown — the user asked for E4B by default), else E2B. The user may still
/// override; the UI warns.
pub fn recommended(total_ram: u64) -> &'static ModelSpec {
    if total_ram == 0 || total_ram >= CATALOG[0].ram_floor_bytes {
        &CATALOG[0]
    } else {
        &CATALOG[1]
    }
}

/// Download `spec` into `models_dir` with streaming sha256 verification —
/// `install.rs` semantics (temp file, size cap, rename only after the
/// checksum matches) plus cancellation, which a 5 GB download needs.
pub fn download_model(
    spec: &ModelSpec,
    models_dir: &Path,
    cancel: &CancelToken,
    on_progress: impl FnMut(InstallProgress),
) -> Result<PathBuf> {
    let agent = ureq::AgentBuilder::new().build();
    download_model_with(&agent, &spec.url(), spec, models_dir, cancel, on_progress)
}

/// Testable core: URL and agent injectable so tests use a local server.
pub(crate) fn download_model_with(
    agent: &ureq::Agent,
    url: &str,
    spec: &ModelSpec,
    models_dir: &Path,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(InstallProgress),
) -> Result<PathBuf> {
    std::fs::create_dir_all(models_dir).map_err(|e| Error::EngineDownload(e.to_string()))?;
    let tmp = models_dir.join(format!("{}.downloading", spec.filename));
    let final_path = models_dir.join(spec.filename);

    let resp = agent
        .get(url)
        .call()
        .map_err(|e| Error::EngineDownload(format!("request failed: {e}")))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(spec.size_bytes);

    // Same MITM/disk-fill cap as install.rs: verification happens at EOF, so
    // bound what a hostile host can stream first.
    let max_bytes = spec.size_bytes.saturating_mul(2).max(64 * 1024 * 1024);

    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&tmp).map_err(|e| Error::EngineDownload(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    let mut received: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::Cancelled);
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::EngineDownload(e.to_string()))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > max_bytes {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::EngineDownload(
                "download exceeded the expected size; aborting".into(),
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
    if !digest.eq_ignore_ascii_case(spec.sha256) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::EngineDownload(format!(
            "checksum mismatch for {}: expected {}, got {digest}",
            spec.filename, spec.sha256
        )));
    }
    std::fs::rename(&tmp, &final_path).map_err(|e| Error::EngineDownload(e.to_string()))?;
    on_progress(InstallProgress::Done);
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn serve_bytes(payload: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.gguf", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(&payload);
            }
        });
        url
    }

    fn tiny_spec(payload: &[u8], correct_sha: bool) -> ModelSpec {
        // Leak the strings: ModelSpec uses &'static (fine for tests).
        let sha: &'static str = Box::leak(
            if correct_sha {
                hex::encode(Sha256::digest(payload))
            } else {
                "0".repeat(64)
            }
            .into_boxed_str(),
        );
        ModelSpec {
            id: "test-model",
            display_name: "Test",
            repo: "test/repo",
            filename: "test.gguf",
            sha256: sha,
            size_bytes: payload.len() as u64,
            ram_floor_bytes: 0,
            ctx_size: 4096,
        }
    }

    #[test]
    fn catalog_is_sane() {
        assert_eq!(CATALOG.len(), 2);
        for spec in &CATALOG {
            assert_eq!(spec.sha256.len(), 64, "{}: sha must be 64 hex", spec.id);
            assert!(spec.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(spec.size_bytes > GIB);
            assert!(spec.url().starts_with("https://huggingface.co/"));
        }
        assert_ne!(CATALOG[0].sha256, CATALOG[1].sha256);
        assert!(spec_by_id("gemma-4-E4B-it-Q4_K_M").is_some());
        assert!(spec_by_id("nope").is_none());
    }

    #[test]
    fn recommendation_thresholds() {
        assert_eq!(recommended(16 * GIB).id, CATALOG[0].id);
        assert_eq!(recommended(8 * GIB).id, CATALOG[1].id);
        // Unknown RAM → the user's requested default (E4B), not the fallback.
        assert_eq!(recommended(0).id, CATALOG[0].id);
    }

    #[test]
    fn download_verifies_and_installs() {
        let payload = b"pretend this is a gguf".to_vec();
        let url = serve_bytes(payload.clone());
        let spec = tiny_spec(&payload, true);
        let dir = tempfile::tempdir().unwrap();
        let agent = ureq::AgentBuilder::new().build();
        let mut phases = Vec::new();
        let path = download_model_with(&agent, &url, &spec, dir.path(), &CancelToken::new(), |p| {
            phases.push(std::mem::discriminant(&p));
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), payload);
        assert!(spec.installed_at(dir.path()).is_some());
        assert!(!dir.path().join("test.gguf.downloading").exists());
    }

    #[test]
    fn checksum_mismatch_discards_partial() {
        let payload = b"tampered bytes".to_vec();
        let url = serve_bytes(payload.clone());
        let spec = tiny_spec(&payload, false);
        let dir = tempfile::tempdir().unwrap();
        let agent = ureq::AgentBuilder::new().build();
        let err = download_model_with(&agent, &url, &spec, dir.path(), &CancelToken::new(), |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
        assert!(spec.installed_at(dir.path()).is_none());
        assert!(!dir.path().join("test.gguf.downloading").exists());
        assert!(!dir.path().join("test.gguf").exists());
    }

    #[test]
    fn cancel_discards_partial() {
        let payload = vec![7u8; 2 * 1024 * 1024];
        let url = serve_bytes(payload.clone());
        let spec = tiny_spec(&payload, true);
        let dir = tempfile::tempdir().unwrap();
        let agent = ureq::AgentBuilder::new().build();
        let cancel = CancelToken::new();
        cancel.cancel();
        let err =
            download_model_with(&agent, &url, &spec, dir.path(), &cancel, |_| {}).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert!(!dir.path().join("test.gguf.downloading").exists());
    }

    #[test]
    fn installed_at_requires_exact_size() {
        let dir = tempfile::tempdir().unwrap();
        let spec = tiny_spec(b"12345", true);
        assert!(spec.installed_at(dir.path()).is_none());
        std::fs::write(dir.path().join("test.gguf"), b"123").unwrap(); // truncated
        assert!(spec.installed_at(dir.path()).is_none());
        std::fs::write(dir.path().join("test.gguf"), b"12345").unwrap();
        assert!(spec.installed_at(dir.path()).is_some());
    }
}
