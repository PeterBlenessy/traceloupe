import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
  ShieldAlert, ShieldCheck, ShieldQuestion, RefreshCw, Loader2, AlertTriangle, Info, ExternalLink, Download, Link2, } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card, CardDescription, CardHeader, CardTitle, } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, } from "@/components/ui/sheet";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Separator } from "@/components/ui/separator";
import { NoBackupState, ErrorState, ListSkeleton } from "@/components/view";
import { useViewToolbar } from "@/components/toolbar-context";
import { formatListTime } from "@/lib/format";
import { client, type Finding, type ScanRun, type Severity } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ConsentDialogs } from "@/views/security-consent";

const SEVERITY_META: Record<
  Severity,
  { label: string; badge: string; icon: typeof ShieldAlert }
> = {
  critical: {
    label: "Critical",
    badge:
      "bg-destructive text-white dark:bg-destructive/70 border-transparent",
    icon: ShieldAlert,
  },
  warning: {
    label: "Warning",
    badge:
      "bg-amber-500/15 text-amber-700 dark:text-amber-400 border-amber-500/30",
    icon: AlertTriangle,
  },
  info: {
    label: "Info",
    badge: "bg-muted text-muted-foreground border-transparent",
    icon: Info,
  },
};

const MODULE_LABEL: Record<string, string> = {
  apps: "Installed apps",
  messages: "Messages",
  attachments: "Attachments",
  safari: "Safari",
  notes: "Notes",
  calendar: "Calendar",
  contacts: "Contacts",
  interactions: "Interactions",
  manifest: "Backup files",
};

function SeverityBadge({ severity }: { severity: Severity }) {
  const meta = SEVERITY_META[severity];
  return (
    <Badge className={cn("gap-1 font-medium", meta.badge)}>
      <meta.icon className="size-3" />
      {meta.label}
    </Badge>
  );
}

/** Where a finding's source artifact lives, as a deep link into another view. */
function findingLink(f: Finding): { to: string; label: string } | null {
  switch (f.refKind) {
    case "message":
      return { to: `/messages?message=${f.refId}`, label: "Open message" };
    case "safari_history":
    case "safari_bookmark":
      return { to: "/safari", label: "Open in Safari" };
    case "note":
      return { to: `/notes?id=${f.refId}`, label: "Open note" };
    case "calendar_event":
      return { to: "/calendar", label: "Open in Calendar" };
    case "contact":
      return { to: `/contacts?id=${f.refId}`, label: "Open contact" };
    case "app":
    case "manifest_domain":
      return { to: "/apps", label: "Open in Apps" };
    default:
      return null;
  }
}

