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
  jumpTo,
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
  /** Imperatively jump to a row: scroll `index` to the top when `token` changes. */
  jumpTo?: { index: number; token: number };
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

  // One-shot scroll to the bottom. We set scrollTop directly rather than via
  // virtualizer.scrollToIndex(): scrolling to a far index in a dynamically
  // measured list makes react-virtual retry across frames as measurements shift
  // the target, which never converges and freezes the app.
  const scrolledFor = useRef<unknown>(undefined);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (startAtBottom && el && count > 0 && scrolledFor.current !== resetKey) {
      scrolledFor.current = resetKey;
      el.scrollTop = el.scrollHeight;
    }
  }, [count, resetKey, startAtBottom]);

  // Imperative jump: align `index` to the top. We set scrollTop directly (index
  // × estimate) rather than virtualizer.scrollToIndex() to avoid its multi-frame
  // retry on large dynamic lists — approximate is fine for a jump shortcut.
  const jumpedFor = useRef<number | undefined>(undefined);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (jumpTo && el && jumpedFor.current !== jumpTo.token) {
      jumpedFor.current = jumpTo.token;
      el.scrollTop = jumpTo.index * estimateSize;
    }
  }, [jumpTo, estimateSize]);

  // Report the top visible row so a caller can highlight where we are.
  const topIndex = virtualItems[0]?.index ?? 0;
  const onTopRef = useRef(onTopIndexChange);
  onTopRef.current = onTopIndexChange;
  useEffect(() => {
    onTopRef.current?.(topIndex);
  }, [topIndex]);

  return (
    <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto">
      <div
        className="relative w-full"
        style={{ height: virtualizer.getTotalSize() }}
      >
        {virtualItems.map((vi) => {
          const item = itemAt(vi.index);
          return (
            <div
              key={vi.key}
              data-index={vi.index}
              ref={virtualizer.measureElement}
              className="absolute left-0 top-0 w-full"
              style={{ transform: `translateY(${vi.start}px)` }}
            >
              {item
                ? renderItem(item, vi.index, itemAt(vi.index - 1))
                : (renderPlaceholder?.(vi.index) ?? (
                    <div className="px-4 pb-1">
                      <div className="h-9 animate-pulse rounded-2xl bg-muted/40" />
                    </div>
                  ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
