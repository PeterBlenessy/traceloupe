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
      resetKey={search ?? ""}
      emptyMessage={search ? "No matching calls." : "No calls in this backup."}
      header={
        <div className="w-56">
          <ListSearch value={q} onChange={setQ} placeholder="Search calls" />
        </div>
      }
      windowKey={(page) => ["callsWindow", search, page]}
      fetchWindow={(offset, limit) => client.getCallsWindow(search, offset, limit)}
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
        <ItemTitle className={cn(missed && "text-destructive")}>
          {call.address ?? "Unknown"}
        </ItemTitle>
        <ItemDescription>
          {[call.service, call.direction].filter(Boolean).join(" · ")}
        </ItemDescription>
      </ItemContent>
      <div className="flex flex-col items-end gap-0.5 text-xs text-muted-foreground">
        <span>{formatDateTime(call.occurredAt)}</span>
        {duration && <span>{duration}</span>}
      </div>
    </Item>
  );
}
