import { createContext, useContext, useEffect, useState } from "react";
import { formatCount } from "@/lib/format";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { client, type ReimportResult } from "@/lib/ipc";
import { useSettings } from "@/components/settings-provider";

/**
 * React Query key prefixes each module's data feeds, so a re-import invalidates
 * only the affected views rather than the whole cache (a blanket invalidate would
 * mark heavy queries — e.g. a huge message timeline — stale for no reason).
 */
const INVALIDATE_KEYS: Record<string, string[]> = {
  recordings: ["recordings"],
  notes: ["notes"],
  calls: ["callsCount", "callsWindow"],
  safari: ["safariCount", "safariWindow"],
  camera_roll: ["mediaCount", "mediaSources", "mediaWindow", "mediaRanges"],
  messages: [
    "threads",
    "messageCount",
    "messageWindow",
    "messageRanges",
    "timelineRangeCount",
    "timelineWindow",
  ],
};

/** Human count of what a re-import produced, for the success toast. */
function summarize(module: string, r: ReimportResult): string {
  const { n, noun } =
    module === "recordings"
      ? { n: r.recordings, noun: "recordings" }
      : module === "camera_roll"
        ? { n: r.mediaItems, noun: "photos & videos" }
        : module === "notes"
          ? { n: r.notes, noun: "notes" }
          : module === "calls"
            ? { n: r.calls, noun: "calls" }
            : module === "safari"
              ? { n: r.safariVisits, noun: "Safari visits" }
              : { n: r.messages, noun: "messages" };
  return `Re-imported ${formatCount(n)} ${noun}`;
}

type ReimportContextValue = {
  /** Module ids currently re-importing — for the toolbar activity list (#73). */
  running: Set<string>;
  /** True while `module` is being re-imported. */
  isRunning: (module: string) => boolean;
  /** Kick off a single-module re-import (no-op if that module is already running). */
  reimport: (module: string) => void;
};

const ReimportContext = createContext<ReimportContextValue | null>(null);

/**
 * Owns the per-module re-import lifecycle above the routes, so a running
 * re-import — and its spinner — survives navigating between views. (When this
 * state lived inside the per-view button, switching away unmounted it and the
 * button came back stale even though the backend was still working.)
 *
 * Feedback is via shadcn's Sonner toasts; on success it invalidates only the
 * query keys the module feeds so the current view refreshes. An error toast
 * stays until dismissed (a decrypt/parse error is worth reading) and is logged.
 */
export function ReimportProvider({ children }: { children: React.ReactNode }) {
  const { showOffloadedPhotos: showOffloaded } = useSettings();
  const qc = useQueryClient();
  const [running, setRunning] = useState<Set<string>>(new Set());

  // A re-import runs in the Rust process and survives a webview reload; this
  // state does not. Re-attach on mount so a reload doesn't show the module as
  // idle (and let the user start a second one) while the first still runs (#72).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const live = await client.getReimportStatus().catch(() => []);
      if (!cancelled && live.length > 0) setRunning(new Set(live));
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const setModuleRunning = (module: string, on: boolean) =>
    setRunning((prev) => {
      const next = new Set(prev);
      if (on) next.add(module);
      else next.delete(module);
      return next;
    });

  const reimport = async (module: string) => {
    if (running.has(module)) return;
    setModuleRunning(module, true);
    try {
      const result = await client.reimportModule(module, showOffloaded);
      const prefixes = INVALIDATE_KEYS[module] ?? [];
      await Promise.all(
        prefixes.map((key) => qc.invalidateQueries({ queryKey: [key] })),
      );
      toast.success(summarize(module, result));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      console.error(`[reimport ${module}]`, msg);
      toast.error("Re-import failed", { description: msg, duration: Infinity });
    } finally {
      setModuleRunning(module, false);
    }
  };

  return (
    <ReimportContext.Provider
      value={{ running, isRunning: (m) => running.has(m), reimport }}
    >
      {children}
    </ReimportContext.Provider>
  );
}

export function useReimport() {
  const ctx = useContext(ReimportContext);
  if (!ctx)
    throw new Error("useReimport must be used within a ReimportProvider");
  return ctx;
}
