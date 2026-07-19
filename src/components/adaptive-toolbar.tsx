import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ChevronsLeft, ChevronsRight } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Adaptive "islands" toolbar.
 *
 * Related actions are grouped into segmented, bordered **islands**. Each island
 * shows as many of its items as fit and tucks the rest behind a trailing "⋮";
 * the available width is shared **evenly** across islands so none dominates.
 * Clicking an island's "⋮" **expands** it (reveals all its items) and collapses
 * the others — to their 2-item minimum if there's room, otherwise to a single
 * representative icon. A `search`-style singular control sits at the end with its
 * own collapse/expand.
 *
 * Items are ordered by importance (most-relevant first) by the caller, so when
 * space is tight the trailing/low-value items are the ones that fold away (e.g.
 * empty time-range presets).
 */
export interface ToolbarItem {
  key: string;
  /** The rendered control (a chip/button). Kept interactive in place. */
  node: ReactNode;
  /** Marks this island as "active" (drives the ⋮ trigger's active styling). */
  active?: boolean;
}

export interface ToolbarIsland {
  key: string;
  /** Human label (a11y + the collapsed-icon tooltip). */
  label: string;
  /** Representative icon shown when the island collapses to a single button. */
  icon: ReactNode;
  /** Items, most-important first. */
  items: ToolbarItem[];
  /** True if any item in the island is in a non-default (active) state. */
  active?: boolean;
}

// Layout constants (px). Approximate; the fit math only needs to be close.
const GAP = 2; // between items within an island
const ISLAND_GAP = 8; // between islands / the search
const MORE_W = 30; // the "⋮" trigger
const PAD = 8; // an island's border + inner padding (both edges)

type Mode = "normal" | "expanded" | "icon";
interface IslandLayout {
  mode: Mode;
  count: number; // visible item count (normal/expanded)
  more: boolean; // show the ⋮ trigger
}

