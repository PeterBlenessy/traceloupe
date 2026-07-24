import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";
import { usePersistedState } from "@/lib/use-persisted-state";
import {
  Square, ExternalLink, EyeOff, HeartPulse, History, LayoutList, Loader2, MessageSquareWarning, MessagesSquare, NotebookText, Play, RotateCcw, ShieldCheck, ShieldUser, ShieldQuestion, Trash2, } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card, CardContent, CardDescription, CardHeader, CardTitle, } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { NoBackupState, ErrorState, ListSkeleton } from "@/components/view";
import { SettingsLink } from "@/components/settings-dialog-context";
import { useViewToolbar } from "@/components/toolbar-context";
import { makeYearPresets, useTimePresets } from "@/components/time-filter";
import { FilterControl } from "@/components/filter-control";
import { badgeGroup, timeGroup } from "@/components/filter-groups";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useSafetyScan } from "@/components/safety-scan-provider";
import { formatDuration, formatListTime, formatTimelineTime } from "@/lib/format";
import {
  client,
  type ContentCategory,
  type ContentFinding,
  type SafetyScanHistoryItem,
  type SafetyScanReport,
  type TimeRange,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";

const CATEGORY_LABEL: Record<ContentCategory, string> = {
  "threat-violence": "Threats & violence",
  "harassment-bullying": "Harassment & bullying",
  "sexual-content": "Sexual content",
  "grooming-exploitation": "Grooming & exploitation",
  "self-harm": "Self-harm",
  "hate-identity": "Hate & identity attacks",
  "coercive-control": "Coercive control",
  "scam-fraud": "Scams & fraud",
  "drugs-illegal": "Drugs & illegal activity",
};

const SEVERITY_META: Record<1 | 2 | 3, { label: string; badge: string }> = {
  3: {
    label: "Serious",
    badge: "bg-destructive text-white dark:bg-destructive/70 border-transparent",
  },
  2: {
    label: "Harmful",
    badge:
      "bg-amber-500/15 text-amber-700 dark:text-amber-400 border-amber-500/30",
  },
  1: {
    label: "Concerning",
    badge: "bg-muted text-muted-foreground border-transparent",
  },
};

/** The scanned period, from the stored [start, end] epoch bounds. */
function formatScanRange(start: number | null, end: number | null): string {
  if (start == null && end == null) return "all history";
  const fmt = (t: number) =>
    new Date(t * 1000).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
  if (start != null && end != null) {
    const s = new Date(start * 1000);
    const e = new Date(end * 1000);
    // A whole calendar year (end stored as Dec 31 23:59:59) reads as "2024".
    if (
      s.getFullYear() === e.getFullYear() &&
      s.getMonth() === 0 &&
      s.getDate() === 1 &&
      e.getMonth() === 11 &&
      e.getDate() === 31
    ) {
      return String(s.getFullYear());
    }
    return `${fmt(start)} – ${fmt(end)}`;
  }
  return start != null ? `since ${fmt(start)}` : `until ${fmt(end!)}`;
}

export function SafetyScanView() {
  const qc = useQueryClient();
  const { scan, startScan, cancelScan, preferredModelId } = useSafetyScan();
  // Same time filter as the rest of the app: the shared FilterControl popover
  // with a `timeGroup` — every period shown (24h/7d/30d + a chip per year the
  // backup spans), empty windows disabled via counts rather than hidden.
  // `timeGroup` emits a half-open [lo, hi); the scan backend's range end is
  // inclusive, so hi maps to `end = hi - 1` at start time.
  const { now, presets: basePresets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Which content to scan: "all" (default), "messages", or "notes".
  const [source, setSource] = useState("all");
  const [showDismissed, setShowDismissed] = useState(false);
  // Immediate feedback for Stop: the backend aborts within ~1s, but reflect the
  // click at once. Reset when the scan actually clears.
  const [stopping, setStopping] = useState(false);
  useEffect(() => {
    if (!scan) setStopping(false);
  }, [scan]);
  // Dismissible per-user; the classifier's accuracy is not yet validated on
  // real hardware, so the disclaimer stays until the user acknowledges it.
  const [expDismissed, setExpDismissed] = usePersistedState(
    "safety-scan:experimental-ack",
    false,
  );

  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  // The [min, max] message timestamps → a chip per year the backup covers,
  // replacing the single cumulative "this year" preset (as the Messages timeline
  // does), while keeping the recency windows.
  const { data: dateBounds } = useQuery({
    queryKey: ["messageDateBounds"],
    queryFn: () => client.messageDateBounds(),
    enabled: active === true,
  });
  const presets = useMemo(() => {
    if (!dateBounds) return basePresets;
    const minYear = new Date(dateBounds[0] * 1000).getFullYear();
    const maxYear = new Date(now * 1000).getFullYear();
    return [
      ...basePresets.filter((p) => p.key !== "year"),
      ...makeYearPresets(minYear, maxYear),
    ];
  }, [basePresets, dateBounds, now]);
  // Per-window item counts, so empty periods are shown-but-disabled (not
  // hidden). Messages and notes are counted separately so each period's number
  // can follow the selected Content source. These are item counts that match
  // the Messages / Notes views — never internal chunk counts.
  const presetRanges = useMemo(
    () => presets.map((p) => ({ lo: p.lo, hi: p.hi })),
    [presets],
  );
  const { data: presetMsgCounts } = useQuery({
    queryKey: ["messageRanges", now, presets.length],
    queryFn: () => client.countMessageRanges(presetRanges, null),
    enabled: active === true,
  });
  const { data: presetNoteCounts } = useQuery({
    queryKey: ["noteRanges", now, presets.length],
    queryFn: () => client.countNoteRanges(presetRanges),
    enabled: active === true,
  });
  // Counts for the currently-selected period, feeding the Content options
  // (All = messages + notes, Messages, Notes) for that period.
  const { data: rangeCounts } = useQuery({
    queryKey: ["safetyRangeCounts", range.lo, range.hi],
    queryFn: async () => {
      const [msg, note] = await Promise.all([
        client.countMessageRanges([range], null),
        client.countNoteRanges([range]),
      ]);
      return { messages: msg[0] ?? 0, notes: note[0] ?? 0 };
    },
    enabled: active === true,
  });
  // Each period's count follows the selected source, so the number next to a
  // period reflects exactly what that scan would cover.
  const presetCounts = useMemo(() => {
    if (!presetMsgCounts && !presetNoteCounts) return undefined;
    return presets.map((_, i) => {
      const m = presetMsgCounts?.[i] ?? 0;
      const n = presetNoteCounts?.[i] ?? 0;
      return source === "messages" ? m : source === "notes" ? n : m + n;
    });
  }, [presets, presetMsgCounts, presetNoteCounts, source]);
  // Item counts (matching the Messages / Notes views) for each Content option,
  // within the selected period.
  const sourceCounts = useMemo(() => {
    const m = rangeCounts?.messages ?? 0;
    const n = rangeCounts?.notes ?? 0;
    return { all: m + n, messages: m, notes: n };
  }, [rangeCounts]);
  const modelStatus = useQuery({
    queryKey: ["safetyScan", "modelStatus"],
    queryFn: () => client.getSafetyScanModelStatus(),
  });
  // The view shows ONE scan at a time (default: the latest); the history rail
  // switches which. Report and findings are always the selected scan's, so it
  // is never ambiguous which scan a finding belongs to.
  const history = useQuery({
    queryKey: ["safetyScan", "history"],
    queryFn: () => client.listSafetyScans(),
    enabled: active === true,
  });
  const [selectedScanId, setSelectedScanId] = useState<number | null>(null);
  const scans = history.data ?? [];
  const selectedScan =
    scans.find((s) => s.id === selectedScanId) ?? scans[0] ?? null;
  const findings = useQuery({
    queryKey: ["safetyScan", "findings", selectedScan?.id ?? null],
    queryFn: () => client.listContentFindings(selectedScan?.id),
    enabled: selectedScan != null,
  });
  const report = useQuery({
    queryKey: ["safetyScan", "report", selectedScan?.id ?? null],
    queryFn: () => client.getSafetyScanReport(selectedScan?.id),
    enabled: selectedScan != null,
  });

  const dismiss = useMutation({
    mutationFn: (f: { fingerprint: string; category: string; dismissed: boolean }) =>
      client.dismissContentFinding(f.fingerprint, f.category, f.dismissed),
    onSuccess: () => {
      // Refresh both the findings list and the inline badges (marks).
      qc.invalidateQueries({ queryKey: ["safetyScan", "findings"] });
      qc.invalidateQueries({ queryKey: ["safetyScan", "marks"] });
    },
  });

  // Publish just the title to the shared top toolbar (like every other view);
  // the scan's own controls stay in the run card since they're inputs to the
  // Run action, not filters over displayed content.
  useViewToolbar(
    useMemo(() => (active === true ? { title: "Safety Scan" } : null), [active]),
  );

  // Gate on an open backup, like every content view — there is nothing to scan
  // without one.
  if (active === false) {
    return (
      <NoBackupState
        icon={ShieldUser}
        title="Run a Safety Scan"
        lead="A local AI reads messages and notes and flags possible harmful content — a prompt to review conversations yourself, not a verdict."
        features={[
          { label: "Categories", detail: "Threats, harassment, grooming, self-harm, coercive control, scams, and more." },
          { label: "Time range", detail: "Scan all history, a specific year, or a custom date range." },
          { label: "Report & findings", detail: "A narrative report, per-thread summaries, and severity-ranked findings." },
          { label: "Follow through", detail: "Open the source conversation, and dismiss false positives for good." },
        ]}
        note="The model runs sandboxed on this Mac — nothing is uploaded, and the backup text never touches disk."
      />
    );
  }
  if (modelStatus.isPending) return <ListSkeleton />;
  if (modelStatus.isError) return <ErrorState error={modelStatus.error} />;
  const ms = modelStatus.data;
  const running = scan !== null;
  // Which model this scan will use: the user's Settings pick when it's still
  // installed, otherwise the recommended installed tier the backend reports.
  const installedIds = ms.models.filter((m) => m.installed).map((m) => m.id);
  const effectiveModelId =
    preferredModelId && installedIds.includes(preferredModelId)
      ? preferredModelId
      : ms.readyModelId;
  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
        {!expDismissed && (
          <Alert>
            <ShieldUser className="size-4" />
            <AlertTitle className="flex items-center gap-2">
              Experimental feature
            </AlertTitle>
            <AlertDescription className="flex flex-col gap-2">
              <span>
                Safety Scan is new and its classification accuracy has not yet
                been validated. Verdicts come from a local AI and can be
                wrong in both directions — treat every finding as a prompt to
                review the actual conversation yourself, and don't rely on a
                clean result as a guarantee.
              </span>
              <Button
                variant="outline"
                size="sm"
                className="w-fit"
                onClick={() => setExpDismissed(true)}
              >
                Got it
              </Button>
            </AlertDescription>
          </Alert>
        )}

        {ms.readyModelId === null ? (
          <NoModelPrompt />
        ) : (
          // One stable card — a running scan shows its progress inline below the
          // button rather than swapping the whole box (which was jumpy).
          <Card>
            {/* No card title — the view is already titled "Safety Scan" in the
                toolbar, and a "Run a scan" heading next to the Start button read
                as a second button. */}
            <CardHeader>
              <CardDescription>
                The scan runs entirely on this Mac: a local AI reads your
                messages and notes in small windows and flags possible threats,
                harassment, grooming, self-harm, coercive control, scams and
                more. Verdicts are probabilistic — treat each flag as something
                to review, not a fact. Already-scanned content is skipped
                automatically.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex flex-wrap items-center gap-3">
                {/* Time range left of the button (same row); the Filter popover
                    morphs rightward so it opens into the card, not the sidebar. */}
                <div
                  className={cn(
                    "flex items-center gap-2",
                    running && "pointer-events-none opacity-60",
                  )}
                >
                  <Label className="text-xs text-muted-foreground">
                    Scan
                  </Label>
                  <FilterControl
                    align="right"
                    groups={[
                      badgeGroup({
                        key: "source",
                        label: "Content",
                        description: "What to scan",
                        options: [
                          { value: "all", label: "All", count: sourceCounts.all },
                          { value: "messages", label: "Messages", count: sourceCounts.messages },
                          { value: "notes", label: "Notes", count: sourceCounts.notes },
                        ],
                        value: source,
                        onChange: setSource,
                      }),
                      timeGroup({
                        description: "Which period to scan, by date",
                        presets,
                        counts: presetCounts,
                        value: range,
                        onChange: setRange,
                      }),
                    ]}
                  />
                </div>
                {running ? (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      {/* A disabled trigger still needs its tooltip; the span
                          also keeps the layout stable while the label swaps
                          Stop → Stopping… (min-w prevents a mid-swap reflow). */}
                      <span className="inline-flex">
                        <Button
                          variant="outline"
                          className="min-w-28"
                          disabled={stopping}
                          onClick={() => {
                            setStopping(true);
                            cancelScan();
                          }}
                        >
                          {stopping ? (
                            <Loader2 className="size-4 animate-spin" />
                          ) : (
                            <Square className="size-4" />
                          )}
                          {stopping ? "Stopping…" : "Stop"}
                        </Button>
                      </span>
                    </TooltipTrigger>
                    <TooltipContent>
                      {stopping
                        ? "The scan aborts within a moment — progress so far is kept"
                        : "Stop the scan; progress so far is kept and resumable"}
                    </TooltipContent>
                  </Tooltip>
                ) : (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        onClick={() =>
                          void startScan({
                            modelId: effectiveModelId,
                            rangeStart: range.lo,
                            // timeGroup's hi is exclusive; the scan range end is
                            // inclusive, so step back one second.
                            rangeEnd: range.hi != null ? range.hi - 1 : null,
                            sources: source,
                          })
                        }
                      >
                        <Play className="size-4" /> Start Safety Scan
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      Scan the selected period and content with the local AI —
                      already-scanned content is skipped
                    </TooltipContent>
                  </Tooltip>
                )}
              </div>
              {running && scan && <ScanProgress scanEvent={scan} />}
            </CardContent>
          </Card>
        )}

        {history.isPending ? (
          <ListSkeleton rows={3} />
        ) : history.error ? (
          <ErrorState error={history.error} />
        ) : selectedScan ? (
          // Master–detail: the scan history rail on the left, the selected
          // scan's report + findings on the right. There is no "latest vs
          // history" split — the rail is the navigation, and everything on
          // the right belongs to the highlighted scan.
          <div className="grid items-start gap-4 lg:grid-cols-[280px_minmax(0,1fr)]">
            <ScanRail
              scans={scans}
              selectedId={selectedScan.id}
              onSelect={setSelectedScanId}
              // A 'running' DB row is only genuinely live while this app has a
              // scan in flight — after a crash/kill the row is stranded and
              // must read "Interrupted", not show a spinner.
              liveId={running ? (scans[0]?.id ?? null) : null}
            />
            <div className="min-w-0 space-y-4">
              <ScanReportCard
                scan={selectedScan}
                latest={selectedScan.id === scans[0]?.id}
                live={running && selectedScan.id === scans[0]?.id}
                onBackToLatest={() => setSelectedScanId(null)}
                // Resume = start a new scan with this scan's period + content;
                // checkpointing skips everything already classified. Only for
                // scans that didn't complete, and never while one is running.
                onResume={
                  !running && selectedScan.status !== "completed"
                    ? () => {
                        // Follow the resumed run: clear the pin so the view
                        // tracks the new (latest) scan as it appears, instead
                        // of staying on the old row with a Back-to-latest.
                        setSelectedScanId(null);
                        void startScan({
                          modelId: effectiveModelId,
                          rangeStart: selectedScan.rangeStart,
                          rangeEnd: selectedScan.rangeEnd,
                          sources: selectedScan.sources,
                        });
                      }
                    : undefined
                }
                report={report.data}
                findings={findings.data ?? []}
              />
              <FindingsList
                scan={selectedScan}
                findings={findings.data ?? []}
                showDismissed={showDismissed}
                setShowDismissed={setShowDismissed}
                onDismiss={(f, dismissed) =>
                  dismiss.mutate({
                    fingerprint: f.fingerprint,
                    category: f.category,
                    dismissed,
                  })
                }
              />
            </div>
          </div>
        ) : (
          <Card>
            <CardHeader>
              <CardTitle>No scan yet</CardTitle>
              <CardDescription>
                Run a Safety Scan to review this backup's messages and notes.
              </CardDescription>
            </CardHeader>
          </Card>
        )}
      </div>
    </div>
  );
}

/** The scan view can't run without a model. Model download lives in Settings →
 *  Safety (a one-time multi-GB setup, not per-scan content), so here we just
 *  point there. */
function NoModelPrompt() {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <ShieldQuestion className="size-4" /> A local AI is required
        </CardTitle>
        <CardDescription>
          Safety Scan analyzes your messages and notes with a local AI that runs entirely on this Mac. Download it once from{" "}
          <SettingsLink tab="safety">Settings → Safety</SettingsLink>, then come back here to run a scan.
        </CardDescription>
      </CardHeader>
    </Card>
  );
}

