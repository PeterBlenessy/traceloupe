/**
 * The home dashboard's tiles (#157).
 *
 * **This file knows nothing about which modules exist.** Every tile's label,
 * route and icon arrive from the backend, so a kind of data added later shows up
 * here with no change to this file — and an icon name it does not recognise
 * falls back to a generic glyph rather than dropping the tile. The only thing
 * hardcoded is the icon *lookup*, and missing from it is a cosmetic outcome, not
 * a broken one.
 */
import { useMemo } from "react";
import { useNavigate } from "@tanstack/react-router";
import { Boxes } from "lucide-react";

import type { ModuleMetric } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useBoundedList } from "@/lib/bounded-list";
import { navFor, navOrder } from "@/lib/nav";
import { BrandIcon } from "@/lib/brand-icon";
import { serviceSlug } from "@/lib/apps";
import { formatCount } from "@/lib/format";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/** A module the nav has never heard of still gets a tile — right label, right
 *  link, right numbers — and this glyph until someone gives it one. */
const FALLBACK = Boxes;

/** How many facet icons a tile shows. Four fits the tile at every text size;
 *  the backend sends more so the view can skip ones it has no icon for. */
const FACET_CAP = 4;

/** The dashboard is one tile per kind of data. It cannot grow with the backup —
 *  only with the number of parsers — so it renders every row rather than
 *  virtualizing, and says so out loud if that ever stops being true. */
const MAX_TILES = 32;

/** Below this many rows a sparkline is noise, not a shape: four items across
 *  twenty-four buckets draws four spikes and twenty gaps, which says nothing a
 *  reader can use and invites reading meaning into the gaps. Those tiles keep
 *  their count and span and drop the strip. */
const MIN_FOR_SPARKLINE = 12;

/** "2017 – 2024", or a single year when the data does not span one. */
function formatSpan(firstAt: number | null, lastAt: number | null): string | null {
  if (firstAt == null || lastAt == null) return null;
  const y = (t: number) => new Date(t * 1000).getFullYear();
  const a = y(firstAt);
  const b = y(lastAt);
  return a === b ? `${a}` : `${a} – ${b}`;
}

/** A bar per bucket, scaled to the busiest one.
 *
 *  Deliberately unlabelled: a sparkline is a shape, and the tile already states
 *  the count and the span in words. Bars are `flex-1` rather than positioned, so
 *  the strip fits whatever width the grid gives it at any text size. */
function Sparkline({ series }: { series: number[] }) {
  const max = Math.max(1, ...series);
  return (
    <div aria-hidden="true" className="flex h-5 w-full items-end gap-0.5">
      {series.map((n, i) => (
        <div
          key={i}
          className="flex-1 rounded-[1px] bg-current"
          style={{
            height: `${Math.max(n > 0 ? 12 : 3, (n / max) * 100)}%`,
            opacity: n > 0 ? 0.55 : 0.18,
          }}
        />
      ))}
    </div>
  );
}

/** Every tile, whatever it holds, is this shell.
 *
 *  One implementation on purpose: the first version had a data tile and a scan
 *  tile built separately, and they rendered 119.8px and 96.5px in the same grid
 *  — a taller value line and a "Run" badge that outgrew its row. Fixed row
 *  heights here mean a tile cannot disagree with its neighbour no matter what
 *  goes in it, and they ride the text scale like everything else. */
