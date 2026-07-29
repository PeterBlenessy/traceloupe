import { useMemo, useState, type ComponentType, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Activity, ChevronDown, Droplet, Footprints, Globe, HeartPulse, MapPin, Moon, Trophy } from "lucide-react";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import {
  badgeGroup,
  multiBadgeGroup,
  timeGroup,
  type FilterGroup,
} from "@/components/filter-groups";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useEncryptedOnlyEmpty } from "@/lib/use-encrypted-only";
import { useSettings } from "@/components/settings-provider";
import { NoBackupState, VirtualListView } from "@/components/view";
import { dateFormat, formatCount, formatDate, formatDateTime, formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { modelName } from "@/lib/device-names";
import {
  client,
  type CycleEntry,
  type HealthAchievement,
  type HealthDay,
  type HealthTimezone,
  type SleepSession,
  type Workout,
  type TimeRange,
} from "@/lib/ipc";

/** The Health data sections, selectable via the Section filter. */
type HealthSection =
  | "workouts"
  | "daily"
  | "sleep"
  | "timezones"
  | "awards"
  | "cycle";
const SECTIONS: { value: HealthSection; label: string }[] = [
  { value: "workouts", label: "Workouts" },
  { value: "daily", label: "Daily activity" },
  { value: "sleep", label: "Sleep" },
  { value: "timezones", label: "Timezones" },
  { value: "awards", label: "Awards" },
  { value: "cycle", label: "Cycle Tracking" },
];

/** Metres → a compact "5.2 km" / "820 m". */
function formatDistance(m: number | null): string | null {
  if (m == null || m <= 0) return null;
  return m >= 1000 ? `${(m / 1000).toFixed(2)} km` : `${Math.round(m)} m`;
}

/** Days are aggregated per UTC day at import; format the label in UTC so the
 *  local timezone can't shift it onto a neighbouring date. */
function formatDayUTC(at: number): string {
  return dateFormat({
    timeZone: "UTC",
    weekday: "short",
    year: "numeric",
    month: "short",
    day: "numeric",
  }).format(new Date(at * 1000));
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
        <Tooltip>
          <TooltipTrigger asChild>
            <Item asChild className="rounded-md transition-colors hover:bg-accent/50">
              <button
                type="button"
                onClick={() => setExpanded((e) => !e)}
                className="w-full text-left"
              >
                {inner}
              </button>
            </Item>
          </TooltipTrigger>
          <TooltipContent>{expanded ? "Hide route" : "Show route"}</TooltipContent>
        </Tooltip>
      ) : (
        <Item>{inner}</Item>
      )}
      {expanded && <RoutePreview workoutId={workout.id} />}
    </div>
  );
}

