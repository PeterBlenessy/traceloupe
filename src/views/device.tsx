/**
 * The Device view — how this iPhone is configured, as opposed to what is on it.
 *
 * It is the home for `surface = "device"` artifacts, and the reason it exists is
 * that it was missing. `Surface::Device` had been in the enum since the module
 * format landed, but nothing in the app ever called
 * `useHostedArtifacts("device")`, so a module declaring it would load, validate,
 * run, decrypt its store, write its rows — and render nowhere, silently (#231).
 *
 * WHY A NEW DESTINATION RATHER THAN FOLDING INTO AN EXISTING ONE
 *
 * The agreed rule (#220) is that artifact data folds into the view closest in
 * meaning, and only genuinely outstanding data earns its own screen. Device
 * configuration has no close existing view: configured accounts and Bluetooth
 * pairings are not photos, messages or apps. Folding accounts into Apps was tried
 * first and measurement killed it — every `ZOWNINGBUNDLEID` in a real backup is a
 * system daemon (`accountsd`, `dataaccessd`, `purplebuddy`), not an installed
 * app, so the join attached almost nothing.
 *
 * Unlike Apps, this view is a **list of one**: there is a single device, so an
 * artifact here is shown whole rather than attached to a row, and no
 * `join_column` is required (`Surface::attaches_to_a_row`).
 *
 * It knows no artifact by name. A new device-level TOML module appears here with
 * no change to this file.
 */
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Smartphone } from "lucide-react";

import { ArtifactTable } from "@/components/artifact-table";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useViewToolbar } from "@/components/toolbar-context";
import { EmptyView, ErrorState, ListSkeleton, NoBackupState } from "@/components/view";
import { useHostedArtifacts } from "@/lib/use-hosted-artifacts";
import { modelName } from "@/lib/device-names";
import { formatDateTime } from "@/lib/format";
import { client } from "@/lib/ipc";
import { cn } from "@/lib/utils";

