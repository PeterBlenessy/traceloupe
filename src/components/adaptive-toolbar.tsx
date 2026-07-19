import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
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
const MORE_W = 30; // the expand chevron
const ICON_W = 38; // a collapsed island icon button (+ its gap)
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
  middle,
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
  /** When provided, replaces the whole island engine with this right-aligned
   *  cluster (the Filter/Sort/Search paradigm). `islands`/`search` are ignored. */
  middle?: ReactNode;
  className?: string;
}) {
  // The Filter/Sort/Search cluster bypasses the island fit entirely: it's a
  // small, stable set of controls, right-aligned before the app controls.
  if (middle) {
    return (
      // `data-tauri-drag-region` on the containers makes their empty areas drag
      // the window (the merged titlebar has no native drag bar). Interactive
      // children aren't drag regions, so buttons still click normally.
      <div
        data-tauri-drag-region
        className={cn(
          "relative flex min-w-0 flex-1 items-center gap-2",
          className,
        )}
      >
        {leading && (
          <div data-tauri-drag-region className="flex shrink-0 items-center gap-2">
            {leading}
          </div>
        )}
        <div
          data-tauri-drag-region
          className="flex min-w-0 flex-1 items-center justify-end gap-2"
        >
          {middle}
        </div>
        {trailing && (
          <div className="flex shrink-0 items-center gap-2">{trailing}</div>
        )}
      </div>
    );
  }
  const areaRef = useRef<HTMLDivElement>(null);
  const middleRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLDivElement>(null);
  const measRef = useRef<Map<string, HTMLSpanElement | null>>(new Map());
  const [expanded, setExpanded] = useState<string | null>(null);
  const [layouts, setLayouts] = useState<Record<string, IslandLayout>>({});

  const compute = useCallback(() => {
    const middle = middleRef.current;
    if (!middle) return;
    // The middle is a flex-1 container between the fixed leading (title) and the
    // fixed trailing (app controls); its width is exactly the room for the view
    // islands + the search that lives inside it. Measuring it directly means the
    // fit can never overflow the app controls off-screen.
    const searchW = searchRef.current?.offsetWidth ?? 0;
    const W = middle.clientWidth - searchW;
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

    // W already excludes the fixed leading/trailing (which contains the search).
    const N = islands.length;
    const avail = Math.max(0, W - ISLAND_GAP * (N + 1));

    // Width of island k showing `count` items (with the expand chevron if some
    // items are still hidden).
    const widthAt = (k: string, count: number) => {
      const items = w[k];
      let s = PAD;
      for (let i = 0; i < count; i++) s += items[i] + (i > 0 ? GAP : 0);
      if (count < items.length) s += GAP + MORE_W;
      return s;
    };
    const minCount = (k: string) => Math.min(2, w[k].length);

    const next: Record<string, IslandLayout> = {};
    if (!expanded || !islands.some((i) => i.key === expanded)) {
      // Start every island at its 2-item minimum.
      const counts: Record<string, number> = {};
      let total = 0;
      for (const isl of islands) {
        counts[isl.key] = minCount(isl.key);
        total += widthAt(isl.key, counts[isl.key]);
      }
      if (total <= avail) {
        // Everyone fits their minimum — greedily reveal more items into the slack
        // (round-robin so no single island hogs the extra room).
        let changed = true;
        while (changed) {
          changed = false;
          for (const isl of islands) {
            if (counts[isl.key] < isl.items.length) {
              const delta =
                widthAt(isl.key, counts[isl.key] + 1) - widthAt(isl.key, counts[isl.key]);
              if (total + delta <= avail) {
                total += delta;
                counts[isl.key] += 1;
                changed = true;
              }
            }
          }
        }
        for (const isl of islands)
          next[isl.key] = {
            mode: "normal",
            count: counts[isl.key],
            more: counts[isl.key] < isl.items.length,
          };
      } else {
        // Can't fit every minimum — collapse the widest islands to icons (one
        // click from their contents) until the rest fit, so nothing overflows.
        const icon = new Set<string>();
        const order = [...islands].sort(
          (a, b) => widthAt(b.key, minCount(b.key)) - widthAt(a.key, minCount(a.key)),
        );
        const measure = () =>
          islands.reduce(
            (t, isl) => t + (icon.has(isl.key) ? ICON_W : widthAt(isl.key, minCount(isl.key))),
            0,
          );
        for (let i = 0; measure() > avail && i < order.length; i++) icon.add(order[i].key);
        for (const isl of islands) {
          if (icon.has(isl.key)) {
            next[isl.key] = { mode: "icon", count: 0, more: false };
          } else {
            const c = minCount(isl.key);
            next[isl.key] = { mode: "normal", count: c, more: c < isl.items.length };
          }
        }
      }
    } else {
      // One island expanded: it takes its full content; the rest keep their 2-item
      // minimum where it fits, else collapse to a single icon.
      const K = expanded;
      next[K] = { mode: "expanded", count: w[K].length, more: true };
      const wK = islandContentAll(K) + PAD;
      let rem = avail - wK;
      const others = islands.filter((i) => i.key !== K);
      for (const isl of others) {
        const c = minCount(isl.key);
        const wMin = widthAt(isl.key, c);
        if (isl.items.length >= 2 && rem >= wMin) {
          rem -= wMin;
          next[isl.key] = { mode: "normal", count: c, more: c < isl.items.length };
        } else {
          rem -= ICON_W;
          next[isl.key] = { mode: "icon", count: 0, more: false };
        }
      }
    }
    setLayouts(next);
  }, [islands, expanded, searchExpanded, search]);

  useLayoutEffect(() => {
    compute();
    const ro = new ResizeObserver(compute);
    // The middle container resizes as the leading/trailing (and window) change;
    // the search resizes when it expands. Both drive the island fit.
    if (middleRef.current) ro.observe(middleRef.current);
    if (searchRef.current) ro.observe(searchRef.current);
    return () => ro.disconnect();
  }, [compute]);

  return (
    <div
      ref={areaRef}
      data-tauri-drag-region
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
        <div data-tauri-drag-region className="flex shrink-0 items-center gap-2">
          {leading}
        </div>
      )}

      {/* Middle: view islands + search, right-aligned (flex-1 pushes them over to
          the trailing app controls). Clips here if the fit ever runs long, so the
          app controls (a separate sibling) are never pushed off-screen. */}
      <div
        ref={middleRef}
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center justify-end gap-2 overflow-hidden"
      >
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

        // Islands size to their content so the arrow sits right after the last
        // visible item. The even distribution is in the item-count fit (how many
        // items each island reveals for its equal share), not in stretching.
        const visible = isl.items.slice(0, L.count);
        return (
          <div
            key={isl.key}
            className="flex min-w-0 shrink-0 items-center gap-0.5 rounded-lg border border-border/70 bg-muted/40 p-0.5"
          >
            <div className="flex min-w-0 items-center gap-0.5 overflow-hidden">
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
                  <ChevronLeft className="size-4" />
                ) : (
                  <ChevronRight className="size-4" />
                )}
              </button>
            )}
          </div>
        );
        })}

        {/* Search belongs to the view — right after the islands, inside the
            clipped middle, before the app controls. */}
        {search && (
          <div ref={searchRef} className="flex shrink-0 items-center">
            {search}
          </div>
        )}
      </div>

      {/* App-wide controls are the rightmost actions and never clip. */}
      {trailing && (
        <div className="flex shrink-0 items-center gap-2">{trailing}</div>
      )}
    </div>
  );
}
