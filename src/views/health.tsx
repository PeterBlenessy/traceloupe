import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Activity, HeartPulse } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyView, ViewHeader } from "@/components/view";
import { formatCount, formatDate, formatDateTime, formatDuration } from "@/lib/format";
import { client, type Workout } from "@/lib/ipc";

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

export function HealthView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: workouts } = useQuery({
    queryKey: ["workouts"],
    queryFn: () => client.listWorkouts(),
    enabled: active === true,
  });
  const { data: summary } = useQuery({
    queryKey: ["healthSummary"],
    queryFn: () => client.healthSummary(),
    enabled: active === true,
  });

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

  const hasHealth =
    (summary?.sampleCount ?? 0) > 0 || (workouts?.length ?? 0) > 0;

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Health" count={workouts?.length} />
      <div className="min-h-0 flex-1 overflow-auto">
        <div className="mx-auto max-w-2xl">
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
          {!hasHealth ? (
            <p className="px-4 py-6 text-sm text-muted-foreground">
              No health data in this backup.
            </p>
          ) : (
            <div className="h-full">
              {(workouts?.length ?? 0) > 0 && (
                <>
                  <h3 className="px-4 pb-1 pt-3 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    Workouts
                  </h3>
                  {workouts!.map((w) => (
                    <WorkoutRow key={w.id} workout={w} />
                  ))}
                </>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
