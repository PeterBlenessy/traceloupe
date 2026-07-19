import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { CalendarDays, Clock, MapPin, Repeat } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useDebounced } from "@/lib/use-debounced";
import {
  EmptyView,
  ListSearch,
  VirtualListView,
} from "@/components/view";
import { useSettings } from "@/components/settings-provider";
import { formatDate, formatDateTime, formatTime } from "@/lib/format";
import { client, type CalendarEvent, type TimeRange } from "@/lib/ipc";

/** The event's when-line: "All day" or a start (→ end) date/time. */
function whenLabel(e: CalendarEvent): string {
  if (e.startAt == null) return "—";
  if (e.allDay) return `${formatDate(e.startAt)} · All day`;
  const start = formatDateTime(e.startAt);
  if (e.endAt != null && e.endAt > e.startAt) {
    // Same-day end → show just the end time, else the full end date/time.
    const sameDay =
      new Date(e.startAt * 1000).toDateString() ===
      new Date(e.endAt * 1000).toDateString();
    return `${start} – ${sameDay ? formatTime(e.endAt) : formatDateTime(e.endAt)}`;
  }
  return start;
}

function EventRow({ event }: { event: CalendarEvent }) {
  return (
    <div data-slot="list-row" className="rounded-md px-3 py-2.5">
      <div className="flex items-baseline justify-between gap-2">
        <span className="flex min-w-0 items-center gap-1.5 font-medium">
          {event.recurring && (
            <Repeat
              className="size-3.5 shrink-0 text-muted-foreground"
              aria-label="Repeating event"
            />
          )}
          <span className="truncate">{event.title ?? "(untitled event)"}</span>
        </span>
        {event.calendarName && (
          <Badge variant="secondary" className="shrink-0">
            {event.calendarName}
          </Badge>
        )}
      </div>
      <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
        <Clock className="size-3 shrink-0" />
        <span className="select-text">{whenLabel(event)}</span>
      </div>
      {event.location && (
        <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
          <MapPin className="size-3 shrink-0" />
          <span className="select-text truncate">{event.location}</span>
        </div>
      )}
      {event.notes && (
        <p className="mt-1 select-text whitespace-pre-wrap break-words text-sm text-muted-foreground">
          {event.notes}
        </p>
      )}
    </div>
  );
}

/** True when `at` falls in a half-open [lo, hi) window; undated only pass "All". */
function inWindow(at: number | null, lo: number | null, hi: number | null) {
  if (lo == null && hi == null) return true;
  if (at == null) return false;
  return (lo == null || at >= lo) && (hi == null || at < hi);
}

export function CalendarView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: events,
    isPending,
    error,
  } = useQuery({
    queryKey: ["calendarEvents"],
    queryFn: () => client.listCalendarEvents(),
    enabled: active === true,
  });

  const [cal, setCal] = usePersistedState<string>("calendar:cal", "all");
  const [avail, setAvail] = usePersistedState<string>("calendar:avail", "all");
  const [sort, setSort] = usePersistedState<SortState>("calendar:sort", {
    by: "start",
    desc: true,
  });
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim().toLowerCase());
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Re-render (and remount the list) when the clock preference changes so the
  // shared time formatters are re-read for every visible row.
  const { clockFormat } = useSettings();

  // The distinct calendars present, for the calendar facet.
  const calendars = useMemo(
    () =>
      Array.from(
        new Set(
          (events ?? [])
            .map((e) => e.calendarName)
            .filter((c): c is string => !!c),
        ),
      ).sort((a, b) => a.localeCompare(b)),
    [events],
  );
  // Clamp a stale persisted calendar to what this backup has.
  const effCal = cal !== "all" && calendars.includes(cal) ? cal : "all";
  // Distinct free/busy availabilities present, for that facet.
  const avails = useMemo(
    () =>
      Array.from(
        new Set(
          (events ?? [])
            .map((e) => e.availability)
            .filter((a): a is string => !!a),
        ),
      ).sort(),
    [events],
  );
  const effAvail = avail !== "all" && avails.includes(avail) ? avail : "all";

  // Calendar + availability + search filtered (base for the time-chip counts).
  const baseFiltered = useMemo(() => {
    return (events ?? []).filter((e) => {
      if (effCal !== "all" && e.calendarName !== effCal) return false;
      if (effAvail !== "all" && e.availability !== effAvail) return false;
      if (search) {
        const hay = [e.title, e.notes, e.location]
          .filter(Boolean)
          .join(" ")
          .toLowerCase();
        if (!hay.includes(search)) return false;
      }
      return true;
    });
  }, [events, effCal, effAvail, search]);

  const presetCounts = useMemo(
    () => presets.map((p) => baseFiltered.filter((e) => inWindow(e.startAt, p.lo, p.hi)).length),
    [presets, baseFiltered],
  );

  const filtered = useMemo(() => {
    const inRange = baseFiltered.filter((e) => inWindow(e.startAt, range.lo, range.hi));
    return sortItems(
      inRange,
      (e) => (sort.by === "title" ? (e.title ?? "").toLowerCase() : e.startAt),
      sort.desc,
    );
  }, [baseFiltered, sort, range]);

  const hasEvents = (events?.length ?? 0) > 0;
  const filterGroups = useMemo<FilterGroup[]>(() => {
    if (!hasEvents) return [];
    const calOptions: BadgeFilterOption[] = [
      { value: "all", label: "All", count: events?.length },
      ...calendars.map((c) => ({
        value: c,
        label: c,
        count: (events ?? []).filter((e) => e.calendarName === c).length,
      })),
    ];
    const availOptions: BadgeFilterOption[] = [
      { value: "all", label: "Any" },
      ...avails.map((a) => ({
        value: a,
        label: a.charAt(0).toUpperCase() + a.slice(1),
        count: (events ?? []).filter((e) => e.availability === a).length,
      })),
    ];
    const list: FilterGroup[] = [];
    if (calendars.length > 1)
      list.push(badgeGroup({ key: "cal", label: "Calendar", description: "Which calendar the event is on", options: calOptions, value: effCal, onChange: setCal }));
    if (avails.length > 1)
      list.push(badgeGroup({ key: "avail", label: "Availability", description: "Free or busy", options: availOptions, value: effAvail, onChange: setAvail }));
    list.push(timeGroup({ description: "When the event starts", presets, counts: presetCounts, value: range, onChange: setRange }));
    return list;
  }, [hasEvents, events, calendars, avails, effCal, effAvail, presets, presetCounts, range, setCal, setAvail, setRange]);
  const sortNode = useMemo(
    () =>
      hasEvents ? (
        <SortControl
          fields={[
            { value: "start", label: "Date" },
            { value: "title", label: "Title" },
          ]}
          value={sort}
          onChange={setSort}
        />
      ) : undefined,
    [hasEvents, sort, setSort],
  );
  const searchNode = useMemo(
    () => (hasEvents ? <ListSearch value={q} onChange={setQ} placeholder="Search events" /> : undefined),
    [hasEvents, q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Calendar", count: filtered.length, islands: [], filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, filtered.length, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <EmptyView
        icon={CalendarDays}
        title="No backup open"
        description="Import a backup to see calendar events."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <VirtualListView<CalendarEvent>
      key={clockFormat}
      headless
      title="Calendar"
      count={filtered.length}
      estimateSize={76}
      isPending={isPending}
      error={error}
      emptyMessage={
        hasEvents ? "No events match these filters." : "No calendar events in this backup."
      }
      items={filtered}
      getKey={(e) => e.id}
      renderItem={(e) => <EventRow event={e} />}
    />
  );
}
