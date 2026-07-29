//! Make the llama-server sidecar die with TraceLoupe — and die by SIGKILL.
//!
//! # Why this exists
//!
//! `LlamaServer` kills its child on `Drop`, but **`Drop` does not run when the
//! app process itself is terminated**: a scan holds the server on a background
//! thread, and neither a signal (Ctrl-C in `tauri dev`, a closed terminal, a
//! logout SIGTERM) nor a `process::exit` from the main thread unwinds that
//! stack. The child is a separate process, so it simply survives — reparented
//! to `launchd`, still holding the GPU and several GB of the model.
//!
//! That is not hypothetical. Issue #31's crash report is exactly it: a sidecar
//! launched 23:35:39, its parent gone, `parentProc: launchd`, still *mid-token-
//! generation* when a signal finally reached it 32 minutes later.
//!
//! # Why SIGKILL, and only SIGKILL
//!
//! llama-server's own SIGTERM/SIGINT handler calls `exit()`, which runs the
//! ggml Metal teardown. With residency sets active (macOS 15+, the default)
//! that teardown does not survive being entered while inference is in flight.
//! Both outcomes are reproducible on the pinned build (`b10075`):
//!
//! - **abort** — `ggml_metal_rsets_free` fails `GGML_ASSERT([rsets->data count]
//!   == 0)`, SIGABRT, and macOS files a crash report against our sidecar;
//! - **wedge** — `exit()` blocks forever in `std::thread::join()` while a
//!   dispatch block spins in `__ggml_metal_rsets_init_block_invoke → usleep`.
//!   Measured: still alive and stuck 5 minutes after the SIGTERM.
//!
//! A wedged sidecar is the worse half: it keeps the GPU and the model's RAM and
//! never exits. So TraceLoupe must never *ask* the sidecar to shut down. It
//! SIGKILLs it, which the process cannot intercept, wedge, or abort on.
//!
//! Upstream is aware (ggml-org/llama.cpp#22593, #19137); the proposed fix
//! (PR #22595) has been open since May 2026 and addresses the missing
//! `rsets_rm` pairing, not this teardown-ordering case. Bumping
//! `LLAMA_CPP_VERSION` will not retire this module.
//!
//! # The two halves
//!
//! 1. **Isolation** (`server.rs`, `process_group(0)`) — the sidecar gets its
//!    own process group, so a signal aimed at *our* group (Ctrl-C, a closing
//!    terminal, the dev harness going away) never reaches it. Nothing but
//!    TraceLoupe can hand it a graceful signal to wedge on.
//! 2. **The reaper** (this module) — every live sidecar pid is registered
//!    process-wide and SIGKILLed from a signal handler and from `atexit`, so
//!    the app dying at all — gracefully or not — takes the sidecar with it.
//!
//! Neither half is safe alone: isolation without the reaper would turn every
//! Ctrl-C into an orphan, and the reaper without isolation would still race the
//! group signal that wedges the child before we can kill it.
//!
//! Unfixable by design: `SIGKILL` on TraceLoupe itself, and `panic = "abort"`.
//! Nothing in userspace runs at those, so nothing can clean up.

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
    use std::sync::Once;

    /// Concurrent sidecars we can track. A scan is serialized by
    /// `SafetyScanGate`, but a health check, an eval run and a scan can each
    /// hold one, so this is sized well above what is reachable — and going over
    /// only costs the reaper's coverage of the extra child, never correctness.
    const SLOTS: usize = 8;

    /// Live sidecar pids; 0 = free slot. Plain atomics, because the signal
    /// handler reads this and a `Mutex` in a signal handler can deadlock
    /// against the thread that was interrupted while holding it.
    static SIDECARS: [AtomicI32; SLOTS] = [
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
    ];

    /// The signals we take over, and where their previous handler went.
    const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
    static PREV: [AtomicUsize; 3] = [
        AtomicUsize::new(libc::SIG_DFL),
        AtomicUsize::new(libc::SIG_DFL),
        AtomicUsize::new(libc::SIG_DFL),
    ];

    static INSTALL: Once = Once::new();

    /// SIGKILL every registered sidecar and clear the registry.
    ///
    /// Async-signal-safe: `kill(2)` is on POSIX's list, and the atomics are
    /// lock-free on every target we build for. Nothing here allocates, locks,
    /// or touches stdio.
    pub fn reap_all() {
        for slot in SIDECARS.iter() {
            let pid = slot.swap(0, Ordering::SeqCst);
            if pid > 0 {
                // SAFETY: a bare pid, never a negative (process-group) target —
                // so this can only ever signal the one process we spawned.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }

    /// Reap, then let the signal do what it would have done without us.
    extern "C" fn on_signal(sig: libc::c_int) {
        reap_all();

        let prev = SIGNALS
            .iter()
            .position(|s| *s == sig)
            .map(|i| PREV[i].load(Ordering::SeqCst))
            .unwrap_or(libc::SIG_DFL);

        unsafe {
            if prev == libc::SIG_DFL {
                // Restore the default disposition and re-raise, so our exit
                // status is exactly what it would have been unhandled.
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = libc::SIG_DFL;
                libc::sigaction(sig, &sa, std::ptr::null_mut());
                libc::raise(sig);
            } else if prev != libc::SIG_IGN {
                // Someone else (Tauri, a test harness) had a handler here.
                // Chain to it rather than swallowing their signal.
                let f: extern "C" fn(libc::c_int) = std::mem::transmute(prev);
                f(sig);
            }
        }
    }

    /// Reap on a normal `exit()` / `main` return — the path a window-close quit
    /// takes, where no signal is ever delivered.
    extern "C" fn on_exit() {
        reap_all();
    }

    /// Install the handlers once, on first sidecar. Deliberately lazy: an app
    /// that never runs a Safety Scan gets no signal-disposition changes at all.
    fn install() {
        INSTALL.call_once(|| unsafe {
            for (i, sig) in SIGNALS.iter().enumerate() {
                let mut old: libc::sigaction = std::mem::zeroed();
                libc::sigaction(*sig, std::ptr::null(), &mut old);

                // A signal we inherited as ignored (`nohup`, a detached job)
                // means this process is meant to survive it — so the sidecar
                // should too. POSIX convention: leave it alone.
                if old.sa_sigaction == libc::SIG_IGN {
                    continue;
                }
                PREV[i].store(old.sa_sigaction, Ordering::SeqCst);

                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = on_signal as *const () as usize;
                libc::sigemptyset(&mut sa.sa_mask);
                sa.sa_flags = libc::SA_RESTART;
                libc::sigaction(*sig, &sa, std::ptr::null_mut());
            }
            libc::atexit(on_exit);
        });
    }

    /// Track `pid` until it is unregistered, and make sure the handlers are up.
    pub fn register(pid: i32) {
        install();
        for slot in SIDECARS.iter() {
            if slot
                .compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
        // Registry full: the child is still killed by `Drop`/`shutdown`, it
        // just loses the crash-path safety net. Silent by design — there is no
        // user-actionable event here, and this is unreachable in practice.
    }

    /// Stop tracking `pid`. Must be called once the child is dead, so a
    /// recycled pid can never be signalled by a later reap.
    pub fn unregister(pid: i32) {
        for slot in SIDECARS.iter() {
            let _ = slot.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    /// Test-only: is `pid` currently tracked?
    #[cfg(test)]
    pub fn is_registered(pid: i32) -> bool {
        SIDECARS.iter().any(|s| s.load(Ordering::SeqCst) == pid)
    }

    /// Test-only: the handler currently installed for `sig`, as a raw pointer.
    #[cfg(test)]
    pub fn installed_handler(sig: libc::c_int) -> usize {
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(sig, std::ptr::null(), &mut old);
            old.sa_sigaction
        }
    }

    /// Test-only: the address of our own handler, to compare against.
    #[cfg(test)]
    pub fn our_handler() -> usize {
        on_signal as *const () as usize
    }

    /// Test-only: force the handlers in without spawning anything.
    #[cfg(test)]
    pub fn install_for_test() {
        install();
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn reap_all() {}
    pub fn register(_pid: i32) {}
    pub fn unregister(_pid: i32) {}
}

pub use imp::*;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn spawn_sleeper() -> std::process::Child {
        Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn alive(pid: i32) -> bool {
        // kill -0: succeeds only while the process exists and is unreaped.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn reap_all_sigkills_a_registered_child() {
        // The link that matters: whatever calls reap_all() — signal handler or
        // atexit — must actually end the sidecar. Break the body of reap_all
        // and this test fails.
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        register(pid);
        assert!(alive(pid), "sleeper should be running before the reap");

        reap_all();
        // Reap the zombie so `kill -0` reports gone rather than "exists".
        let status = child.wait().unwrap();
        assert!(!alive(pid), "child survived reap_all()");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            assert_eq!(
                status.signal(),
                Some(libc::SIGKILL),
                "the sidecar must die by SIGKILL — a graceful signal wedges or aborts it"
            );
        }
    }

    #[test]
    fn reap_all_leaves_an_unregistered_child_alone() {
        // The pid-recycling guard: once we have reaped a child ourselves, a
        // later reap must not signal whatever now owns that pid number.
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        register(pid);
        unregister(pid);
        assert!(!is_registered(pid));

        reap_all();
        std::thread::sleep(Duration::from_millis(50));
        assert!(alive(pid), "an unregistered child must not be reaped");

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn register_tracks_and_unregister_clears() {
        let pid = 999_001; // never signalled — reap_all is not called here.
        assert!(!is_registered(pid));
        register(pid);
        assert!(is_registered(pid), "register must track the pid");
        unregister(pid);
        assert!(!is_registered(pid), "unregister must clear the pid");
    }

    #[test]
    fn install_takes_over_the_terminating_signals() {
        // The other link: the handler has to actually be on SIGINT/SIGTERM/
        // SIGHUP, or nothing ever calls reap_all() on a signal. Drop a signal
        // from SIGNALS and this fails for that signal.
        install_for_test();
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            assert_eq!(
                installed_handler(sig),
                our_handler(),
                "signal {sig} must route to the reaper"
            );
        }
    }
}
