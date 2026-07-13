import { useState } from "react";
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
import { SortControl, type SortState } from "@/components/sort-control";
import { EmptyView, LazyListView, ListSearch } from "@/components/view";
import { formatDateTime, formatDuration } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { cn } from "@/lib/utils";
import { client, type Call } from "@/lib/ipc";

export function CallsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  const [sort, setSort] = useState<SortState>({ by: "date", desc: true });
  // Subscribe to the clock preference so rows re-render (with the new time
  // format) when it changes; folded into resetKey below.
  const { clockFormat } = useSettings();
  const { data: count } = useQuery({
    queryKey: ["callsCount", search],
    queryFn: () => client.countCalls(search),
    enabled: active === true,
  });

  if (active === false) {
    return (
      <EmptyView icon={PhoneCall} title="No backup open" description="Import a backup to see call history.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <LazyListView<Call>
      title="Calls"
      count={active === true ? count : undefined}
      resetKey={`${search ?? ""}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={search ? "No matching calls." : "No calls in this backup."}
      header={
        <div className="flex items-center gap-2">
          <div className="w-56">
            <ListSearch value={q} onChange={setQ} placeholder="Search calls" />
          </div>
          <SortControl
            fields={[
              { value: "date", label: "Date" },
              { value: "name", label: "Name" },
              { value: "duration", label: "Duration" },
            ]}
            value={sort}
            onChange={setSort}
          />
        </div>
      }
      windowKey={(page) => ["callsWindow", search, sort.by, sort.desc, page]}
      fetchWindow={(offset, limit) =>
        client.getCallsWindow(search, offset, limit, sort.by, sort.desc)
      }
      renderItem={(c) => <CallRow call={c} />}
    />
  );
}

function callVisual(call: Call): { Icon: typeof PhoneCall; className: string } {
  const missed = call.answered === false && call.direction === "incoming";
  if (call.service?.toLowerCase().includes("facetime")) {
    return { Icon: Video, className: "text-muted-foreground" };
  }
  if (missed) return { Icon: PhoneMissed, className: "text-destructive" };
  if (call.direction === "outgoing") {
    return { Icon: PhoneOutgoing, className: "text-muted-foreground" };
  }
  return { Icon: PhoneIncoming, className: "text-muted-foreground" };
}

function CallRow({ call }: { call: Call }) {
  const { Icon, className } = callVisual(call);
  const missed = call.answered === false && call.direction === "incoming";
  const duration = formatDuration(call.durationS);
  return (
    <Item>
      <ItemMedia>
        <Icon className={cn("size-5", className)} />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className={cn("truncate", missed && "text-destructive")}>
          {call.address ?? "Unknown"}
        </ItemTitle>
        <ItemDescription className="truncate">
          {[call.service, call.direction].filter(Boolean).join(" · ")}
        </ItemDescription>
      </ItemContent>
      <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
        <span>{formatDateTime(call.occurredAt)}</span>
        {duration && <span>{duration}</span>}
      </div>
    </Item>
  );
}