/** Inline scan progress shown inside the run card (below the button) so the
 *  card never gets swapped out mid-scan. */
function ScanProgress({
  scanEvent,
}: {
  scanEvent: NonNullable<ReturnType<typeof useSafetyScan>["scan"]>;
}) {
  const label =
    scanEvent.phase === "loading"
      ? "Loading the model…"
      : scanEvent.phase === "summarizing"
        ? "Writing the scan report…"
        : "Scanning…";
  const pct =
    scanEvent.phase === "classifying" && scanEvent.total > 0
      ? (scanEvent.done / scanEvent.total) * 100
      : null;
  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2 text-sm">
        <Loader2 className="size-4 animate-spin text-muted-foreground" />
        {label}
      </div>
      <Progress value={pct ?? undefined} />
      {scanEvent.phase === "classifying" && scanEvent.total > 0 && (
        <div className="text-xs text-muted-foreground">
          {Math.round((scanEvent.done / scanEvent.total) * 100)}% ·{" "}
          {scanEvent.findings} finding{scanEvent.findings === 1 ? "" : "s"} so
          far — you can leave this page; the scan keeps running.
        </div>
      )}
    </div>
  );
}

/** A label for a scan's status, in user terms. */
const SCAN_STATUS_LABEL: Record<string, string> = {
  completed: "Completed",
  cancelled: "Stopped",
  failed: "Failed",
  running: "Running",
  interrupted: "Interrupted",
};

