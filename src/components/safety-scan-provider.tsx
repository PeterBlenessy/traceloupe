import { createContext, useContext, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
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
  startScan: (opts: {
    modelId?: string | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
  }) => Promise<void>;
  cancelScan: () => void;
  startDownload: (modelId: string) => Promise<void>;
  cancelDownload: () => void;
};

const SafetyScanContext = createContext<SafetyScanContextValue | null>(null);

/** Terminal events clear the "running" state. */
function scanIsTerminal(e: SafetyScanEvent) {
  return e.phase === "done" || e.phase === "error";
}

/** A short, dismissable error toast. The full technical string (which can
 *  include multi-line llama-server output) goes to the dev logs; the toast
 *  shows a friendly title + its first line. */
function toastScanError(message: string) {
  const title =
    message.includes("exited during startup") ||
    message.includes("model not installed")
      ? "Safety Scan couldn't start the local model"
      : "Safety Scan failed";
  toast.error(title, {
    description: message.split("\n")[0].slice(0, 200),
  });
}

export function SafetyScanProvider({ children }: { children: React.ReactNode }) {
  const qc = useQueryClient();
  const [scan, setScan] = useState<SafetyScanEvent | null>(null);
  const [download, setDownload] = useState<SafetyModelProgressEvent | null>(null);
  const [downloadingModelId, setDownloadingModelId] = useState<string | null>(
    null,
  );
  const unlistenScan = useRef<(() => void) | null>(null);
  const unlistenModel = useRef<(() => void) | null>(null);

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

  const startScan = async (opts: {
    modelId?: string | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
  }) => {
    setScan({ phase: "loading" });
    if (!unlistenScan.current) {
      // Claim the slot synchronously (before the await) so a second call in
      // the same tick can't register a duplicate listener.
      unlistenScan.current = () => {};
      unlistenScan.current = await client.onSafetyScanProgress((p) => {
        setScan(scanIsTerminal(p) ? null : p);
        // Errors are a dismissable toast, not red text baked into the view.
        // The full technical detail (incl. llama-server output) is in the dev
        // logs; the toast stays short and readable.
        if (p.phase === "error") toastScanError(p.message);
        if (p.phase === "done") {
          // New findings and a new report exist; let every consumer refetch.
          qc.invalidateQueries({ queryKey: ["safetyScan"] });
        }
      });
    }
    try {
      await client.runSafetyScan(opts);
    } catch {
      // The listener owns the error toast (and survives a refresh); only clear
      // the running state here.
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
        startScan,
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
