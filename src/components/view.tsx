/**
 * Shared view primitives — the design base every artifact view builds on, so
 * Messages / Contacts / Calls / Safari are consistent by construction rather
 * than by copy-paste. See docs/ui.md.
 *
 * These compose shadcn primitives (Empty, Item, ScrollArea); they add layout,
 * not new styling systems. If a view needs something bespoke, prefer adding a
 * primitive here over inlining it in the view.
 */
import { useRef, useState } from "react";
import { Search, SlidersHorizontal, TriangleAlert, X } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { ResizeHandle, useResizableWidth } from "@/components/resize";
import { VirtualList } from "@/components/virtual-list";
import { LazyVirtualList } from "@/components/lazy-virtual-list";
import { usePersistedState } from "@/lib/use-persisted-state";
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
        // flex-1 (not ml-auto content-width) so a child can actually claim the
        // free header width — an OverflowRow filter needs it to measure how many
        // chips fit; justify-end keeps plain metadata right-aligned as before.
        <div className="flex min-w-0 flex-1 items-center justify-end gap-2">
          {children}
        </div>
      )}
    </header>
  );
}

/** The one list loading state — a stack of row-height skeletons. Shared by every
 *  list primitive so "loading" looks identical everywhere (no ad-hoc pulses). */
export function ListSkeleton({ rows = 8 }: { rows?: number }) {
  return (
    <div className="w-full p-2">
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className="mb-1 h-14 w-full" />
      ))}
    </div>
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
 * The canonical panel header every list view shares (see docs/ui.md): a title row
 * (title + count + inline filter chips), an optional full-width search row, and an
 * optional filter/sort toolbar row. Used by `VirtualListView`/`LazyListView` and
 * composed directly by the master column of `ListDetail` views — so all list
 * headers are consistent by construction, not by copy-paste.
 */
export function PanelHeader({
  title,
  count,
  icon,
  actions,
  search,
  toolbar,
}: {
  title: string;
  count?: number;
  icon?: React.ReactNode;
  /** Inline controls on the title row (e.g. source/type filter chips). */
  actions?: React.ReactNode;
  /** The search control — rendered inline in the title row (expanding icon). */
  search?: React.ReactNode;
  /** Filter/sort controls, revealed in a row below the title by the Filter
   *  toggle so the header stays minimal until the user wants them. */
  toolbar?: React.ReactNode;
}) {
  // Remember per view whether the filter row is revealed.
  const [filtersOpen, setFiltersOpen] = usePersistedState(
    `panel-filters:${title}`,
    false,
  );
  return (
    <>
      <ViewHeader title={title} count={count} icon={icon}>
        {actions && (
          // Give the filter actions the free width (and make the actions element
          // itself fill it) so an OverflowRow measures the real available space
          // and shows every chip that fits instead of collapsing into "⋮".
          <div className="flex min-w-0 flex-1 items-center gap-1 [&>*]:min-w-0 [&>*]:flex-1">
            {actions}
          </div>
        )}
        {toolbar && (
          <button
            type="button"
            aria-label="Filters and sort"
            aria-pressed={filtersOpen}
            title="Filters & sort"
            onClick={() => setFiltersOpen((o) => !o)}
            className={cn(
              "inline-flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
              filtersOpen && "bg-accent text-foreground",
            )}
          >
            <SlidersHorizontal className="size-4" />
          </button>
        )}
        {search}
      </ViewHeader>
      {toolbar && filtersOpen && (
        // No wrap: a wide toolbar (time chips + sort) scrolls its own content
        // rather than pushing the sort onto a second row.
        <div className="flex min-w-0 shrink-0 items-center gap-2 border-b px-3 py-1.5">
          {toolbar}
        </div>
      )}
    </>
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
  search,
  toolbar,
  items,
  renderItem,
  getKey,
  estimateSize = 64,
  isPending,
  error,
  emptyMessage = "Nothing here.",
  emptyIcon,
}: {
  title: string;
  count?: number;
  header?: React.ReactNode;
  search?: React.ReactNode;
  toolbar?: React.ReactNode;
  items: T[];
  renderItem: (item: T) => React.ReactNode;
  getKey?: (item: T, index: number) => React.Key;
  estimateSize?: number;
  isPending?: boolean;
  error?: unknown;
  emptyMessage?: string;
  emptyIcon?: React.ComponentType<{ className?: string }>;
}) {
  return (
    <div className="flex h-full flex-col">
      <PanelHeader
        title={title}
        count={count}
        actions={header}
        search={search}
        toolbar={toolbar}
      />
      {error ? (
        <ErrorState error={error} />
      ) : isPending ? (
        <ListSkeleton />
      ) : items.length === 0 ? (
        <EmptyView icon={emptyIcon} title={emptyMessage} />
      ) : (
        <VirtualList
          items={items}
          estimateSize={estimateSize}
          getKey={getKey}
          renderItem={(item) => (
            <div className="px-2 pb-1">{renderItem(item)}</div>
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
  search,
  toolbar,
  count,
  windowKey,
  fetchWindow,
  renderItem,
  resetKey,
  estimateSize = 64,
  error,
  emptyMessage = "Nothing here.",
  emptyIcon,
  headless,
}: {
  title: string;
  header?: React.ReactNode;
  /** An optional full-width search row directly below the title. */
  search?: React.ReactNode;
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
  emptyIcon?: React.ComponentType<{ className?: string }>;
  /** Skip the internal PanelHeader — the view publishes to the unified app
   *  toolbar (see useViewToolbar) and renders only the list body. */
  headless?: boolean;
}) {
  return (
    <div className="flex h-full flex-col">
      {!headless && (
        <PanelHeader
          title={title}
          count={count}
          actions={header}
          search={search}
          toolbar={toolbar}
        />
      )}
      {error ? (
        <ErrorState error={error} />
      ) : count === undefined ? (
        <ListSkeleton />
      ) : count === 0 ? (
        <EmptyView icon={emptyIcon} title={emptyMessage} />
      ) : (
        <LazyVirtualList<T>
          // Remount when the filter changes so scroll/measurement reset cleanly.
          key={String(resetKey)}
          count={count}
          estimateSize={estimateSize}
          windowKey={windowKey}
          fetchWindow={fetchWindow}
          renderItem={(item) => (
            <div className="px-2 pb-1">{renderItem(item)}</div>
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

/**
 * A macOS-toolbar-style search that collapses to a single icon and expands into
 * an input on click (and stays open while it has text) — so it lives inline in
 * the header toolbar instead of taking a whole row. Controlled value.
 */
export function ListSearch({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  const [focused, setFocused] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const open = focused || value.length > 0;

  if (!open) {
    return (
      <button
        type="button"
        aria-label={placeholder}
        title={placeholder}
        onClick={() => {
          setFocused(true);
          requestAnimationFrame(() => inputRef.current?.focus());
        }}
        className="inline-flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
      >
        <Search className="size-4" />
      </button>
    );
  }
  return (
    <div className="relative w-44 shrink-0 transition-[width] duration-200 sm:w-56">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        ref={inputRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={() => setFocused(false)}
        placeholder={placeholder}
        className="h-8 select-text pl-8 pr-7"
      />
      {value && (
        <button
          type="button"
          aria-label="Clear search"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onChange("")}
          className="absolute right-1.5 top-1/2 flex size-5 -translate-y-1/2 items-center justify-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      )}
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
