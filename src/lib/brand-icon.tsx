/**
 * Real app/brand logos, bundled locally (the app's CSP blocks remote assets).
 *
 * Uses the `simple-icons` package — official brand SVG paths + colors, bundled at
 * build time and tree-shaken to just the icons we reference. Run `npm install` to
 * pull it. Any app without a brand icon falls back to a clean monogram tile, so
 * the UI never shows a mismatched/placeholder glyph.
 */
import {
  siWhatsapp,
  siInstagram,
  siTelegram,
  siTiktok,
  siDiscord,
  siSlack,
  siX,
  siLinkedin,
  siSignal,
  siSpotify,
  siGmail,
  siViber,
  siThreema,
  siMessenger,
  siImessage,
  siKik,
  siSnapchat,
  siUber,
} from "simple-icons";

interface SimpleIcon {
  hex: string;
  path: string;
}

/** Stable slug → brand icon. Apps not here render a monogram (imo/Teams). */
const ICONS: Record<string, SimpleIcon> = {
  whatsapp: siWhatsapp,
  instagram: siInstagram,
  telegram: siTelegram,
  tiktok: siTiktok,
  discord: siDiscord,
  slack: siSlack,
  x: siX,
  linkedin: siLinkedin,
  signal: siSignal,
  spotify: siSpotify,
  gmail: siGmail,
  viber: siViber,
  threema: siThreema,
  messenger: siMessenger,
  imessage: siImessage,
  kik: siKik,
  snapchat: siSnapchat,
  uber: siUber,
};

/**
 * Relative luminance (0=black, 1=white) of a 6-digit hex color. Used to detect
 * near-monochrome brand marks (X, TikTok, Uber, Apple) whose own color would
 * vanish against a same-toned background — those inherit `currentColor` instead.
 */
function luminance(hex: string): number {
  const n = parseInt(hex, 16);
  const r = (n >> 16) & 0xff;
  const g = (n >> 8) & 0xff;
  const b = n & 0xff;
  return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
}

/** A brand logo for `slug`, or a monogram of `name` when there's no icon. */
export function BrandIcon({
  slug,
  name,
  className = "size-4",
}: {
  slug: string | null | undefined;
  name: string;
  className?: string;
}) {
  const icon = slug ? ICONS[slug] : undefined;
  if (icon) {
    // Near-black or near-white marks read as "currentColor" so they stay visible
    // in both light and dark themes; distinct brand colors keep their own hue.
    const lum = luminance(icon.hex);
    const fill = lum < 0.12 || lum > 0.9 ? "currentColor" : `#${icon.hex}`;
    return (
      <svg
        role="img"
        aria-label={name}
        viewBox="0 0 24 24"
        className={className}
        fill={fill}
      >
        <path d={icon.path} />
      </svg>
    );
  }
  return (
    <span
      aria-label={name}
      className="inline-flex items-center justify-center text-[0.65em] font-semibold text-muted-foreground"
    >
      {name.slice(0, 2).toUpperCase()}
    </span>
  );
}

/** Whether a brand logo exists for `slug` (else a monogram is used). */
export function hasBrandIcon(slug: string | null | undefined): boolean {
  return !!slug && slug in ICONS;
}
