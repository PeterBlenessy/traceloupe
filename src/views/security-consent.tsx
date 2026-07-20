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
import { client, type DetectionSettings } from "@/lib/ipc";

/**
 * The two one-time consent prompts (decisions log, CONTEXT.md):
 *  1. Passive Check — asked at first launch after the feature ships.
 *  2. Indicator fetch — asked before the first network fetch of feeds.
 * Each is shown until the corresponding consent leaves "unasked". Both write
 * through DetectionSettings so they never reappear once answered.
 */
export function ConsentDialogs() {
  const qc = useQueryClient();
  const { data: settings } = useQuery({
    queryKey: ["detectionSettings"],
    queryFn: () => client.getDetectionSettings(),
  });

  async function save(next: DetectionSettings, runPassive: boolean) {
    await client.setDetectionSettings(next);
    qc.setQueryData(["detectionSettings"], next);
    if (runPassive) {
      await client.runPassiveCheckNow().catch(() => null);
      qc.invalidateQueries({ queryKey: ["scanRuns"] });
      qc.invalidateQueries({ queryKey: ["findings"] });
    }
  }

  if (!settings) return null;

  const askPassive = settings.passiveConsent === "unasked";
  const askFetch =
    !askPassive &&
    settings.autoUpdateIndicators &&
    settings.fetchConsent === "unasked";

  return (
    <>
      <Dialog open={askPassive}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Check backups for spyware automatically?</DialogTitle>
            <DialogDescription>
              TraceLoupe can quietly check each imported backup against public
              lists of known stalkerware apps, and flag anything it finds in the
              Security section. It looks only at which apps were installed —
              nothing about your data leaves your Mac. You can change this any
              time in Settings.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() =>
                save({ ...settings, passiveConsent: "denied" }, false)
              }
            >
              Not now
            </Button>
            <Button
              onClick={() =>
                save(
                  {
                    ...settings,
                    passiveConsent: "granted",
                    passiveEnabled: true,
                  },
                  true,
                )
              }
            >
              Yes, check automatically
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={askFetch}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Keep spyware indicators up to date?</DialogTitle>
            <DialogDescription>
              To catch the latest threats, TraceLoupe can fetch updated
              indicator lists from public security repositories (Amnesty
              International, the MVT project, and Echap) over HTTPS at the start
              of a scan. Nothing about you or your backup is ever sent — only the
              lists are downloaded. You can turn this off in Settings.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() =>
                save(
                  {
                    ...settings,
                    fetchConsent: "denied",
                    autoUpdateIndicators: false,
                  },
                  false,
                )
              }
            >
              Use bundled lists only
            </Button>
            <Button
              onClick={() =>
                save({ ...settings, fetchConsent: "granted" }, false)
              }
            >
              Allow updates
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
