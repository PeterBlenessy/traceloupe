/**
 * The Timeline: everything the device did, in one stream, in time order.
 *
 * Every other view answers "what messages are there", "what photos are there".
 * This one answers **"what happened, and when"** — the question an examination
 * usually starts from, and the one the app could not answer: reconstructing an
 * afternoon meant opening six views and interleaving them by eye.
 *
 * A row carries its **content**, not a label saying something occurred. The
 * message shows its text, the photo shows the photo, the note its snippet, the
 * visit its page title. "Photo taken" is a log line; the photo is the evidence.
 *
 * Consecutive media collapse into one strip. Forty photos from one walk are one
 * moment, and forty rows of them bury the rest of the day.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import {
  Activity,
  AppWindow,
  CalendarDays,
  Camera,
  Clock,
  Globe,
  HeartPulse,
  ListTodo,
  Mic,
  MessageSquare,
  NotebookText,
  Phone,
  Search,
  Smartphone,
  Video,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { SortControl, type SortState } from "@/components/sort-control";
import { VirtualList } from "@/components/virtual-list";
import { useViewToolbar } from "@/components/toolbar-context";
import { multiBadgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { useTimePresets } from "@/components/time-filter";
import {
  EmptyView,
  ErrorState,
  ListSearch,
  ListSkeleton,
  NoBackupState,
} from "@/components/view";
import { formatDateTime, formatDuration, formatListTime } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { cn } from "@/lib/utils";
import { client, type TimeRange, type TimelineEvent } from "@/lib/ipc";

/** Events per page. The timeline unions every table, so it is the widest query
 *  in the app — it is windowed from the first render, never "load it all". */
const PAGE = 100;

/** Consecutive media within this many seconds read as one moment. Fifteen
 *  minutes covers a walk or a burst without swallowing a whole afternoon. */
const GROUP_GAP_S = 15 * 60;

/** What each kind is called and how it is drawn. Kept here so a new event kind
 *  is one entry rather than a scatter of `switch`es. */
const KINDS: Record<
  TimelineEvent["kind"],
  { label: string; icon: typeof Camera; tint: string }
> = {
  message: { label: "Messages", icon: MessageSquare, tint: "text-sky-400" },
  photo: { label: "Photos", icon: Camera, tint: "text-emerald-400" },
  video: { label: "Videos", icon: Video, tint: "text-emerald-400" },
  screenshot: { label: "Screenshots", icon: Smartphone, tint: "text-violet-400" },
  call: { label: "Calls", icon: Phone, tint: "text-amber-400" },
  visit: { label: "Web visits", icon: Globe, tint: "text-blue-400" },
  note: { label: "Notes", icon: NotebookText, tint: "text-yellow-400" },
  recording: { label: "Recordings", icon: Mic, tint: "text-rose-400" },
  app: { label: "Apps installed", icon: AppWindow, tint: "text-teal-400" },
  search: { label: "Searches", icon: Search, tint: "text-blue-400" },
  event: { label: "Calendar", icon: CalendarDays, tint: "text-orange-400" },
  reminder: { label: "Reminders", icon: ListTodo, tint: "text-lime-400" },
  workout: { label: "Workouts", icon: Activity, tint: "text-pink-400" },
  health: { label: "Health entries", icon: HeartPulse, tint: "text-rose-300" },
};

const MEDIA_KINDS: TimelineEvent["kind"][] = ["photo", "video", "screenshot"];

/** Actions that ARE the row's obvious reading, so saying them adds nothing.
 *  Anything else — added, edited, deleted, due — is stated on the entry. */
const DEFAULT_ACTIONS = new Set([
  "sent", "taken", "placed", "visited", "searched", "created",
  "recorded", "started", "logged", "installed",
]);

/** A day heading, one event, or a run of media shown as a strip. */
type Row =
  | { type: "day"; key: string; at: number }
  | { type: "event"; key: string; event: TimelineEvent }
  | { type: "media"; key: string; events: TimelineEvent[] }
  /** Sentinel: the virtualizer only mounts what is on screen, so this row
   *  rendering at all means the person scrolled to the end. */
  | { type: "more"; key: string };

/** A key that does not assume ids are unique across tables. They are in the
 *  cache, but a source that numbered its rows per-conversation would silently
 *  collapse two events into one row. */