/** Date-led identity for a scan: people remember *when* they scanned; the
 *  period covered is a property, shown in the subtitle. */
function scanTitle(s: SafetyScanHistoryItem): string {
  return formatTimelineTime(s.startedAt);
}

/** Human label for a scan's content scope. */
const SOURCES_LABEL: Record<string, string> = {
  all: "Messages & Notes",
  messages: "Messages",
  notes: "Notes",
};

/** The rail's compact outcome badge: one chip, colored by the worst severity.
 *  `live` says whether a scan is genuinely in flight right now — a DB row can
 *  be stranded 'running' after a crash/kill, and showing a spinner for it
 *  reads as "something is scanning" when nothing is. */
function ScanOutcomeBadge({
  scan,
  live,
}: {
  scan: SafetyScanHistoryItem;
  live: boolean;
}) {
  if (scan.status === "running")
    return live ? (
      <Badge variant="outline" className="shrink-0">
        <Loader2 className="size-3 animate-spin" /> running
      </Badge>
    ) : (
      <Badge variant="outline" className="shrink-0 text-muted-foreground">
        Interrupted
      </Badge>
    );
  // "Clean" is a completed scan's verdict — a stopped/failed scan with zero
  // findings just didn't get to look, so it shows its status instead.
  if (scan.findings === 0)
    return scan.status === "completed" ? (
      <Badge
        variant="outline"
        className="shrink-0 border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
      >
        Clean
      </Badge>
    ) : (
      <Badge variant="outline" className="shrink-0 text-muted-foreground">
        {SCAN_STATUS_LABEL[scan.status] ?? scan.status}
      </Badge>
    );
  const worst = scan.serious > 0 ? 3 : scan.harmful > 0 ? 2 : 1;
  return (
    <Badge className={cn("shrink-0", SEVERITY_META[worst as 1 | 2 | 3].badge)}>
      {scan.findings} finding{scan.findings === 1 ? "" : "s"}
    </Badge>
  );
}

