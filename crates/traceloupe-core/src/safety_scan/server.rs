//! llama-server sidecar lifecycle (plan T1): resolve the pinned binary, spawn
//! it on loopback under a macOS Seatbelt sandbox, wait for /health, and make
//! sure it dies with us.
//!
//! Sandbox policy (ADR 0002): the server process gets no network beyond its
//! own loopback listen socket, and no file reads under /Users except the model
//! directory (and, for dev builds, the directory the binary lives in). System
//! frameworks, dyld caches, and Metal shader caches live outside /Users, so
//! GPU inference works untouched. Logging is silenced by wiring the child's
//! stdio to /dev/null — prompt text must never reach a log file.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Error, Result};

/// The sidecar binary name Tauri bundles per-target, mirroring NoteSage.
fn bundled_binary_name() -> String {
    format!("llama-server-{}-apple-darwin", std::env::consts::ARCH)
}

/// Locate the llama-server binary.
///
/// **Security invariant:** a *shipped* build runs ONLY the binary TraceLoupe
/// bundles — never an externally-installed one — because whatever runs here
/// processes backup text (in the prompts). So a release build resolves only
/// the bundled sidecar placed next to the app executable by Tauri's
/// `externalBin`. The env-override and `$PATH` conveniences exist for `tauri
/// dev` / `cargo test` and are compiled out of release builds entirely.
pub fn resolve_binary() -> Result<PathBuf> {
    // Production path: the bundled sidecar next to the executable
    // (Contents/MacOS on macOS). Tauri may keep the target-triple suffix or
    // strip it, so accept both names — mirrors NoteSage.
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        for name in [bundled_binary_name(), "llama-server".to_string()] {
            let candidate = exe_dir.join(&name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    // Dev-only fallbacks — NEVER compiled into a release build, so a shipped
    // app cannot be pointed at an external binary.
    #[cfg(debug_assertions)]
    {
        // The staged sidecar from `scripts/download-llama-server.sh`, which
        // lands in <repo>/src-tauri/binaries/ (with lib/ beside it). Walk up
        // from the dev executable so this works regardless of where cargo put
        // the target dir — this is what makes `tauri dev` "just work" after
        // running the download script once, no env var needed (NoteSage's
        // dev-source fallback).
        if let Ok(exe) = std::env::current_exe() {
            for ancestor in exe.ancestors() {
                let candidate = ancestor
                    .join("src-tauri/binaries")
                    .join(bundled_binary_name());
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
        // Explicit override, then PATH (e.g. `brew install llama.cpp`).
        if let Ok(p) = std::env::var("TRACELOUPE_LLAMA_SERVER") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Ok(p);
            }
            return Err(Error::Inference(format!(
                "TRACELOUPE_LLAMA_SERVER points at a missing file: {}",
                p.display()
            )));
        }
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join("llama-server");
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(Error::Inference(
        "bundled llama-server not found next to the app executable".into(),
    ))
}

/// Pick a free loopback port. Racy by nature (we close the probe socket
/// before llama-server binds), acceptable for a local single-user app.
pub fn pick_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::Inference(format!("no free loopback port: {}", e.kind())))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::Inference(format!("no local addr: {}", e.kind())))?
        .port();
    Ok(port)
}

