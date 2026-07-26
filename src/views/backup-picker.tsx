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
import { client, type BackupInfo } from "@/lib/ipc";
import { useImport } from "@/components/import-provider";
import {
  openPerfEnd,
  openPerfInFlight,
  openPerfPhase,
  openPerfStart,
} from "@/lib/open-perf";
import { modelName } from "@/lib/device-names";
import { formatDateTime } from "@/lib/format";

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
      </div>
      {info?.isEncrypted === false && (
        <p className="mt-2 text-xs text-muted-foreground">
          Unencrypted — Safari &amp; call history, Health, and passwords are
          excluded by iOS. Encrypt the backup to include them.
        </p>
      )}

      <AppFeatures />
    </div>
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
    ? new Date(backup.lastBackupDate * 1000).toLocaleString()
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
            <span className="truncate font-medium">
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
                size="sm"
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
                size="sm"
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
                    size="sm"
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
                size="sm"
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
              size="sm"
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
