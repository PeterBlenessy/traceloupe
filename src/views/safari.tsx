import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Globe } from "lucide-react";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { Button } from "@/components/ui/button";
import { EmptyView, ListSearch, VirtualListView } from "@/components/view";
import { formatDateTime } from "@/lib/format";
import { client, type HistoryVisit } from "@/lib/ipc";

export function SafariView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: history, isPending } = useQuery({
    queryKey: ["safari"],
    queryFn: () => client.listSafariHistory(),
    enabled: active === true,
  });
  const [q, setQ] = useState("");

  const filtered = useMemo(() => {
    if (!history) return [];
    const needle = q.trim().toLowerCase();
    if (!needle) return history;
    return history.filter(
      (h) => h.url.toLowerCase().includes(needle) || h.title?.toLowerCase().includes(needle),
    );
  }, [history, q]);

  if (active === false) {
    return (
      <EmptyView icon={Globe} title="No backup open" description="Import a backup to see Safari history.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <VirtualListView
      title="Safari"
      count={history?.length}
      isPending={isPending}
      emptyMessage="No Safari history in this backup."
      header={
        history && history.length > 0 ? (
          <div className="w-56">
            <ListSearch value={q} onChange={setQ} placeholder="Search history" />
          </div>
        ) : undefined
      }
      items={filtered}
      getKey={(x) => x.id}
      renderItem={(x) => <VisitRow visit={x} />}
    />
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

function VisitRow({ visit }: { visit: HistoryVisit }) {
  return (
    <Item>
      <ItemMedia>
        <Globe className="size-5 text-muted-foreground" />
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="truncate">{visit.title ?? hostOf(visit.url)}</ItemTitle>
        <ItemDescription className="truncate">{visit.url}</ItemDescription>
      </ItemContent>
      <div className="flex flex-col items-end gap-0.5 text-xs text-muted-foreground">
        <span>{formatDateTime(visit.visitedAt)}</span>
        {visit.visitCount != null && <span>{visit.visitCount} visits</span>}
      </div>
    </Item>
  );
}
