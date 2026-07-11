import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Lock, LockOpen, Smartphone } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { client, type BackupInfo } from "@/lib/ipc";
import { ImportDialog } from "@/views/import-dialog";

export function BackupPicker() {
  const navigate = useNavigate();
  const [selected, setSelected] = useState<BackupInfo | null>(null);
  const { data, isPending, error } = useQuery({
    queryKey: ["backups"],
    queryFn: () => client.listBackups(),
  });

  return (
    <div className="mx-auto max-w-2xl p-8">
      <h1 className="text-2xl font-semibold">Your iPhone backups</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        Backups found on this Mac. Select one to import and browse its
        contents. Everything stays on this machine.
      </p>
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
        {data?.status === "permissionDenied" && <FdaGuidance path={data.path} />}
        {data?.status === "notFound" && (
          <Card>
            <CardHeader>
              <CardTitle>No backup folder found</CardTitle>
              <CardDescription>
                Nothing at <code className="select-text">{data.path}</code>. Connect your
                iPhone and create an encrypted backup with Finder, or open a
                backup folder copied from another Mac.
              </CardDescription>
            </CardHeader>
          </Card>
        )}
        {data?.status === "ok" && data.backups.length === 0 && (
          <Card>
            <CardHeader>
              <CardTitle>No backups yet</CardTitle>
              <CardDescription>
                The backup folder exists but contains no backups. Create an
                encrypted backup of your iPhone with Finder first.
              </CardDescription>
            </CardHeader>
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

function FdaGuidance({ path }: { path: string }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>macOS is blocking access to your backups</CardTitle>
        <CardDescription>
          Finder's backup folder is protected. To let Salvage read it, grant it
          Full Disk Access:
        </CardDescription>
      </CardHeader>
      <CardContent className="text-sm text-muted-foreground">
        <ol className="list-decimal space-y-1 pl-5">
          <li>
            Open <b>System Settings → Privacy &amp; Security → Full Disk
            Access</b>
          </li>
          <li>
            Salvage won't be listed yet — click <b>+</b>, then select the Salvage
            app (in <b>Applications</b>) and turn it on
          </li>
          <li>Quit and reopen Salvage</li>
        </ol>
        <p className="mt-3">
          Alternatively, copy the backup folder somewhere Salvage can already
          read (like your Desktop) and open it from there — no Full Disk Access
          needed. Blocked path: <code className="select-text">{path}</code>
        </p>
      </CardContent>
    </Card>
  );
}