/** The scan-history rail: every scan on this backup, newest first, with
 *  outcome filters and sorting. Selecting a row drives the whole right side. */
function ScanRail({
  scans,
  selectedId,
  onSelect,
  liveId,
}: {
  scans: SafetyScanHistoryItem[];
  selectedId: number;
  onSelect: (id: number | null) => void;
  /** The scan genuinely in flight right now, if any (see ScanOutcomeBadge). */
  liveId: number | null;
}) {
  const qc = useQueryClient();
  const [outcome, setOutcome] = useState("all");
  const [sort, setSort] = useState<SortState>({ by: "date", desc: true });
  const [confirmId, setConfirmId] = useState<number | null>(null);
  const del = useMutation({
    mutationFn: (id: number) => client.deleteSafetyScan(id),
    onSuccess: (_, id) => {
      setConfirmId(null);
      // If the deleted scan was selected, fall back to the latest.
      if (id === selectedId) onSelect(null);
      qc.invalidateQueries({ queryKey: ["safetyScan"] });
    },
    onError: (e) => {
      // Never fail silently — a dead confirm dialog reads as "the button is
      // broken". Close it and say what went wrong.
      setConfirmId(null);
      toast.error("Couldn't delete the scan", {
        description: e instanceof Error ? e.message : String(e),
      });
    },
  });

  const visible = useMemo(() => {
    let rows = scans.filter((s) =>
      outcome === "findings"
        ? s.findings > 0
        : outcome === "clean"
          ? s.findings === 0 && s.status === "completed"
          : outcome === "stopped"
            ? s.status === "cancelled" ||
              s.status === "failed" ||
              s.status === "interrupted"
            : true,
    );
    rows = sortItems(
      rows,
      sort.by === "findings" ? (s) => s.findings : (s) => s.startedAt,
      sort.desc,
    );
    return rows;
  }, [scans, outcome, sort]);

  // A filter must never hide the selection: if the selected scan gets
  // filtered out, move the selection to the first visible row so the rail
  // and the detail pane can't disagree about what's shown.
  useEffect(() => {
    if (visible.length > 0 && !visible.some((s) => s.id === selectedId))
      onSelect(visible[0].id);
  }, [visible, selectedId, onSelect]);

  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-sm">
          <History className="size-4" /> Scan history
        </CardTitle>
        <div className="flex items-center gap-2 pt-1">
          <FilterControl
            align="right"
            groups={[
              badgeGroup({
                key: "outcome",
                label: "Outcome",
                description: "Which scans to list",
                options: [
                  { value: "all", label: "All", count: scans.length },
                  {
                    value: "findings",
                    label: "With findings",
                    count: scans.filter((s) => s.findings > 0).length,
                  },
                  {
                    value: "clean",
                    label: "Clean",
                    count: scans.filter(
                      (s) => s.findings === 0 && s.status === "completed",
                    ).length,
                  },
                  {
                    value: "stopped",
                    label: "Stopped",
                    count: scans.filter(
                      (s) =>
                        s.status === "cancelled" ||
                        s.status === "failed" ||
                        s.status === "interrupted",
                    ).length,
                  },
                ],
                value: outcome,
                onChange: setOutcome,
              }),
            ]}
          />
          <SortControl
            fields={[
              { value: "date", label: "Date" },
              { value: "findings", label: "Findings" },
            ]}
            value={sort}
            onChange={setSort}
          />
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-1.5">
        {visible.length === 0 && (
          <p className="text-xs text-muted-foreground">No scans match.</p>
        )}
        {visible.map((s) => (
          <div
            key={s.id}
            role="button"
            tabIndex={0}
            aria-current={s.id === selectedId}
            onClick={() => onSelect(s.id)}
            onKeyDown={(e) => {
              // Keydown bubbles up from the nested delete button — only act on
              // keys pressed on the row itself, or Enter on the button would
              // select the row instead of deleting.
              if (e.target !== e.currentTarget) return;
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onSelect(s.id);
              }
            }}
            className={cn(
              "group flex cursor-pointer items-center justify-between gap-2 rounded-md border px-3 py-2 hover:bg-accent/50",
              s.id === selectedId && "border-primary/50 bg-primary/5",
            )}
          >
            <div className="min-w-0">
              <div className="truncate text-sm font-medium">{scanTitle(s)}</div>
              <div className="truncate text-xs text-muted-foreground">
                {SOURCES_LABEL[s.sources] ?? s.sources}
                {" · "}
                {formatScanRange(s.rangeStart, s.rangeEnd)}
                {" · "}
                {s.status === "running" && s.id !== liveId
                  ? "Interrupted"
                  : (SCAN_STATUS_LABEL[s.status] ?? s.status)}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <ScanOutcomeBadge scan={s} live={s.id === liveId} />
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-7 text-muted-foreground hover:text-destructive"
                    aria-label="Delete this scan"
                    onClick={(e) => {
                      e.stopPropagation();
                      setConfirmId(s.id);
                    }}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>Delete this scan</TooltipContent>
              </Tooltip>
            </div>
          </div>
        ))}
      </CardContent>

      <Dialog
        open={confirmId != null}
        onOpenChange={(o) => !o && setConfirmId(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete this scan?</DialogTitle>
            <DialogDescription>
              This scan's findings and report are removed from this backup.
              Findings you dismissed stay dismissed for future scans. This can't
              be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmId(null)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={del.isPending}
              onClick={() => confirmId != null && del.mutate(confirmId)}
            >
              {del.isPending ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <Trash2 className="size-4" />
              )}
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Card>
  );
}