function eventKey(e: TimelineEvent): string {
  return `e-${e.kind}-${e.id}-${e.at}-${e.source ?? ""}`;
}

/**
 * Group a flat, time-ordered stream into rows.
 *
 * Two things happen here: a heading whenever the day changes, and consecutive
 * media of the same kind within `GROUP_GAP_S` folded into one strip. The fold
 * is deliberately conservative — a photo taken between two messages stays its
 * own row, because the interleaving is the point of a timeline.
 */
export function buildRows(events: TimelineEvent[]): Row[] {
  const rows: Row[] = [];
  let day = "";
  let run: TimelineEvent[] = [];

  const flush = () => {
    if (run.length === 0) return;
    // A lone photo is just a photo; a strip of one would be a worse row than
    // the event itself.
    rows.push(
      run.length === 1
        ? { type: "event", key: eventKey(run[0]), event: run[0] }
        : { type: "media", key: `m-${eventKey(run[0])}-${run.length}`, events: run },
    );
    run = [];
  };

  for (const e of events) {
    const d = new Date(e.at * 1000).toDateString();
    if (d !== day) {
      flush();
      day = d;
      rows.push({ type: "day", key: `d-${d}`, at: e.at });
    }
    if (MEDIA_KINDS.includes(e.kind)) {
      const last = run[run.length - 1];
      // Same kind AND same act: forty shots are one moment, but a deletion
      // among them is a different thing happening and must not be folded away.
      if (
        last &&
        last.kind === e.kind &&
        last.action === e.action &&
        Math.abs(e.at - last.at) <= GROUP_GAP_S
      ) {
        run.push(e);
      } else {
        flush();
        run = [e];
      }
    } else {
      flush();
      rows.push({ type: "event", key: eventKey(e), event: e });
    }
  }
  flush();
  return rows;
}

