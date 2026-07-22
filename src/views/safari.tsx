import { useCallback, useMemo, useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { Bookmark, BookOpen, EyeOff, Globe, SquareStack, Trash2 } from "lucide-react";
import {
  Item, ItemContent, ItemDescription, ItemMedia, ItemTitle, } from "@/components/ui/item";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState, LazyListView, ListSearch } from "@/components/view";
import { formatCount, formatDate, formatDateTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useDebounced } from "@/lib/use-debounced";
import {
  client,
  type HistoryVisit,
  type SafariBookmark,
  type TimeRange,
} from "@/lib/ipc";

/** The Safari data types, selectable via the pill filter on the title row. */
type SafariType = "history" | "bookmark" | "reading_list" | "tab";
const TYPES: { value: SafariType; label: string }[] = [
  { value: "history", label: "History" },
  { value: "bookmark", label: "Bookmarks" },
  { value: "reading_list", label: "Reading List" },
  { value: "tab", label: "Tabs" },
];
const EMPTY: Record<SafariType, string> = {
  history: "No Safari history in this backup.",
  bookmark: "No bookmarks in this backup.",
  reading_list: "No reading-list items in this backup.",
  tab: "No open tabs in this backup.",
};

export function SafariView() {
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [type, setType] = usePersistedState<SafariType>("safari:type", "history");
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  const [sort, setSort] = usePersistedState<SortState>("safari:sort", { by: "date", desc: true });
  const { now, presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Subscribe to the clock preference so times re-render on change.
  const { clockFormat } = useSettings();

  const isHistory = type === "history";
  const rangeArgs = [range.lo, range.hi] as const;

  const { data: count, error } = useQuery({
    queryKey: ["safariCount", type, search, range.lo, range.hi],
    queryFn: () =>
      isHistory
        ? client.countSafari(search, ...rangeArgs)
        : client.countSafariBookmarks(type, search, ...rangeArgs),
    enabled: active === true,
  });
  const { data: presetCounts } = useQuery({
    queryKey: ["safariRanges", type, now, search],
    queryFn: () => {
      const ranges = presets.map((p) => ({ lo: p.lo, hi: p.hi }));
      return isHistory
        ? client.countSafariRanges(search, ranges)
        : client.countSafariBookmarkRanges(type, search, ranges);
    },
    enabled: active === true,
  });

  const changeType = useCallback(
    (next: SafariType) => {
      setType(next);
      setSort({ by: "date", desc: true }); // "visits" only applies to history
    },
    [setType, setSort],
  );

  const filterGroups = useMemo<FilterGroup[]>(
    () => [
      badgeGroup({
        key: "type",
        label: "Type",
        description: "History, bookmarks, reading list or tabs",
        options: TYPES.map((t) => ({ value: t.value, label: t.label })),
        value: type,
        onChange: (v) => changeType(v as SafariType),
      }),
      timeGroup({ description: "When it was last visited", presets, counts: presetCounts, value: range, onChange: setRange }),
    ],
    [type, presets, presetCounts, range, changeType, setRange],
  );
  const sortNode = useMemo(
    () => (
      <SortControl
        fields={
          isHistory
            ? [
                { value: "date", label: "Date" },
                { value: "title", label: "Title" },
                { value: "visits", label: "Visits" },
              ]
            : [
                { value: "date", label: "Date" },
                { value: "title", label: "Title" },
              ]
        }
        value={sort}
        onChange={setSort}
      />
    ),
    [isHistory, sort, setSort],
  );
  const searchNode = useMemo(
    () => <ListSearch value={q} onChange={setQ} placeholder="Search Safari" />,
    [q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Safari", count, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, count, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <NoBackupState
        icon={Globe}
        title="See Safari activity"
        lead="The device's web activity — history, bookmarks, reading list, and open tabs — reconstructed from the backup, with each entry opening in your browser."
        features={[
          { label: "Search", detail: "Search across all Safari data." },
          { label: "Filter by type", detail: "Switch between History, Bookmarks, Reading List, and Tabs." },
          { label: "Time range", detail: "Limit to any date range." },
          { label: "Sort & detail", detail: "Sort by date, title, or visit count; see folders and Private/Read state." },
        ]}
        note="Everything stays on this Mac."
      />
    );
  }

  return (
    <LazyListView<HistoryVisit | SafariBookmark>
      title="Safari"
      count={count}
      error={error}
      resetKey={`${type}:${search ?? ""}:${range.lo}:${range.hi}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={search ? "No matches." : EMPTY[type]}
      emptyIcon={Globe}
      windowKey={(page) => [
        "safariWindow",
        type,
        search,
        range.lo,
        range.hi,
        sort.by,
        sort.desc,
        page,
      ]}
      fetchWindow={(offset, limit) =>
        isHistory
          ? client.getSafariWindow(
              search,
              range.lo,
              range.hi,
              offset,
              limit,
              sort.by,
              sort.desc,
            )
          : client.getSafariBookmarksWindow(
              type,
              search,
              range.lo,
              range.hi,
              offset,
              limit,
              sort.by,
              sort.desc,
            )
      }
      renderItem={(item) =>
        "kind" in item ? (
          <BookmarkRow item={item} />
        ) : (
          <VisitRow visit={item} />
        )
      }
    />
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

function VisitRow({ visit }: { visit: HistoryVisit }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Item
          asChild
          className="rounded-md transition-colors hover:bg-accent/50"
        >
          <button
            type="button"
            onClick={() => void client.openExternal(visit.url)}
            className="w-full text-left"
          >
            <ItemMedia>
              {visit.deleted ? (
                <Trash2 className="size-5 text-muted-foreground" />
              ) : (
                <Globe className="size-5 text-muted-foreground" />
              )}
            </ItemMedia>
            <ItemContent>
              <ItemTitle className={cn("truncate", visit.deleted && "line-through")}>
                {visit.title ?? hostOf(visit.url)}
              </ItemTitle>
              <ItemDescription className="truncate">{visit.url}</ItemDescription>
            </ItemContent>
            <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
              <span>{visit.deleted ? "Deleted" : formatDateTime(visit.visitedAt)}</span>
              {visit.visitCount != null && (
                <span>{formatCount(visit.visitCount)} visits</span>
              )}
            </div>
          </button>
        </Item>
      </TooltipTrigger>
      <TooltipContent>{`Open ${visit.url}`}</TooltipContent>
    </Tooltip>
  );
}

function BookmarkRow({ item }: { item: SafariBookmark }) {
  const Icon =
    item.kind === "reading_list"
      ? BookOpen
      : item.kind === "tab"
        ? SquareStack
        : Bookmark;
  // Reading-list items carry a preview snippet; bookmarks/tabs show their folder.
  const secondary =
    item.kind === "reading_list" ? item.previewText : item.folder;
  const url = item.url;
  const inner = (
    <>
      <ItemMedia>
        <Icon className="size-5 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="flex items-center gap-1.5 truncate">
          <span className="truncate">
            {item.title ?? (url ? hostOf(url) : "Untitled")}
          </span>
          {item.private && (
            <span
              className="inline-flex shrink-0 items-center gap-1 rounded-full bg-purple-500/15 px-1.5 py-0.5 text-[10px] font-medium text-purple-500"
              title="Open in a private-browsing window"
            >
              <EyeOff className="size-2.5" />
              Private
            </span>
          )}
        </ItemTitle>
        {url && <ItemDescription className="truncate">{url}</ItemDescription>}
        {secondary && (
          <ItemDescription className="truncate text-muted-foreground/80">
            {secondary}
          </ItemDescription>
        )}
      </ItemContent>
      <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
        {item.dateAdded != null && <span>{formatDateTime(item.dateAdded)}</span>}
        {item.kind === "tab" && item.dateViewed != null && (
          <span className="text-muted-foreground/60">
            Last viewed {formatDate(item.dateViewed)}
          </span>
        )}
        {item.kind === "reading_list" &&
          (item.dateViewed != null ? (
            <span className="text-muted-foreground/60">
              Read {formatDate(item.dateViewed)}
            </span>
          ) : (
            <span className="rounded-full bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary">
              Unread
            </span>
          ))}
      </div>
    </>
  );
  // Openable when it has a URL (bookmarks/tabs/reading list); folders don't.
  if (!url) return <Item>{inner}</Item>;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Item asChild className="rounded-md transition-colors hover:bg-accent/50">
          <button
            type="button"
            onClick={() => void client.openExternal(url)}
            className="w-full text-left"
          >
            {inner}
          </button>
        </Item>
      </TooltipTrigger>
      <TooltipContent>{`Open ${url}`}</TooltipContent>
    </Tooltip>
  );
}
