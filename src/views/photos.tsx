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
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Camera,
  CircleDot,
  Copy,
  EyeOff,
  Frame,
  Heart,
  Import,
  Image as ImageIcon,
  ImageOff,
  Images,
  MessageSquare,
  MapPin,
  Play,
  Smartphone,
  Trash2,
  Users,
} from "lucide-react";

/** Media items fetched per lazy window (shared by the grid and the lightbox's
 *  neighbour lookup so their cache keys line up). */
const PAGE = 100;

/** Human labels for the media-subtype badge tooltip. */
const SUBTYPE_LABELS: Record<string, string> = {
  screenshot: "Screenshot",
  panorama: "Panorama",
  live: "Live Photo",
  burst: "Burst",
};
import { emptyListMessage, isTimeFiltered } from "@/lib/empty-message";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { MediaLightbox } from "@/components/media-lightbox";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { makeYearPresets, useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState, EmptyView, ErrorState, ListSearch } from "@/components/view";
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
  // Time filter — recency windows, then ONE CHIP PER YEAR the library actually
  // spans (from the capture-date bounds), not just the current calendar year.
  // `makeTimePresets` alone only ever offers "this year", which is why the
  // filter looked stuck on 2026 no matter how old the photos were.
  const { now, presets: basePresets } = useTimePresets();
  const { data: dateBounds } = useQuery({
    queryKey: ["mediaDateBounds"],
    queryFn: () => client.mediaDateBounds(),
    enabled: active === true,
  });
  const presets = useMemo(() => {
    if (!dateBounds) return basePresets;
    const minYear = new Date(dateBounds[0] * 1000).getFullYear();
    const maxYear = new Date(dateBounds[1] * 1000).getFullYear();
    return [
      ...basePresets.filter((p) => p.key !== "year"),
      ...makeYearPresets(minYear, maxYear),
    ];
  }, [basePresets, dateBounds]);
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
    // `presets.length` keys the year chips: when the date bounds resolve and the
    // per-year presets appear, the counts must refetch to cover them.
    queryKey: ["mediaRanges", now, source, search, presets.length],
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
      <NoBackupState
        icon={ImageIcon}
        title="Browse photos & videos"
        lead="Every photo and video from the iPhone's Camera Roll, restored from the backup and viewable full-size — Live Photos, screenshots, panoramas, and bursts included."
        features={[
          { label: "Search", detail: "Find shots by filename, person, place, or album." },
          { label: "Filter & time range", detail: "Narrow by source album and jump to any date range, with live counts." },
          { label: "Sort", detail: "Order by date or source, ascending or descending." },
          { label: "Full detail", detail: "Open a lightbox with EXIF, camera settings, people, and a map pin for where it was taken." },
        ]}
        note="Everything is read locally on this Mac — nothing is uploaded, and the backup is never modified."
      />
    );
  }

  return (
    <div className="flex h-full flex-col">
      {error ? (
        <ErrorState error={error} />
      ) : count === undefined ? (
        <div
          data-underlap=""
          className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-1 p-1"
        >
          {Array.from({ length: 12 }).map((_, i) => (
            <Skeleton key={i} className="aspect-square" />
          ))}
        </div>
      ) : count === 0 ? (
        <EmptyView
          icon={Images}
          title={emptyListMessage(
            {
              search,
              timeFiltered: isTimeFiltered(range),
              otherFiltered: source !== "all",
            },
            "No photos or videos in this backup.",
            "photos or videos",
          )}
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
    <div
      ref={scrollRef}
      data-underlap=""
      className="min-h-0 flex-1 overflow-auto p-1 [scrollbar-gutter:stable]"
    >
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
/** The house Tooltip triple. Three lightbox chips used a native `title=`,
 *  which check-design.mjs forbids -- and never caught here only because the
 *  lint never opens a photo. */
function Hint({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent className="max-w-72">{label}</TooltipContent>
    </Tooltip>
  );
}

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
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            void client.openExternal(url);
          }}
          className="inline-flex items-center gap-1 hover:text-white hover:underline"
        >
          <MapPin className="size-3.5" />
          <span className="max-w-[12rem] truncate">{label}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent>Open in Maps</TooltipContent>
    </Tooltip>
  );
}

