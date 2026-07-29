//! Manual verification for issue #31: does the real llama-server sidecar die
//! cleanly when TraceLoupe is torn down mid-inference?
//!
//! The unit tests in `safety_scan::server` / `safety_scan::reaper` prove the
//! mechanism against a fake binary; this proves it against the actual GPU
//! process, which is where the Metal residency-set abort/wedge lives. It needs
//! a staged sidecar and a real multi-GB model, so it can never run in CI.
//!
//! Two teardown shapes, both of which left a live sidecar before the fix:
//!
//! - `signal` — block until something signals us. Stands in for Ctrl-C, a
//!   closing terminal, or a logout SIGTERM reaching the whole process group.
//! - `exit` — a scan thread holds the server while the main thread calls
//!   `process::exit`. Stands in for a window-close quit, and is exactly what
//!   issue #31's crash report captured: the sidecar reparented to `launchd`,
//!   still generating tokens 32 minutes later. `Drop` cannot help here — the
//!   scan thread's stack is never unwound.
//!
//! ```text
//! cargo run -p traceloupe-core --example sidecar_teardown -- <model.gguf> [signal|exit]
//! ```
//!
//! Expected after the fix: the sidecar is gone and macOS files no crash
//! report. `scripts/verify-sidecar-teardown.sh` drives both and checks that.

use std::path::PathBuf;
use std::time::Duration;

use traceloupe_core::safety_scan::server::{self, LlamaServer, ServerConfig};

fn main() {
    let model_path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: sidecar_teardown <model.gguf>"),
    );
    let port = server::pick_port().expect("free port");
    let cfg = ServerConfig {
        binary: server::resolve_binary().expect("staged llama-server (pnpm setup:llama)"),
        model_path,
        port,
        ctx_size: 4096,
        parallel: 1,
        api_key: None,
        gpu_layers: -1,
        // The sandbox is orthogonal to teardown and needs a scratch dir we do
        // not have here; the lifetime behaviour under test is identical.
        sandbox: false,
        scratch_dir: std::env::temp_dir().join("traceloupe-sidecar-teardown"),
    };

    let mut server = LlamaServer::spawn(&cfg, None).expect("spawn");
    println!("SIDECAR_PID={}", server.pid());
    server
        .wait_healthy(Duration::from_secs(300))
        .expect("healthy");
    println!("HEALTHY");

    // Put inference in flight, so a teardown lands where the crash report says
    // it landed: inside the Metal command-buffer wait, not on an idle server.
    let url = format!("{}/completion", server.base_url());
    std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(600))
            .build();
        loop {
            let _ = agent.post(&url).set("Content-Type", "application/json").send_string(
                r#"{"prompt":"Write a long detailed essay about the history of computing.","n_predict":600}"#,
            );
        }
    });
    std::thread::sleep(Duration::from_secs(4));
    println!("GENERATING");

    match std::env::args()
        .nth(2)
        .unwrap_or_else(|| "signal".into())
        .as_str()
    {
        // Hold the server here and block. A signal now takes the process down
        // WITHOUT unwinding — which is precisely why `Drop` cannot be the
        // thing that stops the sidecar.
        "signal" => loop {
            std::thread::sleep(Duration::from_secs(3600));
            let _ = &server;
        },
        // A scan thread owns the server; the main thread quits under it. No
        // unwinding, no `Drop`, no signal — the child just carries on.
        "exit" => {
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(3600));
                let _ = &server;
            });
            std::thread::sleep(Duration::from_millis(200));
            println!("EXITING");
            std::process::exit(0);
        }
        other => panic!("unknown mode {other:?} — expected signal|exit"),
    }
}
