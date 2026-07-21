import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { usePersistedState } from "@/lib/use-persisted-state";
import {
  Ban, ExternalLink, EyeOff, HeartPulse, Loader2, MessageSquareWarning, NotebookText, Play, RotateCcw, ShieldUser, ShieldQuestion, } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card, CardContent, CardDescription, CardHeader, CardTitle, } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { NoBackupState, ViewHeader, ErrorState, ListSkeleton } from "@/components/view";
import { TimeFilterBar, useTimePresets } from "@/components/time-filter";
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
  // Same preset chips + custom range as Photos/Messages/etc., over the message
  // and note timestamps. `TimeFilterBar` emits a half-open [lo, hi); the scan
  // backend's range end is inclusive, so hi maps to `end = hi - 1`.
  const { presets } = useTimePresets();
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

  // Gate on an open backup, like every content view — there is nothing to scan
  // without one.
  if (active === false) {
    return (
      <NoBackupState
        icon={ShieldUser}
        title="Open a backup to run a Safety Scan"
        lead="A local AI model reads messages and notes and flags possible harmful content — a prompt to review conversations yourself, not a verdict."
        features={[
          { label: "Categories", detail: "Threats, harassment, grooming, self-harm, coercive control, scams, and more." },
          { label: "Time range", detail: "Scan all history, the last 12 months, or a specific year." },
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
  const effectiveModel = ms.models.find((m) => m.id === effectiveModelId);

  return (
    <div className="flex h-full flex-col">
      <ViewHeader
        icon={<ShieldUser className="size-4 text-muted-foreground" />}
        title="Safety Scan"
      >
        <span className="text-xs text-muted-foreground">
          Local analysis of messages and notes for harmful content
        </span>
      </ViewHeader>
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
                been validated. Verdicts come from a local AI model and can be
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
        ) : running ? (
          <RunningCard scanEvent={scan} onCancel={cancelScan} />
        ) : (
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Play className="size-4" /> Run a scan
              </CardTitle>
              <CardDescription>
                The scan runs entirely on this Mac: a local model reads your
                messages and notes in small windows and flags possible threats,
                harassment, grooming, self-harm, coercive control, scams and
                more. Verdicts are probabilistic — treat each flag as something
                to review, not a fact. Already-scanned content is skipped
                automatically.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="space-y-1.5">
                <Label className="text-xs text-muted-foreground">
                  Time range
                </Label>
                <TimeFilterBar
                  presets={presets}
                  value={range}
                  onChange={setRange}
                />
              </div>
              <div className="flex flex-wrap items-center gap-3">
                <Button
                  onClick={() =>
                    void startScan({
                      modelId: effectiveModelId,
                      rangeStart: range.lo,
                      // TimeFilterBar's hi is exclusive; the scan range end is
                      // inclusive, so step back one second.
                      rangeEnd: range.hi != null ? range.hi - 1 : null,
                    })
                  }
                >
                  <Play className="size-4" /> Start Safety Scan
                </Button>
                <span className="text-xs text-muted-foreground">
                  Model: {effectiveModel?.displayName}
                  {effectiveModel &&
                    !effectiveModel.recommended &&
                    " (fallback)"}
                  {installedIds.length >= 2 &&
                    " · change in Settings → Safety"}
                </span>
              </div>
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
          <ShieldQuestion className="size-4" /> A local model is required
        </CardTitle>
        <CardDescription>
          Safety Scan analyzes your messages and notes with a local language
          model that runs entirely on this Mac. Download it once from{" "}
          <span className="font-medium text-foreground">Settings → Safety</span>{" "}
          (bottom-left), then come back here to run a scan.
        </CardDescription>
      </CardHeader>
    </Card>
  );
}

function RunningCard({
  scanEvent,
  onCancel,
}: {
  scanEvent: NonNullable<ReturnType<typeof useSafetyScan>["scan"]>;
  onCancel: () => void;
}) {
  const label =
    scanEvent.phase === "loading"
      ? "Starting the local model server…"
      : scanEvent.phase === "summarizing"
        ? "Writing the scan report…"
        : "Scanning…";
  const pct =
    scanEvent.phase === "classifying" && scanEvent.total > 0
      ? (scanEvent.done / scanEvent.total) * 100
      : null;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Loader2 className="size-4 animate-spin" /> {label}
        </CardTitle>
        {scanEvent.phase === "classifying" && (
          <CardDescription>
            {scanEvent.done}/{scanEvent.total} chunks ·{" "}
            {scanEvent.findings} finding{scanEvent.findings === 1 ? "" : "s"} so
            far — you can leave this page; the scan keeps running.
          </CardDescription>
        )}
      </CardHeader>
      <CardContent className="flex items-center gap-3">
        {pct !== null ? (
          <Progress className="flex-1" value={pct} />
        ) : (
          <Progress className="flex-1" />
        )}
        <Button variant="outline" size="sm" onClick={onCancel}>
          <Ban className="size-4" /> Stop
        </Button>
      </CardContent>
    </Card>
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
