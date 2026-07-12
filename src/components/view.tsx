/**
 * Shared view primitives — the design base every artifact view builds on, so
 * Messages / Contacts / Calls / Safari are consistent by construction rather
 * than by copy-paste. See docs/ui.md.
 *
 * These compose shadcn primitives (Empty, Item, ScrollArea); they add layout,
 * not new styling systems. If a view needs something bespoke, prefer adding a
 * primitive here over inlining it in the view.
 */
import { Search } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { VirtualList } from "@/components/virtual-list";
import { LazyVirtualList } from "@/components/lazy-virtual-list";
import { cn } from "@/lib/utils";

/** The header strip at the top of a view: title, optional count, actions. */
export function ViewHeader({
  title,
  count,
  children,
}: {
  title: string;
  count?: number;
  children?: React.ReactNode;
}) {
  return (
    <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
      <h1 className="text-base font-semibold">{title}</h1>
      {count !== undefined && (
        <span className="text-sm text-muted-foreground">{count}</span>
      )}
      {children && <div className="ml-auto flex items-center gap-2">{children}</div>}
    </header>
  );
}

/** A full-height view that is a single scrolling column (Calls, Safari). */
export function ListView({
  title,
  count,
  header,
  children,
}: {
  title: string;
  count?: number;
  header?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={title} count={count}>
        {header}
      </ViewHeader>
      <ScrollArea className="flex-1">
        <div className="mx-auto max-w-3xl p-2">{children}</div>
      </ScrollArea>
    </div>
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
  emptyMessage?: string;
}) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={title} count={count}>
        {header}
      </ViewHeader>
      {isPending ? (
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
  count,
  windowKey,
  fetchWindow,
  renderItem,
  resetKey,
  estimateSize = 64,
  emptyMessage = "Nothing here.",
}: {
  title: string;
  header?: React.ReactNode;
  /** Total matching rows (from a count query); undefined while loading. */
  count: number | undefined;
  windowKey: (page: number) => unknown[];
  fetchWindow: (offset: number, limit: number) => Promise<T[]>;
  renderItem: (item: T) => React.ReactNode;
  resetKey?: unknown;
  estimateSize?: number;
  emptyMessage?: string;
}) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={title} count={count}>
        {header}
      </ViewHeader>
      {count === undefined ? (
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

/** Master list on the left, detail pane on the right (Messages, Contacts). */
export function ListDetail({
  master,
  detail,
}: {
  master: React.ReactNode;
  detail: React.ReactNode;
}) {
  return (
    <div className="flex h-full">
      <div className="flex w-72 shrink-0 flex-col border-r">{master}</div>
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
