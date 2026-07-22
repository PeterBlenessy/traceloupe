import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { client, type DetectionSettings } from "@/lib/ipc";

/**
 * One-time Security Check onboarding. A SINGLE dialog with two switches (was
 * two stacked modals) — the user sets both consents at once, and the choices
 * are genuinely changeable later in Settings → Security.
 */
export function ConsentDialogs() {
  const qc = useQueryClient();
  const { data: settings } = useQuery({
    queryKey: ["detectionSettings"],
    queryFn: () => client.getDetectionSettings(),
  });

  // Recommended defaults; the switches are pre-checked so "Save" is the easy path.
  const [autoCheck, setAutoCheck] = useState(true);
  const [autoUpdate, setAutoUpdate] = useState(true);

  if (!settings) return null;
  // Show until both consents have been answered.
  const open =
    settings.passiveConsent === "unasked" ||
    settings.fetchConsent === "unasked";
  if (!open) return null;

  async function apply(check: boolean, update: boolean) {
    const next: DetectionSettings = {
      ...settings!,
      passiveConsent: check ? "granted" : "denied",
      passiveEnabled: check,
      fetchConsent: update ? "granted" : "denied",
      autoUpdateIndicators: update,
    };
    await client.setDetectionSettings(next);
    qc.setQueryData(["detectionSettings"], next);
    if (check) {
      await client.runPassiveCheckNow().catch(() => null);
      qc.invalidateQueries({ queryKey: ["scanRuns"] });
      qc.invalidateQueries({ queryKey: ["findings"] });
    }
  }

  return (
    <Dialog open>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Set up Security Check</DialogTitle>
          <DialogDescription>
            Security Check compares your backup against public lists of known
            spyware and stalkerware. Choose how it runs — you can change these
            anytime in Settings → Security.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-4 py-2">
          <label
            htmlFor="consent-auto-check"
            className="flex items-start justify-between gap-4"
          >
            <span className="space-y-0.5">
              <span className="block text-sm font-medium">
                Check each imported backup automatically
              </span>
              <span className="block text-xs text-muted-foreground">
                Looks only at which apps were installed. Nothing about your data
                leaves your Mac.
              </span>
            </span>
            <Switch
              id="consent-auto-check"
              checked={autoCheck}
              onCheckedChange={setAutoCheck}
            />
          </label>

          <label
            htmlFor="consent-auto-update"
            className="flex items-start justify-between gap-4"
          >
            <span className="space-y-0.5">
              <span className="block text-sm font-medium">
                Download the latest indicator lists
              </span>
              <span className="block text-xs text-muted-foreground">
                Fetches updated lists from Amnesty International, the MVT project,
                and Echap over HTTPS. Only the lists are downloaded.
              </span>
            </span>
            <Switch
              id="consent-auto-update"
              checked={autoUpdate}
              onCheckedChange={setAutoUpdate}
            />
          </label>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => apply(false, false)}>
            Not now
          </Button>
          <Button onClick={() => apply(autoCheck, autoUpdate)}>
            {autoCheck ? "Save & run first check" : "Save"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