export function TimelineView() {
  const navigate = useNavigate();
  const { presets } = useTimePresets();
  const [kinds, setKinds] = useState<string[]>([]);
  const [sources, setSources] = useState<string[]>([]);
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<SortState>({ by: "time", desc: true });
  const debounced = useDebounced(search, 250);

  const { data: hasBackup } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });

  const facets = useQuery({
    queryKey: ["timelineFacets"],
    queryFn: () => client.timelineFacets(),
    enabled: !!hasBackup,
  });

  const args = useMemo(
    () => ({
      kinds,
      sources,
      lo: range.lo,
      hi: range.hi,
      search: debounced || null,
      desc: sort.desc,
    }),
    [kinds, sources, range.lo, range.hi, debounced, sort.desc],
  );

  const total = useQuery({
    queryKey: ["timelineCount", args],
    queryFn: () => client.countTimelineEvents({ ...args, offset: 0, limit: 1 }),
    enabled: !!hasBackup,
  });

  const pages = useInfiniteQuery({
    queryKey: ["timelineEvents", args],
    initialPageParam: 0,
    queryFn: ({ pageParam }) =>
      client.getTimelineEvents({ ...args, offset: pageParam as number, limit: PAGE }),
    getNextPageParam: (last, all) =>
      last.length < PAGE ? undefined : all.reduce((n, p) => n + p.length, 0),
    enabled: !!hasBackup,
  });

  const events = useMemo(() => (pages.data?.pages ?? []).flat(), [pages.data]);
  const rows = useMemo(() => {
    const built = buildRows(events);
    if (pages.hasNextPage) built.push({ type: "more", key: "more" });
    return built;
  }, [events, pages.hasNextPage]);

  const onReachEnd = useCallback(() => {
    if (pages.hasNextPage && !pages.isFetchingNextPage) void pages.fetchNextPage();
  }, [pages]);

  const filterGroups = useMemo<FilterGroup[]>(() => {
    const out: FilterGroup[] = [];
    const kindOptions: BadgeFilterOption[] = (facets.data?.kinds ?? []).map((f) => ({
      value: f.value,
      label: KINDS[f.value as TimelineEvent["kind"]]?.label ?? f.value,
      count: f.count,
    }));
    if (kindOptions.length > 1) {
      out.push(
        multiBadgeGroup({
          key: "kind",
          label: "What happened",
          description: "Kinds of event to show (pick any)",
          options: kindOptions,
          selected: kinds,
          onToggle: (v) =>
            setKinds((prev) => (prev.includes(v) ? prev.filter((x) => x !== v) : [...prev, v])),
        }),
      );
    }
    // Capped: a busy device has hundreds of conversations, and a facet list that
    // long is a wall, not a filter. The count says what was left out.
    const sourceFacets = facets.data?.sources ?? [];
    const shown = sourceFacets.slice(0, 25);
    if (shown.length > 1) {
      out.push(
        multiBadgeGroup({
          key: "source",
          label:
            sourceFacets.length > shown.length
              ? `Where (top ${shown.length} of ${sourceFacets.length})`
              : "Where",
          description: "The conversation, album or app it came from (pick any)",
          options: shown.map((f) => ({ value: f.value, label: f.value, count: f.count })),
          selected: sources,
          onToggle: (v) =>
            setSources((prev) =>
              prev.includes(v) ? prev.filter((x) => x !== v) : [...prev, v],
            ),
        }),
      );
    }
    out.push(
      timeGroup({
        description: "When it happened",
        presets,
        value: range,
        onChange: setRange,
      }),
    );
    return out;
  }, [facets.data, kinds, sources, presets, range]);

  useViewToolbar(
    useMemo(
      () => ({
        title: "Timeline",
        count: total.data,
        filter: filterGroups,
        sort: (
          <SortControl
            value={sort}
            onChange={setSort}
            fields={[{ value: "time", label: "Time" }]}
          />
        ),
        search: (
          <ListSearch
            value={search}
            onChange={setSearch}
            placeholder="Search the timeline"
          />
        ),
      }),
      [total.data, filterGroups, sort, search],
    ),
  );

  if (!hasBackup) {
    return (
      <NoBackupState
        icon={Clock}
        title="See what happened, and when"
        lead="Everything the device did, in one stream — messages, photos, screenshots, calls, pages visited, notes written, apps installed — in the order it happened."
        features={[
          { label: "Content, not a log", detail: "Each entry shows the thing itself: the message text, the photo, the note." },
          { label: "One afternoon at a time", detail: "Filter by what happened, where it came from, or a date range." },
          { label: "Bursts stay readable", detail: "Photos taken in a row are shown as one strip, not forty lines." },
        ]}
      />
    );
  }
  if (pages.error) return <ErrorState error={pages.error} />;
  if (pages.isPending) return <ListSkeleton />;
  if (rows.length === 0) {
    const filtered = kinds.length > 0 || sources.length > 0 || !!debounced || range.lo != null;
    return (
      <EmptyView
        icon={Clock}
        title={filtered ? "Nothing in this slice" : "Nothing to place in time"}
        description={
          filtered
            ? "No event matches these filters. Widen the range, or clear a facet."
            : "Nothing in this backup carries a timestamp TraceLoupe can read yet."
        }
      />
    );
  }

  return (
    // `h-full min-h-0`: the virtualizer measures its scroll parent, and with an
    // unbounded one it renders EVERY row — 1600 of them here — which is the
    // freeze #67 exists to prevent. It also keeps the end-of-list sentinel
    // permanently on screen, so the list paged itself to the end of the backup.
    <div className="flex h-full min-h-0 flex-col">
    <VirtualList
      items={rows}
      getKey={(r) => r.key}
      estimateSize={72}
      renderItem={(r) =>
        r.type === "more" ? (
          <LoadMore onVisible={onReachEnd} />
        ) : r.type === "day" ? (
          <div className="sticky top-0 z-10 bg-background/95 px-4 pt-4 pb-1 text-xs font-semibold text-muted-foreground backdrop-blur">
            {formatDateTime(r.at).split(",")[0]}
          </div>
        ) : r.type === "media" ? (
          <MediaStrip events={r.events} onOpen={() => navigate({ to: "/photos" })} />
        ) : (
          <EventRow
            event={r.event}
            onOpen={() => {
              const e = r.event;
              if (MEDIA_KINDS.includes(e.kind)) return void navigate({ to: "/photos" });
              if (e.kind === "message") return void navigate({ to: "/messages" });
              if (e.kind === "visit") return void navigate({ to: "/safari" });
              if (e.kind === "note") return void navigate({ to: "/notes" });
              if (e.kind === "call") return void navigate({ to: "/calls" });
              if (e.kind === "recording") return void navigate({ to: "/recordings" });
              if (e.kind === "app") return void navigate({ to: "/apps" });
              if (e.kind === "search") return void navigate({ to: "/safari" });
              if (e.kind === "event") return void navigate({ to: "/calendar" });
              if (e.kind === "reminder") return void navigate({ to: "/reminders" });
              if (e.kind === "workout" || e.kind === "health")
                return void navigate({ to: "/health" });
            }}
          />
        )
      }
    />
    </div>
  );
}

