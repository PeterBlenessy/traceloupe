import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  type ReactNode,
} from "react";
import { useQueries } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";

/** Rows fetched per lazy window. Small keeps each IPC round-trip cheap. */
const PAGE = 100;

/**
 * A virtualized list over a very large, lazily-fetched dataset. Only the rows in
 * view (plus overscan) are mounted, and only the ~100-row windows they fall in
 * are fetched — and cached/evicted by React Query — so both DOM size and memory
 * stay bounded no matter how many rows exist.
 *
 * Two details here are load-bearing and easy to get wrong:
 *   - The scroll container uses `min-h-0`. Without it a flex child grows to its
 *     full content height instead of scrolling, the virtualizer sees the whole
 *     list as visible, and mounts every row — freezing the app.
 *   - The page set is memoized on the primitive first/last page, NOT on the
 *     fresh-every-render `getVirtualItems()` array. An unstable `useQueries`
 *     input makes its store emit a new snapshot each render — an infinite loop.
 *   - Only loaded rows are handed to `measureElement`; unloaded rows reserve
 *     `estimateSize` and are never measured. Measuring a short placeholder as a
 *     row's true height collapses the total size, then every row re-expands as
 *     its window resolves — the jump users feel as the list "sticking" while
 *     scrolling. See the render block below.
 */