function Thumb({ item, onOpen }: { item: MediaItem; onOpen: () => void }) {
  const isVideo = item.kind === "video";
  const cacheKey = useMediaCacheKey();
  const [failed, setFailed] = useState(false);
  return (
    <button
      onClick={onOpen}
      aria-label={item.filename ?? (isVideo ? "Open video" : "Open photo")}
      className="group relative aspect-square w-full overflow-hidden rounded-sm bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      {failed ? (
        // The media route 404s when the file cannot be produced -- most often
        // an encrypted backup whose session decryptor is gone after a cancelled
        // Touch ID. A bare broken <img> is a silent blank tile that reads as
        // "this photo is missing from the backup", which is a different and
        // false claim.
        <span className="flex size-full flex-col items-center justify-center gap-1 text-muted-foreground">
          <ImageOff className="size-5" />
          <span className="px-1 text-3xs">Unavailable</span>
        </span>
      ) : (
        <img
          src={client.mediaUrl(item.id, { thumb: true, cacheKey })}
          alt={item.filename ?? ""}
          onError={() => setFailed(true)}
          // Decode OFF the main thread so a fast scroll doesn't stall on each
          // tile's decode competing with virtualizer measurement, and only
          // fetch/decode tiles near the viewport (`lazy`) rather than every
          // overscan row at once — the two levers behind "scrolling gets
          // sluggish the longer you browse."
          decoding="async"
          loading="lazy"
          // Not `scale-105`: Tailwind v4 emits the standalone `scale` property
          // and WKWebView will not animate it, so the zoom snapped. The
          // `transform` shorthand does animate. See docs/reference/ui.md.
          className="size-full object-cover transition-transform [transform:scale(1)] group-hover:[transform:scale(1.05)]"
        />
      )}
      {item.source && (
        <span className="absolute bottom-1 left-1 rounded bg-black/55 px-1.5 py-0.5 text-3xs font-medium text-white opacity-0 transition-opacity group-hover:opacity-100">
          {item.source}
        </span>
      )}
      <div className="absolute right-1 top-1 flex gap-1">
        {item.trashed && (
          <span
            className="rounded-full bg-status-danger/80 p-1 text-white"
            title="In Recently Deleted"
          >
            <Trash2 className="size-3" />
          </span>
        )}
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
            <Heart className="size-3 fill-status-danger text-status-danger-text" />
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
            title={SUBTYPE_LABELS[item.subtype] ?? item.subtype}
          >
            {item.subtype === "panorama" ? (
              <Frame className="size-3" />
            ) : item.subtype === "live" ? (
              <CircleDot className="size-3" />
            ) : item.subtype === "burst" ? (
              <Copy className="size-3" />
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
  // Cleared on every navigation: the previous file failing says nothing about
  // the next one.
  const [fullFailed, setFullFailed] = useState(false);
  // Retry counter for the current image. Paging faster than a full-res load
  // finishes makes WebKit cancel the scheme task and cache the URL as *failed*,
  // so landing back on it shows black. We retry a couple of times with a
  // cache-busting `r=` so the handler re-runs, and only then give up.
  const [retry, setRetry] = useState(0);
  const MAX_RETRIES = 2;
  useEffect(() => {
    setFullFailed(false);
    setRetry(0);
  }, [index, cacheKey]);
  const onImageError = () => {
    if (retry < MAX_RETRIES) setRetry((r) => r + 1);
    else setFullFailed(true);
  };
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
            {item.trashed && (
              <Trash2
                className="size-3.5 shrink-0 text-status-danger-text"
                aria-label="In Recently Deleted"
              />
            )}
            {item.hidden && (
              <EyeOff className="size-3.5 shrink-0" aria-label="In the Hidden album" />
            )}
            {item.favorite && (
              <Heart className="size-3.5 shrink-0 fill-status-danger text-status-danger-text" />
            )}
            <span className="select-text truncate">{item.filename ?? "—"}</span>
            {item.persons && (
              <Hint label={item.persons}>
                <span className="inline-flex min-w-0 shrink items-center gap-1 text-neutral-400">
                  <Users className="size-3.5 shrink-0" />
                  <span className="select-text truncate">{item.persons}</span>
                </span>
              </Hint>
            )}
            {(item.sharedCaption || item.sharedLikes) && (
              // Activity by OTHER PEOPLE on a photo this device shared with
              // them — a different kind of fact from anything else on this bar,
              // which is all about the photo itself.
              <Hint
                label={
                  item.sharedCaption
                    ? `Shared album caption: “${item.sharedCaption}”${
                        item.sharedLikes ? ` · liked ${item.sharedLikes}×` : ""
                      }`
                    : `Liked ${item.sharedLikes}× in a shared album`
                }
              >
                <span className="inline-flex min-w-0 shrink items-center gap-1 text-neutral-400">
                  <MessageSquare className="size-3.5 shrink-0" />
                  <span className="select-text truncate">
                    {item.sharedCaption ?? `${item.sharedLikes} likes`}
                  </span>
                </span>
              </Hint>
            )}
            {item.albums && (
              <Hint label={`Albums: ${item.albums}`}>
                <span className="inline-flex min-w-0 shrink items-center gap-1 text-neutral-400">
                  <Images className="size-3.5 shrink-0" />
                  <span className="select-text truncate">{item.albums}</span>
                </span>
              </Hint>
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
            {item.addedAt != null &&
              (item.takenAt == null ||
                Math.abs(item.addedAt - item.takenAt) > 86400) && (
                <Hint label="Added to this device's library later than it was captured — likely received, saved, or imported">
                  <span className="inline-flex items-center gap-1 text-status-warning-text">
                    <Import className="size-3 shrink-0" />
                    Added {formatDateTime(item.addedAt)}
                  </span>
                </Hint>
              )}
          </div>
        </div>
        {(item.camera || item.lens || item.exif || item.width || item.fileSize) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-2xs text-neutral-400">
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
        item && fullFailed ? (
          // Same rule as the tile: a black modal with nothing in it is not an
          // answer. Decryption is the usual cause, so name it.
          <div className="flex flex-col items-center gap-2 px-8 text-center text-white/70">
            <ImageOff className="size-8" />
            <p className="text-sm">
              This file could not be read from the backup.
            </p>
            <p className="text-xs text-white/50">
              If the backup is encrypted, unlock it and try again.
            </p>
          </div>
        ) : item ? (
          isVideo ? (
            <video
              key={item.id}
              src={client.mediaUrl(item.id, { cacheKey })}
              onError={() => setFullFailed(true)}
              // iOS's pre-rendered thumbnail as the poster, so a still shows
              // instantly (and if autoplay is blocked, it isn't a black frame).
              poster={client.mediaUrl(item.id, { thumb: true, cacheKey })}
              controls
              autoPlay
              className="max-h-full max-w-full object-contain"
            />
          ) : (
            <img
              // `retry` is in the key so a retry actually remounts the element
              // with the cache-busting URL rather than reusing the failed one.
              key={`${item.id}:${retry}`}
              // A downscaled preview, not the 12-megapixel original: it loads in
              // a fraction of the time, which is the real fix for black frames
              // (fewer cancelled loads) as well as being much lighter on memory.
              src={client.mediaUrl(item.id, { preview: true, cacheKey, retry })}
              alt={item.filename ?? ""}
              onError={onImageError}
              // Decode off the main thread so paging next/prev doesn't block the
              // UI thread on each decode.
              decoding="async"
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