export function DeviceView() {
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: info,
    isPending: infoPending,
    error,
  } = useQuery({
    queryKey: ["deviceInfo"],
    queryFn: () => client.deviceInfo(),
    enabled: active === true,
  });
  const { hosted, isPending: hostedPending } = useHostedArtifacts("device", active === true);

  // A backup imported before these modules existed has no rows, and the artifacts
  // would simply be absent with nothing saying why — the trap #216 fixed.
  const { data: extraction } = useQuery({
    queryKey: ["artifactsExtractionState"],
    queryFn: () => client.artifactsExtractionState(),
    enabled: active === true,
  });
  const queryClient = useQueryClient();
  const [extracting, setExtracting] = useState(false);
  const needsExtraction = extraction === "never-run" || extraction === "stale";

  async function runExtraction() {
    setExtracting(true);
    try {
      await client.extractArtifacts();
      await queryClient.invalidateQueries({ queryKey: ["artifacts"] });
      await queryClient.invalidateQueries({ queryKey: ["artifactRows"] });
      await queryClient.invalidateQueries({ queryKey: ["artifactsExtractionState"] });
    } finally {
      setExtracting(false);
    }
  }

  useViewToolbar(null);

  if (active === false) {
    return (
      <NoBackupState
        icon={Smartphone}
        title="Inspect the device itself"
        lead="How this iPhone is set up, rather than what is stored on it — the services signed in to it, the accessories paired with it, and the identity it reports."
        features={[
          {
            label: "Identity",
            detail: "Name, model, iOS version, serial number and when the backup was taken.",
          },
          {
            label: "Accounts",
            detail: "Every service signed in on the device, and when each was added.",
          },
          {
            label: "Bluetooth",
            detail: "Paired accessories and the address each one advertises.",
          },
          {
            label: "Grows by itself",
            detail: "New device-level artifacts appear here as TraceLoupe learns to read them.",
          },
        ]}
        note="Read straight from the backup on this Mac."
      />
    );
  }

  if (error) return <ErrorState error={error} />;
  if (infoPending || hostedPending) return <ListSkeleton />;

  // Only artifacts that actually produced rows. An artifact with none is not
  // evidence of anything, and a page of empty tables reads as a broken view.
  const withRows = hosted.filter((h) => h.rows.length > 0);

  const facts: { label: string; value: string }[] = [];
  if (info) {
    const model = modelName(info.productType);
    if (info.deviceName) facts.push({ label: "Name", value: info.deviceName });
    if (model ?? info.productType) {
      facts.push({ label: "Model", value: model ?? info.productType! });
    }
    if (info.productVersion) facts.push({ label: "iOS", value: info.productVersion });
    if (info.serialNumber) facts.push({ label: "Serial", value: info.serialNumber });
    if (info.lastBackupDate) {
      facts.push({ label: "Backed up", value: formatDateTime(info.lastBackupDate) });
    }
    facts.push({
      label: "Backup",
      // `null` is genuinely unknown here, and saying "not encrypted" for it would
      // be a claim the manifest does not support.
      value:
        info.isEncrypted === null
          ? "Not recorded"
          : info.isEncrypted
            ? "Encrypted"
            : "Not encrypted",
    });
  }

  return (
    // `min-w-0` on both this column and the scroller below: a flex item's
    // min-width defaults to `auto`, so a wide child expands it instead of being
    // contained. Without it the nowrap artifact tables stretched the whole view
    // 152px past the window — and because nothing scrolls horizontally at that
    // level, the overflowing columns were not merely awkward, they were
    // unreachable. Measured, not guessed: main's right edge sat at 1252 in an
    // 1100px viewport.
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      {needsExtraction && (
        // No `underlap` anywhere in this view, so a bar here cannot end up under
        // the translucent title bar the way #224's did.
        <div className="flex items-center gap-2 border-b px-3 py-1.5 text-xs">
          <span className="text-muted-foreground">
            {extraction === "never-run"
              ? "Device details have not been read from this backup yet."
              : "TraceLoupe can read more about this device than was extracted."}
          </span>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button size="sm" variant="ghost" onClick={runExtraction} disabled={extracting}>
                {extracting ? "Reading…" : "Read them now"}
              </Button>
            </TooltipTrigger>
            <TooltipContent>Reads from the backup on disk — no re-import needed</TooltipContent>
          </Tooltip>
        </div>
      )}

      {/* `pt-16` clears the absolutely-positioned translucent title bar, and is
          only right when this scroller IS the topmost element. With the
          extraction prompt above it that stops being true and the padding is
          just dead space — the same reasoning that turns `underlap` off in Apps,
          for the same bar. */}
      <div
        className={cn(
          "min-h-0 min-w-0 flex-1 overflow-y-auto px-4 pb-8",
          needsExtraction ? "pt-4" : "pt-16",
        )}
      >
        {facts.length > 0 && (
          <section className="mb-6">
            <h2 className="mb-2 text-sm font-semibold">Identity</h2>
            <dl className="grid gap-x-6 gap-y-2 sm:grid-cols-2">
              {facts.map((f) => (
                <div key={f.label} className="flex items-baseline justify-between gap-4 border-b py-1.5">
                  <dt className="text-xs text-muted-foreground">{f.label}</dt>
                  <dd className="text-xs font-medium">{f.value}</dd>
                </div>
              ))}
            </dl>
          </section>
        )}

        {withRows.map((h) => (
          <section key={h.artifact.id} className="mb-6">
            <h2 className="text-sm font-semibold">{h.artifact.name}</h2>
            <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
              {h.artifact.description}
            </p>
            <ArtifactTable artifact={h.artifact} rows={h.rows} />
          </section>
        ))}

        {facts.length === 0 && withRows.length === 0 && !needsExtraction && (
          <EmptyView
            icon={Smartphone}
            title="Nothing about the device was recorded"
            description="This backup carried no device details TraceLoupe can read."
          />
        )}
      </div>
    </div>
  );
}
