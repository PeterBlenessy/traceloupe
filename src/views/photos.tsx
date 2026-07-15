import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ChevronLeft,
  ChevronRight,
  Image as ImageIcon,
  Play,
  X,
} from "lucide-react";

/** Media items fetched per lazy window (shared by the grid and the lightbox's
 *  neighbour lookup so their cache keys line up). */
const PAGE = 100;
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { SortControl, type SortState } from "@/components/sort-control";
import { EmptyView, ErrorState, ViewHeader } from "@/components/view";
import { formatCount, formatDateTime } from "@/lib/format";
import { client, type MediaItem } from "@/lib/ipc";

export function PhotosView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [source, setSource] = useState<string>("all");
  const sourceArg = source === "all" ? null : source;
  const { data: count, error } = useQuery({
    queryKey: ["mediaCount", source],
    queryFn: () => client.countMedia(sourceArg),
    enabled: active === true,
  });
  const { data: sources } = useQuery({
    queryKey: ["mediaSources"],
    queryFn: () => client.mediaSources(),
    enabled: active === true,
  });
  const [openIndex, setOpenIndex] = useState<number | null>(null);
  const [sort, setSort] = useState<SortState>({ by: "date", desc: true });
  const qc = useQueryClient();

  const ensurePage = useCallback(
    (page: number) => {
      void qc.prefetchQuery({
        queryKey: ["mediaWindow", sourceArg, sort.by, sort.desc, page],
        queryFn: () =>
          client.getMediaWindow(
            sourceArg,
            page * PAGE,
            PAGE,
            sort.by,
            sort.desc,
          ),
      });
    },
    [qc, sourceArg, sort],
  );

  if (active === false) {
    return (
      <EmptyView
        icon={ImageIcon}
        title="No backup open"
        description="Import a backup to see photos and videos."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const hasFilter = (sources?.length ?? 0) > 1;
  const total = sources?.reduce((sum, [, c]) => sum + c, 0) ?? 0;

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Photos" count={count} />
      <div className="flex shrink-0 items-center justify-between gap-2 border-b px-2 py-2">
        {hasFilter ? (
          <SourceFilter
            sources={sources ?? []}
            total={total}
            value={source}
            onChange={setSource}
          />
        ) : (
          <span />
        )}
        <SortControl
          fields={[
            { value: "date", label: "Date" },
            { value: "source", label: "Source" },
          ]}
          value={sort}
          onChange={setSort}
        />
      </div>
      {error ? (
        <ErrorState error={error} />
      ) : count === undefined ? (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-1 p-1">
          {Array.from({ length: 12 }).map((_, i) => (
            <Skeleton key={i} className="aspect-square" />
          ))}
        </div>
      ) : count === 0 ? (
        <p className="p-6 text-center text-sm text-muted-foreground">
          {source === "all"
            ? "No photos or videos in this backup."
            : "No media from this source."}
        </p>
      ) : (
        // key by source+sort so the grid remounts (scroll + measurement reset) on change.
        <MediaGrid
          key={`${source}:${sort.by}:${sort.desc}`}
          count={count}
          source={sourceArg}
          sort={sort}
          onOpen={setOpenIndex}
        />
      )}

      <Lightbox
        index={openIndex}
        count={count ?? 0}
        source={sourceArg}
        sort={sort}
        ensurePage={ensurePage}
        onNavigate={setOpenIndex}
        onClose={() => setOpenIndex(null)}
      />
    </div>
  );
}

