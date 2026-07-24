import { useEffect, useMemo, useState } from "react";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";
import { usePersistedState } from "@/lib/use-persisted-state";
import {
  Square, ExternalLink, EyeOff, FileText, HeartPulse, History, LayoutList, Loader2, MessageSquare, MessageSquareWarning, MessagesSquare, NotebookText, Play, Printer, RotateCcw, RotateCw, ShieldCheck, ShieldUser, ShieldQuestion, Trash2, } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card, CardContent, CardDescription, CardHeader, CardTitle, } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
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
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { NoBackupState, ErrorState, ListSkeleton } from "@/components/view";
import { SettingsLink } from "@/components/settings-dialog-context";
import { useViewToolbar } from "@/components/toolbar-context";
import { makeYearPresets, useTimePresets } from "@/components/time-filter";
import { FilterControl } from "@/components/filter-control";
import { badgeGroup, timeGroup } from "@/components/filter-groups";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useSafetyScan } from "@/components/safety-scan-provider";
import {
  formatDateTimeYear,
  formatDuration,
  formatListTime,
  formatTimelineTime,
} from "@/lib/format";
import { serviceSlug } from "@/lib/apps";
import { BrandIcon, hasBrandIcon } from "@/lib/brand-icon";
import { useContactResolver } from "@/lib/use-contact-resolver";
import { useSettings } from "@/components/settings-provider";
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
  // Which scan's report document is open (from a history card's doc icon or the
  // detail header's Report button). Null = closed.
  const [reportScan, setReportScan] = useState<SafetyScanHistoryItem | null>(
    null,
  );
  const scans = history.data ?? [];
  const selectedScan =
    scans.find((s) => s.id === selectedScanId) ?? scans[0] ?? null;
  // The row genuinely in flight: a resumed run reopens an OLD row, so the live
  // one is found by status, never assumed to be the newest. During model load
  // no row is 'running' yet — nothing spins; the progress card covers it.
  // (`scan` is the provider's live event state — null when nothing runs.)
  const liveId =
    scan !== null
      ? (scans.find((s) => s.status === "running")?.id ?? null)
      : null;
  const findings = useQuery({
    queryKey: ["safetyScan", "findings", selectedScan?.id ?? null],
    queryFn: () => client.listContentFindings(selectedScan?.id),
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
  // Resume a non-completed scan from its history card: reopen THAT row (so the
  // view stays pinned to it) and continue it from where it stopped. Only Start
  // ever creates a new scan.
  const resumeScan = (scanId: number) => {
    setSelectedScanId(scanId);
    void startScan({ modelId: effectiveModelId, resumeScanId: scanId });
  };
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
          <div className="grid items-start gap-4 grid-cols-[420px_minmax(0,1fr)]">
            <ScanRail
              scans={scans}
              selectedId={selectedScan.id}
              onSelect={setSelectedScanId}
              liveId={liveId}
              onResume={resumeScan}
              running={running}
              onOpenReport={setReportScan}
            />
            <div className="min-w-0 space-y-4">
              {/* The report lives behind the history card's doc icon; the detail
                  side is just the findings (open the report from there). Only a
                  past scan needs the return-to-latest shortcut. */}
              {selectedScan.id !== scans[0]?.id && (
                <div className="flex justify-end">
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => setSelectedScanId(null)}
                  >
                    Back to latest
                  </Button>
                </div>
              )}
              {(findings.data?.length ?? 0) > 0 ? (
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
              ) : (
                <Card>
                  <CardContent className="flex flex-col items-center gap-2 py-10 text-center text-sm text-muted-foreground">
                    {selectedScan.status === "completed" ? (
                      <>
                        <ShieldCheck className="size-6 text-emerald-600 dark:text-emerald-400" />
                        Nothing flagged in this scan's scope. Open the report for
                        the full summary.
                      </>
                    ) : (
                      "No findings for this scan yet."
                    )}
                  </CardContent>
                </Card>
              )}
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
      {reportScan && (
        <SafetyReportDialog
          scan={reportScan}
          onOpenChange={(o) => !o && setReportScan(null)}
        />
      )}
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

/** Turn a stored model id (or an "e2b→e4b" cascade pair) into a readable
 *  label: "Gemma E2B → E4B (re-checked)" for a cascade, "Gemma E4B" otherwise. */
function modelLabel(raw: string): string {
  const pretty = (id: string) =>
    /e4b/i.test(id) ? "Gemma E4B" : /e2b/i.test(id) ? "Gemma E2B" : id;
  const [sweep, strong] = raw.split("→");
  if (strong && strong !== sweep) {
    return `${pretty(sweep)} → ${pretty(strong)} (flagged items re-checked)`;
  }
  return pretty(sweep);
}

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
  if (scan.status === "running" && live)
    return (
      <Badge variant="outline" className="shrink-0">
        <Loader2 className="size-3 animate-spin" /> running
      </Badge>
    );
  // A scan cut short mid-run — a stranded 'running' row (not live) or one
  // repaired to 'interrupted' — still found what it found before it stopped.
  const interrupted =
    scan.status === "interrupted" || (scan.status === "running" && !live);

  // The outcome pill: the findings count, or a verdict/status when there are
  // none. "Clean" is a completed scan's verdict — a stopped/failed/interrupted
  // scan with zero findings just didn't get to look, so it shows its status.
  const worst = scan.serious > 0 ? 3 : scan.harmful > 0 ? 2 : 1;
  const outcome =
    scan.findings > 0 ? (
      <Badge
        className={cn(
          "shrink-0 tabular-nums",
          SEVERITY_META[worst as 1 | 2 | 3].badge,
        )}
        title={`${scan.findings} finding${scan.findings === 1 ? "" : "s"}`}
      >
        {scan.findings}
      </Badge>
    ) : scan.status === "completed" ? (
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

  // Interrupted scans keep their findings pill; the state is shown as a label
  // to its LEFT. With zero findings the outcome badge already reads
  // "Interrupted", so it isn't doubled up.
  if (interrupted && scan.findings > 0)
    return (
      <div className="flex shrink-0 items-center gap-1.5">
        <Badge variant="outline" className="shrink-0 text-muted-foreground">
          Interrupted
        </Badge>
        {outcome}
      </div>
    );
  return outcome;
}

/** The scan-history rail: every scan on this backup, newest first, with
 *  outcome filters and sorting. Selecting a row drives the whole right side. */
function ScanRail({
  scans,
  selectedId,
  onSelect,
  liveId,
  onResume,
  running,
  onOpenReport,
}: {
  scans: SafetyScanHistoryItem[];
  selectedId: number;
  onSelect: (id: number | null) => void;
  /** The scan genuinely in flight right now, if any (see ScanOutcomeBadge). */
  liveId: number | null;
  /** Resume a non-completed scan from its card. */
  onResume: (id: number) => void;
  /** A scan is already in flight — no other scan can be resumed meanwhile. */
  running: boolean;
  /** Open the report document for a scan (owned by the parent view). */
  onOpenReport: (scan: SafetyScanHistoryItem) => void;
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
            <div className="flex shrink-0 items-center gap-0.5">
              <ScanOutcomeBadge scan={s} live={s.id === liveId} />
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-6 text-muted-foreground hover:text-foreground"
                    aria-label="Open scan report"
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenReport(s);
                    }}
                  >
                    <FileText className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>Open the report for this scan</TooltipContent>
              </Tooltip>
              {/* Resume lives on the card of the scan it continues — not in the
                  report — so it's next to the run it acts on. Shown for any
                  non-completed scan when nothing else is running. */}
              {!running && s.status !== "completed" && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="size-6 text-muted-foreground hover:text-foreground"
                      aria-label="Resume this scan"
                      onClick={(e) => {
                        e.stopPropagation();
                        onResume(s.id);
                      }}
                    >
                      <RotateCw className="size-3.5" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Resume this scan where it stopped</TooltipContent>
                </Tooltip>
              )}
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-6 text-muted-foreground hover:text-destructive"
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

