/**
 * A reusable time-filter toolbar: quick-preset chips (All · 24h · 7d · 30d ·
 * <year>, each with an optional count) plus a custom from–to date range. Emits a
 * half-open [lo, hi) epoch-second `TimeRange`. Shared by Timeline, Photos, and
 * Notes so they all filter by time the same way.
 */
import { useMemo, useState } from "react";
import { CalendarRange } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { formatCount } from "@/lib/format";
import type { TimeRange } from "@/lib/ipc";
import { cn } from "@/lib/utils";

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

/**
 * Anchor "now" once (stable across renders) and derive the presets from it, so
 * preset bounds and any count query keyed on them stay stable.
 */
export function useTimePresets(): { now: number; presets: TimePreset[] } {
  const [now] = useState(() => Math.floor(Date.now() / 1000));
  const presets = useMemo(() => makeTimePresets(now), [now]);
  return { now, presets };
}

/** The whole toolbar: preset chips + custom range, left-aligned. */
export function TimeFilterBar({
  presets,
  value,
  onChange,
  counts,
  className,
}: {
  presets: TimePreset[];
  value: TimeRange;
  onChange: (r: TimeRange) => void;
  /** Per-preset message/item counts, aligned to `presets`; optional. */
  counts?: (number | undefined)[];
  className?: string;
}) {
  // Which chip (if any) matches the active range; a custom range matches none.
  const activeKey =
    presets.find((p) => p.lo === value.lo && p.hi === value.hi)?.key ?? null;
  return (
    <div className={cn("flex flex-wrap items-center gap-1", className)}>
      {presets.map((p, i) => (
        <FilterChip
          key={p.key}
          label={p.label}
          count={counts?.[i]}
          active={activeKey === p.key}
          onClick={() => onChange({ lo: p.lo, hi: p.hi })}
        />
      ))}
      <DateRangeFilter
        value={value}
        active={activeKey === null}
        onChange={onChange}
      />
    </div>
  );
}

/** A pill toggle for a quick time filter, showing its count. */
function FilterChip({
  label,
  count,
  active,
  onClick,
}: {
  label: string;
  count: number | undefined;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      data-active={active}
      className="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground hover:bg-accent data-[active=true]:border-primary data-[active=true]:bg-primary/10 data-[active=true]:text-foreground"
    >
      {label}
      {count !== undefined && (
        <span className="tabular-nums opacity-70">{formatCount(count)}</span>
      )}
    </button>
  );
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
function DateRangeFilter({
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
        <button
          type="button"
          data-active={active}
          className="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground hover:bg-accent data-[active=true]:border-primary data-[active=true]:bg-primary/10 data-[active=true]:text-foreground"
        >
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
