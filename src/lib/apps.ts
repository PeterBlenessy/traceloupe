/**
 * Curated metadata for known apps, keyed by iOS bundle id. Maps the bundle ids
 * from a backup's "Installed Applications" to friendly names, an icon, and how
 * much TraceLoupe can recover from each. Extend this as extraction support grows
 * (the Tier-1/2/3 roadmap).
 */

export type AppSupport =
  | "native" // TraceLoupe parses this app's chats natively today
  | "available" // extractable today (non-native path)
  | "planned" // on the roadmap, not yet built
  | "limited" // stores little/nothing locally (e.g. Snapchat, Signal)
  | "system" // Apple app already covered by a dedicated view
  | "unknown"; // not in the catalog

export interface AppMeta {
  name: string;
  support: AppSupport;
  /** A locally-rendered icon (emoji — the app blocks remote assets via CSP). */
  icon?: string;
}

const CATALOG: Record<string, AppMeta> = {
  // Native chat apps (parsed via the app-chat framework).
  "net.whatsapp.WhatsApp": { name: "WhatsApp", support: "native", icon: "💬" },
  "com.burbn.instagram": { name: "Instagram", support: "native", icon: "📸" },
  "com.zhiliaoapp.musically": { name: "TikTok", support: "native", icon: "🎵" },
  "org.telegram.messenger": { name: "Telegram", support: "native", icon: "✈️" },
  "com.facebook.Messenger": {
    name: "Messenger",
    support: "native",
    icon: "💬",
  },
  "com.kik.chat": { name: "Kik", support: "native", icon: "💬" },
  "com.imo.imoim": { name: "imo", support: "native", icon: "💬" },
  "ch.threema.iapp": { name: "Threema", support: "native", icon: "🔒" },
  "com.viber": { name: "Viber", support: "native", icon: "💜" },
  "com.microsoft.skype.teams": {
    name: "Microsoft Teams",
    support: "native",
    icon: "👥",
  },
  "com.linkedin.LinkedIn": { name: "LinkedIn", support: "native", icon: "💼" },
  // Cache.db apps — data lives in the CFURL cache; parser pending.
  "com.hammerandchisel.discord": {
    name: "Discord",
    support: "planned",
    icon: "🎮",
  },
  "com.tinyspeck.chatlyio": { name: "Slack", support: "planned", icon: "💬" },
  "com.atebits.Tweetie2": {
    name: "X (Twitter)",
    support: "planned",
    icon: "🐦",
  },
  // Little/no recoverable local store.
  "com.toyopagroup.picaboo": {
    name: "Snapchat",
    support: "limited",
    icon: "👻",
  },
  "org.whispersystems.signal": {
    name: "Signal",
    support: "limited",
    icon: "🔒",
  },
  // On the roadmap.
  "com.spotify.client": { name: "Spotify", support: "planned", icon: "🎧" },
  "com.google.Gmail": { name: "Gmail", support: "planned", icon: "✉️" },
  "com.ubercab.UberClient": { name: "Uber", support: "planned", icon: "🚗" },
};

/** Display names of the services parsed natively (keyed for `serviceIcon`). */
const SERVICE_ICONS: Record<string, string> = {
  WhatsApp: "💬",
  Instagram: "📸",
  TikTok: "🎵",
  Telegram: "✈️",
  Messenger: "💬",
  Kik: "💬",
  imo: "💬",
  Threema: "🔒",
  Viber: "💜",
  Teams: "👥",
  LinkedIn: "💼",
  iMessage: "🟦",
  SMS: "💬",
};

/** An emoji icon for a Messages service label (filter chips, thread rows). */
export function serviceIcon(service: string | null | undefined): string | null {
  if (!service) return null;
  return SERVICE_ICONS[service] ?? null;
}

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
  native: "Native",
  available: "Extractable",
  planned: "Coming soon",
  limited: "Minimal local data",
  system: "Built-in",
  unknown: null,
};
