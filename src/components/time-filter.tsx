/**
 * Time-filter building blocks for the shared toolbar filter: the quick presets
 * (`useTimePresets` / `makeTimePresets` / `makeYearPresets`) fed into `timeGroup`
 * (see `filter-groups.tsx`), and the custom from–to `DateRangeFilter` it renders.
 * All emit a half-open [lo, hi) epoch-second `TimeRange`.
 */
import { useMemo, useState } from "react";
import { CalendarRange } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { filterPillClass } from "@/components/filter-pill";
import type { TimeRange } from "@/lib/ipc";

/** A cumulative quick-filter: everything since `lo` (null = no lower bound). */
export type TimePreset = {
  key: string;
  label: string;
  lo: number | null;
  hi: number | null;
};

/**
 * The quick time filters shown as chips, anchored at `now` (epoch seconds):
 * All, then cumulative recency windows, then the current calendar year. Each
 * includes everything more recent (they are filters, not disjoint buckets).
 */
export function makeTimePresets(now: number): TimePreset[] {
  const DAY = 86_400;
  const year = new Date(now * 1000).getFullYear();
  const yearStart = Math.floor(new Date(year, 0, 1).getTime() / 1000);
  return [
    { key: "all", label: "All", lo: null, hi: null },
    { key: "24h", label: "24h", lo: now - DAY, hi: null },
    { key: "7d", label: "7d", lo: now - 7 * DAY, hi: null },
    { key: "30d", label: "30d", lo: now - 30 * DAY, hi: null },
    { key: "year", label: String(year), lo: yearStart, hi: null },
  ];
}

/** Disjoint per-calendar-year presets, newest year first, for the inclusive
 *  span `[minYear, maxYear]`. Each is `[Jan 1, next Jan 1)` in local time. */
export function makeYearPresets(minYear: number, maxYear: number): TimePreset[] {
  const out: TimePreset[] = [];
  for (let y = maxYear; y >= minYear; y--) {
    out.push({
      key: `y${y}`,
      label: String(y),
      lo: Math.floor(new Date(y, 0, 1).getTime() / 1000),
      hi: Math.floor(new Date(y + 1, 0, 1).getTime() / 1000),
    });
  }
  return out;
}

/**
 * Anchor "now" once (stable across renders) and derive the presets from it, so
 * preset bounds and any count query keyed on them stay stable.
 */
export function useTimePresets(): { now: number; presets: TimePreset[] } {
  const [now] = useState(() => Math.floor(Date.now() / 1000));
  const presets = useMemo(() => makeTimePresets(now), [now]);
  return { now, presets };
}

/** Epoch seconds → a `yyyy-mm-dd` string for a native date input (local time). */
function toDateInput(epochSeconds: number): string {
  const d = new Date(epochSeconds * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/**
 * A custom from–to date filter. Emits a half-open [lo, hi) epoch-second range
 * where the "to" day is inclusive (hi = start of the following day). Reflects
 * the bounds only while a custom range is active — a preset shouldn't populate
 * the date fields.
 */
export function DateRangeFilter({
  value,
  active,
  onChange,
}: {
  value: TimeRange;
  active: boolean;
  onChange: (r: TimeRange) => void;
}) {
  const fromStr = active && value.lo != null ? toDateInput(value.lo) : "";
  const toStr = active && value.hi != null ? toDateInput(value.hi - 1) : "";

  const apply = (from: string, to: string) => {
    const lo = from
      ? Math.floor(new Date(`${from}T00:00:00`).getTime() / 1000)
      : null;
    const hi = to
      ? Math.floor(new Date(`${to}T00:00:00`).getTime() / 1000) + 86_400
      : null;
    onChange({ lo, hi });
  };

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button type="button" className={filterPillClass(active)}>
          <CalendarRange className="size-3.5" />
          {active ? `${fromStr || "…"} – ${toStr || "…"}` : "Range"}
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-64 space-y-3">
        <label className="block space-y-1">
          <span className="text-xs text-muted-foreground">From</span>
          <input
            type="date"
            value={fromStr}
            onChange={(e) => apply(e.target.value, toStr)}
            className="w-full rounded-md border bg-transparent px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        </label>
        <label className="block space-y-1">
          <span className="text-xs text-muted-foreground">To</span>
          <input
            type="date"
            value={toStr}
            onChange={(e) => apply(fromStr, e.target.value)}
            className="w-full rounded-md border bg-transparent px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        </label>
        <Button
          variant="ghost"
          size="sm"
          className="w-full"
          onClick={() => onChange({ lo: null, hi: null })}
        >
          Clear
        </Button>
      </PopoverContent>
    </Popover>
  );
}
