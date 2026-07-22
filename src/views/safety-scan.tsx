import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { usePersistedState } from "@/lib/use-persisted-state";
import {
  Square, ExternalLink, EyeOff, HeartPulse, Loader2, MessageSquareWarning, NotebookText, Play, RotateCcw, ShieldUser, ShieldQuestion, } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card, CardContent, CardDescription, CardHeader, CardTitle, } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { NoBackupState, ErrorState, ListSkeleton } from "@/components/view";
import { useViewToolbar } from "@/components/toolbar-context";
import { makeYearPresets, useTimePresets } from "@/components/time-filter";
import { FilterControl } from "@/components/filter-control";
import { timeGroup } from "@/components/filter-groups";
import { useSafetyScan } from "@/components/safety-scan-provider";
import { formatListTime } from "@/lib/format";
import {
  client,
  type ContentCategory,
  type ContentFinding,
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
  const [showDismissed, setShowDismissed] = useState(false);
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
  // Per-window message counts, so empty periods are shown-but-disabled (not
  // hidden). Counts reflect messages — the bulk of scanned content.
  const { data: presetCounts } = useQuery({
    queryKey: ["messageRanges", now, presets.length],
    queryFn: () =>
      client.countMessageRanges(
        presets.map((p) => ({ lo: p.lo, hi: p.hi })),
        null,
      ),
    enabled: active === true,
  });
  const modelStatus = useQuery({
    queryKey: ["safetyScan", "modelStatus"],
    queryFn: () => client.getSafetyScanModelStatus(),
  });
  const findings = useQuery({
    queryKey: ["safetyScan", "findings"],
    queryFn: () => client.listContentFindings(),
  });
  const report = useQuery({
    queryKey: ["safetyScan", "report"],
    queryFn: () => client.getSafetyScanReport(),
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
        title="Open a backup to run a Safety Scan"
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
                    Time range
                  </Label>
                  <FilterControl
                    align="right"
                    groups={[
                      timeGroup({
                        description: "Which messages and notes to scan, by date",
                        presets,
                        counts: presetCounts,
                        value: range,
                        onChange: setRange,
                      }),
                    ]}
                  />
                </div>
                {running ? (
                  <Button variant="outline" onClick={cancelScan}>
                    <Square className="size-4" /> Stop
                  </Button>
                ) : (
                  <Button
                    onClick={() =>
                      void startScan({
                        modelId: effectiveModelId,
                        rangeStart: range.lo,
                        // timeGroup's hi is exclusive; the scan range end is
                        // inclusive, so step back one second.
                        rangeEnd: range.hi != null ? range.hi - 1 : null,
                      })
                    }
                  >
                    <Play className="size-4" /> Start Safety Scan
                  </Button>
                )}
              </div>
              {running && scan && <ScanProgress scanEvent={scan} />}
            </CardContent>
          </Card>
        )}

        {report.data?.report && (
          <Card>
            <CardHeader>
              <CardTitle>Scan report</CardTitle>
              {report.data.scan && (
                <CardDescription>
                  {report.data.scan.status === "completed"
                    ? "Completed"
                    : report.data.scan.status}{" "}
                  {report.data.scan.finishedAt
                    ? formatListTime(report.data.scan.finishedAt)
                    : ""}
                  {" · "}
                  {report.data.scan.chunksDone}/{report.data.scan.chunksTotal}{" "}
                  chunks
                </CardDescription>
              )}
            </CardHeader>
            <CardContent className="space-y-3">
              <p className="text-sm leading-relaxed">{report.data.report}</p>
              {report.data.threadSummaries.length > 0 && (
                <>
                  <Separator />
                  <div className="space-y-2">
                    {report.data.threadSummaries.map(([thread, text]) => (
                      <div key={thread} className="text-sm">
                        <span className="font-medium">{thread}: </span>
                        <span className="text-muted-foreground">{text}</span>
                      </div>
                    ))}
                  </div>
                </>
              )}
            </CardContent>
          </Card>
        )}

        <FindingsList
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
          <span className="font-medium text-foreground">Settings → Safety</span>{" "}
          (bottom-left), then come back here to run a scan.
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
          {scanEvent.done}/{scanEvent.total} chunks · {scanEvent.findings}{" "}
          finding{scanEvent.findings === 1 ? "" : "s"} so far — you can leave
          this page; the scan keeps running.
        </div>
      )}
    </div>
  );
}

function FindingsList({
  findings,
  showDismissed,
  setShowDismissed,
  onDismiss,
}: {
  findings: ContentFinding[];
  showDismissed: boolean;
  setShowDismissed: (v: boolean) => void;
  onDismiss: (f: ContentFinding, dismissed: boolean) => void;
}) {
  const navigate = useNavigate();
  const visible = useMemo(
    () => findings.filter((f) => showDismissed || !f.dismissed),
    [findings, showDismissed],
  );
  const dismissedCount = findings.filter((f) => f.dismissed).length;

  if (findings.length === 0) return null;
  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div>
            <CardTitle className="flex items-center gap-2">
              <MessageSquareWarning className="size-4" /> Findings
              <Badge variant="secondary">{visible.length}</Badge>
            </CardTitle>
            <CardDescription>
              Model verdicts, most severe first. Dismiss anything you judge a
              false positive — dismissals persist across re-scans.
            </CardDescription>
          </div>
          {dismissedCount > 0 && (
            <div className="flex items-center gap-2">
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
        </div>
      </CardHeader>
      <CardContent className="space-y-2">
        {visible.map((f) => {
          // severity is a u8 at the IPC seam, not really typed 1|2|3 — guard so
          // an out-of-range value can't blank the whole Findings card.
          const sev = SEVERITY_META[f.severity] ?? SEVERITY_META[1];
          return (
            <div
              key={`${f.fingerprint}:${f.category}`}
              className={cn(
                "flex flex-col gap-1 rounded-md border p-3",
                f.dismissed && "opacity-55",
              )}
            >
              <div className="flex flex-wrap items-center gap-2">
                <Badge className={sev.badge}>{sev.label}</Badge>
                <Badge variant="outline">{CATEGORY_LABEL[f.category]}</Badge>
                {f.sourceKind === "note" ? (
                  <span className="flex items-center gap-1 text-xs text-muted-foreground">
                    <NotebookText className="size-3" /> Note
                  </span>
                ) : (
                  <span className="text-xs text-muted-foreground">
                    {f.threadIdentifier ?? "Conversation"}
                  </span>
                )}
                {f.occurredAt && (
                  <span className="text-xs text-muted-foreground">
                    {formatListTime(f.occurredAt)}
                  </span>
                )}
                {f.stale && (
                  <Badge variant="outline" className="text-muted-foreground">
                    <HeartPulse className="size-3" /> outdated
                  </Badge>
                )}
              </div>
              <p className="text-sm">{f.rationale}</p>
              <div className="flex items-center gap-2">
                {f.sourceKind === "message" && f.threadId != null && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      navigate({
                        to: "/messages",
                        search: { thread: f.threadId! },
                      })
                    }
                  >
                    <ExternalLink className="size-4" /> Open conversation
                  </Button>
                )}
                {f.sourceKind === "note" && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => navigate({ to: "/notes" })}
                  >
                    <ExternalLink className="size-4" /> Open Notes
                  </Button>
                )}
                {f.dismissed ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => onDismiss(f, false)}
                  >
                    <RotateCcw className="size-4" /> Restore
                  </Button>
                ) : (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => onDismiss(f, true)}
                  >
                    <EyeOff className="size-4" /> Dismiss
                  </Button>
                )}
              </div>
            </div>
          );
        })}
      </CardContent>
    </Card>
  );
}