export function AdaptiveToolbar({
  leading,
  islands,
  trailing,
  search,
  searchExpanded,
  className,
}: {
  /** Fixed content pinned to the left (e.g. the view title + count). */
  leading?: ReactNode;
  /** The even-distributed view islands (fill the middle). */
  islands: ToolbarIsland[];
  /** Fixed content pinned to the right (e.g. app-wide controls). */
  trailing?: ReactNode;
  /** A singular trailing control (e.g. expanding search), after `trailing`. */
  search?: ReactNode;
  /** Whether `search` is currently in its expanded (wide) state, so islands
   *  reserve room for it. */
  searchExpanded?: boolean;
  className?: string;
}) {
  const areaRef = useRef<HTMLDivElement>(null);
  const leadingRef = useRef<HTMLDivElement>(null);
  const trailingRef = useRef<HTMLDivElement>(null);
  const measRef = useRef<Map<string, HTMLSpanElement | null>>(new Map());
  const [expanded, setExpanded] = useState<string | null>(null);
  const [layouts, setLayouts] = useState<Record<string, IslandLayout>>({});

  const compute = useCallback(() => {
    const area = areaRef.current;
    if (!area) return;
    const leadW = leadingRef.current?.offsetWidth ?? 0;
    const trailW = trailingRef.current?.offsetWidth ?? 0;
    const W = area.clientWidth - leadW - trailW;
    if (W <= 0) return;

    // Per-item measured widths.
    const w: Record<string, number[]> = {};
    for (const isl of islands) {
      w[isl.key] = isl.items.map(
        (it) => measRef.current.get(`${isl.key}:${it.key}`)?.offsetWidth ?? 0,
      );
    }
    const islandContentAll = (k: string) =>
      w[k].reduce((a, x, i) => a + x + (i > 0 ? GAP : 0), 0);
    const twoItemW = (k: string) =>
      (w[k][0] ?? 0) + GAP + (w[k][1] ?? 0) + GAP + MORE_W + PAD;

    // W already excludes the fixed leading/trailing (which contains the search).
    const N = islands.length;
    const avail = Math.max(0, W - ISLAND_GAP * (N + 1));

    // Fit as many items as possible into `budget`, reserving the ⋮ if some fold.
    const fit = (k: string, budget: number, min: number): IslandLayout => {
      const items = w[k];
      if (islandContentAll(k) + PAD <= budget) {
        return { mode: "normal", count: items.length, more: false };
      }
      let used = PAD;
      let count = 0;
      for (let i = 0; i < items.length; i++) {
        const add = items[i] + (count > 0 ? GAP : 0);
        if (used + add + GAP + MORE_W <= budget) {
          used += add;
          count += 1;
        } else break;
      }
      count = Math.min(items.length, Math.max(count, min));
      return { mode: "normal", count, more: count < items.length };
    };

    const next: Record<string, IslandLayout> = {};
    if (!expanded || !islands.some((i) => i.key === expanded)) {
      const share = avail / Math.max(N, 1);
      for (const isl of islands) {
        const min = Math.min(2, isl.items.length);
        next[isl.key] = fit(isl.key, share, min);
      }
    } else {
      // One island expanded: it takes its full content; the rest share what's
      // left, dropping to a single icon when they can't keep 2 items.
      const K = expanded;
      next[K] = { mode: "expanded", count: w[K].length, more: true };
      const wK = islandContentAll(K) + PAD;
      const others = islands.filter((i) => i.key !== K);
      const shareO = others.length ? (avail - wK) / others.length : 0;
      for (const isl of others) {
        const min = Math.min(2, isl.items.length);
        if (shareO >= twoItemW(isl.key) && isl.items.length >= 2) {
          next[isl.key] = fit(isl.key, shareO, min);
        } else {
          next[isl.key] = { mode: "icon", count: 0, more: false };
        }
      }
    }
    setLayouts(next);
  }, [islands, expanded, searchExpanded, search]);

  useLayoutEffect(() => {
    compute();
    const ro = new ResizeObserver(compute);
    if (areaRef.current) ro.observe(areaRef.current);
    // Observe the fixed ends too: an expanding search (in `trailing`) changes the
    // space left for islands without resizing the toolbar itself.
    if (leadingRef.current) ro.observe(leadingRef.current);
    if (trailingRef.current) ro.observe(trailingRef.current);
    return () => ro.disconnect();
  }, [compute]);

  return (
    <div
      ref={areaRef}
      className={cn("relative flex min-w-0 flex-1 items-center gap-2", className)}
    >
      {/* Hidden measurement layer — natural item widths. */}
      <div
        aria-hidden
        className="pointer-events-none invisible absolute left-0 top-0 flex items-center"
        style={{ gap: GAP }}
      >
        {islands.map((isl) =>
          isl.items.map((it) => (
            <span
              key={`${isl.key}:${it.key}`}
              ref={(el) => {
                measRef.current.set(`${isl.key}:${it.key}`, el);
              }}
            >
              {it.node}
            </span>
          )),
        )}
      </div>

      {leading && (
        <div ref={leadingRef} className="flex shrink-0 items-center gap-2">
          {leading}
        </div>
      )}

      {islands.map((isl) => {
        const L = layouts[isl.key] ?? {
          mode: "normal" as Mode,
          count: Math.min(2, isl.items.length),
          more: isl.items.length > 2,
        };
        const isExpanded = expanded === isl.key;

        if (L.mode === "icon") {
          return (
            <button
              key={isl.key}
              type="button"
              aria-label={isl.label}
              title={isl.label}
              onClick={() => setExpanded(isl.key)}
              data-active={isl.active}
              className="inline-flex size-8 shrink-0 items-center justify-center rounded-lg border border-border/70 bg-muted/40 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[active=true]:text-foreground"
            >
              {isl.icon}
            </button>
          );
        }

        // Expanded island sizes to content; normal islands share width evenly.
        const visible = isl.items.slice(0, L.count);
        return (
          <div
            key={isl.key}
            className={cn(
              "flex min-w-0 items-center gap-0.5 rounded-lg border border-border/70 bg-muted/40 p-0.5",
              isExpanded ? "shrink-0" : "flex-1",
            )}
          >
            <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden">
              {visible.map((it) => (
                <span key={it.key} className="shrink-0">
                  {it.node}
                </span>
              ))}
            </div>
            {(L.more || isExpanded) && (
              <button
                type="button"
                aria-label={`${isl.label} options`}
                title={isExpanded ? `Collapse ${isl.label}` : `More ${isl.label}`}
                onClick={() => setExpanded(isExpanded ? null : isl.key)}
                data-active={isExpanded || isl.active}
                className="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[active=true]:text-foreground"
              >
                {isExpanded ? (
                  <ChevronsLeft className="size-4" />
                ) : (
                  <ChevronsRight className="size-4" />
                )}
              </button>
            )}
          </div>
        );
      })}

      {(trailing || search) && (
        <div ref={trailingRef} className="ml-auto flex shrink-0 items-center gap-2">
          {trailing}
          {search}
        </div>
      )}
    </div>
  );
}
