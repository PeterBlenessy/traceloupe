/**
 * Empty-state wording for a view whose data **was in the backup and could not
 * be read**.
 *
 * This is the fifth reason a view can be empty, and the app was collapsing it
 * into the first:
 *
 *   1. the backup does not contain it        — plain wording
 *   2. this KIND of backup cannot contain it — `use-encrypted-only.ts`
 *   3. the person chose not to import it     — `use-not-imported.ts`
 *   4. a filter is hiding it                 — `empty-message.ts`
 *   5. the parse failed                      — here
 *
 * It is the worst of the five, because it is the only one where the data is
 * there and the shortfall is ours. Saying (1) — "No calls in this backup." —
 * tells someone their device holds no calls when in truth a store was
 * malformed, a schema unrecognised, or decryption incomplete.
 *
 * That is not hypothetical. #268 was exactly this shape: `sms.db` decrypted
 * truncated, would not open, and Messages read as empty for months. The import
 * knew; it pushed a warning string and threw the fact away. Now it writes
 * `module_status`, and this hook reads it back.
 */
import { useQuery } from "@tanstack/react-query";

import { client, type ModuleStatus } from "@/lib/ipc";

/** Every module's outcome from the import that built the open cache. */
export function useModuleStatus(): ModuleStatus[] {
  const { data } = useQuery({
    queryKey: ["moduleStatus"],
    queryFn: () => client.moduleStatus(),
  });
  return data ?? [];
}

/** The failure detail for `moduleId`, or null when it parsed or was absent. */
export function useParseFailure(moduleId: string | undefined): string | null {
  const rows = useModuleStatus();
  if (!moduleId) return null;
  const row = rows.find((r) => r.module === moduleId);
  return row?.status === "failed" ? (row.detail ?? "") : null;
}

/**
 * The empty message for a view, accounting for a failed parse.
 *
 * `subject` is the thing the view lists, lowercase mid-sentence ("call
 * history", "messages"). `plain` is the wording for every other reason — pass
 * whatever the other four helpers already resolved to, so this one composes on
 * top rather than competing with them.
 *
 * Deliberately does NOT show the raw error: it names a file the reader has no
 * way to act on. The remedy that does work — take a fresh backup — is the one
 * offered. The detail stays available for the import warnings and the log.
 */
export function useParseFailedEmpty(
  moduleId: string | undefined,
  subject: string,
  plain: string,
): string {
  const failure = useParseFailure(moduleId);
  if (failure == null) return plain;
  return `We couldn't read the ${subject} in this backup — the store is there, but it wouldn't open. Taking a fresh backup from the device usually fixes it.`;
}
