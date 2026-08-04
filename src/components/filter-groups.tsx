import type { ReactNode } from "react";
import { filterPillClass, filterPillCount } from "@/components/filter-pill";
import { DateRangeFilter, type TimePreset } from "@/components/time-filter";
import type { BadgeFilterOption } from "@/components/badge-filter";
import { formatCount } from "@/lib/format";
import type { TimeRange } from "@/lib/ipc";

/**
 * The data model behind the **Filter** control (see {@link FilterControl}).
 *
 * A view contributes only the groups it actually has data for — a Notes backup
 * with no locked notes contributes no "Lock" group at all (not a disabled one).
 * Within a present group individual pills may still be disabled (e.g. a time
 * preset whose window holds no items), because the group itself is stably shown.
 */
export interface FilterPill {
  key: string;
  label: string;
  icon?: ReactNode;
  count?: number;
  selected: boolean;
  /** Greyed + non-interactive (e.g. a time window with zero items). */
  disabled?: boolean;
  onSelect: () => void;
}

/** One labelled row in the filter panel (Time / Folder / Tags / …). */
export interface FilterGroup {
  key: string;
  /** Row heading, e.g. "Time". */
  label: string;
  /** One line telling the user what the group means. */
  description: string;
  pills: FilterPill[];
  /** An extra control after the pills (e.g. the custom date-range picker). */
  extra?: ReactNode;
  /** The non-default selections in this group, for the closed-state summary. */
  summary: FilterSummary[];
}

/** A single active selection, surfaced as a removable chip when the panel is
 *  closed. */
export interface FilterSummary {
  key: string;
  label: string;
  icon?: ReactNode;
  /** Reset just this selection back to its default. */
  onClear: () => void;
}

/** A single-select facet (source / folder / tags / lock …) as a filter group.
 *  `options[0]` is treated as the "all"/default; picking it clears the group. */
export function badgeGroup(opts: {
  key: string;
  label: string;
  description: string;
  options: BadgeFilterOption[];
  value: string;
  onChange: (v: string) => void;
}): FilterGroup {
  const { key, label, description, options, value, onChange } = opts;
  const dflt = options[0]?.value;
  const selectedOpt = options.find((o) => o.value === value);
  return {
    key,
    label,
    description,
    pills: options.map((o) => ({
      key: o.value,
      label: o.label,
      icon: o.icon,
      count: o.count,
      selected: value === o.value,
      onSelect: () => onChange(o.value),
    })),
    summary:
      dflt != null && value !== dflt && selectedOpt
        ? [
            {
              key,
              label: selectedOpt.label,
              icon: selectedOpt.icon,
              onClear: () => onChange(dflt),
            },
          ]
        : [],
  };
}

/** A multi-select facet (e.g. tags): no "all" pill — an empty selection means
 *  "all", and each selected value becomes its own removable summary chip. */
export function multiBadgeGroup(opts: {
  key: string;
  label: string;
  description: string;
  options: BadgeFilterOption[];
  selected: string[];
  /** Toggle a value in/out of the selection. */
  onToggle: (value: string) => void;
}): FilterGroup {
  const { key, label, description, options, selected, onToggle } = opts;
  const sel = new Set(selected);
  return {
    key,
    label,
    description,
    pills: options.map((o) => ({
      key: o.value,
      label: o.label,
      icon: o.icon,
      count: o.count,
      selected: sel.has(o.value),
      onSelect: () => onToggle(o.value),
    })),
    // Everything selected is not a narrowing, so it earns no chips. Without
    // this, a filter nobody has touched fills the toolbar with one chip per
    // option — six of them on Health — which reads as an active filter and is
    // the opposite of what it means.
    summary:
      options.every((o) => sel.has(o.value))
        ? []
        : options
            .filter((o) => sel.has(o.value))
            .map((o) => ({
              key: `${key}:${o.value}`,
              label: o.label,
              icon: o.icon,
              onClear: () => onToggle(o.value),
            })),
  };
}

/** The time-range group: a pill per preset (empty windows disabled) plus the
 *  custom Range picker. */
