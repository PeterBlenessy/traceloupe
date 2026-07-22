//! Safety Scan Tauri commands (plan T7): model provisioning, scan lifecycle,
//! findings queries. Follows the import/security-scan wiring: blocking work on
//! spawn_blocking, progress via events, CancelToken in managed state, an async
//! gate so two scans never run concurrently.
//!
//! Events:
//! - `safetyscan://model-progress` — model download phases
//! - `safetyscan://progress`       — scan phases (loading → classifying →
//!   summarizing → done/error/cancelled)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::ActiveBackup;
use traceloupe_core::analysis::{AnalysisDb, Category};
use traceloupe_core::cache::CacheDb;
use traceloupe_core::install::InstallProgress;
use traceloupe_core::safety_scan::chunker::{ScanSources, TimeRange};
use traceloupe_core::safety_scan::{client, engine, models, server, summary};
use traceloupe_core::sidecar::CancelToken;

#[derive(Default)]
pub struct SafetyScanCancel(pub Mutex<Option<CancelToken>>);
#[derive(Default)]
pub struct SafetyDownloadCancel(pub Mutex<Option<CancelToken>>);
/// Serializes scans; `try_lock` makes a second start an error, not a queue.
#[derive(Default)]
pub struct SafetyScanGate(pub tauri::async_runtime::Mutex<()>);
/// Serializes model downloads — two concurrent downloads of the same model
/// would race on the temp file.
#[derive(Default)]
pub struct SafetyDownloadGate(pub tauri::async_runtime::Mutex<()>);

/// Live snapshot of the in-flight model download, so the UI can rehydrate after
/// a refresh (the download runs in this process and survives a webview reload,
/// but the frontend loses its state). `None` when no download is running.
#[derive(Default, Clone)]
pub struct SafetyDownloadStatus(pub Arc<Mutex<Option<DownloadSnapshot>>>);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSnapshot {
    pub model_id: String,
    pub received: u64,
    pub total: u64,
    /// "downloading" | "verifying"
    pub phase: String,
}

/// `…/caches/<id>/cache.db` → sibling `analysis.db` (survives re-import).
fn analysis_path(cache_path: &Path) -> Result<PathBuf, String> {
    Ok(cache_path
        .parent()
        .ok_or("unexpected cache layout")?
        .join("analysis.db"))
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|d| d.join("models"))
        .map_err(|e| e.to_string())
}

// ---------- model provisioning ----------

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    /// One-line role blurb (why you'd pick this model).
    pub note: String,
    pub size_bytes: u64,
    pub installed: bool,
    pub recommended: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub total_ram_bytes: u64,
    pub models: Vec<ModelInfo>,
    /// Set when a usable model is already installed (preferring the
    /// recommended tier).
    pub ready_model_id: Option<String>,
}

#[tauri::command]
pub fn get_safety_scan_model_status(app: AppHandle) -> Result<ModelStatus, String> {
    let dir = models_dir(&app)?;
    let ram = models::total_ram_bytes();
    let rec = models::recommended(ram);
    let infos: Vec<ModelInfo> = models::CATALOG
        .iter()
        .map(|s| ModelInfo {
            id: s.id.into(),
            display_name: s.display_name.into(),
            note: s.note.into(),
            size_bytes: s.size_bytes,
            installed: s.installed_at(&dir).is_some(),
            recommended: s.id == rec.id,
        })
        .collect();
    let ready = infos
        .iter()
        .filter(|m| m.installed)
        .max_by_key(|m| m.recommended)
        .map(|m| m.id.clone());
    Ok(ModelStatus {
        total_ram_bytes: ram,
        models: infos,
        ready_model_id: ready,
    })
}

/// Result of a one-shot server health check (NoteSage-style "is it actually
/// running and is the model loaded?"). Our sidecar is per-scan, not persistent,
/// so this spins one up, waits for `/health`, then shuts it down.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub ok: bool,
    pub model_id: String,
    pub display_name: String,
    /// Time from spawn to a healthy `/health` (only meaningful when `ok`).
    pub startup_ms: u64,
    /// Human-readable outcome — the success line, or the failure reason.
    pub message: String,
}

