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
import { useSettings } from "@/components/settings-provider";
import { SortControl, type SortState } from "@/components/sort-control";
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
  const [sort, setSort] = useState<SortState>({ by: "date", desc: true });
  // Subscribe to the clock preference so times re-render on change.
  const { clockFormat } = useSettings();
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
      resetKey={`${search ?? ""}:${clockFormat}:${sort.by}:${sort.desc}`}
      emptyMessage={search ? "No matching history." : "No Safari history in this backup."}
      header={
        <div className="flex items-center gap-2">
          <div className="w-56">
            <ListSearch value={q} onChange={setQ} placeholder="Search history" />
          </div>
          <SortControl
            fields={[
              { value: "date", label: "Date" },
              { value: "title", label: "Title" },
              { value: "visits", label: "Visits" },
            ]}
            value={sort}
            onChange={setSort}
          />
        </div>
      }
      windowKey={(page) => ["safariWindow", search, sort.by, sort.desc, page]}
      fetchWindow={(offset, limit) =>
        client.getSafariWindow(search, offset, limit, sort.by, sort.desc)
      }
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
      <div className="flex shrink-0 flex-col items-end gap-0.5 whitespace-nowrap text-xs text-muted-foreground">
        <span>{formatDateTime(visit.visitedAt)}</span>
        {visit.visitCount != null && <span>{visit.visitCount} visits</span>}
      </div>
    </Item>
  );
}
