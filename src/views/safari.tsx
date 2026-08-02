import { useCallback, useMemo, useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import {
  Bookmark,
  BookOpen,
  CloudDownload,
  CornerDownRight,
  EyeOff,
  Globe,
  Search,
  SquareStack,
  Trash2,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import {
  Item, ItemContent, ItemDescription, ItemMedia, ItemTitle, } from "@/components/ui/item";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { useEncryptedOnlyEmpty } from "@/lib/use-encrypted-only";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState, LazyListView, ListSearch } from "@/components/view";
import { formatDate, formatDateTime, plural } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useDebounced } from "@/lib/use-debounced";
import { emptyListMessage, isTimeFiltered } from "@/lib/empty-message";
import { useNotImportedEmpty } from "@/lib/use-not-imported";
import { useParseFailedEmpty } from "@/lib/use-parse-failed";
import {
  client,
  type HistoryVisit,
  type SafariBookmark,
  type TimeRange,
  type WebSearch,
} from "@/lib/ipc";

/** The Safari data types, selectable via the pill filter on the title row. */
type SafariType =
  | "history"
  | "search"
  | "bookmark"
  | "reading_list"
  | "tab";
const TYPES: { value: SafariType; label: string }[] = [
  { value: "history", label: "History" },
  { value: "search", label: "Searches" },
  { value: "bookmark", label: "Bookmarks" },
  { value: "reading_list", label: "Reading List" },
  { value: "tab", label: "Tabs" },
];
/** The plural thing each pill lists, for empty-state wording that names it. */
const NOUN: Record<SafariType, string> = {
  history: "visits",
  search: "searches",
  bookmark: "bookmarks",
  reading_list: "reading-list items",
  tab: "tabs",
};
const EMPTY: Record<SafariType, string> = {
  history: "No Safari history in this backup.",
  search: "No web searches in this backup.",
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
  const isSearch = type === "search";
  const rangeArgs = [range.lo, range.hi] as const;

  const { data: count, error } = useQuery({
    queryKey: ["safariCount", type, search, range.lo, range.hi],
    queryFn: () =>
      isHistory
        ? client.countSafari(search, ...rangeArgs)
        : isSearch
          ? client.countSafariSearches(search, ...rangeArgs)
          : client.countSafariBookmarks(type, search, ...rangeArgs),
    enabled: active === true,
  });
  const { data: presetCounts } = useQuery({
    queryKey: ["safariRanges", type, now, search],
    queryFn: () => {
      const ranges = presets.map((p) => ({ lo: p.lo, hi: p.hi }));
      return isHistory
        ? client.countSafariRanges(search, ranges)
        : isSearch
          ? client.countSafariSearchRanges(search, ranges)
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
        description: "History, searches, bookmarks, reading list or tabs",
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
            : isSearch
              ? [
                  { value: "date", label: "Date" },
                  { value: "term", label: "Term" },
                  { value: "engine", label: "Engine" },
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
    [isHistory, isSearch, sort, setSort],
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
  // Only the iCloud half of Tabs is encrypted-backup-only
  // (Library/Safari/SafariTabs.db); local tabs come from BrowserState.db and
  // are in any backup. So this explains the gap without claiming the whole
  // section is missing.
  const emptyTabs = useEncryptedOnlyEmpty(
    "Tabs synced from your other Apple devices",
    EMPTY.tab,
  );
  // Safari's own module covers history/bookmarks/tabs; if it was unticked at
  // import, every pill here is empty for that reason rather than the device's.
  const notImported = useNotImportedEmpty("safari", "Safari data", "");
  // A Safari store that was present and would not open (#288).
  const parseFailed = useParseFailedEmpty("safari", "Safari data", "");
  const emptyForType =
    notImported || parseFailed || (type === "tab" ? emptyTabs : EMPTY[type]);

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
    <LazyListView<HistoryVisit | SafariBookmark | WebSearch>
      title="Safari"
      count={count}
      error={error}
      resetKey={`${type}:${search ?? ""}:${range.lo}:${range.hi}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={emptyListMessage(
        { search, timeFiltered: isTimeFiltered(range) },
        emptyForType,
        NOUN[type],
      )}
      emptyIcon={Globe}
      underlap
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
          : isSearch
          ? client.getSafariSearchesWindow(
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
        ) : "term" in item ? (
          <SearchRow search={item} />
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
  // "Default" is the profile almost every visit has, so labelling it would put a
  // badge on every row and tell the reader nothing. Only a named profile shows.
  const namedProfile =
    visit.profile && visit.profile !== "Default" ? visit.profile : null;
  const redirect = visit.redirectSource ?? visit.redirectDestination;
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
              <ItemTitle
                className={cn(
                  "flex items-center gap-1.5 truncate",
                  visit.deleted && "line-through",
                )}
              >
                <span className="truncate">
                  {visit.title ?? hostOf(visit.url)}
                </span>
                {namedProfile && (
                  <Badge variant="secondary" className="shrink-0 font-normal">
                    {namedProfile}
                  </Badge>
                )}
              </ItemTitle>
              <ItemDescription className="flex items-center gap-1.5 truncate">
                {redirect && (
                  <CornerDownRight className="size-3 shrink-0 text-muted-foreground" />
                )}
                <span className="truncate">{visit.url}</span>
              </ItemDescription>
            </ItemContent>
            <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
              <span className="flex items-center gap-1">
                {visit.synced && <CloudDownload className="size-3.5" />}
                {visit.deleted ? "Deleted" : formatDateTime(visit.visitedAt)}
              </span>
              {visit.visitCount != null && (
                <span>{plural(visit.visitCount, "visit")}</span>
              )}
            </div>
          </button>
        </Item>
      </TooltipTrigger>
      <TooltipContent className="max-w-sm">
        <div className="space-y-1">
          <div>{`Open ${visit.url}`}</div>
          {visit.synced && (
            <div className="text-muted-foreground">
              Visited on another device signed into this iCloud account, not on
              this one.
            </div>
          )}
          {namedProfile && (
            <div className="text-muted-foreground">{`Safari profile: ${namedProfile}`}</div>
          )}
          {visit.redirectSource && (
            <div className="text-muted-foreground">{`Redirected from ${visit.redirectSource}`}</div>
          )}
          {visit.redirectDestination && (
            <div className="text-muted-foreground">{`Redirected to ${visit.redirectDestination}`}</div>
          )}
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

/** A web search — typed into the search field, or recovered from a result-page
 *  URL in history. The two are different evidence, so the row says which. */
function SearchRow({ search }: { search: WebSearch }) {
  const typed = search.source === "typed";
  const inner = (
    <>
      <ItemMedia>
        <Search className="size-5 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{search.term}</ItemTitle>
        <ItemDescription className="truncate">
          {search.engine ?? (typed ? "Typed in Safari" : "Search")}
        </ItemDescription>
      </ItemContent>
      <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
        <span>{formatDateTime(search.searchedAt)}</span>
        <span>{typed ? "Typed" : "Visited"}</span>
      </div>
    </>
  );
  // A typed search has no URL to open, so it is not a button — making it one
  // would offer a click that does nothing.
  const url = search.url;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        {url ? (
          <Item
            asChild
            className="rounded-md transition-colors hover:bg-accent/50"
          >
            <button
              type="button"
              onClick={() => void client.openExternal(url)}
              className="w-full text-left"
            >
              {inner}
            </button>
          </Item>
        ) : (
          <Item className="rounded-md">{inner}</Item>
        )}
      </TooltipTrigger>
      <TooltipContent className="max-w-sm">
        {url
          ? `Open ${url}`
          : "Typed into Safari's search field. Recorded without a result page, so there is nothing to open."}
      </TooltipContent>
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
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-status-note-soft px-1.5 py-0.5 text-3xs font-medium text-status-note-text">
                  <EyeOff className="size-2.5" />
                  Private
                </span>
              </TooltipTrigger>
              {/* The old native title= said "Open in a private-browsing window",
                  which reads as an instruction to the user. This is a record of
                  what the device did, not something to do. */}
              <TooltipContent>
                This tab was open in a private-browsing window.
              </TooltipContent>
            </Tooltip>
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
            <span className="rounded-full bg-primary/10 px-1.5 py-0.5 text-3xs font-medium text-primary">
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