/** Findings grouped for the report: message findings by conversation, notes
 *  together; each group sorted worst-severity first, groups likewise. */
function groupReportFindings(
  findings: ContentFinding[],
  labelOf: (id: string) => string,
): {
  key: string;
  label: string;
  threadId: number | null;
  isNote: boolean;
  findings: ContentFinding[];
}[] {
  const groups = new Map<string, ContentFinding[]>();
  for (const f of findings) {
    const key =
      f.sourceKind === "note" ? "__notes__" : (f.threadIdentifier ?? "__?__");
    const arr = groups.get(key) ?? [];
    arr.push(f);
    groups.set(key, arr);
  }
  const worst = (fs: ContentFinding[]) => Math.max(...fs.map((f) => f.severity));
  return [...groups.entries()]
    .map(([key, fs]) => ({
      key,
      isNote: key === "__notes__",
      label: key === "__notes__" ? "Notes" : labelOf(key),
      threadId: fs.find((f) => f.threadId != null)?.threadId ?? null,
      findings: [...fs].sort((a, b) => b.severity - a.severity),
    }))
    .sort((a, b) => worst(b.findings) - worst(a.findings));
}

/** The Safety Scan report as a styled, printable document: a mostly-deterministic
 *  frame (header, totals, findings grouped by conversation with resolved names)
 *  with the model's narrative + per-conversation prose spliced in. This is also
 *  the export source — Print renders exactly this (see the `safety-report-print`
 *  print styles in index.css). */
