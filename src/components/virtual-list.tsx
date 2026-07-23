import { useRef, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

/**
 * Virtualizes an already-loaded array: only the rows in view (plus overscan) are
 * mounted, so a list of tens of thousands of items stays cheap to render. Use
 * this when the whole list is already in memory; use LazyVirtualList when the
 * data itself must be fetched in windows.
 *
 * The scroll container uses `min-h-0` so it actually scrolls instead of growing
 * to its content height (see the note in lazy-virtual-list.tsx). It fills its
 * flex parent, so place it inside a `flex h-full flex-col` with a fixed-height
 * ancestor (the app already provides one via `h-svh` on the shell).
 */
export function VirtualList<T>({
  items,
  renderItem,
  estimateSize = 56,
  getKey,
  className,
  underlap = false,
}: {
  items: T[];
  renderItem: (item: T, index: number) => ReactNode;
  estimateSize?: number;
  /** Stable key per row; defaults to the index. */
  getKey?: (item: T, index: number) => React.Key;
  /** Extra classes for the inner content wrapper (e.g. max-width, padding). */
  className?: string;
  /** Let this list scroll beneath the translucent title bar (only sensible
   *  when the list is the view's topmost element; see index.css). */
  underlap?: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => estimateSize,
    overscan: 12,
    getItemKey: getKey ? (i) => getKey(items[i], i) : undefined,
  });

  return (
    <div
      ref={scrollRef}
      data-underlap={underlap ? "" : undefined}
      className="min-h-0 flex-1 overflow-auto"
    >
      <div
        className={className}
        style={{ position: "relative", width: "100%", height: virtualizer.getTotalSize() }}
      >
        {virtualizer.getVirtualItems().map((vi) => (
          <div
            key={vi.key}
            data-index={vi.index}
            ref={virtualizer.measureElement}
            className="absolute left-0 top-0 w-full"
            style={{ transform: `translateY(${vi.start}px)` }}
          >
            {renderItem(items[vi.index], vi.index)}
          </div>
        ))}
      </div>
    </div>
  );
}