/** The selected scan's report as a structured frame — stats header, narrative,
 *  per-conversation summaries, provenance footer — instead of one text blob. */
function ScanReportCard({
  scan,
  latest,
  live,
  onBackToLatest,
  onResume,
  report,
  findings,
}: {
  scan: SafetyScanHistoryItem;
  latest: boolean;
  /** True while this scan is genuinely in flight (not a stranded row). */
  live: boolean;
  onBackToLatest: () => void;
  /** Present when this scan can be resumed (didn't complete, nothing running). */
  onResume?: () => void;
  report: SafetyScanReport | undefined;
  findings: ContentFinding[];
}) {
  const navigate = useNavigate();
  const duration =
    scan.finishedAt != null ? formatDuration(scan.finishedAt - scan.startedAt) : "";
  // Deep-link a thread summary to its conversation via any of the scan's
  // findings that carries the resolved cache thread id.
  const threadIdOf = (identifier: string): number | null =>
    findings.find(
      (f) => f.threadIdentifier === identifier && f.threadId != null,
    )?.threadId ?? null;
  const clean = scan.findings === 0 && scan.status === "completed";

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between gap-2">
          <CardTitle className="flex items-center gap-2">
            {clean && (
              <ShieldCheck className="size-5 text-emerald-600 dark:text-emerald-400" />
            )}
            {clean ? "No harmful content flagged" : "Scan report"}
          </CardTitle>
          <div className="flex shrink-0 items-center gap-2">
            {onResume && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button size="sm" onClick={onResume}>
                    <Play className="size-4" /> Resume scan
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  Starts a new scan over the same period and content — anything
                  already scanned is skipped automatically
                </TooltipContent>
              </Tooltip>
            )}
            {!latest && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="outline" size="sm" onClick={onBackToLatest}>
                    Back to latest
                  </Button>
                </TooltipTrigger>
                <TooltipContent>Show the most recent scan again</TooltipContent>
              </Tooltip>
            )}
          </div>
        </div>
        <CardDescription>
          {SCAN_STATUS_LABEL[scan.status] ?? scan.status} {scanTitle(scan)}
          {" · scanned "}
          {formatScanRange(scan.rangeStart, scan.rangeEnd)}
          {!latest && " — a past scan; its findings are listed below"}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {/* Stats: what this scan amounted to, at a glance. */}
        <div className="flex flex-wrap items-center gap-1.5">
          {scan.serious > 0 && (
            <Badge className={SEVERITY_META[3].badge}>
              {scan.serious} serious
            </Badge>
          )}
          {scan.harmful > 0 && (
            <Badge className={SEVERITY_META[2].badge}>
              {scan.harmful} harmful
            </Badge>
          )}
          {scan.concerning > 0 && (
            <Badge className={SEVERITY_META[1].badge}>
              {scan.concerning} concerning
            </Badge>
          )}
          {scan.findings === 0 && (
            <Badge variant="outline" className="text-muted-foreground">
              no findings
            </Badge>
          )}
          {duration && (
            <Badge variant="outline" className="text-muted-foreground">
              took {duration}
            </Badge>
          )}
        </div>

        {report?.report ? (
          <p className="text-sm leading-relaxed">{report.report}</p>
        ) : (
          <p className="text-sm text-muted-foreground">
            {scan.status === "cancelled"
              ? "This scan was stopped before it finished, so it has no written report. Any findings it made before stopping are listed below."
              : scan.status === "interrupted" ||
                  (scan.status === "running" && !live)
                ? "This scan was interrupted before finishing (the app closed mid-scan). Its progress is checkpointed — Resume starts a new scan that skips everything already covered."
                : scan.status === "running"
                  ? "The scan is still running — findings appear below as they are made."
                  : clean
                    ? "The model flagged nothing in this period. That is not a guarantee — spot-check important conversations yourself."
                    : "This scan didn't produce a written report."}
          </p>
        )}

        {report != null && report.threadSummaries.length > 0 && (
          <div className="space-y-1">
            <div className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
              Per conversation
            </div>
            {report.threadSummaries.map(([thread, text]) => {
              const threadId = threadIdOf(thread);
              return (
                <div
                  key={thread}
                  className="flex items-baseline gap-2 border-t py-1.5 text-sm first:border-t-0"
                >
                  {threadId != null ? (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          className="shrink-0 rounded-sm font-medium text-primary underline-offset-2 outline-hidden hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                          onClick={() =>
                            navigate({
                              to: "/messages",
                              search: { thread: threadId },
                            })
                          }
                        >
                          {thread} →
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>Open this conversation</TooltipContent>
                    </Tooltip>
                  ) : (
                    <span className="shrink-0 font-medium">{thread}</span>
                  )}
                  <span className="text-muted-foreground">{text}</span>
                </div>
              );
            })}
          </div>
        )}

        {/* Provenance footer: the receipt this verdict carries. */}
        <div className="border-t pt-2 text-xs text-muted-foreground">
          Scanned {formatScanRange(scan.rangeStart, scan.rangeEnd)} ·{" "}
          {scan.model} · on-device
        </div>
      </CardContent>
    </Card>
  );
}