export function timeGroup(opts: {
  description: string;
  presets: TimePreset[];
  counts?: (number | undefined)[];
  value: TimeRange;
  onChange: (r: TimeRange) => void;
}): FilterGroup {
  const { description, presets, counts, value, onChange } = opts;
  const activeKey =
    presets.find((p) => p.lo === value.lo && p.hi === value.hi)?.key ?? null;
  const all = presets[0];

  const pills: FilterPill[] = presets.map((p, i) => {
    const c = counts?.[i];
    return {
      key: p.key,
      label: p.label,
      count: c,
      selected: activeKey === p.key,
      // "All" is never disabled; an otherwise-empty window is greyed out.
      disabled: p.key !== all?.key && c != null && c <= 0,
      onSelect: () => onChange({ lo: p.lo, hi: p.hi }),
    };
  });

  const summary: FilterSummary[] = [];
  if (activeKey === null) {
    summary.push({
      key: "time",
      label: "Custom range",
      onClear: () => all && onChange({ lo: all.lo, hi: all.hi }),
    });
  } else if (all && activeKey !== all.key) {
    const p = presets.find((x) => x.key === activeKey);
    if (p)
      summary.push({
        key: "time",
        label: p.label,
        onClear: () => onChange({ lo: all.lo, hi: all.hi }),
      });
  }

  return {
    key: "time",
    label: "Time",
    description,
    pills,
    extra: (
      <DateRangeFilter value={value} active={activeKey === null} onChange={onChange} />
    ),
    summary,
  };
}

function rangeEq(a: TimeRange, b: TimeRange): boolean {
  return a.lo === b.lo && a.hi === b.hi;
}

/** MULTI-select time filter: several presets/ranges can be active at once (e.g.
 *  two years), unioned. An empty selection means "all time" — the same
 *  "empty = everything" rule as {@link multiBadgeGroup}. `presets[0]` is the
 *  "All" pill and clears the selection. Each active range becomes a removable
 *  chip; the custom picker appends a range. */
export function multiTimeGroup(opts: {
  description: string;
  presets: TimePreset[];
  counts?: (number | undefined)[];
  values: TimeRange[];
  /** Add the range if absent, remove it if already selected. */
  onToggle: (r: TimeRange) => void;
  /** Reset to "all time" (empty selection). */
  onClear: () => void;
}): FilterGroup {
  const { description, presets, counts, values, onToggle, onClear } = opts;
  const all = presets[0];
  const selected = (p: TimePreset) =>
    values.some((v) => rangeEq(v, { lo: p.lo, hi: p.hi }));

  const pills: FilterPill[] = presets.map((p, i) => {
    const isAll = p.key === all?.key;
    const c = counts?.[i];
    return {
      key: p.key,
      label: p.label,
      count: c,
      // "All" lights up only when nothing else is picked; it clears the set.
      selected: isAll ? values.length === 0 : selected(p),
      disabled: !isAll && c != null && c <= 0,
      onSelect: () => (isAll ? onClear() : onToggle({ lo: p.lo, hi: p.hi })),
    };
  });

  // One removable chip per active range, labelled by its matching preset (or
  // "Custom range" for a hand-picked window).
  const summary: FilterSummary[] = values.map((v, i) => {
    const p = presets.find((x) => rangeEq({ lo: x.lo, hi: x.hi }, v));
    return {
      key: `time:${i}`,
      label: p ? p.label : "Custom range",
      onClear: () => onToggle(v),
    };
  });

  return {
    key: "time",
    label: "Time",
    description,
    pills,
    // The custom picker APPENDS a range rather than replacing, so it composes
    // with the year chips; it resets to empty after adding.
    extra: (
      <DateRangeFilter
        value={{ lo: null, hi: null }}
        active={false}
        onChange={(r) => onToggle(r)}
      />
    ),
    summary,
  };
}

/** A pill button inside the panel (shared look with the rest of the app). */
export function FilterPillButton({ pill }: { pill: FilterPill }) {
  return (
    <button
      type="button"
      disabled={pill.disabled}
      onClick={pill.onSelect}
      className={filterPillClass(
        pill.selected,
        pill.disabled ? "opacity-40 pointer-events-none" : undefined,
      )}
    >
      {pill.icon}
      {pill.label}
      {pill.count != null && (
        <span className={filterPillCount}>{formatCount(pill.count)}</span>
      )}
    </button>
  );
}
