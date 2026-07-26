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

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::stream::ProgressStream;

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

/// The last progress event emitted for the in-flight scan, or None when no scan
/// is running. The scan lives in the Rust process and survives a webview reload;
/// this React-independent snapshot is what lets the frontend re-attach to it
/// after one, exactly as [`SafetyDownloadStatus`] does for downloads.
///
/// Without it a reload — from a crash, ⌘R, the webview respawning, or a dev-server
/// HMR reload — left the UI showing an idle "Start safety scan" while the backend
/// scanned on for hours.
#[derive(Default)]
pub struct SafetyScanStatus(pub Arc<Mutex<Option<ScanEvent>>>);

/// Re-attach to an in-flight scan after the frontend lost its state. Returns the
/// last emitted progress event, or None when nothing is running.
#[tauri::command]
pub fn get_safety_scan_status(status_state: State<'_, SafetyScanStatus>) -> Option<ScanEvent> {
    status_state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// `…/caches/<id>/cache.db` → sibling `analysis.db` (survives re-import).
pub(crate) fn analysis_path(cache_path: &Path) -> Result<PathBuf, String> {
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
                // The health check probes startup, not throughput.
                parallel: 1,
                api_key: None,
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
pub enum ModelProgressEvent {
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
            MODEL_PROGRESS.send(ev);
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
            // The error path is a stream site too — converting only the happy
            // path would have left download failures reaching nobody.
            MODEL_PROGRESS.send(ModelProgressEvent::Error {
                message: msg.clone(),
            });
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

/// Emit a scan progress event AND record it as the re-attach snapshot.
///
/// Every progress emit goes through here so the snapshot can't drift from what
/// the UI was last told. Terminal phases clear the snapshot: after Done/Error
/// there is nothing to re-attach to, and a stale snapshot would make a reloaded
/// UI show a scan that already finished.
fn emit_scan(app: &AppHandle, event: ScanEvent) {
    if let Some(state) = app.try_state::<SafetyScanStatus>() {
        let terminal = matches!(event, ScanEvent::Done { .. } | ScanEvent::Error { .. });
        let mut g = state.0.lock().unwrap_or_else(|e| e.into_inner());
        *g = if terminal { None } else { Some(event.clone()) };
    }
    SCAN_PROGRESS.send(event);
}

/// The two Safety Scan streams. One producer, one consumer each
/// (`safety-scan-provider.tsx` fans both out through React context), which is
/// what makes a Channel the right primitive — see [`crate::stream`].
static SCAN_PROGRESS: ProgressStream<ScanEvent> = ProgressStream::new();
static MODEL_PROGRESS: ProgressStream<ModelProgressEvent> = ProgressStream::new();

/// Subscribe to scan progress. Paired with `get_safety_scan_status`, which
/// re-attaches a reloaded UI to a scan already running; this carries the updates
/// from there on.
#[tauri::command]
pub fn subscribe_safety_scan_progress(channel: Channel<ScanEvent>) {
    SCAN_PROGRESS.subscribe(channel);
}

/// Subscribe to model-download progress. Paired with
/// `get_safety_scan_download_status`.
#[tauri::command]
pub fn subscribe_safety_model_progress(channel: Channel<ModelProgressEvent>) {
    MODEL_PROGRESS.subscribe(channel);
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum ScanEvent {
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
    // Resume THIS existing scan (same row, accumulating findings) instead of
    // creating a new one. Its stored range/sources are authoritative.
    resume_scan_id: Option<i64>,
) -> Result<(), String> {
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "a Safety Scan is already running")?;

    let cache_path = active.path()?;
    let analysis_db_path = analysis_path(&cache_path)?;

    // Resuming: read the scan's own stored scope rather than trusting the UI
    // to echo it back — resume means "this scan, exactly".
    let (range_start, range_end, sources) = match resume_scan_id {
        Some(id) => {
            let db = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;
            let row = db
                .scan_by_id(id)
                .map_err(|e| e.to_string())?
                .ok_or("scan to resume no longer exists")?;
            (row.range_start, row.range_end, Some(row.sources))
        }
        None => (range_start, range_end, sources),
    };

    // `sources` is "all", the legacy "messages"/"notes", or a comma-joined set
    // of the picked message services (e.g. "iMessage,TikTok") plus optionally
    // "notes" — the multi-select Content filter.
    let scan_sources = match sources.as_deref() {
        None | Some("all") | Some("") => ScanSources::default(),
        Some("messages") => ScanSources {
            notes: false,
            message_services: None,
        },
        Some("notes") => ScanSources {
            notes: true,
            message_services: Some(Vec::new()),
        },
        Some(list) => {
            let tokens: Vec<&str> = list
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            let notes = tokens.contains(&"notes");
            let all_messages = tokens.contains(&"messages");
            let services: Vec<String> = tokens
                .iter()
                .filter(|t| **t != "notes" && **t != "messages")
                .map(|t| t.to_string())
                .collect();
            ScanSources {
                notes,
                message_services: if all_messages { None } else { Some(services) },
            }
        }
    };
    // Canonicalise what we store on the row so the scope predicates match it.
    let sources_slug = scan_sources.slug();
    let dir = models_dir(&app)?;
    let spec = match model_id.as_deref() {
        Some(id) => models::spec_by_id(id).ok_or("unknown model id")?,
        None => models::recommended(models::total_ram_bytes()),
    };
    let model_path = spec
        .installed_at(&dir)
        .ok_or("model not installed — download it first")?;
    let binary = server::resolve_binary().map_err(|e| e.to_string())?;

    // Cascade (#35): when the effective model is E4B and E2B is also
    // installed, sweep everything with the fast tier and re-check flagged
    // chunks with the strong one. Single-tier machines keep one-pass behavior.
    // Computed BEFORE the row is created so the row records the SWEEP model —
    // stamping the E4B id up front would falsely claim E4B judged content even
    // when it never re-checked anything (verification Finding B).
    let cascade_sweep = if spec.id.contains("E4B") {
        models::spec_by_id("gemma-4-E2B-it-Q4_K_M").filter(|e| e.installed_at(&dir).is_some())
    } else {
        None
    };
    let primary_spec = cascade_sweep.unwrap_or(spec);
    let primary_path = primary_spec
        .installed_at(&dir)
        .ok_or("model not installed — download it first")?;
    // The strong tier's identity/path for the re-check phase (None = no cascade).
    let strong = cascade_sweep.map(|_| (spec.id.to_string(), model_path.clone(), spec.ctx_size));

    // Flip the scan row to 'running' NOW — before the slow (30–180 s) model
    // load — so the stored state and the history rail reflect the user's
    // action the moment it happens, in step with the Stop button, instead of
    // a minute later. The engine then continues this same row. If startup
    // fails below, the error path repairs the row back to 'interrupted'.
    // Seeded with the SWEEP model; a completed cascade upgrades it to
    // "e2b→e4b" only once the strong tier has actually re-checked.
    let scan_row_id = {
        let db = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;
        match resume_scan_id {
            Some(id) => {
                db.resume_scan(id, primary_spec.id)
                    .map_err(|e| e.to_string())?;
                id
            }
            None => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                db.begin_scan(
                    primary_spec.id,
                    (range_start, range_end),
                    &sources_slug,
                    now,
                )
                .map_err(|e| e.to_string())?
            }
        }
    };
    let analysis_db_path_repair = analysis_db_path.clone();

    // The sandbox's only writable location — TraceLoupe-owned, wiped each run
    // (see below) so nothing the sidecar writes ever persists or is treated as
    // backup data.
    let scratch_dir = models_dir(&app)?.join("sidecar-scratch");

    let cancel = CancelToken::new();
    *cancel_state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());

    // Concurrency: Apple Silicon inference is memory-bandwidth-bound at batch
    // 1, so a few server slots give near-linear throughput (#34). KV memory
    // scales with slots × per-slot context, so slots are gated on RAM; ≤8 GB
    // machines keep today's sequential behavior.
    let gib = 1024u64 * 1024 * 1024;
    let total_ram = models::total_ram_bytes();
    let parallel: u32 = if total_ram >= 32 * gib {
        4
    } else if total_ram >= 16 * gib {
        2
    } else {
        1
    };

    // One random bearer token for this run's server(s) — closes the loopback
    // "CORS * / no key" gap (a malicious page in the user's browser can't drive
    // the local model without it). Both the sweep and cascade-strong servers
    // use it, and every client sends it.
    let api_key = server::generate_api_key();

    let app2 = app.clone();
    let spec_id = primary_spec.id.to_string();
    // ServerConfig.ctx_size is the TOTAL across slots: keep the full per-slot
    // context at any parallelism.
    let ctx_size = primary_spec.ctx_size * parallel;
    let binary2 = binary.clone();
    let binary_log = binary.display().to_string();
    let model_log = primary_path.display().to_string();
    let model_path = primary_path;
    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        emit_scan(&app2, ScanEvent::Loading);
        // Keep the Mac awake for the whole scan. A long scan (hours) would
        // otherwise stall if the machine idle-sleeps mid-chunk: the in-flight
        // request dies, the 300 s read timeout fails the chunk on wake, and an
        // unattended run quietly stops. This RAII guard releases on EVERY exit
        // path out of this closure (normal finish, `?` early return on a spawn/
        // health/engine error, or a panic) — a superset of "where the watcher
        // stops". Only system idle sleep is held; the display still sleeps.
        let _keep_awake = crate::power::KeepAwake::prevent_idle_sleep("TraceLoupe Safety Scan");
        crate::logging::info(
            &app2,
            format!("Safety Scan: starting (model={spec_id}, sandbox=on, parallel={parallel})"),
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
                parallel,
                api_key: api_key.clone(),
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
        //
        // ONE watcher reads the CURRENT server pid from a shared atomic that the
        // cascade swap updates — so after the sweep→strong swap it never kills
        // the retired (possibly OS-reused) sweep pid (verification Finding C).
        let current_pid = Arc::new(std::sync::atomic::AtomicU32::new(llama.pid()));
        let watch_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let cancel = cancel.clone();
            let done = watch_done.clone();
            let app = app2.clone();
            let current_pid = current_pid.clone();
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::SeqCst) {
                    if cancel.is_cancelled() {
                        crate::logging::info(
                            &app,
                            "Safety Scan: cancel requested — stopping the model server",
                        );
                        let _ = std::process::Command::new("/bin/kill")
                            .arg("-9")
                            .arg(
                                current_pid
                                    .load(std::sync::atomic::Ordering::SeqCst)
                                    .to_string(),
                            )
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
        )
        .with_api_key(api_key.clone());
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        let mut analysis = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;
        let range = TimeRange {
            start: range_start,
            end: range_end,
        };

        // Cascade provider: called by the engine AFTER the sweep. Spawns the
        // strong tier on a NEW port and only tears the sweep server down once
        // the strong one is confirmed healthy — so if the strong model can't
        // load (e.g. it doesn't fit in RAM on top of the sweep model), the
        // sweep server stays alive: the engine keeps the sweep verdicts and
        // the summary still has a live server to talk to (Finding 2). A new
        // cancel watcher for the strong pid keeps Stop fast.
        let mut provider = strong.map(|(strong_id, strong_path, strong_ctx)| {
            let (llama_ref, app3, scratch2, binary3) = (
                &mut llama,
                app2.clone(),
                scratch_dir.clone(),
                binary2.clone(),
            );
            let (cancel2, current_pid2) = (cancel.clone(), current_pid.clone());
            let api_key2 = api_key.clone();
            move || -> traceloupe_core::Result<client::LlmClient> {
                use traceloupe_core::Error;
                let inf = |m: String| Error::Inference(m);
                crate::logging::info(
                    &app3,
                    format!("Safety Scan: cascade re-check — loading {strong_id}"),
                );
                let port = server::pick_port()?;
                let (log_tx, log_rx) = std::sync::mpsc::channel::<String>();
                let app_log = app3.clone();
                std::thread::spawn(move || {
                    while let Ok(line) = log_rx.recv() {
                        crate::logging::debug(&app_log, format!("[llama-server] {line}"));
                    }
                });
                // Spawn into a LOCAL: the sweep server (in *llama_ref) stays up
                // until this one is healthy. On any error below, `strong` is
                // dropped (its Drop kills it) and *llama_ref is untouched.
                let mut strong = server::LlamaServer::spawn(
                    &server::ServerConfig {
                        binary: binary3.clone(),
                        model_path: strong_path.clone(),
                        port,
                        ctx_size: strong_ctx * parallel,
                        parallel,
                        api_key: api_key2.clone(),
                        gpu_layers: -1,
                        sandbox: true,
                        scratch_dir: scratch2.clone(),
                    },
                    Some(log_tx),
                )?;
                let deadline = std::time::Instant::now() + Duration::from_secs(180);
                loop {
                    match strong.wait_healthy(Duration::from_secs(2)) {
                        Ok(()) => break,
                        Err(e) => {
                            if cancel2.is_cancelled() {
                                return Err(inf("cancelled".into()));
                            }
                            if strong.has_exited() || std::time::Instant::now() >= deadline {
                                return Err(e);
                            }
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
                // Strong tier is healthy: NOW retire the sweep server and take
                // ownership of the strong one.
                let strong_pid = strong.pid();
                llama_ref.shutdown();
                *llama_ref = strong;
                // Point the (single) cancel watcher at the strong pid before it
                // can fire on the now-reaped sweep pid — must happen AFTER the
                // sweep shutdown so a cancel in this window still hits a live
                // server, not the gap.
                current_pid2.store(strong_pid, std::sync::atomic::Ordering::SeqCst);
                if cancel2.is_cancelled() {
                    // A cancel that landed during the swap: the watcher may have
                    // already killed the old pid; make sure the new server dies.
                    llama_ref.shutdown();
                    return Err(inf("cancelled".into()));
                }
                Ok(client::LlmClient::new(
                    llama_ref.base_url(),
                    &strong_id,
                    Duration::from_secs(300),
                )
                .with_api_key(api_key2.clone()))
            }
        });

        let mut last_emit = std::time::Instant::now();
        let outcome = engine::run_scan(
            &cache,
            &mut analysis,
            &llm,
            range,
            scan_sources,
            // The command already created/reopened the row (above) so the UI
            // saw it flip immediately; the engine continues that same row.
            Some(scan_row_id),
            parallel as usize,
            provider
                .as_mut()
                .map(|p| p as &mut dyn FnMut() -> traceloupe_core::Result<client::LlmClient>),
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
                    emit_scan(
                        &app2,
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

        // A cascade swaps `llama` to the strong server, killing the sweep
        // server `llm` was built against — so drop the provider (releasing its
        // &mut llama) and rebuild the summary client from whatever server is
        // live now. Without a cascade this is the same server, just rebuilt.
        drop(provider);
        let summary_client =
            client::LlmClient::new(llama.base_url(), &spec_id, Duration::from_secs(300))
                .with_api_key(api_key.clone());

        emit_scan(&app2, ScanEvent::Summarizing);
        // Best-effort: the classification is done and findings are saved, so a
        // summary failure (e.g. the only live server died) must NOT fail the
        // whole scan and discard a completed result — log it and move on. The
        // report simply won't exist; the UI already handles "no written
        // report", and a later resume regenerates it.
        if let Err(e) =
            summary::run_summaries(&mut analysis, &summary_client, outcome.scan_id, &cancel)
        {
            crate::logging::warn(&app2, format!("Safety Scan: report summary skipped — {e}"));
        }
        // Stop the watcher (it may already have fired on cancel) before we take
        // the server down ourselves.
        watch_done.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = watcher.join();
        llama.shutdown();
        // Wipe scratch now (a crashed run's residue is cleared at the next
        // run's start-of-run wipe; this keeps the happy path tidy).
        let _ = std::fs::remove_dir_all(&scratch_dir);

        emit_scan(
            &app2,
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
    // UI never sits waiting on a "loading" scan that silently died. The row was
    // flipped to 'running' before startup, so a failure before/inside the
    // engine must repair it back to 'interrupted' (best effort; the next
    // backup open is the backstop).
    let repair_on_error = || {
        if let Ok(db) = AnalysisDb::open(&analysis_db_path_repair) {
            let _ = db.repair_stranded_scans();
        }
    };
    let result = match join {
        Ok(r) => r,
        Err(e) => {
            repair_on_error();
            let msg = format!("scan task failed: {e}");
            emit_scan(
                &app,
                ScanEvent::Error {
                    message: msg.clone(),
                },
            );
            return Err(msg);
        }
    };
    if let Err(msg) = &result {
        repair_on_error();
        emit_scan(
            &app,
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
    /// The messaging service for the app icon (e.g. "iMessage", "TikTok"), or
    /// "Notes" for note findings. None when the source can't be resolved.
    pub service: Option<String>,
    pub occurred_at: Option<i64>,
    pub fingerprint: String,
    pub category: String,
    pub severity: u8,
    pub rationale: String,
    pub stale: bool,
    pub dismissed: bool,
    /// True when the cascade's strong tier (E4B) re-checked and kept this
    /// finding — the honest "confidence" signal (two independent models agree),
    /// vs a sweep-only (E2B, unconfirmed) flag.
    pub rechecked: bool,
}

/// Generate (or return) ONE thread's summary on demand (#18).
///
/// Scan end only writes prose for the top few threads by severity, so this fills
/// in the rest when the user opens one. Cached results are free and survive
/// re-scans; with no model server live it returns a deterministic summary built
/// from the findings rather than an error, so the UI never has to render an empty
/// panel or explain a failure. `source` tells the UI which it got.
#[tauri::command]
pub async fn generate_thread_summary(
    active: State<'_, ActiveBackup>,
    scan_id: i64,
    thread_ref: String,
) -> Result<Option<ThreadSummaryDto>, String> {
    let analysis_db_path = analysis_path(&active.path()?)?;
    // Opening the DB and (in the deterministic case) hashing findings is blocking
    // work; keep it off the async executor.
    tauri::async_runtime::spawn_blocking(move || {
        let mut db = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;
        // No client: an idle app has no sidecar, and spawning a 4–5 GB model for one
        // 250-token call would cost the user 30–180 s. The deterministic summary is
        // the honest, instant answer — see the ADR note on why warm-server reuse
        // was rejected.
        let out = summary::summarize_thread_on_demand(&mut db, None, scan_id, &thread_ref)
            .map_err(|e| e.to_string())?;
        Ok(out.map(|(content, source)| ThreadSummaryDto {
            thread_ref,
            content,
            source: format!("{source:?}").to_lowercase(),
        }))
    })
    .await
    .map_err(|e| format!("summary task failed: {e}"))?
}

/// One thread's summary plus how it was produced ("cached" | "model" |
/// "deterministic") — the UI labels model prose differently from the
/// deterministic fallback rather than passing one off as the other.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummaryDto {
    pub thread_ref: String,
    pub content: String,
    pub source: String,
}

/// Findings, newest-severity first. `scan_id` restricts to one scan (the
/// history view shows the selected scan's findings); None returns all.
#[tauri::command]
pub fn list_content_findings(
    active: State<'_, ActiveBackup>,
    scan_id: Option<i64>,
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
    // For message findings, resolve the thread id (deep-link) AND the service
    // (app icon) in one lookup; best effort — a stale source_id yields neither.
    let thread_meta = |source_id: Option<i64>| -> (Option<i64>, Option<String>) {
        let Some((cache, id)) = cache.as_ref().zip(source_id) else {
            return (None, None);
        };
        cache
            .conn()
            .query_row(
                "SELECT m.thread_id, t.service FROM messages m \
                 LEFT JOIN threads t ON t.id = m.thread_id WHERE m.id = ?1",
                [id],
                |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .unwrap_or((None, None))
    };
    // A scan shows every finding within its SCOPE (sources + time range), not
    // just the ones its own run classified — classification is cached per chunk
    // across scans, so scoping by scan_id makes a re-scan of already-covered data
    // look empty. None (no scan selected) still returns all findings.
    let findings = match scan_id {
        Some(id) => match db.scan_by_id(id).map_err(|e| e.to_string())? {
            Some(s) => db.list_findings_in_scope(&s.sources, s.range_start, s.range_end),
            None => db.list_findings(Some(id)),
        },
        None => db.list_findings(None),
    }
    .map_err(|e| e.to_string())?;
    Ok(findings
        .into_iter()
        .map(|f| {
            let is_message = f.source_kind == traceloupe_core::analysis::SourceKind::Message;
            let (thread_id, service) = if is_message {
                thread_meta(f.source_id)
            } else {
                // Note findings all come from Apple Notes.
                (None, Some("Notes".to_string()))
            };
            ContentFindingDto {
                id: f.id,
                source_kind: f.source_kind.as_str().into(),
                thread_id,
                service,
                source_id: f.source_id,
                thread_identifier: f.thread_identifier,
                occurred_at: f.occurred_at,
                fingerprint: f.fingerprint,
                category: f.category.as_str().into(),
                severity: f.severity,
                rationale: f.rationale,
                stale: f.stale,
                dismissed: f.dismissed,
                rechecked: f.rechecked,
            }
        })
        .collect())
}

/// A flagged source, fetched from the cache ON DEMAND for the peek popover.
///
/// This is on-device, on-demand UI content and is never persisted. When report
/// EXPORT is built, `text` must be gated behind a user setting (default OFF) so
/// verbatim flagged content isn't baked into a shareable file — see issue #38.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingSnippet {
    /// The flagged text (message body, or note title + stripped body).
    pub text: String,
    /// Who sent it: "Me" for the device owner, else the handle/name. None for
    /// notes.
    pub sender: Option<String>,
    /// The other side of the conversation (thread name/handle) — shown as
    /// "Me → recipient" when the device owner's own message is flagged. None
    /// for notes.
    pub recipient: Option<String>,
    /// When it was sent (unix seconds), for the popover header. None for notes.
    pub occurred_at: Option<i64>,
    /// The service for the app icon ("iMessage"/"TikTok"/…), "Notes" for notes.
    pub service: Option<String>,
}

/// The flagged source for a finding, fetched from the cache ON DEMAND (never
/// stored in analysis.db — ADR 0002 keeps raw text out of the analysis store).
/// Returns None when the source row is gone or its id is stale after a
/// re-import, so the UI can say "source no longer available" instead of lying.
#[tauri::command]
pub fn content_finding_snippet(
    active: State<'_, ActiveBackup>,
    source_kind: String,
    source_id: Option<i64>,
) -> Result<Option<FindingSnippet>, String> {
    let cache_path = active.path()?;
    let Some(id) = source_id else {
        return Ok(None);
    };
    let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
    let snippet = match source_kind.as_str() {
        "message" => cache
            .conn()
            .query_row(
                "SELECT m.body, m.sender, m.is_from_me, m.sent_at, t.service, \
                 t.display_name, t.identifier \
                 FROM messages m LEFT JOIN threads t ON t.id = m.thread_id \
                 WHERE m.id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .ok()
            .and_then(
                |(body, sender, is_from_me, sent_at, service, display_name, identifier)| {
                    let text = body?.trim().to_string();
                    if text.is_empty() {
                        return None;
                    }
                    let sender = if is_from_me {
                        Some("Me".to_string())
                    } else {
                        sender.filter(|s| !s.trim().is_empty())
                    };
                    // The conversation's name/handle, for "Me → recipient".
                    let recipient = display_name.filter(|s| !s.trim().is_empty()).or(identifier);
                    Some(FindingSnippet {
                        text,
                        sender,
                        recipient,
                        occurred_at: sent_at,
                        service,
                    })
                },
            ),
        "note" => cache
            .conn()
            .query_row(
                "SELECT title, body_html FROM notes WHERE id = ?1 AND locked = 0",
                [id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .ok()
            .and_then(|(title, body)| {
                let body = traceloupe_core::safety_scan::chunker::html_to_text(
                    body.as_deref().unwrap_or(""),
                );
                let text = match title {
                    Some(t) if !t.trim().is_empty() => format!("{t}\n{body}"),
                    _ => body,
                };
                let text = text.trim().to_string();
                if text.is_empty() {
                    return None;
                }
                Some(FindingSnippet {
                    text,
                    sender: None,
                    recipient: None,
                    occurred_at: None,
                    service: Some("Notes".to_string()),
                })
            }),
        _ => None,
    };
    Ok(snippet)
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
    for f in db.list_findings(None).map_err(|e| e.to_string())? {
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
    pub model: String,
    pub range_start: Option<i64>,
    pub range_end: Option<i64>,
    /// Which content the scan covered: 'all' | 'messages' | 'notes'.
    pub sources: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub findings: i64,
    /// Live finding counts by severity (3=serious, 2=harmful, 1=concerning).
    pub serious: i64,
    pub harmful: i64,
    pub concerning: i64,
}

/// Remove a past scan and everything scoped to it (findings, progress,
/// summaries). Dismissals survive so a re-scan still honours them.
#[tauri::command]
pub fn delete_safety_scan(active: State<'_, ActiveBackup>, scan_id: i64) -> Result<(), String> {
    let path = analysis_path(&active.path()?)?;
    if !path.exists() {
        return Ok(());
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    db.delete_scan(scan_id).map_err(|e| e.to_string())
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
            model: s.model,
            range_start: s.range_start,
            range_end: s.range_end,
            sources: s.sources,
            status: s.status,
            started_at: s.started_at,
            finished_at: s.finished_at,
            findings: s.findings,
            serious: s.serious,
            harmful: s.harmful,
            concerning: s.concerning,
        })
        .collect())
}

#[tauri::command]
pub fn get_safety_scan_report(
    active: State<'_, ActiveBackup>,
    scan_id: Option<i64>,
) -> Result<SafetyScanReport, String> {
    let path = analysis_path(&active.path()?)?;
    if !path.exists() {
        return Ok(SafetyScanReport {
            scan: None,
            report: None,
            thread_summaries: Vec::new(),
        });
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    // A specific past scan when the history list asks for one; otherwise latest.
    let looked_up = match scan_id {
        Some(id) => db.scan_by_id(id).map_err(|e| e.to_string())?,
        None => db.latest_scan().map_err(|e| e.to_string())?,
    };
    let Some(scan) = looked_up else {
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
