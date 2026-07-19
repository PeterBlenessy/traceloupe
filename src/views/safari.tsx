import { useCallback, useMemo, useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Bookmark, BookOpen, Globe, SquareStack, Trash2 } from "lucide-react";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { Button } from "@/components/ui/button";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { EmptyView, LazyListView, ListSearch } from "@/components/view";
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
  const navigate = useNavigate();
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
      <EmptyView
        icon={Globe}
        title="No backup open"
        description="Import a backup to see Safari data."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <LazyListView<HistoryVisit | SafariBookmark>
      title="Safari"
      count={count}
      error={error}
      resetKey={`${type}:${search ?? ""}:${range.lo}:${range.hi}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={search ? "No matches." : EMPTY[type]}
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
    <Item
      asChild
      className="rounded-md transition-colors hover:bg-accent/50"
    >
      <button
        type="button"
        onClick={() => void client.openExternal(visit.url)}
        title={`Open ${visit.url}`}
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
        <ItemTitle className="truncate">
          {item.title ?? (url ? hostOf(url) : "Untitled")}
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
    <Item asChild className="rounded-md transition-colors hover:bg-accent/50">
      <button
        type="button"
        onClick={() => void client.openExternal(url)}
        title={`Open ${url}`}
        className="w-full text-left"
      >
        {inner}
      </button>
    </Item>
  );
}
