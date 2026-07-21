import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowDownLeft, ArrowUpRight, Users, Waypoints } from "lucide-react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { timeGroup, type FilterGroup } from "@/components/filter-groups";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useDebounced } from "@/lib/use-debounced";
import { NoBackupState, ListSearch, VirtualListView } from "@/components/view";
import { initials } from "@/lib/contact";
import { appMeta } from "@/lib/apps";
import { BrandIcon } from "@/lib/brand-icon";
import { formatCount, formatDate } from "@/lib/format";
import {
  client,
  type Interaction,
  type InteractionChannel,
  type TimeRange,
} from "@/lib/ipc";

function label(i: Interaction): string {
  return i.displayName ?? i.identifier ?? "Unknown";
}

/** Apple system-app bundle ids CoreDuet uses, mapped to channel names; other
 *  bundle ids fall back to the app catalog (Snapchat, Gmail, …) then the id. */
const CHANNEL_NAMES: Record<string, string> = {
  "com.apple.MobileSMS": "Messages",
  "com.apple.InCallService": "Phone",
  "com.apple.TelephonyUtilities": "Phone",
  "com.apple.facetime": "FaceTime",
  "com.apple.Contacts.Autocomplete": "Contacts",
  "com.apple.mobileslideshow": "Photos",
  "com.apple.camera": "Camera",
  "com.apple.ScreenshotServicesService": "Screenshots",
};
function channelName(bundleId: string): string {
  return CHANNEL_NAMES[bundleId] ?? appMeta(bundleId).name;
}

interface Channel {
  name: string;
  slug?: string;
  incoming: number;
  outgoing: number;
}

/** Collapse raw per-bundle rows into display channels: bundle ids that share a
 *  name (InCallService + TelephonyUtilities → Phone) merge, zero-total channels
 *  drop, and the result is busiest-first. */
function mergeChannels(rows: InteractionChannel[]): Channel[] {
  const byName = new Map<string, Channel>();
  for (const r of rows) {
    const name = channelName(r.bundleId);
    const existing = byName.get(name);
    if (existing) {
      existing.incoming += r.incoming;
      existing.outgoing += r.outgoing;
      existing.slug ??= appMeta(r.bundleId).slug;
    } else {
      byName.set(name, {
        name,
        slug: appMeta(r.bundleId).slug,
        incoming: r.incoming,
        outgoing: r.outgoing,
      });
    }
  }
  return [...byName.values()]
    .filter((c) => c.incoming + c.outgoing > 0)
    .sort((a, b) => b.incoming + b.outgoing - (a.incoming + a.outgoing));
}

/** A horizontal strip of the apps that CoreDuet interactions flowed through,
 *  busiest first — the communication channels the person used. */
function ChannelsBar({ channels }: { channels: InteractionChannel[] }) {
  const merged = useMemo(() => mergeChannels(channels), [channels]);
  if (merged.length === 0) return null;
  return (
    <div className="border-b px-4 py-3">
      <div className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
        Channels
      </div>
      <div className="flex flex-wrap gap-2">
        {merged.map((c) => (
          <div
            key={c.name}
            className="flex items-center gap-2 rounded-lg border bg-muted/30 px-2.5 py-1.5"
            title={`${formatCount(c.incoming)} in · ${formatCount(c.outgoing)} out`}
          >
            <BrandIcon slug={c.slug} name={c.name} className="size-4" />
            <span className="text-sm">{c.name}</span>
            <span className="text-xs font-medium tabular-nums text-muted-foreground">
              {formatCount(c.incoming + c.outgoing)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function InteractionRow({ interaction }: { interaction: Interaction }) {
  const name = label(interaction);
  const total = interaction.incoming + interaction.outgoing;
  return (
    <Item>
      <ItemMedia>
        <Avatar className="size-9">
          <AvatarFallback>{initials(name)}</AvatarFallback>
        </Avatar>
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{name}</ItemTitle>
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
      </ItemContent>
      <div className="flex shrink-0 flex-col items-end gap-0.5 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">{formatCount(total)}</span>
        <span className="inline-flex items-center gap-1.5 tabular-nums">
          <ArrowDownLeft className="size-3" />
          {formatCount(interaction.incoming)}
          <ArrowUpRight className="ml-1 size-3" />
          {formatCount(interaction.outgoing)}
          {interaction.incomingRecipient > 0 && (
            <span
              className="ml-1 inline-flex items-center gap-1"
              title="Sent to a group you were in"
            >
              <Users className="size-3" />
              {formatCount(interaction.incomingRecipient)}
            </span>
          )}
        </span>
      </div>
    </Item>
  );
}

/** True when `at` falls in a half-open [lo, hi) window; undated only pass "All". */
function inWindow(at: number | null, lo: number | null, hi: number | null) {
  if (lo == null && hi == null) return true;
  if (at == null) return false;
  return (lo == null || at >= lo) && (hi == null || at < hi);
}

export function InteractionsView() {
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
  const { data: channels } = useQuery({
    queryKey: ["interactionChannels"],
    queryFn: () => client.interactionChannels(),
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

  const hasData = (interactions?.length ?? 0) > 0;
  const filterGroups = useMemo<FilterGroup[]>(
    () =>
      hasData
        ? [timeGroup({ description: "When you last interacted", presets, counts: presetCounts, value: range, onChange: setRange })]
        : [],
    [hasData, presets, presetCounts, range],
  );
  const sortNode = useMemo(
    () =>
      hasData ? (
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
      ) : undefined,
    [hasData, sort, setSort],
  );
  const searchNode = useMemo(
    () =>
      hasData ? <ListSearch value={q} onChange={setQ} placeholder="Search people" /> : undefined,
    [hasData, q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Interactions", count: filtered.length, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, filtered.length, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <NoBackupState
        icon={Waypoints}
        title="Open a backup to map interactions"
        lead="A relationship graph built from the device's CoreDuet activity — who was contacted most, across which apps, and when."
        features={[
          { label: "Search", detail: "Search for a specific person." },
          { label: "Channels", detail: "See which apps interactions flowed through, busiest first." },
          { label: "Sort", detail: "Rank people by total, incoming, outgoing, or most recent." },
          { label: "Per person", detail: "Interaction totals, in/out breakdown, and first-to-last date range." },
        ]}
        note="Computed locally from the backup on this Mac."
      />
    );
  }

  return (
    <div className="flex h-full flex-col">
      <ChannelsBar channels={channels ?? []} />
      <div className="min-h-0 flex-1">
        <VirtualListView<Interaction>
          title="Interactions"
          count={filtered.length}
          estimateSize={68}
          isPending={isPending}
          error={error}
          emptyMessage={
            hasData ? "No contacts match these filters." : "No interaction data in this backup."
          }
          items={filtered}
          getKey={(i) => i.id}
          renderItem={(i) => <InteractionRow interaction={i} />}
        />
      </div>
    </div>
  );
}
