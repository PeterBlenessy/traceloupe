import {
  forwardRef,
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
  Download,
  Eye,
  EyeOff,
  Cloud,
  FolderOpen,
  Frame,
  Heart,
  ShieldAlert,
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
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";

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
import { emptyListMessage } from "@/lib/empty-message";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { MediaLightbox } from "@/components/media-lightbox";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { makeYearPresets, useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { multiBadgeGroup, multiTimeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState, EmptyView, ErrorState, ListSearch } from "@/components/view";
import { useDebounced } from "@/lib/use-debounced";
import { formatCount, formatDateTimeYear } from "@/lib/format";
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
  const [sourcesPref, setSourcesPref] = usePersistedState<string[]>(
    "photos:sources",
    [],
  );
  const { data: sources } = useQuery({
    queryKey: ["mediaSources"],
    queryFn: () => client.mediaSources(),
    enabled: active === true,
  });
  // Clamp stale persisted sources to what THIS backup actually has, so a filter
  // carried over from another backup can't leave the grid stuck empty. MULTI-
  // select: an empty array means "all".
  const sourcesSel = useMemo(
    () => sourcesPref.filter((s) => (sources ?? []).some(([n]) => n === s)),
    [sourcesPref, sources],
  );
  const toggleSource = useCallback(
    (v: string) =>
      setSourcesPref((prev) =>
        prev.includes(v) ? prev.filter((x) => x !== v) : [...prev, v],
      ),
    [setSourcesPref],
  );
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
  // MULTI-select time: several ranges (e.g. two years) unioned; [] = all time.
  const [ranges, setRanges] = useState<TimeRange[]>([]);
  const toggleRange = useCallback(
    (r: TimeRange) =>
      setRanges((prev) =>
        prev.some((x) => x.lo === r.lo && x.hi === r.hi)
          ? prev.filter((x) => !(x.lo === r.lo && x.hi === r.hi))
          : [...prev, r],
      ),
    [],
  );
  // Show only the photos/videos the user has marked as unsafe.
  const [unsafeOnly, setUnsafeOnly] = useState(false);
  // Show only what was hidden ON THE DEVICE (Photos' Hidden album). Hiding is a
  // flag on the asset, not a move — the files stay in DCIM — so these have
  // always been in the gallery, just indistinguishable without a filter.
  const [hiddenOnly, setHiddenOnly] = useState(false);
  // Multi-select, like every other facet: "original" and "thumbnail" are not
  // opposites you toggle between, they are two populations you may want either
  // or both of.
  const [availability, setAvailability] = useState<string[]>([]);
  const toggleAvailability = (k: string) =>
    setAvailability((prev) =>
      prev.includes(k) ? prev.filter((v) => v !== k) : [...prev, k],
    );
  const { data: availabilityFacets } = useQuery({
    queryKey: ["mediaAvailability"],
    queryFn: () => client.mediaAvailability(),
  });
  // Free-text search over the filename (debounced).
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  // Stable primitive keys for the multi-select arrays, so React Query refetches
  // when the SELECTION changes but not on every render's new array identity.
  const sourcesKey = sourcesSel.join(" ");
  const rangesKey = ranges.map((r) => `${r.lo}-${r.hi}`).join(" ");
  const { data: count, error } = useQuery({
    queryKey: [
      "mediaCount",
      sourcesKey,
      rangesKey,
      search,
      unsafeOnly,
      hiddenOnly,
      availability.join(","),
    ],
    queryFn: () =>
      client.countMedia(
        sourcesSel,
        ranges,
        search,
        unsafeOnly,
        hiddenOnly,
        availability,
      ),
    enabled: active === true,
  });
  // How many are marked unsafe, for the Unsafe pill's count (and whether to show it).
  const { data: favCount } = useQuery({
    queryKey: ["mediaFavCount"],
    queryFn: () => client.countMedia([], [], null, true),
    enabled: active === true,
  });
  // How many the device hid, for the Hidden pill's count (and whether to show it).
  const { data: hiddenCount } = useQuery({
    queryKey: ["mediaHiddenCount"],
    queryFn: () => client.countMedia([], [], null, false, true),
    enabled: active === true,
  });
  // Per-preset counts for the time chips, within the current sources + search.
  const { data: presetCounts } = useQuery({
    // `presets.length` keys the year chips: when the date bounds resolve and the
    // per-year presets appear, the counts must refetch to cover them.
    queryKey: [
      "mediaRanges",
      now,
      sourcesKey,
      search,
      presets.length,
      unsafeOnly,
      hiddenOnly,
      availability.join(","),
    ],
    queryFn: () =>
      client.countMediaRanges(
        sourcesSel,
        presets.map((p) => ({ lo: p.lo, hi: p.hi })),
        search,
        unsafeOnly,
        hiddenOnly,
        availability,
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
          sourcesKey,
          rangesKey,
          search,
          sort.by,
          sort.desc,
          unsafeOnly,
          hiddenOnly,
          availability.join(","),
          page,
        ],
        queryFn: () =>
          client.getMediaWindow(
            sourcesSel,
            ranges,
            search,
            page * PAGE,
            PAGE,
            sort.by,
            sort.desc,
            unsafeOnly,
            hiddenOnly,
            availability,
          ),
      });
    },
    [
      qc,
      sourcesSel,
      ranges,
      sourcesKey,
      rangesKey,
      search,
      sort,
      unsafeOnly,
      hiddenOnly,
    ],
  );

  // Toggle the unsafe mark, persist it, and refresh the media queries so the
  // grid, the counts, and the Unsafe pill all reflect it.
  const markUnsafe = useCallback(
    async (item: MediaItem) => {
      await client.setMediaFavorite(item.id, !item.userFavorite);
      void qc.invalidateQueries({ queryKey: ["mediaWindow"] });
      void qc.invalidateQueries({ queryKey: ["mediaCount"] });
      void qc.invalidateQueries({ queryKey: ["mediaRanges"] });
      void qc.invalidateQueries({ queryKey: ["mediaFavCount"] });
      void qc.invalidateQueries({ queryKey: ["mediaHiddenCount"] });
    },
    [qc],
  );

  const hasFilter = (sources?.length ?? 0) > 1;
  // No "All" option: multi-select treats an empty selection as "all".
  const sourceOptions = useMemo(
    () =>
      (sources ?? []).map(([name, c]) => {
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
    [sources],
  );
  const filterGroups = useMemo<FilterGroup[]>(() => {
    const list: FilterGroup[] = [];
    if (hasFilter)
      list.push(
        multiBadgeGroup({
          key: "source",
          label: "Source",
          description: "Which apps or albums the media came from",
          options: sourceOptions,
          selected: sourcesSel,
          onToggle: toggleSource,
        }),
      );
    // A single toggle pill: "Unsafe". Shown once anything is marked (or while
    // the filter is on), so it never appears as an empty "Unsafe (0)".
    if (unsafeOnly || (favCount ?? 0) > 0)
      list.push({
        key: "unsafe",
        label: "Unsafe",
        description: "Photos and videos you've marked as unsafe",
        pills: [
          {
            key: "unsafe",
            label: "Unsafe",
            icon: <ShieldAlert className="size-3.5" />,
            count: favCount,
            selected: unsafeOnly,
            onSelect: () => setUnsafeOnly((v) => !v),
          },
        ],
        summary: unsafeOnly
          ? [
              {
                key: "unsafe",
                label: "Unsafe",
                icon: <ShieldAlert className="size-3.5" />,
                onClear: () => setUnsafeOnly(false),
              },
            ]
          : [],
      });
    // "Hidden" — what the DEVICE hid (Photos' Hidden album), as distinct from
    // the user's own "Unsafe" mark. Shown only when the backup actually has
    // some, so it never reads as an empty "Hidden (0)".
    if (hiddenOnly || (hiddenCount ?? 0) > 0)
      list.push({
        key: "hidden",
        label: "Hidden",
        description: "Photos and videos hidden on the device",
        pills: [
          {
            key: "hidden",
            label: "Hidden",
            icon: <EyeOff className="size-3.5" />,
            count: hiddenCount,
            selected: hiddenOnly,
            onSelect: () => setHiddenOnly((v) => !v),
          },
        ],
        summary: hiddenOnly
          ? [
              {
                key: "hidden",
                label: "Hidden",
                icon: <EyeOff className="size-3.5" />,
                onClear: () => setHiddenOnly(false),
              },
            ]
          : [],
      });
    // Only offer this when the backup actually holds both kinds. A backup taken
    // without iCloud Photos is all originals, and a facet whose every option
    // selects everything is noise.
    const availOptions = (availabilityFacets ?? [])
      .filter(([k]) => k === "original" || k === "thumbnail")
      .map(([k, n]) => ({
        value: k,
        label: k === "original" ? "In this backup" : "iCloud only",
        count: n,
      }));
    if (availOptions.length > 1 || availability.length > 0)
      list.push(
        multiBadgeGroup({
          key: "availability",
          label: "Full resolution",
          description:
            "Whether the full-size original is in this backup. iCloud-only photos are shown from the thumbnail iOS backed up.",
          options: availOptions,
          selected: availability,
          onToggle: toggleAvailability,
        }),
      );
    list.push(
      multiTimeGroup({
        description: "When the media was created",
        presets,
        counts: presetCounts,
        values: ranges,
        onToggle: toggleRange,
        onClear: () => setRanges([]),
      }),
    );
    return list;
  }, [hasFilter, sourceOptions, sourcesSel, toggleSource, presets, presetCounts, ranges, toggleRange, unsafeOnly, favCount, hiddenOnly, hiddenCount, availability, availabilityFacets, toggleAvailability]);
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
              timeFiltered: ranges.length > 0,
              otherFiltered: sourcesSel.length > 0,
            },
            "No photos or videos in this backup.",
            "photos or videos",
          )}
        />
      ) : (
        // key by sources+ranges+search+sort so the grid remounts (scroll +
        // measurement reset) on any filter change.
        <MediaGrid
          key={`${sourcesKey}:${rangesKey}:${search}:${sort.by}:${sort.desc}:${unsafeOnly}:${hiddenOnly}:${availability.join(",")}`}
          count={count}
          sources={sourcesSel}
          ranges={ranges}
          search={search}
          sort={sort}
          availability={availability}
          unsafeOnly={unsafeOnly}
          hiddenOnly={hiddenOnly}
          onOpen={setOpenIndex}
          onMarkUnsafe={markUnsafe}
        />
      )}

      <Lightbox
        index={openIndex}
        count={count ?? 0}
        sources={sourcesSel}
        ranges={ranges}
        search={search}
        sort={sort}
        unsafeOnly={unsafeOnly}
        hiddenOnly={hiddenOnly}
        availability={availability}
        onMarkUnsafe={markUnsafe}
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
  sources,
  ranges,
  search,
  sort,
  unsafeOnly,
  hiddenOnly,
  availability,
  onOpen,
  onMarkUnsafe,
}: {
  count: number;
  sources: string[];
  ranges: TimeRange[];
  search: string | null;
  sort: SortState;
  unsafeOnly: boolean;
  hiddenOnly: boolean;
  availability: string[];
  onOpen: (index: number) => void;
  onMarkUnsafe: (item: MediaItem) => void;
}) {
  // Same primitive keys the parent uses, so windows prefetched there hit here.
  const sourcesKey = sources.join(" ");
  const rangesKey = ranges.map((r) => `${r.lo}-${r.hi}`).join(" ");
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
        sourcesKey,
        rangesKey,
        search,
        sort.by,
        sort.desc,
        unsafeOnly,
        hiddenOnly,
        availability.join(","),
        p,
      ],
      queryFn: () =>
        client.getMediaWindow(
          sources,
          ranges,
          search,
          p * PAGE,
          PAGE,
          sort.by,
          sort.desc,
          unsafeOnly,
          hiddenOnly,
          availability,
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
                      <MediaMenu item={item} onOpen={() => onOpen(index)} onMarkUnsafe={onMarkUnsafe}>
                        <Thumb
                          item={item}
                          onOpen={() => onOpen(index)}
                          onMarkUnsafe={onMarkUnsafe}
                        />
                      </MediaMenu>
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

/** Right-click actions for a photo/video, shared by the grid tile and the
 *  lightbox. The native WebKit menu can't fetch our custom `traceloupe-media://`
 *  scheme, so its "Save Image"/"Copy" silently fail — this replaces it with
 *  actions wired to real Tauri commands. */
function MediaMenu({
  item,
  onOpen,
  onMarkUnsafe,
  children,
}: {
  item: MediaItem;
  onOpen?: () => void;
  onMarkUnsafe: (item: MediaItem) => void;
  children: React.ReactNode;
}) {
  const isVideo = item.kind === "video";
  const base = item.filename ?? `item-${item.id}`;
  const jpegName = `${base.replace(/\.[^.]+$/, "")}.jpg`;
  const ext = base.includes(".") ? base.split(".").pop()!.toUpperCase() : "original";
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        {onOpen && (
          <ContextMenuItem onSelect={onOpen}>
            <Eye /> Open preview
          </ContextMenuItem>
        )}
        <ContextMenuItem onSelect={() => onMarkUnsafe(item)}>
          <ShieldAlert className={item.userFavorite ? "text-amber-400" : undefined} />
          {item.userFavorite ? "Clear unsafe" : "Mark unsafe"}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <Download /> Download…
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem
              onSelect={() => void client.saveMedia(item.id, base, false)}
            >
              Save original ({ext})
            </ContextMenuItem>
            {!isVideo && (
              <ContextMenuItem
                onSelect={() => void client.saveMedia(item.id, jpegName, true)}
              >
                Save as JPEG
              </ContextMenuItem>
            )}
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => void client.revealMedia(item.id)}>
          <FolderOpen /> Reveal in Finder
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

const Thumb = forwardRef<
  HTMLButtonElement,
  {
    item: MediaItem;
    onOpen: () => void;
    onMarkUnsafe: (item: MediaItem) => void;
  } & React.ButtonHTMLAttributes<HTMLButtonElement>
>(function Thumb({ item, onOpen, onMarkUnsafe, ...rest }, ref) {
  const isVideo = item.kind === "video";
  const cacheKey = useMediaCacheKey();
  const [failed, setFailed] = useState(false);
  return (
    <button
      ref={ref}
      onClick={onOpen}
      aria-label={item.filename ?? (isVideo ? "Open video" : "Open photo")}
      className="group relative aspect-square w-full overflow-hidden rounded-sm bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      {...rest}
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
          // NO `decoding="async"` — it is what made the tiles BLINK.
          //
          // Async decoding lets WebKit paint an element whose decode is not
          // ready, i.e. paint the tile with NO IMAGE and fill it in later. When
          // hovering re-rasterizes the scroll layer, every tile whose decoded
          // bitmap has to be regenerated shows that empty frame first — the
          // expensive decodes worst, which is why it read as "HEIC only". No
          // re-fetch and no React remount is involved (both were measured: the
          // <img> nodes are identical across a hover and fire no `load`), which
          // is why the earlier caching/remount explanations did not fit.
          //
          // Synchronous decoding (the default) paints the image or waits — it
          // never paints a hole. The decode cost that motivated `async` is much
          // lower now anyway: the handler serves a cached JPEG without touching
          // (or decrypting) the original.
          //
          // NO hover zoom and NO `loading="lazy"` here either, on purpose. A
          // `transform: scale` on hover makes WKWebView re-rasterize the scroll
          // layer, and `lazy` then re-fetches the neighbouring no-cache
          // custom-scheme URLs during that repaint — so hovering one tile made
          // every tile below it blink. `lazy` also buys little next to the
          // virtualizer, which already only mounts near-viewport rows.
          className="size-full object-cover"
        />
      )}
      {/* NO `transition-opacity` on the hover-revealed overlays — see the note
          on the mark below. An ANIMATED opacity is what promotes a tile to its
          own compositing layer and blanks everything painted after it. */}
      {item.source && (
        <span className="absolute bottom-1 left-1 rounded bg-black/55 px-1.5 py-0.5 text-3xs font-medium text-white opacity-0 group-hover:opacity-100">
          {item.source}
        </span>
      )}
      <div className="absolute right-1 top-1 flex gap-1">
        {/* The user's "unsafe" mark: interactive (nested `role="button"`, not a
            <button>, since the tile itself is a button). Always shown when
            marked; revealed on hover otherwise so any tile can be marked. */}
        <span
          role="button"
          tabIndex={0}
          aria-label={item.userFavorite ? "Clear unsafe mark" : "Mark unsafe"}
          title={item.userFavorite ? "Marked unsafe — click to clear" : "Mark as unsafe"}
          onClick={(e) => {
            e.stopPropagation();
            onMarkUnsafe(item);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              e.stopPropagation();
              onMarkUnsafe(item);
            }
          }}
          // NO `transition-opacity`. THIS IS THE HOVER FLICKER.
          //
          // A TRANSITIONING opacity is an accelerated animation, so WebKit
          // promotes the element to its own compositing layer for the duration.
          // By overlap testing, everything painted AFTER a composited layer that
          // intersects it must be re-rasterized too — which blanks those tiles
          // for a frame. That is exactly the reported shape: the flicker hits
          // the same row and the rows BELOW, never above (paint order), and
          // which tiles are hit depends on where you hover from and to.
          //
          // Toggling opacity WITHOUT a transition is a plain repaint of this one
          // element: no promotion, no cascade. The reveal is instant, which for
          // a hover affordance is no loss.
          className={`cursor-pointer rounded-full bg-black/55 p-1 ${
            item.userFavorite ? "" : "opacity-0 group-hover:opacity-100"
          }`}
        >
          <ShieldAlert
            className={`size-3 ${
              item.userFavorite
                ? "text-amber-400"
                : "text-white"
            }`}
          />
        </span>
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
        {/* Say it on the tile. Without this an iCloud-only photo looks exactly
            like a full-resolution one until you open it and find it soft — the
            grid would be quietly overstating what this backup contains. */}
        {item.availability === "thumbnail" && (
          <span
            className="rounded-full bg-black/55 p-1 text-white"
            title="Only the thumbnail is in this backup — the full-size original is in iCloud"
          >
            <Cloud className="size-3" />
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
});

function Lightbox({
  index,
  count,
  sources,
  ranges,
  search,
  sort,
  unsafeOnly,
  hiddenOnly,
  availability,
  onMarkUnsafe,
  ensurePage,
  onNavigate,
  onClose,
}: {
  index: number | null;
  count: number;
  sources: string[];
  ranges: TimeRange[];
  search: string | null;
  sort: SortState;
  unsafeOnly: boolean;
  hiddenOnly: boolean;
  availability: string[];
  onMarkUnsafe: (item: MediaItem) => void;
  ensurePage: (page: number) => void;
  onNavigate: (index: number) => void;
  onClose: () => void;
}) {
  const open = index != null;
  const sourcesKey = sources.join(" ");
  const rangesKey = ranges.map((r) => `${r.lo}-${r.hi}`).join(" ");
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
      sourcesKey,
      rangesKey,
      search,
      sort.by,
      sort.desc,
      unsafeOnly,
      hiddenOnly,
      availability.join(","),
      page,
    ],
    queryFn: () =>
      client.getMediaWindow(
        sources,
        ranges,
        search,
        page * PAGE,
        PAGE,
        sort.by,
        sort.desc,
        unsafeOnly,
        hiddenOnly,
        availability,
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
            <Hint
              label={
                item.userFavorite ? "Marked unsafe — click to clear" : "Mark as unsafe"
              }
            >
              <button
                type="button"
                aria-label={item.userFavorite ? "Clear unsafe mark" : "Mark unsafe"}
                onClick={() => onMarkUnsafe(item)}
                className="shrink-0 rounded-full p-0.5 hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                <ShieldAlert
                  className={`size-4 ${
                    item.userFavorite
                      ? "text-amber-400"
                      : "text-neutral-400"
                  }`}
                />
              </button>
            </Hint>
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
            {item.takenAt && <span>{formatDateTimeYear(item.takenAt)}</span>}
            {item.addedAt != null &&
              (item.takenAt == null ||
                Math.abs(item.addedAt - item.takenAt) > 86400) && (
                <Hint label="Added to this device's library later than it was captured — likely received, saved, or imported">
                  <span className="inline-flex items-center gap-1 text-status-warning-text">
                    <Import className="size-3 shrink-0" />
                    Added {formatDateTimeYear(item.addedAt)}
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
        item && item.availability === "thumbnail" ? (
          // The original is in iCloud, so there is no full-size file to open —
          // but the thumbnail iOS backed up IS here. Showing it beats an error
          // for the same reason the grid shows it: a soft picture answers "which
          // photo is this?", and "could not be read" answers nothing and implies
          // damage that has not occurred.
          <MediaMenu item={item} onMarkUnsafe={onMarkUnsafe}>
            <div className="flex max-h-full flex-col items-center gap-4">
              <img
                key={item.id}
                src={client.mediaUrl(item.id, { thumb: true, cacheKey })}
                alt={item.filename ?? "Photo"}
                className="max-h-[60vh] max-w-full rounded-lg object-contain"
              />
              <div className="flex max-w-md flex-col items-center gap-1 px-8 text-center">
                <p className="flex items-center gap-1.5 text-sm text-white/80">
                  <Cloud className="size-4 shrink-0" />
                  Thumbnail only — the full-size original is in iCloud
                </p>
                <p className="text-xs text-white/50">
                  With iCloud Photos on, iOS leaves the photo file out of the
                  device backup and keeps only this thumbnail. TraceLoupe reads
                  the backup and nothing else, so this is everything there is to
                  show offline.
                </p>
              </div>
            </div>
          </MediaMenu>
        ) : item && fullFailed ? (
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
            <MediaMenu item={item} onMarkUnsafe={onMarkUnsafe}>
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
            </MediaMenu>
          ) : (
            <MediaMenu item={item} onMarkUnsafe={onMarkUnsafe}>
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
            </MediaMenu>
          )
        ) : (
          <div className="size-16 animate-pulse rounded-full bg-white/20" />
        )
      }
      meta={meta}
    />
  );
}
