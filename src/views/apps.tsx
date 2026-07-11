import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Boxes, Download } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyView, ListSearch, ListView } from "@/components/view";
import { appMeta, SUPPORT_LABEL, type AppSupport } from "@/lib/apps";
import { client } from "@/lib/ipc";

interface AppRow {
  bundleId: string;
  name: string;
  support: AppSupport;
}

export function AppsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: bundleIds, isPending } = useQuery({
    queryKey: ["installedApps"],
    queryFn: () => client.listInstalledApps(),
    enabled: active === true,
  });
  const [q, setQ] = useState("");

  // Salvage-recoverable apps first, system apps last; each group by name.
  const apps: AppRow[] = useMemo(() => {
    if (!bundleIds) return [];
    const rank: Record<AppSupport, number> = {
      available: 0,
      planned: 1,
      limited: 2,
      unknown: 3,
      system: 4,
    };
    return bundleIds
      .map((bundleId) => ({ bundleId, ...appMeta(bundleId) }))
      .sort((a, b) => rank[a.support] - rank[b.support] || a.name.localeCompare(b.name));
  }, [bundleIds]);

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return apps;
    return apps.filter(
      (a) => a.name.toLowerCase().includes(needle) || a.bundleId.toLowerCase().includes(needle),
    );
  }, [apps, q]);

  if (active === false) {
    return (
      <EmptyView icon={Boxes} title="No backup open" description="Import a backup to see its apps.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <ListView
      title="Apps"
      count={apps.length}
      header={
        apps.length > 0 ? (
          <div className="w-56">
            <ListSearch value={q} onChange={setQ} placeholder="Search apps" />
          </div>
        ) : undefined
      }
    >
      {isPending &&
        Array.from({ length: 8 }).map((_, i) => <Skeleton key={i} className="mb-1 h-14 w-full" />)}
      {apps.length === 0 && !isPending && (
        <p className="p-6 text-center text-sm text-muted-foreground">
          No installed-app list in this backup.
        </p>
      )}
      {filtered.map((app) => (
        <AppItem key={app.bundleId} app={app} />
      ))}
    </ListView>
  );
}

function AppItem({ app }: { app: AppRow }) {
  const [extractNote, setExtractNote] = useState(false);
  const label = SUPPORT_LABEL[app.support];
  const canExtract = app.support === "available" || app.support === "planned";

  return (
    <Item>
      <ItemMedia>
        <div className="flex size-9 items-center justify-center rounded-lg bg-muted text-xs font-semibold text-muted-foreground">
          {app.name.slice(0, 2).toUpperCase()}
        </div>
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="flex items-center gap-2">
          {app.name}
          {label && (
            <Badge variant={app.support === "available" ? "default" : "secondary"}>
              {label}
            </Badge>
          )}
        </ItemTitle>
        <ItemDescription className="truncate">{app.bundleId}</ItemDescription>
      </ItemContent>
      {canExtract && (
        <ItemActions>
          {extractNote ? (
            <span className="text-xs text-muted-foreground">Per-app extraction is coming soon</span>
          ) : (
            <Button variant="outline" size="sm" onClick={() => setExtractNote(true)}>
              <Download className="size-4" />
              Extract data
            </Button>
          )}
        </ItemActions>
      )}
    </Item>
  );
}
