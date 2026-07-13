import { ArrowDownWideNarrow, ArrowUpNarrowWide } from "lucide-react";
import { Button } from "@/components/ui/button";

/** One selectable sort field: the value sent to the backend + its label. */
export interface SortField {
  value: string;
  label: string;
}

/** The current sort: which field, and whether descending. */
export interface SortState {
  by: string;
  desc: boolean;
}

/**
 * Stable client-side sort for fully-loaded lists (Notes, Contacts, threads).
 * Nulls always sort last, regardless of direction — matching the backend's
 * `NULLS LAST/FIRST` handling for the windowed lists.
 */
export function sortItems<T>(
  items: T[],
  key: (t: T) => number | string | null | undefined,
  desc: boolean,
): T[] {
  const sign = desc ? -1 : 1;
  return [...items].sort((a, b) => {
    const ka = key(a) ?? null;
    const kb = key(b) ?? null;
    if (ka === null && kb === null) return 0;
    if (ka === null) return 1;
    if (kb === null) return -1;
    return ka < kb ? -sign : ka > kb ? sign : 0;
  });
}

/**
 * A compact list-sort control: a "sort by" dropdown plus a direction toggle.
 * Session-only — callers hold the state; nothing is persisted.
 */
export function SortControl({
  fields,
  value,
  onChange,
}: {
  fields: SortField[];
  value: SortState;
  onChange: (next: SortState) => void;
}) {
  return (
    <div className="flex items-center gap-1">
      <select
        value={value.by}
        onChange={(e) => onChange({ ...value, by: e.target.value })}
        aria-label="Sort by"
        className="rounded-md border bg-transparent px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        {fields.map((f) => (
          <option key={f.value} value={f.value}>
            {f.label}
          </option>
        ))}
      </select>
      <Button
        variant="ghost"
        size="icon"
        onClick={() => onChange({ ...value, desc: !value.desc })}
        aria-label={value.desc ? "Sort descending" : "Sort ascending"}
        title={value.desc ? "Descending — click for ascending" : "Ascending — click for descending"}
      >
        {value.desc ? (
          <ArrowDownWideNarrow className="size-4" />
        ) : (
          <ArrowUpNarrowWide className="size-4" />
        )}
      </Button>
    </div>
  );
}
