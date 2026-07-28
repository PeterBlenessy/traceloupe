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
import {
  Boxes,
  Calendar,
  Camera,
  Globe,
  HeartPulse,
  Mic,
  MessageSquare,
  NotebookText,
  Phone,
  ListChecks,
  ShieldCheck,
  Users,
  Waypoints,
  type LucideIcon,
} from "lucide-react";

import type { ModuleMetric } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useBoundedList } from "@/lib/bounded-list";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/** Icon names the backend may send. An unknown name is not an error — the tile
 *  renders with the fallback and everything else about it still works. */
const ICONS: Record<string, LucideIcon> = {
  messages: MessageSquare,
  photos: Camera,
  contacts: Users,
  calls: Phone,
  safari: Globe,
  notes: NotebookText,
  recordings: Mic,
  calendar: Calendar,
  reminders: ListChecks,
  health: HeartPulse,
  interactions: Waypoints,
  apps: Boxes,
  security: ShieldCheck,
};
const FALLBACK = Boxes;

/** The dashboard is one tile per kind of data. It cannot grow with the backup —
 *  only with the number of parsers — so it renders every row rather than
 *  virtualizing, and says so out loud if that ever stops being true. */
const MAX_TILES = 32;

/** Below this many rows a sparkline is noise, not a shape: four items across
 *  twenty-four buckets draws four spikes and twenty gaps, which says nothing a
 *  reader can use and invites reading meaning into the gaps. Those tiles keep
 *  their count and span and drop the strip. */
const MIN_FOR_SPARKLINE = 12;

function formatCount(n: number): string {
  return n.toLocaleString();
}

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
  icon,
  label,
  value,
  middle,
  footer,
  onClick,
  tooltip,
}: {
  icon: string;
  label: string;
  value: React.ReactNode;
  middle: React.ReactNode;
  footer?: string;
  onClick: () => void;
  tooltip: React.ReactNode;
}) {
  const Icon = ICONS[icon] ?? FALLBACK;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onClick}
          className={cn(
            "group flex flex-col items-center gap-1.5 rounded-lg border p-3 text-center",
            "transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
          )}
        >
          <div className="flex h-4 w-full items-center justify-center gap-1.5 text-muted-foreground">
            <Icon className="size-3.5 shrink-0" />
            <span className="truncate text-xs">{label}</span>
          </div>
          <div className="flex h-7 w-full items-center justify-center">{value}</div>
          <div className="flex h-5 w-full items-end justify-center">{middle}</div>
          <div className="h-3.5 text-3xs text-muted-foreground">{footer ?? ""}</div>
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
      icon={metric.icon}
      label={metric.label}
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
export function ScanTile({
  label,
  icon,
  status,
  detail,
  onRun,
  onOpen,
}: {
  label: string;
  icon: string;
  /** The headline: "never run", "3 days ago". */
  status: string;
  /** The supporting line: "clean", "2 findings". */
  detail?: string;
  /** Given only when the scan has never run. */
  onRun?: () => void;
  onOpen: () => void;
}) {
  return (
    <TileShell
      icon={icon}
      label={label}
      value={
        <span
          className={cn(
            "truncate text-base font-semibold",
            onRun && "text-muted-foreground",
          )}
        >
          {status}
        </span>
      }
      middle={
        onRun ? (
          // h-5, exactly the row: a badge that outgrew it made this tile taller
          // than every other one in the grid.
          <span className="inline-flex h-5 items-center rounded-md border px-2 text-xs font-medium group-hover:bg-background">
            Run
          </span>
        ) : (
          <span className="truncate text-xs text-muted-foreground">{detail ?? ""}</span>
        )
      }
      onClick={onRun ?? onOpen}
      tooltip={onRun ? `Run ${label} on this backup` : `Open ${label}`}
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
    // Busiest first: the tile with the most in it is the most likely
    // destination, and the order then holds steady across backups rather than
    // following whatever order the backend happened to declare.
    () => [...metrics].sort((a, b) => b.count - a.count),
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
