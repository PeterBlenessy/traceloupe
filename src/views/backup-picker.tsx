import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { toast } from "sonner";
import { Check, ChevronRight, FolderOpen, Lock, LockOpen, LogOut, RotateCw, Settings, Smartphone, Trash2 } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { client, type BackupInfo, type ContentCategory } from "@/lib/ipc";
import { useImport } from "@/components/import-provider";
import {
  openPerfEnd,
  openPerfInFlight,
  openPerfPhase,
  openPerfStart,
} from "@/lib/open-perf";
import { modelName } from "@/lib/device-names";
import {
  DashboardTiles,
  DashboardTilesSkeleton,
  ScanTile,
} from "@/components/dashboard-tiles";
import { CATEGORY_LABEL, formatSources } from "@/views/safety-scan";
import { formatCount, formatDateTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { ArtifactTable } from "@/components/artifact-table";
import { useHostedArtifacts } from "@/lib/use-hosted-artifacts";
import { useEncryptedOnlyEmpty } from "@/lib/use-encrypted-only";
import { useBoundedList } from "@/lib/bounded-list";

export function BackupPicker() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const imp = useImport();
  const { choose } = useSearch({ strict: false }) as { choose?: true };
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: importedIds } = useQuery({
    queryKey: ["importedBackupIds"],
    queryFn: () => client.importedBackupIds(),
  });
  const imported = new Set(importedIds ?? []);
  // A folder the user picked (via the native panel), overriding the default
  // MobileSync scan. Selecting a folder grants access without Full Disk Access.
  const [root, setRoot] = useState<string | null>(null);
  const { data, isPending, error, refetch } = useQuery({
    queryKey: ["backups", root],
    queryFn: () => client.listBackups(root ?? undefined),
  });
  // One card per backup in ONE folder — a handful in practice. Declared rather
  // than assumed, so a folder full of backups shows up in dev instead of on a
  // user's machine (#67).
  useBoundedList(
    "backup-picker backups",
    data?.status === "ok" ? data.backups.length : 0,
    60,
  );

  // EVERY hook must run above this early return. `active` flips false→true the
  // moment a backup opens (handleOpen sets it optimistically), so the very next
  // render takes the DeviceHome branch — and if any hook lived below here, that
  // render would call fewer hooks than the previous one, which React aborts with
  // "Rendered fewer hooks than expected", crashing the view to the error
  // boundary. Keep every hook above this line.
  //
  // `/` is the app's one home. With a backup open it IS the Device view (full
  // device detail, densely laid out) — the separate /device route is gone.
  // `?choose` forces the picker back so the user can still switch backups.
  if (active === true && !choose) {
    return <DeviceHome onChooseOther={() => navigate({ to: "/", search: { choose: true } })} />;
  }

  // Opening an already-parsed backup is instant (just point at its cache).
  // A never-parsed one needs a first-time read: unencrypted starts straight
  // away, encrypted asks for a password first — both via the dialog.
  async function handleOpen(b: BackupInfo) {
    if (imported.has(b.id)) {
      try {
        // Time the whole open in devtools (#40) — filter by [open-perf].
        openPerfStart(b.deviceName ?? b.id);
        await client.openBackup(b.id);
        openPerfPhase("openBackup IPC (cache only — keys warm up in background)");
        // Mark active optimistically before invalidating (queries are
        // staleTime: Infinity), so the target view doesn't read a stale
        // `hasActiveBackup: false` and bounce back to the picker.
        qc.setQueryData(["hasActiveBackup"], true);
        // Drop any cached artifact data from a previously-open backup; with
        // staleTime: Infinity it would otherwise persist across backups.
        //
        // NOT awaited (#40): invalidateQueries' promise resolves only once every
        // active query has REFETCHED, so awaiting it held the user on the picker
        // through a full round of backend queries — seconds on a large backup —
        // before the view even changed. Firing it and navigating immediately is
        // just as correct (the invalidation is already registered, and the marks
        // are applied synchronously) and each view shows its own loading state
        // while its data lands. Matches what import-provider already does.
        void qc.invalidateQueries();
        openPerfPhase("invalidate queries (fired, not awaited)");
        // Land on `/` — now the Device view for the freshly opened backup.
        navigate({ to: "/" });
        openPerfPhase("navigate to landing");
      } catch (e) {
        toast.error("Couldn't open backup", {
          description: e instanceof Error ? e.message : String(e),
        });
      }
    } else {
      imp.open(b); // first-time read: the provider owns the import + its dialog
    }
  }
  // Delete an imported backup's caches + stored password (not the original), then
  // refresh which backups show as imported.
  async function handleForget(b: BackupInfo) {
    try {
      await client.forgetBackup(b.id);
      await qc.invalidateQueries({ queryKey: ["importedBackupIds"] });
      await qc.invalidateQueries({ queryKey: ["hasActiveBackup"] });
    } catch (e) {
      toast.error("Couldn't forget backup", {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  async function chooseFolder() {
    try {
      const picked = await client.pickBackupFolder();
      if (picked) {
        // Re-run discovery on the picked folder (setRoot changes the query key).
        if (picked === root) void refetch();
        else setRoot(picked);
      }
    } catch (e) {
      toast.error("Couldn't open that folder", {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  const chooseButton = (
    <Button variant="outline" onClick={chooseFolder}>
      <FolderOpen className="size-4" />
      Choose folder…
    </Button>
  );

  // The empty/blocked/not-found cards carry their own button, so only show the
  // header one while actually listing backups — avoids two side by side.
  const showHeaderButton = data?.status === "ok" && data.backups.length > 0;

  return (
    <div className="mx-auto max-w-2xl p-8">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Your iPhone backups</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Pick a backup to open. The first time, TraceLoupe reads it once; after
            that it opens instantly. Everything stays on this machine.
          </p>
        </div>
        {showHeaderButton && chooseButton}
      </div>
      {root && (
        <p className="mt-3 text-xs text-muted-foreground">
          Looking in <code className="select-text">{root}</code>
        </p>
      )}
      <div className="mt-6 flex flex-col gap-3">
        {isPending && (
          <>
            <Skeleton className="h-24 w-full" />
            <Skeleton className="h-24 w-full" />
          </>
        )}
        {error && (
          <Card>
            <CardHeader>
              <CardTitle>Something went wrong</CardTitle>
              <CardDescription>{String(error)}</CardDescription>
            </CardHeader>
          </Card>
        )}
        {data?.status === "permissionDenied" && (
          <FdaGuidance path={data.path} action={chooseButton} />
        )}
        {data?.status === "notFound" && (
          <Card>
            <CardHeader>
              <CardTitle>No backup folder found</CardTitle>
              <CardDescription>
                Nothing at <code className="select-text">{data.path}</code>. Create
                a backup with Finder, or choose a folder.
              </CardDescription>
            </CardHeader>
            <CardContent>{chooseButton}</CardContent>
          </Card>
        )}
        {data?.status === "ok" && data.backups.length === 0 && (
          <Card>
            <CardHeader>
              <CardTitle>No backups here</CardTitle>
              <CardDescription>
                {root
                  ? "That folder has no backups in it. Choose a different one."
                  : "No backups in the default folder yet. Create one with Finder, or choose a folder."}
              </CardDescription>
            </CardHeader>
            <CardContent>{chooseButton}</CardContent>
          </Card>
        )}
        {data?.status === "ok" &&
          data.backups.map((b) => (
            <BackupCard
              key={b.id}
              backup={b}
              imported={imported.has(b.id)}
              onSelect={() => handleOpen(b)}
              onReimport={() => imp.open(b)}
              onForget={() => handleForget(b)}
            />
          ))}
      </div>

      <AppFeatures />
    </div>
  );
}

/** The metrics for the open backup: one tile per kind of data it yielded, plus
 *  the two scans.
 *
 *  Loaded AFTER the device header paints. #40 measures "the backup is open" as
 *  the moment this view has its data, and putting a dozen aggregate queries in
 *  front of that would spend the very number #40 exists to protect. */
function HomeDashboard() {
  const navigate = useNavigate();
  const { data: metrics, isPending } = useQuery({
    queryKey: ["moduleMetrics"],
    queryFn: () => client.moduleMetrics(),
  });
  const { data: securityRuns } = useQuery({
    queryKey: ["scanRuns"],
    queryFn: () => client.listScanRuns(),
  });
  const { data: safetyScans } = useQuery({
    queryKey: ["safetyScan", "history"],
    queryFn: () => client.listSafetyScans(),
  });

  const security = (securityRuns ?? []).find((r) => r.status === "done");
  const safety = (safetyScans ?? []).find((s) => s.status === "completed");
  // Live totals across everything scanned, with the LATEST run's date and that
  // run's coverage. Findings are scoped rather than owned by a run, so pairing
  // one run's date with another run's findings would describe two scans in one
  // tile without saying so — the defect the report's totals row had.
  const { data: safetyCounts } = useQuery({
    queryKey: ["safetyScan", "findingCounts", null],
    queryFn: () => client.countContentFindings(undefined),
    enabled: safety != null,
  });
  const { data: safetyAnalytics } = useQuery({
    queryKey: ["safetyScan", "analytics", "home"],
    queryFn: () => client.contentFindingAnalytics(undefined, { excludeStale: true }),
    enabled: safety != null,
  });
  const { data: securityFindings } = useQuery({
    queryKey: ["findings", security?.id ?? null],
    queryFn: () => client.listFindings(security!.id),
    enabled: security != null,
  });

  const topCategories = (safetyAnalytics?.byCategory ?? [])
    .slice(0, 3)
    .map((b) => CATEGORY_LABEL[b.key as ContentCategory] ?? b.key);
  const newSinceLastRun = (securityFindings ?? []).filter((f) => f.isNew).length;

  return (
    <section className="mt-8 space-y-3">
      <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
        In this backup
      </h2>
      {isPending ? (
        <DashboardTilesSkeleton />
      ) : (
        <DashboardTiles metrics={metrics ?? []}>
          <ScanTile
            route="/security"
            label="Security"
            status={
              security
                ? formatRelative(security.finishedAt ?? security.startedAt)
                : "never run"
            }
            bands={
              security
                ? [
                    { n: security.critical, color: "var(--status-danger)", label: "critical" },
                    { n: security.warning, color: "var(--status-warning)", label: "warning" },
                    { n: security.info, color: "var(--muted-foreground)", label: "info" },
                  ]
                : undefined
            }
            lines={
              security
                ? [
                    security.critical + security.warning + security.info === 0
                      ? "nothing found"
                      : `${security.critical} critical · ${security.warning} warning · ${security.info} info`,
                    newSinceLastRun > 0
                      ? `${newSinceLastRun} new since the run before`
                      : "nothing new since the run before",
                    // A clean scan against stale feeds is a weaker claim than a
                    // clean scan against fresh ones, and nothing else says so.
                    security.feedsGeneratedAt != null
                      ? `feeds were ${formatRelative(security.feedsGeneratedAt)} · ${security.modules.length} modules`
                      : `${security.modules.length} modules covered`,
                  ]
                : undefined
            }
            onRun={security ? undefined : () => void navigate({ to: "/security" })}
            onOpen={() => void navigate({ to: "/security" })}
          />
          <ScanTile
            route="/safety-scan"
            label="Safety"
            status={
              safety ? formatRelative(safety.finishedAt ?? safety.startedAt) : "never run"
            }
            bands={
              safetyCounts
                ? [
                    { n: safetyCounts.serious, color: "var(--status-danger)", label: "serious" },
                    { n: safetyCounts.harmful, color: "var(--status-warning)", label: "harmful" },
                    { n: safetyCounts.concerning, color: "var(--muted-foreground)", label: "concerning" },
                  ]
                : undefined
            }
            lines={
              safety
                ? [
                    safetyCounts && safetyCounts.live > 0
                      ? `${safetyCounts.serious} serious · ${safetyCounts.harmful} harmful · ${safetyCounts.concerning} concerning`
                      : "nothing flagged",
                    // Not "unreviewed": nothing records whether a finding has
                    // been LOOKED at, only whether it was dismissed. Saying
                    // otherwise would be inventing a signal we do not have.
                    safetyCounts && safetyCounts.dismissed > 0
                      ? `${safetyCounts.dismissed} dismissed as false positives`
                      : "",
                    topCategories.length > 0
                      ? topCategories.join(" · ")
                      : `scanned ${formatSources(safety.sources)}`,
                  ]
                : undefined
            }
            onRun={safety ? undefined : () => void navigate({ to: "/safety-scan" })}
            onOpen={() => void navigate({ to: "/safety-scan" })}
          />
        </DashboardTiles>
      )}
    </section>
  );
}

/** "today" / "3 days ago" / "Mar 2024" — a scan's age, not its timestamp. */
function formatRelative(at: number): string {
  const days = Math.floor((Date.now() / 1000 - at) / 86_400);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  return formatDateTime(at).split(",")[0] ?? "earlier";
}

/** One label/value line of the device detail table. Dense: the pair sits on a
 *  single row, values right-aligned and selectable. */
function DeviceRow({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b px-3 py-1.5 last:border-b-0">
      <span className="shrink-0 text-xs text-muted-foreground">{label}</span>
      <span className="select-text truncate text-right text-xs font-medium">
        {value ?? "—"}
      </span>
    </div>
  );
}

/**
 * The landing view once a backup is open (#39). This IS the Device view — the
 * old standalone `/device` route was merged in here, so there is ONE home:
 * picker before a backup is open, full device detail after.
 *
 * "Condensed" means DENSE, not less: every field the old Device view showed is
 * here, in a tighter two-column table instead of a tall centred list. The
 * "open a backup" section is dropped (a backup is already open) and the big
 * phone icon with it — the sidebar hero already shows the device icon + name.
 * The app-features intro stays below.
 */
function DeviceHome({ onChooseOther }: { onChooseOther: () => void }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const imp = useImport();
  const { data: info } = useQuery<BackupInfo | null>({
    queryKey: ["deviceInfo"],
    queryFn: () => client.deviceInfo(),
  });
  // Close out the open-perf trace when the landing view actually has its data —
  // that's the moment the app is usable, which is what #40 is about.
  useEffect(() => {
    if (info !== undefined && openPerfInFlight()) openPerfEnd();
  }, [info]);
  const model = modelName(info?.productType ?? null);
  const subtitle =
    [model, info?.productVersion ? `iOS ${info.productVersion}` : null]
      .filter(Boolean)
      .join(" · ") || "Backup open";

  // Close the open backup: clear session state, then return to the picker.
  async function closeBackup() {
    try {
      await client.closeBackup();
      qc.setQueryData(["hasActiveBackup"], false);
      await qc.invalidateQueries();
      navigate({ to: "/" });
    } catch (e) {
      toast.error("Couldn't close backup", {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  const encryption =
    info?.isEncrypted == null ? (
      "—"
    ) : (
      <span className="inline-flex items-center gap-1.5">
        {info.isEncrypted ? (
          <>
            <Lock className="size-3.5" /> Encrypted
          </>
        ) : (
          <>
            <LockOpen className="size-3.5" /> Not encrypted
          </>
        )}
      </span>
    );

  return (
    <div className="mx-auto max-w-2xl p-8">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h1 className="truncate text-2xl font-semibold">
            {info?.deviceName ?? "Device"}
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">{subtitle}</p>
        </div>
        {/* The backup-management actions the old /device toolbar carried. */}
        <div className="flex shrink-0 items-center gap-1 rounded-lg border border-border/70 bg-muted/40 p-0.5">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                disabled={!info}
                aria-label="Re-import backup"
                onClick={() => info && imp.open(info)}
              >
                <RotateCw className="size-5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              {info
                ? "Re-import (parse this backup again — updates data, e.g. new fields)"
                : "Re-import — waiting for this backup's device info"}
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="Open a different backup"
                onClick={onChooseOther}
              >
                <FolderOpen className="size-5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Open a different backup</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="Close backup"
                onClick={() => void closeBackup()}
              >
                <LogOut className="size-5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              Close this backup (its imported data is kept)
            </TooltipContent>
          </Tooltip>
        </div>
      </div>

      {/* Full device detail, dense: two columns of label/value rows. */}
      <div className="mt-6 grid gap-x-6 sm:grid-cols-2">
        <div className="overflow-hidden rounded-lg border">
          <DeviceRow label="Device name" value={info?.deviceName} />
          <DeviceRow label="Model" value={model} />
          <DeviceRow label="Model identifier" value={info?.productType} />
        </div>
        <div className="mt-2.5 overflow-hidden rounded-lg border sm:mt-0">
          <DeviceRow label="iOS version" value={info?.productVersion} />
          <DeviceRow label="Serial number" value={info?.serialNumber} />
          <DeviceRow
            label="Last backup"
            value={
              info?.lastBackupDate != null
                ? formatDateTime(info.lastBackupDate)
                : null
            }
          />
        </div>
        <div className="mt-2.5 overflow-hidden rounded-lg border sm:col-span-2">
          <DeviceRow label="Encryption" value={encryption} />
        </div>
        {/* Artifacts that declare themselves FACTS rather than tables land here,
            beside the fields read straight from the manifest. A device fact is a
            device fact whichever store it came from, and the reader should not
            have to know that "iOS version" is manifest metadata while "Siri
            language" is a parsed preference. */}
        <DeviceFacts />
      </div>
      {info?.isEncrypted === false && (
        <p className="mt-2 text-xs text-muted-foreground">
          Unencrypted — Health, saved passwords, and tabs synced from your other
          Apple devices are excluded by iOS. Encrypt the backup to include them.
        </p>
      )}

      <DeviceExtractionPrompt />

      <DeviceMoreInformation />

      <HomeDashboard />
    </div>
  );
}

/**
 * The offer to read the device artifacts, at the identity level.
 *
 * It used to live INSIDE the "More information" disclosure, which was fine while
 * everything it produced was also inside that disclosure. Now that facts fold into
 * the identity grid, a prompt hidden behind a collapsed section is the only way to
 * populate fields that appear somewhere else entirely — so the grid would sit
 * half-empty with no visible way to fill it, and nothing on screen would explain
 * why. Measured the same way it was found: with the disclosure shut, no facts
 * appeared and no control offered to change that.
 */
function DeviceExtractionPrompt() {
  const { data: extraction } = useQuery({
    queryKey: ["artifactsExtractionState"],
    queryFn: () => client.artifactsExtractionState(),
  });
  const qc = useQueryClient();
  const [extracting, setExtracting] = useState(false);
  if (extraction !== "never-run" && extraction !== "stale") return null;

  async function run() {
    setExtracting(true);
    try {
      await client.extractArtifacts();
      await qc.invalidateQueries({ queryKey: ["artifacts"] });
      await qc.invalidateQueries({ queryKey: ["artifactRows"] });
      await qc.invalidateQueries({ queryKey: ["artifactsExtractionState"] });
    } catch (e) {
      toast.error("Couldn't read the device details", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setExtracting(false);
    }
  }

  return (
    <div className="mt-3 flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-1.5 text-xs">
      <span className="text-muted-foreground">
        {extraction === "never-run"
          ? "More about this device has not been read from the backup yet."
          : "TraceLoupe can read more about this device than was extracted."}
      </span>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button size="sm" variant="ghost" onClick={run} disabled={extracting}>
            {extracting ? "Reading…" : "Read it now"}
          </Button>
        </TooltipTrigger>
        <TooltipContent>Reads from the backup on disk — no re-import needed</TooltipContent>
      </Tooltip>
    </div>
  );
}

/**
 * Facts-shaped device artifacts, folded into the identity grid above.
 *
 * The module format made everything a table, and much of the device tail is not
 * tabular — iLEAPP's "Identifiers" category alone is 16 artifacts that are mostly
 * single values (UDID, IMEI, advertising id, AirDrop id, device name). Sixteen
 * one-row tables is an absurd way to show sixteen facts, and the two modules that
 * shipped that way read worse than they would as rows here.
 *
 * A fact with no value is not shown at all rather than as a dash: an empty row in
 * this grid would claim the device HAS the setting and left it blank, when the
 * store simply did not record it.
 */
function DeviceFacts() {
  const { hosted } = useHostedArtifacts("device", true);
  const facts = hosted
    .filter((h) => h.artifact.shape === "facts")
    .flatMap((h) => {
      const row = h.rows[0];
      if (!row) return [];
      return h.artifact.columns
        .map((c) => ({ key: `${h.artifact.id}:${c}`, label: c, value: row[c] }))
        .filter((f) => f.value !== null && f.value !== undefined && f.value !== "");
    });
  if (facts.length === 0) return null;

  // Split across the SAME two columns the manifest fields use. A full-width block
  // underneath read as a separate section that nothing had announced — and the
  // whole point of folding facts in here is that a device fact is a device fact
  // whichever store it came from.
  const half = Math.ceil(facts.length / 2);
  const columns = [facts.slice(0, half), facts.slice(half)].filter((c) => c.length > 0);

  return (
    <>
      {columns.map((col, i) => (
        <div
          key={col[0].key}
          className={cn(
            "mt-2.5 overflow-hidden rounded-lg border",
            // Matches the manifest cards: the second column loses its top margin
            // once the grid is side by side.
            i === 1 && "sm:mt-2.5",
          )}
        >
          {col.map((f) => (
            <DeviceRow
              key={f.key}
              label={f.label}
              value={typeof f.value === "boolean" ? (f.value ? "Yes" : "No") : String(f.value)}
            />
          ))}
        </div>
      ))}
    </>
  );
}

/**
 * The device-level artifacts, behind a "More information" disclosure.
 *
 * These are `surface = "device"` modules — configured accounts, Bluetooth
 * pairings, and whatever future TOML declares itself device-level. They belong
 * with the device, and the device view is this one: `/` is the app's single home
 * and IS the Device view once a backup is open, which is why the old standalone
 * `/device` route was merged in here in the first place.
 *
 * Collapsed by default, deliberately. The fields above answer "what device is
 * this?" in six rows; these tables answer follow-up questions and are much
 * longer, so putting them inline would bury the dashboard under them. It is a
 * disclosure rather than a separate destination for the same reason the route was
 * merged: one home, not two.
 *
 * It knows no artifact by name — a new device-level module appears here with no
 * change to this file.
 */
/** Every Apple device that ever wrote Health data to this phone, and the OS
 *  builds each ran.
 *
 *  Health data survives migration between phones, so this reaches back past
 *  devices the person no longer owns — which is why it lives here rather than in
 *  the Health view: it describes the DEVICES, not anyone's fitness.
 *
 *  The two lists come from one table read two ways. A device that contributed a
 *  single sample appears in the first and not the second: it is a device that
 *  was owned, but a zero-length window dates no upgrade. */
function DeviceHistory() {
  const { data: devices } = useQuery({
    queryKey: ["devicesUsed"],
    queryFn: () => client.listDevicesUsed(),
  });
  const { data: osHistory } = useQuery({
    queryKey: ["deviceOsHistory"],
    queryFn: () => client.listDeviceOsHistory(),
  });
  if (!devices?.length) return null;

  const span = (d: { firstAt: number | null; lastAt: number | null }) =>
    d.firstAt == null
      ? "—"
      : `${formatDateTime(d.firstAt)} — ${formatDateTime(d.lastAt)}`;

  return (
    <>
      <section className="mt-4">
        <h2 className="text-sm font-semibold">Devices used</h2>
        <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
          Apple devices that recorded Health data for this person. Health follows
          someone between phones, so this can include devices replaced long
          before this backup was taken.
        </p>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead className="text-muted-foreground">
              <tr className="border-b">
                <th className="py-1 pr-4 text-left font-medium">Device</th>
                <th className="py-1 pr-4 text-left font-medium">In use</th>
                <th className="py-1 text-right font-medium">Samples</th>
              </tr>
            </thead>
            <tbody>
              {devices.map((d) => (
                <tr key={d.model} className="border-b last:border-0">
                  <td className="py-1 pr-4">
                    {modelName(d.model)}
                    {modelName(d.model) !== d.model && (
                      <span className="ml-1.5 text-muted-foreground">{d.model}</span>
                    )}
                  </td>
                  <td className="py-1 pr-4 text-muted-foreground">{span(d)}</td>
                  <td className="py-1 text-right tabular-nums text-muted-foreground">
                    {formatCount(d.samples)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
      {osHistory && osHistory.length > 0 && (
        <section className="mt-4">
          <h2 className="text-sm font-semibold">OS history</h2>
          <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
            Which OS build each device was running while it recorded Health data.
            Builds, not versions — the store records `21D50`, and the mapping to
            `17.3` is not in this backup.
          </p>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead className="text-muted-foreground">
                <tr className="border-b">
                  <th className="py-1 pr-4 text-left font-medium">Device</th>
                  <th className="py-1 pr-4 text-left font-medium">Build</th>
                  <th className="py-1 text-left font-medium">In use</th>
                </tr>
              </thead>
              <tbody>
                {osHistory.map((d) => (
                  <tr key={`${d.model}:${d.osBuild}`} className="border-b last:border-0">
                    <td className="py-1 pr-4">{modelName(d.model)}</td>
                    <td className="py-1 pr-4 tabular-nums">{d.osBuild}</td>
                    <td className="py-1 text-muted-foreground">{span(d)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </>
  );
}

function DeviceMoreInformation() {
  const { hosted } = useHostedArtifacts("device", true);
  const { data: extraction } = useQuery({
    queryKey: ["artifactsExtractionState"],
    queryFn: () => client.artifactsExtractionState(),
  });
  // Controlled, and the component must stay mounted while it is true. Both
  // matter: reading the details invalidates the artifact queries, which briefly
  // leaves nothing to show, and an uncontrolled Collapsible that unmounts loses
  // its open state — measured, the section snapped shut the moment "Read them
  // now" was pressed, dropping the user back to a collapsed row with no idea
  // whether anything had happened.
  const [open, setOpen] = useState(false);
  const needsExtraction = extraction === "never-run" || extraction === "stale";


  // An artifact with no rows is not evidence of anything, and a disclosure that
  // opens onto empty tables is worse than one that is not offered.
  // Tables only: a facts artifact is already in the identity grid above, and
  // showing it here as well would put the same values on screen twice.
  const withRows = hosted.filter((h) => h.artifact.shape !== "facts" && h.rows.length > 0);
  // EXCEPT the encryption-gated ones. An artifact that needs an encrypted backup
  // is deliberately listed with `rowCount === 0` so it can say WHY it is empty
  // instead of vanishing (#197) — "absent" and "impossible here" are different
  // facts, and collapsing them is the failure that whole thread exists to
  // prevent. Filtering purely on `rows.length > 0` silently dropped them, and
  // since #220 moved hosted artifacts out of the standalone view there was no
  // other screen left that would explain them: they rendered nowhere at all.
  const gatedAndEmpty = hosted.filter(
    (h) =>
      h.artifact.shape !== "facts" && h.rows.length === 0 && h.artifact.requiresEncryptedBackup,
  );
  // The device history is not an artifact module, so it is counted separately —
  // otherwise a backup whose only device-level records are the Health device
  // history would decide there is "nothing to show" and never offer the
  // disclosure that contains it.
  const { data: devicesUsed } = useQuery({
    queryKey: ["devicesUsed"],
    queryFn: () => client.listDevicesUsed(),
  });
  const hasHistory = (devicesUsed?.length ?? 0) > 0;
  const nothingToShow =
    withRows.length === 0 && gatedAndEmpty.length === 0 && !needsExtraction && !hasHistory;
  // Never offered when there is nothing to disclose and nothing to read. But once
  // it is open, it stays — a control that vanishes under the pointer is worse than
  // one that admits it found nothing, and unmounting would also discard `open`.
  if (nothingToShow && !open) return null;

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="mt-6">
      {/* NESTING ORDER MATTERS: `CollapsibleTrigger` must be the OUTER `asChild`,
          with `TooltipTrigger` inside it, both landing on one real button.
          Measured, because this is not guessable: with the tooltip outermost the
          trigger's `data-state` stayed "closed" after a click on a fully settled
          page — the disclosure simply could not be opened — and that held both for
          a plain `CollapsibleTrigger` wrapped in `TooltipTrigger asChild` and for
          the two composed the other way round. Reversing them opens it. If a
          future edit flips these, the control goes dead silently, so leave the
          order alone. */}
      <Tooltip>
        <CollapsibleTrigger asChild>
          <TooltipTrigger asChild>
            <button
              type="button"
              className="group inline-flex items-center gap-1 text-xs font-medium text-foreground hover:text-primary"
            >
              <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" />
              More information
              {withRows.length > 0 && (
                <span className="text-muted-foreground">
                  ({withRows.length === 1 ? "1 record set" : `${withRows.length} record sets`})
                </span>
              )}
            </button>
          </TooltipTrigger>
        </CollapsibleTrigger>
        <TooltipContent>
          Accounts, Bluetooth pairings and other device-level detail from this backup
        </TooltipContent>
      </Tooltip>
      <CollapsibleContent>
        {nothingToShow && (
          <p className="mt-3 text-xs text-muted-foreground">
            This backup carried no device-level records TraceLoupe can read yet.
          </p>
        )}
        <DeviceHistory />
        {withRows.map((h) => (
          <section key={h.artifact.id} className="mt-4">
            <h2 className="text-sm font-semibold">{h.artifact.name}</h2>
            <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
              {h.artifact.description}
            </p>
            <ArtifactTable artifact={h.artifact} rows={h.rows} />
          </section>
        ))}
        {gatedAndEmpty.map((h) => (
          <GatedArtifactNote
            key={h.artifact.id}
            name={h.artifact.name}
            description={h.artifact.description}
          />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
}

/** An encryption-gated artifact that is empty because it CANNOT hold anything in
 *  this backup — said out loud, rather than filtered away. Uses the shared hook so
 *  the wording matches every other gated surface instead of drifting from it. */
function GatedArtifactNote({ name, description }: { name: string; description: string }) {
  const message = useEncryptedOnlyEmpty(name, `No rows in ${name}.`);
  return (
    <section className="mt-4">
      <h2 className="text-sm font-semibold">{name}</h2>
      <p className="text-xs leading-relaxed text-muted-foreground">{description}</p>
      <p className="mt-1.5 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        {message}
      </p>
    </section>
  );
}

/** What TraceLoupe does, shown on the home view — the app's front door had no
 *  feature presentation of its own (unlike each content view's empty state). */
function AppFeatures() {
  const features = [
    {
      label: "Browse the whole device",
      detail:
        "Messages, Photos, Contacts, Calls, Safari, Notes, Health and more — reconstructed and searchable.",
    },
    {
      label: "Security Check",
      detail:
        "Scan the backup for traces of known spyware and stalkerware against curated threat feeds.",
    },
    {
      label: "Safety Scan",
      detail:
        "A local AI flags harmful content in messages and notes — threats, harassment, grooming and more.",
    },
    {
      label: "Private by design",
      detail:
        "Everything runs on this Mac. Nothing is uploaded, and the backup is never modified.",
    },
  ];
  return (
    <section className="mt-10 border-t pt-6">
      <h2 className="text-sm font-semibold">What you can do with TraceLoupe</h2>
      <p className="mt-1 text-sm text-muted-foreground">
        Open an iPhone backup and TraceLoupe turns it into a browsable,
        searchable archive — plus security and safety checks, all on your Mac.
      </p>
      <ul className="mt-4 grid gap-2.5 sm:grid-cols-2">
        {features.map((f) => (
          <li key={f.label} className="rounded-lg border bg-card/40 p-3">
            <div className="text-xs font-medium">{f.label}</div>
            <div className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
              {f.detail}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

function BackupCard({
  backup,
  imported,
  onSelect,
  onReimport,
  onForget,
}: {
  backup: BackupInfo;
  imported: boolean;
  onSelect: () => void;
  onReimport: () => void;
  onForget: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const date = backup.lastBackupDate
    ? formatDateTime(backup.lastBackupDate)
    : "unknown date";
  return (
    <Card
      role="button"
      tabIndex={0}
      aria-label={`Open ${backup.deviceName ?? backup.id}`}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      className="cursor-pointer transition-colors hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    >
      <CardContent className="flex items-center gap-4 py-4">
        <Smartphone className="size-8 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-base font-medium">
              {backup.deviceName ?? backup.id}
            </span>
            {backup.isEncrypted === true && (
              <Badge variant="secondary" className="gap-1">
                <Lock className="size-3" /> encrypted
              </Badge>
            )}
            {backup.isEncrypted === false && (
              <Badge
                variant="outline"
                className="gap-1"
                title="Unencrypted backups omit Safari & call history, Health, and saved passwords. Encrypt the backup to include them."
              >
                <LockOpen className="size-3" /> not encrypted
              </Badge>
            )}
          </div>
          <div className="mt-0.5 text-sm text-muted-foreground">
            {backup.productVersion ? `iOS ${backup.productVersion} · ` : ""}
            {date}
          </div>
          {backup.isEncrypted === false && (
            <p className="mt-1 text-xs text-muted-foreground">
              Unencrypted — Safari &amp; call history, Health, and passwords are
              excluded by iOS. Encrypt the backup to include them.
            </p>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2 text-sm text-muted-foreground">
          {imported && confirming ? (
            <>
              <span className="text-xs">Remove imported data?</span>
              <Button
                variant="destructive"
                onClick={(e) => {
                  e.stopPropagation();
                  setConfirming(false);
                  onForget();
                }}
              >
                Remove
              </Button>
              <Button
                variant="ghost"
                onClick={(e) => {
                  e.stopPropagation();
                  setConfirming(false);
                }}
              >
                Cancel
              </Button>
            </>
          ) : imported ? (
            <>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    onClick={(e) => {
                      e.stopPropagation();
                      onReimport();
                    }}
                  >
                    <RotateCw className="size-4" />
                    Re-import
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  Parse this backup again (updates data, e.g. contact photos)
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Remove imported data"
                    onClick={(e) => {
                      e.stopPropagation();
                      setConfirming(true);
                    }}
                  >
                    <Trash2 className="size-4" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  Remove this backup's imported data (keeps the original backup)
                </TooltipContent>
              </Tooltip>
              <Button
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect();
                }}
              >
                <Check className="size-4" /> Open
              </Button>
            </>
          ) : (
            <Button
              onClick={(e) => {
                e.stopPropagation();
                onSelect();
              }}
            >
              <FolderOpen className="size-4" /> Read &amp; open
            </Button>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function FdaGuidance({ path, action }: { path: string; action: React.ReactNode }) {
  const [openError, setOpenError] = useState<string | null>(null);

  async function openSettings() {
    setOpenError(null);
    try {
      await client.openFullDiskAccessSettings();
    } catch (e) {
      setOpenError(String(e));
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>macOS is blocking access to your backups</CardTitle>
        <CardDescription>
          Finder's backup folder is protected. The easiest way in: choose the
          folder yourself — selecting it grants TraceLoupe access, no Full Disk
          Access needed.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4 text-sm text-muted-foreground">
        {action}
        <Collapsible>
          <CollapsibleTrigger className="group inline-flex items-center gap-1 text-xs font-medium text-foreground hover:text-primary">
            <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" />
            Or grant Full Disk Access
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div className="mt-2 rounded-md border bg-muted/40 p-3 text-xs">
              <ol className="list-decimal space-y-1 pl-4">
                <li>
                  <button
                    onClick={openSettings}
                    className="inline-flex items-center gap-1 font-medium text-foreground underline underline-offset-2 hover:text-primary"
                  >
                    <Settings className="size-3.5" />
                    Open Full Disk Access settings
                  </button>
                </li>
                <li>
                  TraceLoupe won't be listed yet — click <b>+</b>, then select
                  the TraceLoupe app (in <b>Applications</b>) and turn it on
                </li>
                <li>Quit and reopen TraceLoupe</li>
              </ol>
              {openError && (
                <p className="mt-2 select-text text-destructive">
                  Couldn't open Settings: {openError}
                </p>
              )}
              <p className="mt-2 text-muted-foreground/80">
                Blocked path:{" "}
                <code className="select-text">{path}</code>
              </p>
            </div>
          </CollapsibleContent>
        </Collapsible>
      </CardContent>
    </Card>
  );
}
