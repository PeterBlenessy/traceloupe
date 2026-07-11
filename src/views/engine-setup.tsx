import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Cpu, Download, Loader2, TriangleAlert } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { client, type EngineProgress } from "@/lib/ipc";

type Stage =
  | { kind: "idle" }
  | { kind: "installing"; progress: EngineProgress | null }
  | { kind: "error"; message: string };

/**
 * Shown when no parsing engine is resolvable. Offers a one-click download of
 * the pinned engine (verified by checksum) when a build has been published;
 * otherwise explains the local dev setup.
 */
export function EngineSetup() {
  const qc = useQueryClient();
  const { data: info } = useQuery({ queryKey: ["engineInfo"], queryFn: () => client.engineInfo() });
  const [stage, setStage] = useState<Stage>({ kind: "idle" });
  const unlisten = useRef<(() => void) | null>(null);

  useEffect(
    () => () => {
      unlisten.current?.();
    },
    [],
  );

  async function install() {
    setStage({ kind: "installing", progress: null });
    const off = await client.onEngineProgress((p) =>
      setStage({ kind: "installing", progress: p }),
    );
    unlisten.current = off;
    try {
      await client.installEngine();
      off();
      // The engine is now resolvable — refresh status so the picker unlocks.
      qc.invalidateQueries({ queryKey: ["engineStatus"] });
      qc.invalidateQueries({ queryKey: ["engineInfo"] });
    } catch (e) {
      off();
      setStage({ kind: "error", message: String(e) });
    }
  }

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Cpu className="size-4" />
          <CardTitle>Parsing engine needed</CardTitle>
        </div>
        <CardDescription>
          Salvage reads backups with the iLEAPP engine ({info?.version ?? "…"}),
          which downloads once and is reused after. It isn't bundled, to keep the
          app small.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {stage.kind === "installing" ? (
          <InstallingView progress={stage.progress} />
        ) : info?.canDownload ? (
          <Button onClick={install}>
            <Download className="size-4" />
            Download engine
          </Button>
        ) : (
          <p className="text-sm text-muted-foreground">
            A downloadable build hasn't been published yet. For development, set
            it up locally with <code className="select-text">pnpm setup:engine</code>{" "}
            and launch with <code className="select-text">pnpm app:dev</code>.
          </p>
        )}
        {stage.kind === "error" && (
          <Alert variant="destructive">
            <TriangleAlert className="size-4" />
            <AlertTitle>Couldn't install the engine</AlertTitle>
            <AlertDescription className="select-text break-words">
              {stage.message}
            </AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
  );
}

function InstallingView({ progress }: { progress: EngineProgress | null }) {
  const downloading = progress?.phase === "downloading" ? progress : null;
  const verifying = progress?.phase === "verifying";
  const pct = verifying ? 100 : downloading ? Math.round(downloading.fraction * 100) : 2;
  const mb = (n: number) => (n / 1_000_000).toFixed(0);
  const label = verifying
    ? "Verifying…"
    : downloading
      ? `Downloading ${mb(downloading.received)} / ${mb(downloading.total)} MB`
      : "Starting…";
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" />
        <span>{label}</span>
      </div>
      <Progress value={pct} />
    </div>
  );
}
