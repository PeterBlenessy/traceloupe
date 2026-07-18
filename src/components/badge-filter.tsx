import { useMemo } from "react";
import { Badge } from "@/components/ui/badge";
import { OverflowRow, type OverflowItem } from "@/components/overflow-row";
import { formatCount } from "@/lib/format";
import { cn } from "@/lib/utils";

export interface BadgeFilterOption {
  value: string;
  label: string;
  /** Optional leading icon (e.g. a brand mark). */
  icon?: React.ReactNode;
  /** Optional trailing count. */
  count?: number;
}

/**
 * A single-select filter rendered as clickable badges (the same `Badge` pill used
 * for the Apps "Native"/"Coming soon" tags): the selected option is filled, the
 * rest are muted. Used for every list filter (service, source, type, content…) so
 * they all look the same.
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
            <Badge
              asChild
              variant={active ? "default" : "secondary"}
              className={cn(
                "shrink-0 cursor-pointer gap-1 transition-colors hover:opacity-90",
                inMenu && "w-full justify-start",
              )}
            >
              <button type="button" onClick={() => onChange(o.value)}>
                {o.icon}
                {o.label}
                {o.count != null && (
                  <span
                    className={cn(
                      "tabular-nums",
                      active ? "opacity-70" : "text-muted-foreground/70",
                    )}
                  >
                    {formatCount(o.count)}
                  </span>
                )}
              </button>
            </Badge>
          ),
        };
      }),
    [options, value, onChange],
  );
  return <OverflowRow items={items} className={className} />;
}