/// Spin the sandboxed llama-server up for `model_id` (or the recommended tier),
/// confirm the model loads and `/health` goes green, then tear it down. Gives
/// the user on-demand proof the local model actually runs on this Mac.
#[tauri::command]
pub async fn safety_scan_health_check(
    app: AppHandle,
    gate: State<'_, SafetyScanGate>,
    model_id: Option<String>,
) -> Result<HealthReport, String> {
    // Share the scan gate: never boot a second 5 GB server while a scan (which
    // owns the GPU/RAM budget) is in flight.
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "a Safety Scan is already running")?;

    let dir = models_dir(&app)?;
    let spec = match model_id.as_deref() {
        Some(id) => models::spec_by_id(id).ok_or("unknown model id")?,
        None => models::recommended(models::total_ram_bytes()),
    };
    let model_path = spec
        .installed_at(&dir)
        .ok_or("model not installed — download it first")?;
    let binary = server::resolve_binary().map_err(|e| e.to_string())?;
    let scratch_dir = dir.join("healthcheck-scratch");

    let spec_id = spec.id.to_string();
    let display_name = spec.display_name.to_string();
    let ctx_size = spec.ctx_size;
    let app2 = app.clone();

    let report = tauri::async_runtime::spawn_blocking(move || -> HealthReport {
        let fail = |message: String| HealthReport {
            ok: false,
            model_id: spec_id.clone(),
            display_name: display_name.clone(),
            startup_ms: 0,
            message,
        };

        crate::logging::info(&app2, format!("Safety Scan health check: model={spec_id}"));
        let _ = std::fs::remove_dir_all(&scratch_dir);
        let port = match server::pick_port() {
            Ok(p) => p,
            Err(e) => return fail(e.to_string()),
        };

        // Forward llama-server output to the dev log, same as a real scan.
        let (log_tx, log_rx) = std::sync::mpsc::channel::<String>();
        let app_log = app2.clone();
        std::thread::spawn(move || {
            while let Ok(line) = log_rx.recv() {
                crate::logging::debug(&app_log, format!("[llama-server] {line}"));
            }
        });

        let started = std::time::Instant::now();
        let mut llama = match server::LlamaServer::spawn(
            &server::ServerConfig {
                binary,
                model_path,
                port,
                ctx_size,
                gpu_layers: -1,
                sandbox: true,
                scratch_dir,
            },
            Some(log_tx),
        ) {
            Ok(s) => s,
            Err(e) => {
                crate::logging::error(
                    &app2,
                    format!("Safety Scan health check: spawn failed: {e}"),
                );
                return fail(e.to_string());
            }
        };

        // Bounded wait — a health check should fail fast, not hang for the full
        // 180s scan budget. A cold 5 GB load + Metal warmup fits in ~90s.
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        loop {
            match llama.wait_healthy(Duration::from_secs(2)) {
                Ok(()) => {
                    let startup_ms = started.elapsed().as_millis() as u64;
                    llama.shutdown();
                    crate::logging::info(
                        &app2,
                        format!("Safety Scan health check: healthy in {startup_ms} ms"),
                    );
                    return HealthReport {
                        ok: true,
                        model_id: spec_id.clone(),
                        display_name: display_name.clone(),
                        startup_ms,
                        message: format!(
                            "Server started and {display_name} loaded in {:.1}s.",
                            startup_ms as f64 / 1000.0
                        ),
                    };
                }
                Err(e) => {
                    if llama.has_exited() {
                        let tail = llama.output_tail();
                        crate::logging::error(
                            &app2,
                            format!("Safety Scan health check: {e}\n{tail}"),
                        );
                        return fail(e.to_string());
                    }
                    if std::time::Instant::now() >= deadline {
                        llama.shutdown();
                        crate::logging::error(&app2, format!("Safety Scan health check: {e}"));
                        return fail("timed out waiting for the model to load".into());
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(report)
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
enum ModelProgressEvent {
    Downloading { received: u64, total: u64 },
    Verifying,
    Done,
    Error { message: String },
}

#[tauri::command]
pub async fn download_safety_scan_model(
    app: AppHandle,
    gate: State<'_, SafetyDownloadGate>,
    cancel_state: State<'_, SafetyDownloadCancel>,
    status_state: State<'_, SafetyDownloadStatus>,
    model_id: String,
) -> Result<(), String> {
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "a model download is already running")?;
    let spec = models::spec_by_id(&model_id).ok_or("unknown model id")?;
    let dir = models_dir(&app)?;
    let cancel = CancelToken::new();
    *cancel_state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());

    // Publish a live snapshot so a refreshed UI can rehydrate this download.
    let status = status_state.0.clone();
    *status.lock().unwrap_or_else(|e| e.into_inner()) = Some(DownloadSnapshot {
        model_id: model_id.clone(),
        received: 0,
        total: spec.size_bytes,
        phase: "downloading".into(),
    });

    let app2 = app.clone();
    let status_w = status.clone();
    let model_id_c = model_id.clone();
    let join = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = std::time::Instant::now();
        models::download_model(spec, &dir, &cancel, |p| {
            let ev = match p {
                InstallProgress::Downloading { received, total } => {
                    // Status is cheap to update every tick (drives rehydration);
                    // the event is throttled (~5/s) to keep the UI light.
                    *status_w.lock().unwrap_or_else(|e| e.into_inner()) = Some(DownloadSnapshot {
                        model_id: model_id_c.clone(),
                        received,
                        total,
                        phase: "downloading".into(),
                    });
                    if last_emit.elapsed() < Duration::from_millis(200) {
                        return;
                    }
                    last_emit = std::time::Instant::now();
                    ModelProgressEvent::Downloading { received, total }
                }
                InstallProgress::Verifying => {
                    if let Some(s) = status_w.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        s.phase = "verifying".into();
                    }
                    ModelProgressEvent::Verifying
                }
                InstallProgress::Done => ModelProgressEvent::Done,
            };
            let _ = app2.emit("safetyscan://model-progress", ev);
        })
    })
    .await;

    // Clear the live snapshot on EVERY exit path — including a panicked task
    // (a JoinError from `?` below would otherwise skip this and wedge the UI
    // into a permanent, non-cancellable "downloading" state).
    *status.lock().unwrap_or_else(|e| e.into_inner()) = None;

    let result = join.map_err(|e| e.to_string())?;

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(
                "safetyscan://model-progress",
                ModelProgressEvent::Error {
                    message: msg.clone(),
                },
            );
            Err(msg)
        }
    }
}

