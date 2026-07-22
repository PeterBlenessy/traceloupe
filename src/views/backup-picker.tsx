import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";
import { Check, ChevronRight, FolderOpen, Lock, LockOpen, RotateCw, Settings, Smartphone, Trash2 } from "lucide-react";
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

export function BackupPicker() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const imp = useImport();
  const { data: importedIds } = useQuery({
    queryKey: ["importedBackupIds"],
    queryFn: () => client.importedBackupIds(),
  });
  const imported = new Set(importedIds ?? []);

  // Opening an already-parsed backup is instant (just point at its cache).
  // A never-parsed one needs a first-time read: unencrypted starts straight
  // away, encrypted asks for a password first — both via the dialog.
  async function handleOpen(b: BackupInfo) {
    if (imported.has(b.id)) {
      try {
        await client.openBackup(b.id);
        // Mark active optimistically before invalidating (queries are
        // staleTime: Infinity), so the target view doesn't read a stale
        // `hasActiveBackup: false` and bounce back to the picker.
        qc.setQueryData(["hasActiveBackup"], true);
        // Drop any cached artifact data from a previously-open backup; with
        // staleTime: Infinity it would otherwise persist across backups.
        await qc.invalidateQueries();
        navigate({ to: "/messages" });
      } catch (e) {
        toast.error("Couldn't open backup", {
          description: e instanceof Error ? e.message : String(e),
        });
      }
    } else {
      imp.open(b); // first-time read: the provider owns the import + its dialog
    }
  }
  // A folder the user picked (via the native panel), overriding the default
  // MobileSync scan. Selecting a folder grants access without Full Disk Access.
  const [root, setRoot] = useState<string | null>(null);
  const { data, isPending, error, refetch } = useQuery({
    queryKey: ["backups", root],
    queryFn: () => client.listBackups(root ?? undefined),
  });

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
