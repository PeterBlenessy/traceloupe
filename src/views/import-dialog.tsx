import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useSettings } from "@/components/settings-provider";
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
  autoStart = false,
  open,
  onOpenChange,
  onDone,
}: {
  backup: BackupInfo;
  /** Start reading immediately without a password step (unencrypted backups). */
  autoStart?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const [stage, setStage] = useState<Stage>(() =>
    autoStart ? { kind: "running", progress: null } : { kind: "password" },
  );
  const [password, setPassword] = useState("");
  const qc = useQueryClient();
  // Resolve which data types to import: the user's saved choice, or the catalog
  // defaults if they haven't customized it.
  const { importModules } = useSettings();
  const { data: catalog } = useQuery({
    queryKey: ["importModules"],
    queryFn: () => client.listImportModules(),
  });
  const modules =
    importModules ?? catalog?.filter((m) => m.default).map((m) => m.id) ?? [];
  // Keep the latest progress even across re-subscribes.
  const unlisten = useRef<(() => void) | null>(null);
  const started = useRef(false);

  useEffect(() => {
    if (!open) {
      started.current = false;
      setStage(autoStart ? { kind: "running", progress: null } : { kind: "password" });
      setPassword("");
    }
    return () => {
      unlisten.current?.();
      unlisten.current = null;
    };
  }, [open, autoStart]);

  // Unencrypted backups: kick off the read as soon as the dialog opens.
  useEffect(() => {
    if (open && autoStart && !started.current) {
      started.current = true;
      void runImport();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, autoStart]);

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
        modules,
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

  // Encryption drives the prompt: only encrypted backups need a password.
  // `isEncrypted` is null when the flag couldn't be read — treat as optional.
  const encrypted = backup.isEncrypted === true;
  const showPasswordField = backup.isEncrypted !== false;
  const canImport = !encrypted || password.length > 0;

  const prompt = encrypted
    ? "This backup is encrypted. Enter its password to open it — Salvage reads it once, then it's instant."
    : backup.isEncrypted === false
      ? "This backup isn't encrypted."
      : "Enter the backup password if it's encrypted."; // encryption unknown

  // While an import is running the dialog is fully modal: the work runs in the
  // background and dismissing it (outside-click / Escape / close button) would
  // orphan the in-flight import and restart it on reopen. So block dismissal.
  const running = stage.kind === "running";

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && running) return; // can't close mid-import
        onOpenChange(next);
      }}
    >
      <DialogContent
        className="sm:max-w-md"
        showCloseButton={!running}
        onPointerDownOutside={(e) => running && e.preventDefault()}
        onInteractOutside={(e) => running && e.preventDefault()}
        onEscapeKeyDown={(e) => running && e.preventDefault()}
      >
        <DialogHeader>
          <DialogTitle>{backup.deviceName ?? backup.id}</DialogTitle>
          <DialogDescription>
            {backup.productVersion ? `iOS ${backup.productVersion} · ` : ""}
            {prompt} Everything stays on this Mac.
          </DialogDescription>
        </DialogHeader>

        {stage.kind === "password" && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (canImport) void runImport();
            }}
            className="space-y-4"
          >
            {showPasswordField && (
              <div className="relative">
                <Lock className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
                <Input
                  type="password"
                  autoFocus
                  placeholder={encrypted ? "Backup password" : "Backup password (optional)"}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  className="pl-8 select-text"
                />
              </div>
            )}
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!canImport}>
                Open
              </Button>
            </DialogFooter>
          </form>
        )}

        {stage.kind === "running" && <RunningView progress={stage.progress} />}

        {stage.kind === "error" && (
          <div className="space-y-4">
            <Alert variant="destructive">
              <TriangleAlert className="size-4" />
              <AlertTitle>Couldn't open the backup</AlertTitle>
              <AlertDescription className="select-text break-words">
                {stage.message}
              </AlertDescription>
            </Alert>
            <DialogFooter>
              <Button variant="ghost" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button
                onClick={() =>
                  autoStart ? void runImport() : setStage({ kind: "password" })
                }
              >
                Try again
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

function RunningView({ progress }: { progress: ImportProgress | null }) {
  const parsing = progress?.phase === "parsing" ? progress : null;
  const normalizing = progress?.phase === "normalizing" ? progress : null;
  // During parsing show real fraction; during normalizing show a full-ish bar
  // (no per-row fraction) with the live sub-step so it doesn't look stuck.
  const pct = normalizing ? 100 : parsing ? Math.round(parsing.fraction * 100) : 3;
  const label = normalizing
    ? `Organizing ${normalizing.step.toLowerCase()}…`
    : parsing
      ? `Reading ${parsing.artifact} (${parsing.current}/${parsing.total})`
      : "Opening the backup…";

  return (
    <div className="space-y-3 py-2">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" />
        <span className="truncate">{label}</span>
      </div>
      <Progress value={pct} />
      <p className="text-xs text-muted-foreground">
        Reading this backup for the first time. It opens instantly next time.
      </p>
    </div>
  );
}
