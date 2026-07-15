/**
 * Shared view primitives — the design base every artifact view builds on, so
 * Messages / Contacts / Calls / Safari are consistent by construction rather
 * than by copy-paste. See docs/ui.md.
 *
 * These compose shadcn primitives (Empty, Item, ScrollArea); they add layout,
 * not new styling systems. If a view needs something bespoke, prefer adding a
 * primitive here over inlining it in the view.
 */
import { Search, TriangleAlert } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { ResizeHandle, useResizableWidth } from "@/components/resize";
import { VirtualList } from "@/components/virtual-list";
import { LazyVirtualList } from "@/components/lazy-virtual-list";
import { formatCount } from "@/lib/format";
import { cn } from "@/lib/utils";

/** The header strip at the top of a view: title, optional count, actions. */
export function ViewHeader({
  title,
  count,
  icon,
  children,
}: {
  title: string;
  count?: number;
  icon?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
      {icon}
      <h1 className="text-base font-semibold">{title}</h1>
      {count !== undefined && (
        <span className="text-xs tabular-nums text-muted-foreground/60">
          {formatCount(count)}
        </span>
      )}
      {children && (
        <div className="ml-auto flex min-w-0 items-center gap-2">{children}</div>
      )}
    </header>
  );
}

/** A load-failure state, so a failed query surfaces the error instead of quietly
 *  looking like an empty list. */
export function ErrorState({ error }: { error: unknown }) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <TriangleAlert />
        </EmptyMedia>
        <EmptyTitle>Couldn't load this</EmptyTitle>
        <EmptyDescription className="max-w-md select-text break-words">
          {error instanceof Error ? error.message : String(error)}
        </EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

/**
 * A full-height, single-column view whose rows are virtualized — the same shape
 * as ListView, but only the visible rows mount, so it stays fast for lists of
 * tens of thousands of items (Calls, Safari, Apps). Rows are centered to the
 * same max width as ListView.
 */
export function VirtualListView<T>({
  title,
  count,
  header,
  items,
  renderItem,
  getKey,
  estimateSize = 64,
  isPending,
  error,
  emptyMessage = "Nothing here.",
}: {
  title: string;
  count?: number;
  header?: React.ReactNode;
  items: T[];
  renderItem: (item: T) => React.ReactNode;
  getKey?: (item: T, index: number) => React.Key;
  estimateSize?: number;
  isPending?: boolean;
  error?: unknown;
  emptyMessage?: string;
}) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={title} count={count}>
        {header}
      </ViewHeader>
      {error ? (
        <ErrorState error={error} />
      ) : isPending ? (
        <div className="mx-auto w-full max-w-3xl p-2">
          {Array.from({ length: 8 }).map((_, i) => (
            <Skeleton key={i} className="mb-1 h-14 w-full" />
          ))}
        </div>
      ) : items.length === 0 ? (
        <p className="p-6 text-center text-sm text-muted-foreground">{emptyMessage}</p>
      ) : (
        <VirtualList
          items={items}
          estimateSize={estimateSize}
          getKey={getKey}
          renderItem={(item) => (
            <div className="mx-auto max-w-3xl px-2 pb-1">{renderItem(item)}</div>
          )}
        />
      )}
    </div>
  );
}

/**
 * Like VirtualListView, but the data itself is fetched in windows (a cheap COUNT
 * plus lazily-loaded slices) instead of all at once — for lists that can reach
 * tens of thousands of rows (Photos, Calls, Safari). Filtering/search must be
 * done by `fetchWindow`/`count` (server-side), and `resetKey` should change when
 * the filter changes so the scroll position resets.
 */
export function LazyListView<T>({
  title,
  header,
  toolbar,
  count,
  windowKey,
  fetchWindow,
  renderItem,
  resetKey,
  estimateSize = 64,
  error,
  emptyMessage = "Nothing here.",
}: {
  title: string;
  header?: React.ReactNode;
  /** An optional full-width row below the title (filters, etc.). */
  toolbar?: React.ReactNode;
  /** Total matching rows (from a count query); undefined while loading. */
  count: number | undefined;
  windowKey: (page: number) => unknown[];
  fetchWindow: (offset: number, limit: number) => Promise<T[]>;
  renderItem: (item: T) => React.ReactNode;
  resetKey?: unknown;
  estimateSize?: number;
  error?: unknown;
  emptyMessage?: string;
}) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={title} count={count}>
        {header}
      </ViewHeader>
      {toolbar && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-1.5">
          {toolbar}
        </div>
      )}
      {error ? (
        <ErrorState error={error} />
      ) : count === undefined ? (
        <div className="mx-auto w-full max-w-3xl p-2">
          {Array.from({ length: 8 }).map((_, i) => (
            <Skeleton key={i} className="mb-1 h-14 w-full" />
          ))}
        </div>
      ) : count === 0 ? (
        <p className="p-6 text-center text-sm text-muted-foreground">{emptyMessage}</p>
      ) : (
        <LazyVirtualList<T>
          // Remount when the filter changes so scroll/measurement reset cleanly.
          key={String(resetKey)}
          count={count}
          estimateSize={estimateSize}
          windowKey={windowKey}
          fetchWindow={fetchWindow}
          renderItem={(item) => (
            <div className="mx-auto max-w-3xl px-2 pb-1">{renderItem(item)}</div>
          )}
        />
      )}
    </div>
  );
}

/** Master list on the left, detail pane on the right (Messages, Contacts). The
 *  master width is drag-resizable and persisted (shared across these views). */
export function ListDetail({
  master,
  detail,
}: {
  master: React.ReactNode;
  detail: React.ReactNode;
}) {
  const { width, startResize } = useResizableWidth("traceloupe-master-width", 288, 200, 560);
  return (
    <div className="flex h-full">
      {/* min-h-0 lets the master's scroll area shrink to the column height and
          actually scroll, instead of growing with its content. */}
      <div className="flex min-h-0 shrink-0 flex-col border-r" style={{ width }}>
        {master}
      </div>
      <ResizeHandle onPointerDown={(e) => startResize(e, "right")} />
      <div className="min-w-0 flex-1">{detail}</div>
    </div>
  );
}

/** A search box for filtering a list. Controlled. */
export function ListSearch({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  return (
    <div className="relative">
      <Search className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="h-9 select-text pl-8"
      />
    </div>
  );
}

/** Standard empty / no-selection state, built on shadcn Empty. */
export function EmptyView({
  icon: Icon,
  title,
  description,
  children,
  className,
}: {
  icon?: React.ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <Empty className={cn("h-full", className)}>
      <EmptyHeader>
        {Icon && (
          <EmptyMedia variant="icon">
            <Icon className="size-6" />
          </EmptyMedia>
        )}
        <EmptyTitle>{title}</EmptyTitle>
        {description && <EmptyDescription>{description}</EmptyDescription>}
      </EmptyHeader>
      {children}
    </Empty>
  );
}

export { Search };