/** One compact finding row: severity · category · where · when · truncated
 *  rationale, with an inline dismiss control. Click for the full detail sheet. */
function FindingRow({
  finding: f,
  onOpen,
  onDismiss,
}: {
  finding: ContentFinding;
  onOpen: () => void;
  onDismiss: (dismissed: boolean) => void;
}) {
  const sev = SEVERITY_META[f.severity] ?? SEVERITY_META[1];
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        // Keydown bubbles up from the nested dismiss button — only act on keys
        // pressed on the row itself, or Enter on the button would open the
        // sheet instead of dismissing.
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      className={cn(
        "flex cursor-pointer flex-wrap items-center gap-x-2 gap-y-1 rounded-md border px-3 py-2 hover:bg-accent/50",
        f.dismissed && "opacity-55",
      )}
    >
      <Badge className={sev.badge}>{sev.label}</Badge>
      <Badge variant="outline">{CATEGORY_LABEL[f.category]}</Badge>
      <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
        {f.sourceKind === "note" ? (
          <>
            <NotebookText className="size-3" /> Note
          </>
        ) : (
          (f.threadIdentifier ?? "Conversation")
        )}
        {f.occurredAt != null && ` · ${formatListTime(f.occurredAt)}`}
      </span>
      {f.stale && (
        <Badge variant="outline" className="text-muted-foreground">
          <HeartPulse className="size-3" /> outdated
        </Badge>
      )}
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
        {f.rationale}
      </span>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            className="size-7 shrink-0 text-muted-foreground"
            aria-label={f.dismissed ? "Restore this finding" : "Dismiss this finding"}
            onClick={(e) => {
              e.stopPropagation();
              onDismiss(!f.dismissed);
            }}
          >
            {f.dismissed ? (
              <RotateCcw className="size-3.5" />
            ) : (
              <EyeOff className="size-3.5" />
            )}
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {f.dismissed
            ? "Restore — it was not a false positive after all"
            : "Dismiss as a false positive (persists across re-scans)"}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}

