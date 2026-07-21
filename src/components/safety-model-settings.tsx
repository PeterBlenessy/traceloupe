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
  const { download, startDownload, cancelDownload } = useSafetyScan();
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
  const downloading = download !== null;

  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">
        Safety Scan runs a local language model to analyze your messages and
        notes. It is downloaded once (checksum-verified), runs sandboxed with no
        network access, and your data never leaves this Mac.
      </p>

      {/* Download outcomes are toasts (see the provider); the shared `error`
          state belongs to scans and must not bleed red text into this pane. */}
      {downloading && download.phase === "downloading" ? (
        <div className="space-y-2">
          <Progress
            value={
              download.total > 0 ? (download.received / download.total) * 100 : 0
            }
          />
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>
              {gb(download.received)} /{" "}
              {download.total ? gb(download.total) : "…"}
            </span>
            <Button variant="ghost" size="sm" onClick={cancelDownload}>
              <Ban className="size-4" /> Cancel
            </Button>
          </div>
        </div>
      ) : downloading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="size-4 animate-spin" /> Verifying…
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          {ms.models.map((m) => (
            <div
              key={m.id}
              className="flex items-center justify-between rounded-md border p-3"
            >
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
              {m.installed ? (
                <Badge variant="outline">Installed</Badge>
              ) : (
                <Button
                  variant={m.recommended ? "default" : "outline"}
                  size="sm"
                  onClick={() => void startDownload(m.id)}
                >
                  <Download className="size-4" /> Download
                </Button>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