function TileShell({
  route,
  fallbackLabel,
  facets,
  value,
  middle,
  footer,
  band,
  headerRight,
  wide = false,
  align = "center",
  onClick,
  tooltip,
}: {
  route: string;
  fallbackLabel: string;
  /** Drawn instead of the module icon when the module has parts worth naming. */
  facets?: { label: string; count: number }[];
  /** Replaces the facet row entirely — the scan tiles' severity strip. */
  band?: React.ReactNode;
  /** Right-aligned in the header, e.g. a scan's age. */
  headerRight?: React.ReactNode;
  /** Two columns wide. Scan tiles carry three facts and a data tile's width
   *  cannot hold them. */
  wide?: boolean;
  /** Scan tiles read left-to-right; data tiles centre their number. */
  align?: "center" | "start";
  value: React.ReactNode;
  middle: React.ReactNode;
  footer?: string;
  onClick: () => void;
  tooltip: React.ReactNode;
}) {
  // Label and icon come from the sidebar's entry for this route, so the two
  // surfaces cannot drift; the backend's values are the fallback for a module
  // the nav does not know about.
  const item = navFor(route);
  const label = item?.label ?? fallbackLabel;
  const Icon = item?.icon ?? FALLBACK;
  // Only facets we can actually DRAW. BrandIcon falls back to text initials,
  // and a row reading "COCOCOCO" (four unresolved bundle ids) is worse than no
  // row — so unresolvable brands are skipped, and a module whose facets are
  // categories rather than brands (Health) gets them as words instead.
  const all = facets ?? [];
  const brands = all.filter((f) => serviceSlug(f.label) != null).slice(0, FACET_CAP);
  const words =
    brands.length === 0 && all.length > 0
      ? all
          .filter((f) => !f.label.includes("."))
          .slice(0, 3)
          .map((f) => f.label)
      : [];
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onClick}
          className={cn(
            "group flex flex-col gap-1.5 rounded-lg border p-3",
            align === "start"
              ? "items-stretch text-left"
              : "items-center text-center",
            wide && "col-span-2",
            "transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
          )}
        >
          <div
            className={cn(
              "flex h-4 w-full items-center gap-1.5 text-muted-foreground",
              align === "start" ? "justify-start" : "justify-center",
            )}
          >
            <Icon className="size-3.5 shrink-0" />
            <span className="truncate text-xs">{label}</span>
            {headerRight && (
              <span className="ml-auto shrink-0 text-xs">{headerRight}</span>
            )}
          </div>
          {/* What is actually inside, when the module has parts: the services a
              conversation used, the channels, the Health categories. Reserved
              height either way so a tile with facets and one without still
              agree — the defect that shipped the first time round. */}
          <div
            className={cn(
              "flex h-4 w-full items-center gap-1 overflow-hidden",
              align === "start" ? "justify-start" : "justify-center",
            )}
          >
            {band}
            {brands.map((f) => (
              <BrandIcon
                key={f.label}
                slug={serviceSlug(f.label)}
                name={f.label}
                className="size-3.5"
              />
            ))}
            {words.length > 0 && (
              <span className="truncate text-3xs text-muted-foreground">
                {words.join(" · ")}
              </span>
            )}
          </div>
          <div
            className={cn(
              "flex h-7 w-full items-center",
              align === "start" ? "justify-start" : "justify-center",
            )}
          >
            {value}
          </div>
          <div
            className={cn(
              "flex h-5 w-full items-end",
              align === "start" ? "justify-start" : "justify-center",
            )}
          >
            {middle}
          </div>
          <div className="h-3.5 w-full truncate text-3xs text-muted-foreground">
            {footer ?? ""}
          </div>
        </button>
      </TooltipTrigger>
      <TooltipContent>{tooltip}</TooltipContent>
    </Tooltip>
  );
}

function Tile({ metric }: { metric: ModuleMetric }) {
  const navigate = useNavigate();
  const span = formatSpan(metric.firstAt, metric.lastAt);
  return (
    <TileShell
      route={metric.route}
      fallbackLabel={metric.label}
      facets={metric.facets}
      value={
        <span className="text-xl font-semibold tabular-nums">
          {formatCount(metric.count)}
        </span>
      }
      middle={
        metric.series.length > 0 && metric.count >= MIN_FOR_SPARKLINE ? (
          <div className="w-full text-foreground">
            <Sparkline series={metric.series} />
          </div>
        ) : null
      }
      footer={span ?? undefined}
      onClick={() => void navigate({ to: metric.route })}
      tooltip={
        <>
          {formatCount(metric.count)} {metric.label.toLowerCase()}
          {span ? ` · ${span}` : ""} — open
        </>
      }
    />
  );
}

/** Placeholder tiles while the metrics load, so the grid does not appear from
 *  nothing under the device header. */
export function DashboardTilesSkeleton() {
  return (
    <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 lg:grid-cols-4">
      {Array.from({ length: 6 }, (_, i) => (
        <div key={i} className="h-[104px] animate-pulse rounded-lg border bg-muted/40" />
      ))}
    </div>
  );
}