function FindingsList({
  scan,
  findings,
  showDismissed,
  setShowDismissed,
  onDismiss,
}: {
  scan: SafetyScanHistoryItem;
  findings: ContentFinding[];
  showDismissed: boolean;
  setShowDismissed: (v: boolean) => void;
  onDismiss: (f: ContentFinding, dismissed: boolean) => void;
}) {
  const [severity, setSeverity] = useState("all");
  const [sort, setSort] = useState<SortState>({ by: "severity", desc: true });
  const [grouped, setGrouped] = useState(false);
  const [selected, setSelected] = useState<ContentFinding | null>(null);

  const visible = useMemo(() => {
    let rows = findings.filter((f) => showDismissed || !f.dismissed);
    if (severity !== "all")
      rows = rows.filter((f) => f.severity === Number(severity));
    return sortItems(
      rows,
      sort.by === "date"
        ? (f) => f.occurredAt
        : // Secondary date order inside a severity band, so equal severities
          // don't shuffle: severity is the integer part, recency the fraction.
          (f) => f.severity * 1e12 + (f.occurredAt ?? 0),
      sort.desc,
    );
  }, [findings, showDismissed, severity, sort]);
  const dismissedCount = findings.filter((f) => f.dismissed).length;

  // Group by conversation (thread identifier); notes gather under "Notes".
  const groups = useMemo(() => {
    if (!grouped) return null;
    const map = new Map<string, ContentFinding[]>();
    for (const f of visible) {
      const key =
        f.sourceKind === "note" ? "Notes" : (f.threadIdentifier ?? "Conversation");
      (map.get(key) ?? map.set(key, []).get(key)!).push(f);
    }
    return [...map.entries()];
  }, [grouped, visible]);

  if (findings.length === 0) return null;
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <CardTitle className="flex items-center gap-2">
              <MessageSquareWarning className="size-4" /> Findings
              <Badge variant="secondary">{visible.length}</Badge>
            </CardTitle>
            <CardDescription>
              What the scan of {scanTitle(scan)} flagged. Dismiss anything you
              judge a false positive — dismissals persist across re-scans.
            </CardDescription>
          </div>
          <div className="flex items-center gap-2">
            <FilterControl
              groups={[
                badgeGroup({
                  key: "severity",
                  label: "Severity",
                  description: "Show only findings of one severity",
                  options: [
                    { value: "all", label: "All", count: findings.length },
                    { value: "3", label: "Serious", count: findings.filter((f) => f.severity === 3).length },
                    { value: "2", label: "Harmful", count: findings.filter((f) => f.severity === 2).length },
                    { value: "1", label: "Concerning", count: findings.filter((f) => f.severity === 1).length },
                  ],
                  value: severity,
                  onChange: setSeverity,
                }),
              ]}
            />
            <SortControl
              fields={[
                { value: "severity", label: "Severity" },
                { value: "date", label: "Date" },
              ]}
              value={sort}
              onChange={setSort}
            />
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={grouped ? "grouped" : "flat"}
              onValueChange={(v) => v && setGrouped(v === "grouped")}
            >
              <Tooltip>
                <TooltipTrigger asChild>
                  <ToggleGroupItem value="flat" aria-label="Flat list">
                    <LayoutList className="size-4" />
                  </ToggleGroupItem>
                </TooltipTrigger>
                <TooltipContent>One flat list, most severe first</TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <ToggleGroupItem value="grouped" aria-label="Group by conversation">
                    <MessagesSquare className="size-4" />
                  </ToggleGroupItem>
                </TooltipTrigger>
                <TooltipContent>Group by conversation</TooltipContent>
              </Tooltip>
            </ToggleGroup>
          </div>
        </div>
        {dismissedCount > 0 && (
          <div className="flex items-center gap-2 pt-1">
            <Switch
              id="show-dismissed"
              checked={showDismissed}
              onCheckedChange={setShowDismissed}
            />
            <Label
              htmlFor="show-dismissed"
              className="text-xs text-muted-foreground"
            >
              Show dismissed ({dismissedCount})
            </Label>
          </div>
        )}
      </CardHeader>
      <CardContent className="space-y-2">
        {groups
          ? groups.map(([name, rows]) => (
              <div key={name} className="space-y-1.5">
                <div className="flex items-center gap-2 pt-1 text-xs font-medium text-muted-foreground">
                  {name === "Notes" ? (
                    <NotebookText className="size-3.5" />
                  ) : (
                    <MessagesSquare className="size-3.5" />
                  )}
                  {name}
                  <Badge variant="outline" className="px-1.5 py-0 text-[10px]">
                    {rows.length}
                  </Badge>
                </div>
                {rows.map((f) => (
                  <FindingRow
                    key={`${f.fingerprint}:${f.category}`}
                    finding={f}
                    onOpen={() => setSelected(f)}
                    onDismiss={(d) => onDismiss(f, d)}
                  />
                ))}
              </div>
            ))
          : visible.map((f) => (
              <FindingRow
                key={`${f.fingerprint}:${f.category}`}
                finding={f}
                onOpen={() => setSelected(f)}
                onDismiss={(d) => onDismiss(f, d)}
              />
            ))}
        {visible.length === 0 && (
          <p className="text-xs text-muted-foreground">
            No findings match the current filter.
          </p>
        )}
      </CardContent>

      <FindingSheet
        scan={scan}
        finding={selected}
        onClose={() => setSelected(null)}
        onDismiss={(f, d) => {
          onDismiss(f, d);
          setSelected(null);
        }}
      />
    </Card>
  );
}

