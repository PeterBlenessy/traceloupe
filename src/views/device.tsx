import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Lock, LockOpen, Smartphone } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useViewToolbar } from "@/components/toolbar-context";
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
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: info, error } = useQuery<BackupInfo | null>({
    queryKey: ["deviceInfo"],
    queryFn: () => client.deviceInfo(),
    enabled: active === true,
  });

  const toolbar = useMemo(
    () => (active === true ? { title: "Device" } : null),
    [active],
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