/** A scan's tile: status where a data tile has its count, and a Run action when
 *  it has never run.
 *
 *  Same grid, same weight as the data tiles, and never a banner — the home
 *  screen states what has happened rather than asking for something every time
 *  it is opened. */
/** A severity strip at tile scale — the report's chart language, three bands.
 *
 *  Same shape for both scans so the pair reads as a pair, even though one
 *  counts serious/harmful/concerning and the other critical/warning/info. */
function SeverityStrip({ bands }: { bands: { n: number; color: string }[] }) {
  const total = bands.reduce((a, b) => a + b.n, 0);
  if (total === 0) return null;
  return (
    <div aria-hidden="true" className="flex h-2 w-full overflow-hidden rounded-[2px]">
      {bands.map((b, i) =>
        b.n > 0 ? (
          <div key={i} style={{ width: `${(b.n / total) * 100}%`, background: b.color }} />
        ) : null,
      )}
    </div>
  );
}

/** A scan's tile: status where a data tile has its count, and the detail that
 *  makes a scan result mean something — the split, what changed, what it
 *  actually covered.
 *
 *  Double width, because those three lines do not fit in a data tile and a scan
 *  summarised as one number is the thing that made these tiles worth
 *  rethinking (#163). */
/** A scan's tile — through the SAME shell as a data tile.
 *
 *  They were separate components twice, and diverged in height twice (119.8 vs
 *  96.5, then 144 vs 122). One row layout is the only thing that actually stops
 *  it: whatever goes in the slots, the rows are the rows. */
export function ScanTile({
  route,
  label,
  status,
  bands,
  lines,
  onRun,
  onOpen,
}: {
  route: string;
  /** Fallback only; the sidebar's name for this route wins. */
  label: string;
  /** The headline: "never run", "3 days ago". */
  status: string;
  /** Severity split, drawn as one strip. */
  bands?: { n: number; color: string; label: string }[];
  /** Supporting facts, most useful first. */
  lines?: string[];
  /** Given only when the scan has never run. */
  onRun?: () => void;
  onOpen: () => void;
}) {
  const name = navFor(route)?.label ?? label;
  const shown = (lines ?? []).filter(Boolean);
  return (
    <TileShell
      route={route}
      fallbackLabel={label}
      wide
      align="start"
      headerRight={status}
      band={bands ? <SeverityStrip bands={bands} /> : null}
      value={
        onRun ? (
          <span className="inline-flex h-6 items-center rounded-md border px-2 text-xs font-medium group-hover:bg-background">
            Run
          </span>
        ) : (
          <span className="truncate text-sm">{shown[0] ?? ""}</span>
        )
      }
      middle={
        <span className="truncate text-xs text-muted-foreground">
          {onRun ? "" : shown[1] ?? ""}
        </span>
      }
      // Two facts on one line rather than a taller tile: the scan tiles are
      // double-width, so they have the room sideways that they do not have down.
      footer={onRun ? undefined : shown.slice(2).join(" · ") || undefined}
      onClick={onRun ?? onOpen}
      tooltip={onRun ? `Run ${name} on this backup` : `Open ${name}`}
    />
  );
}

export function DashboardTiles({
  metrics,
  children,
}: {
  metrics: ModuleMetric[];
  /** Scan tiles, rendered in the same grid so they share its rhythm. */
  children?: React.ReactNode;
}) {
  const sorted = useMemo(
    // Sidebar order, not busiest-first. Busiest-first changed with every backup
    // so no tile's position was ever learnable, and it put Messages before
    // Photos while the sidebar does the opposite — two navigational surfaces
    // showing the same destinations in different orders (#163).
    () => [...metrics].sort((a, b) => navOrder(a.route) - navOrder(b.route)),
    [metrics],
  );
  useBoundedList("home dashboard tiles", sorted.length, MAX_TILES);

  return (
    <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 lg:grid-cols-4">
      {sorted.map((m) => (
        <Tile key={m.id} metric={m} />
      ))}
      {children}
    </div>
  );
}
