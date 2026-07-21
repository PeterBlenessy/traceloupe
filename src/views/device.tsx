import { useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { FolderOpen, Lock, LockOpen, LogOut, RotateCw, Smartphone } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useViewToolbar } from "@/components/toolbar-context";
import { useImport } from "@/components/import-provider";
import { EmptyView, ErrorState } from "@/components/view";
import { formatDateTime } from "@/lib/format";
import { client, type BackupInfo } from "@/lib/ipc";

import { modelName } from "@/lib/device-names";

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b px-4 py-3 last:border-b-0">
      <span className="shrink-0 text-sm text-muted-foreground">{label}</span>
      <span className="select-text text-right text-sm">{value ?? "—"}</span>
    </div>
  );
}

export function DeviceView() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const imp = useImport();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: info, error } = useQuery<BackupInfo | null>({
    queryKey: ["deviceInfo"],
    queryFn: () => client.deviceInfo(),
    enabled: active === true,
  });

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

  // The backup-management actions the sidebar header no longer exposes (the
  // "TraceLoupe" header now opens this Device view instead of the picker).
  const actions = useMemo(
    () => (
      // Icon-only, matching the other views' toolbar controls (Messages/Notes,
      // the density/theme toggles); labels move into shadcn tooltips.
      <div className="flex items-center gap-1 rounded-lg border border-border/70 bg-muted/40 p-0.5">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              disabled={!info}
              aria-label="Re-import backup"
              onClick={() => info && imp.open(info)}
            >
              <RotateCw className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            Re-import (parse this backup again — updates data, e.g. new fields)
          </TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              aria-label="Open a different backup"
              onClick={() => navigate({ to: "/" })}
            >
              <FolderOpen className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Open a different backup</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              aria-label="Close backup"
              onClick={() => void closeBackup()}
            >
              <LogOut className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Close this backup (its imported data is kept)</TooltipContent>
        </Tooltip>
      </div>
    ),
    // closeBackup is stable enough for this view; deps kept minimal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [info, imp, navigate],
  );

  const toolbar = useMemo(
    () => (active === true ? { title: "Device", modes: actions } : null),
    [active, actions],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <EmptyView
        icon={Smartphone}
        title="No backup open"
        description="Import a backup to see its device info."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const model = modelName(info?.productType ?? null);

  if (error) {
    return (
      <div className="flex h-full flex-col">
        <ErrorState error={error} />
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <div className="mx-auto max-w-xl p-6">
          <div className="flex flex-col items-center gap-3 pb-6 text-center">
            <div className="flex size-16 items-center justify-center rounded-2xl bg-accent">
              <Smartphone className="size-8 text-muted-foreground" />
            </div>
            <div>
              <h2 className="text-xl font-semibold">
                {info?.deviceName ?? "Unknown device"}
              </h2>
              {model && <p className="text-sm text-muted-foreground">{model}</p>}
            </div>
          </div>

          <div className="overflow-hidden rounded-lg border">
            <Row label="Device name" value={info?.deviceName} />
            <Row label="Model" value={model} />
            <Row label="Model identifier" value={info?.productType} />
            <Row label="iOS version" value={info?.productVersion} />
            <Row label="Serial number" value={info?.serialNumber} />
            <Row
              label="Last backup"
              value={
                info?.lastBackupDate != null
                  ? formatDateTime(info.lastBackupDate)
                  : null
              }
            />
            <Row
              label="Encryption"
              value={
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
                )
              }
            />
          </div>
        </div>
      </div>
    </div>
  );
}