function SourceFilter({
  sources,
  total,
  value,
  onChange,
}: {
  sources: [string, number][];
  total: number;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <ToggleGroup
      type="single"
      value={value}
      onValueChange={(v) => onChange(v || "all")}
      variant="outline"
      size="sm"
      className="flex-wrap justify-start"
    >
      <ToggleGroupItem value="all">All {formatCount(total)}</ToggleGroupItem>
      {sources.map(([name, count]) => (
        <ToggleGroupItem key={name} value={name}>
          {name} {formatCount(count)}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

/**
 * Row-virtualized thumbnail grid. A real camera roll holds thousands of media
 * items, and every rendered <img> spawns a native `sips` transcode — so we mount
 * only the rows in view. Columns are derived from the live container width to
 * keep the responsive auto-fill layout.
 */
function MediaGrid({
  count,
  source,
  sort,
  onOpen,
}: {
  count: number;
  source: string | null;
  sort: SortState;
  onOpen: (index: number) => void;
}) {
  const GAP = 4; // matches gap-1 / p-1 (0.25rem)
  const MIN = 144; // 9rem minimum tile
  const scrollRef = useRef<HTMLDivElement>(null);
  const [cols, setCols] = useState(1);
  const [cell, setCell] = useState(MIN);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const compute = () => {
      const w = el.clientWidth - GAP * 2;
      const c = Math.max(1, Math.floor((w + GAP) / (MIN + GAP)));
      setCols(c);
      setCell((w - GAP * (c - 1)) / c);
    };
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const rowCount = Math.ceil(count / cols);
  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => cell + GAP,
    overscan: 3,
  });
  useLayoutEffect(() => {
    rowVirtualizer.measure();
  }, [cell, cols, rowVirtualizer]);

  // Lazily fetch only the item-windows the visible rows cover.
  const virtualRows = rowVirtualizer.getVirtualItems();
  const firstRow = virtualRows[0]?.index ?? 0;
  const lastRow = virtualRows[virtualRows.length - 1]?.index ?? 0;
  const firstPage = Math.floor((firstRow * cols) / PAGE);
  const lastPage = Math.floor(((lastRow + 1) * cols - 1) / PAGE);
  const pages = useMemo(() => {
    const out: number[] = [];
    for (let p = Math.max(0, firstPage); p <= Math.max(0, lastPage); p++)
      out.push(p);
    return out;
  }, [firstPage, lastPage]);
  const queries = useQueries({
    queries: pages.map((p) => ({
      queryKey: ["mediaWindow", source, sort.by, sort.desc, p],
      queryFn: () =>
        client.getMediaWindow(source, p * PAGE, PAGE, sort.by, sort.desc),
    })),
  });
  const loaded = new Map<number, MediaItem[]>();
  pages.forEach((p, i) => {
    const data = queries[i].data;
    if (data) loaded.set(p, data);
  });
  const itemAt = (index: number): MediaItem | undefined =>
    loaded.get(Math.floor(index / PAGE))?.[index % PAGE];

  return (
    // min-h-0 lets this flex child actually scroll; without it the grid grows to
    // its full content height and the virtualizer mounts every row (and spawns a
    // `sips` transcode per thumbnail), freezing the app.
    <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto p-1">
      <div
        className="relative w-full"
        style={{ height: rowVirtualizer.getTotalSize() }}
      >
        {virtualRows.map((row) => {
          const start = row.index * cols;
          return (
            <div
              key={row.key}
              className="absolute left-0 top-0 flex w-full gap-1"
              style={{ transform: `translateY(${row.start}px)`, height: cell }}
            >
              {Array.from({ length: cols }).map((_, c) => {
                const index = start + c;
                if (index >= count) return null;
                const item = itemAt(index);
                return (
                  <div key={index} style={{ width: cell }}>
                    {item ? (
                      <Thumb item={item} onOpen={() => onOpen(index)} />
                    ) : (
                      <div className="aspect-square w-full animate-pulse rounded-sm bg-muted" />
                    )}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Thumb({ item, onOpen }: { item: MediaItem; onOpen: () => void }) {
  const isVideo = item.kind === "video";
  return (
    <button
      onClick={onOpen}
      className="group relative aspect-square w-full overflow-hidden rounded-sm bg-muted"
    >
      <img
        src={client.mediaUrl(item.id, { thumb: true })}
        alt={item.filename ?? ""}
        loading="lazy"
        className="size-full object-cover transition-transform group-hover:scale-105"
      />
      {item.source && (
        <span className="absolute bottom-1 left-1 rounded bg-black/55 px-1.5 py-0.5 text-[10px] font-medium text-white opacity-0 transition-opacity group-hover:opacity-100">
          {item.source}
        </span>
      )}
      {isVideo && (
        <span className="absolute inset-0 flex items-center justify-center bg-black/20">
          <Play className="size-8 fill-white text-white" />
        </span>
      )}
    </button>
  );
}

function Lightbox({
  index,
  count,
  source,
  sort,
  ensurePage,
  onNavigate,
  onClose,
}: {
  index: number | null;
  count: number;
  source: string | null;
  sort: SortState;
  ensurePage: (page: number) => void;
  onNavigate: (index: number) => void;
  onClose: () => void;
}) {
  const open = index != null;
  // Subscribe to the current item's window (same key the grid fills) so the view
  // re-renders when a not-yet-loaded window resolves — a non-reactive cache read
  // would leave the spinner stuck until the next interaction.
  const page = index != null ? Math.floor(index / PAGE) : 0;
  const { data: win } = useQuery({
    queryKey: ["mediaWindow", source, sort.by, sort.desc, page],
    queryFn: () =>
      client.getMediaWindow(source, page * PAGE, PAGE, sort.by, sort.desc),
    enabled: index != null,
  });
  const item = index != null ? win?.[index % PAGE] : undefined;
  const hasPrev = index != null && index > 0;
  const hasNext = index != null && index < count - 1;
  const go = (delta: number) => {
    if (index == null) return;
    const next = index + delta;
    if (next >= 0 && next < count) onNavigate(next);
  };

  // Preload the current and neighbouring windows so paging lands on a real image
  // rather than a blank while its window fetches.
  useEffect(() => {
    if (index == null) return;
    for (const i of [index, index - 1, index + 1]) {
      if (i >= 0 && i < count) ensurePage(Math.floor(i / PAGE));
    }
  }, [index, count, ensurePage]);

  // Arrow keys page prev/next (Dialog already handles Escape).
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") go(-1);
      else if (e.key === "ArrowRight") go(1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, index, count]);

  const isVideo = item?.kind === "video";
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="flex h-[95vh] max-w-[95vw] flex-col border-none bg-transparent p-0 shadow-none sm:max-w-[95vw]"
      >
        <DialogTitle className="sr-only">
          {item?.filename ?? "Media"}
        </DialogTitle>
        {/* Close */}
        <button
          onClick={onClose}
          aria-label="Close"
          className="absolute right-2 top-2 z-10 rounded-full bg-black/50 p-2 text-white hover:bg-black/70"
        >
          <X className="size-5" />
        </button>
        <div className="relative flex min-h-0 flex-1 items-center justify-center">
          {hasPrev && (
            <button
              onClick={() => go(-1)}
              aria-label="Previous"
              className="absolute left-2 z-10 rounded-full bg-black/50 p-2 text-white hover:bg-black/70"
            >
              <ChevronLeft className="size-6" />
            </button>
          )}
          {item ? (
            isVideo ? (
              <video
                key={item.id}
                src={client.mediaUrl(item.id)}
                controls
                autoPlay
                className="max-h-full max-w-full object-contain"
              />
            ) : (
              <img
                key={item.id}
                src={client.mediaUrl(item.id)}
                alt={item.filename ?? ""}
                className="max-h-full max-w-full object-contain"
              />
            )
          ) : (
            <div className="size-16 animate-pulse rounded-full bg-white/20" />
          )}
          {hasNext && (
            <button
              onClick={() => go(1)}
              aria-label="Next"
              className="absolute right-2 z-10 rounded-full bg-black/50 p-2 text-white hover:bg-black/70"
            >
              <ChevronRight className="size-6" />
            </button>
          )}
        </div>
        <div className="flex items-center justify-between gap-2 px-2 py-1.5 text-xs text-white/80">
          <span className="select-text truncate">{item?.filename ?? "—"}</span>
          <div className="flex shrink-0 items-center gap-3">
            {index != null && (
              <span className="tabular-nums">
                {formatCount(index + 1)} / {formatCount(count)}
              </span>
            )}
            {item?.source && <span>{item.source}</span>}
            {item?.takenAt && <span>{formatDateTime(item.takenAt)}</span>}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
