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
import { useNavigate } from "@tanstack/react-router";
import { Search, TriangleAlert, X } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Button } from "@/components/ui/button";
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
 * A full-height, single-column view whose rows are virtualized — the same shape
 * as ListView, but only the visible rows mount, so it stays fast for lists of
 * tens of thousands of items (Calls, Safari, Apps). Rows are centered to the
 * same max width as ListView.
 */
export function VirtualListView<T>({
  items,
  renderItem,
  getKey,
  estimateSize = 64,
  isPending,
  error,
  emptyMessage = "Nothing here.",
  emptyIcon,
}: {
  /** Unused now that all list views publish to the unified toolbar; kept so
   *  callers can pass it for readability. */
  title?: string;
  count?: number;
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
  count,
  windowKey,
  fetchWindow,
  renderItem,
  resetKey,
  estimateSize = 64,
  error,
  emptyMessage = "Nothing here.",
  emptyIcon,
}: {
  /** Unused now that all list views publish to the unified toolbar; kept so
   *  callers can pass it for readability. */
  title?: string;
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
}) {
  return (
    <div className="flex h-full flex-col">
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

  // One bordered box whose WIDTH animates from the collapsed icon button (w-8)
  // to the open input (w-44/w-56). Width animates reliably in WebKit — unlike the
  // `scale`/`translate` properties. The border/bg live on the container (so the
  // collapsed state is a clean island button); the input inside is transparent
  // and clipped, so nothing bleeds under the neighbouring app controls.
  return (
    <div
      onClick={() => {
        if (!open) {
          setFocused(true);
          requestAnimationFrame(() => inputRef.current?.focus());
        }
      }}
      className={cn(
        "relative flex h-8 shrink-0 items-center overflow-hidden rounded-lg border border-border/70 bg-muted/40 text-muted-foreground transition-[width] duration-200 ease-out",
        open ? "w-44 sm:w-56" : "w-8 cursor-pointer hover:bg-accent hover:text-foreground",
      )}
    >
      <Search className="pointer-events-none absolute left-2 top-1/2 size-4 -translate-y-1/2" />
      <input
        ref={inputRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        placeholder={open ? placeholder : ""}
        aria-label={placeholder}
        title={open ? undefined : placeholder}
        className="h-full w-full select-text bg-transparent pl-8 pr-7 text-sm text-foreground outline-none placeholder:text-muted-foreground"
      />
      {open && value && (
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

/**
 * The rich "no backup open" state every content view shows before a backup is
 * loaded. Doubles as onboarding: it leads with the VALUE — a value title, a lead
 * paragraph, a grid of capabilities, a privacy note — and only then, as a helper
 * line on the CTA, asks the user to open a backup. (Leading with "open a backup"
 * front-loads a chore before any value.) Kept here so all views stay consistent
 * by construction. See docs/ui.md.
 */
export function NoBackupState({
  icon: Icon,
  title,
  lead,
  features,
  note,
  cta = "You'll need to open an iPhone backup to get started — TraceLoupe reads it once, then everything above is here.",
}: {
  icon: React.ComponentType<{ className?: string }>;
  /** Value title — what the view gives you, NOT "Open a backup to …"
   *  (e.g. "Browse photos & videos"). The backup ask lives in `cta`. */
  title: string;
  /** One or two sentences on what the view is. */
  lead: string;
  /** Capability highlights — what the user can actually do in this view. */
  features?: { label: string; detail: string }[];
  /** Closing line, typically the local/privacy assurance. */
  note?: string;
  /** The helper line directly above the "Choose a backup" button — where the
   *  "open a backup" ask belongs, tied to the action rather than the headline. */
  cta?: string;
}) {
  const navigate = useNavigate();
  return (
    // Top-aligned (not vertically centered): the sidebar items are top-aligned,
    // so a centered hero reads as floating/misaligned next to them.
    <div className="flex h-full items-start justify-center overflow-y-auto p-6">
      <div className="w-full max-w-xl pt-16 pb-8 text-center">
        <div className="mx-auto mb-4 flex size-12 items-center justify-center rounded-xl bg-muted text-muted-foreground">
          <Icon className="size-6" />
        </div>
        <h2 className="text-lg font-semibold">{title}</h2>
        <p className="mx-auto mt-2 max-w-prose text-sm leading-relaxed text-muted-foreground">
          {lead}
        </p>
        {features && features.length > 0 && (
          <ul className="mx-auto mt-5 grid max-w-md gap-2.5 text-left sm:grid-cols-2">
            {features.map((f) => (
              <li key={f.label} className="rounded-lg border bg-card/40 p-3">
                <div className="text-xs font-medium">{f.label}</div>
                <div className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
                  {f.detail}
                </div>
              </li>
            ))}
          </ul>
        )}
        {note && (
          <p className="mx-auto mt-4 max-w-prose text-xs leading-relaxed text-muted-foreground/80">
            {note}
          </p>
        )}
        <p className="mx-auto mt-8 max-w-prose text-sm text-muted-foreground">
          {cta}
        </p>
        <div className="mt-3">
          <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
        </div>
      </div>
    </div>
  );
}

export { Search };
