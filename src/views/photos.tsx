import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Image as ImageIcon, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { SortControl, type SortState } from "@/components/sort-control";
import { EmptyView, ViewHeader } from "@/components/view";
import { formatDateTime } from "@/lib/format";
import { client, type MediaItem } from "@/lib/ipc";

export function PhotosView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [source, setSource] = useState<string>("all");
  const sourceArg = source === "all" ? null : source;
  const { data: count } = useQuery({
    queryKey: ["mediaCount", source],
    queryFn: () => client.countMedia(sourceArg),
    enabled: active === true,
  });
  const { data: sources } = useQuery({
    queryKey: ["mediaSources"],
    queryFn: () => client.mediaSources(),
    enabled: active === true,
  });
  const [openItem, setOpenItem] = useState<MediaItem | null>(null);
  const [sort, setSort] = useState<SortState>({ by: "date", desc: true });

  if (active === false) {
    return (
      <EmptyView icon={ImageIcon} title="No backup open" description="Import a backup to see photos and videos.">
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
      {count === undefined ? (
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
          onOpen={setOpenItem}
        />
      )}

      <Lightbox item={openItem} onClose={() => setOpenItem(null)} />
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
      <ToggleGroupItem value="all">All {total}</ToggleGroupItem>
      {sources.map(([name, count]) => (
        <ToggleGroupItem key={name} value={name}>
          {name} {count}
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
  onOpen: (item: MediaItem) => void;
}) {
  const GAP = 4; // matches gap-1 / p-1 (0.25rem)
  const MIN = 144; // 9rem minimum tile
  const PAGE = 100; // media items fetched per lazy window
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
    for (let p = Math.max(0, firstPage); p <= Math.max(0, lastPage); p++) out.push(p);
    return out;
  }, [firstPage, lastPage]);
  const queries = useQueries({
    queries: pages.map((p) => ({
      queryKey: ["mediaWindow", source, sort.by, sort.desc, p],
      queryFn: () => client.getMediaWindow(source, p * PAGE, PAGE, sort.by, sort.desc),
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
      <div className="relative w-full" style={{ height: rowVirtualizer.getTotalSize() }}>
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
                      <Thumb item={item} onOpen={() => onOpen(item)} />
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

function Lightbox({ item, onClose }: { item: MediaItem | null; onClose: () => void }) {
  return (
    <Dialog open={!!item} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-3xl gap-2 p-2">
        <DialogTitle className="sr-only">{item?.filename ?? "Media"}</DialogTitle>
        {item && (
          <>
            <div className="flex items-center justify-center bg-muted/40">
              <img
                src={client.mediaUrl(item.id)}
                alt={item.filename ?? ""}
                className="max-h-[70vh] w-auto object-contain"
              />
            </div>
            <div className="flex items-center justify-between gap-2 px-2 pb-1 text-xs text-muted-foreground">
              <span className="select-text truncate">{item.filename ?? "—"}</span>
              <div className="flex shrink-0 items-center gap-2">
                {item.source && <span>{item.source}</span>}
                {item.takenAt && <span>{formatDateTime(item.takenAt)}</span>}
              </div>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
