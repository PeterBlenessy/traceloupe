import { useMemo, useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
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
import { Button } from "@/components/ui/button";
import { useSettings } from "@/components/settings-provider";
import { type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { sortIsland, timeIsland } from "@/components/toolbar-islands";
import { EmptyView, LazyListView, ListSearch } from "@/components/view";
import { formatDateTime, formatDuration } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { useContactResolver, type ResolvedContact } from "@/lib/use-contact-resolver";
import { cn } from "@/lib/utils";
import { client, type Call, type TimeRange } from "@/lib/ipc";

export function CallsView() {
  const navigate = useNavigate();
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

  const islands = useMemo(
    () => [
      timeIsland({ presets, counts: presetCounts, value: range, onChange: setRange }),
      sortIsland({
        fields: [
          { value: "date", label: "Date" },
          { value: "name", label: "Name" },
          { value: "duration", label: "Duration" },
        ],
        value: sort,
        onChange: setSort,
      }),
    ],
    [presets, presetCounts, range, sort, setSort],
  );
  const searchNode = useMemo(
    () => <ListSearch value={q} onChange={setQ} placeholder="Search calls" />,
    [q],
  );
  const toolbar = useMemo(
    () => (active === true ? { title: "Calls", count, islands, search: searchNode } : null),
    [active, count, islands, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <EmptyView icon={PhoneCall} title="No backup open" description="Import a backup to see call history.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <LazyListView<Call>
      headless
      title="Calls"
      count={active === true ? count : undefined}
      error={error}
      resetKey={`${search ?? ""}:${range.lo}:${range.hi}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={search ? "No matching calls." : "No calls in this backup."}
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
      <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
        <span>{formatDateTime(call.occurredAt)}</span>
        {duration && <span>{duration}</span>}
      </div>
    </Item>
  );
}
