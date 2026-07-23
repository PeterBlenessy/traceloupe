import type { FeedInfo } from "@/lib/ipc";

/** The public research sources the indicator feeds come from — so the named
 *  orgs and "STIX/YAML" aren't bare jargon but link to who's behind them. */
export const FEED_SOURCES: { match: string; label: string; url: string }[] = [
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

export function feedOrg(source: string) {
  const s = source.toLowerCase();
  return FEED_SOURCES.find((o) => s.includes(o.match)) ?? null;
}

/** What each feed CLASS means — the "why is this checked" a bare repo path
 *  can't convey. Keys match the `class` field the indicator loader emits. */
export const CLASS_META: Record<string, { label: string; blurb: string }> = {
  mercenary: {
    label: "Mercenary spyware",
    blurb:
      "State-grade surveillance tools. Indicators come from forensic investigations of real infections — domains, processes, and file paths the malware leaves behind.",
  },
  stalkerware: {
    label: "Stalkerware",
    blurb:
      "Commercial apps sold for covertly monitoring a partner or family member — app IDs, domains, and signing certificates tracked by researchers.",
  },
  watchware: {
    label: "Watchware",
    blurb:
      "Consumer tracking apps that report a person's location or activity to someone else. Less covert than stalkerware, still worth flagging.",
  },
  custom: {
    label: "Custom indicators",
    blurb: "Loaded from your custom indicator folder.",
  },
};

/** "AmnestyTech/pegasus" → "Pegasus", "echap/ioc" → null (generic bucket). */
export function feedThreatName(source: string): string | null {
  const seg = source.split("/").pop() ?? source;
  if (/^(ioc|iocs|indicators?|feed)s?$/i.test(seg)) return null;
  return seg.charAt(0).toUpperCase() + seg.slice(1);
}

/** Threat-led display name for a feed: "Pegasus", else its class label
 *  ("Stalkerware"), else the raw class — never the bare repo path. */
export function feedDisplayName(f: FeedInfo): string {
  return feedThreatName(f.source) ?? CLASS_META[f.class]?.label ?? f.class;
}