export function LazyVirtualList<T>({
  count,
  windowKey,
  fetchWindow,
  renderItem,
  renderPlaceholder,
  estimateSize = 56,
  startAtBottom = false,
  resetKey,
  persistKey,
  jumpTo,
  scrollEnd,
  onTopIndexChange,
}: {
  /** Total number of rows (from a cheap COUNT query). */
  count: number;
  /** Stable React Query key for a page, e.g. ["messageWindow", threadId, page]. */
  windowKey: (page: number) => unknown[];
  /** Fetch `limit` rows starting at `offset`. */
  fetchWindow: (offset: number, limit: number) => Promise<T[]>;
  renderItem: (item: T, index: number, prev: T | undefined) => ReactNode;
  renderPlaceholder?: (index: number) => ReactNode;
  estimateSize?: number;
  /** Open scrolled to the newest (bottom) row. */
  startAtBottom?: boolean;
  /** Re-scroll to the bottom whenever this value changes (e.g. a thread id). */
  resetKey?: unknown;
  /** Persist & restore the scroll position under this key (localStorage). Also
   *  acts as the anchor key, so each distinct key remembers its own position. */
  persistKey?: string;
  /** Imperatively jump to a row: scroll `index` to the top when `token` changes. */
  jumpTo?: { index: number; token: number };
  /** Imperatively scroll to the very top/bottom when `token` changes. Uses a
   *  direct scrollTop (0 / scrollHeight), so it's reliable even on a tall
   *  variable-height list where scrollToIndex would thrash. */
  scrollEnd?: { dir: "top" | "bottom"; token: number };
  /** Reports the top-most visible row index as the user scrolls. */
  onTopIndexChange?: (index: number) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => estimateSize,
    overscan: 12,
  });
  const virtualItems = virtualizer.getVirtualItems();

  const firstPage = Math.floor((virtualItems[0]?.index ?? 0) / PAGE);
  const lastPage = Math.floor(
    (virtualItems[virtualItems.length - 1]?.index ?? 0) / PAGE,
  );
  const pages = useMemo(() => {
    // Include the page before the visible range so the row at a page boundary
    // can see its predecessor (used for date/time separators and group sender
    // labels); otherwise those render spuriously at the top of each page.
    const out: number[] = [];
    for (let p = Math.max(0, firstPage - 1); p <= lastPage; p++) out.push(p);
    return out;
  }, [firstPage, lastPage]);

  const queries = useQueries({
    queries: pages.map((p) => ({
      queryKey: windowKey(p),
      queryFn: () => fetchWindow(p * PAGE, PAGE),
    })),
  });
  const loaded = new Map<number, T[]>();
  pages.forEach((p, i) => {
    const data = queries[i].data as T[] | undefined;
    if (data) loaded.set(p, data);
  });
  const itemAt = (index: number): T | undefined =>
    loaded.get(Math.floor(index / PAGE))?.[index % PAGE];

  // One-shot initial scroll. We set scrollTop directly rather than via
  // virtualizer.scrollToIndex(): scrolling to a far index in a dynamically
  // measured list makes react-virtual retry across frames as measurements shift
  // the target, which never converges and freezes the app.
  //
  // With `persistKey`, a previously-saved scrollTop for that key is restored
  // instead of anchoring to top/bottom — so returning to a view (or restarting
  // the app) lands where you left off. `persistKey` also drives the anchor, so
  // changing it (a different filter/thread) re-anchors or restores that key's
  // own position.
  const anchorKey = persistKey ?? resetKey;
  const scrolledFor = useRef<unknown>(undefined);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (
      el &&
      count > 0 &&
      anchorKey !== undefined &&
      scrolledFor.current !== anchorKey
    ) {
      scrolledFor.current = anchorKey;
      const saved = persistKey
        ? Number(localStorage.getItem(`lvl:${persistKey}`))
        : NaN;
      if (Number.isFinite(saved) && saved > 0) {
        el.scrollTop = saved;
      } else {
        // Ascending (oldest-first) pins the newest row at the bottom; descending
        // (newest-first) pins the newest row at the top.
        el.scrollTop = startAtBottom ? el.scrollHeight : 0;
      }
    }
  }, [count, anchorKey, startAtBottom, persistKey]);

  // Persist the scroll position (debounced) so it can be restored above.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !persistKey) return;
    let t: ReturnType<typeof setTimeout>;
    const onScroll = () => {
      clearTimeout(t);
      t = setTimeout(() => {
        localStorage.setItem(`lvl:${persistKey}`, String(el.scrollTop));
      }, 200);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      clearTimeout(t);
    };
  }, [persistKey]);

  // Imperative jump: align `index` to the top. scrollToIndex accounts for
  // already-measured row heights (e.g. the timeline's occasional date headers),
  // so it lands on the target far more accurately than a flat index × estimate —
  // safe here because the rows that use this (the fixed-height timeline) don't
  // trigger the multi-frame re-measure that made scrollToIndex thrash on
  // variable-height lists.
  const jumpedFor = useRef<number | undefined>(undefined);
  useLayoutEffect(() => {
    if (jumpTo && scrollRef.current && jumpedFor.current !== jumpTo.token) {
      jumpedFor.current = jumpTo.token;
      virtualizer.scrollToIndex(jumpTo.index, { align: "start" });
    }
  }, [jumpTo, virtualizer]);

  // Imperative scroll to the very top/bottom via a direct scrollTop — reliable on
  // a tall variable-height list (unlike scrollToIndex to the far end).
  const scrolledEndFor = useRef<number | undefined>(undefined);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (scrollEnd && el && scrolledEndFor.current !== scrollEnd.token) {
      scrolledEndFor.current = scrollEnd.token;
      el.scrollTop = scrollEnd.dir === "bottom" ? el.scrollHeight : 0;
    }
  }, [scrollEnd]);

  // Report the top visible row so a caller can highlight where we are.
  const topIndex = virtualItems[0]?.index ?? 0;
  const onTopRef = useRef(onTopIndexChange);
  onTopRef.current = onTopIndexChange;
  useEffect(() => {
    onTopRef.current?.(topIndex);
  }, [topIndex]);

  return (
    // `overflow-anchor: none` stops the browser's own scroll-anchoring from
    // fighting the virtualizer when row heights settle after a window loads.
    <div
      ref={scrollRef}
      className="min-h-0 flex-1 overflow-auto [overflow-anchor:none]"
    >
      <div
        className="relative w-full"
        style={{ height: virtualizer.getTotalSize() }}
      >
        {virtualItems.map((vi) => {
          const item = itemAt(vi.index);
          // Only rows with real content are measured. A not-yet-loaded row
          // reserves exactly `estimateSize` and is NOT handed to
          // `measureElement`: otherwise its short skeleton would be measured as
          // the row's true height, react-virtual would collapse the total size,
          // then re-expand each row as its window resolves — that collapse/
          // re-expand under the scroll position is the "stuck then un-stuck"
          // jump. Reserving the estimate keeps scrolling smooth over unloaded
          // regions; the real height is measured once content arrives.
          if (!item) {
            return (
              <div
                key={vi.key}
                className="absolute left-0 top-0 w-full"
                style={{
                  transform: `translateY(${vi.start}px)`,
                  height: estimateSize,
                }}
              >
                {renderPlaceholder?.(vi.index) ?? (
                  <div className="px-4 pb-1">
                    <div className="h-9 animate-pulse rounded-2xl bg-muted/40" />
                  </div>
                )}
              </div>
            );
          }
          return (
            <div
              key={vi.key}
              data-index={vi.index}
              ref={virtualizer.measureElement}
              className="absolute left-0 top-0 w-full"
              style={{ transform: `translateY(${vi.start}px)` }}
            >
              {renderItem(item, vi.index, itemAt(vi.index - 1))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
