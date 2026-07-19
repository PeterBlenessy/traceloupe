import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Funnel, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { filterPillClass } from "@/components/filter-pill";
import {
  FilterPillButton,
  type FilterGroup,
  type FilterSummary,
} from "@/components/filter-groups";

/** The open panel's width (matches the inner content's fixed width, 26rem). */
const PANEL_W = 416;

/**
 * The **Filter** control: one affordance that keeps the toolbar calm.
 *
 * - **Closed, nothing applied** — a lone funnel button.
 * - **Closed, filters applied** — the funnel + the active selections as removable
 *   chips, in one bordered island.
 * - **Open** — a portalled box **morphs out of the funnel button**: it animates
 *   its `width`/`height`/`border-radius` from the button's footprint to the full
 *   panel (the NoteSage command-bar technique). Width/height animate reliably in
 *   WebKit — unlike the `scale`/`translate` properties, which this app's webview
 *   won't animate. `overflow-hidden` clips the content as the box grows; the
 *   content fades in. Reverses on close. Escape / outside-click / the funnel all
 *   close it.
 *
 * Only groups the view actually has are passed in, so an absent facet never
 * appears as an empty row.
 */
export function FilterControl({ groups }: { groups: FilterGroup[] }) {
  const [mounted, setMounted] = useState(false); // overlay is in the tree
  const [expanded, setExpanded] = useState(false); // morph target (full panel)
  const [rect, setRect] = useState({ top: 0, right: 0, w: 32, h: 32 });
  const [contentH, setContentH] = useState(0);
  // Chips mid-removal: kept in the DOM while they collapse (width→0) before the
  // filter is actually cleared, so the island shrinks smoothly.
  const [removing, setRemoving] = useState<Set<string>>(() => new Set());
  const btnRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef(0);
  // Every pending timer, cleared on unmount so late callbacks can't fire.
  const timers = useRef<Set<number>>(new Set());
  const summaries: FilterSummary[] = groups.flatMap((g) => g.summary);
  const hasActive = summaries.length > 0;

  const later = (fn: () => void, ms: number) => {
    const id = window.setTimeout(() => {
      timers.current.delete(id);
      fn();
    }, ms);
    timers.current.add(id);
    return id;
  };

  // Collapse a chip, then clear its filter once the shrink animation is done.
  // Guard re-entry: a second click on the same X within the window would fire a
  // toggle-based clear twice (multi-select), toggling the value back on.
  const removeChip = (s: FilterSummary) => {
    if (removing.has(s.key)) return;
    setRemoving((prev) => new Set(prev).add(s.key));
    later(() => {
      s.onClear();
      setRemoving((prev) => {
        const next = new Set(prev);
        next.delete(s.key);
        return next;
      });
    }, 200);
  };

  // Anchor the box's top-right to the button's top-right (viewport coords), so
  // growing width pushes its left edge out and growing height drops its bottom.
  const measureButton = () => {
    const r = btnRef.current?.getBoundingClientRect();
    if (r) setRect({ top: r.top, right: window.innerWidth - r.right, w: r.width, h: r.height });
  };

  const open = () => {
    window.clearTimeout(closeTimer.current);
    timers.current.delete(closeTimer.current);
    measureButton();
    if (mounted) setExpanded(true); // re-open mid-close: grow straight back
    else setMounted(true); // first open: the effect below kicks the grow
  };
  const close = () => {
    window.clearTimeout(closeTimer.current);
    setExpanded(false);
    closeTimer.current = later(() => setMounted(false), 240);
  };

  // On mount, measure the content's natural height, then (after the browser has
  // painted the compact box) flip to expanded so the size transition plays.
  useLayoutEffect(() => {
    if (!mounted) return;
    setContentH(contentRef.current?.scrollHeight ?? 0);
    let r2 = 0;
    const r1 = requestAnimationFrame(() => {
      r2 = requestAnimationFrame(() => setExpanded(true));
    });
    return () => {
      cancelAnimationFrame(r1);
      cancelAnimationFrame(r2);
    };
  }, [mounted]);

  // Keep the box height matched to the content as it changes (e.g. a group's
  // pills reflow), so nothing gets clipped while the panel is open.
  useEffect(() => {
    if (!mounted || !contentRef.current) return;
    const el = contentRef.current;
    const ro = new ResizeObserver(() => setContentH(el.scrollHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, [mounted]);

  // Move focus into the panel on open; restore it to the funnel on close.
  useEffect(() => {
    if (expanded) panelRef.current?.focus();
    else if (mounted) btnRef.current?.focus();
  }, [expanded, mounted]);

  // Escape / outside-click close; keep aligned if the window resizes.
  useEffect(() => {
    if (!mounted) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (panelRef.current?.contains(t) || btnRef.current?.contains(t)) return;
      close();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("resize", measureButton);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("resize", measureButton);
    };
  }, [mounted]);

  // Cancel any pending timers if we unmount mid-animation.
  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach((id) => window.clearTimeout(id));
  }, []);

  return (
    <div className="relative flex shrink-0 items-center">
      {/* Closed representation: a lone funnel button, or the funnel + active chips
          wrapped in a bordered island. */}
      <div
        className={cn(
          "flex items-center",
          hasActive && "rounded-lg border border-border/70 bg-muted/40 p-0.5",
        )}
      >
        <button
          ref={btnRef}
          type="button"
          aria-label="Filter"
          aria-haspopup="dialog"
          aria-expanded={expanded}
          title="Filter"
          onClick={() => (mounted ? close() : open())}
          data-active={mounted || hasActive}
          className={cn(
            "inline-flex shrink-0 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[active=true]:text-foreground",
            hasActive
              ? "size-7 rounded-md"
              : "size-8 rounded-lg border border-border/70 bg-muted/40 data-[active=true]:bg-accent",
          )}
        >
          <Funnel className="size-4" />
        </button>
        {summaries.map((s) => (
          // Collapsing wrapper: max-width→0 shrinks the chip (and its leading gap)
          // when it's being removed, so the island narrows smoothly.
          <span
            key={s.key}
            className={cn(
              "flex items-center overflow-hidden transition-all duration-200 ease-out",
              removing.has(s.key) ? "max-w-0 opacity-0" : "max-w-[16rem] opacity-100",
            )}
          >
            <span
              className={filterPillClass(true, "ml-1 cursor-pointer py-0.5 pr-1")}
              onClick={() => open()}
            >
              {s.icon}
              {s.label}
              <button
                type="button"
                aria-label={`Clear ${s.label}`}
                onClick={(e) => {
                  e.stopPropagation();
                  removeChip(s);
                }}
                className="ml-0.5 inline-flex size-4 items-center justify-center rounded-full text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
              >
                <X className="size-3" />
              </button>
            </span>
          </span>
        ))}
      </div>

      {mounted &&
        createPortal(
          <>
            {/* Scrim: dims the content while open. Purely visual — outside
                clicks are handled by the window pointerdown listener, so it stays
                pointer-events-none (letting clicks reach the controls during the
                close animation, e.g. to re-open). */}
            <div
              className={cn(
                "pointer-events-none fixed inset-0 z-[60] bg-black/30 transition-opacity duration-200 ease-out",
                expanded ? "opacity-100" : "opacity-0",
              )}
            />
            {/* The morphing box. width/height/radius animate from the button's
                footprint to the full panel via transition-all. */}
            <div
              ref={panelRef}
              role="dialog"
              aria-label="Filters"
              tabIndex={-1}
              style={{
                top: rect.top,
                right: rect.right,
                width: expanded ? PANEL_W : rect.w,
                height: expanded ? contentH : rect.h,
              }}
              className={cn(
                "fixed z-[61] overflow-hidden border bg-popover text-popover-foreground shadow-lg outline-none transition-all duration-200 ease-out",
                expanded ? "rounded-xl" : "rounded-lg",
              )}
            >
              {/* Fixed-width content (so its height is stable to measure); fades in
                  as the box grows. */}
              <div
                ref={contentRef}
                className={cn(
                  "w-[26rem] p-3 transition-opacity duration-150",
                  expanded ? "opacity-100 delay-75" : "opacity-0",
                )}
              >
                <div className="mb-2 flex items-center justify-between px-1">
                  <span className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                    <Funnel className="size-3.5" />
                    Filters
                  </span>
                  {hasActive && (
                    <button
                      type="button"
                      // Skip chips already mid-removal — their pending timer will
                      // clear them (double-firing a toggle-based clear re-adds it).
                      onClick={() =>
                        summaries.forEach((s) => !removing.has(s.key) && s.onClear())
                      }
                      className="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                    >
                      Clear all
                    </button>
                  )}
                </div>
                <div className="flex flex-col gap-3">
                  {groups.map((g) => (
                    <div key={g.key} className="rounded-lg px-1">
                      <div className="mb-1.5">
                        <div className="text-sm font-medium">{g.label}</div>
                        <div className="text-xs text-muted-foreground">
                          {g.description}
                        </div>
                      </div>
                      <div className="flex flex-wrap items-center gap-1.5">
                        {g.pills.map((p) => (
                          <FilterPillButton key={p.key} pill={p} />
                        ))}
                        {g.extra}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          </>,
          document.body,
        )}
    </div>
  );
}
