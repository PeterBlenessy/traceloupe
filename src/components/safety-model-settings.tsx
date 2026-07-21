import { useEffect } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  Ban,
  CheckCircle2,
  Download,
  HeartPulse,
  Loader2,
  XCircle,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useSafetyScan } from "@/components/safety-scan-provider";
import { client } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/**
 * Safety Scan model management (plan T2/T8): the download UI for the local
 * Gemma models. Lives in Settings → Safety, not the scan view, because it is a
 * one-time multi-GB setup concern rather than per-scan content.
 *
 * Also home to two visibility features the user asked for (NoteSage parity):
 * - a picker for *which* installed model scans (E2B is an explicit fallback);
 * - an on-demand health check that proves the local server actually loads.
 */

function gb(bytes: number) {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export function SafetyModelSettings() {
  const {
    download,
    downloadingModelId,
    startDownload,
    cancelDownload,
    preferredModelId,
    setPreferredModelId,
  } = useSafetyScan();
  const modelStatus = useQuery({
    queryKey: ["safetyScan", "modelStatus"],
    queryFn: () => client.getSafetyScanModelStatus(),
  });

  // The model that a scan will actually use: the user's pick when it's still
  // installed, otherwise the RAM-recommended tier the backend reports.
  const installed = modelStatus.data?.models.filter((m) => m.installed) ?? [];
  const effectiveModelId =
    preferredModelId && installed.some((m) => m.id === preferredModelId)
      ? preferredModelId
      : (modelStatus.data?.readyModelId ?? null);

  const health = useMutation({
    mutationFn: () => client.safetyScanHealthCheck(effectiveModelId),
  });
  // Drop a stale result when the target model changes.
  useEffect(() => {
    health.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [effectiveModelId]);

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
  const canPick = installed.length >= 2;

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        Downloaded once and checksum-verified. The model runs sandboxed with no
        network access — your data never leaves this Mac.
      </p>

      {/* The list is always rendered so it never jumps; progress appears inside
          the downloading model's own row. When two models are installed the
          rows become a picker for which one scans. Download outcomes are toasts
          (see the provider) — no red text in this pane. */}
      <RadioGroup
        value={effectiveModelId ?? ""}
        onValueChange={(id) => setPreferredModelId(id)}
        className="gap-2"
      >
        {ms.models.map((m) => {
          const isDownloading = downloadingModelId === m.id && download !== null;
          const isActive = canPick && m.id === effectiveModelId;
          return (
            <label
              key={m.id}
              htmlFor={`safety-model-${m.id}`}
              className={cn(
                "block rounded-md border p-3",
                canPick && m.installed && "cursor-pointer",
                isActive && "border-primary/60 bg-primary/5",
              )}
            >
              <div className="flex items-center justify-between gap-3">
                <div className="flex items-start gap-3">
                  {canPick && m.installed && (
                    <RadioGroupItem
                      id={`safety-model-${m.id}`}
                      value={m.id}
                      className="mt-1"
                    />
                  )}
                  <div>
                    <div className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
                      {m.displayName}
                      {m.recommended ? (
                        <Badge variant="secondary">Recommended for this Mac</Badge>
                      ) : (
                        <Badge variant="outline">Fallback</Badge>
                      )}
                      {isActive && (
                        <Badge className="bg-primary/15 text-primary border-transparent">
                          Used for scanning
                        </Badge>
                      )}
                    </div>
                    <div className="mt-0.5 text-xs text-muted-foreground">
                      {m.note}
                    </div>
                    <div className="mt-0.5 text-xs text-muted-foreground">
                      {gb(m.sizeBytes)} download
                      {m.installed && " · installed"}
                    </div>
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
                  !canPick && <Badge variant="outline">Installed</Badge>
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
            </label>
          );
        })}
      </RadioGroup>

      {/* Server health — NoteSage shows a persistent "server running, model
          loaded" light. Ours starts on demand for each scan, so we can't show
          an always-on light honestly; instead the user can run the check and
          confirm the model loads on this Mac right now. */}
      {effectiveModelId && (
        <div className="border-t pt-3">
          <div className="flex min-h-7 items-center gap-4">
            <div className="min-w-0 flex-1 text-sm font-medium">
              Server health
            </div>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="icon"
                  className="shrink-0"
                  disabled={health.isPending}
                  aria-label="Check model"
                  onClick={() => health.mutate()}
                >
                  {health.isPending ? (
                    <Loader2 className="size-4 animate-spin" />
                  ) : (
                    <HeartPulse className="size-4" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                Check the model — starts the server and confirms it loads
              </TooltipContent>
            </Tooltip>
          </div>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            The local model starts on demand for each scan and stops when it
            finishes. Run a check to confirm it loads and responds on this Mac.
          </p>
          {health.isPending && (
            <p className="mt-2 text-xs text-muted-foreground">
              Starting the server and loading the model…
            </p>
          )}
          {!health.isPending && health.data && (
            <p className="mt-2 flex items-center gap-1.5 text-xs text-foreground">
              {health.data.ok ? (
                <CheckCircle2 className="size-4 shrink-0 text-emerald-600 dark:text-emerald-400" />
              ) : (
                <XCircle className="size-4 shrink-0 text-destructive" />
              )}
              {health.data.message}
            </p>
          )}
          {!health.isPending && health.isError && (
            <p className="mt-2 flex items-center gap-1.5 text-xs text-foreground">
              <XCircle className="size-4 shrink-0 text-destructive" />
              {String(health.error)}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