/** "412/500 kcal" — a ring's progress against its goal (goal optional). */
function ringBit(
  label: string,
  value: number | null,
  goal: number | null,
  unit: string,
): string | null {
  if (value == null) return null;
  const v = formatCount(Math.round(value));
  return goal != null
    ? `${label} ${v}/${formatCount(Math.round(goal))} ${unit}`
    : `${label} ${v} ${unit}`;
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
  const rings = [
    ringBit("Move", day.moveKcal, day.moveGoalKcal, "kcal"),
    ringBit("Exercise", day.exerciseMin, day.exerciseGoalMin, "min"),
    ringBit("Stand", day.standHours, day.standGoalHours, "hr"),
  ].filter(Boolean);
  const mobility = [
    day.walkSpeedMs != null ? `walk ${day.walkSpeedMs.toFixed(1)} m/s` : null,
    day.stepLengthM != null ? `step ${day.stepLengthM.toFixed(2)} m` : null,
    day.doubleSupportPct != null
      ? `support ${Math.round(day.doubleSupportPct * 100)}%`
      : null,
    day.walkAsymmetryPct != null
      ? `asym ${Math.round(day.walkAsymmetryPct * 100)}%`
      : null,
    day.audioDbMax != null ? `audio ≤${Math.round(day.audioDbMax)} dB` : null,
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
        {(rings.length > 0 || mobility.length > 0) && (
          <div className="text-xs text-muted-foreground/70">
            {[...rings, ...mobility].join(" · ")}
          </div>
        )}
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

/** "MonthlyChallengeTypeDaysDoublingMoveGoal" → "Monthly Challenge Type Days
 *  Doubling Move Goal" — split the CamelCase template id for display. */
function humanizeAward(name: string): string {
  return name
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .replace(/([a-zA-Z])(\d)/g, "$1 $2");
}

function AchievementRow({ award }: { award: HealthAchievement }) {
  const value =
    award.value != null
      ? `${formatCount(Math.round(award.value))}${award.unit && award.unit !== "count" ? ` ${award.unit}` : "×"}`
      : null;
  return (
    <Item>
      <ItemMedia>
        <Trophy className="size-4 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{humanizeAward(award.name)}</ItemTitle>
        {value && (
          <div className="text-xs text-muted-foreground">{value}</div>
        )}
      </ItemContent>
      {award.earnedAt != null && (
        <div className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
          {formatDayUTC(award.earnedAt)}
        </div>
      )}
    </Item>
  );
}

function CycleRow({ entry }: { entry: CycleEntry }) {
  return (
    <Item>
      <ItemMedia>
        <Droplet className="size-4 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">
          {entry.category}
          {entry.detail && (
            <span className="text-muted-foreground"> · {entry.detail}</span>
          )}
        </ItemTitle>
      </ItemContent>
      {entry.loggedAt != null && (
        // Local date to match the time-filter windowing (raw instant).
        <div className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
          {formatDate(entry.loggedAt)}
        </div>
      )}
    </Item>
  );
}

function TimezoneRow({ tz }: { tz: HealthTimezone }) {
  const devices = tz.devices.map((d) => modelName(d) ?? d).join(", ");
  return (
    <Item>
      <ItemMedia>
        <Globe className="size-4 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{tz.tzName.replace(/_/g, " ")}</ItemTitle>
        <div className="truncate text-xs text-muted-foreground">
          {formatCount(tz.samples)} samples
          {devices && ` · ${devices}`}
        </div>
      </ItemContent>
      {tz.firstAt != null && tz.lastAt != null && (
        <div className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
          {formatDate(tz.firstAt)} – {formatDate(tz.lastAt)}
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

/** `dayAt` is midnight UTC of an aggregated day, but the time presets compute
 *  their bounds at LOCAL midnight — comparing them directly shifts days across
 *  preset edges for any non-UTC timezone. Convert to the local-midnight
 *  instant of the same calendar date before windowing. */
function localDayStart(dayAt: number): number {
  return dayAt + new Date(dayAt * 1000).getTimezoneOffset() * 60;
}

/** Everything section-specific, so filtering, windowing, sorting, counting and
 *  rendering all run through ONE pipeline. Adding a section = adding an entry;
 *  there is no per-section branch to forget elsewhere. */
interface SectionDef<T = unknown> {
  /** The section's list (undefined until its gated query has run). */
  items: T[] | undefined;
  pending: boolean;
  error: unknown;
  /** Section-badge count — summary-based so gated lists show it unfetched. */
  count: number | undefined;
  /** Timestamp compared against the time-filter window. */
  windowAt(item: T): number | null;
  sortFields: { value: string; label: string }[];
  sortKey(item: T, by: string): number | string | null;
  sort: SortState;
  setSort(s: SortState): void;
  timeDescription: string;
  emptyIcon: ComponentType<{ className?: string }>;
  /** Message when the list itself is empty / when filters exclude everything. */
  emptyAll: string;
  emptyFiltered: string;
  /** Rows render time-of-day, so remount them when the clock format changes. */
  clockSensitive: boolean;
  render(item: T): ReactNode;
  rowKey(item: T): React.Key;
}

/** Identity helper so each entry infers its own item type while the record
 *  stores the erased form. */
function defineSection<T>(def: SectionDef<T>): SectionDef {
  return def as SectionDef;
}

export function HealthView() {
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  // Sections are MULTI-select with All as the default. They read as a filter —
  // pills in the filter popover, like every other view — so behaving like a
  // mode switch that could only ever show one thing, with no way back to
  // everything, was the bug (#162).
  //
  // The key changed with the shape: a persisted single value would deserialize
  // into `sections.includes` and silently match nothing.
  const [sections, setSections] = usePersistedState<HealthSection[]>(
    "health:sections",
    [],
  );
  /** Empty selection means "everything present" — so a new section appears
   *  without anyone having to re-tick it. */
  const selected = useMemo(
    () => (sections.length > 0 ? sections : SECTIONS.map((x) => x.value)),
    [sections],
  );
  const isOn = (v: HealthSection) => selected.includes(v);
  const toggleSection = (v: string) => {
    const value = v as HealthSection;
    const next = selected.includes(value)
      ? selected.filter((x) => x !== value)
      : [...selected, value];
    // Deselecting the last one would show nothing at all; treat "none" as
    // "everything", which is also what an untouched filter means.
    setSections(next.length === 0 || next.length === SECTIONS.length ? [] : next);
  };
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
  // The daily/sleep lists are only fetched for their visible section — the
  // summary's dayCount/sleepCount covers the section badges, so mounting on
  // Workouts doesn't materialize thousands of unrendered rows.
  const {
    data: days,
    isPending: daysPending,
    error: daysError,
  } = useQuery({
    queryKey: ["healthDaily"],
    queryFn: () => client.healthDaily(),
    enabled: active === true && isOn("daily"),
  });
  const {
    data: sleep,
    isPending: sleepPending,
    error: sleepError,
  } = useQuery({
    queryKey: ["healthSleep"],
    queryFn: () => client.listSleep(),
    enabled: active === true && isOn("sleep"),
  });
  const {
    data: timezones,
    isPending: tzPending,
    error: tzError,
  } = useQuery({
    queryKey: ["healthTimezones"],
    queryFn: () => client.listHealthTimezones(),
    enabled: active === true && isOn("timezones"),
  });
  const {
    data: awards,
    isPending: awardsPending,
    error: awardsError,
  } = useQuery({
    queryKey: ["healthAwards"],
    queryFn: () => client.listHealthAchievements(),
    enabled: active === true && isOn("awards"),
  });
  const {
    data: cycle,
    isPending: cyclePending,
    error: cycleError,
  } = useQuery({
    queryKey: ["healthCycle"],
    queryFn: () => client.listCycle(),
    enabled: active === true && isOn("cycle"),
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
  const [tzSort, setTzSort] = usePersistedState<SortState>("health:tzSort", {
    by: "samples",
    desc: true,
  });
  const [awardSort, setAwardSort] = usePersistedState<SortState>("health:awardSort", {
    by: "date",
    desc: true,
  });
  const [cycleSort, setCycleSort] = usePersistedState<SortState>("health:cycleSort", {
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

  // Presence comes from the summary (always fetched), not the gated lists —
  // otherwise the Section filter would vanish until each list was visited.
  const hasWorkouts = (workouts?.length ?? 0) > 0;
  const hasDays = (summary?.dayCount ?? 0) > 0;
  const hasSleep = (summary?.sleepCount ?? 0) > 0;
  const hasTz = (summary?.timezoneCount ?? 0) > 0;
  const hasAwards = (summary?.achievementCount ?? 0) > 0;
  const hasCycle = (summary?.cycleCount ?? 0) > 0;
  const hasAny = hasWorkouts || hasDays || hasSleep || hasTz || hasAwards || hasCycle;

  // One descriptor per section; everything below (windowing, sorting, counts,
  // toolbar, rendering) is section-agnostic.
  const defs = useMemo<Record<HealthSection, SectionDef>>(
    () => ({
      workouts: defineSection<Workout>({
        items:
          workouts &&
          workouts.filter((w) => effActivity === "all" || w.activity === effActivity),
        pending: isPending,
        error,
        count: workouts?.length,
        windowAt: (w) => w.startAt,
        sortFields: [
          { value: "date", label: "Date" },
          { value: "duration", label: "Duration" },
          { value: "distance", label: "Distance" },
        ],
        sortKey: (w, by) =>
          by === "duration"
            ? (w.durationS ?? 0)
            : by === "distance"
              ? (w.distanceM ?? 0)
              : w.startAt,
        sort,
        setSort,
        timeDescription: "When the workout took place",
        emptyIcon: Activity,
        emptyAll: hasAny
          ? "No workouts in this backup — switch Section to see daily activity or sleep."
          : "No health data in this backup.",
        emptyFiltered: "No workouts match these filters.",
        clockSensitive: true,
        render: (w) => <WorkoutRow workout={w} />,
        rowKey: (w) => w.id,
      }),
      daily: defineSection<HealthDay>({
        items: days,
        pending: daysPending,
        error: daysError,
        count: summary?.dayCount,
        // Compare via the local-midnight instant of the row's calendar date.
        windowAt: (d) => localDayStart(d.dayAt),
        sortFields: [
          { value: "date", label: "Date" },
          { value: "steps", label: "Steps" },
          { value: "distance", label: "Distance" },
        ],
        sortKey: (d, by) =>
          by === "steps"
            ? (d.steps ?? 0)
            : by === "distance"
              ? (d.distanceM ?? 0)
              : d.dayAt,
        sort: daySort,
        setSort: setDaySort,
        timeDescription: "The day the activity was recorded",
        emptyIcon: Footprints,
        emptyAll: "No daily activity indexed — re-import this backup to index it.",
        emptyFiltered: "No days match these filters.",
        // DayRow renders no time-of-day (formatDayUTC is date-only), so a
        // clock-preference change must not reset the list.
        clockSensitive: false,
        render: (d) => <DayRow day={d} />,
        rowKey: (d) => d.dayAt,
      }),
      sleep: defineSection<SleepSession>({
        items: sleep,
        pending: sleepPending,
        error: sleepError,
        count: summary?.sleepCount,
        windowAt: (s) => s.startAt,
        sortFields: [
          { value: "date", label: "Date" },
          { value: "duration", label: "Duration" },
        ],
        sortKey: (s, by) =>
          by === "duration"
            ? s.startAt != null && s.endAt != null
              ? s.endAt - s.startAt
              : 0
            : s.startAt,
        sort: sleepSort,
        setSort: setSleepSort,
        timeDescription: "When the sleep was recorded",
        emptyIcon: Moon,
        emptyAll: "No sleep data indexed — re-import this backup to index it.",
        emptyFiltered: "No sleep sessions match these filters.",
        clockSensitive: true,
        render: (s) => <SleepRow session={s} />,
        rowKey: (s) => s.id,
      }),
      timezones: defineSection<HealthTimezone>({
        items: timezones,
        pending: tzPending,
        error: tzError,
        count: summary?.timezoneCount,
        // A span, not an instant — window on when the tz was last recorded.
        windowAt: (t) => t.lastAt,
        sortFields: [
          { value: "samples", label: "Samples" },
          { value: "date", label: "Last seen" },
        ],
        sortKey: (t, by) => (by === "date" ? t.lastAt : t.samples),
        sort: tzSort,
        setSort: setTzSort,
        timeDescription: "When the timezone was last recorded",
        emptyIcon: Globe,
        emptyAll: "No timezone history indexed — re-import this backup to index it.",
        emptyFiltered: "No timezones match these filters.",
        clockSensitive: false,
        render: (t) => <TimezoneRow tz={t} />,
        rowKey: (t) => t.tzName,
      }),
      awards: defineSection<HealthAchievement>({
        items: awards,
        pending: awardsPending,
        error: awardsError,
        count: summary?.achievementCount,
        windowAt: (a) => (a.earnedAt != null ? localDayStart(a.earnedAt) : null),
        sortFields: [
          { value: "date", label: "Date" },
          { value: "name", label: "Name" },
        ],
        sortKey: (a, by) => (by === "name" ? a.name : a.earnedAt),
        sort: awardSort,
        setSort: setAwardSort,
        timeDescription: "When the award was earned",
        emptyIcon: Trophy,
        emptyAll: "No awards indexed — re-import this backup to index it.",
        emptyFiltered: "No awards match these filters.",
        clockSensitive: false,
        render: (a) => <AchievementRow award={a} />,
        rowKey: (a) => a.id,
      }),
      cycle: defineSection<CycleEntry>({
        items: cycle,
        pending: cyclePending,
        error: cycleError,
        count: summary?.cycleCount,
        // A real logged instant (not a midnight-UTC aggregated day), so window
        // on the raw timestamp like workouts/sleep — no localDayStart shift.
        windowAt: (e) => e.loggedAt,
        sortFields: [
          { value: "date", label: "Date" },
          { value: "name", label: "Type" },
        ],
        sortKey: (e, by) => (by === "name" ? e.category : e.loggedAt),
        sort: cycleSort,
        setSort: setCycleSort,
        timeDescription: "When the entry was logged",
        emptyIcon: Droplet,
        emptyAll: "No cycle-tracking data indexed — re-import this backup to index it.",
        emptyFiltered: "No cycle-tracking entries match these filters.",
        clockSensitive: false,
        render: (e) => <CycleRow entry={e} />,
        rowKey: (e) => e.id,
      }),
    }),
    [
      workouts, isPending, error, effActivity, sort, setSort,
      days, daysPending, daysError, daySort, setDaySort,
      sleep, sleepPending, sleepError, sleepSort, setSleepSort,
      timezones, tzPending, tzError, tzSort, setTzSort,
      awards, awardsPending, awardsError, awardSort, setAwardSort,
      cycle, cyclePending, cycleError, cycleSort, setCycleSort,
      summary, hasAny,
    ],
  );
  // One list across every selected section. Each row remembers which section it
  // came from, so rendering, sorting and the time window still go through that
  // section's own descriptor — the sections stay section-agnostic, they just
  // stop being mutually exclusive.
  type Row = { section: HealthSection; item: unknown };
  const only = selected.length === 1 ? defs[selected[0]] : undefined;
  const baseItems = useMemo<Row[]>(
    () =>
      selected.flatMap((k) =>
        (defs[k].items ?? []).map((item) => ({ section: k, item })),
      ),
    [selected, defs],
  );
  const pending = selected.some((k) => defs[k].pending);
  const anyError = selected.map((k) => defs[k].error).find(Boolean);
  const presetCounts = useMemo(
    () =>
      presets.map(
        (p) =>
          baseItems.filter((r) =>
            inWindow(defs[r.section].windowAt(r.item), p.lo, p.hi),
          ).length,
      ),
    [presets, baseItems, defs],
  );
  const shown = useMemo(
    () =>
      sortItems(
        baseItems.filter((r) =>
          inWindow(defs[r.section].windowAt(r.item), range.lo, range.hi),
        ),
        // Across several sections only time is comparable — a workout's
        // "distance" means nothing to a sleep session. One section keeps its own
        // sort fields, so single-section behaviour is exactly as it was.
        (r) =>
          only
            ? only.sortKey(r.item, only.sort.by)
            : defs[r.section].windowAt(r.item),
        // Newest first across sections — the only ordering that means the same
        // thing for a workout, a night's sleep and an award.
        only ? only.sort.desc : true,
      ),
    [baseItems, range, defs, only],
  );

  const filterGroups = useMemo<FilterGroup[]>(() => {
    if (!hasAny) return [];
    const out: FilterGroup[] = [
      multiBadgeGroup({
        key: "section",
        label: "Section",
        description: "Which kinds of Health data to list",
        // Only what this backup actually has: a pill for an absent section is
        // an invitation to an empty list.
        options: SECTIONS.filter((x) => (defs[x.value].count ?? 0) > 0).map((x) => ({
          value: x.value,
          label: x.label,
          count: defs[x.value].count,
        })),
        selected,
        onToggle: toggleSection,
      }),
    ];
    if (isOn("workouts") && activities.length > 1) {
      const activityOptions: BadgeFilterOption[] = [
        { value: "all", label: "All", count: workouts?.length },
        ...activities.map((a) => ({ value: a, label: a, count: (workouts ?? []).filter((w) => w.activity === a).length })),
      ];
      out.push(badgeGroup({ key: "activity", label: "Activity", description: "Type of workout", options: activityOptions, value: effActivity, onChange: setActivity }));
    }
    out.push(
      timeGroup({
        description: only ? only.timeDescription : "When it happened",
        presets,
        counts: presetCounts,
        value: range,
        onChange: setRange,
      }),
    );
    return out;
  }, [hasAny, defs, only, selected, workouts, activities, effActivity, presets, presetCounts, range, setActivity, setRange, toggleSection, isOn]);
  const sortNode = useMemo(
    () =>
      // Only one section's fields make sense at a time; across several, the
      // list is time-ordered and there is nothing to choose between.
      hasAny && only ? (
        <SortControl fields={only.sortFields} value={only.sort} onChange={only.setSort} />
      ) : undefined,
    [hasAny, only],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? {
            title: "Health",
            count: hasAny ? shown.length : undefined,
            filter: filterGroups,
            sort: sortNode,
          }
        : null,
    [active, hasAny, shown.length, filterGroups, sortNode],
  );
  useViewToolbar(toolbar);

  // Health and MedicalID are encrypted-backup-only, so an unencrypted backup
  // has no Health data at all — which must not read as "this person recorded
  // none". See use-encrypted-only.ts.
  const noHealthMessage = useEncryptedOnlyEmpty(
    "Health data",
    "No Health data in the selected sections.",
  );

  if (active === false) {
    return (
      <NoBackupState
        icon={HeartPulse}
        title="Explore health data"
        lead="The Health app's records — workouts, daily activity, sleep, awards, and more — summarized and charted over time. Requires an encrypted backup, the only kind that includes Health data."
        features={[
          { label: "Sections", detail: "Switch between Workouts, Daily activity, Sleep, Timezones, Awards, and Cycle tracking." },
          { label: "Filters", detail: "Filter by activity type and time range within each section." },
          { label: "Sort", detail: "Order by date, duration, distance, steps, and more." },
          { label: "Rich detail", detail: "Workout GPS route maps, activity rings, heart-rate ranges, and sleep stages." },
        ]}
        note="Everything is processed locally on this Mac."
      />
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

  return (
    <div className="flex h-full flex-col">
      {summaryStrip}
      <div
        // Remount on section change; clock-sensitive sections also remount
        // when the 12h/24h preference flips so times re-format.
        key={`${selected.join(",")}:${selected.some((k) => defs[k].clockSensitive) ? clockFormat : ""}`}
        className="min-h-0 flex-1"
      >
        <VirtualListView
          items={shown}
          getKey={(r) => `${r.section}:${defs[r.section].rowKey(r.item)}`}
          estimateSize={56}
          isPending={pending}
          error={anyError}
          emptyIcon={only ? only.emptyIcon : HeartPulse}
          emptyMessage={
            baseItems.length > 0
              ? (only?.emptyFiltered ?? "Nothing in the selected sections matches these filters.")
              : (only?.emptyAll ?? noHealthMessage)
          }
          renderItem={(r) => defs[r.section].render(r.item)}
        />
      </div>
    </div>
  );
}
