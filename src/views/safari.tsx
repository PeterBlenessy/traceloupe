import { useState } from "react";
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
import { EmptyView, LazyListView, ListSearch } from "@/components/view";
import { formatDateTime } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { client, type HistoryVisit } from "@/lib/ipc";

export function SafariView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  const { data: count } = useQuery({
    queryKey: ["safariCount", search],
    queryFn: () => client.countSafari(search),
    enabled: active === true,
  });

  if (active === false) {
    return (
      <EmptyView icon={Globe} title="No backup open" description="Import a backup to see Safari history.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <LazyListView<HistoryVisit>
      title="Safari"
      count={count}
      resetKey={search ?? ""}
      emptyMessage={search ? "No matching history." : "No Safari history in this backup."}
      header={
        <div className="w-56">
          <ListSearch value={q} onChange={setQ} placeholder="Search history" />
        </div>
      }
      windowKey={(page) => ["safariWindow", search, page]}
      fetchWindow={(offset, limit) => client.getSafariWindow(search, offset, limit)}
      renderItem={(h) => <VisitRow visit={h} />}
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
