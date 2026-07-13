/**
 * Curated metadata for known apps, keyed by iOS bundle id. Maps the bundle ids
 * from a backup's "Installed Applications" to friendly names and how much
 * TraceLoupe can recover from each. Extend this as extraction support grows
 * (the Tier-1/2/3 roadmap).
 */

export type AppSupport =
  | "available" // TraceLoupe can extract this app's data today
  | "planned" // on the roadmap, not yet built
  | "limited" // stores little/nothing locally (e.g. Snapchat, Signal)
  | "system" // Apple app already covered by a dedicated view
  | "unknown"; // not in the catalog

export interface AppMeta {
  name: string;
  support: AppSupport;
}

const CATALOG: Record<string, AppMeta> = {
  "net.whatsapp.WhatsApp": { name: "WhatsApp", support: "planned" },
  "com.burbn.instagram": { name: "Instagram", support: "planned" },
  "com.toyopagroup.picaboo": { name: "Snapchat", support: "limited" },
  "com.zhiliaoapp.musically": { name: "TikTok", support: "planned" },
  "org.telegram.messenger": { name: "Telegram", support: "planned" },
  "org.whispersystems.signal": { name: "Signal", support: "limited" },
  "com.spotify.client": { name: "Spotify", support: "planned" },
  "com.google.Gmail": { name: "Gmail", support: "planned" },
  "com.tinyspeck.chatlyio": { name: "Slack", support: "planned" },
  "com.ubercab.UberClient": { name: "Uber", support: "planned" },
  "com.facebook.Messenger": { name: "Messenger", support: "planned" },
  "com.atebits.Tweetie2": { name: "X (Twitter)", support: "planned" },
  "com.hammerandchisel.discord": { name: "Discord", support: "planned" },
};

/** Resolve a bundle id to display metadata, guessing a name when unknown. */
export function appMeta(bundleId: string): AppMeta {
  const known = CATALOG[bundleId];
  if (known) return known;
  if (bundleId.startsWith("com.apple.")) {
    return { name: prettyFromBundle(bundleId), support: "system" };
  }
  return { name: prettyFromBundle(bundleId), support: "unknown" };
}

/** "com.burbn.instagram" → "Instagram"; a readable fallback name. */
function prettyFromBundle(bundleId: string): string {
  const last = bundleId.split(".").pop() ?? bundleId;
  return last.charAt(0).toUpperCase() + last.slice(1);
}

export const SUPPORT_LABEL: Record<AppSupport, string | null> = {
  available: "Extractable",
  planned: "Coming soon",
  limited: "Minimal local data",
  system: "Built-in",
  unknown: null,
};
