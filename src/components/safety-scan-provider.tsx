import { createContext, useContext, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { usePersistedState } from "@/lib/use-persisted-state";
import {
  client,
  type SafetyModelProgressEvent,
  type SafetyScanEvent,
} from "@/lib/ipc";

/**
 * Owns the Safety Scan lifecycle above the routes (mirror of ImportProvider):
 * a running scan or model download survives navigating away from the Safety
 * Scan view. Views subscribe to this state; the view's buttons call
 * `startScan`/`startDownload`.
 */
type SafetyScanContextValue = {
  /** Latest scan progress event, or null when no scan is running. */
  scan: SafetyScanEvent | null;
  /** Latest model-download event, or null when no download is running. */
  download: SafetyModelProgressEvent | null;
  /** Which model id is downloading (so the UI can show progress in-row). */
  downloadingModelId: string | null;
  /** The user's chosen scan model, or null to use the recommended one. Shared
   *  by Settings (the picker) and the scan view (which model will run). */
  preferredModelId: string | null;
  setPreferredModelId: (id: string | null) => void;
  startScan: (opts: {
    modelId?: string | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
    sources?: string | null;
    /** Resume this scan row instead of creating a new one. */
    resumeScanId?: number | null;
  }) => Promise<void>;
  /** Start a triage scan (census → focused deep-scan → confirm). Same stream
   *  and one-at-a-time gate as startScan; no resume (the census is
   *  incremental, so a new scan loses nothing). */
  startTriageScan: (opts: {
    modelId?: string | null;
    mode?: "thorough" | "balanced" | "precise" | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
    sources?: string | null;
    /** Wall-clock cap on the deep scan, in hours; null reads every candidate.
     *  Whatever it does not reach is reported unscanned, never "clean". */
    budgetHours?: number | null;
  }) => Promise<void>;
  /** The last completed triage scan's coverage numbers — the honest "N of M
   *  candidates deep-scanned" line. The triageDone event is terminal (it
   *  clears `scan`), so it is captured here; cleared when a new scan starts. */
  lastTriageDone: Extract<SafetyScanEvent, { phase: "triageDone" }> | null;
  cancelScan: () => void;
  startDownload: (modelId: string) => Promise<void>;
  cancelDownload: () => void;
};

const SafetyScanContext = createContext<SafetyScanContextValue | null>(null);

/** Terminal events clear the "running" state. */
function scanIsTerminal(e: SafetyScanEvent) {
  return e.phase === "done" || e.phase === "triageDone" || e.phase === "error";
}

/** A short, dismissable error toast. The full technical string (which can
 *  include multi-line llama-server output) goes to the dev logs; the toast
 *  shows a friendly title + its first line. */
function toastScanError(message: string) {
  const title =
    message.includes("exited during startup") ||
    message.includes("model not installed")
      ? "Safety Scan couldn't start the local AI"
      : "Safety Scan failed";
  toast.error(title, {
    description: message.split("\n")[0].slice(0, 200),
  });
}

export function SafetyScanProvider({ children }: { children: React.ReactNode }) {
  const qc = useQueryClient();
  const [scan, setScan] = useState<SafetyScanEvent | null>(null);
  const [lastTriageDone, setLastTriageDone] = useState<Extract<
    SafetyScanEvent,
    { phase: "triageDone" }
  > | null>(null);
  const [download, setDownload] = useState<SafetyModelProgressEvent | null>(null);
  const [downloadingModelId, setDownloadingModelId] = useState<string | null>(
    null,
  );
  // Persisted so the choice survives navigation and restarts. null ⇒ backend
  // picks the RAM-recommended tier. Consumers validate it's still installed.
  const [preferredModelId, setPreferredModelId] = usePersistedState<
    string | null
  >("safety-scan:preferred-model", null);
  const unlistenScan = useRef<(() => void) | null>(null);
  const unlistenModel = useRef<(() => void) | null>(null);
  // Per-run live-refresh bookkeeping: refresh the history once when the run's
  // row appears, and the findings each time the live count moves.
  const runLive = useRef({ historyRefreshed: false, findings: 0 });

  // Subscribe to model-download progress exactly once. Shared by startDownload
  // and the rehydration effect below.
  const subscribeModel = async () => {
    if (unlistenModel.current) return;
    unlistenModel.current = () => {}; // claim synchronously against a double-call
    unlistenModel.current = await client.onSafetyModelProgress((p) => {
      const terminal = p.phase === "done" || p.phase === "error";
      setDownload(terminal ? null : p);
      if (terminal) setDownloadingModelId(null);
      if (p.phase === "error") {
        // The listener OWNS download outcome toasts — it's the only handler that
        // survives a webview refresh (the startDownload promise does not), so
        // toasting here too would double up. Cancelling emits an error event
        // whose message is the shared "import cancelled" — that's a user action,
        // not a failure, so it's a quiet toast, never red.
        if (p.message.toLowerCase().includes("cancel")) {
          toast("Model download cancelled");
        } else {
          toast.error(`Model download failed: ${p.message}`);
        }
      }
      if (p.phase === "done") {
        toast.success("Model ready");
        qc.invalidateQueries({ queryKey: ["safetyScan", "modelStatus"] });
      }
    });
  };

  useEffect(() => {
    let cancelled = false;
    // A download runs in the Rust process and survives a webview refresh, but
    // this React state doesn't — so on mount, rehydrate any in-flight download
    // from the backend and re-attach to its progress. Without this, a refresh
    // goes blank and re-clicking Download collides with the download gate.
    // Same for a SCAN: it runs in the Rust process for hours and survives a
    // webview reload, but this React state does not — so re-attach to whatever
    // is in flight. Without this a reload (a crash, ⌘R, the webview respawning,
    // or a dev-server HMR reload) left the view showing an idle "Start safety
    // scan" over a scan that was still running, with its progress frozen.
    void (async () => {
      const live = await client.getSafetyScanStatus();
      if (cancelled || !live) return;
      setScan(live);
      // Reflect what's already on disk, then follow it live.
      qc.invalidateQueries({ queryKey: ["safetyScan"] });
      await subscribeScan();
    })();
    // The grooming-signal artefacts (#521) install silently for EXISTING
    // users — their scan models predate the signal, so no download path would
    // otherwise ever fetch them. Backend-gated, idempotent, log-only.
    void client.ensureGroomingArtefacts().catch(() => {});
    void (async () => {
      const status = await client.getSafetyScanDownloadStatus();
      if (cancelled || !status) return;
      setDownloadingModelId(status.modelId);
      setDownload(
        status.phase === "verifying"
          ? { phase: "verifying" }
          : {
              phase: "downloading",
              received: status.received,
              total: status.total,
            },
      );
      await subscribeModel();
    })();
    return () => {
      cancelled = true;
      unlistenScan.current?.();
      unlistenModel.current?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Subscribe to scan progress exactly once. Shared by startScan and the
  // rehydration effect below — the same reason subscribeModel exists.
  const subscribeScan = async () => {
    if (unlistenScan.current) return;
    // Claim the slot synchronously (before the await) so a second call in the
    // same tick can't register a duplicate listener.
    unlistenScan.current = () => {};
    unlistenScan.current = await client.onSafetyScanProgress((p) => {
      setScan(scanIsTerminal(p) ? null : p);
      // Errors are a dismissable toast, not red text baked into the view.
      // The full technical detail (incl. llama-server output) is in the dev
      // logs; the toast stays short and readable.
      if (p.phase === "error") toastScanError(p.message);
      if (p.phase === "loading") {
        // The command flips the scan row to 'running' before the model
        // load and then emits Loading — refetch the history now so the
        // rail's badge changes in step with the Stop button, not a model
        // load later.
        qc.invalidateQueries({ queryKey: ["safetyScan", "history"] });
      }
      if (p.phase === "classifying" || p.phase === "censusing") {
        // Backstop history refresh once real work starts (the triage census
        // is that phase for a triage scan).
        if (!runLive.current.historyRefreshed) {
          runLive.current.historyRefreshed = true;
          qc.invalidateQueries({ queryKey: ["safetyScan", "history"] });
        }
      }
      if (p.phase === "classifying" || p.phase === "deepScanning") {
        // Refresh the findings whenever the live count moves so they stream
        // in. (deepScanning's count is provisional — pre-confirmation — which
        // is fine: the terminal invalidate below settles it.)
        if (p.findings !== runLive.current.findings) {
          runLive.current.findings = p.findings;
          qc.invalidateQueries({ queryKey: ["safetyScan", "findings"] });
        }
      }
      if (p.phase === "triageDone") {
        // Terminal for `scan`, so the coverage numbers are captured here — the
        // run card renders them after the progress UI clears.
        setLastTriageDone(p);
      }
      if (p.phase === "done" || p.phase === "triageDone") {
      // New findings and a new report exist; let every consumer refetch.
      qc.invalidateQueries({ queryKey: ["safetyScan"] });
      }
    });
  };

  const startScan = async (opts: {
    modelId?: string | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
    sources?: string | null;
    /** Resume this scan row instead of creating a new one. */
    resumeScanId?: number | null;
  }) => {
    setScan({ phase: "loading" });
    setLastTriageDone(null);
    runLive.current = { historyRefreshed: false, findings: 0 };
    await subscribeScan();
    try {
      // Fall back to the persisted preference when the caller didn't pin a
      // model; the backend maps null ⇒ recommended tier.
      await client.runSafetyScan({
        ...opts,
        modelId: opts.modelId ?? preferredModelId,
      });
    } catch {
      // The listener owns the error toast (and survives a refresh); only clear
      // the running state here.
      setScan(null);
    }
  };

  const startTriageScan = async (opts: {
    modelId?: string | null;
    mode?: "thorough" | "balanced" | "precise" | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
    sources?: string | null;
  }) => {
    setScan({ phase: "loading" });
    setLastTriageDone(null);
    runLive.current = { historyRefreshed: false, findings: 0 };
    await subscribeScan();
    try {
      await client.runTriageScan({
        ...opts,
        modelId: opts.modelId ?? preferredModelId,
      });
    } catch {
      // The listener owns the error toast; only clear the running state.
      setScan(null);
    }
  };

  const cancelScan = () => {
    void client.cancelSafetyScan();
  };

  const startDownload = async (modelId: string) => {
    // Already downloading — its progress is already showing and it keeps
    // running in the background; don't start a second one or surface anything.
    if (download) return;
    setDownloadingModelId(modelId);
    setDownload({ phase: "downloading", received: 0, total: 0 });
    await subscribeModel();
    try {
      await client.downloadSafetyScanModel(modelId);
      void client.ensureGroomingArtefacts().catch(() => {});
      // The mock client resolves without emitting events; refresh regardless.
      setDownload(null);
      setDownloadingModelId(null);
      qc.invalidateQueries({ queryKey: ["safetyScan", "modelStatus"] });
    } catch (e) {
      // Outcome toasts (cancel/fail) are owned by the progress listener, which
      // also fires after a refresh — toasting here would double-report.
      // "already running" means the real download continues, so keep showing
      // its progress; any other rejection clears the pill defensively (the
      // terminal event usually already did).
      if (!String(e).includes("already running")) {
        setDownload(null);
        setDownloadingModelId(null);
      }
    }
  };

  const cancelDownload = () => {
    void client.cancelSafetyScanModelDownload();
  };

  return (
    <SafetyScanContext.Provider
      value={{
        scan,
        download,
        downloadingModelId,
        preferredModelId,
        setPreferredModelId,
        startScan,
        startTriageScan,
        lastTriageDone,
        cancelScan,
        startDownload,
        cancelDownload,
      }}
    >
      {children}
    </SafetyScanContext.Provider>
  );
}

export function useSafetyScan() {
  const ctx = useContext(SafetyScanContext);
  if (!ctx)
    throw new Error("useSafetyScan must be used within a SafetyScanProvider");
  return ctx;
}
