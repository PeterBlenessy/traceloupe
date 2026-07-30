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

import { formatDateTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { ArtifactRow, ArtifactSummary } from "@/lib/ipc";

/** A value as text.
 *
 *  A column declared `timestamp` arrives as Unix seconds and renders as a date —
 *  the reason the module declares an epoch at all. Everything else shows as the
 *  module produced it: this component must not second-guess a value, because it
 *  cannot know what the artifact meant by it. */
export function cellText(value: ArtifactRow[string], isDate: boolean): string {
  if (value === null || value === undefined) return "—";
  if (isDate && typeof value === "number") return formatDateTime(value);
  if (typeof value === "boolean") return value ? "Yes" : "No";
  return String(value);
}

/** Columns whose values look like Unix-second timestamps.
 *
 *  Inferred from values because the summary carries column names but not their
 *  kinds. That is the wrong long-term answer and is marked as such — the module
 *  already knows, and the kind belongs in the summary. Kept narrow (a plausible
 *  date range, numbers only) so a count or an id cannot be mistaken for a date. */
export function dateColumns(columns: string[], rows: ArtifactRow[]): Set<string> {
  const out = new Set<string>();
  for (const col of columns) {
    const values = rows.map((r) => r[col]).filter((v) => v !== null && v !== undefined);
    if (values.length === 0) continue;
    if (values.every((v) => typeof v === "number" && v > 946_684_800 && v < 4_102_444_800)) {
      out.add(col);
    }
  }
  return out;
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
  const dates = useMemo(() => dateColumns(columns, rows), [columns, rows]);

  if (rows.length === 0) return null;

  return (
    <div className={cn("min-w-0", className)}>
      <div className="flex gap-3 border-b pb-1 text-2xs font-medium uppercase tracking-wide text-muted-foreground/70">
        {columns.map((c) => (
          <span key={c} className="min-w-0 flex-1 truncate">
            {c}
          </span>
        ))}
      </div>
      {/* No per-cell tooltip: a native `title=` is banned (it looks nothing like
          the rest of the app) and wrapping every cell in the shared Tooltip is a
          lot of machinery for a hover nobody asked for. Cells are selectable so
          a long value can be copied out instead. */}
      {rows.map((row, i) => (
        <div key={i} className="flex gap-3 py-1 text-xs">
          {columns.map((c) => (
            <span
              key={c}
              className={cn(
                "min-w-0 flex-1 select-text truncate",
                row[c] === null && "text-muted-foreground",
              )}
            >
              {cellText(row[c], dates.has(c))}
            </span>
          ))}
        </div>
      ))}
    </div>
  );
}
