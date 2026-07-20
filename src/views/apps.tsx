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
import { useViewToolbar } from "@/components/toolbar-context";
import { EmptyView, ListSearch, VirtualListView } from "@/components/view";
import { appMeta, SUPPORT_LABEL, type AppSupport } from "@/lib/apps";
import { BrandIcon } from "@/lib/brand-icon";
import { cn } from "@/lib/utils";
import { client } from "@/lib/ipc";

interface AppRow {
  bundleId: string;
  name: string;
  /** The built-in catalog name, used as the Messages `service` filter value
   *  (threads are tagged with this, not the App Store `name`). */
  serviceName: string;
  support: AppSupport;
  slug?: string;
  /** App Store metadata from the backup's Info.plist (may be absent). */
  seller: string | null;
  version: string | null;
  genre: string | null;
  released: string | null;
}

/** "2018" — just the year of an RFC-3339 release date (the day/time is noise
 *  for an app's original App Store release). */
function releasedYear(released: string | null): string | null {
  if (!released) return null;
  const d = new Date(released);
  return Number.isNaN(d.getTime()) ? null : String(d.getUTCFullYear());
}

export function AppsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: installed,
    isPending,
    error,
  } = useQuery({
    queryKey: ["installedApps"],
    queryFn: () => client.listInstalledApps(),
    enabled: active === true,
  });
  const [q, setQ] = useState("");

  // TraceLoupe-recoverable apps first, system apps last; each group by name.
  // Prefer the backup's own App Store name over the built-in catalog name.
  const apps: AppRow[] = useMemo(() => {
    if (!installed) return [];
    const rank: Record<AppSupport, number> = {
      native: 0,
      available: 1,
      planned: 2,
      limited: 3,
      unknown: 4,
      system: 5,
    };
    return installed
      .map((app): AppRow => {
        const meta = appMeta(app.bundleId);
        return {
          bundleId: app.bundleId,
          name: app.name ?? meta.name,
          serviceName: meta.name,
          support: meta.support,
          slug: meta.slug,
          seller: app.seller,
          version: app.version,
          genre: app.genre,
          released: app.released,
        };
      })
      .sort(
        (a, b) =>
          rank[a.support] - rank[b.support] || a.name.localeCompare(b.name),
      );
  }, [installed]);

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return apps;
    return apps.filter(
      (a) =>
        a.name.toLowerCase().includes(needle) ||
        a.bundleId.toLowerCase().includes(needle) ||
        (a.seller?.toLowerCase().includes(needle) ?? false) ||
        (a.genre?.toLowerCase().includes(needle) ?? false),
    );
  }, [apps, q]);

  const searchNode = useMemo(
    () => (apps.length > 0 ? <ListSearch value={q} onChange={setQ} placeholder="Search apps" /> : undefined),
    [apps.length, q],
  );
  const toolbar = useMemo(
    () => (active === true ? { title: "Apps", count: filtered.length, filter: [], search: searchNode } : null),
    [active, filtered.length, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <EmptyView
        icon={Boxes}
        title="No backup open"
        description="Import a backup to see its apps."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <VirtualListView
      title="Apps"
      count={filtered.length}
      isPending={isPending}
      error={error}
      emptyMessage="No installed-app list in this backup."
      items={filtered}
      getKey={(a) => a.bundleId}
      renderItem={(a) => <AppItem app={a} />}
    />
  );
}

function AppItem({ app }: { app: AppRow }) {
  const navigate = useNavigate();
  const label = SUPPORT_LABEL[app.support];
  // Only the "coming soon" placeholder — never for apps we already parse natively
  // (their chats show in Messages) or that keep no local data.
  const canExtract = app.support === "planned";

  return (
    <Item>
      <ItemMedia>
        <div className="flex size-9 items-center justify-center rounded-lg bg-muted">
          <BrandIcon slug={app.slug} name={app.name} className="size-5" />
        </div>
      </ItemMedia>
      <ItemContent>
        <ItemTitle className="flex items-center gap-2">
          {app.name}
          {app.version && (
            <span className="text-xs font-normal tabular-nums text-muted-foreground/70">
              v{app.version}
            </span>
          )}
          {label && (
            // Both states share the soft "secondary" pill shape (identical box, so
            // no optical height difference); "native" only re-tints it. A solid
            // near-white `default` badge optically blooms taller on the dark row.
            <Badge
              variant="secondary"
              className={cn(
                "px-2 py-0.5 font-medium",
                app.support === "native" &&
                  "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400",
              )}
            >
              {label}
            </Badge>
          )}
        </ItemTitle>
        {/* Seller · genre · release year from the backup's App Store metadata,
            when present; otherwise fall back to the bundle id. */}
        {app.seller || app.genre || app.released ? (
          <ItemDescription className="truncate">
            {[app.seller, app.genre, releasedYear(app.released)]
              .filter(Boolean)
              .join(" · ")}
          </ItemDescription>
        ) : null}
        <ItemDescription className="truncate text-muted-foreground/60">
          {app.bundleId}
        </ItemDescription>
      </ItemContent>
      {app.support === "native" ? (
        <ItemActions>
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              navigate({ to: "/messages", search: { service: app.serviceName } })
            }
            className="text-xs text-muted-foreground"
          >
            Chats in Messages →
          </Button>
        </ItemActions>
      ) : canExtract ? (
        <ItemActions>
          <Button
            variant="outline"
            size="sm"
            disabled
            title="Per-app extraction is coming soon"
          >
            <Download className="size-4" />
            Extract data
          </Button>
        </ItemActions>
      ) : null}
    </Item>
  );
}
