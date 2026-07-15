import { Badge } from "@/components/ui/badge";
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
 * The row never wraps — it scrolls horizontally when the panel is too narrow
 * (`min-w-0` + `overflow-x-auto`, scrollbar hidden), so a long filter list can't
 * push the header onto a second line.
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
  return (
    <div
      className={cn(
        "flex min-w-0 flex-nowrap items-center gap-1.5 overflow-x-auto [&::-webkit-scrollbar]:hidden",
        className,
      )}
    >
      {options.map((o) => {
        const active = value === o.value;
        return (
          <Badge
            key={o.value}
            asChild
            variant={active ? "default" : "secondary"}
            className="shrink-0 cursor-pointer gap-1 transition-colors hover:opacity-90"
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
        );
      })}
    </div>
  );
}
