import { cn } from "@/lib/utils";

/**
 * The single visual language for every filter chip in the app — time-preset
 * chips, the custom-range trigger, and single-select badge filters (service,
 * source, Safari type, note lock, message content-kind). A subtle bordered pill
 * that tints toward `primary` when active (macOS-style selection, not a loud
 * solid fill). The `filter-groups` pills (`timeGroup`/`badgeGroup`), `BadgeFilter`,
 * and the `DateRangeFilter` trigger all route through this so their selected/hover
 * states can't drift apart.
 */
export function filterPillClass(active: boolean, extra?: string): string {
  return cn(
    "inline-flex shrink-0 cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 py-1 text-xs transition-colors",
    // Discrete borders — no louder than the island border (border-border/70).
    // Selected reads via a subtle fill + medium weight, not a bright accent.
    active
      ? "border-border bg-accent font-medium text-foreground"
      : "border-border/60 text-muted-foreground hover:bg-accent/60 hover:text-foreground",
    extra,
  );
}

/** The count suffix shown inside a filter pill (kept uniform across all chips). */
export const filterPillCount = "text-[10px] tabular-nums opacity-60";