/// The in-flight model download, if any — lets a refreshed UI rehydrate its
/// progress instead of going blank (and then colliding with the download gate).
#[tauri::command]
pub fn get_safety_scan_download_status(
    status_state: State<'_, SafetyDownloadStatus>,
) -> Option<DownloadSnapshot> {
    status_state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[tauri::command]
pub fn cancel_safety_scan_model_download(
    cancel_state: State<'_, SafetyDownloadCancel>,
) -> Result<(), String> {
    if let Some(c) = cancel_state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        c.cancel();
    }
    Ok(())
}

// ---------- scan lifecycle ----------

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
enum ScanEvent {
    Loading,
    Classifying {
        done: usize,
        total: usize,
        findings: usize,
    },
    Summarizing,
    Done {
        scan_id: i64,
        status: String,
        findings: usize,
        classified: usize,
        reused: usize,
        skipped: usize,
    },
    Error {
        message: String,
    },
}

#[tauri::command]
// A Tauri command: each param maps to a field of the JS invoke() call, so they
// stay individual rather than bundled into a struct.
#[allow(clippy::too_many_arguments)]
pub async fn run_safety_scan(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    gate: State<'_, SafetyScanGate>,
    cancel_state: State<'_, SafetyScanCancel>,
    model_id: Option<String>,
    range_start: Option<i64>,
    range_end: Option<i64>,
    // Which content to scan: "all" (default), "messages", or "notes".
    sources: Option<String>,
) -> Result<(), String> {
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "a Safety Scan is already running")?;

    let scan_sources = match sources.as_deref() {
        Some("messages") => ScanSources {
            messages: true,
            notes: false,
        },
        Some("notes") => ScanSources {
            messages: false,
            notes: true,
        },
        _ => ScanSources::default(),
    };

    let cache_path = active.path()?;
    let analysis_db_path = analysis_path(&cache_path)?;
    let dir = models_dir(&app)?;
    let spec = match model_id.as_deref() {
        Some(id) => models::spec_by_id(id).ok_or("unknown model id")?,
        None => models::recommended(models::total_ram_bytes()),
    };
    let model_path = spec
        .installed_at(&dir)
        .ok_or("model not installed — download it first")?;
    let binary = server::resolve_binary().map_err(|e| e.to_string())?;
    // The sandbox's only writable location — TraceLoupe-owned, wiped each run
    // (see below) so nothing the sidecar writes ever persists or is treated as
    // backup data.
    let scratch_dir = models_dir(&app)?.join("sidecar-scratch");

    let cancel = CancelToken::new();
    *cancel_state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());

    let app2 = app.clone();
    let spec_id = spec.id.to_string();
    let ctx_size = spec.ctx_size;
    let binary_log = binary.display().to_string();
    let model_log = model_path.display().to_string();
    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let _ = app2.emit("safetyscan://progress", ScanEvent::Loading);
        crate::logging::info(
            &app2,
            format!("Safety Scan: starting (model={spec_id}, sandbox=on)"),
        );
        crate::logging::debug(&app2, format!("Safety Scan: binary={binary_log}"));
        crate::logging::debug(&app2, format!("Safety Scan: model={model_log}"));
        // Start from a clean scratch dir; spawn() re-creates it.
        let _ = std::fs::remove_dir_all(&scratch_dir);
        let port = server::pick_port().map_err(|e| e.to_string())?;
        crate::logging::debug(&app2, format!("Safety Scan: llama-server port={port}"));

        // Forward every llama-server output line to the app log (dev console).
        let (log_tx, log_rx) = std::sync::mpsc::channel::<String>();
        let app_log = app2.clone();
        std::thread::spawn(move || {
            while let Ok(line) = log_rx.recv() {
                crate::logging::debug(&app_log, format!("[llama-server] {line}"));
            }
        });

        let mut llama = server::LlamaServer::spawn(
            &server::ServerConfig {
                binary,
                model_path,
                port,
                ctx_size,
                gpu_layers: -1,
                sandbox: true,
                scratch_dir: scratch_dir.clone(),
            },
            Some(log_tx),
        )
        .map_err(|e| {
            crate::logging::error(&app2, format!("Safety Scan: spawn failed: {e}"));
            e.to_string()
        })?;
        crate::logging::info(
            &app2,
            "Safety Scan: llama-server spawned, waiting for /health…",
        );
        // 4–5 GB GGUF load + Metal warmup: allow generous startup time, but
        // poll so cancellation during load still works.
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        loop {
            match llama.wait_healthy(Duration::from_secs(2)) {
                Ok(()) => {
                    crate::logging::info(&app2, "Safety Scan: llama-server healthy — model loaded");
                    break;
                }
                Err(e) => {
                    if cancel.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    // A dead child returns instantly on every subsequent poll;
                    // surface the failure now instead of tight-spinning to the
                    // 180s deadline (e.g. an OOM-kill during a forced-E4B load).
                    if llama.has_exited() {
                        crate::logging::error(&app2, format!("Safety Scan: {e}"));
                        return Err(e.to_string());
                    }
                    if std::time::Instant::now() >= deadline {
                        crate::logging::error(&app2, format!("Safety Scan: {e}"));
                        return Err(e.to_string());
                    }
                    // Backstop so a fast-returning error never busy-loops.
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }

        // Cancel-watcher: the engine only checks cancellation *between* chunks,
        // and one chunk is a single ~1-min blocking LLM request. So on Stop, kill
        // the model server — that drops the in-flight request immediately (its
        // retry then fails fast and the between-chunk check breaks the loop),
        // making Stop felt in a fraction of a second instead of up to a minute.
        let server_pid = llama.pid();
        let watch_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let cancel = cancel.clone();
            let done = watch_done.clone();
            let app = app2.clone();
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::SeqCst) {
                    if cancel.is_cancelled() {
                        crate::logging::info(
                            &app,
                            "Safety Scan: cancel requested — stopping the model server",
                        );
                        let _ = std::process::Command::new("/bin/kill")
                            .arg("-9")
                            .arg(server_pid.to_string())
                            .status();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(120));
                }
            })
        };

        let llm = client::LlmClient::new(
            llama.base_url(),
            &spec_id,
            // Per-chunk generation on E2B-class hardware can be slow; the
            // read timeout must comfortably exceed the worst single chunk.
            Duration::from_secs(300),
        );
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        let mut analysis = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;
        let range = TimeRange {
            start: range_start,
            end: range_end,
        };

        let mut last_emit = std::time::Instant::now();
        let outcome = engine::run_scan(
            &cache,
            &mut analysis,
            &llm,
            range,
            scan_sources,
            &cancel,
            |p| {
                // Always emit the first (done == 0) tick — it's what flips the UI from
                // "loading" to "scanning" the instant the model is ready; the 150 ms
                // throttle only smooths the frequent mid-scan updates.
                if p.chunks_done == 0
                    || last_emit.elapsed() >= Duration::from_millis(150)
                    || p.chunks_done == p.chunks_total
                {
                    last_emit = std::time::Instant::now();
                    let _ = app2.emit(
                        "safetyscan://progress",
                        ScanEvent::Classifying {
                            done: p.chunks_done,
                            total: p.chunks_total,
                            findings: p.findings,
                        },
                    );
                }
            },
        )
        .map_err(|e| e.to_string())?;

        let _ = app2.emit("safetyscan://progress", ScanEvent::Summarizing);
        summary::run_summaries(&mut analysis, &llm, outcome.scan_id, &cancel)
            .map_err(|e| e.to_string())?;
        // Stop the watcher (it may already have fired on cancel) before we take
        // the server down ourselves.
        watch_done.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = watcher.join();
        llama.shutdown();
        // Wipe scratch now (a crashed run's residue is cleared at the next
        // run's start-of-run wipe; this keeps the happy path tidy).
        let _ = std::fs::remove_dir_all(&scratch_dir);

        let _ = app2.emit(
            "safetyscan://progress",
            ScanEvent::Done {
                scan_id: outcome.scan_id,
                status: format!("{:?}", outcome.status).to_lowercase(),
                findings: outcome.findings,
                classified: outcome.classified,
                reused: outcome.reused,
                skipped: outcome.skipped,
            },
        );
        Ok(())
    })
    .await;

    // Surface an error event on BOTH a normal Err and a panicked task, so the
    // UI never sits waiting on a "loading" scan that silently died. (A stranded
    // `running` scan row is repaired by the next begin_scan.)
    let result = match join {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("scan task failed: {e}");
            let _ = app.emit(
                "safetyscan://progress",
                ScanEvent::Error {
                    message: msg.clone(),
                },
            );
            return Err(msg);
        }
    };
    if let Err(msg) = &result {
        let _ = app.emit(
            "safetyscan://progress",
            ScanEvent::Error {
                message: msg.clone(),
            },
        );
    }
    result
}

