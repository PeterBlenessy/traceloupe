/**
 * Empty-state wording for a view whose data was **excluded from the import**.
 *
 * This is the third reason a view can be empty, and the app was collapsing it
 * into the first:
 *
 *   1. the backup does not contain it        — "No notes in this backup."
 *   2. this KIND of backup cannot contain it — `use-encrypted-only.ts`
 *   3. the person chose not to import it     — here
 *   4. a filter is hiding it                 — `empty-message.ts`
 *
 * Saying (1) when the truth is (3) tells someone their device holds no notes,
 * when in fact they unticked Notes on the import screen. That is the same class
 * of wrong answer the other two helpers exist to prevent, and the fix is the
 * same: say what is actually true, and say what to do about it.
 *
 * `importModules` is `null` when everything was imported, which is the default —
 * so an unconfigured import can never produce this message.
 */
import { useSettings } from "@/components/settings-provider";

/** Whether `moduleId` was left out of the import that produced the open backup. */
export function useModuleExcluded(moduleId: string | undefined): boolean {
  const { importModules } = useSettings();
  if (!moduleId || importModules == null) return false;
  return !importModules.includes(moduleId);
}

/**
 * The empty message for a view, accounting for a skipped import.
 *
 * `subject` is the plural thing the view lists, capitalised as a sentence would
 * start it ("Notes", "Voice recordings"). `plain` is the wording when the module
 * WAS imported and there is genuinely nothing.
 */
export function useNotImportedEmpty(
  moduleId: string | undefined,
  subject: string,
  plain: string,
): string {
  const excluded = useModuleExcluded(moduleId);
  if (!excluded) return plain;
  // Names the remedy, because the state is entirely recoverable — unlike an
  // unencrypted backup, which needs a new backup from the device.
  return `${subject} weren't included when this backup was imported. Re-import it with ${subject.toLowerCase()} selected to see them.`;
}
