import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Check, FolderOpen, Lock, LockOpen, RotateCw, Settings, Smartphone } from "lucide-react";
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
import { client, type BackupInfo } from "@/lib/ipc";
import { useImport } from "@/components/import-provider";
import { EngineSetup } from "@/views/engine-setup";

export function BackupPicker() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const imp = useImport();
  const { data: engineReady } = useQuery({
    queryKey: ["engineStatus"],
    queryFn: () => client.engineStatus(),
  });
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
      await client.openBackup(b.id);
      // Drop any cached artifact data from a previously-open backup; with
      // staleTime: Infinity it would otherwise persist across backups.
      await qc.invalidateQueries();
      navigate({ to: "/messages" });
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

  async function chooseFolder() {
    const picked = await client.pickBackupFolder();
    if (picked) {
      // Re-run discovery on the picked folder (setRoot changes the query key).
      if (picked === root) void refetch();
      else setRoot(picked);
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
            Pick a backup to open. The first time, Salvage reads it once; after
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
      {engineReady === false && (
        <div className="mt-6">
          <EngineSetup />
        </div>
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
            />
          ))}
      </div>

    </div>
  );
}

function BackupCard({
  backup,
  imported,
  onSelect,
  onReimport,
}: {
  backup: BackupInfo;
  imported: boolean;
  onSelect: () => void;
  onReimport: () => void;
}) {
  const date = backup.lastBackupDate
    ? new Date(backup.lastBackupDate * 1000).toLocaleString()
    : "unknown date";
  return (
    <Card
      onClick={onSelect}
      className="cursor-pointer transition-colors hover:bg-accent/50"
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
          {imported ? (
            <>
              <Button
                variant="ghost"
                size="sm"
                title="Parse this backup again (updates data, e.g. contact photos)"
                onClick={(e) => {
                  e.stopPropagation();
                  onReimport();
                }}
              >
                <RotateCw className="size-4" />
                Re-import
              </Button>
              <span className="inline-flex items-center gap-1 text-foreground">
                <Check className="size-4" /> Open
              </span>
            </>
          ) : (
            "Read & open"
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
          folder yourself — selecting it grants Salvage access, no Full Disk
          Access needed.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4 text-sm text-muted-foreground">
        {action}
        <div>
          <p className="mb-2 font-medium text-foreground">
            Or grant Full Disk Access:
          </p>
          <ol className="list-decimal space-y-1 pl-5">
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
              Salvage won't be listed yet — click <b>+</b>, then select the
              Salvage app (in <b>Applications</b>) and turn it on
            </li>
            <li>Quit and reopen Salvage</li>
          </ol>
          {openError && (
            <p className="mt-2 select-text text-xs text-destructive">
              Couldn't open Settings: {openError}
            </p>
          )}
        </div>
        <p className="text-xs">
          Blocked path: <code className="select-text">{path}</code>
        </p>
      </CardContent>
    </Card>
  );
}
