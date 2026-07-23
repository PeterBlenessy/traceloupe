import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, Loader2, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { client, type DetectionSettings } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** The public research sources the indicator feeds come from — so the named
 *  orgs and "STIX/YAML" aren't bare jargon but link to who's behind them. */
const FEED_SOURCES: { match: string; label: string; url: string }[] = [
  {
    match: "amnesty",
    label: "Amnesty International Security Lab",
    url: "https://securitylab.amnesty.org/",
  },
  {
    match: "mvt",
    label: "MVT Project — Mobile Verification Toolkit",
    url: "https://github.com/mvt-project/mvt",
  },
  {
    match: "echap",
    label: "Échap — anti-stalkerware collective",
    url: "https://github.com/AssoEchap/stalkerware-indicators",
  },
];
function feedOrg(source: string) {
  const s = source.toLowerCase();
  return FEED_SOURCES.find((o) => s.includes(o.match)) ?? null;
}

/**
 * Security Check settings (Settings → Security): the consents, plus all
 * indicator-feed MANAGEMENT — updating the lists, seeing each feed's source
 * and size, and pointing at a custom STIX/YAML folder. The Security view keeps
 * only read-only provenance ("N indicators · updated …") next to its verdicts;
 * everything that changes the feeds lives here.
 */
export function SecuritySettings() {
  const qc = useQueryClient();
  const settings = useQuery({
    queryKey: ["detectionSettings"],
    queryFn: () => client.getDetectionSettings(),
  });
  const info = useQuery({
    queryKey: ["indicatorInfo"],
    queryFn: () => client.getIndicatorInfo(),
  });

  const update = useMutation({
    mutationFn: () => client.updateIndicators(),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["indicatorInfo"] }),
  });
  const setCustomDir = useMutation({
    mutationFn: async (dir: string | null) => {
      const s = settings.data ?? (await client.getDetectionSettings());
      await client.setDetectionSettings({ ...s, customIndicatorDir: dir });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["detectionSettings"] });
      qc.invalidateQueries({ queryKey: ["indicatorInfo"] });
    },
  });

  async function updateSettings(patch: Partial<DetectionSettings>) {
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
  const totalIndicators =
    info.data?.feeds.reduce((n, f) => n + f.count, 0) ?? 0;

  return (
    <div className="flex flex-col gap-4">
      <Row
        id="sec-auto-check"
        label="Check each imported backup automatically"
        description="Compares installed apps against public stalkerware lists and flags matches in Security. Nothing about your data leaves your Mac."
        checked={autoCheck}
        onChange={(on) =>
          updateSettings(
            on
              ? { passiveEnabled: true, passiveConsent: "granted" }
              : { passiveEnabled: false },
          )
        }
      />
      <Row
        id="sec-auto-update"
        label="Download the latest indicator lists"
        description="Fetches updated lists from Amnesty International, the MVT project, and Échap over HTTPS at the start of a scan. Only the lists are downloaded — nothing about you or your backup is sent."
        checked={autoUpdate}
        onChange={(on) =>
          updateSettings(
            on
              ? { autoUpdateIndicators: true, fetchConsent: "granted" }
              : { autoUpdateIndicators: false },
          )
        }
      />

      <Separator />

      {/* ---- Indicator feeds: freshness, sources, manual update. ---- */}
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0 text-sm text-muted-foreground">
          {info.data ? (
            <>
              <span className="font-medium text-foreground">
                {totalIndicators.toLocaleString()}
              </span>{" "}
              indicators from {info.data.feeds.length} feeds · updated{" "}
              {info.data.generatedAt ? info.data.generatedAt.slice(0, 10) : "—"}
            </>
          ) : (
            "Loading indicator feeds…"
          )}
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => update.mutate()}
          disabled={update.isPending}
        >
          <RefreshCw
            className={cn("size-4", update.isPending && "animate-spin")}
          />
          Update now
        </Button>
      </div>

      {info.data && info.data.feeds.length > 0 && (
        <div className="space-y-3 rounded-lg border bg-muted/30 p-3 text-xs text-muted-foreground">
          <p>
            Public threat-intelligence feeds maintained by human-rights and
            anti-stalkerware researchers. TraceLoupe downloads only the
            indicator lists — nothing about you or your backup is sent.
          </p>
          <ul className="space-y-1.5">
            {info.data.feeds.map((f) => {
              const org = feedOrg(f.source);
              return (
                <li
                  key={f.source}
                  className="flex items-center justify-between gap-3"
                >
                  <span className="min-w-0">
                    <span className="font-mono text-foreground/80">
                      {f.source}
                    </span>{" "}
                    · {f.count.toLocaleString()} · {f.class}
                  </span>
                  {org && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          onClick={() => void client.openExternal(org.url)}
                          className="inline-flex shrink-0 items-center gap-0.5 underline underline-offset-2 hover:text-foreground"
                        >
                          <ExternalLink className="size-3" />
                          {org.label}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>{org.label}</TooltipContent>
                    </Tooltip>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {/* Custom indicator folder (researcher mode). */}
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-0.5">
          <span className="text-sm">Custom indicators</span>
          <p className="text-xs text-muted-foreground">
            {s.customIndicatorDir ? (
              <span className="font-mono">{s.customIndicatorDir}</span>
            ) : (
              "Add a folder of .stix / .yaml files — structured lists of known-bad domains, addresses, files and app IDs — to include in every scan."
            )}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {s.customIndicatorDir && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setCustomDir.mutate(null)}
              disabled={setCustomDir.isPending}
            >
              Clear
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            disabled={setCustomDir.isPending}
            onClick={async () => {
              const dir = await client.pickFolder(
                "Choose a custom indicator folder",
              );
              if (dir) setCustomDir.mutate(dir);
            }}
          >
            Choose folder…
          </Button>
        </div>
      </div>
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