export function SecurityView() {
  const qc = useQueryClient();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const enabled = active === true;

  const runs = useQuery({
    queryKey: ["scanRuns"],
    queryFn: () => client.listScanRuns(),
    enabled,
  });
  const info = useQuery({
    queryKey: ["indicatorInfo"],
    queryFn: () => client.getIndicatorInfo(),
  });

  const latestRun = runs.data?.[0] ?? null;
  const findings = useQuery({
    queryKey: ["findings", latestRun?.id],
    queryFn: () => client.listFindings(latestRun!.id),
    enabled: enabled && !!latestRun && latestRun.status === "done",
  });

  const [progress, setProgress] = useState<string | null>(null);
  useEffect(() => {
    const un = client.onScanProgress((p) =>
      setProgress(p.total ? `${p.module}` : p.module),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  const scan = useMutation({
    mutationFn: () => client.runSecurityScan("explicit"),
    onSuccess: () => {
      setProgress(null);
      qc.invalidateQueries({ queryKey: ["scanRuns"] });
      qc.invalidateQueries({ queryKey: ["findings"] });
      qc.invalidateQueries({ queryKey: ["indicatorInfo"] });
    },
  });

  const update = useMutation({
    mutationFn: () => client.updateIndicators(),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["indicatorInfo"] }),
  });

  const settings = useQuery({
    queryKey: ["detectionSettings"],
    queryFn: () => client.getDetectionSettings(),
  });
  const setCustomDir = useMutation({
    mutationFn: async (dir: string | null) => {
      const s = settings.data ?? (await client.getDetectionSettings());
      await client.setDetectionSettings({ ...s, customIndicatorDir: dir });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["detectionSettings"] });
      qc.invalidateQueries({ queryKey: ["indicatorInfo"] });
    },
  });

  const [selected, setSelected] = useState<Finding | null>(null);

  const totalIndicators = useMemo(
    () => info.data?.feeds.reduce((n, f) => n + f.count, 0) ?? 0,
    [info.data],
  );

  // Publish the title to the shared top toolbar (like every other view). The
  // scan actions live in the content — the toolbar has no actions slot and
  // they belong next to the indicator status they act on.
  useViewToolbar(
    useMemo(
      () => (enabled ? { title: "Security Check" } : null),
      [enabled],
    ),
  );

  if (!enabled) {
    return (
      <NoBackupState
        icon={ShieldQuestion}
        title="Run a Security Check"
        lead="Scans an imported iPhone backup for traces of known spyware and stalkerware, matching it against curated threat feeds — entirely on this Mac."
        features={[
          { label: "What it checks", detail: "Installed apps, configuration profiles, and network indicators against known-threat feeds." },
          { label: "Ranked results", detail: "Findings graded Critical, Warning, or Info, with what matched and where." },
          { label: "Fresh indicators", detail: "Update the threat feeds, or load your own STIX/YAML indicators." },
          { label: "Follow through", detail: "Export a report, open each finding in its source view, and see safety guidance." },
        ]}
        note="Nothing is uploaded, and the check never modifies the backup."
      />
    );
  }

  const running = scan.isPending;

  return (
    // A self-contained flex-col with `h-full`: the Outlet wrapper isn't a flex
    // column, so the scroll region needs its own bounded-height parent.
    <div className="flex h-full flex-col">
      <ConsentDialogs />
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <div className="mx-auto flex max-w-3xl flex-col gap-4">
          {/* What this is / disclaimer — always visible. */}
          <Alert>
            <Info className="size-4" />
            <AlertTitle>Detection, not a guarantee</AlertTitle>
            <AlertDescription>
              This checks your backup against public lists of known spyware and
              stalkerware indicators (domains, addresses, files, app IDs). A
              match does not by itself mean your device is compromised — for
              example, simply visiting a monitoring vendor's website can trigger
              one. A clean result does not prove a device is safe. This is not a
              substitute for expert help.
            </AlertDescription>
          </Alert>

          {/* Indicator freshness + the scan actions (which act on it). */}
          <div className="flex items-center justify-between gap-3 rounded-lg border px-4 py-2.5 text-sm">
            <div className="min-w-0 text-muted-foreground">
              {info.data ? (
                <>
                  <span className="font-medium text-foreground">
                    {totalIndicators.toLocaleString()}
                  </span>{" "}
                  indicators from {info.data.feeds.length} feeds · updated{" "}
                  {info.data.generatedAt ? info.data.generatedAt.slice(0, 10) : "—"}
                </>
              ) : (
                "Loading indicator feeds…"
              )}
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => update.mutate()}
                disabled={update.isPending || running}
              >
                <RefreshCw
                  className={cn("size-4", update.isPending && "animate-spin")}
                />
                Update indicators
              </Button>
              <Button size="sm" onClick={() => scan.mutate()} disabled={running}>
                {running ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <ShieldAlert className="size-4" />
                )}
                {running ? "Scanning…" : "Run scan"}
              </Button>
            </div>
          </div>

          {/* Custom indicator folder (researcher mode). */}
          <div className="flex items-center justify-between gap-2 rounded-lg border px-4 py-2.5 text-sm">
            <div className="min-w-0 text-muted-foreground">
              Custom indicators:{" "}
              {settings.data?.customIndicatorDir ? (
                <span className="font-mono text-xs text-foreground">
                  {settings.data.customIndicatorDir}
                </span>
              ) : (
                <span>none — add a folder of .stix / .yaml files to scan</span>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {settings.data?.customIndicatorDir && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setCustomDir.mutate(null)}
                  disabled={setCustomDir.isPending}
                >
                  Clear
                </Button>
              )}
              <Button
                variant="outline"
                size="sm"
                disabled={setCustomDir.isPending || running}
                onClick={async () => {
                  const dir = await client.pickFolder("Choose a custom indicator folder");
                  if (dir) setCustomDir.mutate(dir);
                }}
              >
                Choose folder…
              </Button>
            </div>
          </div>

          {running && (
            <div className="flex items-center gap-2 rounded-lg border bg-muted/40 px-4 py-3 text-sm">
              <Loader2 className="size-4 animate-spin text-muted-foreground" />
              <span className="text-muted-foreground">
                {progress ? `Scanning: ${progress}` : "Starting scan…"}
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto"
                onClick={() => client.cancelScan()}
              >
                Cancel
              </Button>
            </div>
          )}

          {!running && runs.isPending && <ListSkeleton rows={4} />}
          {runs.error && <ErrorState error={runs.error} />}

          {!running && runs.data && !latestRun && (
            <Card>
              <CardHeader>
                <CardTitle>No scan yet</CardTitle>
                <CardDescription>
                  Run a scan to check this backup against the latest indicators.
                </CardDescription>
              </CardHeader>
            </Card>
          )}

          {!running && latestRun && (
            <ResultSummary
              run={latestRun}
              findings={findings.data ?? []}
              loadingFindings={findings.isPending}
              onSelect={setSelected}
            />
          )}
        </div>
      </div>

      <FindingDetail finding={selected} onClose={() => setSelected(null)} />
    </div>
  );
}

