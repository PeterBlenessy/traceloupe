import { useMemo, useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import {
  PhoneCall,
  PhoneIncoming,
  PhoneMissed,
  PhoneOutgoing,
  Video,
} from "lucide-react";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { timeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState, LazyListView, ListSearch } from "@/components/view";
import { formatDateTime, formatDuration } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { useContactResolver, type ResolvedContact } from "@/lib/use-contact-resolver";
import { cn } from "@/lib/utils";
import { client, type Call, type TimeRange } from "@/lib/ipc";

export function CallsView() {
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  const [sort, setSort] = usePersistedState<SortState>("calls:sort", { by: "date", desc: true });
  // Time filter — same preset chips + custom range as Photos/Safari, over the call date.
  const { now, presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Subscribe to the clock preference so rows re-render (with the new time
  // format) when it changes; folded into resetKey below.
  const { clockFormat } = useSettings();
  const { data: count, error } = useQuery({
    queryKey: ["callsCount", search, range.lo, range.hi],
    queryFn: () => client.countCalls(search, range.lo, range.hi),
    enabled: active === true,
  });
  const { data: presetCounts } = useQuery({
    queryKey: ["callRanges", now, search],
    queryFn: () =>
      client.countCallRanges(
        presets.map((p) => ({ lo: p.lo, hi: p.hi })),
        search,
      ),
    enabled: active === true,
  });
  // Resolve each call's phone number to a saved contact, like Messages does.
  const resolve = useContactResolver();

  const filterGroups = useMemo<FilterGroup[]>(
    () => [timeGroup({ description: "When the call happened", presets, counts: presetCounts, value: range, onChange: setRange })],
    [presets, presetCounts, range],
  );
  const sortNode = useMemo(
    () => (
      <SortControl
        fields={[
          { value: "date", label: "Date" },
          { value: "name", label: "Name" },
          { value: "duration", label: "Duration" },
        ]}
        value={sort}
        onChange={setSort}
      />
    ),
    [sort, setSort],
  );
  const searchNode = useMemo(
    () => <ListSearch value={q} onChange={setQ} placeholder="Search calls" />,
    [q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Calls", count, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, count, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <NoBackupState
        icon={PhoneCall}
        title="See call history"
        lead="Incoming, outgoing, and missed calls — FaceTime and cellular — with durations, timestamps, and the country each number belongs to. Numbers are resolved to saved contacts."
        features={[
          { label: "Search", detail: "Search across the whole call log." },
          { label: "Time range", detail: "Focus on any date range with preset or custom windows." },
          { label: "Sort", detail: "Order by date, name, or duration." },
          { label: "At a glance", detail: "Direction and call-type icons plus a country flag on every row." },
        ]}
        note="Parsed locally from the backup on this Mac."
      />
    );
  }

  return (
    <LazyListView<Call>
      title="Calls"
      count={active === true ? count : undefined}
      error={error}
      resetKey={`${search ?? ""}:${range.lo}:${range.hi}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={search ? "No matching calls." : "No calls in this backup."}
      emptyIcon={PhoneCall}
      underlap
      windowKey={(page) => [
        "callsWindow",
        search,
        range.lo,
        range.hi,
        sort.by,
        sort.desc,
        page,
      ]}
      fetchWindow={(offset, limit) =>
        client.getCallsWindow(
          search,
          range.lo,
          range.hi,
          offset,
          limit,
          sort.by,
          sort.desc,
        )
      }
      renderItem={(c) => <CallRow call={c} contact={resolve(c.address)} />}
    />
  );
}

function callVisual(call: Call): { Icon: typeof PhoneCall; className: string } {
  const missed = call.answered === false && call.direction === "incoming";
  // Only actual video calls get the video icon — FaceTime *audio* (callType
  // "audio") falls through to the direction icons, like a phone call.
  if (call.callType === "video") {
    return { Icon: Video, className: "text-muted-foreground" };
  }
  if (missed) return { Icon: PhoneMissed, className: "text-destructive" };
  if (call.direction === "outgoing") {
    return { Icon: PhoneOutgoing, className: "text-muted-foreground" };
  }
  return { Icon: PhoneIncoming, className: "text-muted-foreground" };
}

/// A friendly medium label: "FaceTime Video" / "FaceTime Audio" / "Phone".
/** A lowercase ISO alpha-2 code → its flag emoji (regional-indicator pair), or
 *  null. Lets a call's number country show at a glance without a lookup table. */
function countryFlag(code: string | null): string | null {
  if (!code || !/^[a-z]{2}$/.test(code)) return null;
  const base = 0x1f1e6; // regional indicator 'A'
  return String.fromCodePoint(
    base + (code.charCodeAt(0) - 97),
    base + (code.charCodeAt(1) - 97),
  );
}

function serviceLabel(call: Call): string | null {
  const isFaceTime = call.service?.toLowerCase().includes("facetime");
  if (isFaceTime) {
    if (call.callType === "video") return "FaceTime Video";
    if (call.callType === "audio") return "FaceTime Audio";
    return "FaceTime";
  }
  if (call.service?.toLowerCase() === "phone") return "Phone";
  return call.service;
}

function CallRow({ call, contact }: { call: Call; contact: ResolvedContact | null }) {
  const { Icon, className } = callVisual(call);
  const missed = call.answered === false && call.direction === "incoming";
  const duration = formatDuration(call.durationS);
  // Prefer the saved contact's name; fall back to the raw number. When a name is
  // shown, keep the number visible in the subtitle so it's not lost.
  const title = contact?.name ?? call.address ?? "Unknown";
  const subtitle = [
    contact && call.address ? call.address : null,
    serviceLabel(call),
    call.direction,
    call.location,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <Item>
      <ItemMedia>
        <Icon className={cn("size-5", className)} />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className={cn("truncate", missed && "text-destructive")}>
          {title}
        </ItemTitle>
        <ItemDescription className="truncate">{subtitle}</ItemDescription>
      </ItemContent>
      {call.countryCode && countryFlag(call.countryCode) && (
        <span
          className="shrink-0 text-base leading-none"
          title={`Number country: ${call.countryCode.toUpperCase()}`}
        >
          {countryFlag(call.countryCode)}
        </span>
      )}
      <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
        <span>{formatDateTime(call.occurredAt)}</span>
        {duration && <span>{duration}</span>}
      </div>
    </Item>
  );
}
