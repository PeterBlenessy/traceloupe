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
use traceloupe_core::analysis::{AnalysisDb, Category, ChartBucket, FindingQuery, FindingSort};
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
                embedding: false,
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
        let terminal = matches!(
            event,
            ScanEvent::Done { .. } | ScanEvent::TriageDone { .. } | ScanEvent::Error { .. }
        );
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
        /// Findings in this scan's scope right now — earlier runs included.
        findings: usize,
        /// How many of those were already there when this run started.
        preexisting: usize,
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
    // --- triage scan phases (#472). Same stream, same snapshot: one scan of
    // either kind runs at a time, and the re-attach path must see whichever it
    // is. ---
    /// The embedding census: `done` of `total` messages scored this run.
    Censusing { done: usize, total: usize },
    /// Focused deep-scan of the ranked worklist.
    DeepScanning {
        done: usize,
        total: usize,
        /// Provisional findings so far (pre-confirmation).
        findings: usize,
    },
    /// Confirmation of provisional findings (confirm-on modes only).
    Confirming { done: usize, total: usize },
    /// Terminal event of a triage scan, with the honest coverage numbers.
    TriageDone {
        scan_id: i64,
        status: String,
        findings: usize,
        censused: usize,
        candidates: usize,
        deep_scanned: usize,
        /// Candidates the budget left unread — reported, never called clean.
        unscanned: usize,
        unconfirmed: usize,
    },
    Error {
        message: String,
    },
}

/// Parse the UI's `sources` string. It is "all", the legacy
/// "messages"/"notes", a `thread:<identifier>` single-conversation scope, or a
/// comma-joined set of the picked message services (e.g. "iMessage,TikTok")
/// plus optionally "notes" — the multi-select Content filter. Shared by the
/// batch and triage scan commands so a scope means the same thing to both.
fn parse_scan_sources(sources: Option<&str>) -> ScanSources {
    match sources {
        None | Some("all") | Some("") => ScanSources::default(),
        // One conversation. Everything after the prefix is the thread
        // identifier verbatim — it may contain colons (TikTok) or commas, so it
        // is never split.
        Some(t) if t.starts_with("thread:") => ScanSources {
            thread: Some(t["thread:".len()..].to_string()),
            notes: false,
            message_services: None,
        },
        Some("messages") => ScanSources {
            thread: None,
            notes: false,
            message_services: None,
        },
        Some("notes") => ScanSources {
            thread: None,
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
                thread: None,
                notes,
                message_services: if all_messages { None } else { Some(services) },
            }
        }
    }
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

    let scan_sources = parse_scan_sources(sources.as_deref());
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

    // The cascade (#35) is OFF (#446). It swept with E2B and re-checked the
    // flagged chunks with E4B, which is only worth a second model load and a
    // second pass if the sweep is fast and high-recall. Measured on an M3, three
    // runs per tier (docs/validation/safety-scan-validation.md), E2B is neither:
    //
    //   per chunk          E4B 7.5-8.9s     E2B 10.3-12.0s
    //   harassment P/R     E4B 0.50/1.00    E2B 0.00/0.00
    //   findings on 200 generated mundane messages   E4B 6   E2B 10
    //   chunks failing to classify                   E4B 0/8  E2B 1/8
    //
    // A sweep miss is permanent — the strong tier only ever sees what the sweep
    // flagged — so E2B's zero recall on harassment-bullying meant a two-tier
    // machine could not surface a harassment finding at all. The pairing cost
    // wall clock AND findings.
    //
    // `run_scan` still takes a `recheck` provider and the engine still tests it:
    // the mechanism is sound, the PAIRING was wrong. A future tier that is
    // genuinely faster and higher-recall can be wired back in here.
    let strong: Option<(String, std::path::PathBuf, u32)> = None;
    let primary_spec = spec;
    let primary_path = model_path.clone();

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
                embedding: false,
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
                        embedding: false,
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
                            preexisting: p.preexisting,
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

