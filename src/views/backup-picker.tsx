import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { FolderOpen, Lock, LockOpen, Smartphone } from "lucide-react";
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
import { ImportDialog } from "@/views/import-dialog";

export function BackupPicker() {
  const navigate = useNavigate();
  const [selected, setSelected] = useState<BackupInfo | null>(null);
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

  return (
    <div className="mx-auto max-w-2xl p-8">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Your iPhone backups</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Backups found on this Mac. Select one to import and browse its
            contents. Everything stays on this machine.
          </p>
        </div>
        {chooseButton}
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
            <BackupCard key={b.id} backup={b} onSelect={() => setSelected(b)} />
          ))}
      </div>

      {selected && (
        <ImportDialog
          backup={selected}
          open={!!selected}
          onOpenChange={(open) => !open && setSelected(null)}
          onDone={() => {
            setSelected(null);
            navigate({ to: "/messages" });
          }}
        />
      )}
    </div>
  );
}

function BackupCard({ backup, onSelect }: { backup: BackupInfo; onSelect: () => void }) {
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
              <Badge variant="outline" className="gap-1">
                <LockOpen className="size-3" /> not encrypted
              </Badge>
            )}
          </div>
          <div className="mt-0.5 text-sm text-muted-foreground">
            {backup.productVersion ? `iOS ${backup.productVersion} · ` : ""}
            {date}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function FdaGuidance({ path, action }: { path: string; action: React.ReactNode }) {
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
          <p className="mb-1 font-medium text-foreground">
            Or grant Full Disk Access:
          </p>
          <ol className="list-decimal space-y-1 pl-5">
            <li>
              Open <b>System Settings → Privacy &amp; Security → Full Disk
              Access</b>
            </li>
            <li>
              Salvage won't be listed yet — click <b>+</b>, then select the
              Salvage app (in <b>Applications</b>) and turn it on
            </li>
            <li>Quit and reopen Salvage</li>
          </ol>
        </div>
        <p className="text-xs">
          Blocked path: <code className="select-text">{path}</code>
        </p>
      </CardContent>
    </Card>
  );
}
