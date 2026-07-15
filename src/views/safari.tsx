import { useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Bookmark, BookOpen, Globe, SquareStack } from "lucide-react";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { Button } from "@/components/ui/button";
import { BadgeFilter } from "@/components/badge-filter";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { TimeFilterBar, useTimePresets } from "@/components/time-filter";
import { EmptyView, LazyListView, ListSearch } from "@/components/view";
import { formatDateTime } from "@/lib/format";
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

  const changeType = (next: SafariType) => {
    setType(next);
    setSort({ by: "date", desc: true }); // "visits" only applies to history
  };

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
      header={
        <BadgeFilter
          value={type}
          onChange={(v) => changeType(v as SafariType)}
          options={TYPES.map((t) => ({ value: t.value, label: t.label }))}
        />
      }
      search={
        <ListSearch value={q} onChange={setQ} placeholder="Search Safari" />
      }
      toolbar={
        <>
          <TimeFilterBar
            className="flex-1"
            presets={presets}
            value={range}
            onChange={setRange}
            counts={presetCounts}
          />
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
        </>
      }
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
          <Globe className="size-5 text-muted-foreground" />
        </ItemMedia>
        <ItemContent>
          <ItemTitle className="truncate">
            {visit.title ?? hostOf(visit.url)}
          </ItemTitle>
          <ItemDescription className="truncate">{visit.url}</ItemDescription>
        </ItemContent>
        <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
          <span>{formatDateTime(visit.visitedAt)}</span>
          {visit.visitCount != null && <span>{visit.visitCount} visits</span>}
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
      {item.dateAdded != null && (
        <div className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
          {formatDateTime(item.dateAdded)}
        </div>
      )}
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
