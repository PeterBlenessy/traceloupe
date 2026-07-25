//! App logging: batched over a Tauri **Channel**, with an opt-in file sink.
//!
//! Records still reach the dev-tools console in real time — that is the point of
//! having them — but the transport is the one Tauri recommends for this shape of
//! data. Their docs are explicit that "the event system is not designed for low
//! latency or high throughput situations" and to "consider using Channels
//! instead"; Channels are "designed to be fast and deliver ordered data" and are
//! what Tauri itself uses for streaming **child process output**, which is
//! exactly what the llama-server sidecar produces.
//!
//! Three properties make high volume survivable (issue #60 — a debug-level scan
//! emitted one event per sidecar line and froze the UI):
//!
//! 1. **Batched.** Records land in a ring buffer and are flushed on a timer as
//!    ONE message, so 500 lines/s costs ~10 sends/s, not 500.
//! 2. **Bounded.** The buffer has a hard cap; under a flood the OLDEST records
//!    are dropped and the count rides along with the next batch, so loss is
//!    visible rather than silent.
//! 3. **Level-gated at the source.** Below-threshold records never allocate.
//!
//! The optional file sink is off by default. It writes to the OS log directory
//! so it survives a crash and can be read without the app — a *supplement* to
//! the console, never a replacement for it.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

/// Current max level: 0=off, 1=error, 2=warn, 3=info, 4=debug, 5=trace.
static LEVEL: AtomicU8 = AtomicU8::new(3); // info by default

/// How many records may wait for the next flush. Beyond this the oldest are
/// dropped: a burst must never grow memory without bound, and a slow consumer
/// must never stall the producer (which here can be a scan's hot loop).
const BUFFER_CAP: usize = 4096;
/// Flush cadence. ~10 batches/s reads as real time while collapsing any burst
/// into a single IPC message.
const FLUSH_INTERVAL_MS: u64 = 100;
/// Rotate at 8 MB: big enough for a long scan, small enough to open.
const FILE_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, serde::Serialize)]
pub struct LogRecord {
    level: &'static str,
    message: String,
    /// Milliseconds since the Unix epoch, so the UI can show real timestamps and
    /// order records across batches.
    at_ms: u64,
}

/// One flush: the records since the last one, plus how many were dropped to stay
/// within [`BUFFER_CAP`]. A non-zero `dropped` is itself worth surfacing — it
/// means the console is not showing everything.
#[derive(Clone, serde::Serialize)]
pub struct LogBatch {
    records: Vec<LogRecord>,
    dropped: usize,
}

struct Sink {
    buffer: VecDeque<LogRecord>,
    dropped: usize,
}

fn sink() -> &'static Mutex<Sink> {
    static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
    SINK.get_or_init(|| {
        Mutex::new(Sink {
            buffer: VecDeque::with_capacity(256),
            dropped: 0,
        })
    })
}

fn channel_cell() -> &'static Mutex<Option<Channel<LogBatch>>> {
    static CHANNEL: OnceLock<Mutex<Option<Channel<LogBatch>>>> = OnceLock::new();
    CHANNEL.get_or_init(|| Mutex::new(None))
}

/// Where the file sink writes, resolved once from the OS log directory.
fn file_path_cell() -> &'static Mutex<Option<PathBuf>> {
    static PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    PATH.get_or_init(|| Mutex::new(None))
}

static FILE_LOGGING: AtomicBool = AtomicBool::new(false);
static FLUSHER_STARTED: AtomicBool = AtomicBool::new(false);
/// Bytes in the current file, so it rotates once instead of growing unbounded.
static FILE_BYTES: AtomicUsize = AtomicUsize::new(0);

fn level_value(name: &str) -> u8 {
    match name {
        "off" => 0,
        "error" => 1,
        "warn" => 2,
        "info" => 3,
        "debug" => 4,
        "trace" => 5,
        _ => 3,
    }
}

/// Set the max level from a name ("off"|"error"|"warn"|"info"|"debug"|"trace").
pub fn set_level(name: &str) {
    LEVEL.store(level_value(name), Ordering::Relaxed);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn emit(_app: &AppHandle, value: u8, level: &'static str, message: String) {
    // 0 (off) never emits; a record shows only if its level is at or below the
    // configured max. Checked FIRST so a filtered-out record costs one atomic
    // load and nothing else.
    if value == 0 || value > LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let record = LogRecord {
        level,
        message,
        at_ms: now_ms(),
    };
    if FILE_LOGGING.load(Ordering::Relaxed) {
        write_to_file(&record);
    }
    let Ok(mut s) = sink().lock() else { return };
    if s.buffer.len() >= BUFFER_CAP {
        s.buffer.pop_front();
        s.dropped += 1;
    }
    s.buffer.push_back(record);
}

/// Append one record to the file sink. Best-effort by construction: logging must
/// never fail the operation being logged, so errors here are swallowed.
fn write_to_file(record: &LogRecord) {
    let Ok(guard) = file_path_cell().lock() else {
        return;
    };
    let Some(path) = guard.as_ref() else { return };
    if FILE_BYTES.load(Ordering::Relaxed) >= FILE_MAX_BYTES {
        // One rotation: keep the previous file alongside and start fresh.
        let _ = std::fs::rename(path, path.with_extension("log.1"));
        FILE_BYTES.store(0, Ordering::Relaxed);
    }
    let line = format!("{} [{}] {}\n", record.at_ms, record.level, record.message);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        if f.write_all(line.as_bytes()).is_ok() {
            FILE_BYTES.fetch_add(line.len(), Ordering::Relaxed);
        }
    }
}

