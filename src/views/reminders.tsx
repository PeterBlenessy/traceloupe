import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Circle, CircleCheck, Flag, ListTodo } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useDebounced } from "@/lib/use-debounced";
import { EmptyView, ListSearch, VirtualListView } from "@/components/view";
import { formatDate } from "@/lib/format";
import { cn } from "@/lib/utils";
import { client, type Reminder, type TimeRange } from "@/lib/ipc";

/** True when `at` falls in a half-open [lo, hi) window; undated only pass "All". */
function inWindow(at: number | null, lo: number | null, hi: number | null) {
  if (lo == null && hi == null) return true;
  if (at == null) return false;
  return (lo == null || at >= lo) && (hi == null || at < hi);
}

function ReminderRow({ reminder }: { reminder: Reminder }) {
  return (
    <div
      data-slot="list-row"
      className="flex items-start gap-2.5 rounded-md px-3 py-2.5"
    >
      {reminder.completed ? (
        <CircleCheck className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      ) : (
        <Circle className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline justify-between gap-2">
          <span
            className={cn(
              "truncate",
              reminder.completed && "text-muted-foreground line-through",
            )}
          >
            {reminder.title ?? "(untitled)"}
            {reminder.flagged && (
              <Flag className="ml-1.5 inline size-3 fill-orange-500 text-orange-500" />
            )}
          </span>
          {reminder.listName && (
            <Badge variant="secondary" className="shrink-0">
              {reminder.listName}
            </Badge>
          )}
        </div>
        {reminder.notes && (
          <p className="mt-0.5 select-text whitespace-pre-wrap break-words text-sm text-muted-foreground">
            {reminder.notes}
          </p>
        )}
        {(reminder.dueAt != null || reminder.completedAt != null) && (
          <div className="mt-0.5 text-xs text-muted-foreground">
            {reminder.completed && reminder.completedAt != null
              ? `Completed ${formatDate(reminder.completedAt)}`
              : reminder.dueAt != null
                ? `Due ${formatDate(reminder.dueAt)}`
                : null}
          </div>
        )}
      </div>
    </div>
  );
}

export function RemindersView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: reminders,
    isPending,
    error,
  } = useQuery({
    queryKey: ["reminders"],
    queryFn: () => client.listReminders(),
    enabled: active === true,
  });

  const [status, setStatus] = usePersistedState<string>("reminders:status", "all");
  const [list, setList] = usePersistedState<string>("reminders:list", "all");
  const [sort, setSort] = usePersistedState<SortState>("reminders:sort", {
    by: "title",
    desc: false,
  });
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim().toLowerCase());
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });

  const lists = useMemo(
    () =>
      Array.from(
        new Set(
          (reminders ?? []).map((r) => r.listName).filter((l): l is string => !!l),
        ),
      ).sort((a, b) => a.localeCompare(b)),
    [reminders],
  );
  const effList = list !== "all" && lists.includes(list) ? list : "all";
  const hasFlagged = useMemo(
    () => (reminders ?? []).some((r) => r.flagged),
    [reminders],
  );
  // Clamp a stale "flagged" status when this backup has no flagged reminders
  // (the chip is hidden), so the list can't silently empty with no way to reset.
  const effStatus = status === "flagged" && !hasFlagged ? "all" : status;

  // Status + list + search filtered (base for the created-date chip counts).
  const baseFiltered = useMemo(() => {
    return (reminders ?? []).filter((r) => {
      if (effStatus === "open" && r.completed) return false;
      if (effStatus === "completed" && !r.completed) return false;
      if (effStatus === "flagged" && !r.flagged) return false;
      if (effList !== "all" && r.listName !== effList) return false;
      if (search) {
        const hay = [r.title, r.notes, r.listName]
          .filter(Boolean)
          .join(" ")
          .toLowerCase();
        if (!hay.includes(search)) return false;
      }
      return true;
    });
  }, [reminders, effStatus, effList, search]);

  const presetCounts = useMemo(
    () => presets.map((p) => baseFiltered.filter((r) => inWindow(r.createdAt, p.lo, p.hi)).length),
    [presets, baseFiltered],
  );

  const filtered = useMemo(() => {
    const inRange = baseFiltered.filter((r) => inWindow(r.createdAt, range.lo, range.hi));
    return sortItems(
      inRange,
      (r) =>
        sort.by === "due"
          ? r.dueAt
          : sort.by === "created"
            ? r.createdAt
            : (r.title ?? "").toLowerCase(),
      sort.desc,
    );
  }, [baseFiltered, range, sort]);

  const hasReminders = (reminders?.length ?? 0) > 0;
  const filterGroups = useMemo<FilterGroup[]>(() => {
    if (!hasReminders) return [];
    const openCount = (reminders ?? []).filter((r) => !r.completed).length;
    const doneCount = (reminders?.length ?? 0) - openCount;
    const statusOptions: BadgeFilterOption[] = [
      { value: "all", label: "All", count: reminders?.length },
      { value: "open", label: "Open", count: openCount },
      { value: "completed", label: "Completed", count: doneCount },
      ...(hasFlagged
        ? [{ value: "flagged", label: "Flagged", count: (reminders ?? []).filter((r) => r.flagged).length }]
        : []),
    ];
    const listOptions: BadgeFilterOption[] = [
      { value: "all", label: "All lists" },
      ...lists.map((l) => ({ value: l, label: l, count: (reminders ?? []).filter((r) => r.listName === l).length })),
    ];
    const out: FilterGroup[] = [
      badgeGroup({ key: "status", label: "Status", description: "Open, completed or flagged", options: statusOptions, value: effStatus, onChange: setStatus }),
    ];
    if (lists.length > 1)
      out.push(badgeGroup({ key: "list", label: "List", description: "Which reminder list", options: listOptions, value: effList, onChange: setList }));
    out.push(timeGroup({ description: "When the reminder was created", presets, counts: presetCounts, value: range, onChange: setRange }));
    return out;
  }, [hasReminders, reminders, hasFlagged, lists, effStatus, effList, presets, presetCounts, range, setStatus, setList, setRange]);
  const sortNode = useMemo(
    () =>
      hasReminders ? (
        <SortControl
          fields={[
            { value: "title", label: "Title" },
            { value: "created", label: "Created" },
            { value: "due", label: "Due" },
          ]}
          value={sort}
          onChange={setSort}
        />
      ) : undefined,
    [hasReminders, sort, setSort],
  );
  const searchNode = useMemo(
    () => (hasReminders ? <ListSearch value={q} onChange={setQ} placeholder="Search reminders" /> : undefined),
    [hasReminders, q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Reminders", count: filtered.length, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, filtered.length, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <EmptyView
        icon={ListTodo}
        title="No backup open"
        description="Import a backup to see reminders."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <VirtualListView<Reminder>
      headless
      title="Reminders"
      count={filtered.length}
      estimateSize={64}
      isPending={isPending}
      error={error}
      emptyMessage={
        hasReminders ? "No reminders match these filters." : "No reminders in this backup."
      }
      items={filtered}
      getKey={(r) => r.id}
      renderItem={(r) => <ReminderRow reminder={r} />}
    />
  );
}
