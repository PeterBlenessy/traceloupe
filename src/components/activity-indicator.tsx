/**
 * One toolbar pill for everything the app is doing (#73).
 *
 * Long-running work used to get a pill each — Safety Scan, model download,
 * import — rendered side by side in a `shrink-0` row. Two problems: a Security
 * scan had no pill at all and so was invisible once you navigated away, and
 * concurrent work (they are independent, so all of them can run at once) pushed
 * the view's own title, filters and search out of the toolbar.
 *
 * So: one pill, constant width. It names the activity when there is exactly one
 * — preserving what a single scan looked like before — and collapses to a count
 * when there are several. Clicking opens the list, each row with its own
 * progress and a link to the view that owns it.
 *
 * Adding a new kind of background job means adding an entry to `useActivities`,
 * not another toolbar slot.
 */
import { Link } from "@tanstack/react-router";
import { useRouterState } from "@tanstack/react-router";
import { Loader2 } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Progress } from "@/components/ui/progress";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useImport } from "@/components/import-provider";
import { useSafetyScan } from "@/components/safety-scan-provider";
import { useSecurityScan } from "@/components/security-scan-provider";
import { useReimport } from "@/components/reimport-provider";

/** One thing the app is doing, as the toolbar needs to show it. */
export type Activity = {
  key: string;
  /** Short label, e.g. "Safety Scan". */
  title: string;
  /** What it's doing right now, e.g. "Scanning · 38%". */
  detail: string;
  /** 0–100 when known; null for indeterminate phases (model loading, verifying). */
  percent: number | null;
  /** Route that owns this activity, so a row can jump to it. */
  to: string;
};

/** Everything currently running, in a stable order. */
function useActivities(): Activity[] {
  const { scan, download, downloadingModelId } = useSafetyScan();
  const { active: importing } = useImport();
  const { progress: security } = useSecurityScan();
  const { running: reimporting } = useReimport();
  const out: Activity[] = [];

  if (scan) {
    const detail =
      scan.phase === "loading"
        ? "Loading model…"
        : scan.phase === "summarizing"
          ? "Writing report…"
          : scan.phase === "classifying" && scan.total > 0
            ? // Percentage, matching what the Safety Scan view itself shows —
              // the chunk count is an internal unit and means nothing to a
              // reader glancing at the toolbar.
              `Scanning · ${Math.round((scan.done / scan.total) * 100)}%`
            : "Scanning…";
    out.push({
      key: "safety-scan",
      title: "Safety Scan",
      detail,
      percent:
        scan.phase === "classifying" && scan.total > 0
          ? (scan.done / scan.total) * 100
          : null,
      to: "/safety-scan",
    });
  }

  if (download) {
    // The event is a discriminated union; only the downloading phase carries
    // byte counts, and terminal phases never reach here (the provider clears
    // `download` on them).
    const dl = download.phase === "downloading" ? download : null;
    const detail =
      download.phase === "verifying"
        ? "Verifying…"
        : dl && dl.total > 0
          ? `${Math.round((dl.received / dl.total) * 100)}%`
          : "Downloading…";
    out.push({
      key: "model-download",
      title: downloadingModelId ? "Downloading model" : "Model download",
      detail,
      percent: dl && dl.total > 0 ? (dl.received / dl.total) * 100 : null,
      to: "/safety-scan",
    });
  }

  if (importing) {
    const p = importing.progress;
    const detail =
      p == null
        ? "Starting…"
        : p.phase === "parsing"
          ? `Reading · ${p.current}/${p.total}`
          : p.phase === "indexing"
            ? `${p.step} · ${p.index}/${p.total}`
            : "Working…";
    const percent =
      p == null
        ? null
        : p.phase === "parsing" && p.total > 0
          ? (p.current / p.total) * 100
          : p.phase === "indexing" && p.total > 0
            ? (p.index / p.total) * 100
            : null;
    out.push({
      key: "import",
      title: `Importing ${importing.backup.deviceName ?? "backup"}`,
      detail,
      percent,
      to: "/",
    });
  }

  if (security) {
    out.push({
      key: "security-scan",
      title: "Security Check",
      detail:
        security.total > 0
          ? `${security.module} · ${security.index}/${security.total}`
          : security.module,
      percent:
        security.total > 0 ? (security.index / security.total) * 100 : null,
      to: "/security",
    });
  }

  // Re-imports have no progress events — only which modules are in flight — so
  // they list without a bar rather than with a fabricated one.
  for (const module of reimporting) {
    out.push({
      key: `reimport:${module}`,
      title: "Re-importing",
      detail: module,
      percent: null,
      to: "/",
    });
  }

  return out;
}

export function ActivityIndicator() {
  const activities = useActivities();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  if (activities.length === 0) return null;

  // With exactly one activity, and the user already looking at the view that
  // owns it, the pill is redundant — the view shows its own progress.
  if (activities.length === 1 && activities[0].to === pathname) return null;

  const single = activities.length === 1 ? activities[0] : null;
  const label = single
    ? `${single.title} · ${single.detail}`
    : `${activities.length} ongoing`;

  return (
    <Popover>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-label={
                single ? `${single.title}: ${single.detail}` : label
              }
              className="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            >
              <Loader2 className="size-3 animate-spin" />
              <span className="max-w-[14rem] truncate">{label}</span>
            </button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent>
          {single
            ? `${single.title} — click for details`
            : `${activities.length} things running — click to see each`}
        </TooltipContent>
      </Tooltip>
      <PopoverContent align="end" className="w-72 p-2">
        <div className="px-1 pb-1.5 text-[calc(0.65625rem*var(--text-scale))] font-medium uppercase tracking-wider text-muted-foreground">
          Ongoing
        </div>
        <ul className="space-y-1">
          {activities.map((a) => (
            <li key={a.key}>
              <Link
                to={a.to}
                className="block rounded-md px-2 py-1.5 hover:bg-accent"
              >
                <div className="flex items-baseline justify-between gap-2">
                  <span className="truncate text-sm font-medium">{a.title}</span>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {a.detail}
                  </span>
                </div>
                {/* Indeterminate phases get no bar rather than a fake one. */}
                {a.percent !== null && (
                  <Progress value={a.percent} className="mt-1.5 h-1" />
                )}
              </Link>
            </li>
          ))}
        </ul>
      </PopoverContent>
    </Popover>
  );
}
