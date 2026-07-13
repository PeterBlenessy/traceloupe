import { createContext, useContext, useRef, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { client, type BackupInfo, type ImportProgress } from "@/lib/ipc";
import { ImportDialog } from "@/views/import-dialog";

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
    const off = await client.onImportProgress((p) =>
      setActive((a) => (a && a.backup.id === backup.id ? { ...a, progress: p } : a)),
    );
    unlisten.current = off;
    try {
      const result = await client.importBackup({
        backupPath: backup.path,
        backupId: backup.id,
        password,
        modules,
      });
      // Surface partial-failure warnings (a malformed artifact was skipped) so
      // they aren't lost — the import still succeeded for everything else.
      if (result.warnings.length > 0) {
        console.warn(
          `%c[salvage]%c import completed with ${result.warnings.length} warning(s):`,
          "color:#a78bfa;font-weight:600",
          "color:inherit",
          result.warnings,
        );
      }
      off();
      unlisten.current = null;
      qc.invalidateQueries();
      const wasForeground = foreground.current;
      setActive(null);
      // Foreground (dialog open): jump to the freshly imported data. Background:
      // leave the user where they are — the data is now available via invalidate.
      if (wasForeground) {
        setDialogBackup(null);
        navigate({ to: "/messages" });
      }
    } catch (e) {
      off();
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
