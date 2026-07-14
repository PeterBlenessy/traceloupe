import { createContext, useContext, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { client, type ReimportResult } from "@/lib/ipc";

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
  camera_roll: ["mediaCount", "mediaSources", "mediaWindow"],
  messages: [
    "threads",
    "messageCount",
    "messageWindow",
    "messageRanges",
    "timelineCount",
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
  return `Re-imported ${n.toLocaleString()} ${noun}`;
}

type ReimportContextValue = {
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
  const qc = useQueryClient();
  const [running, setRunning] = useState<Set<string>>(new Set());

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
      const result = await client.reimportModule(module);
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
      value={{ isRunning: (m) => running.has(m), reimport }}
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
