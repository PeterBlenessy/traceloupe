import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Activity, HeartPulse } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { BadgeFilter, type BadgeFilterOption } from "@/components/badge-filter";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { TimeFilterBar, useTimePresets } from "@/components/time-filter";
import { usePersistedState } from "@/lib/use-persisted-state";
import { EmptyView, ErrorState, PanelHeader } from "@/components/view";
import { formatCount, formatDate, formatDateTime, formatDuration } from "@/lib/format";
import { client, type Workout, type TimeRange } from "@/lib/ipc";

/** Metres → a compact "5.2 km" / "820 m". */
function formatDistance(m: number | null): string | null {
  if (m == null || m <= 0) return null;
  return m >= 1000 ? `${(m / 1000).toFixed(2)} km` : `${Math.round(m)} m`;
}

function WorkoutRow({ workout }: { workout: Workout }) {
  const bits = [
    formatDuration(workout.durationS),
    formatDistance(workout.distanceM),
  ].filter(Boolean);
  return (
    <div className="px-2 py-0.5">
      <div className="flex items-center gap-3 rounded-md border px-3 py-2.5">
        <Activity className="size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium">
            {workout.activity ?? "Workout"}
          </div>
          <div className="text-xs text-muted-foreground">
            {workout.startAt != null ? formatDateTime(workout.startAt) : "—"}
            {bits.length > 0 && ` · ${bits.join(" · ")}`}
          </div>
        </div>
      </div>
    </div>
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

  const [activity, setActivity] = usePersistedState<string>("health:activity", "all");
  const [sort, setSort] = usePersistedState<SortState>("health:sort", {
    by: "date",
    desc: true,
  });
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });

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
      (workouts ?? []).filter(
        (w) => effActivity === "all" || w.activity === effActivity,
      ),
    [workouts, effActivity],
  );
  const presetCounts = useMemo(
    () => presets.map((p) => baseFiltered.filter((w) => inWindow(w.startAt, p.lo, p.hi)).length),
    [presets, baseFiltered],
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

  const hasWorkouts = (workouts?.length ?? 0) > 0;
  const activityOptions: BadgeFilterOption[] = [
    { value: "all", label: "All", count: workouts?.length },
    ...activities.map((a) => ({
      value: a,
      label: a,
      count: (workouts ?? []).filter((w) => w.activity === a).length,
    })),
  ];

  return (
    <div className="flex h-full flex-col">
      <PanelHeader
        title="Health"
        count={hasWorkouts ? filtered.length : undefined}
        toolbar={
          hasWorkouts ? (
            <>
              {/* Time range fills the left; the facet + sort sit at the right. */}
              <TimeFilterBar
                className="flex-1"
                presets={presets}
                value={range}
                onChange={setRange}
                counts={presetCounts}
              />
              {activities.length > 1 && (
                <BadgeFilter options={activityOptions} value={activity} onChange={setActivity} />
              )}
              <SortControl
                fields={[
                  { value: "date", label: "Date" },
                  { value: "duration", label: "Duration" },
                  { value: "distance", label: "Distance" },
                ]}
                value={sort}
                onChange={setSort}
              />
            </>
          ) : undefined
        }
      />
      {error ? (
        <ErrorState error={error} />
      ) : isPending ? (
        <div className="w-full p-2">
          {Array.from({ length: 8 }).map((_, i) => (
            <Skeleton key={i} className="mb-1 h-14 w-full" />
          ))}
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          {summary && summary.sampleCount > 0 && (
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
          )}
          {!hasWorkouts ? (
            <p className="p-6 text-center text-sm text-muted-foreground">
              No health data in this backup.
            </p>
          ) : filtered.length === 0 ? (
            <p className="p-6 text-center text-sm text-muted-foreground">
              No workouts match these filters.
            </p>
          ) : (
            <div className="w-full">
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
