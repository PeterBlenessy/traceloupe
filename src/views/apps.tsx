import { useMemo, useState } from "react";
import { dateFormat } from "@/lib/format";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Boxes } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import {
  Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle, } from "@/components/ui/item";
import { useViewToolbar } from "@/components/toolbar-context";
import { useSettings } from "@/components/settings-provider";
import { NoBackupState, ListSearch, VirtualListView } from "@/components/view";
import { ArtifactTable } from "@/components/artifact-table";
import { useHostedArtifacts, type HostedArtifact } from "@/lib/use-hosted-artifacts";
import { appMeta, SUPPORT_LABEL, type AppSupport } from "@/lib/apps";
import { BrandIcon, hasBrandIcon } from "@/lib/brand-icon";
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
  /** Per-copy download receipt: when, and which account installed it. */
  downloaded: string | null;
  appleId: string | null;
  contentRating: string | null;
  subgenre: string | null;
}

/** A stable, distinct tinted tile for an app without a bundled brand logo —
 *  hue derived from its bundle id, so each app reads as its own icon rather
 *  than a uniform grey monogram. (Real App Store artwork can't be used: the
 *  webview CSP blocks remote images and the backup carries no icon bitmap.) */
function appTile(bundleId: string): { backgroundColor: string; color: string } {
  let h = 0;
  for (let i = 0; i < bundleId.length; i++)
    h = (h * 31 + bundleId.charCodeAt(i)) % 360;
  // oklch, not hsl: HSL lightness is not perceptual, so a fixed 62% is legible
  // at one hue and not at another — the design lint caught a pink tile at
  // 4.11:1. oklch's L IS perceptual, so one value holds across every hue.
  // light-dark() picks the tier for the current theme, which works because
  // `color-scheme` is set on the root.
  return {
    backgroundColor: `oklch(0.62 0.14 ${h} / 0.16)`,
    color: `light-dark(oklch(0.48 0.13 ${h}), oklch(0.82 0.13 ${h}))`,
  };
}

/** "2018" — just the year of an RFC-3339 release date (the day/time is noise
 *  for an app's original App Store release). */
function releasedYear(released: string | null): string | null {
  if (!released) return null;
  const d = new Date(released);
  return Number.isNaN(d.getTime()) ? null : String(d.getUTCFullYear());
}

/** "12 Mar 2024" — full day for a download date (unlike a release year, the
 *  exact install day matters forensically). */
function downloadedLabel(downloaded: string): string {
  const d = new Date(downloaded);
  return Number.isNaN(d.getTime())
    ? downloaded
    : dateFormat({ day: "numeric", month: "short", year: "numeric" }).format(d);
}

