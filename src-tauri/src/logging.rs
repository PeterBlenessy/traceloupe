//! Lightweight app logging piped to the webview dev-tools console.
//!
//! Records are emitted on the `app://log` event; the frontend prints them to
//! `console.*`. The max level is runtime-adjustable from Settings via
//! [`set_level`] (the `set_log_level` command), so a user can turn on debug/trace
//! timing without a rebuild. Cheap: below-threshold records short-circuit before
//! any serialization or IPC.

use std::sync::atomic::{AtomicU8, Ordering};

use tauri::{AppHandle, Emitter};

/// Current max level: 0=off, 1=error, 2=warn, 3=info, 4=debug, 5=trace.
static LEVEL: AtomicU8 = AtomicU8::new(3); // info by default

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

#[derive(Clone, serde::Serialize)]
struct LogRecord {
    level: &'static str,
    message: String,
}

fn emit(app: &AppHandle, value: u8, level: &'static str, message: String) {
    // 0 (off) never emits; a record shows only if its level is at or below the
    // configured max (error=1 is always shown unless off).
    if value == 0 || value > LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let _ = app.emit("app://log", LogRecord { level, message });
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
