import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ArrowDownLeft, ArrowUpRight, Waypoints } from "lucide-react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { TimeFilterBar, useTimePresets } from "@/components/time-filter";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useDebounced } from "@/lib/use-debounced";
import { EmptyView, ListSearch, VirtualListView } from "@/components/view";
import { initials } from "@/lib/contact";
import { formatCount, formatDate } from "@/lib/format";
import { client, type Interaction, type TimeRange } from "@/lib/ipc";

function label(i: Interaction): string {
  return i.displayName ?? i.identifier ?? "Unknown";
}

function InteractionRow({ interaction }: { interaction: Interaction }) {
  const name = label(interaction);
  const total = interaction.incoming + interaction.outgoing;
  return (
    <div className="flex items-center gap-3 rounded-md border px-3 py-2.5">
      <Avatar className="size-9 shrink-0">
        <AvatarFallback>{initials(name)}</AvatarFallback>
      </Avatar>
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium">{name}</div>
        {interaction.displayName && interaction.identifier && (
          <div className="truncate text-xs text-muted-foreground">
            {interaction.identifier}
          </div>
        )}
        {interaction.firstAt != null && interaction.lastAt != null && (
          <div className="text-xs text-muted-foreground">
            {formatDate(interaction.firstAt)} – {formatDate(interaction.lastAt)}
          </div>
        )}
      </div>
      <div className="flex shrink-0 flex-col items-end gap-0.5 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">{formatCount(total)}</span>
        <span className="inline-flex items-center gap-1.5 tabular-nums">
          <ArrowDownLeft className="size-3" />
          {formatCount(interaction.incoming)}
          <ArrowUpRight className="ml-1 size-3" />
          {formatCount(interaction.outgoing)}
        </span>
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

export function InteractionsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: interactions,
    isPending,
    error,
  } = useQuery({
    queryKey: ["interactions"],
    queryFn: () => client.listInteractions(),
    enabled: active === true,
  });

  const [sort, setSort] = usePersistedState<SortState>("interactions:sort", {
    by: "total",
    desc: true,
  });
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim().toLowerCase());
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });

  const baseFiltered = useMemo(() => {
    return (interactions ?? []).filter((i) => {
      if (!search) return true;
      return [i.displayName, i.identifier]
        .filter(Boolean)
        .join(" ")
        .toLowerCase()
        .includes(search);
    });
  }, [interactions, search]);

  // Time filter is on the most-recent interaction date.
  const presetCounts = useMemo(
    () => presets.map((p) => baseFiltered.filter((i) => inWindow(i.lastAt, p.lo, p.hi)).length),
    [presets, baseFiltered],
  );

  const filtered = useMemo(() => {
    const inRange = baseFiltered.filter((i) => inWindow(i.lastAt, range.lo, range.hi));
    return sortItems(
      inRange,
      (i) =>
        sort.by === "incoming"
          ? i.incoming
          : sort.by === "outgoing"
            ? i.outgoing
            : sort.by === "recent"
              ? i.lastAt
              : i.incoming + i.outgoing,
      sort.desc,
    );
  }, [baseFiltered, range, sort]);

  if (active === false) {
    return (
      <EmptyView
        icon={Waypoints}
        title="No backup open"
        description="Import a backup to see the interaction graph."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const hasData = (interactions?.length ?? 0) > 0;

  return (
    <VirtualListView<Interaction>
      title="Interactions"
      count={filtered.length}
      estimateSize={68}
      isPending={isPending}
      error={error}
      emptyMessage={
        hasData ? "No contacts match these filters." : "No interaction data in this backup."
      }
      search={
        hasData ? (
          <ListSearch value={q} onChange={setQ} placeholder="Search people" />
        ) : undefined
      }
      toolbar={
        hasData ? (
          <>
            <TimeFilterBar
              className="flex-1"
              presets={presets}
              value={range}
              onChange={setRange}
              counts={presetCounts}
            />
            <SortControl
              fields={[
                { value: "total", label: "Total" },
                { value: "incoming", label: "In" },
                { value: "outgoing", label: "Out" },
                { value: "recent", label: "Recent" },
              ]}
              value={sort}
              onChange={setSort}
            />
          </>
        ) : undefined
      }
      items={filtered}
      getKey={(i) => i.id}
      renderItem={(i) => <InteractionRow interaction={i} />}
    />
  );
}