/** The full detail for one finding — the same interaction Security's findings
 *  table has: compact row → everything in a sheet. */
function FindingSheet({
  scan,
  finding,
  onClose,
  onDismiss,
}: {
  scan: SafetyScanHistoryItem;
  finding: ContentFinding | null;
  onClose: () => void;
  onDismiss: (f: ContentFinding, dismissed: boolean) => void;
}) {
  const navigate = useNavigate();
  return (
    <Sheet open={!!finding} onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full gap-0 sm:max-w-md">
        {finding && (
          <>
            <SheetHeader>
              <div className="flex items-center gap-2">
                <Badge
                  className={(SEVERITY_META[finding.severity] ?? SEVERITY_META[1]).badge}
                >
                  {(SEVERITY_META[finding.severity] ?? SEVERITY_META[1]).label}
                </Badge>
                <SheetTitle>{CATEGORY_LABEL[finding.category]}</SheetTitle>
              </div>
              <SheetDescription>
                {finding.sourceKind === "note"
                  ? "Flagged in a note"
                  : `Flagged in ${finding.threadIdentifier ?? "a conversation"}`}
                {finding.occurredAt != null &&
                  ` · ${formatTimelineTime(finding.occurredAt)}`}
              </SheetDescription>
            </SheetHeader>
            <div className="flex flex-col gap-3 px-4 pb-4 text-sm">
              <div className="flex flex-col gap-0.5">
                <span className="text-xs font-medium text-muted-foreground">
                  Found in scan
                </span>
                <span>{scanTitle(scan)}</span>
              </div>
              <div className="flex flex-col gap-0.5">
                <span className="text-xs font-medium text-muted-foreground">
                  Model rationale
                </span>
                <span>{finding.rationale}</span>
              </div>
              {finding.stale && (
                <p className="text-xs text-muted-foreground">
                  The source content changed since this verdict (or was removed
                  in a later import) — treat it as outdated.
                </p>
              )}
              <Separator />
              <p className="text-xs text-muted-foreground">
                A model verdict is a prompt to review the conversation yourself,
                not proof. False positives are expected.
              </p>
              <div className="flex flex-col gap-2">
                {finding.sourceKind === "message" && finding.threadId != null && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          navigate({
                            to: "/messages",
                            search: { thread: finding.threadId! },
                          });
                          onClose();
                        }}
                      >
                        <ExternalLink className="size-4" /> Open conversation
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      Read the flagged conversation in Messages
                    </TooltipContent>
                  </Tooltip>
                )}
                {/* Same null-source guard as the conversation button: a stale
                    finding whose note was removed has no id to open. */}
                {finding.sourceKind === "note" && finding.sourceId != null && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          navigate({
                            to: "/notes",
                            search: {
                              id: finding.sourceId!,
                              from: "safety",
                            },
                          });
                          onClose();
                        }}
                      >
                        <ExternalLink className="size-4" /> Open note
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      Read the flagged note in Notes
                    </TooltipContent>
                  </Tooltip>
                )}
                {finding.dismissed ? (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => onDismiss(finding, false)}
                      >
                        <RotateCcw className="size-4" /> Restore
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      Restore — it was not a false positive after all
                    </TooltipContent>
                  </Tooltip>
                ) : (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => onDismiss(finding, true)}
                      >
                        <EyeOff className="size-4" /> Dismiss as false positive
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      Hide this finding; the dismissal persists across re-scans
                    </TooltipContent>
                  </Tooltip>
                )}
              </div>
            </div>
          </>
        )}
      </SheetContent>
    </Sheet>
  );
}
