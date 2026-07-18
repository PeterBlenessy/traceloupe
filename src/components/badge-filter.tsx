import { useMemo } from "react";
import { OverflowRow, type OverflowItem } from "@/components/overflow-row";
import { filterPillClass, filterPillCount } from "@/components/filter-pill";
import { formatCount } from "@/lib/format";

export interface BadgeFilterOption {
  value: string;
  label: string;
  /** Optional leading icon (e.g. a brand mark). */
  icon?: React.ReactNode;
  /** Optional trailing count. */
  count?: number;
}

/**
 * A single-select filter rendered as clickable pills sharing the app-wide filter
 * chip language (see {@link filterPillClass}): the selected option tints toward
 * `primary`, the rest are muted outlines. Used for every list filter (service,
 * source, type, content…) so they match the time-preset chips exactly.
 *
 * The row never wraps — when it's too narrow it keeps as many badges inline as
 * fit and moves the rest into a "⋮" overflow menu (see {@link OverflowRow}), so a
 * long filter list can't push the header onto a second line or hide options in a
 * horizontal scroll.
 */
export function BadgeFilter({
  options,
  value,
  onChange,
  className,
}: {
  options: BadgeFilterOption[];
  value: string;
  onChange: (v: string) => void;
  className?: string;
}) {
  const items = useMemo<OverflowItem[]>(
    () =>
      options.map((o) => {
        const active = value === o.value;
        return {
          key: o.value,
          active,
          render: (inMenu: boolean) => (
            <button
              type="button"
              onClick={() => onChange(o.value)}
              className={filterPillClass(active, inMenu ? "w-full justify-start" : undefined)}
            >
              {o.icon}
              {o.label}
              {o.count != null && (
                <span className={filterPillCount}>{formatCount(o.count)}</span>
              )}
            </button>
          ),
        };
      }),
    [options, value, onChange],
  );
  return <OverflowRow items={items} className={className} />;
}
