import { Fragment, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { MoreVertical } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** One item in an {@link OverflowRow}. `render` is called for the inline row and
 *  the measurement copy with `inMenu=false`, and for the overflow menu with
 *  `inMenu=true` (so an item can go full-width in the menu). */
export interface OverflowItem {
  key: string;
  /** Marks the "⋮" trigger active when this item overflowed off-screen. */
  active?: boolean;
  render: (inMenu: boolean) => ReactNode;
}

/**
 * A horizontal row of items that keeps as many inline as fit and tucks the rest
 * behind a vertical-ellipsis ("⋮") menu, instead of scrolling. Natural item
 * widths are measured from a hidden copy of the full row (so overflowed items
 * stay measurable) and the split is recomputed on resize.
 *
 * Pass a **memoized** `items` array (the measurement effect re-runs when it
 * changes). The row shrinks with its flex parent by default; add `flex-1` via
 * `className` if it should also grow to fill.
 */
export function OverflowRow({
  items,
  gapPx = 6,
  className,
  menuClassName,
  title = "More",
}: {
  items: OverflowItem[];
  /** Horizontal gap between items, in px (kept in sync between layout & measure). */
  gapPx?: number;
  className?: string;
  menuClassName?: string;
  title?: string;
}) {
  const areaRef = useRef<HTMLDivElement>(null);
  const measureRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const [visible, setVisible] = useState(items.length);

  useLayoutEffect(() => {
    const area = areaRef.current;
    if (!area) return;
    const MORE = 34; // reserved width for the "⋮" trigger + its gap
    const compute = () => {
      const avail = area.clientWidth;
      const widths = items.map((_, i) => measureRefs.current[i]?.offsetWidth ?? 0);
      const total = widths.reduce((a, w, i) => a + w + (i > 0 ? gapPx : 0), 0);
      if (total <= avail) {
        setVisible(items.length);
        return;
      }
      let used = 0;
      let count = 0;
      for (let i = 0; i < widths.length; i++) {
        const add = widths[i] + (count > 0 ? gapPx : 0);
        if (used + add + gapPx + MORE <= avail) {
          used += add;
          count += 1;
        } else break;
      }
      setVisible(count);
    };
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(area);
    return () => ro.disconnect();
  }, [items, gapPx]);

  const hidden = items.slice(visible);
  const hiddenActive = hidden.some((it) => it.active);

  return (
    <div ref={areaRef} className={cn("relative min-w-0", className)}>
      {/* Hidden full row, only for measuring natural item widths. */}
      <div
        aria-hidden
        className="pointer-events-none invisible absolute left-0 top-0 flex flex-nowrap items-center"
        style={{ gap: gapPx }}
      >
        {items.map((it, i) => (
          <span
            key={it.key}
            ref={(el) => {
              measureRefs.current[i] = el;
            }}
          >
            {it.render(false)}
          </span>
        ))}
      </div>
      {/* Visible row: the items that fit, then a "⋮" menu for the rest. */}
      <div
        className="flex flex-nowrap items-center overflow-hidden"
        style={{ gap: gapPx }}
      >
        {items.slice(0, visible).map((it) => (
          <Fragment key={it.key}>{it.render(false)}</Fragment>
        ))}
        {hidden.length > 0 && (
          <Popover>
            <Tooltip>
              <TooltipTrigger asChild>
                <PopoverTrigger asChild>
                  <button
                    type="button"
                    data-active={hiddenActive}
                    aria-label={title}
                    className="inline-flex size-7 shrink-0 items-center justify-center rounded-full border text-muted-foreground hover:bg-accent data-[active=true]:border-primary data-[active=true]:bg-primary/10 data-[active=true]:text-foreground"
                  >
                    <MoreVertical className="size-4" />
                  </button>
                </PopoverTrigger>
              </TooltipTrigger>
              <TooltipContent>{title}</TooltipContent>
            </Tooltip>
            <PopoverContent
              align="end"
              className={cn(
                "grid max-h-72 w-44 gap-1 overflow-y-auto",
                menuClassName,
              )}
            >
              {hidden.map((it) => (
                <Fragment key={it.key}>{it.render(true)}</Fragment>
              ))}
            </PopoverContent>
          </Popover>
        )}
      </div>
    </div>
  );
}
