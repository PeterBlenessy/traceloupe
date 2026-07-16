import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Lock, LockOpen, Smartphone } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyView, ErrorState, ViewHeader } from "@/components/view";
import { formatDateTime } from "@/lib/format";
import { client, type BackupInfo } from "@/lib/ipc";

/** A few common `ProductType` → marketing-name mappings; falls back to the raw id. */
const MODEL_NAMES: Record<string, string> = {
  "iPhone14,2": "iPhone 13 Pro",
  "iPhone14,3": "iPhone 13 Pro Max",
  "iPhone14,4": "iPhone 13 mini",
  "iPhone14,5": "iPhone 13",
  "iPhone14,7": "iPhone 14",
  "iPhone14,8": "iPhone 14 Plus",
  "iPhone15,2": "iPhone 14 Pro",
  "iPhone15,3": "iPhone 14 Pro Max",
  "iPhone15,4": "iPhone 15",
  "iPhone15,5": "iPhone 15 Plus",
  "iPhone16,1": "iPhone 15 Pro",
  "iPhone16,2": "iPhone 15 Pro Max",
};

function modelName(productType: string | null): string | null {
  if (!productType) return null;
  return MODEL_NAMES[productType] ?? productType;
}

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
        <ViewHeader title="Device" />
        <ErrorState error={error} />
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Device" />
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
