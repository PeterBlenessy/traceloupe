import { createContext, useContext, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
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

  useEffect(() => {
    return () => {
      unlistenScan.current?.();
      unlistenModel.current?.();
    };
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
    setError(null);
    setDownload({ phase: "downloading", received: 0, total: 0 });
    if (!unlistenModel.current) {
      unlistenModel.current = () => {};
      unlistenModel.current = await client.onSafetyModelProgress((p) => {
        setDownload(p.phase === "done" || p.phase === "error" ? null : p);
        if (p.phase === "error") setError(p.message);
        if (p.phase === "done") {
          qc.invalidateQueries({ queryKey: ["safetyScan", "modelStatus"] });
        }
      });
    }
    try {
      await client.downloadSafetyScanModel(modelId);
      // The mock client resolves without emitting events; refresh regardless.
      setDownload(null);
      qc.invalidateQueries({ queryKey: ["safetyScan", "modelStatus"] });
    } catch (e) {
      setDownload(null);
      setError(String(e));
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