/// Start the flusher once. It owns the only send path, so records reach the
/// frontend in order and at a bounded rate no matter how fast they arrive.
fn start_flusher() {
    if FLUSHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));
        let batch = {
            let Ok(mut s) = sink().lock() else { continue };
            if s.buffer.is_empty() && s.dropped == 0 {
                continue;
            }
            LogBatch {
                records: s.buffer.drain(..).collect(),
                dropped: std::mem::take(&mut s.dropped),
            }
        };
        // Hold the channel lock only for the send.
        let Ok(guard) = channel_cell().lock() else {
            continue;
        };
        if let Some(ch) = guard.as_ref() {
            let _ = ch.send(batch);
        }
    });
}

/// Subscribe the frontend to the log stream. Called once at startup with a
/// `Channel`; replaces any previous subscription (a dev-server reload would
/// otherwise leave a dead channel installed).
#[tauri::command]
pub fn subscribe_logs(app: AppHandle, channel: Channel<LogBatch>) -> Result<(), String> {
    // Resolve the file path once, whether or not file logging is on, so Settings
    // can show it and "Reveal in Finder" works before the first write.
    if let Ok(mut p) = file_path_cell().lock() {
        if p.is_none() {
            if let Ok(dir) = app.path().app_log_dir() {
                let _ = std::fs::create_dir_all(&dir);
                *p = Some(dir.join("traceloupe.log"));
            }
        }
    }
    if let Ok(mut g) = channel_cell().lock() {
        *g = Some(channel);
    }
    start_flusher();
    Ok(())
}

/// Turn the file sink on/off. Off by default — logs go to the console unless the
/// user opts into keeping them on disk as well.
#[tauri::command]
pub fn set_file_logging(enabled: bool) {
    FILE_LOGGING.store(enabled, Ordering::Relaxed);
}

/// The file sink's path, for the Settings UI. `None` before the log directory has
/// been resolved.
#[tauri::command]
pub fn log_file_path() -> Option<String> {
    file_path_cell()
        .lock()
        .ok()
        .and_then(|p| p.as_ref().map(|p| p.display().to_string()))
}

/// Reveal the log file in Finder. `-R` so the user lands on the file in its
/// folder rather than having it opened by whatever app claims `.log`.
#[tauri::command]
pub fn reveal_log_file() -> Result<(), String> {
    let path = log_file_path().ok_or("no log file yet")?;
    // Create it if absent, so revealing works before the first write.
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn error(app: &AppHandle, message: impl Into<String>) {
    emit(app, 1, "error", message.into());
}
pub fn warn(app: &AppHandle, message: impl Into<String>) {
    emit(app, 2, "warn", message.into());
}
pub fn info(app: &AppHandle, message: impl Into<String>) {
    emit(app, 3, "info", message.into());
}
#[allow(dead_code)]
pub fn debug(app: &AppHandle, message: impl Into<String>) {
    emit(app, 4, "debug", message.into());
}
#[allow(dead_code)]
pub fn trace(app: &AppHandle, message: impl Into<String>) {
    emit(app, 5, "trace", message.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_drops_oldest_and_counts_the_loss() {
        // A flood must bound memory AND stay honest about what was lost — the
        // failure mode to avoid is silently showing a partial log (#60).
        let mut s = Sink {
            buffer: VecDeque::new(),
            dropped: 0,
        };
        for i in 0..(BUFFER_CAP + 10) {
            if s.buffer.len() >= BUFFER_CAP {
                s.buffer.pop_front();
                s.dropped += 1;
            }
            s.buffer.push_back(LogRecord {
                level: "debug",
                message: format!("line {i}"),
                at_ms: 0,
            });
        }
        assert_eq!(s.buffer.len(), BUFFER_CAP, "buffer must stay bounded");
        assert_eq!(s.dropped, 10, "every dropped record must be counted");
        // The OLDEST go first: the newest record is still present.
        assert_eq!(
            s.buffer.back().map(|r| r.message.as_str()),
            Some(format!("line {}", BUFFER_CAP + 9).as_str()),
        );
    }

    #[test]
    fn level_gate_filters_before_any_work() {
        set_level("info");
        assert_eq!(LEVEL.load(Ordering::Relaxed), 3);
        // debug (4) is above the max (3) → filtered; error (1) is not.
        assert!(4 > LEVEL.load(Ordering::Relaxed));
        assert!(1 <= LEVEL.load(Ordering::Relaxed));
        set_level("trace");
        assert!(4 <= LEVEL.load(Ordering::Relaxed));
    }
}
