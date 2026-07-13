import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { client } from "@/lib/ipc";

/**
 * Re-import a single native data type (recordings, camera_roll, messages, notes)
 * into the open backup without a full re-import. On success it invalidates the
 * query cache so the current view refreshes. On failure it shows the error inline
 * (selectable) and logs it to the console, so a decrypt/parse error is visible
 * and copyable rather than swallowed.
 */
export function ReimportButton({
  module,
  label = "Re-import",
}: {
  module: string;
  label?: string;
}) {
  const qc = useQueryClient();
  const [status, setStatus] = useState<"idle" | "running" | "error">("idle");
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setStatus("running");
    setError(null);
    try {
      await client.reimportModule(module);
      // Refresh whatever the current view is reading.
      await qc.invalidateQueries();
      setStatus("idle");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      // Surface the full error in the console so it's copyable for diagnosis.
      console.error(`[reimport ${module}]`, msg);
      setError(msg);
      setStatus("error");
    }
  }

  return (
    <div className="flex min-w-0 items-center gap-2">
      {status === "error" && error && (
        <span className="max-w-md select-text truncate text-xs text-destructive" title={error}>
          {error}
        </span>
      )}
      <Button size="sm" variant="outline" onClick={run} disabled={status === "running"}>
        {status === "running" ? (
          <Loader2 className="size-4 animate-spin" />
        ) : (
          <RefreshCw className="size-4" />
        )}
        {label}
      </Button>
    </div>
  );
}
