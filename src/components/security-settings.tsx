import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { client, type DetectionSettings } from "@/lib/ipc";

/**
 * Security Check settings (Settings → Security). Gives the passive-check and
 * indicator-update consents a real home — the onboarding dialog promises these
 * can be changed here, so they must exist here.
 */
export function SecuritySettings() {
  const qc = useQueryClient();
  const settings = useQuery({
    queryKey: ["detectionSettings"],
    queryFn: () => client.getDetectionSettings(),
  });

  async function update(patch: Partial<DetectionSettings>) {
    if (!settings.data) return;
    const next = { ...settings.data, ...patch };
    await client.setDetectionSettings(next);
    qc.setQueryData(["detectionSettings"], next);
  }

  if (settings.isPending) {
    return (
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" /> Loading…
      </div>
    );
  }
  if (settings.isError || !settings.data) {
    return (
      <p className="text-sm text-destructive">
        Couldn't read Security settings: {String(settings.error)}
      </p>
    );
  }
  const s = settings.data;
  const autoCheck = s.passiveEnabled && s.passiveConsent === "granted";
  const autoUpdate = s.autoUpdateIndicators && s.fetchConsent === "granted";

  return (
    <div className="flex flex-col gap-4">
      <Row
        id="sec-auto-check"
        label="Check each imported backup automatically"
        description="Compares installed apps against public stalkerware lists and flags matches in Security. Nothing about your data leaves your Mac."
        checked={autoCheck}
        onChange={(on) =>
          update(
            on
              ? { passiveEnabled: true, passiveConsent: "granted" }
              : { passiveEnabled: false },
          )
        }
      />
      <Row
        id="sec-auto-update"
        label="Download the latest indicator lists"
        description="Fetches updated lists from Amnesty International, the MVT project, and Echap over HTTPS at the start of a scan. Only the lists are downloaded — nothing about you or your backup is sent."
        checked={autoUpdate}
        onChange={(on) =>
          update(
            on
              ? { autoUpdateIndicators: true, fetchConsent: "granted" }
              : { autoUpdateIndicators: false },
          )
        }
      />
    </div>
  );
}

function Row({
  id,
  label,
  description,
  checked,
  onChange,
}: {
  id: string;
  label: string;
  description: string;
  checked: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="space-y-0.5">
        <Label htmlFor={id} className="text-sm">
          {label}
        </Label>
        <p className="text-xs text-muted-foreground">{description}</p>
      </div>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}