export function AppsView() {
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
  // Artifacts that declare `surface = "apps"`, keyed by the bundle id they
  // belong to. Apps never learns what any of them are — the module says where it
  // belongs and which column identifies the row, and this attaches it.
  const { hosted } = useHostedArtifacts("apps", active === true);
  // A backup imported before these modules existed has no rows, and permissions
  // would simply be absent with nothing saying why — the same trap #216 fixed.
  const { data: extraction } = useQuery({
    queryKey: ["artifactsExtractionState"],
    queryFn: () => client.artifactsExtractionState(),
    enabled: active === true,
  });
  const queryClient = useQueryClient();
  const [extracting, setExtracting] = useState(false);
  const needsExtraction = extraction === "never-run" || extraction === "stale";

  async function runExtraction() {
    setExtracting(true);
    try {
      await client.extractArtifacts();
      await queryClient.invalidateQueries({ queryKey: ["artifacts"] });
      await queryClient.invalidateQueries({ queryKey: ["artifactRows"] });
      await queryClient.invalidateQueries({ queryKey: ["artifactsExtractionState"] });
    } finally {
      setExtracting(false);
    }
  }

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
          downloaded: app.downloaded,
          appleId: app.appleId,
          contentRating: app.contentRating,
          subgenre: app.subgenre,
        };
      })
      .sort(
        (a, b) =>
          rank[a.support] - rank[b.support] || a.name.localeCompare(b.name),
      );
  }, [installed]);

  // Opt-in real App Store artwork (Settings → Apps). Only fetch for apps
  // without a bundled brand logo; results are cached on disk by the backend.
  const { fetchAppIcons } = useSettings();
  const iconBundleIds = useMemo(
    () => apps.filter((a) => !hasBrandIcon(a.slug)).map((a) => a.bundleId),
    [apps],
  );
  const { data: iconList } = useQuery({
    queryKey: ["appIcons", iconBundleIds],
    queryFn: () => client.getAppIcons(iconBundleIds),
    enabled: fetchAppIcons && iconBundleIds.length > 0,
    staleTime: Infinity,
  });
  const iconMap = useMemo(() => {
    const m = new Map<string, string>();
    iconList?.forEach((i) => m.set(i.bundleId, i.dataUri));
    return m;
  }, [iconList]);

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return apps;
    return apps.filter(
      (a) =>
        a.name.toLowerCase().includes(needle) ||
        a.bundleId.toLowerCase().includes(needle) ||
        (a.seller?.toLowerCase().includes(needle) ?? false) ||
        (a.genre?.toLowerCase().includes(needle) ?? false) ||
        (a.appleId?.toLowerCase().includes(needle) ?? false),
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
      <NoBackupState
        icon={Boxes}
        title="Inspect installed apps"
        lead="Every app installed on the device — with version, App Store details, bundle id, and the data it left in the backup — a starting point for spotting unfamiliar or hidden apps."
        features={[
          { label: "Search", detail: "Search by name, bundle id, seller, genre, or account." },
          { label: "App Store detail", detail: "Seller, genre, subgenre, age rating, and release year." },
          { label: "Install receipt", detail: "When each app was downloaded, and the Apple ID that installed it." },
          { label: "Cross-link", detail: "Jump from a supported app to its chats in Messages." },
          {
            label: "Permissions",
            detail:
              "See what each app was allowed to reach — camera, microphone, photos, contacts, location — and when that was decided.",
          },
        ]}
        note="Read locally on this Mac — nothing is uploaded."
      />
    );
  }

  return (
    <div className="flex h-full flex-col">
      {/* Offered, not automatic: extraction rebuilds the decryptor, which can
          block on Touch ID, and a prompt must not appear because someone opened
          a view. */}
      {needsExtraction && (
        <div className="flex items-center gap-2 border-b px-3 py-1.5 text-xs">
          <span className="text-muted-foreground">
            {extraction === "never-run"
              ? "App permissions have not been read from this backup yet."
              : "TraceLoupe can read more about these apps than was extracted."}
          </span>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button size="sm" variant="ghost" onClick={runExtraction} disabled={extracting}>
                {extracting ? "Reading…" : "Read them now"}
              </Button>
            </TooltipTrigger>
            <TooltipContent>Reads from the backup on disk — no re-import needed</TooltipContent>
          </Tooltip>
        </div>
      )}
      {/* `underlap` lets the list scroll beneath the translucent title bar, and
          is documented as "only sensible when the list is the view's topmost
          element". With the extraction prompt above it that stops being true —
          the list's offset covered the prompt's button, leaving it visible but
          unclickable, which is exactly the failure in #224's "Back to Safety
          Scan". So underlap is off whenever the prompt is showing. */}
      <div className="min-h-0 flex-1">
    <VirtualListView
      title="Apps"
      count={filtered.length}
      isPending={isPending}
      error={error}
      emptyMessage="No installed-app list in this backup."
      emptyIcon={Boxes}
      underlap={!needsExtraction}
      items={filtered}
      getKey={(a) => a.bundleId}
      renderItem={(a) => (
        <AppItem app={a} iconUri={iconMap.get(a.bundleId)} hosted={hosted} />
      )}
    />
      </div>
    </div>
  );
}

/** The hosted artifacts that have rows for this app. */
function artifactsFor(hosted: HostedArtifact[], bundleId: string) {
  const key = bundleId.toLowerCase();
  return hosted
    .map((h) => ({ artifact: h.artifact, rows: h.byKey.get(key) ?? [] }))
    .filter((h) => h.rows.length > 0);
}