fn sb_quote(path: &Path) -> String {
    // Seatbelt string literals: backslash-escape quotes and backslashes.
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

/// The Seatbelt profile. Starts default-allow (Metal, dyld, sysctl, iokit all
/// work — a deny-by-default profile can't run a GPU inference binary), then
/// clamps the three egress channels that could leak backup data:
///
/// 1. **Network** — loopback only (the sidecar's own listen socket).
/// 2. **Filesystem writes** — this is the one that matters most: the backup
///    text lives in the HTTP prompt bodies we send the process, so a
///    write-anywhere binary could persist every message/note to disk *inside*
///    the sandbox. We deny ALL writes and re-allow exactly one location — a
///    TraceLoupe-owned `scratch_dir` (Metal's shader cache is redirected there
///    via `MTL_SHADER_CACHE_PATH`, see `spawn`) plus `/dev/null`. Nothing the
///    process writes can land anywhere else.
/// 3. **Filesystem reads** — no user data except the model, our binary, and
///    the scratch dir.
///
/// `scratch_dir` MUST be a directory TraceLoupe creates, owns, and wipes — it
/// is the only place the sandboxed process can write, so it must never sit
/// where its contents would be treated as backup-native or persist unbounded.
pub fn sandbox_profile(model_dir: &Path, binary_dir: &Path, scratch_dir: &Path) -> String {
    format!(
        r#"(version 1)
(allow default)

;; Network: loopback only.
(deny network*)
(allow network-bind (local ip "localhost:*"))
(allow network-inbound (local ip "localhost:*"))
(allow network-outbound (remote ip "localhost:*"))

;; Filesystem writes: deny everything, then re-allow only the controlled
;; scratch dir and /dev/null. Backup text is in the prompts, so this is what
;; stops it reaching disk.
(deny file-write*)
(allow file-write* (subpath "{scratch}"))
(allow file-write-data (literal "/dev/null"))

;; Filesystem reads: no user data outside the model, our binary, and scratch.
(deny file-read* (subpath "/Users"))
(allow file-read* (subpath "{model}"))
(allow file-read* (subpath "{bin}"))
(allow file-read* (subpath "{scratch}"))
"#,
        model = sb_quote(model_dir),
        bin = sb_quote(binary_dir),
        scratch = sb_quote(scratch_dir),
    )
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub binary: PathBuf,
    pub model_path: PathBuf,
    pub port: u16,
    pub ctx_size: u32,
    /// `-1` = offload everything to the GPU (Apple Silicon default).
    pub gpu_layers: i32,
    /// Wrap the process in the Seatbelt profile (macOS only; on other
    /// platforms this flag is ignored — Safety Scan is macOS-first).
    pub sandbox: bool,
    /// The ONLY directory the sandboxed process may write to. TraceLoupe
    /// creates and owns it; Metal's shader cache is redirected here so GPU
    /// init has somewhere to write without opening the rest of the disk. Must
    /// not sit under the backup mirror or anywhere its contents would be read
    /// back as backup data.
    pub scratch_dir: PathBuf,
}

/// A running llama-server. Kills the child on Drop — an orphaned GPU server
/// eating RAM after the app quits is never acceptable.
pub struct LlamaServer {
    child: Child,
    base_url: String,
    /// Temp profile file kept alive for the child's lifetime.
    _profile: Option<tempfile::NamedTempFile>,
}

impl LlamaServer {
    pub fn spawn(cfg: &ServerConfig) -> Result<Self> {
        let model_dir = cfg
            .model_path
            .parent()
            .ok_or_else(|| Error::Inference("model path has no parent dir".into()))?;
        let binary_dir = cfg
            .binary
            .parent()
            .ok_or_else(|| Error::Inference("binary path has no parent dir".into()))?;

        let server_args = [
            "--model".as_ref(),
            cfg.model_path.as_os_str(),
            "--host".as_ref(),
            "127.0.0.1".as_ref(),
            "--port".as_ref(),
            cfg.port.to_string().as_str().as_ref(),
            "--ctx-size".as_ref(),
            cfg.ctx_size.to_string().as_str().as_ref(),
            "--n-gpu-layers".as_ref(),
            cfg.gpu_layers.to_string().as_str().as_ref(),
        ]
        .map(std::ffi::OsString::from);

        // The scratch dir must exist before the sandbox denies writes
        // everywhere else — it's the process's only writable location.
        std::fs::create_dir_all(&cfg.scratch_dir)
            .map_err(|e| Error::Inference(format!("creating sandbox scratch dir: {}", e.kind())))?;

        let use_sandbox = cfg.sandbox && cfg!(target_os = "macos");
        let (mut cmd, profile_file) = if use_sandbox {
            let profile = sandbox_profile(model_dir, binary_dir, &cfg.scratch_dir);
            let mut f = tempfile::NamedTempFile::new()
                .map_err(|e| Error::Inference(format!("sandbox profile tmp: {}", e.kind())))?;
            f.write_all(profile.as_bytes())
                .map_err(|e| Error::Inference(format!("sandbox profile write: {}", e.kind())))?;
            let mut cmd = Command::new("/usr/bin/sandbox-exec");
            cmd.arg("-f").arg(f.path()).arg(&cfg.binary);
            (cmd, Some(f))
        } else {
            (Command::new(&cfg.binary), None)
        };
        cmd.args(server_args)
            .stdin(Stdio::null())
            // Silence, not log files: server output can echo prompt text.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Point every path the runtime might write at the one allowed
            // location, so denying writes elsewhere doesn't break GPU init:
            // Metal's compiled-shader cache and any temp files land in scratch.
            .env("MTL_SHADER_CACHE_PATH", &cfg.scratch_dir)
            .env("TMPDIR", &cfg.scratch_dir);

        let child = cmd
            .spawn()
            .map_err(|e| Error::Inference(format!("spawning llama-server: {}", e.kind())))?;
        Ok(Self {
            child,
            base_url: format!("http://127.0.0.1:{}", cfg.port),
            _profile: profile_file,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Poll /health until the server answers, the child dies, or `timeout`
    /// passes. Model load for a 4–5 GB GGUF takes a while — callers should
    /// allow tens of seconds.
    pub fn wait_healthy(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(250))
            .timeout_read(Duration::from_millis(500))
            .build();
        let url = format!("{}/health", self.base_url);
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|e| Error::Inference(format!("try_wait: {}", e.kind())))?
            {
                return Err(Error::Inference(format!(
                    "llama-server exited during startup ({status})"
                )));
            }
            if agent.get(&url).call().is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Inference("llama-server health timeout".into()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// True once the child process has exited. Lets a health-poll loop bail
    /// immediately instead of spinning on a dead, already-reaped child.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Kill and reap the child. Idempotent.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake "server" binary: a shell script we control.
    fn fake_binary(dir: &Path, script_body: &str) -> PathBuf {
        let path = dir.join("fake-llama-server");
        std::fs::write(&path, format!("#!/bin/sh\n{script_body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn cfg(binary: PathBuf, port: u16) -> ServerConfig {
        ServerConfig {
            binary,
            model_path: PathBuf::from("/tmp/model-dir/model.gguf"),
            port,
            ctx_size: 4096,
            gpu_layers: -1,
            sandbox: false,
            scratch_dir: std::env::temp_dir().join("traceloupe-scratch-test"),
        }
    }

    #[test]
    fn bundled_binary_name_is_target_scoped() {
        // The sidecar name Tauri's externalBin produces per target. The full
        // resolve_binary() flow (next to current_exe, dev-only fallbacks) is
        // exercised by a real bundled build — see docs/safety-scan-plan.md.
        let name = bundled_binary_name();
        assert!(name.starts_with("llama-server-"));
        assert!(name.ends_with("-apple-darwin"));
        assert!(name.contains(std::env::consts::ARCH));
    }

    #[test]
    fn drop_kills_the_child() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_binary(tmp.path(), "sleep 30");
        let server = LlamaServer::spawn(&cfg(bin, pick_port().unwrap())).unwrap();
        let pid = server.pid();
        drop(server);
        // kill -0: succeeds only if the process still exists.
        let alive = Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success();
        assert!(!alive, "child survived Drop");
    }

    #[test]
    fn wait_healthy_detects_early_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_binary(tmp.path(), "exit 7");
        let mut server = LlamaServer::spawn(&cfg(bin, pick_port().unwrap())).unwrap();
        let err = server.wait_healthy(Duration::from_secs(5)).unwrap_err();
        assert!(err.to_string().contains("exited during startup"), "{err}");
    }

    #[test]
    fn has_exited_reports_dead_child() {
        let tmp = tempfile::tempdir().unwrap();
        // A child that exits immediately; give it a beat to actually die.
        let bin = fake_binary(tmp.path(), "exit 0");
        let mut server = LlamaServer::spawn(&cfg(bin, pick_port().unwrap())).unwrap();
        let _ = server.wait_healthy(Duration::from_secs(2)); // reaps the exit
        assert!(server.has_exited(), "a dead child must report exited");
        // Idempotent on a reaped child — this is what stops the poll loop
        // busy-spinning.
        assert!(server.has_exited());
    }

    #[test]
    fn has_exited_false_while_running() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_binary(tmp.path(), "sleep 30");
        let mut server = LlamaServer::spawn(&cfg(bin, pick_port().unwrap())).unwrap();
        assert!(!server.has_exited());
        server.shutdown();
    }

    #[test]
    fn wait_healthy_succeeds_against_listening_port() {
        // Fake server that doesn't listen; health comes from our own responder
        // on the configured port — wait_healthy only cares about the port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut s = stream;
                use std::io::{Read, Write};
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_binary(tmp.path(), "sleep 30");
        let mut server = LlamaServer::spawn(&cfg(bin, port)).unwrap();
        server.wait_healthy(Duration::from_secs(5)).unwrap();
        server.shutdown();
    }

    #[test]
    fn profile_contains_the_guarantees() {
        let p = sandbox_profile(
            Path::new("/Users/x/models"),
            Path::new("/Applications/T.app"),
            Path::new("/Users/x/scratch"),
        );
        assert!(p.contains("(deny network*)"));
        assert!(p.contains(r#"(allow network-bind (local ip "localhost:*"))"#));
        assert!(p.contains(r#"(deny file-read* (subpath "/Users"))"#));
        assert!(p.contains(r#"(allow file-read* (subpath "/Users/x/models"))"#));
        // The write containment: deny all, allow only scratch.
        assert!(p.contains("(deny file-write*)"));
        assert!(p.contains(r#"(allow file-write* (subpath "/Users/x/scratch"))"#));
    }

    // The real-sandbox tests: only meaningful on macOS with sandbox-exec.
    #[cfg(target_os = "macos")]
    mod seatbelt {
        use super::*;

        fn run_sandboxed(profile: &str, cmd: &[&str]) -> bool {
            let mut f = tempfile::NamedTempFile::new().unwrap();
            f.write_all(profile.as_bytes()).unwrap();
            Command::new("/usr/bin/sandbox-exec")
                .arg("-f")
                .arg(f.path())
                .args(cmd)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        }

        #[test]
        fn denies_reads_outside_model_dir_but_allows_inside() {
            // Both dirs under $HOME (i.e. under /Users) so the deny actually bites.
            let home = std::env::var("HOME").unwrap();
            let base = Path::new(&home)
                .join("Library/Caches")
                .join(format!("traceloupe-sandbox-test-{}", std::process::id()));
            let allowed = base.join("model");
            let denied = base.join("other");
            std::fs::create_dir_all(&allowed).unwrap();
            std::fs::create_dir_all(&denied).unwrap();
            std::fs::write(allowed.join("m.gguf"), "model").unwrap();
            std::fs::write(denied.join("secret.txt"), "secret").unwrap();

            let profile = sandbox_profile(&allowed, Path::new("/usr/bin"), &base.join("scratch"));
            let inside = run_sandboxed(
                &profile,
                &["/bin/cat", allowed.join("m.gguf").to_str().unwrap()],
            );
            let outside = run_sandboxed(
                &profile,
                &["/bin/cat", denied.join("secret.txt").to_str().unwrap()],
            );
            std::fs::remove_dir_all(&base).unwrap();
            assert!(inside, "read inside the model dir must be allowed");
            assert!(!outside, "read outside the model dir must be denied");
        }

        #[test]
        fn denies_writes_except_scratch() {
            // The containment that matters: backup text is in the prompts, so
            // the process must not be able to write it anywhere but scratch.
            let home = std::env::var("HOME").unwrap();
            let base = Path::new(&home)
                .join("Library/Caches")
                .join(format!("traceloupe-write-test-{}", std::process::id()));
            let scratch = base.join("scratch");
            let elsewhere = base.join("elsewhere");
            std::fs::create_dir_all(&scratch).unwrap();
            std::fs::create_dir_all(&elsewhere).unwrap();

            let profile = sandbox_profile(&base, Path::new("/usr/bin"), &scratch);
            // A write into scratch is allowed…
            let into_scratch = run_sandboxed(
                &profile,
                &[
                    "/bin/sh",
                    "-c",
                    &format!("echo leak > {}", scratch.join("ok.txt").display()),
                ],
            );
            // …a write anywhere else (simulating the binary persisting a prompt
            // it received) is denied.
            let into_elsewhere = run_sandboxed(
                &profile,
                &[
                    "/bin/sh",
                    "-c",
                    &format!("echo leak > {}", elsewhere.join("leak.txt").display()),
                ],
            );
            let leaked = elsewhere.join("leak.txt").exists();
            std::fs::remove_dir_all(&base).unwrap();
            assert!(into_scratch, "writing into scratch must be allowed");
            assert!(!into_elsewhere, "writing outside scratch must be denied");
            assert!(!leaked, "no file may be created outside scratch");
        }

        #[test]
        fn denies_external_network_allows_loopback() {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                let _ = listener.accept();
            });
            let profile = sandbox_profile(
                Path::new("/tmp"),
                Path::new("/usr/bin"),
                Path::new("/tmp/scratch"),
            );
            let loopback = run_sandboxed(
                &profile,
                &["/usr/bin/nc", "-z", "127.0.0.1", &port.to_string()],
            );
            // -G 2: 2s connect timeout so a *silently dropped* (vs refused)
            // connection can't hang the test.
            let external = run_sandboxed(
                &profile,
                &["/usr/bin/nc", "-z", "-G", "2", "1.1.1.1", "443"],
            );
            assert!(loopback, "loopback connect must be allowed");
            assert!(!external, "external connect must be denied by the sandbox");
        }
    }
}
