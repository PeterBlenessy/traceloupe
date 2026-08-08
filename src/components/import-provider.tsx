import { createContext, useContext, useEffect, useRef, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { client, type BackupInfo, type ImportProgress } from "@/lib/ipc";
import { ImportDialog } from "@/views/import-dialog";
import { toast } from "sonner";

/**
 * Owns the import lifecycle above the routes, so an import survives closing its
 * dialog ("run in background") and navigating away. The dialog and the
 * background indicator are views of this state; the picker just calls `open()`.
 */
type ActiveImport = { backup: BackupInfo; progress: ImportProgress | null };

type ImportContextValue = {
  /** The import currently running (may be backgrounded), or null. */
  active: ActiveImport | null;
  /** True when a running import's dialog is hidden (running in background). */
  backgrounded: boolean;
  /** Open the import dialog for a backup (password step / auto-start). */
  open: (backup: BackupInfo) => void;
  /** Begin the import (called by the dialog on submit / auto-start). */
  start: (backup: BackupInfo, password: string, modules: string[]) => void;
  /** Hide the dialog but keep the import running. */
  runInBackground: () => void;
  /** Reopen the dialog for the backgrounded import. */
  reopen: () => void;
  /** Stop the running import and close the dialog. */
  stop: () => void;
  /** Close the dialog when no import is running (password / error stage). */
  close: () => void;
  /** The last import error, keyed to its backup (for the dialog's error view). */
  error: { backupId: string; message: string } | null;
};

const ImportContext = createContext<ImportContextValue | null>(null);

export function ImportProvider({ children }: { children: React.ReactNode }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  // The backup whose dialog is open (null = dialog hidden/closed).
  const [dialogBackup, setDialogBackup] = useState<BackupInfo | null>(null);
  const [active, setActive] = useState<ActiveImport | null>(null);
  const [error, setError] = useState<{ backupId: string; message: string } | null>(null);
  const unlisten = useRef<(() => void) | null>(null);
  const stopped = useRef(false);
  // Mirror of dialogBackup for the async completion handler (avoids stale reads).
  const foreground = useRef(false);
  foreground.current = dialogBackup !== null;

  // An import runs in the Rust process and survives a webview reload; this React
  // state does not — so on mount, re-attach to whatever is in flight (#72).
  // Without this a reload showed no progress AND re-clicking Import collided
  // with the backend's ImportGate, erroring while the original import was still
  // writing the cache. Mirrors what SafetyScanProvider does for scans.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const live = await client.getImportStatus();
      if (cancelled || !live) return;
      // The backup list is the only place with the full BackupInfo; fall back to
      // a minimal stand-in so progress still shows if it isn't loaded yet.
      const backups = await client.listBackups().catch(() => null);
      const backup =
        backups?.status === "ok"
          ? (backups.backups.find((b) => b.id === live.backupId) ?? null)
          : null;
      if (cancelled) return;
      setActive({
        backup: backup ?? ({ id: live.backupId, path: "" } as BackupInfo),
        progress: live.event,
      });
      await subscribeImport();
    })();
    return () => {
      cancelled = true;
      unlisten.current?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Subscribe to import progress exactly once — shared by `start` and the
  // rehydration effect above, so both attach the SAME listener.
  const subscribeImport = async () => {
    if (unlisten.current) return;
    unlisten.current = () => {}; // claim synchronously against a double-call
    unlisten.current = await client.onImportProgress((p) =>
      setActive((a) => (a ? { ...a, progress: p } : a)),
    );
  };

  const open = (backup: BackupInfo) => {
    setError(null);
    setDialogBackup(backup);
  };
  const close = () => setDialogBackup(null);
  const runInBackground = () => setDialogBackup(null);
  const reopen = () => {
    if (active) setDialogBackup(active.backup);
  };

  const start = async (backup: BackupInfo, password: string, modules: string[]) => {
    stopped.current = false;
    setError(null);
    setActive({ backup, progress: null });
    await subscribeImport();
    try {
      const result = await client.importBackup({
        backupPath: backup.path,
        backupId: backup.id,
        password,
        modules,
      });
      // Partial-failure warnings (a malformed store was skipped) must reach the
      // PERSON, not the devtools console. The comment here used to say "so they
      // aren't lost" above a `console.warn` — which loses them: an import that
      // skipped data reported unqualified success, and this app's whole claim is
      // telling "we read it" apart from "we could not".
      //
      // Shown for long enough to read and act on, listing what was skipped
      // rather than only how many things were.
      if (result.warnings.length > 0) {
        const n = result.warnings.length;
        // The warnings themselves are diagnostics — SQLite column types, missing
        // columns, parser internals. They belong in the log, and they are in it.
        // Putting them in a toast asks the reader to act on a sentence they have
        // no way to act on, and buries the one fact that matters: some data in
        // this backup could not be read, so a view being empty may be on us.
        //
        // The same call the empty states make (`use-parse-failed`): say what it
        // means, say where it shows up, keep the detail available in the log.
        toast.warning("Import finished, with some data skipped", {
          description:
            `${n === 1 ? "One part of" : `${n} parts of`} this backup couldn't be read. ` +
            "The views affected will say so instead of looking empty. " +
            "Technical details are in the log.",
          duration: 12_000,
        });
        console.warn("[traceloupe] import warnings:", result.warnings);
      }
      unlisten.current?.();
      unlisten.current = null;
      // The import made this backup active on the backend. Set that optimistically
      // BEFORE invalidating: queries use staleTime: Infinity, so without this the
      // navigated-to view could read a stale `hasActiveBackup: false`, show "No
      // backup open", and bounce the user back to click Open again.
      qc.setQueryData(["hasActiveBackup"], true);
      qc.invalidateQueries();
      const wasForeground = foreground.current;
      setActive(null);
      // Foreground (dialog open): jump to the freshly imported data. Background:
      // leave the user where they are — the data is now available via invalidate.
      if (wasForeground) {
        setDialogBackup(null);
        // Land on the condensed Device view (`/`), the default post-open route.
        navigate({ to: "/" });
      }
    } catch (e) {
      unlisten.current?.();
      unlisten.current = null;
      setActive(null);
      if (stopped.current) return; // user hit Stop; nothing to show
      setError({ backupId: backup.id, message: String(e) });
      setDialogBackup(backup); // surface the error in the dialog
    }
  };

  const stop = () => {
    stopped.current = true;
    void client.cancelImport();
    setActive(null);
    setDialogBackup(null);
  };

  return (
    <ImportContext.Provider
      value={{
        active,
        backgrounded: active !== null && dialogBackup === null,
        open,
        start,
        runInBackground,
        reopen,
        stop,
        close,
        error,
      }}
    >
      {children}
      {dialogBackup && (
        <ImportDialog
          backup={dialogBackup}
          autoStart={dialogBackup.isEncrypted !== true}
        />
      )}
    </ImportContext.Provider>
  );
}

export function useImport() {
  const ctx = useContext(ImportContext);
  if (!ctx) throw new Error("useImport must be used within an ImportProvider");
  return ctx;
}