// ---------- triage scan (#472) ----------

/// Spawn a sandboxed llama-server and wait for `/health`, forwarding its output
/// to the app log. The startup poll honours cancellation and fails fast when
/// the child dies (e.g. an OOM-kill during load) instead of spinning to the
/// deadline. Shared by the triage scan's initial (embedder) spawn and every
/// healthy-swap after it.
#[allow(clippy::too_many_arguments)]
fn spawn_server_healthy(
    app: &AppHandle,
    binary: &Path,
    model_path: &Path,
    ctx_size: u32,
    embedding: bool,
    api_key: &Option<String>,
    scratch_dir: &Path,
    cancel: &CancelToken,
) -> Result<server::LlamaServer, String> {
    let port = server::pick_port().map_err(|e| e.to_string())?;
    let (log_tx, log_rx) = std::sync::mpsc::channel::<String>();
    let app_log = app.clone();
    std::thread::spawn(move || {
        while let Ok(line) = log_rx.recv() {
            crate::logging::debug(&app_log, format!("[llama-server] {line}"));
        }
    });
    let mut llama = server::LlamaServer::spawn(
        &server::ServerConfig {
            binary: binary.to_path_buf(),
            model_path: model_path.to_path_buf(),
            port,
            ctx_size,
            // The triage pipeline is sequential by design (one focused call at
            // a time), so extra server slots would only cost KV memory.
            parallel: 1,
            api_key: api_key.clone(),
            gpu_layers: -1,
            embedding,
            sandbox: true,
            scratch_dir: scratch_dir.to_path_buf(),
        },
        Some(log_tx),
    )
    .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        match llama.wait_healthy(Duration::from_secs(2)) {
            Ok(()) => return Ok(llama),
            Err(e) => {
                if cancel.is_cancelled() {
                    return Err("cancelled".into());
                }
                if llama.has_exited() || std::time::Instant::now() >= deadline {
                    return Err(e.to_string());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// The triage scan's one resident sidecar and the client bound to it. The
/// pipeline needs two models — the embedder for the census, the classifier for
/// the deep-scan — but never at once (they are multi-GB each), so the slot
/// healthy-swaps: the replacement is spawned on a new port and confirmed
/// healthy BEFORE the incumbent is shut down, exactly like the cascade's
/// sweep→strong swap this is derived from. On a failed swap the incumbent
/// stays up and the scan fails with the census already persisted.
struct TriageSidecar {
    llama: server::LlamaServer,
    client: client::LlmClient,
    role: models::ModelRole,
    // Swap ingredients.
    app: AppHandle,
    binary: PathBuf,
    scratch_dir: PathBuf,
    api_key: Option<String>,
    cancel: CancelToken,
    /// The single cancel-watcher kills whatever pid is stored here; a swap must
    /// repoint it AFTER the old server is down (see the cascade notes).
    current_pid: Arc<std::sync::atomic::AtomicU32>,
}

impl TriageSidecar {
    /// Ensure the CLASSIFIER is the resident model, swapping the embedder out
    /// on the first call and doing nothing on every later one.
    fn ensure_classifier(
        &mut self,
        spec: &models::ModelSpec,
        model_path: &Path,
    ) -> traceloupe_core::Result<()> {
        use traceloupe_core::Error;
        if self.role == models::ModelRole::Classifier {
            return Ok(());
        }
        crate::logging::info(
            &self.app,
            format!("Triage scan: census done — swapping to {}", spec.id),
        );
        let mut next = spawn_server_healthy(
            &self.app,
            &self.binary,
            model_path,
            spec.ctx_size,
            false,
            &self.api_key,
            &self.scratch_dir,
            &self.cancel,
        )
        .map_err(Error::Inference)?;
        // The classifier is healthy: NOW retire the embedder, then repoint the
        // watcher — after the shutdown, so a cancel in this window still hits a
        // live server rather than the gap.
        let next_pid = next.pid();
        self.llama.shutdown();
        std::mem::swap(&mut self.llama, &mut next);
        self.current_pid
            .store(next_pid, std::sync::atomic::Ordering::SeqCst);
        if self.cancel.is_cancelled() {
            // A cancel that landed during the swap may have killed the retired
            // pid; make sure the new server dies too.
            self.llama.shutdown();
            return Err(Error::Inference("cancelled".into()));
        }
        self.client = client::LlmClient::new(
            self.llama.base_url(),
            spec.id,
            Duration::from_secs(300),
        )
        .with_api_key(self.api_key.clone());
        self.role = models::ModelRole::Classifier;
        Ok(())
    }
}

#[tauri::command]
// A Tauri command: each param maps to a field of the JS invoke() call.
#[allow(clippy::too_many_arguments)]
pub async fn run_triage_scan(
    app: AppHandle,
    active: State<'_, ActiveBackup>,
    gate: State<'_, SafetyScanGate>,
    cancel_state: State<'_, SafetyScanCancel>,
    // The CLASSIFIER tier; None picks the RAM-recommended one. The embedder is
    // not a choice — the catalog has exactly one census model.
    model_id: Option<String>,
    // "thorough" | "balanced" | "precise"; None = the product default.
    mode: Option<String>,
    range_start: Option<i64>,
    range_end: Option<i64>,
    sources: Option<String>,
    // Deep-scan budget: at most this many worklist items are classified; the
    // rest are reported unscanned. None = every candidate.
    budget: Option<usize>,
) -> Result<(), String> {
    use traceloupe_core::safety_scan::chunker;
    use traceloupe_core::safety_scan::triage::{self, FocusWindow, ScanMode};
    use traceloupe_core::safety_scan::triage_scan::{self, FocusVerdict, TriageProgress};

    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "a Safety Scan is already running")?;

    let cache_path = active.path()?;
    let analysis_db_path = analysis_path(&cache_path)?;

    let mode = match mode.as_deref() {
        None => ScanMode::default(),
        Some(s) => ScanMode::parse(s).ok_or("unknown scan mode")?,
    };
    // Balanced/Precise promise a second-model confirmation of every finding
    // (that is the mode's whole meaning — it trims recall to buy precision).
    // The confirmer tier is not in the catalog yet, and silently skipping the
    // stage would ship a mode that does not do what it says.
    if mode.confirm() {
        return Err(format!(
            "the {} mode confirms findings with a second model, which is not installed yet — \
             run a thorough scan instead",
            mode.as_str()
        ));
    }

    let scan_sources = parse_scan_sources(sources.as_deref());
    if scan_sources.notes {
        return Err("the triage scan reads messages; include notes via a standard scan".into());
    }
    let sources_slug = scan_sources.slug();
    let dir = models_dir(&app)?;
    let classifier = match model_id.as_deref() {
        Some(id) => models::spec_by_id(id).ok_or("unknown model id")?,
        None => models::recommended(models::total_ram_bytes()),
    };
    let classifier_path = classifier
        .installed_at(&dir)
        .ok_or("model not installed — download it first")?;
    let embedder = models::embedder().ok_or("the model catalog has no census embedder")?;
    let embedder_path = embedder.installed_at(&dir).ok_or(
        "the Fast pre-scan model is not installed — download it from the Safety Scan settings",
    )?;
    let binary = server::resolve_binary().map_err(|e| e.to_string())?;

    // Flip the scan row to 'running' before the slow model load, exactly like
    // the batch scan, so history reflects the click immediately. The mode rides
    // the audit log — the scans table has no mode column, and the audit trail
    // is its provenance record.
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    };
    let scan_row_id = {
        let db = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;
        let id = db
            .begin_scan(classifier.id, (range_start, range_end), &sources_slug, now())
            .map_err(|e| e.to_string())?;
        let _ = db.audit(id, now(), "triage_mode", mode.as_str());
        id
    };
    let analysis_db_path_repair = analysis_db_path.clone();

    let scratch_dir = models_dir(&app)?.join("sidecar-scratch");
    let cancel = CancelToken::new();
    *cancel_state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());
    let api_key = server::generate_api_key();

    let app2 = app.clone();
    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        emit_scan(&app2, ScanEvent::Loading);
        let _keep_awake = crate::power::KeepAwake::prevent_idle_sleep("TraceLoupe Safety Scan");
        crate::logging::info(
            &app2,
            format!(
                "Triage scan: starting (mode={}, classifier={}, embedder={}, sandbox=on)",
                mode.as_str(),
                classifier.id,
                embedder.id
            ),
        );
        let _ = std::fs::remove_dir_all(&scratch_dir);

        // Read the scope BEFORE loading any model: an empty scope should fail
        // in milliseconds, not after a 30 s model load.
        let cache = CacheDb::open(&cache_path).map_err(|e| e.to_string())?;
        let range = TimeRange {
            start: range_start,
            end: range_end,
        };
        let threads = chunker::census_threads(&cache, range, &scan_sources)
            .map_err(|e| e.to_string())?;
        if threads.iter().all(|t| t.is_empty()) {
            return Err("nothing to scan in this scope".into());
        }

        // Census prototypes come from the committed fixture positives, all
        // categories — categories are a saved VIEW over findings, not a scan
        // parameter (journey §6.3).
        let examples = triage_scan::prototype_examples(&Category::ALL);
        if examples.is_empty() {
            return Err("no prototype examples — the census cannot rank anything".into());
        }

        // Phase 0: the embedder sidecar.
        let llama = spawn_server_healthy(
            &app2,
            &binary,
            &embedder_path,
            embedder.ctx_size,
            true,
            &api_key,
            &scratch_dir,
            &cancel,
        )?;
        crate::logging::info(&app2, "Triage scan: embedder healthy — census starting");

        // The single cancel-watcher, pointed at whichever server is current
        // (the swap updates the atomic). Same rationale as the batch scan: an
        // in-flight request is only interruptible by killing the server.
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
                            "Triage scan: cancel requested — stopping the model server",
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

        let embed_client = client::LlmClient::new(
            llama.base_url(),
            embedder.id,
            Duration::from_secs(300),
        )
        .with_api_key(api_key.clone());

        let prototypes =
            triage::build_prototypes(&examples, |t| embed_client.embed(t)).map_err(|e| {
                format!("building census prototypes: {e}")
            })?;
        if prototypes.is_empty() {
            return Err("could not build census prototypes".into());
        }

        let mut analysis = AnalysisDb::open(&analysis_db_path).map_err(|e| e.to_string())?;

        // One resident sidecar, swapped embedder→classifier on the first
        // focused call (i.e. after the census — run_triage's phases guarantee
        // the ordering). RefCell because the embed and classify closures both
        // reach the slot; run_triage calls them strictly sequentially.
        let sidecar = std::cell::RefCell::new(TriageSidecar {
            llama,
            client: embed_client,
            role: models::ModelRole::Embedder,
            app: app2.clone(),
            binary: binary.clone(),
            scratch_dir: scratch_dir.clone(),
            api_key: api_key.clone(),
            cancel: cancel.clone(),
            current_pid: current_pid.clone(),
        });

        let embed = |t: &str| sidecar.borrow().client.embed(t);
        let classify = |w: &FocusWindow| {
            let mut s = sidecar.borrow_mut();
            s.ensure_classifier(classifier, &classifier_path)?;
            triage_scan::classify_focused(&s.client, w)
        };
        // Unreachable while confirm-on modes are refused above; if that gate is
        // ever bypassed this fails the scan instead of silently passing
        // findings through unconfirmed.
        let confirm = |_: &FocusWindow, _: &FocusVerdict| -> traceloupe_core::Result<bool> {
            Err(traceloupe_core::Error::Inference(
                "no confirmer model is installed".into(),
            ))
        };

        let mut last_emit = std::time::Instant::now();
        let progress = |p: TriageProgress| {
            let (event, boundary) = match p {
                TriageProgress::Census { done, total } => (
                    ScanEvent::Censusing { done, total },
                    done == 0 || done == total,
                ),
                TriageProgress::DeepScan {
                    done,
                    total,
                    findings,
                } => (
                    ScanEvent::DeepScanning {
                        done,
                        total,
                        findings,
                    },
                    done == 0 || done == total,
                ),
                TriageProgress::Confirm { done, total } => (
                    ScanEvent::Confirming { done, total },
                    done == 0 || done == total,
                ),
            };
            // Always emit phase boundaries; throttle the mid-phase stream.
            if boundary || last_emit.elapsed() >= Duration::from_millis(150) {
                last_emit = std::time::Instant::now();
                emit_scan(&app2, event);
            }
        };

        let outcome = triage_scan::run_triage(
            &mut analysis,
            scan_row_id,
            &threads,
            &prototypes,
            mode,
            budget,
            now(),
            embed,
            classify,
            confirm,
            &cancel,
            progress,
        )
        .map_err(|e| e.to_string())?;

        let status = if outcome.cancelled {
            traceloupe_core::analysis::ScanStatus::Cancelled
        } else {
            traceloupe_core::analysis::ScanStatus::Completed
        };
        analysis
            .finish_scan(scan_row_id, status, now())
            .map_err(|e| e.to_string())?;

        watch_done.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = watcher.join();
        sidecar.borrow_mut().llama.shutdown();
        let _ = std::fs::remove_dir_all(&scratch_dir);

        crate::logging::info(
            &app2,
            format!(
                "Triage scan: {} — censused {}, candidates {}, deep-scanned {}, findings {}, unscanned {}",
                if outcome.cancelled { "cancelled" } else { "done" },
                outcome.censused,
                outcome.candidates,
                outcome.deep_scanned,
                outcome.findings,
                outcome.unscanned()
            ),
        );
        emit_scan(
            &app2,
            ScanEvent::TriageDone {
                scan_id: scan_row_id,
                status: if outcome.cancelled {
                    "cancelled".into()
                } else {
                    "completed".into()
                },
                findings: outcome.findings,
                censused: outcome.censused,
                candidates: outcome.candidates,
                deep_scanned: outcome.deep_scanned,
                unscanned: outcome.unscanned(),
                unconfirmed: outcome.unconfirmed,
            },
        );
        Ok(())
    })
    .await;

    // Same error surface as the batch scan: repair the row and emit on both a
    // normal Err and a panicked task, so the UI never waits on a scan that
    // silently died.
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
    /// Who sent the flagged message — `me` for the device owner, else the
    /// handle. None for notes, and for findings written before the column
    /// existed. Lets a group-chat finding name who spoke (#402).
    pub sender: Option<String>,
    /// Normalized identity of the flagged text, or None when it is too long to
    /// recur. The UI uses None to mean "do not offer a content rule" — a rule
    /// keyed on content that can never match again would be a lie (#403).
    pub content_key: Option<String>,
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