#[tauri::command]
pub fn cancel_safety_scan(cancel_state: State<'_, SafetyScanCancel>) -> Result<(), String> {
    if let Some(c) = cancel_state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        c.cancel();
    }
    Ok(())
}

// ---------- queries ----------

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentFindingDto {
    pub id: i64,
    pub source_kind: String,
    pub source_id: Option<i64>,
    /// The cache `threads.id` for message findings — the Messages deep-link.
    pub thread_id: Option<i64>,
    pub thread_identifier: Option<String>,
    pub occurred_at: Option<i64>,
    pub fingerprint: String,
    pub category: String,
    pub severity: u8,
    pub rationale: String,
    pub stale: bool,
    pub dismissed: bool,
}

#[tauri::command]
pub fn list_content_findings(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<ContentFindingDto>, String> {
    let cache_path = active.path()?;
    let path = analysis_path(&cache_path)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    // Resolve message → thread ids for deep-links (best effort; a stale
    // source_id after re-import simply yields no link).
    let cache = CacheDb::open(&cache_path).ok();
    let thread_of = |source_id: Option<i64>| -> Option<i64> {
        let (cache, id) = (cache.as_ref()?, source_id?);
        cache
            .conn()
            .query_row("SELECT thread_id FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .ok()
    };
    Ok(db
        .list_findings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|f| ContentFindingDto {
            id: f.id,
            source_kind: f.source_kind.as_str().into(),
            thread_id: if f.source_kind == traceloupe_core::analysis::SourceKind::Message {
                thread_of(f.source_id)
            } else {
                None
            },
            source_id: f.source_id,
            thread_identifier: f.thread_identifier,
            occurred_at: f.occurred_at,
            fingerprint: f.fingerprint,
            category: f.category.as_str().into(),
            severity: f.severity,
            rationale: f.rationale,
            stale: f.stale,
            dismissed: f.dismissed,
        })
        .collect())
}

/// Compact per-source severity marks for inline badges (plan T9): the top
/// live-finding severity per flagged thread and per flagged note, so the
/// Messages/Notes lists can badge rows with a single cheap query.
#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingMarks {
    /// cache `threads.id` → highest severity among that thread's live findings.
    pub threads: std::collections::HashMap<i64, u8>,
    /// cache `notes.id` → highest severity among that note's live findings.
    pub notes: std::collections::HashMap<i64, u8>,
}

