import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Activity, ChevronDown, Footprints, HeartPulse, MapPin, Moon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useSettings } from "@/components/settings-provider";
import { EmptyView, ErrorState, ListSkeleton, VirtualListView } from "@/components/view";
import { formatCount, formatDate, formatDateTime, formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import {
  client,
  type HealthDay,
  type SleepSession,
  type Workout,
  type TimeRange,
} from "@/lib/ipc";

/** The Health data sections, selectable via the Section filter. */
type HealthSection = "workouts" | "daily" | "sleep";
const SECTIONS: { value: HealthSection; label: string }[] = [
  { value: "workouts", label: "Workouts" },
  { value: "daily", label: "Daily activity" },
  { value: "sleep", label: "Sleep" },
];

/** Metres → a compact "5.2 km" / "820 m". */
function formatDistance(m: number | null): string | null {
  if (m == null || m <= 0) return null;
  return m >= 1000 ? `${(m / 1000).toFixed(2)} km` : `${Math.round(m)} m`;
}

/** Days are aggregated per UTC day at import; format the label in UTC so the
 *  local timezone can't shift it onto a neighbouring date. */
function formatDayUTC(at: number): string {
  return new Date(at * 1000).toLocaleDateString(undefined, {
    timeZone: "UTC",
    weekday: "short",
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** The recorded GPS trace as an inline polyline (equirectangular projection,
 *  fitted and centered). Single series in the primary hue; start ring + end dot
 *  mark direction. Fetched lazily when the row expands. */
function RoutePreview({ workoutId }: { workoutId: number }) {
  const { data: route, isPending } = useQuery({
    queryKey: ["workoutRoute", workoutId],
    queryFn: () => client.workoutRoute(workoutId),
  });
  if (isPending)
    return <div className="mx-2 mb-1 h-44 animate-pulse rounded-md bg-muted/40" />;
  if (!route || route.length < 2)
    return (
      <p className="px-4 pb-2 text-xs text-muted-foreground">
        No route points recorded.
      </p>
    );

  const W = 640;
  const H = 260;
  const PAD = 12;
  // Longitude degrees shrink with latitude; scale them so the shape keeps its
  // real-world proportions at this latitude.
  const midLat =
    route.reduce((acc, p) => acc + p.latitude, 0) / route.length;
  const k = Math.cos((midLat * Math.PI) / 180);
  const xs = route.map((p) => p.longitude * k);
  const ys = route.map((p) => -p.latitude);
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minY = Math.min(...ys);
  const maxY = Math.max(...ys);
  const spanX = maxX - minX || 1e-9;
  const spanY = maxY - minY || 1e-9;
  const scale = Math.min((W - 2 * PAD) / spanX, (H - 2 * PAD) / spanY);
  const ox = (W - spanX * scale) / 2;
  const oy = (H - spanY * scale) / 2;
  const px = (i: number) => ox + (xs[i] - minX) * scale;
  const py = (i: number) => oy + (ys[i] - minY) * scale;
  const points = route.map((_, i) => `${px(i).toFixed(1)},${py(i).toFixed(1)}`).join(" ");
  const last = route.length - 1;

  const alts = route
    .map((p) => p.altitude)
    .filter((a): a is number => a != null);
  const caption = [
    `${formatCount(route.length)} GPS points`,
    alts.length > 0
      ? `alt ${Math.round(Math.min(...alts))}–${Math.round(Math.max(...alts))} m`
      : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <div className="mx-2 mb-1 rounded-md border bg-muted/20 px-3 py-2">
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="h-44 w-full text-primary"
        role="img"
        aria-label="Workout GPS route"
      >
        <polyline
          points={points}
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
        {/* Start: open ring; end: filled dot — direction without a legend. */}
        <circle
          cx={px(0)}
          cy={py(0)}
          r={5}
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          className="text-muted-foreground"
        />
        <circle cx={px(last)} cy={py(last)} r={5} fill="currentColor" />
      </svg>
      <p className="pt-1 text-xs text-muted-foreground">{caption}</p>
    </div>
  );
}

function WorkoutRow({ workout }: { workout: Workout }) {
  const [expanded, setExpanded] = useState(false);
  const bits = [
    formatDuration(workout.durationS),
    formatDistance(workout.distanceM),
  ].filter(Boolean);
  const inner = (
    <>
      <ItemMedia>
        <Activity className="size-4 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">
          {workout.activity ?? "Workout"}
        </ItemTitle>
        <div className="text-xs text-muted-foreground">
          {workout.startAt != null ? formatDateTime(workout.startAt) : "—"}
          {bits.length > 0 && ` · ${bits.join(" · ")}`}
        </div>
      </ItemContent>
      {workout.hasRoute && (
        <div className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
          <MapPin className="size-3.5" />
          Route
          <ChevronDown
            className={cn("size-3.5 transition-transform", expanded && "rotate-180")}
          />
        </div>
      )}
    </>
  );
  return (
    <div className="px-2 py-0.5">
      {workout.hasRoute ? (
        <Item asChild className="rounded-md transition-colors hover:bg-accent/50">
          <button
            type="button"
            onClick={() => setExpanded((e) => !e)}
            title={expanded ? "Hide route" : "Show route"}
            className="w-full text-left"
          >
            {inner}
          </button>
        </Item>
      ) : (
        <Item>{inner}</Item>
      )}
      {expanded && <RoutePreview workoutId={workout.id} />}
    </div>
  );
}

function DayRow({ day }: { day: HealthDay }) {
  const bits = [
    day.steps != null ? `${formatCount(day.steps)} steps` : null,
    formatDistance(day.distanceM),
    day.flights ? `${formatCount(day.flights)} floors` : null,
    day.activeKcal != null && day.activeKcal >= 1
      ? `${formatCount(Math.round(day.activeKcal))} kcal active`
      : null,
    day.restingKcal != null && day.restingKcal >= 1
      ? `${formatCount(Math.round(day.restingKcal))} kcal resting`
      : null,
  ].filter(Boolean);
  return (
    <Item>
      <ItemMedia>
        <Footprints className="size-4 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{formatDayUTC(day.dayAt)}</ItemTitle>
        <div className="text-xs text-muted-foreground">
          {bits.length > 0 ? bits.join(" · ") : "No activity recorded"}
        </div>
      </ItemContent>
      {day.hrAvg != null && (
        <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1">
            <HeartPulse className="size-3" />
            {Math.round(day.hrMin ?? day.hrAvg)}–{Math.round(day.hrMax ?? day.hrAvg)} bpm
          </span>
          <span className="text-muted-foreground/60">
            avg {Math.round(day.hrAvg)}
          </span>
        </div>
      )}
    </Item>
  );
}

function SleepRow({ session }: { session: SleepSession }) {
  const duration =
    session.startAt != null && session.endAt != null && session.endAt > session.startAt
      ? formatDuration(session.endAt - session.startAt)
      : null;
  return (
    <Item>
      <ItemMedia>
        <Moon className="size-4 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{session.stage}</ItemTitle>
        <div className="text-xs text-muted-foreground">
          {session.startAt != null ? formatDateTime(session.startAt) : "—"}
          {session.endAt != null && ` – ${formatDateTime(session.endAt)}`}
        </div>
      </ItemContent>
      {duration && (
        <div className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
          {duration}
        </div>
      )}
    </Item>
  );
}

/** True when `at` falls in a half-open [lo, hi) window; undated only pass "All". */
function inWindow(at: number | null, lo: number | null, hi: number | null) {
  if (lo == null && hi == null) return true;
  if (at == null) return false;
  return (lo == null || at >= lo) && (hi == null || at < hi);
}

export function HealthView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [section, setSection] = usePersistedState<HealthSection>(
    "health:section",
    "workouts",
  );
  const {
    data: workouts,
    isPending,
    error,
  } = useQuery({
    queryKey: ["workouts"],
    queryFn: () => client.listWorkouts(),
    enabled: active === true,
  });
  const { data: summary } = useQuery({
    queryKey: ["healthSummary"],
    queryFn: () => client.healthSummary(),
    enabled: active === true,
  });
  const {
    data: days,
    isPending: daysPending,
    error: daysError,
  } = useQuery({
    queryKey: ["healthDaily"],
    queryFn: () => client.healthDaily(),
    enabled: active === true,
  });
  const {
    data: sleep,
    isPending: sleepPending,
    error: sleepError,
  } = useQuery({
    queryKey: ["healthSleep"],
    queryFn: () => client.listSleep(),
    enabled: active === true,
  });

  const [activity, setActivity] = usePersistedState<string>("health:activity", "all");
  const [sort, setSort] = usePersistedState<SortState>("health:sort", {
    by: "date",
    desc: true,
  });
  const [daySort, setDaySort] = usePersistedState<SortState>("health:daySort", {
    by: "date",
    desc: true,
  });
  const [sleepSort, setSleepSort] = usePersistedState<SortState>("health:sleepSort", {
    by: "date",
    desc: true,
  });
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Re-render when the clock preference changes so workout times re-format.
  const { clockFormat } = useSettings();

  const activities = useMemo(
    () =>
      Array.from(
        new Set(
          (workouts ?? []).map((w) => w.activity).filter((a): a is string => !!a),
        ),
      ).sort((a, b) => a.localeCompare(b)),
    [workouts],
  );
  const effActivity =
    activity !== "all" && activities.includes(activity) ? activity : "all";

  const baseFiltered = useMemo(
    () =>
      section !== "workouts"
        ? []
        : (workouts ?? []).filter(
            (w) => effActivity === "all" || w.activity === effActivity,
          ),
    [section, workouts, effActivity],
  );
  const presetCounts = useMemo(
    () =>
      section === "daily"
        ? presets.map((p) => (days ?? []).filter((d) => inWindow(d.dayAt, p.lo, p.hi)).length)
        : section === "sleep"
          ? presets.map((p) => (sleep ?? []).filter((s) => inWindow(s.startAt, p.lo, p.hi)).length)
          : presets.map((p) => baseFiltered.filter((w) => inWindow(w.startAt, p.lo, p.hi)).length),
    [section, presets, baseFiltered, days, sleep],
  );
  const filtered = useMemo(() => {
    const inRange = baseFiltered.filter((w) => inWindow(w.startAt, range.lo, range.hi));
    return sortItems(
      inRange,
      (w) =>
        sort.by === "duration"
          ? (w.durationS ?? 0)
          : sort.by === "distance"
            ? (w.distanceM ?? 0)
            : w.startAt,
      sort.desc,
    );
  }, [baseFiltered, range, sort]);
  const filteredDays = useMemo(() => {
    if (section !== "daily") return [];
    const inRange = (days ?? []).filter((d) => inWindow(d.dayAt, range.lo, range.hi));
    return sortItems(
      inRange,
      (d) =>
        daySort.by === "steps"
          ? (d.steps ?? 0)
          : daySort.by === "distance"
            ? (d.distanceM ?? 0)
            : d.dayAt,
      daySort.desc,
    );
  }, [section, days, range, daySort]);
  const filteredSleep = useMemo(() => {
    if (section !== "sleep") return [];
    const inRange = (sleep ?? []).filter((s) => inWindow(s.startAt, range.lo, range.hi));
    return sortItems(
      inRange,
      (s) =>
        sleepSort.by === "duration"
          ? s.startAt != null && s.endAt != null
            ? s.endAt - s.startAt
            : 0
          : s.startAt,
      sleepSort.desc,
    );
  }, [section, sleep, range, sleepSort]);

  const hasWorkouts = (workouts?.length ?? 0) > 0;
  const hasDays = (days?.length ?? 0) > 0;
  const hasSleep = (sleep?.length ?? 0) > 0;
  const hasAny = hasWorkouts || hasDays || hasSleep;
  const filterGroups = useMemo<FilterGroup[]>(() => {
    if (!hasAny) return [];
    const out: FilterGroup[] = [
      badgeGroup({
        key: "section",
        label: "Section",
        description: "Workout log or per-day activity totals",
        options: SECTIONS.map((s) => ({
          value: s.value,
          label: s.label,
          count:
            s.value === "workouts"
              ? workouts?.length
              : s.value === "daily"
                ? days?.length
                : sleep?.length,
        })),
        value: section,
        onChange: (v) => setSection(v as HealthSection),
      }),
    ];
    if (section === "workouts" && activities.length > 1) {
      const activityOptions: BadgeFilterOption[] = [
        { value: "all", label: "All", count: workouts?.length },
        ...activities.map((a) => ({ value: a, label: a, count: (workouts ?? []).filter((w) => w.activity === a).length })),
      ];
      out.push(badgeGroup({ key: "activity", label: "Activity", description: "Type of workout", options: activityOptions, value: effActivity, onChange: setActivity }));
    }
    out.push(
      timeGroup({
        description:
          section === "daily"
            ? "The day the activity was recorded"
            : section === "sleep"
              ? "When the sleep was recorded"
              : "When the workout took place",
        presets,
        counts: presetCounts,
        value: range,
        onChange: setRange,
      }),
    );
    return out;
  }, [hasAny, workouts, days, sleep, section, setSection, activities, effActivity, presets, presetCounts, range, setActivity, setRange]);
  const sortNode = useMemo(() => {
    if (!hasAny) return undefined;
    return section === "daily" ? (
      <SortControl
        fields={[
          { value: "date", label: "Date" },
          { value: "steps", label: "Steps" },
          { value: "distance", label: "Distance" },
        ]}
        value={daySort}
        onChange={setDaySort}
      />
    ) : section === "sleep" ? (
      <SortControl
        fields={[
          { value: "date", label: "Date" },
          { value: "duration", label: "Duration" },
        ]}
        value={sleepSort}
        onChange={setSleepSort}
      />
    ) : (
      <SortControl
        fields={[
          { value: "date", label: "Date" },
          { value: "duration", label: "Duration" },
          { value: "distance", label: "Distance" },
        ]}
        value={sort}
        onChange={setSort}
      />
    );
  }, [hasAny, section, sort, setSort, daySort, setDaySort]);
  const toolbar = useMemo(
    () =>
      active === true
        ? {
            title: "Health",
            count: hasAny
              ? section === "daily"
                ? filteredDays.length
                : section === "sleep"
                  ? filteredSleep.length
                  : filtered.length
              : undefined,
            filter: filterGroups,
            sort: sortNode,
          }
        : null,
    [active, hasAny, section, filtered.length, filteredDays.length, filteredSleep.length, filterGroups, sortNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <EmptyView
        icon={HeartPulse}
        title="No backup open"
        description="Import a backup to see health data."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const summaryStrip = summary && summary.sampleCount > 0 && (
    <div className="border-b px-4 py-3 text-sm text-muted-foreground">
      <span className="font-medium text-foreground">
        {formatCount(summary.sampleCount)}
      </span>{" "}
      health samples
      {summary.firstAt != null && summary.lastAt != null && (
        <>
          {" · "}
          {formatDate(summary.firstAt)} – {formatDate(summary.lastAt)}
        </>
      )}
      {summary.workoutCount > 0 &&
        ` · ${formatCount(summary.workoutCount)} workouts`}
    </div>
  );

  if (section === "sleep") {
    return (
      <div className="flex h-full flex-col">
        {summaryStrip}
        <div key={clockFormat} className="min-h-0 flex-1">
          <VirtualListView<SleepSession>
            items={filteredSleep}
            getKey={(s) => s.id}
            estimateSize={56}
            isPending={sleepPending}
            error={sleepError}
            emptyIcon={Moon}
            emptyMessage={
              hasSleep
                ? "No sleep sessions match these filters."
                : "No sleep data indexed — re-import this backup to index it."
            }
            renderItem={(s) => <SleepRow session={s} />}
          />
        </div>
      </div>
    );
  }

  if (section === "daily") {
    return (
      <div className="flex h-full flex-col">
        {summaryStrip}
        <div key={clockFormat} className="min-h-0 flex-1">
          <VirtualListView<HealthDay>
            items={filteredDays}
            getKey={(d) => d.dayAt}
            estimateSize={56}
            isPending={daysPending}
            error={daysError}
            emptyIcon={Footprints}
            emptyMessage={
              hasDays
                ? "No days match these filters."
                : "No daily activity indexed — re-import this backup to index it."
            }
            renderItem={(d) => <DayRow day={d} />}
          />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {error ? (
        <ErrorState error={error} />
      ) : isPending ? (
        <ListSkeleton />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          {summaryStrip}
          {!hasWorkouts ? (
            <p className="p-6 text-center text-sm text-muted-foreground">
              No health data in this backup.
            </p>
          ) : filtered.length === 0 ? (
            <p className="p-6 text-center text-sm text-muted-foreground">
              No workouts match these filters.
            </p>
          ) : (
            <div key={clockFormat} className="w-full">
              <h3 className="px-4 pb-1 pt-3 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                Workouts
              </h3>
              {filtered.map((w) => (
                <WorkoutRow key={w.id} workout={w} />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
