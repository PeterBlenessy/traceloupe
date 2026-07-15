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
  /** simple-icons brand slug (rendered by `BrandIcon`); absent → monogram. */
  slug?: string;
}

const CATALOG: Record<string, AppMeta> = {
  // Native chat apps (parsed via the app-chat framework).
  "net.whatsapp.WhatsApp": { name: "WhatsApp", support: "native", slug: "whatsapp" },
  "com.burbn.instagram": { name: "Instagram", support: "native", slug: "instagram" },
  "com.zhiliaoapp.musically": { name: "TikTok", support: "native", slug: "tiktok" },
  "org.telegram.messenger": { name: "Telegram", support: "native", slug: "telegram" },
  "com.facebook.Messenger": { name: "Messenger", support: "native", slug: "messenger" },
  "com.kik.chat": { name: "Kik", support: "native", slug: "kik" },
  "com.imo.imoim": { name: "imo", support: "native" },
  "ch.threema.iapp": { name: "Threema", support: "native", slug: "threema" },
  "com.viber": { name: "Viber", support: "native", slug: "viber" },
  "com.microsoft.skype.teams": { name: "Microsoft Teams", support: "native" },
  "com.linkedin.LinkedIn": { name: "LinkedIn", support: "native", slug: "linkedin" },
  // Cache.db apps — data lives in the CFURL cache; parser pending.
  "com.hammerandchisel.discord": { name: "Discord", support: "planned", slug: "discord" },
  "com.tinyspeck.chatlyio": { name: "Slack", support: "planned", slug: "slack" },
  "com.atebits.Tweetie2": { name: "X (Twitter)", support: "planned", slug: "x" },
  // Little/no recoverable local store.
  "com.toyopagroup.picaboo": { name: "Snapchat", support: "limited", slug: "snapchat" },
  "org.whispersystems.signal": { name: "Signal", support: "limited", slug: "signal" },
  // On the roadmap.
  "com.spotify.client": { name: "Spotify", support: "planned", slug: "spotify" },
  "com.google.Gmail": { name: "Gmail", support: "planned", slug: "gmail" },
  "com.ubercab.UberClient": { name: "Uber", support: "planned", slug: "uber" },
};

/** Messages service / media-source display name → simple-icons brand slug. */
const SERVICE_SLUGS: Record<string, string> = {
  iMessage: "imessage",
  // Media whose source is the Messages app (attachments) — show the iMessage mark.
  Messages: "imessage",
  WhatsApp: "whatsapp",
  Instagram: "instagram",
  TikTok: "tiktok",
  Telegram: "telegram",
  Messenger: "messenger",
  Threema: "threema",
  Viber: "viber",
  LinkedIn: "linkedin",
  Kik: "kik",
  Snapchat: "snapchat",
  Signal: "signal",
};

/** The brand slug for a Messages service label (filter chips), or null. */
export function serviceSlug(service: string | null | undefined): string | null {
  if (!service) return null;
  return SERVICE_SLUGS[service] ?? null;
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
