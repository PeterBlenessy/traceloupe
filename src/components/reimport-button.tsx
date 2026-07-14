import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { client, type ReimportResult } from "@/lib/ipc";

/**
 * React Query key prefixes each module's data feeds, so a re-import invalidates
 * only the affected views rather than the whole cache (a blanket invalidate would
 * mark heavy queries — e.g. a huge message timeline — stale for no reason).
 */
const INVALIDATE_KEYS: Record<string, string[]> = {
  recordings: ["recordings"],
  notes: ["notes"],
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
  const n =
    module === "recordings"
      ? r.recordings
      : module === "camera_roll"
        ? r.mediaItems
        : module === "notes"
          ? r.notes
          : r.messages;
  const noun =
    module === "recordings"
      ? "recordings"
      : module === "camera_roll"
        ? "photos & videos"
        : module === "notes"
          ? "notes"
          : "messages";
  return `Re-imported ${n.toLocaleString()} ${noun}`;
}

/**
 * Re-import a single native data type (recordings, camera_roll, messages, notes)
 * into the open backup without a full re-import. Feedback is via shadcn's Sonner
 * toasts; on success it invalidates the query cache so the current view refreshes.
 * An error toast stays until dismissed (a decrypt/parse error is worth reading),
 * and is also logged to the console for copying.
 */
export function ReimportButton({
  module,
  label = "Re-import",
}: {
  module: string;
  label?: string;
}) {
  const qc = useQueryClient();
  const [running, setRunning] = useState(false);

  async function run() {
    setRunning(true);
    try {
      const result = await client.reimportModule(module);
      // Refresh only the views this module feeds.
      const prefixes = INVALIDATE_KEYS[module] ?? [];
      await Promise.all(
        prefixes.map((key) => qc.invalidateQueries({ queryKey: [key] })),
      );
      toast.success(summarize(module, result));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      console.error(`[reimport ${module}]`, msg);
      toast.error(`Re-import failed`, { description: msg, duration: Infinity });
    } finally {
      setRunning(false);
    }
  }

  return (
    <Button size="sm" variant="outline" onClick={run} disabled={running}>
      {running ? (
        <Loader2 className="size-4 animate-spin" />
      ) : (
        <RefreshCw className="size-4" />
      )}
      {label}
    </Button>
  );
}
