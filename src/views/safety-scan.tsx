import { useEffect, useMemo, useState } from "react";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useNavigate, useSearch} from "@tanstack/react-router";
import { toast } from "sonner";
import { usePersistedState } from "@/lib/use-persisted-state";
import {
  Square, ChartColumn, ChevronDown, ExternalLink, Filter, EyeOff, FileText, HeartPulse, History, LayoutList, Loader2, MessageSquare, MessageSquareWarning, MessagesSquare, NotebookText, Play, Printer, RotateCcw, RotateCw, ShieldCheck, ShieldUser, ShieldQuestion, Trash2, TriangleAlert, } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card, CardContent, CardDescription, CardHeader, CardTitle, } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Progress } from "@/components/ui/progress";
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
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { NoBackupState, ErrorState, ListSkeleton } from "@/components/view";
import { SettingsLink } from "@/components/settings-dialog-context";
import { useViewToolbar } from "@/components/toolbar-context";
import { makeYearPresets, useTimePresets } from "@/components/time-filter";
import { FilterControl } from "@/components/filter-control";
import { VirtualList } from "@/components/virtual-list";
import { LazyVirtualList } from "@/components/lazy-virtual-list";
import { badgeGroup, multiBadgeGroup, timeGroup } from "@/components/filter-groups";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useSafetyScan } from "@/components/safety-scan-provider";
import { dateFormat, formatDateTimeYear, formatDuration, formatListTime, formatTimelineTime } from "@/lib/format";
import { serviceSlug } from "@/lib/apps";
import { BrandIcon, hasBrandIcon } from "@/lib/brand-icon";
import { useContactResolver } from "@/lib/use-contact-resolver";
import { useSettings } from "@/components/settings-provider";
import {
  client,
  type ContentCategory,
  type ContentFinding,
  type ContentFindingCounts,
  type Suppression,
  type SuppressionScope,
  type FindingAnalytics,
  type SafetyScanHistoryItem,
  type SafetyScanReport,
  type TimeRange,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useListNavigation } from "@/lib/use-keyboard-nav";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { FindingCharts } from "@/components/safety-charts";

/** Exported because the home dashboard's Safety tile names the top categories.
 *  Shared rather than copied — a second table of these would drift. */
export const CATEGORY_LABEL: Record<ContentCategory, string> = {
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

/** Thread identifier → the name a reader recognises (group title, contact name,
 *  or the raw handle when nothing resolves). Shared by the report's per-
 *  conversation sections and its charts, so a bar and the block below it can
 *  never disagree about whose conversation it is. */
function useThreadLabel(): (identifier: string) => string {
  const resolve = useContactResolver();
  const { data: threads } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
  });
  const threadByIdent = useMemo(
    () => new Map((threads ?? []).map((t) => [t.identifier, t])),
    [threads],
  );
  return (identifier: string): string => {
    const t = threadByIdent.get(identifier);
    if (!t) return resolve(identifier)?.name ?? identifier;
    if (t.displayName) return resolve(t.displayName)?.name ?? t.displayName;
    const first = t.participants[0];
    return first ? (resolve(first)?.name ?? first) : identifier;
  };
}