function SafetyReportDocument({
  scan,
  report,
  findings,
}: {
  scan: SafetyScanHistoryItem;
  report: SafetyScanReport | undefined;
  findings: ContentFinding[];
}) {
  const resolve = useContactResolver();
  const { showCascadeConfidence, includeReportSnippets } = useSettings();
  const { data: threads } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
  });
  const threadByIdent = useMemo(
    () => new Map((threads ?? []).map((t) => [t.identifier, t])),
    [threads],
  );
  const labelOf = (identifier: string): string => {
    const t = threadByIdent.get(identifier);
    if (!t) return resolve(identifier)?.name ?? identifier;
    if (t.displayName) return resolve(t.displayName)?.name ?? t.displayName;
    const first = t.participants[0];
    return first ? (resolve(first)?.name ?? first) : identifier;
  };
  const live = findings.filter((f) => !f.dismissed && !f.stale);
  // Verbatim flagged text is included ONLY when the user opts in (Settings →
  // Safety → Report). Fetched on demand per finding, never stored (ADR 0002).
  const snippetQueries = useQueries({
    queries: includeReportSnippets
      ? live.map((f) => ({
          queryKey: ["findingSnippet", f.sourceKind, f.sourceId],
          queryFn: () => client.contentFindingSnippet(f.sourceKind, f.sourceId),
        }))
      : [],
  });
  const snippetByFinding = new Map<number, string>();
  if (includeReportSnippets) {
    live.forEach((f, i) => {
      const text = snippetQueries[i]?.data?.text;
      if (text) snippetByFinding.set(f.id, text);
    });
  }
  const groups = groupReportFindings(live, labelOf);
  const summaryByIdent = new Map(report?.threadSummaries ?? []);
  const catCounts = new Map<ContentCategory, number>();
  for (const f of live) catCounts.set(f.category, (catCounts.get(f.category) ?? 0) + 1);

  const sev = (n: number) =>
    n === 3
      ? "text-destructive"
      : n === 2
        ? "text-amber-600 dark:text-amber-400"
        : "text-muted-foreground";

  return (
    <article className="safety-report-print mx-auto max-w-2xl space-y-6 text-sm">
      {/* Header */}
      <header className="space-y-1 border-b pb-4">
        <div className="flex items-center gap-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
          <ShieldUser className="size-4" /> Safety Scan report
        </div>
        <h1 className="text-xl font-semibold">{scanTitle(scan)}</h1>
        <p className="text-muted-foreground">
          {SOURCES_LABEL[scan.sources] ?? scan.sources} ·{" "}
          {formatScanRange(scan.rangeStart, scan.rangeEnd)} · {modelLabel(scan.model)} · on-device
        </p>
        <p className="text-xs text-muted-foreground">
          {scan.finishedAt != null && `Completed ${formatDateTimeYear(scan.finishedAt)}`}
          {scan.finishedAt != null &&
            ` · took ${formatDuration(scan.finishedAt - scan.startedAt)}`}
        </p>
      </header>

      {/* Totals */}
      <section className="grid grid-cols-4 gap-3 text-center">
        {(
          [
            ["Findings", live.length, ""],
            ["Serious", scan.serious, sev(3)],
            ["Harmful", scan.harmful, sev(2)],
            ["Concerning", scan.concerning, sev(1)],
          ] as [string, number, string][]
        ).map(([label, n, cls]) => (
          <div key={label} className="rounded-lg border p-3">
            <div className={cn("text-2xl font-semibold tabular-nums", cls)}>{n}</div>
            <div className="text-xs text-muted-foreground">{label}</div>
          </div>
        ))}
      </section>

      {catCounts.size > 0 && (
        <section className="flex flex-wrap gap-1.5">
          {[...catCounts.entries()]
            .sort((a, b) => b[1] - a[1])
            .map(([cat, n]) => (
              <Badge key={cat} variant="outline">
                {CATEGORY_LABEL[cat]} · {n}
              </Badge>
            ))}
        </section>
      )}

      {/* Narrative */}
      <section className="space-y-1">
        <h2 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
          Overview
        </h2>
        {report?.report ? (
          <p className="leading-relaxed">{report.report}</p>
        ) : live.length === 0 ? (
          <p className="text-muted-foreground">
            Nothing was flagged in this scan's scope. A clean scan is a review aid,
            not a guarantee — spot-check important conversations yourself.
          </p>
        ) : (
          <p className="text-muted-foreground">
            {live.length} finding{live.length === 1 ? "" : "s"} across{" "}
            {groups.length} conversation{groups.length === 1 ? "" : "s"} — see the
            breakdown below.
          </p>
        )}
      </section>

      {/* Per conversation */}
      {groups.length > 0 && (
        <section className="space-y-4">
          <h2 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            By conversation
          </h2>
          {groups.map((g) => {
            const prose = g.isNote ? undefined : summaryByIdent.get(g.key);
            return (
              <div key={g.key} className="space-y-2 rounded-lg border p-3">
                <div className="flex items-center gap-1.5 font-medium">
                  {g.isNote ? (
                    <NotebookText className="size-4 shrink-0 text-muted-foreground" />
                  ) : (
                    <MessageSquare className="size-4 shrink-0 text-muted-foreground" />
                  )}
                  {g.label}
                  <span className="text-xs font-normal text-muted-foreground">
                    · {g.findings.length} finding{g.findings.length === 1 ? "" : "s"}
                  </span>
                </div>
                {prose && <p className="text-muted-foreground">{prose}</p>}
                <ul className="space-y-1.5">
                  {g.findings.map((f) => (
                    <li
                      key={f.id}
                      className="space-y-1 border-t pt-1.5 first:border-t-0"
                    >
                      <div className="flex flex-wrap gap-x-2">
                        <span
                          className={cn("shrink-0 font-medium", sev(f.severity))}
                        >
                          {SEVERITY_META[f.severity]?.label ?? f.severity}
                        </span>
                        <span className="shrink-0 text-muted-foreground">
                          {CATEGORY_LABEL[f.category]}
                        </span>
                        {showCascadeConfidence && f.rechecked && (
                          <span className="shrink-0 text-emerald-600 dark:text-emerald-400">
                            ✓ confirmed
                          </span>
                        )}
                        <span className="text-muted-foreground">
                          {f.occurredAt != null &&
                            `${formatDateTimeYear(f.occurredAt)} — `}
                          {f.rationale}
                        </span>
                      </div>
                      {snippetByFinding.has(f.id) && (
                        <blockquote className="border-l-2 pl-3 text-xs whitespace-pre-wrap text-muted-foreground">
                          {snippetByFinding.get(f.id)}
                        </blockquote>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            );
          })}
        </section>
      )}

      <footer className="border-t pt-3 text-xs text-muted-foreground">
        Generated on-device by {modelLabel(scan.model)}.{" "}
        {includeReportSnippets
          ? "Includes the verbatim flagged content."
          : "Raw message content is not included."}
      </footer>
    </article>
  );
}

/** Opens the styled report for a scan in a dialog, with a Print/Export action
 *  that renders the same document to PDF via the system print dialog. */
function SafetyReportDialog({
  scan,
  onOpenChange,
}: {
  scan: SafetyScanHistoryItem;
  onOpenChange: (open: boolean) => void;
}) {
  const report = useQuery({
    queryKey: ["safetyScan", "report", scan.id],
    queryFn: () => client.getSafetyScanReport(scan.id),
  });
  const findings = useQuery({
    queryKey: ["safetyScan", "findings", scan.id],
    queryFn: () => client.listContentFindings(scan.id),
  });
  const loading = report.isPending || findings.isPending;
  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 p-0 sm:max-w-2xl">
        <DialogTitle className="sr-only">Scan report</DialogTitle>
        <DialogDescription className="sr-only">
          The selected scan's findings, formatted for review and export.
        </DialogDescription>
        {/* Toolbar is outside the printable root, so it never prints. pr-12
            clears the dialog's built-in close (✕) at top-right. */}
        <div className="flex items-center justify-between gap-2 border-b py-2 pl-4 pr-12 print:hidden">
          <span className="text-sm font-medium">Scan report</span>
          <Button
            size="sm"
            variant="outline"
            disabled={loading}
            onClick={() => window.print()}
          >
            <Printer className="size-4" /> Export PDF
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-6">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin" /> Building report…
            </div>
          ) : (
            <SafetyReportDocument
              scan={scan}
              report={report.data}
              findings={findings.data ?? []}
            />
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** One compact finding row: severity · category · where · when · truncated
 *  rationale, with an inline dismiss control. Click for the full detail sheet. */
function FindingRow({
  finding: f,
  onDismiss,
}: {
  finding: ContentFinding;
  onDismiss: (dismissed: boolean) => void;
}) {
  const navigate = useNavigate();
  const resolve = useContactResolver();
  const { showCascadeConfidence } = useSettings();
  // Resolve a handle (phone/email) to a contact name, exactly like the
  // conversation view — so the popover shows people, not raw phone numbers.
  const nameOf = (h: string | null | undefined) =>
    h ? (resolve(h)?.name ?? h) : null;
  const sev = SEVERITY_META[f.severity] ?? SEVERITY_META[1];
  // Fetch the flagged text only once the peek popover is first opened — no
  // upfront query per finding, and the raw text never lands in a list payload.
  const [peeked, setPeeked] = useState(false);
  const snippet = useQuery({
    queryKey: ["findingSnippet", f.sourceKind, f.sourceId],
    queryFn: () => client.contentFindingSnippet(f.sourceKind, f.sourceId),
    enabled: peeked,
  });
  // App glyph: the real brand mark (iMessage, TikTok, …) when the service has
  // one, else a note/message fallback. `f.service` is on the finding, so the
  // icon is right from first paint — no hover needed.
  const slug = serviceSlug(f.service);
  const glyph = (cls: string) =>
    hasBrandIcon(slug) ? (
      <BrandIcon slug={slug} name={f.service ?? ""} className={cls} />
    ) : f.sourceKind === "note" ? (
      <NotebookText className={cls} />
    ) : (
      <MessageSquare className={cls} />
    );
  const canOpen =
    (f.sourceKind === "message" && f.threadId != null) ||
    (f.sourceKind === "note" && f.sourceId != null);
  const openSource = () => {
    if (f.sourceKind === "message" && f.threadId != null) {
      navigate({
        to: "/messages",
        search: {
          thread: f.threadId,
          message: f.sourceId ?? undefined,
          from: "safety",
        },
      });
    } else if (f.sourceKind === "note" && f.sourceId != null) {
      navigate({ to: "/notes", search: { id: f.sourceId, from: "safety" } });
    }
  };
  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 rounded-md border px-3 py-2",
        f.dismissed && "opacity-55",
      )}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <Badge className={sev.badge}>{sev.label}</Badge>
        <Badge variant="outline">{CATEGORY_LABEL[f.category]}</Badge>
        {/* Confidence signal (Developer setting, off by default): a positive
            "Confirmed" mark when the strong tier (E4B) re-checked and kept it —
            two independent models agreeing. Only shown when true, so an E2B-only
            scan (nothing confirmed) isn't noisy. */}
        {showCascadeConfidence && f.rechecked && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Badge
                variant="outline"
                className="border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
              >
                <ShieldCheck className="size-3" /> Confirmed
              </Badge>
            </TooltipTrigger>
            <TooltipContent>
              Re-checked and kept by the stronger model (E4B) — not just the fast
              sweep tier
            </TooltipContent>
          </Tooltip>
        )}
        <span className="flex items-center gap-1 text-xs text-muted-foreground">
          {glyph("size-3.5 shrink-0")}
          {f.sourceKind === "note"
            ? "Note"
            : (nameOf(f.threadIdentifier) ?? "Conversation")}
          {f.occurredAt != null && ` · ${formatListTime(f.occurredAt)}`}
        </span>
        {f.stale && (
          <Badge variant="outline" className="text-muted-foreground">
            <HeartPulse className="size-3" /> outdated
          </Badge>
        )}
      </div>
      <p className="text-sm">{f.rationale}</p>
      <div className="flex flex-wrap items-center gap-1">
        {/* Peek: hover to reveal the flagged text (fetched on demand) and the
            jump to its source. Actions live here on the card, not a sheet. */}
        <HoverCard openDelay={120} closeDelay={80} onOpenChange={(o) => o && setPeeked(true)}>
          <HoverCardTrigger asChild>
            <Button variant="ghost" size="sm" className="gap-1.5 text-xs">
              {glyph("size-3.5")} View flagged text
            </Button>
          </HoverCardTrigger>
          <HoverCardContent className="w-96 text-sm">
            {/* Header: who sent it, when, and the app it came from. */}
            <div className="mb-2 flex items-center gap-1.5">
              {glyph("size-4 shrink-0")}
              <span className="truncate text-sm font-medium">
                {snippet.data?.sender === "Me"
                  ? `Me → ${nameOf(snippet.data.recipient) ?? "conversation"}`
                  : (nameOf(snippet.data?.sender) ??
                    (f.sourceKind === "note"
                      ? "Note"
                      : (nameOf(f.threadIdentifier) ?? "Conversation")))}
              </span>
              {(() => {
                const when = snippet.data?.occurredAt ?? f.occurredAt;
                return when != null ? (
                  <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                    {formatDateTimeYear(when)}
                  </span>
                ) : null;
              })()}
            </div>
            {snippet.isPending ? (
              <p className="text-xs text-muted-foreground">Loading…</p>
            ) : snippet.data ? (
              <blockquote className="border-l-2 pl-3 whitespace-pre-wrap text-muted-foreground">
                {snippet.data.text}
              </blockquote>
            ) : (
              <p className="text-xs text-muted-foreground">
                The source is no longer available (it may have changed since
                this scan).
              </p>
            )}
            {canOpen && (
              <Button
                variant="outline"
                size="sm"
                className="mt-3 w-full"
                onClick={openSource}
              >
                <ExternalLink className="size-4" />
                Open {f.sourceKind === "note" ? "note" : "conversation"}
              </Button>
            )}
          </HoverCardContent>
        </HoverCard>
        <span className="flex-1" />
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 text-xs text-muted-foreground"
              onClick={() => onDismiss(!f.dismissed)}
            >
              {f.dismissed ? (
                <>
                  <RotateCcw className="size-3.5" /> Restore
                </>
              ) : (
                <>
                  <EyeOff className="size-3.5" /> Dismiss
                </>
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
                    onDismiss={(d) => onDismiss(f, d)}
                  />
                ))}
              </div>
            ))
          : visible.map((f) => (
              <FindingRow
                key={`${f.fingerprint}:${f.category}`}
                finding={f}
                onDismiss={(d) => onDismiss(f, d)}
              />
            ))}
        {visible.length === 0 && (
          <p className="text-xs text-muted-foreground">
            No findings match the current filter.
          </p>
        )}
      </CardContent>
    </Card>
  );
}

/** The full detail for one finding — the same interaction Security's findings
 *  table has: compact row → everything in a sheet. */