/** The end-of-list sentinel. Mounting is the signal; there is no scroll maths
 *  to get wrong and no listener to leak. */
function LoadMore({ onVisible }: { onVisible: () => void }) {
  useEffect(() => {
    onVisible();
  }, [onVisible]);
  return (
    <div className="px-4 py-3 text-xs text-muted-foreground">Loading more…</div>
  );
}

/** A run of media as one strip — the moment, not forty rows of it. */
function MediaStrip({
  events,
  onOpen,
}: {
  events: TimelineEvent[];
  onOpen: () => void;
}) {
  const { icon: Icon, tint, label } = KINDS[events[0].kind];
  // Bounded (#67): a burst can be hundreds, and mounting every <img> is what
  // froze the app before. The remainder is stated, never silently dropped.
  const shown = events.slice(0, 12);
  const rest = events.length - shown.length;
  return (
    <button
      type="button"
      data-slot="list-row"
      onClick={onOpen}
      className="flex w-full flex-col gap-1.5 px-4 py-2 text-left hover:bg-accent/40"
    >
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Icon className={cn("size-3.5", tint)} />
        <span className="font-medium">
          {events.length} {label.toLowerCase()}
        </span>
        <span>·</span>
        <span>{formatListTime(events[0].at)}</span>
      </div>
      <div className="flex gap-1 overflow-hidden">
        {shown.map((e) => (
          <img
            key={e.id}
            src={client.mediaUrl(e.id, { thumb: true })}
            alt=""
            className="size-14 shrink-0 rounded-md bg-muted object-cover"
            onError={(ev) => {
              ev.currentTarget.style.visibility = "hidden";
            }}
          />
        ))}
        {rest > 0 && (
          <div className="flex size-14 shrink-0 items-center justify-center rounded-md bg-muted text-xs text-muted-foreground">
            +{rest}
          </div>
        )}
      </div>
    </button>
  );
}

/** One event, showing its own content. */
function EventRow({ event, onOpen }: { event: TimelineEvent; onOpen: () => void }) {
  const { icon: Icon, tint } = KINDS[event.kind];
  const isMedia = MEDIA_KINDS.includes(event.kind);
  return (
    <button
      type="button"
      data-slot="list-row"
      onClick={onOpen}
      className="flex w-full items-start gap-2.5 px-4 py-2 text-left hover:bg-accent/40"
    >
      <Icon className={cn("mt-0.5 size-4 shrink-0", tint)} />
      {isMedia && event.thumbPath && (
        <img
          src={client.mediaUrl(event.id, { thumb: true })}
          alt=""
          className="size-12 shrink-0 rounded-md bg-muted object-cover"
          onError={(e) => {
            e.currentTarget.style.visibility = "hidden";
          }}
        />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="truncate text-sm font-medium">
            {event.title ?? KINDS[event.kind].label}
          </span>
          {/* Only when it is not the obvious reading of the row. "Photo · taken"
              is noise; "Photo · deleted" is the whole point of the entry. */}
          {!DEFAULT_ACTIONS.has(event.action) && (
            <Badge
              variant="outline"
              className="shrink-0 px-1.5 py-0 text-3xs font-normal capitalize"
            >
              {event.action}
            </Badge>
          )}
          {event.source && (
            <span className="min-w-0 truncate text-xs text-muted-foreground">
              {event.source}
            </span>
          )}
        </div>
        {/* The content itself — this is why the view exists. */}
        {event.body && (
          <p className="line-clamp-2 text-sm text-foreground/80">{event.body}</p>
        )}
      </div>
      <div className="flex shrink-0 flex-col items-end gap-0.5">
        <span className="text-xs whitespace-nowrap text-muted-foreground">
          {formatListTime(event.at)}
        </span>
        {event.durationS != null && event.durationS > 0 && (
          <Badge variant="outline" className="px-1.5 py-0 text-3xs font-normal">
            {formatDuration(event.durationS)}
          </Badge>
        )}
      </div>
    </button>
  );
}