const SEVERITY_META: Record<1 | 2 | 3, { label: string; badge: string }> = {
  3: {
    label: "Serious",
    badge: "bg-destructive text-white dark:bg-destructive/70 border-transparent",
  },
  2: {
    label: "Harmful",
    badge:
      "bg-status-warning-soft text-status-warning-text border-status-warning-line",
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
    dateFormat({ day: "numeric", month: "short", year: "numeric" }).format(
      new Date(t * 1000),
    );
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
  // Which content to scan — a multi-select over the real sources (each message
  // service present + Notes). null = "all selected" until the user narrows it.
  const [selectedSources, setSelectedSources] = useState<string[] | null>(null);
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
  // The real message services in this backup (iMessage/SMS/TikTok/…) + Notes,
  // as the Content multi-select's options.
  const { data: threadsForSources } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
    enabled: active === true,
  });
  const messageServices = useMemo(() => {
    const set = new Set<string>();
    for (const t of threadsForSources ?? []) if (t.service) set.add(t.service);
    return [...set].sort();
  }, [threadsForSources]);
  // Selectable tokens: each service, then "notes". Match the backend's
  // canonical `sources` string ("all" when everything is picked).
  const sourceTokens = useMemo(
    () => [...messageServices, "notes"],
    [messageServices],
  );
  const effectiveSelected = selectedSources ?? sourceTokens;
  const includesNotes = effectiveSelected.includes("notes");
  const includesMessages = effectiveSelected.some((s) => s !== "notes");
  const sourcesArg =
    effectiveSelected.length === sourceTokens.length && sourceTokens.length > 0
      ? "all"
      : effectiveSelected.join(",");
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
  // Each period's count follows the selected source, so the number next to a
  // period reflects exactly what that scan would cover.
  const presetCounts = useMemo(() => {
    if (!presetMsgCounts && !presetNoteCounts) return undefined;
    return presets.map((_, i) => {
      const m = includesMessages ? (presetMsgCounts?.[i] ?? 0) : 0;
      const n = includesNotes ? (presetNoteCounts?.[i] ?? 0) : 0;
      return m + n;
    });
  }, [presets, presetMsgCounts, presetNoteCounts, includesMessages, includesNotes]);
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
  // Counts, not rows. The panel pages its own rows now (#65) — this only has to
  // answer "is there anything to show?" and feed the filter pills.
  const findingCounts = useQuery({
    queryKey: ["safetyScan", "findingCounts", selectedScan?.id ?? null],
    queryFn: () => client.countContentFindings(selectedScan?.id),
    enabled: selectedScan != null,
  });

  // Reading a finding is a fact about the user, not an edit to the data — so it
  // refreshes the counts (the unread number moves) but deliberately does NOT
  // invalidate the findings pages, which would re-fetch and re-render the list
  // under the reader every time they opened a row.
  const markSeen = useMutation({
    mutationFn: (f: { fingerprint: string; category: string }) =>
      client.markContentFindingSeen(f.fingerprint, f.category),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["safetyScan", "findingCounts"] });
    },
  });

  // A rule can dismiss many findings at once, so everything the panel shows has
  // to re-read — unlike marking one finding seen.
  const addRule = useMutation({
    mutationFn: (r: {
      scope: SuppressionScope;
      value: string;
      category: string;
      /** Required for `content+sender`; ignored by the other scopes. */
      sender?: string | null;
      reason?: string;
    }) =>
      client.addSafetySuppression(
        r.scope,
        r.value,
        r.category,
        r.sender ?? null,
        r.reason,
      ),
    onSuccess: (n, r) => {
      qc.invalidateQueries({ queryKey: ["safetyScan"] });
      toast.success(
        n === 0
          ? "Rule saved — it will apply to future scans"
          : `Dismissed ${n} finding${n === 1 ? "" : "s"}`,
        {
          description: {
            "content+sender":
              "Only this, and only from them — the same thing from anyone else still gets flagged.",
            "content+any":
              "This exact thing from anyone, in every conversation, now and in future scans.",
            thread: `“${CATEGORY_LABEL[r.category as ContentCategory] ?? r.category}” in this conversation, now and in future scans. Other categories still get flagged.`,
            category: "Everything in this category, now and in future scans",
          }[r.scope],
        },
      );
    },
  });

  const dismiss = useMutation({
    mutationFn: (f: {
      fingerprint: string;
      category: string;
      dismissed: boolean;
      reason?: string;
    }) =>
      client.dismissContentFinding(f.fingerprint, f.category, f.dismissed, f.reason),
    // Silent failure here is worse than elsewhere: the row disappears
    // optimistically, so a failed dismiss looks exactly like a successful one
    // until the next refetch brings it back.
    onError: (e) => {
      toast.error("Couldn't dismiss this finding", {
        description: e instanceof Error ? e.message : String(e),
      });
    },
    onSuccess: () => {
      // Refresh both the findings list and the inline badges (marks).
      qc.invalidateQueries({ queryKey: ["safetyScan", "findings"] });
      qc.invalidateQueries({ queryKey: ["safetyScan", "findingCounts"] });
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
      <div className="flex min-h-0 flex-1 flex-col space-y-4 overflow-y-auto p-4">
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
                {/* Scope (Content + Time) sits left of the Start button on the
                    same row; the Filter popover morphs rightward so it opens
                    into the card, not the sidebar. */}
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
                      // Content sources live IN the popover, grouped exactly the
                      // way periods are grouped under Time (#57) — not as a
                      // separate chip row beside it. Multi-select over the real
                      // message services in this backup, plus Notes; everything
                      // selected means "scan everything".
                      multiBadgeGroup({
                        key: "content",
                        label: "Content",
                        description: "Which conversations and notes to scan",
                        options: sourceTokens.map((tok) => ({
                          value: tok,
                          label: tok === "notes" ? "Notes" : tok,
                          icon:
                            tok === "notes" ? (
                              <NotebookText className="size-3.5" />
                            ) : hasBrandIcon(serviceSlug(tok)) ? (
                              <BrandIcon
                                slug={serviceSlug(tok)}
                                name={tok}
                                className="size-3.5"
                              />
                            ) : (
                              <MessageSquare className="size-3.5" />
                            ),
                        })),
                        selected: effectiveSelected,
                        onToggle: (value) => {
                          const next = effectiveSelected.includes(value)
                            ? effectiveSelected.filter((v) => v !== value)
                            : [...effectiveSelected, value];
                          // Deselecting the last source would scan nothing; keep
                          // at least one so the Start button stays meaningful.
                          if (next.length === 0) return;
                          // Back to everything selected → null, the "all" state,
                          // so the summary shows no chips rather than one per
                          // source.
                          setSelectedSources(
                            next.length === sourceTokens.length ? null : next,
                          );
                        },
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
                        disabled={effectiveSelected.length === 0}
                        onClick={() =>
                          void startScan({
                            modelId: effectiveModelId,
                            rangeStart: range.lo,
                            // timeGroup's hi is exclusive; the scan range end is
                            // inclusive, so step back one second.
                            rangeEnd: range.hi != null ? range.hi - 1 : null,
                            sources: sourcesArg,
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
          // min-h-0 + flex-1 so this row takes exactly the height the window
          // leaves below the cards above, and each column scrolls inside it —
          // instead of a fixed-height list forcing the whole PAGE to scroll.
          <div className="grid min-h-0 flex-1 gap-4 grid-cols-[420px_minmax(0,1fr)]">
            <ScanRail
              scans={scans}
              selectedId={selectedScan.id}
              onSelect={setSelectedScanId}
              liveId={liveId}
              onResume={resumeScan}
              running={running}
            />
            <div className="flex min-h-0 min-w-0 flex-col space-y-4">
              {/* The report lives behind the history card's doc icon; the detail
                  side is just the findings. Rail selection handles navigation. */}
              {/* Rendered for EVERY selected scan, findings or not. The report
                  lives in its toolbar now, and a scan that found nothing has
                  the report most worth reading — "nothing was found" is a
                  result. Its finding-specific controls disable rather than
                  disappear (#171). */}
              <FindingsList
                  scan={selectedScan}
                  counts={findingCounts.data}
                  showDismissed={showDismissed}
                  setShowDismissed={setShowDismissed}
                  onDismiss={(f, dismissed, reason) =>
                    dismiss.mutate({
                      fingerprint: f.fingerprint,
                      category: f.category,
                      dismissed,
                      reason,
                    })
                  }
                  onSeen={(f) =>
                    markSeen.mutate({
                      fingerprint: f.fingerprint,
                      category: f.category,
                    })
                  }
                  onOpenReport={() => setReportScan(selectedScan)}
                  onRule={(scope, value, category, sender, reason) =>
                    addRule.mutate({ scope, value, category, sender, reason })
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
/** One conversation's summary inside the report (#18).
 *
 *  Scan end writes model prose only for the top few threads by severity, so the
 *  rest start empty and are generated when the reader asks. The backend returns a
 *  deterministic, findings-derived summary when no model server is live — real
 *  content rather than an error — and says which it gave, so this labels model
 *  prose and the computed fallback differently instead of passing one off as the
 *  other. Generated text is cached, so asking again is free. */
function ThreadSummaryBlock({
  scanId,
  threadRef,
  initial,
}: {
  scanId: number;
  threadRef: string;
  initial?: string;
}) {
  const [text, setText] = useState(initial);
  const [source, setSource] = useState<string | null>(initial ? "model" : null);
  const [busy, setBusy] = useState(false);

  // A different scan (or a re-scan) can arrive under the same mounted row.
  useEffect(() => {
    setText(initial);
    setSource(initial ? "model" : null);
  }, [initial, scanId, threadRef]);

  async function generate() {
    setBusy(true);
    try {
      const out = await client.generateThreadSummary(scanId, threadRef);
      if (out) {
        setText(out.content);
        setSource(out.source);
      }
    } catch (e) {
      toast.error("Couldn't summarize this conversation", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(false);
    }
  }

  if (text) {
    return (
      <div className="space-y-1">
        <p className="text-foreground/90">{text}</p>
        {source === "deterministic" && (
          <p className="text-xs text-muted-foreground">
            Summarized from the findings (no model loaded) — run a scan to get the
            classifier's own wording.
          </p>
        )}
      </div>
    );
  }
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={() => void generate()}
          className="print:hidden"
        >
          {busy ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <NotebookText className="size-4" />
          )}
          {busy ? "Summarizing…" : "Summarize this conversation"}
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {busy
          ? "Writing this conversation's summary"
          : "Summarize these findings (kept for next time; instant when no model is loaded)"}
      </TooltipContent>
    </Tooltip>
  );
}

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
      {/* Model load and report writing have no percentage. A determinate bar
          pinned at 0% reads as "stuck"; an indeterminate one reads as "working",
          which is the truth. */}
      <Progress
        value={pct ?? undefined}
        className={pct == null ? "[&>*]:animate-pulse" : undefined}
      />
      {scanEvent.phase === "classifying" && scanEvent.total > 0 && (
        <div className="text-xs text-muted-foreground">
          {Math.round((scanEvent.done / scanEvent.total) * 100)}% ·{" "}
          {/* Findings already in scope are NOT this run's work, and conflating
              the two produced a line that read as a contradiction: "0% · 8823
              findings so far". Most visible on a re-scan whose chunk cache no
              longer matches — it starts at 0% with every earlier finding
              already counted. */}
          {scanEvent.preexisting > 0 ? (
            <>
              {scanEvent.findings - scanEvent.preexisting} new ·{" "}
              {scanEvent.preexisting} from earlier scans of this range
            </>
          ) : (
            <>
              {scanEvent.findings} finding
              {scanEvent.findings === 1 ? "" : "s"} so far
            </>
          )}{" "}
          — you can leave this page; the scan keeps running.
        </div>
      )}
      {/* Honest surfacing of the power assertion the backend holds for the
          scan's lifetime (issue #32): a long scan can run for hours, and the
          app keeps the Mac from idle-sleeping so it doesn't stall mid-chunk. */}
      <div className="text-xs text-muted-foreground/80">
        Your Mac stays awake while the scan runs (the display can still sleep).
      </div>
    </div>
  );
}

/** A label for a scan's status, in user terms. */
/** Date-led identity for a scan: people remember *when* they scanned; the
 *  period covered is a property, shown in the subtitle. */
/** What a scan IS: the content it covers and the period it covers it over.
 *
 *  The date used to be the title, which made a scan look like an event. It is a
 *  configuration — re-running one updates its row rather than adding another
 *  (#171) — so the date moved down to the metadata line where it belongs. */
function scanTitle(s: SafetyScanHistoryItem): string {
  return `${formatSources(s.sources)} · ${formatScanRange(s.rangeStart, s.rangeEnd)}`;
}

/** Why a run ended the way it did, for the warning badge's tooltip.
 *
 *  Two of the three explain themselves and need nothing stored; only a failure
 *  carries a message, and if it did not, this badge would promise a reason and
 *  answer "it failed" — which the row already said. */
/** What the re-run button is called, which depends on what it will do. */
function rerunLabel(s: SafetyScanHistoryItem, live: boolean): string {
  if (live) return "This scan is running";
  return endedBadly(s, live) ? "Finish this scan" : "Run this scan again";
}

function endedBadly(s: SafetyScanHistoryItem, live: boolean): string | null {
  if (s.status === "running" && !live)
    return "The app closed while this was running. Progress is kept — run it again to finish.";
  if (s.status === "interrupted")
    return "The app closed while this was running. Progress is kept — run it again to finish.";
  if (s.status === "cancelled") return "You stopped this scan. Progress is kept.";
  if (s.status === "failed")
    return s.error ? `This scan failed: ${s.error}` : "This scan failed.";
  return null;
}

/** Human label for a scan's content scope — "all"/"messages"/"notes", or a
 *  comma-joined set of services + "notes" (e.g. "iMessage,TikTok,notes"). */
export function formatSources(sources: string): string {
  if (sources === "all") return "Messages & Notes";
  if (sources === "messages") return "Messages";
  if (sources === "notes") return "Notes";
  return sources
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean)
    .map((t) => (t === "notes" ? "Notes" : t === "messages" ? "Messages" : t))
    .join(", ");
}

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
  // The outcome pill: the findings count, or a verdict/status when there are
  // none. "Clean" is a completed scan's verdict — a stopped/failed/interrupted
  // scan with zero findings just didn't get to look, so it shows its status.
  const worst = scan.serious > 0 ? 3 : scan.harmful > 0 ? 2 : 1;
  const outcome =
    scan.findings > 0 ? (
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge
            // The severity is otherwise carried by hue alone here, which is
            // exactly what "Differentiate without colour" is about (index.css
            // adds a glyph when it is on).
            data-severity={worst}
            className={cn(
              "shrink-0 cursor-default tabular-nums",
              SEVERITY_META[worst as 1 | 2 | 3].badge,
            )}
          >
            {scan.findings}
          </Badge>
        </TooltipTrigger>
        <TooltipContent>
          <div className="mb-1 font-medium">
            {scan.findings} finding{scan.findings === 1 ? "" : "s"}
          </div>
          <ul className="space-y-0.5">
            {(
              [
                [3, scan.serious],
                [2, scan.harmful],
                [1, scan.concerning],
              ] as [1 | 2 | 3, number][]
            )
              .filter(([, n]) => n > 0)
              .map(([lvl, n]) => (
                <li key={lvl} className="flex items-center gap-1.5">
                  <span
                    className={cn(
                      "size-1.5 rounded-full",
                      lvl === 3
                        ? "bg-destructive"
                        : lvl === 2
                          ? "bg-status-warning"
                          : "bg-muted-foreground",
                    )}
                  />
                  {n} {SEVERITY_META[lvl].label.toLowerCase()}
                </li>
              ))}
          </ul>
        </TooltipContent>
      </Tooltip>
    ) : (
      // Zero findings, whatever the status. The pill is ONLY ever a findings
      // count now (#171) — "Clean", "Stopped" and "Interrupted" were a second
      // vocabulary saying what the row already said twice over, and anything
      // abnormal is said once, on the re-run action.
      <Badge variant="outline" className="shrink-0 tabular-nums text-muted-foreground">
        0
      </Badge>
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

  // ↑/↓ move the selection, Home/End jump. Selection rather than focus, so it
  // works with virtualised rows that are not mounted.
  const { listProps } = useListNavigation({
    items: visible,
    selectedId,
    onSelect,
    getId: (r) => r.id,
  });

  // A filter must never hide the selection: if the selected scan gets
  // filtered out, move the selection to the first visible row so the rail
  // and the detail pane can't disagree about what's shown.
  useEffect(() => {
    if (visible.length > 0 && !visible.some((s) => s.id === selectedId))
      onSelect(visible[0].id);
  }, [visible, selectedId, onSelect]);

  return (
    <Card className="flex h-full min-h-0 flex-col gap-3">
      <CardHeader className="shrink-0">
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
      {/* This list gains a row per scan and never sheds one, so it is unbounded
          in principle — virtualized rather than trusted to stay short (#67).
          Sized by the grid row, which is sized by the window (#79). */}
      {/* One tab stop for the whole list, with ↑/↓ moving the selection — the
          macOS model, and what makes honouring "Keyboard navigation" an
          improvement rather than just fewer tab stops. */}
      <CardContent
        {...listProps}
        className="flex min-h-0 flex-1 flex-col outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        {visible.length === 0 && (
          <p className="text-xs text-muted-foreground">No scans match.</p>
        )}
        <VirtualList
          items={visible}
          estimateSize={62}
          getKey={(s) => s.id}
          renderItem={(s) => (
          <div
            role="option"
            tabIndex={-1}
            /* The Density setting keys off this; the scan-history rows were the
               one list that ignored it. */
            data-slot="list-row"
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
              "group mb-1.5 flex cursor-pointer items-center justify-between gap-2 rounded-md border px-3 py-2 hover:bg-accent/50",
              s.id === selectedId && "border-primary/50 bg-primary/5",
            )}
          >
            {/* The count sits WITH the title, not on the right edge (#92). It
                used to live where the hover actions appear, so the actions
                landed on top of it — which made its severity-breakdown tooltip
                unreachable by construction: hovering the row to reach the pill
                was the very thing that covered it. */}
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 items-center gap-2">
                <span className="truncate text-sm font-medium">
                  {scanTitle(s)}
                </span>
                <ScanOutcomeBadge scan={s} live={s.id === liveId} />
              </div>
              {/* Metadata about the run, not its outcome. The status used to
                  be repeated here AND as a pill; the pill is now only ever a
                  findings count, and anything abnormal is said once, on the
                  re-run action. */}
              <div className="text-xs text-muted-foreground">
                Last run {formatTimelineTime(s.startedAt)}
              </div>
            </div>
            {/* In normal flow, so the right edge is permanently theirs and
                nothing can overlap. Dimmed rather than hidden: the space is
                reserved either way, and an invisible cluster just leaves the
                edge looking empty. They stay clickable and focusable at rest —
                the old version set pointer-events-none until hover.

                Width is reserved for the widest case (three actions; Resume
                only appears on an unfinished scan) and the cluster is
                right-aligned, so the icons form straight columns down the list
                instead of shifting row to row. Sized from the same control
                token as the buttons, so it can't drift from them. */}
            <div className="flex min-w-[calc(2*var(--control-h-sm)+0.25rem)] shrink-0 items-center justify-end gap-0.5 opacity-45 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
              {/* Re-run, on EVERY row — a scan is a configuration, so running it
                  again is the primary thing you do with one, not a recovery
                  action reserved for the ones that broke.

                  A run that ended abnormally carries a warning badge here and
                  the tooltip gives the reason. One shape for all three endings
                  rather than a glyph each: the badge means "this did not finish
                  normally", and the tooltip says which. Colour reinforces —
                  amber for stopped/interrupted, red for failed — so it still
                  reads with "Differentiate without colour" on. */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={running}
                    className={cn(
                      "group/rerun text-muted-foreground hover:text-foreground",
                      // The warning is the one thing that should catch the eye
                      // WITHOUT hovering, so it opts out of the cluster's
                      // resting dimness. Re-run and delete still dim.
                      endedBadly(s, s.id === liveId) && "opacity-100",
                    )}
                    aria-label={rerunLabel(s, s.id === liveId)}
                    onClick={(e) => {
                      e.stopPropagation();
                      onResume(s.id);
                    }}
                  >
                    {/* One icon, two jobs. At rest a run that ended badly shows
                        a warning; pointing at it — or tabbing to it — turns it
                        into the thing that fixes it. A 10px badge on a 14px
                        glyph was too small to read as either.

                        The swap is on focus-visible as well as hover: someone
                        tabbing the rail never hovers, and would otherwise see a
                        warning with no apparent action. */}
                    {endedBadly(s, s.id === liveId) ? (
                      <>
                        {/* One amber for all three endings, not red for failed.
                            Red is spoken for two feet away: in the findings list
                            it means a SERIOUS finding, the safety signal this
                            app exists to produce. A scan that errored is an
                            operational hiccup — nothing is wrong with the user's
                            data and it can simply be run again — so spending the
                            strongest colour in the palette on it dilutes red
                            where it matters.

                            It also matches what was already decided here: the
                            shape says "this did not finish normally" and the
                            tooltip says which. Two colours re-encoded that
                            distinction in a way nobody can decode at 14px — if
                            you know the convention you did not need the colour,
                            and if you do not, hovering tells you. */}
                        <TriangleAlert
                          aria-hidden="true"
                          className="size-3.5 text-status-warning group-hover/rerun:hidden group-focus-visible/rerun:hidden"
                        />
                        <RotateCw className="hidden size-3.5 group-hover/rerun:block group-focus-visible/rerun:block" />
                      </>
                    ) : (
                      <RotateCw className="size-3.5" />
                    )}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  {endedBadly(s, s.id === liveId) ??
                    "Run this scan again over the same content"}
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="text-muted-foreground hover:text-destructive"
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
        )}
        />
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

/** The report is a readable/printable DOCUMENT, so unlike the findings list it
 *  can't be virtualized — print and PDF export need every row in the DOM at
 *  once. So it is BOUNDED instead (#61): rendering ~8000 findings froze the
 *  machine. Severity-ordered, so the cap keeps the most serious; the shortfall
 *  is stated in the document rather than hidden. */
const REPORT_FINDINGS_CAP = 500;

/** The Safety Scan report as a styled, printable document: a mostly-deterministic
 *  frame (header, totals, findings grouped by conversation with resolved names)
 *  with the model's narrative + per-conversation prose spliced in. This is also
 *  the export source — Print renders exactly this (see the `safety-report-print`
 *  print styles in index.css). */
function SafetyReportDocument({
  scan,
  report,
  findings,
  liveTotal,
  analytics,
}: {
  scan: SafetyScanHistoryItem;
  report: SafetyScanReport | undefined;
  /** Already capped, filtered and severity-ordered by SQLite (#65). */
  findings: ContentFinding[];
  /** Every live, non-stale finding in scope — the denominator for the
   *  "N more not shown" line, which used to come from the array's length. */
  liveTotal: number;
  /** The charts' numbers, counted over ALL of them — see [`FindingCharts`]. */
  analytics: FindingAnalytics | undefined;
}) {
  const { showCascadeConfidence, includeReportSnippets } = useSettings();
  const labelOf = useThreadLabel();
  // The page arrives capped, dismissed- and stale-filtered, severity-ordered.
  const live = findings;
  // From the analytics count, not the pill count. Both are "in scope, not
  // dismissed, not stale" — but via two different SQL expressions, and the
  // header would then disagree with this line the first time they drifted. One
  // number, one query.
  const reportTotal = analytics?.charted ?? liveTotal;
  const omittedFromReport = Math.max(0, reportTotal - live.length);
  // Verbatim flagged text is included ONLY when the user opts in (Settings →
  // Safety → Report). Fetched on demand per finding, never stored (ADR 0002).
  // Bounded by the cap above: this is one IPC round trip PER finding, so before
  // the cap an 8000-finding report fired ~8000 `invoke` calls on open.
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

  /** How many of one severity the charts describe — summed across the category
   *  buckets, so it is the same number the bars add up to. */
  const severityTotal = (s: 1 | 2 | 3): number | undefined =>
    analytics?.byCategory.reduce(
      (n, b) => n + b.confirmed[s - 1] + b.unconfirmed[s - 1],
      0,
    );

  const sev = (n: number) =>
    n === 3
      ? "text-destructive"
      : n === 2
        ? "text-status-warning-text"
        : "text-muted-foreground";

  return (
    // The printable report reads at title3 (15px) — a document's reading size,
    // and since the ramp became the platform's this is simply `text-base`.
    <article className="safety-report-print mx-auto max-w-2xl space-y-9 text-base leading-relaxed">
      {/* Header */}
      <header className="space-y-1.5 border-b pb-5">
        <div className="flex items-center gap-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
          <ShieldUser className="size-4" /> Safety Scan report
        </div>
        <h1 className="text-2xl font-semibold">{scanTitle(scan)}</h1>
        <p className="text-sm text-muted-foreground">
          {formatSources(scan.sources)} ·{" "}
          {formatScanRange(scan.rangeStart, scan.rangeEnd)} · {modelLabel(scan.model)} · on-device
        </p>
        <p className="text-sm text-muted-foreground">
          {scan.finishedAt != null && `Completed ${formatDateTimeYear(scan.finishedAt)}`}
          {scan.finishedAt != null &&
            ` · took ${formatDuration(scan.finishedAt - scan.startedAt)}`}
        </p>
      </header>

      {/* Totals. All four from ONE population: the report excludes dismissed AND
          stale findings, but the severity split used to come off the scan row,
          which only excludes dismissed. With any stale finding in scope the
          three tiers then added up to more than the total printed beside them.
          The analytics counts carry the report's own filter, so they agree by
          construction; the scan row is only the fallback while they load. */}
      <section className="grid grid-cols-4 gap-3 text-center">
        {(
          [
            ["Findings", reportTotal, ""],
            ["Serious", severityTotal(3) ?? scan.serious, sev(3)],
            ["Harmful", severityTotal(2) ?? scan.harmful, sev(2)],
            ["Concerning", severityTotal(1) ?? scan.concerning, sev(1)],
          ] as [string, number, string][]
        ).map(([label, n, cls]) => (
          <div key={label} className="rounded-lg border p-3">
            <div className={cn("text-2xl font-semibold tabular-nums", cls)}>{n}</div>
            <div className="text-xs text-muted-foreground">{label}</div>
          </div>
        ))}
      </section>

      {/* Narrative */}
      <section className="space-y-2">
        <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Overview
        </h2>
        {report?.report ? (
          <p className="leading-relaxed">{report.report}</p>
        ) : reportTotal === 0 ? (
          <p className="text-muted-foreground">
            Nothing was flagged in this scan's scope. A clean scan is a review aid,
            not a guarantee — spot-check important conversations yourself.
          </p>
        ) : (
          <p className="text-muted-foreground">
            {reportTotal} finding{reportTotal === 1 ? "" : "s"} across{" "}
            {groups.length} conversation{groups.length === 1 ? "" : "s"} — see the
            breakdown below.
          </p>
        )}
      </section>

      {/* Analysis. Above the per-conversation blocks because it is the part a
          reader can take in at a glance — and because its numbers come from
          every finding, while the blocks below are the capped list. */}
      {analytics && (
        <FindingCharts
          analytics={analytics}
          categoryLabel={(slug) =>
            CATEGORY_LABEL[slug as ContentCategory] ?? slug
          }
          conversationLabel={labelOf}
        />
      )}

      {/* Per conversation */}
      {omittedFromReport > 0 && (
        <p className="text-sm text-muted-foreground">
          Listing the {live.length} most serious findings below.{" "}
          {omittedFromReport} lower-severity finding
          {omittedFromReport === 1 ? " is" : "s are"} counted in the totals above
          but not listed individually — open the Findings list to review them all.
        </p>
      )}
      {groups.length > 0 && (
        <section className="space-y-5">
          <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            By conversation
          </h2>
          {groups.map((g) => {
            const prose = g.isNote ? undefined : summaryByIdent.get(g.key);
            return (
              <div key={g.key} className="space-y-3 rounded-lg border p-4">
                <div className="flex items-center gap-2 text-base font-semibold">
                  {g.isNote ? (
                    <NotebookText className="size-4 shrink-0 text-muted-foreground" />
                  ) : (
                    <MessageSquare className="size-4 shrink-0 text-muted-foreground" />
                  )}
                  {g.label}
                  <span className="text-sm font-normal text-muted-foreground">
                    · {g.findings.length} finding{g.findings.length === 1 ? "" : "s"}
                  </span>
                </div>
                {/* Scan end only writes prose for the top few threads by
                    severity (#18); the rest are summarized here, on demand. */}
                {g.isNote ? null : (
                  <ThreadSummaryBlock
                    scanId={scan.id}
                    threadRef={g.key}
                    initial={prose}
                  />
                )}
                <ul className="space-y-4">
                  {g.findings.map((f) => (
                    <li
                      key={f.id}
                      className="space-y-1.5 border-t pt-4 first:border-t-0 first:pt-0"
                    >
                      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                        <span
                          className={cn(
                            "shrink-0 text-sm font-semibold",
                            sev(f.severity),
                          )}
                        >
                          {SEVERITY_META[f.severity]?.label ?? f.severity}
                        </span>
                        <span className="shrink-0 text-sm text-muted-foreground">
                          {CATEGORY_LABEL[f.category]}
                        </span>
                        {f.occurredAt != null && (
                          <span className="shrink-0 text-sm text-muted-foreground">
                            {formatDateTimeYear(f.occurredAt)}
                          </span>
                        )}
                        {showCascadeConfidence && f.rechecked && (
                          <span className="shrink-0 text-sm text-status-ok-text">
                            ✓ confirmed
                          </span>
                        )}
                      </div>
                      <p>{f.rationale}</p>
                      {snippetByFinding.has(f.id) && (
                        <blockquote className="border-l-2 pl-3 text-sm whitespace-pre-wrap text-muted-foreground">
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
  // The report is bounded at REPORT_FINDINGS_CAP, so ask for exactly that many
  // — severity-ordered by SQLite — instead of fetching every finding and
  // slicing (#65). The count supplies the "N more not shown" line, which used
  // to come from the length of the full array.
  const findings = useQuery({
    queryKey: ["safetyScan", "findings", "report", scan.id],
    queryFn: () =>
      client.listContentFindings(scan.id, {
        includeDismissed: false,
        excludeStale: true,
        sortBy: "severity",
        desc: true,
        groupByThread: false,
        offset: 0,
        limit: REPORT_FINDINGS_CAP,
      }),
  });
  const counts = useQuery({
    queryKey: ["safetyScan", "findingCounts", scan.id],
    queryFn: () => client.countContentFindings(scan.id),
  });
  // Counted over EVERY finding in scope, with the same filter the list above
  // uses — the charts must not describe the capped page (#66).
  const analytics = useQuery({
    queryKey: ["safetyScan", "analytics", "report", scan.id],
    queryFn: () =>
      client.contentFindingAnalytics(scan.id, {
        includeDismissed: false,
        excludeStale: true,
      }),
  });
  // The charts are part of the printable document, so Export must wait for them
  // too — a PDF with a hole where the analysis goes is worse than a slow one.
  const loading = report.isPending || findings.isPending || analytics.isPending;
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
              liveTotal={counts.data?.liveFresh ?? findings.data?.length ?? 0}
              analytics={analytics.data}
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
  onSeen,
  onRule,
}: {
  finding: ContentFinding;
  onDismiss: (dismissed: boolean, reason?: string) => void;
  /** Called the first time the row is expanded — the deliberate act that means
   *  the flagged text was read. */
  onSeen: () => void;
  /** Dismiss a whole conversation or category rather than one finding. */
  onRule: (
    scope: SuppressionScope,
    value: string,
    category: string,
    sender: string | null,
    reason?: string,
  ) => void;
}) {
  const navigate = useNavigate();
  const resolve = useContactResolver();
  const { showCascadeConfidence } = useSettings();
  // Resolve a handle (phone/email) to a contact name, exactly like the
  // conversation view — so the popover shows people, not raw phone numbers.
  const nameOf = (h: string | null | undefined) =>
    h ? (resolve(h)?.name ?? h) : null;
  const sev = SEVERITY_META[f.severity] ?? SEVERITY_META[1];
  // Fetch the flagged text only once the row is EXPANDED — no upfront query per
  // finding, and the raw text never lands in a list payload.
  //
  // This used to hang off a hover card, which fired while scrolling: findings
  // nobody read were fetched from the backup and would have counted as read.
  // Expanding is a click, so it cannot happen by accident (#169).
  const [expanded, setExpanded] = useState(false);
  const snippet = useQuery({
    queryKey: ["findingSnippet", f.sourceKind, f.sourceId],
    queryFn: () => client.contentFindingSnippet(f.sourceKind, f.sourceId),
    enabled: expanded,
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
      // `finding` rides along so the return chip can come back to THIS finding
      // rather than the top of the list (#224).
      navigate({
        to: "/messages",
        search: {
          thread: f.threadId,
          message: f.sourceId ?? undefined,
          from: "safety",
          finding: f.id,
        },
      });
    } else if (f.sourceKind === "note" && f.sourceId != null) {
      navigate({
        to: "/notes",
        search: { id: f.sourceId, from: "safety", finding: f.id },
      });
    }
  };
  return (
    <div
      // Density-aware: index.css overrides padding-block on this slot, which is
      // how every other list row responds to the Density setting (#78). A
      // hand-rolled row without it silently opts out.
      data-slot="list-row"
      className={cn(
        "flex flex-col gap-1.5 rounded-md border px-3 py-2",
        f.dismissed && "opacity-55",
        // Unread reads as unread, the way mail does. Without it the state lives
        // only in the database and the scan tile, and scrolling the list tells
        // you nothing about where you got to.
        !f.seen && !f.dismissed && "border-l-2 border-l-foreground/40",
      )}
    >
      {/* The whole header expands. A 14px chevron is a poor target; the header
          is the row's width, and the chevron is there to say it is clickable. */}
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => {
          const next = !expanded;
          setExpanded(next);
          // One-way: collapsing is not un-reading.
          if (next && !f.seen) onSeen();
        }}
        className="flex w-full flex-wrap items-center gap-x-2 gap-y-1 rounded-sm text-left focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        <Badge className={sev.badge}>{sev.label}</Badge>
        <Badge variant="outline">{CATEGORY_LABEL[f.category]}</Badge>
        {/* Confidence signal (Developer setting, off by default): a positive
            "Confirmed" mark when the strong tier (E4B) re-checked and kept it —
            two independent models agreeing. Only shown when true, so an E2B-only
            scan (nothing confirmed) isn't noisy. */}
        {showCascadeConfidence && f.rechecked && (
          <Badge
            variant="outline"
            className="border-status-ok-line text-status-ok-text"
          >
            <ShieldCheck className="size-3" /> Confirmed
          </Badge>
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
        <span className="flex-1" />
        <ChevronDown
          className={cn(
            "size-4 shrink-0 text-muted-foreground transition-transform",
            expanded && "rotate-180",
          )}
        />
      </button>
      <p className={cn("text-sm", !f.seen && !f.dismissed && "font-medium")}>
        {f.rationale}
      </p>

      {/* Expanded: the evidence, and the only place a verdict can be given.
          Dismiss lives here on purpose — you cannot reject a finding you have
          not read, which also makes "dismissed implies seen" true by
          construction rather than a rule to maintain. */}
      {expanded && (
        <div className="mt-1 space-y-2 border-t pt-2">
          {snippet.isPending ? (
            <p className="text-xs text-muted-foreground">Loading…</p>
          ) : snippet.data ? (
            <>
              <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                {glyph("size-3.5 shrink-0")}
                <span className="truncate font-medium">
                  {snippet.data.sender === "Me"
                    ? `Me → ${nameOf(snippet.data.recipient) ?? "conversation"}`
                    : (nameOf(snippet.data.sender) ??
                      (f.sourceKind === "note" ? "Note" : "Unknown"))}
                </span>
                {snippet.data.occurredAt != null && (
                  <span>· {formatDateTimeYear(snippet.data.occurredAt)}</span>
                )}
              </div>
              <blockquote className="max-h-60 overflow-y-auto border-l-2 pl-3 text-sm whitespace-pre-wrap text-muted-foreground">
                {snippet.data.text}
              </blockquote>
            </>
          ) : (
            <p className="text-xs text-muted-foreground">
              The source is no longer available (it may have changed since this
              scan).
            </p>
          )}

          {f.dismissed && f.dismissReason && (
            <p className="text-xs text-muted-foreground">
              Dismissed: {f.dismissReason}
            </p>
          )}

          <div className="flex flex-wrap items-center gap-1">
            {canOpen && (
              <Button variant="outline" size="sm" onClick={openSource}>
                <ExternalLink className="size-4" />
                Open {f.sourceKind === "note" ? "note" : "conversation"}
              </Button>
            )}
            <span className="flex-1" />
            {f.dismissed ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="gap-1.5 text-xs text-muted-foreground"
                    onClick={() => onDismiss(!f.dismissed, undefined)}
                  >
                    <RotateCcw className="size-3.5" /> Restore
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  Restore — it was not a false positive after all
                </TooltipContent>
              </Tooltip>
            ) : (
              <DismissPopover finding={f} onDismiss={onDismiss} onRule={onRule} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/** The standing rules, and the only way to undo one.
 *
 *  Without this a scoped dismissal is a one-way door: you could tell the app to
 *  dismiss a whole conversation for ever and have no way to take it back. */
function SuppressionChip() {
  const qc = useQueryClient();
  const { data: rules } = useQuery({
    queryKey: ["safetyScan", "suppressions"],
    queryFn: () => client.listSafetySuppressions(),
  });
  const remove = useMutation({
    mutationFn: (r: Suppression) =>
      client.removeSafetySuppression(r.scope, r.value, r.category, r.sender),
    onSuccess: (back) => {
      qc.invalidateQueries({ queryKey: ["safetyScan"] });
      // Say what came back. The earlier behaviour left a removed rule's
      // dismissals in place, which meant the blind spot outlived the rule with
      // nothing left pointing at it.
      toast.success(
        back === 0
          ? "Rule removed"
          : `Rule removed — ${back} finding${back === 1 ? "" : "s"} back in view`,
        {
          description:
            back === 0
              ? "It was not dismissing anything."
              : "Anything you dismissed by hand stays dismissed.",
        },
      );
    },
  });
  const resolve = useThreadLabel();
  const resolveContact = useContactResolver();
  /** A rule's subject, named the way the rest of the app names people. */
  const subject = (r: Suppression): string => {
    if (r.scope === "content+sender") {
      const who =
        r.sender === "me"
          ? "you"
          : (resolveContact(r.sender)?.name ?? r.sender);
      return `“${displayKey(r.value)}” from ${who}`;
    }
    if (r.scope === "content+any") return `“${displayKey(r.value)}” from anyone`;
    if (r.scope === "thread") return (resolve(r.value) ?? r.value);
    return CATEGORY_LABEL[r.value as ContentCategory] ?? r.value;
  };
  if (!rules?.length) return null;

  return (
    <Popover>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <Button variant="ghost" size="sm" className="gap-1.5 text-xs text-muted-foreground">
              <Filter className="size-3.5" />
              {rules.length} rule{rules.length === 1 ? "" : "s"}
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent>
          Conversations and categories you have set to dismiss automatically
        </TooltipContent>
      </Tooltip>
      <PopoverContent align="end" className="w-96 space-y-2">
        <p className="text-xs text-muted-foreground">
          These dismiss matching findings automatically, in this scan and future
          ones. Nothing is hidden — they are counted under “Show dismissed”. The
          number beside each rule is what it is swallowing right now.
        </p>
        <ul className="space-y-1">
          {rules.map((r) => (
            <li
              key={`${r.scope}:${r.value}:${r.category ?? "*"}`}
              className="flex items-center gap-2 rounded-md border px-2 py-1.5"
            >
              <div className="min-w-0 flex-1">
                <p className="truncate text-xs font-medium">{subject(r)}</p>
                <p className="truncate text-3xs text-muted-foreground">
                  {r.scope === "thread"
                    ? r.category
                      ? `${CATEGORY_LABEL[r.category as ContentCategory] ?? r.category} in this conversation`
                      : "Every category in this conversation — made before rules were per-category"
                    : r.scope === "category"
                      ? "Whole category"
                      : (CATEGORY_LABEL[r.category as ContentCategory] ??
                        r.category ??
                        "")}
                  {r.reason ? ` · ${r.reason}` : ""}
                </p>
              </div>
              {/* What it is actually swallowing. A rule you cannot see the
                  effect of is the dangerous version of this feature. */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <span
                    className={
                      r.hits === 0
                        ? "shrink-0 rounded-full border border-dashed px-1.5 py-0.5 text-3xs text-muted-foreground"
                        : "shrink-0 rounded-full bg-muted px-1.5 py-0.5 text-3xs tabular-nums"
                    }
                  >
                    {r.hits === 0 ? "nothing" : r.hits}
                  </span>
                </TooltipTrigger>
                <TooltipContent>
                  {r.hits === 0
                    ? "This rule is not dismissing anything — it is either stale or was never needed"
                    : `Dismissing ${r.hits} finding${r.hits === 1 ? "" : "s"} right now. Turn on “Show dismissed” to read them.`}
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label="Remove this rule"
                    onClick={() => remove.mutate(r)}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  Stop dismissing these automatically. What this rule dismissed
                  comes back; anything you dismissed by hand stays dismissed.
                </TooltipContent>
              </Tooltip>
            </li>
          ))}
        </ul>
      </PopoverContent>
    </Popover>
  );
}

/** A content key rendered as the user actually saw it.
 *
 *  The stored key strips emoji presentation selectors so that ❤ and ❤️ share
 *  one identity — correct for matching, wrong for display: it comes back as a
 *  monochrome text-style heart when what was dismissed was a red one. VS16 goes
 *  back on for display only, and only on characters outside the ASCII range, so
 *  punctuation keys like "!" are untouched. */
function displayKey(key: string): string {
  return [...key]
    .map((c) => (c.codePointAt(0)! > 0x2000 ? `${c}\uFE0F` : c))
    .join("");
}

/** Dismissing, with a reason and a choice of how far it reaches.
 *
 *  A scoped dismissal becomes a standing rule. It DISMISSES what it covers
 *  rather than hiding it — dismissed findings stay counted, reachable behind
 *  "Show dismissed", and carry the reason — because a conversation that is fine
 *  today may not be next month, and that is the case this app exists to catch.
 *
 *  The rungs run narrow to broad, and the narrow ones only appear when they
 *  would mean something: a content rule needs a `contentKey` (short enough to
 *  recur — see `content_key` in the core), and the sender rung needs a sender.
 *  Offering a rule keyed on a 200-word message would be a lie, since no second
 *  message could ever match it, and a dialog that offers rules covering nothing
 *  teaches people to dismiss dialogs.
 *
 *  Nothing here is the primary button. The least destructive action and the
 *  convenient one look alike on purpose: a suppression is a deliberate blind
 *  spot, and the UI should not lean on anyone to create one. "Recommended"
 *  marks the narrowest useful rule, which is guidance, not a nudge. */
function DismissPopover({
  finding: f,
  onDismiss,
  onRule,
}: {
  finding: ContentFinding;
  onDismiss: (dismissed: boolean, reason?: string) => void;
  onRule: (
    scope: SuppressionScope,
    value: string,
    category: string,
    sender: string | null,
    reason?: string,
  ) => void;
}) {
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState("");
  const trimmed = reason.trim() || undefined;
  const resolveContact = useContactResolver();
  // "me" is the device owner, not a handle to look up.
  const senderLabel =
    f.sender === "me"
      ? "you"
      : (resolveContact(f.sender ?? "")?.name ?? f.sender);
  const close = () => {
    setOpen(false);
    setReason("");
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 text-xs text-muted-foreground"
            >
              <EyeOff className="size-3.5" /> Dismiss
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent>
          Dismiss as a false positive — optionally as a standing rule, from
          just this sender upwards
        </TooltipContent>
      </Tooltip>
      <PopoverContent align="end" className="w-80 space-y-3">
        <div className="space-y-1.5">
          <Label htmlFor={`why-${f.id}`} className="text-xs">
            Why is this wrong? <span className="text-muted-foreground">(optional)</span>
          </Label>
          <Input
            id={`why-${f.id}`}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            placeholder="e.g. song lyrics, work banter"
            className="text-sm"
          />
        </div>
        <div className="space-y-1">
          <p className="text-xs text-muted-foreground">Apply to</p>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                className="w-full justify-start"
                onClick={() => {
                  onDismiss(true, trimmed);
                  close();
                }}
              >
                Just this finding
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              Dismiss this one. Nothing is remembered for next time.
            </TooltipContent>
          </Tooltip>
          {f.contentKey && f.sender && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full justify-start"
                  onClick={() => {
                    onRule(
                      "content+sender",
                      f.contentKey!,
                      f.category,
                      f.sender,
                      trimmed,
                    );
                    close();
                  }}
                >
                  <span className="truncate">
                    “{displayKey(f.contentKey)}” from {senderLabel}
                  </span>
                  <span className="ml-auto shrink-0 text-3xs text-muted-foreground">
                    Recommended
                  </span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                The narrowest rule that stops this repeating. The same thing
                from anyone else still gets flagged.
              </TooltipContent>
            </Tooltip>
          )}
          {f.contentKey && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full justify-start"
                  onClick={() => {
                    onRule(
                      "content+any",
                      f.contentKey!,
                      f.category,
                      null,
                      trimmed,
                    );
                    close();
                  }}
                >
                  <span className="truncate">“{displayKey(f.contentKey)}” from anyone</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                Covers this exact thing whoever sends it, in every conversation
              </TooltipContent>
            </Tooltip>
          )}
          {f.threadIdentifier && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full justify-start"
                  onClick={() => {
                    onRule(
                      "thread",
                      f.threadIdentifier!,
                      f.category,
                      null,
                      trimmed,
                    );
                    close();
                  }}
                >
                  <span className="truncate">
                    All “{CATEGORY_LABEL[f.category]}” in this conversation
                  </span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                Every {CATEGORY_LABEL[f.category].toLowerCase()} finding in this
                conversation, whatever it says
              </TooltipContent>
            </Tooltip>
          )}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                className="w-full justify-start"
                onClick={() => {
                  onRule("category", f.category, f.category, null, trimmed);
                  close();
                }}
              >
                <span className="truncate">
                  Everything in “{CATEGORY_LABEL[f.category]}”
                </span>
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              The broadest rule — every conversation, every sender
            </TooltipContent>
          </Tooltip>
        </div>
        {/* Said plainly, because a rule that quietly swallowed future findings
            would be the dangerous version of this feature. */}
        <p className="text-3xs leading-relaxed text-muted-foreground">
          A rule also applies to future scans, and only to the category you
          picked. A rule about one person covers only them — the same message
          from anyone else still gets flagged. Nothing is hidden: findings a
          rule covers are dismissed, still counted, and shown under “Show
          dismissed”. The most serious findings are never dismissed by a rule.
        </p>
      </PopoverContent>
    </Popover>
  );
}

/** The heading a finding sits under in grouped mode: its conversation, or
 *  "Notes" for note findings — which SQL sorts to the end. */
function groupNameOf(f: ContentFinding): string {
  return f.sourceKind === "note"
    ? "Notes"
    : (f.threadIdentifier ?? "Conversation");
}

function FindingsList({
  scan,
  counts,
  showDismissed,
  setShowDismissed,
  onDismiss,
  onSeen,
  onRule,
  onOpenReport,
}: {
  scan: SafetyScanHistoryItem;
  /** Unfiltered totals for the pills; the rows themselves are paged (#65). */
  counts: ContentFindingCounts | undefined;
  showDismissed: boolean;
  setShowDismissed: (v: boolean) => void;
  onDismiss: (f: ContentFinding, dismissed: boolean, reason?: string) => void;
  onSeen: (f: ContentFinding) => void;
  onRule: (
    scope: SuppressionScope,
    value: string,
    category: string,
    sender: string | null,
    reason?: string,
  ) => void;
  onOpenReport: () => void;
}) {
  const [severity, setSeverity] = useState("all");
  const [sort, setSort] = useState<SortState>({ by: "severity", desc: true });
  const [grouped, setGrouped] = useState(false);

  // The panel used to receive every finding and derive the visible list here:
  // ~3 MB of JSON at 8000 findings, re-sent and re-derived on every
  // invalidation. Filtering, ordering and grouping happen in SQLite now, and the
  // rows arrive a page at a time (#65).
  // Returning from a conversation opened via a finding (#224). The panel is
  // virtualized, so the finding has to be resolved to a row INDEX under whatever
  // filters are active now — which may differ from when the user left.
  const returnTo = useSearch({ strict: false }) as { finding?: number };
  const page = useMemo(
    () => ({
      severity:
        severity === "all" ? undefined : (Number(severity) as 1 | 2 | 3),
      includeDismissed: showDismissed,
      sortBy: sort.by === "date" ? ("date" as const) : ("severity" as const),
      desc: sort.desc,
      groupByThread: grouped,
    }),
    [severity, showDismissed, sort, grouped],
  );

  // Resolve the finding to a row index. Null means the current filter excludes
  // it — which must be said rather than silently scrolling nowhere.
  const { data: returnRank, isFetched: rankFetched } = useQuery({
    queryKey: ["contentFindingRank", scan?.id ?? null, page, returnTo.finding],
    queryFn: () => client.contentFindingRank(scan?.id ?? null, page, returnTo.finding!),
    enabled: returnTo.finding != null,
  });

  // How many rows the CURRENT filter matches — the virtualizer's count, from the
  // same predicate the page query uses, so the list can't run out early or leave
  // a gap at the end.
  const matching = useQuery({
    queryKey: ["safetyScan", "findings", "count", scan.id, page],
    queryFn: () =>
      client.countContentFindings(scan.id, {
        severity: page.severity,
        includeDismissed: page.includeDismissed,
      }),
  });
  const total = matching.data?.matching ?? 0;
  const dismissedCount = counts?.dismissed ?? 0;
  // Disabled, not hidden. The rail is master–detail, so controls that vanish and
  // reappear per selection reflow the row under the pointer — and a control that
  // changes between states is worse than one that is merely wrong (#171).
  const noFindings = (counts?.live ?? 0) === 0 && dismissedCount === 0;

  // The same charts the report prints, over the panel's CURRENT filter — the
  // aggregates share the page query's scope predicate, so narrowing to Serious
  // narrows the bars and the rows together (#66). Note `excludeStale` is false
  // here and true in the report: the panel keeps stale findings, so its charts
  // must too, or a bar would count rows the list below does not show.
  const [showCharts, setShowCharts] = usePersistedState(
    "safety-scan:show-analysis",
    false,
  );
  const analytics = useQuery({
    queryKey: ["safetyScan", "analytics", scan.id, page.severity, page.includeDismissed],
    queryFn: () =>
      client.contentFindingAnalytics(scan.id, {
        severity: page.severity,
        includeDismissed: page.includeDismissed,
      }),
    enabled: showCharts,
  });
  const threadLabel = useThreadLabel();

  // No early return for an empty scan. The panel is where the report lives, and
  // a scan that found nothing has the report most worth reading — "nothing was
  // found" is a result (#171). The controls that need findings go inert instead.
  return (
    <Card className="flex min-h-0 flex-1 flex-col">
      <CardHeader className="shrink-0">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <CardTitle className="flex items-center gap-2">
              <MessageSquareWarning className="size-4" /> Findings
              <Badge variant="secondary">{total}</Badge>
            </CardTitle>
            <CardDescription>
              What the scan of {scanTitle(scan)} flagged. Dismiss anything you
              judge a false positive — dismissals persist across re-scans.
            </CardDescription>
          </div>
          <div className="flex items-center gap-2">
            <FilterControl
              disabled={noFindings}
              groups={[
                badgeGroup({
                  key: "severity",
                  label: "Severity",
                  description: "Show only findings of one severity",
                  options: [
                    { value: "all", label: "All", count: counts?.live ?? 0 },
                    { value: "3", label: "Serious", count: counts?.serious ?? 0 },
                    { value: "2", label: "Harmful", count: counts?.harmful ?? 0 },
                    { value: "1", label: "Concerning", count: counts?.concerning ?? 0 },
                  ],
                  value: severity,
                  onChange: setSeverity,
                }),
              ]}
            />
            <SortControl
              disabled={noFindings}
              fields={[
                { value: "severity", label: "Severity" },
                { value: "date", label: "Date" },
              ]}
              value={sort}
              onChange={setSort}
            />
            {/* Always enabled, even with nothing found: "nothing was found" is
                a result, and it used to be the one thing the empty state told
                you to go and read. */}
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  className="gap-1.5"
                  onClick={onOpenReport}
                >
                  <FileText className="size-4" /> Report
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                The scan written up as a printable document
              </TooltipContent>
            </Tooltip>
            <SuppressionChip />
            {/* Dismissed findings: an island in the toolbar row rather than a
                switch on a row of its own, which cost a whole line of vertical
                space in a panel whose job is to show as many findings as fit.
                Only shown when there ARE dismissed findings — a toggle for an
                empty set is a control that can do nothing. */}
            {dismissedCount > 0 && (
              <ToggleGroup
                type="single"
                variant="outline"
                size="island"
                value={showDismissed ? "show" : ""}
                onValueChange={(v) => setShowDismissed(v === "show")}
              >
                <Tooltip>
                  <TooltipTrigger asChild>
                    <ToggleGroupItem value="show" aria-label="Show dismissed findings">
                      <EyeOff className="size-4" />
                    </ToggleGroupItem>
                  </TooltipTrigger>
                  <TooltipContent>
                    {showDismissed
                      ? `Hide the ${dismissedCount} dismissed finding${dismissedCount === 1 ? "" : "s"}`
                      : `Show the ${dismissedCount} dismissed finding${dismissedCount === 1 ? "" : "s"} as well`}
                  </TooltipContent>
                </Tooltip>
              </ToggleGroup>
            )}
            {/* Its own island rather than an item in the view-mode group: this
                reveals a section, it doesn't change how rows are listed. Same
                ToggleGroup shell so its height matches the islands beside it. */}
            <ToggleGroup
              type="single"
              variant="outline"
              size="island"
              value={showCharts ? "charts" : ""}
              onValueChange={(v) => setShowCharts(v === "charts")}
            >
              <Tooltip>
                <TooltipTrigger asChild>
                  <ToggleGroupItem
                    value="charts"
                    aria-label="Show analysis"
                    disabled={noFindings}
                  >
                    <ChartColumn className="size-4" />
                  </ToggleGroupItem>
                </TooltipTrigger>
                <TooltipContent>
                  Charts of when, what and where — counted over every finding the
                  current filter matches
                </TooltipContent>
              </Tooltip>
            </ToggleGroup>
            <ToggleGroup
              type="single"
              variant="outline"
              size="island"
              value={grouped ? "grouped" : "flat"}
              onValueChange={(v) => v && setGrouped(v === "grouped")}
              disabled={noFindings}
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
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col space-y-2">
        {showCharts && analytics.data && (
          // shrink-0 so the charts keep their height and the list takes what is
          // left; the list is the scrolling part, and two nested scrollbars
          // would be worse than a shorter list.
          <div className="shrink-0 border-b pb-3">
            <FindingCharts
              variant="panel"
              analytics={analytics.data}
              categoryLabel={(slug) =>
                CATEGORY_LABEL[slug as ContentCategory] ?? slug
              }
              conversationLabel={threadLabel}
            />
          </div>
        )}
        {/* Virtualized (#61): only on-screen rows are mounted, so 8000+ findings
            cost the same as 20. Bounded height because this list lives inside a
            card on a scrolling page — VirtualList needs a scroll container of
            its own to size against. */}
        {/* Fills the card, which fills the grid row, which fills the window
            (#79) — no fixed fraction, so a tall window shows more rows. */}
        <div className="flex min-h-0 flex-1 flex-col">
          {/* The filter may have changed since the user left, so the finding
              they came back for can be genuinely absent from this list. Saying
              so beats scrolling nowhere and looking broken. */}
          {returnTo.finding != null && rankFetched && returnRank == null && (
            <p className="border-b px-3 py-1.5 text-xs text-muted-foreground">
              The finding you came from isn't in the current filter.
            </p>
          )}
          <LazyVirtualList
            count={total}
            estimateSize={72}
            scrollToRow={returnRank ?? null}
            windowKey={(p) => [
              "safetyScan",
              "findings",
              "page",
              scan.id,
              page,
              p,
            ]}
            fetchWindow={(offset, limit) =>
              client.listContentFindings(scan.id, { ...page, offset, limit })
            }
            renderPlaceholder={() => (
              <div className="pb-1.5">
                <div className="h-16 animate-pulse rounded-lg bg-muted/40" />
              </div>
            )}
            renderItem={(f, _i, prev) => (
              <div className="pb-1.5">
                {/* Grouped mode gets its headings from the ORDER, not from a
                    grouped copy of the whole list: SQL orders by conversation,
                    so a heading is simply "this row's thread differs from the
                    one above". LazyVirtualList fetches the page before the
                    visible range precisely so `prev` exists at a boundary. */}
                {grouped && groupNameOf(f) !== (prev ? groupNameOf(prev) : null) && (
                  <div className="flex items-center gap-2 pt-3 pb-1 text-xs font-medium text-muted-foreground">
                    {f.sourceKind === "note" ? (
                      <NotebookText className="size-3.5" />
                    ) : (
                      <MessagesSquare className="size-3.5" />
                    )}
                    {groupNameOf(f)}
                  </div>
                )}
                <FindingRow
                  finding={f}
                  onDismiss={(d, reason) => onDismiss(f, d, reason)}
                  onSeen={() => onSeen(f)}
                  onRule={onRule}
                />
              </div>
            )}
          />
        </div>
        {/* A failed query gives total === 0 exactly like a clean scan does, and
            "nothing was flagged" is the one sentence a safety tool must not say
            when it does not know. The error branch comes first. */}
        {matching.isError && (
          <p className="text-xs text-status-warn-text">
            These findings couldn't be loaded, so this is not a result — try
            again rather than reading the empty list as clean.
          </p>
        )}
        {total === 0 && !matching.isPending && !matching.isError && (
          <p className="text-xs text-muted-foreground">
            {noFindings
              ? "Nothing was flagged in this scan's scope. A clean scan is a review aid, not a guarantee — spot-check important conversations yourself."
              : "No findings match the current filter."}
          </p>
        )}
      </CardContent>
    </Card>
  );
}

/** The full detail for one finding — the same interaction Security's findings
 *  table has: compact row → everything in a sheet. */
