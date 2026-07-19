import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { MediaCacheKeyBoundary, useMediaCacheKey } from "@/lib/use-media-cache-key";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Camera,
  EyeOff,
  Frame,
  Heart,
  Image as ImageIcon,
  Images,
  MapPin,
  Play,
  Smartphone,
  Users,
} from "lucide-react";

/** Media items fetched per lazy window (shared by the grid and the lightbox's
 *  neighbour lookup so their cache keys line up). */
const PAGE = 100;
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { MediaLightbox } from "@/components/media-lightbox";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { EmptyView, ErrorState, ListSearch } from "@/components/view";
import { useDebounced } from "@/lib/use-debounced";
import { formatCount, formatDateTime } from "@/lib/format";
import { serviceSlug } from "@/lib/apps";
import { BrandIcon, hasBrandIcon } from "@/lib/brand-icon";
import { client, type MediaItem, type TimeRange } from "@/lib/ipc";

export function PhotosView() {
  // One media cache key per mount, shared by every image below (see
  // use-media-cache-key): view-switch remounts bust WebKit's cached-failed
  // scheme tasks while scrolling reuses URLs.
  return (
    <MediaCacheKeyBoundary>
      <PhotosViewInner />
    </MediaCacheKeyBoundary>
  );
}

function PhotosViewInner() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [sourcePref, setSource] = usePersistedState<string>(
    "photos:source",
    "all",
  );
  const { data: sources } = useQuery({
    queryKey: ["mediaSources"],
    queryFn: () => client.mediaSources(),
    enabled: active === true,
  });
  // Clamp a stale persisted source to what THIS backup actually has, so a filter
  // carried over from another backup can't leave the grid stuck empty (its chip
  // may be hidden, leaving no way to reset).
  const source =
    sourcePref !== "all" && (sources ?? []).some(([s]) => s === sourcePref)
      ? sourcePref
      : "all";
  const sourceArg = source === "all" ? null : source;
  // Time filter — same presets + custom range as the Timeline.
  const { now, presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Free-text search over the filename (debounced).
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  const { data: count, error } = useQuery({
    queryKey: ["mediaCount", source, range.lo, range.hi, search],
    queryFn: () => client.countMedia(sourceArg, range.lo, range.hi, search),
    enabled: active === true,
  });
  // Per-preset counts for the time chips, within the current source + search.
  const { data: presetCounts } = useQuery({
    queryKey: ["mediaRanges", now, source, search],
    queryFn: () =>
      client.countMediaRanges(
        sourceArg,
        presets.map((p) => ({ lo: p.lo, hi: p.hi })),
        search,
      ),
    enabled: active === true,
  });
  const [openIndex, setOpenIndex] = useState<number | null>(null);
  const [sort, setSort] = usePersistedState<SortState>("photos:sort", { by: "date", desc: true });
  const qc = useQueryClient();

  const ensurePage = useCallback(
    (page: number) => {
      void qc.prefetchQuery({
        queryKey: [
          "mediaWindow",
          sourceArg,
          range.lo,
          range.hi,
          search,
          sort.by,
          sort.desc,
          page,
        ],
        queryFn: () =>
          client.getMediaWindow(
            sourceArg,
            range.lo,
            range.hi,
            search,
            page * PAGE,
            PAGE,
            sort.by,
            sort.desc,
          ),
      });
    },
    [qc, sourceArg, range, search, sort],
  );

  const hasFilter = (sources?.length ?? 0) > 1;
  const total = sources?.reduce((sum, [, c]) => sum + c, 0) ?? 0;
  const sourceOptions = useMemo(
    () => [
      { value: "all", label: "All", count: total },
      ...(sources ?? []).map(([name, c]) => {
        const slug = serviceSlug(name);
        return {
          value: name,
          label: sourceLabel(name),
          count: c,
          icon: hasBrandIcon(slug) ? (
            <BrandIcon slug={slug} name={name} className="size-3.5" />
          ) : undefined,
        };
      }),
    ],
    [sources, total],
  );
  const filterGroups = useMemo<FilterGroup[]>(() => {
    const list: FilterGroup[] = [];
    if (hasFilter)
      list.push(
        badgeGroup({
          key: "source",
          label: "Source",
          description: "Which app or album the media came from",
          options: sourceOptions,
          value: source,
          onChange: setSource,
        }),
      );
    list.push(timeGroup({ description: "When the media was created", presets, counts: presetCounts, value: range, onChange: setRange }));
    return list;
  }, [hasFilter, sourceOptions, source, setSource, presets, presetCounts, range]);
  const sortNode = useMemo(
    () => (
      <SortControl
        fields={[
          { value: "date", label: "Date" },
          { value: "source", label: "Source" },
        ]}
        value={sort}
        onChange={setSort}
      />
    ),
    [sort, setSort],
  );
  const searchNode = useMemo(
    () => (
      <ListSearch
        value={q}
        onChange={setQ}
        placeholder="Search filename, person, place, or album (e.g. Florida)"
      />
    ),
    [q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Photos", count, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, count, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

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

  return (
    <div className="flex h-full flex-col">
      {error ? (
        <ErrorState error={error} />
      ) : count === undefined ? (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-1 p-1">
          {Array.from({ length: 12 }).map((_, i) => (
            <Skeleton key={i} className="aspect-square" />
          ))}
        </div>
      ) : count === 0 ? (
        <EmptyView
          icon={Images}
          title={
            source === "all"
              ? "No photos or videos in this backup."
              : "No media from this source."
          }
        />
      ) : (
        // key by source+range+search+sort so the grid remounts (scroll +
        // measurement reset) on any filter change.
        <MediaGrid
          key={`${source}:${range.lo}:${range.hi}:${search}:${sort.by}:${sort.desc}`}
          count={count}
          source={sourceArg}
          range={range}
          search={search}
          sort={sort}
          onOpen={setOpenIndex}
        />
      )}

      <Lightbox
        index={openIndex}
        count={count ?? 0}
        source={sourceArg}
        range={range}
        search={search}
        sort={sort}
        ensurePage={ensurePage}
        onNavigate={setOpenIndex}
        onClose={() => setOpenIndex(null)}
      />
    </div>
  );
}

/** Shorten noisy media-source names for display (the filter value stays raw). */
function sourceLabel(name: string): string {
  return name.startsWith("iTunes Backup") ? "iTunes Backup" : name;
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
  range,
  search,
  sort,
  onOpen,
}: {
  count: number;
  source: string | null;
  range: TimeRange;
  search: string | null;
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
      queryKey: [
        "mediaWindow",
        source,
        range.lo,
        range.hi,
        search,
        sort.by,
        sort.desc,
        p,
      ],
      queryFn: () =>
        client.getMediaWindow(
          source,
          range.lo,
          range.hi,
          search,
          p * PAGE,
          PAGE,
          sort.by,
          sort.desc,
        ),
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

/** Human-readable byte size, e.g. "2.0 MB". */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

/** The photo's location as a clickable Apple Maps link — the moment place name
 *  when known, else the coordinates. */
function LocationTag({ item }: { item: MediaItem }) {
  const hasCoords = item.latitude != null && item.longitude != null;
  const label =
    item.location ??
    (hasCoords
      ? `${item.latitude!.toFixed(4)}, ${item.longitude!.toFixed(4)}`
      : null);
  if (!label) return null;
  const url = hasCoords
    ? `https://maps.apple.com/?ll=${item.latitude},${item.longitude}${
        item.location ? `&q=${encodeURIComponent(item.location)}` : ""
      }`
    : `https://maps.apple.com/?q=${encodeURIComponent(item.location!)}`;
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        void client.openExternal(url);
      }}
      className="inline-flex items-center gap-1 hover:text-white hover:underline"
      title="Open in Maps"
    >
      <MapPin className="size-3.5" />
      <span className="max-w-[12rem] truncate">{label}</span>
    </button>
  );
}

