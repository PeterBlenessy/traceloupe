/**
 * The generic renderer for a declarative artifact — a plain table, hostable
 * inside whichever view an artifact belongs to.
 *
 * A **component**, not a destination. The distinction matters and getting it
 * wrong is what #220 fixed: artifacts fold into the view closest in meaning
 * (permissions into Apps), and only genuinely homeless data gets its own screen.
 * A generic *table* was always right; a generic *place* was not.
 *
 * It knows no artifact by name. The backend describes each one — label,
 * description, column headers — and this renders whatever arrives, so a new TOML
 * module needs no change here at all.
 */
import { useMemo } from "react";

import { formatBytes, formatDateTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { ArtifactRow, ArtifactSummary } from "@/lib/ipc";

/** A value as text.
 *
 *  A column declared `timestamp` arrives as Unix seconds and renders as a date —
 *  the reason the module declares an epoch at all. Everything else shows as the
 *  module produced it: this component must not second-guess a value, because it
 *  cannot know what the artifact meant by it. */
export function cellText(
  value: ArtifactRow[string],
  isDate: boolean,
  isBytes = false,
): string {
  if (value === null || value === undefined) return "—";
  if (isDate && typeof value === "number") return formatDateTime(value);
  if (isBytes && typeof value === "number") return formatBytes(value);
  if (typeof value === "boolean") return value ? "Yes" : "No";
  return String(value);
}

/** Which columns render as dates: the ones the module DECLARED as timestamps.
 *
 *  This used to be inferred from the values — "is every number in a plausible
 *  date range?" — which guessed at a fact the module already states. The
 *  Bluetooth pairings module is the case that makes the difference concrete: its
 *  two columns hold device-relative counters, and the only thing keeping them
 *  from rendering as 1970s dates was the range test happening to exclude them.
 *  A counter that grew past 2001 would have started printing dates. */
export function dateColumns(artifact: ArtifactSummary): Set<string> {
  return new Set(artifact.timestampColumns ?? []);
}

export function ArtifactTable({
  artifact,
  rows,
  hideColumns = [],
  className,
}: {
  artifact: ArtifactSummary;
  rows: ArtifactRow[];
  /** Columns to leave out — a host that already identifies the row (an app's own
   *  bundle id, say) should not repeat it in every cell. */
  hideColumns?: string[];
  className?: string;
}) {
  const columns = useMemo(
    () => artifact.columns.filter((c) => !hideColumns.includes(c)),
    [artifact.columns, hideColumns],
  );
  const dates = useMemo(() => dateColumns(artifact), [artifact]);
  const bytes = useMemo(() => new Set(artifact.byteColumns ?? []), [artifact]);

  if (rows.length === 0) return null;

  // A real <table>, not flex rows. Equal-width `flex-1` columns truncated every
  // cell to the same narrow width, which turned a MAC address into
  // "Random 50:32:66:4…" and a UUID into "6C0C35A0-84CE-3…". In a tool whose
  // whole job is showing what a backup says, a value you cannot read is not
  // shown. A table sizes each column to its content and keeps the header aligned
  // with the body for free; anything still too wide scrolls horizontally inside
  // its own container rather than stretching the view.
  return (
    <div className={cn("min-w-0 overflow-x-auto", className)}>
      <table className="w-full border-collapse text-xs">
        <thead>
          <tr className="border-b">
            {columns.map((c) => (
              <th
                key={c}
                scope="col"
                className="whitespace-nowrap pr-4 pb-1 text-left text-2xs font-medium uppercase tracking-wide text-muted-foreground/70 last:pr-0"
              >
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i}>
              {columns.map((c) => (
                <td
                  key={c}
                  className={cn(
                    // `select-text` so a value can still be copied out; nothing
                    // is truncated now, so there is nothing hidden to copy.
                    "whitespace-nowrap py-1 pr-4 align-top select-text last:pr-0",
                    row[c] === null && "text-muted-foreground",
                  )}
                >
                  {cellText(row[c], dates.has(c), bytes.has(c))}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