#[tauri::command]
pub fn safety_scan_finding_marks(active: State<'_, ActiveBackup>) -> Result<FindingMarks, String> {
    let cache_path = active.path()?;
    let path = analysis_path(&cache_path)?;
    let mut marks = FindingMarks::default();
    if !path.exists() {
        return Ok(marks);
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let cache = CacheDb::open(&cache_path).ok();
    for f in db.list_findings().map_err(|e| e.to_string())? {
        // Dismissed and stale findings must not badge a row — the list should
        // match what the Safety Scan page shows by default.
        if f.dismissed || f.stale {
            continue;
        }
        let map = match f.source_kind {
            traceloupe_core::analysis::SourceKind::Message => {
                let Some(cache) = cache.as_ref() else {
                    continue;
                };
                let Some(id) = f.source_id else { continue };
                let thread_id: Option<i64> = cache
                    .conn()
                    .query_row("SELECT thread_id FROM messages WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })
                    .ok();
                let Some(thread_id) = thread_id else { continue };
                marks.threads.entry(thread_id)
            }
            traceloupe_core::analysis::SourceKind::Note => {
                let Some(id) = f.source_id else { continue };
                marks.notes.entry(id)
            }
        };
        map.and_modify(|s| *s = (*s).max(f.severity))
            .or_insert(f.severity);
    }
    Ok(marks)
}

#[tauri::command]
pub fn dismiss_content_finding(
    active: State<'_, ActiveBackup>,
    fingerprint: String,
    category: String,
    dismissed: bool,
) -> Result<(), String> {
    let cat = Category::parse(&category).ok_or("unknown category")?;
    let path = analysis_path(&active.path()?)?;
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    db.set_dismissed(&fingerprint, cat, dismissed, now)
        .map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatusDto {
    pub id: i64,
    pub model: String,
    pub range_start: Option<i64>,
    pub range_end: Option<i64>,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub chunks_total: i64,
    pub chunks_done: i64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyScanReport {
    pub scan: Option<ScanStatusDto>,
    pub report: Option<String>,
    /// (thread_identifier, summary) for each flagged thread.
    pub thread_summaries: Vec<(String, String)>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanHistoryItem {
    pub id: i64,
    pub range_start: Option<i64>,
    pub range_end: Option<i64>,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub findings: i64,
}

/// Past scans (newest first) for the history list.
#[tauri::command]
pub fn list_safety_scans(active: State<'_, ActiveBackup>) -> Result<Vec<ScanHistoryItem>, String> {
    let path = analysis_path(&active.path()?)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    Ok(db
        .list_scans(50)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| ScanHistoryItem {
            id: s.id,
            range_start: s.range_start,
            range_end: s.range_end,
            status: s.status,
            started_at: s.started_at,
            finished_at: s.finished_at,
            findings: s.findings,
        })
        .collect())
}

#[tauri::command]
pub fn get_safety_scan_report(active: State<'_, ActiveBackup>) -> Result<SafetyScanReport, String> {
    let path = analysis_path(&active.path()?)?;
    if !path.exists() {
        return Ok(SafetyScanReport {
            scan: None,
            report: None,
            thread_summaries: Vec::new(),
        });
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let Some(scan) = db.latest_scan().map_err(|e| e.to_string())? else {
        return Ok(SafetyScanReport {
            scan: None,
            report: None,
            thread_summaries: Vec::new(),
        });
    };
    let mut report = None;
    let mut threads = Vec::new();
    for (kind, thread_ref, content) in db.list_summaries(scan.id).map_err(|e| e.to_string())? {
        match kind.as_str() {
            "report" => report = Some(content),
            "thread" => threads.push((thread_ref, content)),
            _ => {}
        }
    }
    Ok(SafetyScanReport {
        scan: Some(ScanStatusDto {
            id: scan.id,
            model: scan.model,
            range_start: scan.range_start,
            range_end: scan.range_end,
            status: scan.status,
            started_at: scan.started_at,
            finished_at: scan.finished_at,
            chunks_total: scan.chunks_total,
            chunks_done: scan.chunks_done,
        }),
        report,
        thread_summaries: threads,
    })
}