function ResultSummary({
  run,
  findings,
  loadingFindings,
  onSelect,
}: {
  run: ScanRun;
  findings: Finding[];
  loadingFindings: boolean;
  onSelect: (f: Finding) => void;
}) {
  const total = run.critical + run.warning + run.info;
  const newCount = findings.filter((f) => f.isNew).length;

  if (total === 0) {
    return (
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <ShieldCheck className="size-5 text-emerald-600 dark:text-emerald-400" />
            <CardTitle>No known indicators matched</CardTitle>
          </div>
          <CardDescription>
            Scanned {formatListTime(run.startedAt)} against{" "}
            {run.indicatorCount?.toLocaleString() ?? "?"} indicators. A clean
            result means no traces of spyware <em>known to these feeds</em> were
            found — it does not guarantee the device is uncompromised.
          </CardDescription>
        </CardHeader>
      </Card>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        {run.critical > 0 && (
          <Badge className={cn("gap-1", SEVERITY_META.critical.badge)}>
            <ShieldAlert className="size-3" />
            {run.critical} critical
          </Badge>
        )}
        {run.warning > 0 && (
          <Badge className={cn("gap-1", SEVERITY_META.warning.badge)}>
            <AlertTriangle className="size-3" />
            {run.warning} warning
          </Badge>
        )}
        {run.info > 0 && (
          <Badge className={cn("gap-1", SEVERITY_META.info.badge)}>
            <Info className="size-3" />
            {run.info} info
          </Badge>
        )}
        <span className="text-xs text-muted-foreground">
          scanned {formatListTime(run.startedAt)}
        </span>
        {newCount > 0 && (
          <span className="text-xs font-medium text-sky-600 dark:text-sky-400">
            {newCount} new since last scan
          </span>
        )}
        <Button
          variant="ghost"
          size="sm"
          className="ml-auto"
          onClick={() => client.exportScanReport(run.id)}
        >
          <Download className="size-4" />
          Export CSV
        </Button>
      </div>

      {loadingFindings ? (
        <ListSkeleton rows={4} />
      ) : (
        <div className="overflow-hidden rounded-lg border">
          <table className="w-full text-sm">
            <thead className="bg-muted/50 text-xs text-muted-foreground">
              <tr>
                <th className="px-3 py-2 text-left font-medium">Severity</th>
                <th className="px-3 py-2 text-left font-medium">Threat</th>
                <th className="px-3 py-2 text-left font-medium">Matched</th>
                <th className="px-3 py-2 text-left font-medium">Where</th>
              </tr>
            </thead>
            <tbody>
              {findings.map((f) => (
                <tr
                  key={f.id}
                  className="cursor-pointer border-t hover:bg-accent/50"
                  onClick={() => onSelect(f)}
                >
                  <td className="px-3 py-2">
                    <SeverityBadge severity={f.severity} />
                  </td>
                  <td className="px-3 py-2 font-medium">
                    <span className="inline-flex items-center gap-1.5">
                      {f.malware}
                      {f.isNew && (
                        <Badge
                          variant="outline"
                          className="border-sky-500/40 px-1.5 py-0 text-[10px] font-semibold text-sky-600 dark:text-sky-400"
                        >
                          NEW
                        </Badge>
                      )}
                    </span>
                  </td>
                  <td className="max-w-[16rem] truncate px-3 py-2 font-mono text-xs text-muted-foreground">
                    {f.matchedValue}
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">
                    {MODULE_LABEL[f.module] ?? f.module}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <WhatNow />
    </div>
  );
}

function WhatNow() {
  return (
    <Alert>
      <AlertTriangle className="size-4" />
      <AlertTitle>If you're worried about your safety</AlertTitle>
      <AlertDescription>
        <p>
          If someone may be monitoring this device, removing an app or changing
          passwords can alert the person who installed it. Consider your
          situation before acting, and reach out for support:
        </p>
        <ul className="mt-1 list-inside list-disc space-y-0.5">
          <li>Access Now Digital Security Helpline — help@accessnow.org</li>
          <li>Amnesty International Security Lab</li>
          <li>Coalition Against Stalkerware — stopstalkerware.org</li>
        </ul>
      </AlertDescription>
    </Alert>
  );
}

/**
 * The opt-in de-shortener (ADR 0001 exception). Resolving a shortened link
 * contacts a remote host with a URL from the backup, so it is a deliberate,
 * per-link, user-approved action: every use prompts unless the user has ticked
 * "don't ask again" for THIS backup. Nothing is contacted until the user clicks
 * Reveal and approves.
 */
function ShortLinkExpander({ text }: { text: string }) {
  const qc = useQueryClient();
  const links = useQuery({
    queryKey: ["shortenerUrls", text],
    queryFn: () => client.findShortenerUrls(text),
  });
  const autoApprove = useQuery({
    queryKey: ["deshortenAutoApprove"],
    queryFn: () => client.deshortenAutoApproveGet(),
  });

  const [results, setResults] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [pending, setPending] = useState<string | null>(null);
  const [dontAsk, setDontAsk] = useState(false);

  const expand = useMutation({
    mutationFn: (url: string) => client.expandShortUrl(url),
    onSuccess: (target, url) => setResults((r) => ({ ...r, [url]: target })),
    onError: (e, url) =>
      setErrors((x) => ({ ...x, [url]: (e as Error).message })),
  });

  if (!links.data || links.data.length === 0) return null;

  function onReveal(url: string) {
    if (autoApprove.data) expand.mutate(url);
    else {
      setDontAsk(false);
      setPending(url);
    }
  }
  async function onApprove() {
    if (!pending) return;
    if (dontAsk) {
      await client.deshortenAutoApproveSet(true);
      qc.invalidateQueries({ queryKey: ["deshortenAutoApprove"] });
    }
    const url = pending;
    setPending(null);
    expand.mutate(url);
  }

  return (
    <div className="flex flex-col gap-2">
      <span className="text-xs font-medium text-muted-foreground">
        Shortened links
      </span>
      {links.data.map((url) => (
        <div key={url} className="flex flex-col gap-1 rounded-md border p-2">
          <span className="break-all font-mono text-xs">{url}</span>
          {results[url] ? (
            <span className="break-all text-xs">
              → <span className="font-mono text-foreground">{results[url]}</span>
            </span>
          ) : errors[url] ? (
            <span className="text-xs text-destructive">
              Couldn’t resolve: {errors[url]}
            </span>
          ) : (
            <Button
              variant="outline"
              size="sm"
              className="w-fit"
              disabled={expand.isPending}
              onClick={() => onReveal(url)}
            >
              <Link2 className="size-4" />
              Reveal destination
            </Button>
          )}
        </div>
      ))}

      <Dialog open={!!pending} onOpenChange={(o) => !o && setPending(null)}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Reveal this shortened link?</DialogTitle>
            <DialogDescription>
              TraceLoupe will contact the link’s shortener to find where it
              points. Only the shortener is contacted, never the final
              destination.
            </DialogDescription>
          </DialogHeader>
          <Alert className="[&>svg]:text-amber-500">
            <AlertTriangle className="size-4" />
            <AlertTitle>This sends data from your backup</AlertTitle>
            <AlertDescription>
              This is the one time information from your backup leaves your Mac.
              If the link was sent by someone monitoring this device, resolving
              it can signal that the device is being examined.
            </AlertDescription>
          </Alert>
          <label className="flex items-center gap-2 text-sm">
            <Switch checked={dontAsk} onCheckedChange={setDontAsk} />
            Don’t ask again for this backup
          </label>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPending(null)}>
              Cancel
            </Button>
            <Button onClick={onApprove}>Reveal link</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function FindingDetail({
  finding,
  onClose,
}: {
  finding: Finding | null;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const link = finding ? findingLink(finding) : null;
  return (
    <Sheet open={!!finding} onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full gap-0 sm:max-w-md">
        {finding && (
          <>
            <SheetHeader>
              <div className="flex items-center gap-2">
                <SeverityBadge severity={finding.severity} />
                <SheetTitle>{finding.malware}</SheetTitle>
              </div>
              <SheetDescription>
                A {finding.kind.replace(/_/g, " ")} indicator matched in{" "}
                {MODULE_LABEL[finding.module] ?? finding.module}.
              </SheetDescription>
            </SheetHeader>
            <div className="flex flex-col gap-3 px-4 pb-4 text-sm">
              <Field label="Matched value">
                <span className="break-all font-mono text-xs">
                  {finding.matchedValue}
                </span>
              </Field>
              {finding.context && (
                <Field label="Context">
                  <span className="break-all text-muted-foreground">
                    {finding.context}
                  </span>
                </Field>
              )}
              {finding.eventTime && (
                <Field label="When">{formatListTime(finding.eventTime)}</Field>
              )}

              <ShortLinkExpander
                text={`${finding.matchedValue} ${finding.context ?? ""}`}
              />

              <Separator />
              <p className="text-xs text-muted-foreground">
                A match is evidence to review, not proof of compromise. False
                positives are common for domains and links.
              </p>
              {link && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    navigate({ to: link.to });
                    onClose();
                  }}
                >
                  <ExternalLink className="size-4" />
                  {link.label}
                </Button>
              )}
            </div>
          </>
        )}
      </SheetContent>
    </Sheet>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      <div>{children}</div>
    </div>
  );
}
