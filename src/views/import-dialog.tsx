import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useSettings } from "@/components/settings-provider";
import { useImport } from "@/components/import-provider";
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

/**
 * The import dialog — a view of the ImportProvider. The provider owns the actual
 * import (so it survives "run in background" and navigation); this renders the
 * password step, live progress, or an error for one backup.
 */
export function ImportDialog({
  backup,
  autoStart = false,
}: {
  backup: BackupInfo;
  /** Start reading immediately without a password step (unencrypted backups). */
  autoStart?: boolean;
}) {
  const { active, start, runInBackground, stop, close, error } = useImport();
  const running = active?.backup.id === backup.id;
  const progress = running ? active!.progress : null;
  const errorMessage = error?.backupId === backup.id ? error.message : null;

  const [password, setPassword] = useState("");
  const startedRef = useRef(false);

  // Resolve which data types to import: the user's saved choice, or the catalog
  // defaults if they haven't customized it. The password never leaves this
  // component except as an invoke() argument; it is not stored.
  const { importModules } = useSettings();
  const { data: catalog } = useQuery({
    queryKey: ["importModules"],
    queryFn: () => client.listImportModules(),
  });
  const modules =
    importModules ?? catalog?.filter((m) => m.default).map((m) => m.id) ?? [];

  // Unencrypted backups: kick off the read as soon as the dialog opens.
  useEffect(() => {
    if (autoStart && !running && !errorMessage && !startedRef.current) {
      startedRef.current = true;
      start(backup, "", modules);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoStart, running, errorMessage]);

  // Encryption drives the prompt: only encrypted backups need a password.
  const encrypted = backup.isEncrypted === true;
  const showPasswordField = backup.isEncrypted !== false;
  const canImport = !encrypted || password.length > 0;

  const prompt = encrypted
    ? "This backup is encrypted. Enter its password to open it — TraceLoupe reads it once, then it's instant."
    : backup.isEncrypted === false
      ? "This backup isn't encrypted."
      : "Enter the backup password if it's encrypted."; // encryption unknown

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (next) return;
        if (running) return; // can't dismiss mid-import (use Stop / Run in background)
        close();
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

        {running ? (
          <RunningView
            progress={progress}
            onStop={stop}
            onRunInBackground={runInBackground}
          />
        ) : errorMessage ? (
          <div className="space-y-4">
            <Alert variant="destructive">
              <TriangleAlert className="size-4" />
              <AlertTitle>Couldn't open the backup</AlertTitle>
              <AlertDescription className="select-text break-words">
                {errorMessage}
              </AlertDescription>
            </Alert>
            <DialogFooter>
              <Button variant="ghost" onClick={close}>
                Close
              </Button>
              <Button onClick={() => start(backup, password, modules)}>Try again</Button>
            </DialogFooter>
          </div>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (canImport) start(backup, password, modules);
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
              <Button type="button" variant="ghost" onClick={close}>
                Cancel
              </Button>
              <Button type="submit" disabled={!canImport}>
                Open
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function RunningView({
  progress,
  onStop,
  onRunInBackground,
}: {
  progress: ImportProgress | null;
  onStop: () => void;
  onRunInBackground: () => void;
}) {
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
      <DialogFooter className="gap-2 sm:justify-between">
        <Button variant="ghost" onClick={onStop}>
          Stop import
        </Button>
        <Button variant="secondary" onClick={onRunInBackground}>
          Run in background
        </Button>
      </DialogFooter>
    </div>
  );
}