function AppItem({
  app,
  iconUri,
  hosted,
}: {
  app: AppRow;
  iconUri?: string;
  hosted: HostedArtifact[];
}) {
  const navigate = useNavigate();
  const label = SUPPORT_LABEL[app.support];
  const [expanded, setExpanded] = useState(false);
  const mine = artifactsFor(hosted, app.bundleId);

  return (
    // Card-per-app (outline + soft card fill): with four classes of info per
    // row, hairline-less rows blurred into one another — the bordered card
    // gives each app a clear boundary, matching the backup picker's language.
    <Item variant="outline" className="bg-card/50">
      <ItemMedia>
        {iconUri ? (
          // Real App Store artwork (opt-in fetch); falls through to the tiles
          // below when not fetched or unresolved.
          <img
            src={iconUri}
            alt={app.name}
            className="size-10 rounded-lg object-cover"
          />
        ) : hasBrandIcon(app.slug) ? (
          <div className="flex size-10 items-center justify-center rounded-lg bg-muted">
            <BrandIcon slug={app.slug} name={app.name} className="size-5" />
          </div>
        ) : (
          <div
            className="flex size-10 items-center justify-center rounded-lg text-sm font-semibold"
            style={appTile(app.bundleId)}
            aria-label={app.name}
          >
            {app.name.slice(0, 1).toUpperCase()}
          </div>
        )}
      </ItemMedia>
      {/* Type hierarchy, one class of info per size/voice:
          name (sm/medium) → App Store metadata (xs muted prose) → download
          receipt (xs, darker — forensically the most telling line) →
          bundle id (11px mono — an identifier, not prose). */}
      <ItemContent className="gap-0.5">
        <ItemTitle className="flex items-center gap-2">
          {app.name}
          {app.version && (
            <span className="font-mono text-2xs font-normal tabular-nums text-muted-foreground/70">
              {app.version}
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
                  "bg-status-ok-soft text-status-ok-text",
              )}
            >
              {label}
            </Badge>
          )}
        </ItemTitle>
        {/* Seller · genre · subgenre · age rating · release year, from the
            backup's App Store metadata, when present. */}
        {app.seller || app.genre || app.subgenre || app.contentRating || app.released ? (
          <ItemDescription className="truncate">
            {[
              app.seller,
              app.genre,
              app.subgenre,
              app.contentRating,
              releasedYear(app.released),
            ]
              .filter(Boolean)
              .join(" · ")}
          </ItemDescription>
        ) : null}
        {/* The download receipt — device-specific and forensically the most
            telling: when this copy was installed, and by which Apple ID. */}
        {app.downloaded || app.appleId ? (
          <ItemDescription className="truncate text-xs text-foreground/80">
            {app.downloaded && (
              <>
                Downloaded{" "}
                <span className="font-medium">
                  {downloadedLabel(app.downloaded)}
                </span>
              </>
            )}
            {app.downloaded && app.appleId && " · "}
            {app.appleId && `via ${app.appleId}`}
          </ItemDescription>
        ) : null}
        <ItemDescription className="truncate font-mono text-2xs text-muted-foreground/60">
          {app.bundleId}
        </ItemDescription>

        {/* A summary on the row itself, not only once expanded: what this app was
            allowed to reach is the kind of thing you want to see while scanning
            the list, without opening anything.
            
            WHAT to badge comes from the module's own `highlight`, not from this
            file. It used to filter a literal "Decision" column for
            "Allowed"/"Limited" and print "none granted" — TCC's shape, hard-coded
            into a view whose premise is that it knows no artifact by name. The
            second apps-surface module made that visible as the nonsense
            "Data usage: none granted". A module with no `highlight` gets the
            record count and nothing invented on its behalf. */}
        {mine.length > 0 && (
          <div className="mt-1 flex flex-wrap items-center gap-1">
            {mine.map(({ artifact, rows }) => {
              const h = artifact.highlight;
              const matching = h
                ? rows.filter(
                    (r) =>
                      !h.whenColumn ||
                      h.whenAnyOf.includes(String(r[h.whenColumn] ?? "")),
                  )
                : [];
              const granted = h
                ? matching.map((r) => String(r[h.column] ?? "")).filter(Boolean)
                : [];
              const shown = granted.slice(0, 3);
              const rest = granted.length - shown.length;
              return (
                <Tooltip key={artifact.id}>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      onClick={() => setExpanded((v) => !v)}
                      className="flex flex-wrap items-center gap-1 rounded-md text-2xs text-muted-foreground hover:text-foreground"
                    >
                      <span className="font-medium">{artifact.name}:</span>
                      {shown.length > 0 ? (
                        <>
                          {shown.map((g) => (
                            <Badge key={g} variant="secondary" className="px-1.5 py-0">
                              {g}
                            </Badge>
                          ))}
                          {rest > 0 && <span>+{rest}</span>}
                        </>
                      ) : h?.noneLabel ? (
                        // Only the phrase the MODULE supplies. Saying nothing is
                        // the right default — silence claims less than a wrong
                        // sentence.
                        <span>{h.noneLabel}</span>
                      ) : null}
                      <span className="underline decoration-dotted">
                        {expanded ? "hide" : `${rows.length} recorded`}
                      </span>
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>{artifact.description}</TooltipContent>
                </Tooltip>
              );
            })}
          </div>
        )}

        {/* Inline expansion rather than a detail panel: the list virtualizer
            measures each row, so a row can grow. Reversible if per-app data
            outgrows it. */}
        {expanded &&
          mine.map(({ artifact, rows }) => (
            <div key={artifact.id} className="mt-2 rounded-md border bg-background/40 p-2">
              <p className="mb-1 text-2xs text-muted-foreground">{artifact.description}</p>
              <ArtifactTable
                artifact={artifact}
                rows={rows}
                hideColumns={artifact.joinColumn ? [artifact.joinColumn] : []}
              />
            </div>
          ))}
      </ItemContent>
      {app.support === "native" && (
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
      )}
    </Item>
  );
}