function Thumb({ item, onOpen }: { item: MediaItem; onOpen: () => void }) {
  const isVideo = item.kind === "video";
  const cacheKey = useMediaCacheKey();
  return (
    <button
      onClick={onOpen}
      aria-label={item.filename ?? (isVideo ? "Open video" : "Open photo")}
      className="group relative aspect-square w-full overflow-hidden rounded-sm bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <img
        src={client.mediaUrl(item.id, { thumb: true, cacheKey })}
        alt={item.filename ?? ""}
        className="size-full object-cover transition-transform group-hover:scale-105"
      />
      {item.source && (
        <span className="absolute bottom-1 left-1 rounded bg-black/55 px-1.5 py-0.5 text-[10px] font-medium text-white opacity-0 transition-opacity group-hover:opacity-100">
          {item.source}
        </span>
      )}
      <div className="absolute right-1 top-1 flex gap-1">
        {item.hidden && (
          <span
            className="rounded-full bg-black/55 p-1 text-white"
            title="In the Hidden album"
          >
            <EyeOff className="size-3" />
          </span>
        )}
        {item.favorite && (
          <span className="rounded-full bg-black/55 p-1" title="Favorite">
            <Heart className="size-3 fill-red-500 text-red-500" />
          </span>
        )}
        {item.persons && (
          <span
            className="rounded-full bg-black/55 p-1 text-white"
            title={item.persons}
          >
            <Users className="size-3" />
          </span>
        )}
        {item.subtype && (
          <span
            className="rounded-full bg-black/55 p-1 text-white"
            title={item.subtype}
          >
            {item.subtype === "panorama" ? (
              <Frame className="size-3" />
            ) : (
              <Smartphone className="size-3" />
            )}
          </span>
        )}
      </div>
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
  range,
  search,
  sort,
  ensurePage,
  onNavigate,
  onClose,
}: {
  index: number | null;
  count: number;
  source: string | null;
  range: TimeRange;
  search: string | null;
  sort: SortState;
  ensurePage: (page: number) => void;
  onNavigate: (index: number) => void;
  onClose: () => void;
}) {
  const open = index != null;
  const { lightboxStyle, showMediaMetadata } = useSettings();
  const cacheKey = useMediaCacheKey();
  // Subscribe to the current item's window (same key the grid fills) so the view
  // re-renders when a not-yet-loaded window resolves — a non-reactive cache read
  // would leave the spinner stuck until the next interaction.
  const page = index != null ? Math.floor(index / PAGE) : 0;
  const { data: win } = useQuery({
    queryKey: [
      "mediaWindow",
      source,
      range.lo,
      range.hi,
      search,
      sort.by,
      sort.desc,
      page,
    ],
    queryFn: () =>
      client.getMediaWindow(
        source,
        range.lo,
        range.hi,
        search,
        page * PAGE,
        PAGE,
        sort.by,
        sort.desc,
      ),
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

  const isVideo = item?.kind === "video";
  const meta =
    item && showMediaMetadata ? (
      <div className="space-y-1">
        <div className="flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-3">
            {item.hidden && (
              <EyeOff className="size-3.5 shrink-0" aria-label="In the Hidden album" />
            )}
            {item.favorite && (
              <Heart className="size-3.5 shrink-0 fill-red-500 text-red-500" />
            )}
            <span className="select-text truncate">{item.filename ?? "—"}</span>
            {item.persons && (
              <span
                className="inline-flex min-w-0 shrink items-center gap-1 text-neutral-400"
                title={item.persons}
              >
                <Users className="size-3.5 shrink-0" />
                <span className="select-text truncate">{item.persons}</span>
              </span>
            )}
            {item.albums && (
              <span
                className="inline-flex min-w-0 shrink items-center gap-1 text-neutral-400"
                title={`Albums: ${item.albums}`}
              >
                <Images className="size-3.5 shrink-0" />
                <span className="select-text truncate">{item.albums}</span>
              </span>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-3">
            <LocationTag item={item} />
            {index != null && (
              <span className="tabular-nums">
                {formatCount(index + 1)} / {formatCount(count)}
              </span>
            )}
            {item.source && <span>{item.source}</span>}
            {item.takenAt && <span>{formatDateTime(item.takenAt)}</span>}
          </div>
        </div>
        {(item.camera || item.lens || item.exif || item.width || item.fileSize) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-neutral-400">
            {item.camera && (
              <span className="inline-flex items-center gap-1">
                <Camera className="size-3 shrink-0" />
                <span className="select-text">{item.camera}</span>
              </span>
            )}
            {item.lens && <span className="select-text">{item.lens}</span>}
            {item.exif && <span className="select-text">{item.exif}</span>}
            {item.width && item.height && (
              <span className="tabular-nums">
                {item.width} × {item.height}
              </span>
            )}
            {item.fileSize && (
              <span className="tabular-nums">{formatBytes(item.fileSize)}</span>
            )}
          </div>
        )}
      </div>
    ) : undefined;

  return (
    <MediaLightbox
      open={open}
      onClose={onClose}
      style={lightboxStyle}
      title={item?.filename ?? "Media"}
      hasPrev={hasPrev}
      hasNext={hasNext}
      onPrev={() => go(-1)}
      onNext={() => go(1)}
      media={
        item ? (
          isVideo ? (
            <video
              key={item.id}
              src={client.mediaUrl(item.id, { cacheKey })}
              // iOS's pre-rendered thumbnail as the poster, so a still shows
              // instantly (and if autoplay is blocked, it isn't a black frame).
              poster={client.mediaUrl(item.id, { thumb: true, cacheKey })}
              controls
              autoPlay
              className="max-h-full max-w-full object-contain"
            />
          ) : (
            <img
              key={item.id}
              src={client.mediaUrl(item.id, { cacheKey })}
              alt={item.filename ?? ""}
              className="max-h-full max-w-full object-contain"
            />
          )
        ) : (
          <div className="size-16 animate-pulse rounded-full bg-white/20" />
        )
      }
      meta={meta}
    />
  );
}
