/**
 * Owns Security Check scan progress above the routes (#72, #73).
 *
 * It used to be `useState` inside `views/security.tsx`, which meant two things:
 * the phase was lost on any webview reload with no way to recover it, and a scan
 * you navigated away from became invisible — no toolbar indicator existed for it
 * at all. Lifting it here fixes both: the state outlives the view, and the
 * activity pill can show it.
 *
 * Mirrors SafetyScanProvider: subscribe once, and re-attach on mount to whatever
 * the Rust process is already running.
 */
import { createContext, useContext, useEffect, useRef, useState } from "react";
import { client, type ScanProgress } from "@/lib/ipc";

type SecurityScanContextValue = {
  /** Latest progress, or null when no scan is running. */
  progress: ScanProgress | null;
  /** Clear it when a scan completes (the caller owns the run's result). */
  clear: () => void;
};

const SecurityScanContext = createContext<SecurityScanContextValue | null>(null);

export function SecurityScanProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const unlisten = useRef<(() => void) | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      // Re-attach first, so a reload mid-scan shows the current phase rather
      // than nothing until the next event happens to arrive.
      const live = await client.getSecurityScanStatus();
      if (!cancelled && live) setProgress(live);
      if (unlisten.current) return;
      unlisten.current = () => {}; // claim synchronously against a double-call
      unlisten.current = await client.onScanProgress((p) => setProgress(p));
    })();
    return () => {
      cancelled = true;
      unlisten.current?.();
      unlisten.current = null;
    };
  }, []);

  return (
    <SecurityScanContext.Provider
      value={{ progress, clear: () => setProgress(null) }}
    >
      {children}
    </SecurityScanContext.Provider>
  );
}

export function useSecurityScan() {
  const ctx = useContext(SecurityScanContext);
  if (!ctx)
    throw new Error("useSecurityScan must be used within a SecurityScanProvider");
  return ctx;
}
