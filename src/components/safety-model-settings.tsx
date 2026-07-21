import { useQuery } from "@tanstack/react-query";
import { Ban, Download, Loader2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useSafetyScan } from "@/components/safety-scan-provider";
import { client } from "@/lib/ipc";

/**
 * Safety Scan model management (plan T2/T8): the download UI for the local
 * Gemma models. Lives in Settings → Safety, not the scan view, because it is a
 * one-time multi-GB setup concern rather than per-scan content.
 */

function gb(bytes: number) {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export function SafetyModelSettings() {
  const { download, downloadingModelId, startDownload, cancelDownload } =
    useSafetyScan();
  const modelStatus = useQuery({
    queryKey: ["safetyScan", "modelStatus"],
    queryFn: () => client.getSafetyScanModelStatus(),
  });

  if (modelStatus.isPending) {
    return (
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" /> Loading…
      </div>
    );
  }
  if (modelStatus.isError) {
    return (
      <p className="text-sm text-destructive">
        Couldn't read model status: {String(modelStatus.error)}
      </p>
    );
  }
  const ms = modelStatus.data;

  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">
        Safety Scan runs a local language model to analyze your messages and
        notes. It is downloaded once (checksum-verified), runs sandboxed with no
        network access, and your data never leaves this Mac.
      </p>

      {/* The list is always rendered so it never jumps; progress appears inside
          the downloading model's own row. Download outcomes are toasts (see
          the provider) — no red text in this pane. */}
      <div className="flex flex-col gap-2">
        {ms.models.map((m) => {
          const isDownloading = downloadingModelId === m.id && download !== null;
          return (
            <div key={m.id} className="rounded-md border p-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="text-sm font-medium">
                    {m.displayName}{" "}
                    {m.recommended && (
                      <Badge variant="secondary" className="ml-1">
                        Recommended for this Mac
                      </Badge>
                    )}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {gb(m.sizeBytes)} download
                    {m.installed && " · installed"}
                  </div>
                </div>
                {isDownloading ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={cancelDownload}
                    disabled={download.phase === "verifying"}
                  >
                    <Ban className="size-4" /> Cancel
                  </Button>
                ) : m.installed ? (
                  <Badge variant="outline">Installed</Badge>
                ) : (
                  <Button
                    variant={m.recommended ? "default" : "outline"}
                    size="sm"
                    // Disable other rows' downloads while one is in flight.
                    disabled={download !== null}
                    onClick={() => void startDownload(m.id)}
                  >
                    <Download className="size-4" /> Download
                  </Button>
                )}
              </div>

              {isDownloading && (
                <div className="mt-2 space-y-1">
                  <Progress
                    value={
                      download.phase === "downloading" && download.total > 0
                        ? (download.received / download.total) * 100
                        : undefined
                    }
                  />
                  <div className="text-xs text-muted-foreground">
                    {download.phase === "verifying"
                      ? "Verifying…"
                      : download.phase === "downloading" && download.total > 0
                        ? `${gb(download.received)} / ${gb(download.total)}`
                        : "Starting…"}
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