/// One page of a scan's findings, filtered and ordered by SQLite (#65).
///
/// This used to return every finding — ~3 MB of JSON at 8000, re-sent and
/// re-derived by the view on each invalidation. The filters and the order live
/// here now because a page only means something relative to a total order.
///
/// `scan_id` restricts to one scan (the history view shows the selected scan's
/// findings); None returns all.
/// Where a finding sits in the current filter and order, or None when the filter
/// excludes it.
///
/// The findings panel is virtualized, so returning to a specific finding needs an
/// index rather than an id (#224). Computed with the same ordering the page query
/// uses, so the two cannot disagree.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn content_finding_rank(
    active: State<'_, ActiveBackup>,
    scan_id: Option<i64>,
    severity: Option<u8>,
    include_dismissed: bool,
    include_low: bool,
    sort_by: String,
    desc: bool,
    group_by_thread: bool,
    exclude_stale: bool,
    finding_id: i64,
) -> Result<Option<i64>, String> {
    let path = analysis_path(&active.path()?)?;
    if !path.exists() {
        return Ok(None);
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let q = FindingQuery {
        severity,
        include_dismissed,
        include_low,
        sort: if sort_by == "date" {
            FindingSort::Date
        } else {
            FindingSort::Severity
        },
        desc,
        group_by_thread,
        exclude_stale,
    };
    // Same scope derivation as the page query: a scan shows everything inside its
    // sources + range, and no scan selected means everything.
    match scan_id {
        Some(id) => match db.scan_by_id(id).map_err(|e| e.to_string())? {
            Some(sc) => db
                .finding_rank(&sc.sources, sc.range_start, sc.range_end, &q, finding_id)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        },
        None => db
            .finding_rank("all", None, None, &q, finding_id)
            .map_err(|e| e.to_string()),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn list_content_findings(
    active: State<'_, ActiveBackup>,
    scan_id: Option<i64>,
    severity: Option<u8>,
    include_dismissed: bool,
    include_low: bool,
    sort_by: String,
    desc: bool,
    group_by_thread: bool,
    exclude_stale: bool,
    offset: i64,
    limit: i64,
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
    let q = FindingQuery {
        severity,
        include_dismissed,
        include_low,
        sort: if sort_by == "date" {
            FindingSort::Date
        } else {
            FindingSort::Severity
        },
        desc,
        group_by_thread,
        exclude_stale,
    };
    let findings = match scan_id {
        Some(id) => match db.scan_by_id(id).map_err(|e| e.to_string())? {
            Some(s) => db.list_findings_in_scope_page(
                &s.sources,
                s.range_start,
                s.range_end,
                &q,
                offset,
                limit,
            ),
            // A scan row that vanished: fall back to every finding rather than
            // an empty page, still windowed.
            None => db.list_findings_in_scope_page("all", None, None, &q, offset, limit),
        },
        None => db.list_findings_in_scope_page("all", None, None, &q, offset, limit),
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
                sender: f.sender,
                content_key: f.content_key,
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

/// A standing "this is fine" rule, and how many findings it dismissed.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuppressionDto {
    pub scope: String,
    pub value: String,
    /// Which category the rule covers. `None` is a rule made before #394, when
    /// a conversation rule covered every category — the UI must say so.
    pub category: Option<String>,
    /// Who the rule is bounded to, for `content+sender`. Empty means any
    /// sender — never NULL, so two rules on the same content from different
    /// people stay distinct.
    pub sender: String,
    pub reason: Option<String>,
    /// How many findings this rule is dismissing right now, counted with the
    /// same predicate the engine acts on. Zero means it is stale or was never
    /// needed — a rule nobody can see the effect of is the shape this feature
    /// exists to avoid.
    pub hits: i64,
}

/// Dismiss a whole conversation or category, now and in future.
///
/// The rule DISMISSES what it covers rather than hiding it: a dismissed finding
/// is counted, reachable and carries the reason, where a hidden one is simply
/// gone. A conversation that is fine today may not be next month, which is the
/// case this app exists to catch.
#[tauri::command]
pub fn add_safety_suppression(
    active: State<'_, ActiveBackup>,
    scope: String,
    value: String,
    category: String,
    sender: Option<String>,
    reason: Option<String>,
) -> Result<usize, String> {
    if !matches!(
        scope.as_str(),
        "thread" | "category" | "content+sender" | "content+any"
    ) {
        return Err("unknown suppression scope".into());
    }
    // A sender-scoped rule with no sender would silently widen to everyone —
    // the opposite of what the reviewer asked for.
    if scope == "content+sender" && sender.as_deref().unwrap_or("").is_empty() {
        return Err("content+sender needs a sender".into());
    }
    // A rule with no category is the pre-#394 breadth that silenced a whole
    // conversation. Only the schema migration may create one.
    if traceloupe_core::analysis::Category::parse(&category).is_none() {
        return Err("unknown category".into());
    }
    let path = analysis_path(&active.path()?)?;
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    db.add_suppression(
        &scope,
        &value,
        &category,
        sender.as_deref(),
        reason.as_deref(),
        now,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_safety_suppressions(
    active: State<'_, ActiveBackup>,
) -> Result<Vec<SuppressionDto>, String> {
    let path = analysis_path(&active.path()?)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    Ok(db
        .list_suppressions()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| SuppressionDto {
            scope: r.scope,
            value: r.value,
            category: r.category,
            sender: r.sender,
            reason: r.reason,
            hits: r.hits,
        })
        .collect())
}

#[tauri::command]
pub fn remove_safety_suppression(
    active: State<'_, ActiveBackup>,
    scope: String,
    value: String,
    category: Option<String>,
    sender: Option<String>,
) -> Result<usize, String> {
    let path = analysis_path(&active.path()?)?;
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    db.remove_suppression(&scope, &value, category.as_deref(), sender.as_deref(), now)
        .map_err(|e| e.to_string())
}

/// Record that a finding's flagged text has been revealed.
///
/// Called when a finding is expanded — the one deliberate act that means it was
/// read. Idempotent, and one-way: collapsing the row is not un-reading it.
#[tauri::command]
pub fn mark_content_finding_seen(
    active: State<'_, ActiveBackup>,
    fingerprint: String,
    category: String,
) -> Result<(), String> {
    let cat = Category::parse(&category).ok_or("unknown category")?;
    let path = analysis_path(&active.path()?)?;
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    db.mark_seen(&fingerprint, cat, now)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn dismiss_content_finding(
    active: State<'_, ActiveBackup>,
    fingerprint: String,
    category: String,
    dismissed: bool,
    // `reason` is the user's record of the judgement when dismissing, kept so
    // the report can show why something was rejected. Ignored when undismissing.
    reason: Option<String>,
) -> Result<(), String> {
    let cat = Category::parse(&category).ok_or("unknown category")?;
    let path = analysis_path(&active.path()?)?;
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    db.set_verdict(
        &fingerprint,
        cat,
        dismissed.then_some("dismissed"),
        reason.as_deref().filter(|_| dismissed),
        now,
    )
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
    /// Why a failed run failed — what the history's warning badge says on hover.
    /// `None` for every other status: cancelled and interrupted explain
    /// themselves.
    pub error: Option<String>,
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
            error: s.error,
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

/// The filter pills' numbers for a scan's scope, in one round trip — counted
/// with the same predicate the page query uses, so a pill can't promise rows the
/// list won't produce (#59).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentFindingCounts {
    /// Rows the CURRENT filter matches — the virtualizer's count.
    pub matching: i64,
    pub live: i64,
    pub live_fresh: i64,
    pub dismissed: i64,
    /// Live, not-stale findings nobody has read yet.
    pub unread: i64,
    pub serious: i64,
    pub harmful: i64,
    pub concerning: i64,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn count_content_findings(
    active: State<'_, ActiveBackup>,
    scan_id: Option<i64>,
    severity: Option<u8>,
    include_dismissed: bool,
    include_low: bool,
    exclude_stale: bool,
) -> Result<ContentFindingCounts, String> {
    let cache_path = active.path()?;
    let path = analysis_path(&cache_path)?;
    if !path.exists() {
        return Ok(ContentFindingCounts {
            matching: 0,
            live: 0,
            live_fresh: 0,
            dismissed: 0,
            unread: 0,
            serious: 0,
            harmful: 0,
            concerning: 0,
        });
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let (sources, start, end) = match scan_id {
        Some(id) => match db.scan_by_id(id).map_err(|e| e.to_string())? {
            Some(s) => (s.sources, s.range_start, s.range_end),
            None => ("all".to_string(), None, None),
        },
        None => ("all".to_string(), None, None),
    };
    let c = db
        .count_findings_breakdown(&sources, start, end)
        .map_err(|e| e.to_string())?;
    let matching = db
        .count_findings_matching(
            &sources,
            start,
            end,
            &FindingQuery {
                severity,
                include_dismissed,
                include_low,
                exclude_stale,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(ContentFindingCounts {
        matching,
        live: c.live,
        live_fresh: c.live_fresh,
        dismissed: c.dismissed,
        unread: c.unread,
        serious: c.serious,
        harmful: c.harmful,
        concerning: c.concerning,
    })
}

/// One bar. `confirmed[i]`/`unconfirmed[i]` are severity `i + 1`.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartBucketDto {
    pub key: String,
    pub confirmed: [i64; 3],
    pub unconfirmed: [i64; 3],
}

impl From<ChartBucket> for ChartBucketDto {
    fn from(b: ChartBucket) -> Self {
        ChartBucketDto {
            key: b.key,
            confirmed: b.confirmed,
            unconfirmed: b.unconfirmed,
        }
    }
}

/// What the report's charts draw (#66).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingAnalyticsDto {
    /// What one bar of `overTime` spans: day | week | month | quarter | year.
    pub unit: String,
    pub over_time: Vec<ChartBucketDto>,
    pub by_category: Vec<ChartBucketDto>,
    pub by_conversation: Vec<ChartBucketDto>,
    pub other_conversations: i64,
    pub other_conversation_findings: i64,
    pub charted: i64,
    pub undated: i64,
    pub dismissed: i64,
}

/// The report's charts, aggregated in SQL over every finding the filter matches.
///
/// Deliberately NOT derived from the findings the panel already holds: that list
/// is one capped page (#65), and a chart drawn from it would describe a subset
/// while looking like it described the scan. Every vector here is bounded — nine
/// categories, one bar per time bucket with the unit chosen to keep that near
/// 10–30, and conversations capped with the remainder stated.
#[tauri::command]
pub fn content_finding_analytics(
    active: State<'_, ActiveBackup>,
    scan_id: Option<i64>,
    severity: Option<u8>,
    include_dismissed: bool,
    include_low: bool,
    exclude_stale: bool,
) -> Result<FindingAnalyticsDto, String> {
    let cache_path = active.path()?;
    let path = analysis_path(&cache_path)?;
    if !path.exists() {
        return Ok(FindingAnalyticsDto {
            unit: "month".into(),
            over_time: Vec::new(),
            by_category: Vec::new(),
            by_conversation: Vec::new(),
            other_conversations: 0,
            other_conversation_findings: 0,
            charted: 0,
            undated: 0,
            dismissed: 0,
        });
    }
    let db = AnalysisDb::open(&path).map_err(|e| e.to_string())?;
    let (sources, start, end) = match scan_id {
        Some(id) => match db.scan_by_id(id).map_err(|e| e.to_string())? {
            Some(s) => (s.sources, s.range_start, s.range_end),
            None => ("all".to_string(), None, None),
        },
        None => ("all".to_string(), None, None),
    };
    // The clock is passed in rather than read inside the query so the window is
    // one decision made in one place — and so the tests can pin it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(i64::MAX);
    let a = db
        .finding_analytics(
            &sources,
            start,
            end,
            &FindingQuery {
                severity,
                include_dismissed,
                include_low,
                exclude_stale,
                ..Default::default()
            },
            now,
        )
        .map_err(|e| e.to_string())?;
    Ok(FindingAnalyticsDto {
        unit: a.unit.as_str().to_string(),
        over_time: a.over_time.into_iter().map(Into::into).collect(),
        by_category: a.by_category.into_iter().map(Into::into).collect(),
        by_conversation: a.by_conversation.into_iter().map(Into::into).collect(),
        other_conversations: a.other_conversations,
        other_conversation_findings: a.other_conversation_findings,
        charted: a.charted,
        undated: a.undated,
        dismissed: a.dismissed,
    })
}
