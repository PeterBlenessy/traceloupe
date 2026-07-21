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
  startScan: (opts: {
    modelId?: string | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
  }) => Promise<void>;
  cancelScan: () => void;
  startDownload: (modelId: string) => Promise<void>;
  cancelDownload: () => void;
  /** Last scan/download error message, cleared on the next start. */
  error: string | null;
};

const SafetyScanContext = createContext<SafetyScanContextValue | null>(null);

/** Terminal events clear the "running" state. */
function scanIsTerminal(e: SafetyScanEvent) {
  return e.phase === "done" || e.phase === "error";
}

export function SafetyScanProvider({ children }: { children: React.ReactNode }) {
  const qc = useQueryClient();
  const [scan, setScan] = useState<SafetyScanEvent | null>(null);
  const [download, setDownload] = useState<SafetyModelProgressEvent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const unlistenScan = useRef<(() => void) | null>(null);
  const unlistenModel = useRef<(() => void) | null>(null);

  // Subscribe to model-download progress exactly once. Shared by startDownload
  // and the rehydration effect below.
  const subscribeModel = async () => {
    if (unlistenModel.current) return;
    unlistenModel.current = () => {}; // claim synchronously against a double-call
    unlistenModel.current = await client.onSafetyModelProgress((p) => {
      setDownload(p.phase === "done" || p.phase === "error" ? null : p);
      if (p.phase === "error") {
        // A failure is a toast, not red text wedged into the Settings UI.
        toast.error(`Model download failed: ${p.message}`);
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
    setError(null);
    setScan({ phase: "loading" });
    if (!unlistenScan.current) {
      // Claim the slot synchronously (before the await) so a second call in
      // the same tick can't register a duplicate listener.
      unlistenScan.current = () => {};
      unlistenScan.current = await client.onSafetyScanProgress((p) => {
        setScan(scanIsTerminal(p) ? null : p);
        if (p.phase === "error") setError(p.message);
        if (p.phase === "done") {
          // New findings and a new report exist; let every consumer refetch.
          qc.invalidateQueries({ queryKey: ["safetyScan"] });
        }
      });
    }
    try {
      await client.runSafetyScan(opts);
    } catch (e) {
      setScan(null);
      setError(String(e));
    }
  };

  const cancelScan = () => {
    void client.cancelSafetyScan();
  };

  const startDownload = async (modelId: string) => {
    // Already downloading — its progress is already showing and it keeps
    // running in the background; don't start a second one or surface anything.
    if (download) return;
    setDownload({ phase: "downloading", received: 0, total: 0 });
    await subscribeModel();
    try {
      await client.downloadSafetyScanModel(modelId);
      // The mock client resolves without emitting events; refresh regardless.
      setDownload(null);
      qc.invalidateQueries({ queryKey: ["safetyScan", "modelStatus"] });
    } catch (e) {
      const msg = String(e);
      // "already running" — a duplicate start; the real one is still going, so
      // leave its progress visible and say nothing.
      if (msg.includes("already running")) return;
      setDownload(null);
      // Cancelling is a user action, not a failure — a quiet toast, no red.
      // (The shared cancel error reads "import cancelled".)
      if (msg.toLowerCase().includes("cancel")) {
        toast("Model download cancelled");
        return;
      }
      toast.error(`Model download failed: ${msg}`);
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
        startScan,
        cancelScan,
        startDownload,
        cancelDownload,
        error,
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
