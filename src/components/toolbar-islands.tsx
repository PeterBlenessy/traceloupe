import type { ReactNode } from "react";
import {
  ArrowDownWideNarrow,
  ArrowUpNarrowWide,
  Clock,
} from "lucide-react";
import { filterPillClass, filterPillCount } from "@/components/filter-pill";
import { DateRangeFilter, type TimePreset } from "@/components/time-filter";
import type { BadgeFilterOption } from "@/components/badge-filter";
import type { SortField, SortState } from "@/components/sort-control";
import type { ToolbarIsland } from "@/components/adaptive-toolbar";
import { formatCount } from "@/lib/format";
import type { TimeRange } from "@/lib/ipc";

/** A single filter pill for an island item. */
function chip(
  active: boolean,
  onClick: () => void,
  label: ReactNode,
  count?: number,
  icon?: ReactNode,
) {
  return (
    <button type="button" onClick={onClick} className={filterPillClass(active)}>
      {icon}
      {label}
      {count != null && <span className={filterPillCount}>{formatCount(count)}</span>}
    </button>
  );
}

/** A single-select facet (source / type / lock / content-kind …) as an island. */
export function badgeIsland(opts: {
  key: string;
  label: string;
  icon: ReactNode;
  options: BadgeFilterOption[];
  value: string;
  onChange: (v: string) => void;
}): ToolbarIsland {
  const { key, label, icon, options, value, onChange } = opts;
  return {
    key,
    label,
    icon,
    active: options.length > 0 && value !== options[0].value,
    items: options.map((o) => ({
      key: o.value,
      active: value === o.value,
      node: chip(value === o.value, () => onChange(o.value), o.label, o.count, o.icon),
    })),
  };
}

/** The time-range island: preset chips + the custom Range picker. Presets with
 *  zero items are pushed to the end (folded into "⋮" first) so the visible chips
 *  are the ranges that actually contain data. */
export function timeIsland(opts: {
  presets: TimePreset[];
  counts?: (number | undefined)[];
  value: TimeRange;
  onChange: (r: TimeRange) => void;
}): ToolbarIsland {
  const { presets, counts, value, onChange } = opts;
  const activeKey =
    presets.find((p) => p.lo === value.lo && p.hi === value.hi)?.key ?? null;

  // "All" (index 0) stays first; the rest surface non-empty presets before empty.
  const rest = presets.slice(1).map((p, i) => ({ p, c: counts?.[i + 1] }));
  const nonEmpty = rest.filter((x) => x.c == null || x.c > 0);
  const empty = rest.filter((x) => x.c != null && x.c <= 0);
  const ordered = [{ p: presets[0], c: counts?.[0] }, ...nonEmpty, ...empty];

  const items = ordered
    .filter((x) => x.p)
    .map(({ p, c }) => ({
      key: p.key,
      active: activeKey === p.key,
      node: chip(activeKey === p.key, () => onChange({ lo: p.lo, hi: p.hi }), p.label, c),
    }));

  items.push({
    key: "__range",
    active: activeKey === null,
    node: (
      <DateRangeFilter value={value} active={activeKey === null} onChange={onChange} />
    ),
  });

  return {
    key: "time",
    label: "Time range",
    icon: <Clock className="size-4" />,
    active: activeKey !== "all" && activeKey !== presets[0]?.key,
    items,
  };
}

/** The sort island: a chip per sort field + a direction toggle. */
export function sortIsland(opts: {
  fields: SortField[];
  value: SortState;
  onChange: (s: SortState) => void;
}): ToolbarIsland {
  const { fields, value, onChange } = opts;
  const items = fields.map((f) => ({
    key: f.value,
    active: value.by === f.value,
    node: chip(value.by === f.value, () => onChange({ ...value, by: f.value }), f.label),
  }));
  items.push({
    key: "__dir",
    active: false,
    node: (
      <button
        type="button"
        onClick={() => onChange({ ...value, desc: !value.desc })}
        aria-label={value.desc ? "Descending — click for ascending" : "Ascending — click for descending"}
        title={value.desc ? "Descending" : "Ascending"}
        className={filterPillClass(false, "px-2")}
      >
        {value.desc ? (
          <ArrowDownWideNarrow className="size-3.5" />
        ) : (
          <ArrowUpNarrowWide className="size-3.5" />
        )}
      </button>
    ),
  });
  return {
    key: "sort",
    label: "Sort",
    icon: <ArrowDownWideNarrow className="size-4" />,
    items,
  };
}
