/**
 * Empty-state wording for data that only exists in an *encrypted* backup.
 *
 * iOS decides what goes into a backup via `Domains.plist`, and its
 * `RelativePathsToOnlyBackupEncrypted` list covers several stores TraceLoupe
 * surfaces — `Health` and `MedicalID` (the whole Health view), and
 * `Library/Safari/SafariTabs.db` (iCloud tabs). See
 * `docs/reference/backup-coverage-audit.md`.
 *
 * So on an unencrypted backup those views are legitimately empty — and saying
 * only "No health data in this backup" reads as *this person recorded none*,
 * which is a different and much stronger claim than *this kind of backup cannot
 * carry it*. Telling those two apart is the app's whole job; the
 * same reasoning is why deleted messages and trashed photos are shown with a
 * badge rather than filtered away.
 */
import { useQuery } from "@tanstack/react-query";

import { client } from "@/lib/ipc";

/** Whether the open backup is encrypted. `null` while unknown or not loaded. */
export function useBackupEncrypted(): boolean | null {
  const { data } = useQuery({
    queryKey: ["deviceInfo"],
    queryFn: () => client.deviceInfo(),
  });
  return data?.isEncrypted ?? null;
}

/**
 * Pick the empty message for a view whose data is encrypted-backup-only.
 *
 * `subject` names the missing data in the app's own words ("Health data",
 * "Tabs synced from your other Apple devices"). Only an explicit `false` swaps the wording —
 * unknown encryption state keeps the plain message, because claiming the
 * backup is unencrypted when we do not know would be its own wrong answer.
 */
export function useEncryptedOnlyEmpty(subject: string, plain: string): string {
  const encrypted = useBackupEncrypted();
  if (encrypted !== false) return plain;
  return `${subject} is only included in encrypted backups, and this backup is not encrypted. Re-run the backup with encryption turned on to see it.`;
}
