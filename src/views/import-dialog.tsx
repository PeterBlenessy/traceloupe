import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Lock, Loader2, TriangleAlert } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { client, type BackupInfo, type ImportProgress } from "@/lib/ipc";

type Stage =
  | { kind: "password" }
  | { kind: "running"; progress: ImportProgress | null }
  | { kind: "error"; message: string };

/**
 * Password prompt → import with live progress. On success the caller's
 * onDone fires (used to route into Messages). The password never leaves
 * this component except as an invoke() argument; it is not stored.
 */
export function ImportDialog({
  backup,
  open,
  onOpenChange,
  onDone,
}: {
  backup: BackupInfo;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const [stage, setStage] = useState<Stage>({ kind: "password" });
  const [password, setPassword] = useState("");
  const qc = useQueryClient();
  // Keep the latest progress even across re-subscribes.
  const unlisten = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (!open) {
      setStage({ kind: "password" });
      setPassword("");
    }
    return () => {
      unlisten.current?.();
      unlisten.current = null;
    };
  }, [open]);

  async function runImport() {
    setStage({ kind: "running", progress: null });
    const off = await client.onImportProgress((p) =>
      setStage({ kind: "running", progress: p }),
    );
    unlisten.current = off;
    try {
      const result = await client.importBackup({
        backupPath: backup.path,
        backupId: backup.id,
        password,
      });
      off();
      unlisten.current = null;
      // Refresh any cached queries that now have data.
      qc.invalidateQueries();
      void result;
      onDone();
    } catch (e) {
      off();
      unlisten.current = null;
      setStage({ kind: "error", message: String(e) });
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{backup.deviceName ?? backup.id}</DialogTitle>
          <DialogDescription>
            {backup.productVersion ? `iOS ${backup.productVersion} · ` : ""}
            This backup is encrypted. Enter its password to import and browse it.
            Everything stays on this Mac.
          </DialogDescription>
        </DialogHeader>

        {stage.kind === "password" && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (password) void runImport();
            }}
            className="space-y-4"
          >
            <div className="relative">
              <Lock className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
              <Input
                type="password"
                autoFocus
                placeholder="Backup password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                className="pl-8 select-text"
              />
            </div>
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!password}>
                Import
              </Button>
            </DialogFooter>
          </form>
        )}

        {stage.kind === "running" && <RunningView progress={stage.progress} />}

        {stage.kind === "error" && (
          <div className="space-y-4">
            <Alert variant="destructive">
              <TriangleAlert className="size-4" />
              <AlertTitle>Import failed</AlertTitle>
              <AlertDescription className="select-text break-words">
                {stage.message}
              </AlertDescription>
            </Alert>
            <DialogFooter>
              <Button variant="ghost" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button onClick={() => setStage({ kind: "password" })}>Try again</Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

function RunningView({ progress }: { progress: ImportProgress | null }) {
  const parsing = progress?.phase === "parsing" ? progress : null;
  const normalizing = progress?.phase === "normalizing";
  // During parsing show real fraction; during normalizing show indeterminate-ish full bar.
  const pct = normalizing ? 100 : parsing ? Math.round(parsing.fraction * 100) : 3;
  const label = normalizing
    ? "Organizing results…"
    : parsing
      ? `Parsing ${parsing.artifact} (${parsing.current}/${parsing.total})`
      : "Starting the parsing engine…";

  return (
    <div className="space-y-3 py-2">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" />
        <span className="truncate">{label}</span>
      </div>
      <Progress value={pct} />
      <p className="text-xs text-muted-foreground">
        First import parses the whole backup once. Browsing is instant afterward.
      </p>
    </div>
  );
}
