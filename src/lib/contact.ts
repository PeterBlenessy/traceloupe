/** Shared helpers for rendering people/handles across views. */

/** Up to two initials for an avatar fallback. Falls back to "#" for numbers. */
export function initials(name: string | null | undefined): string {
  if (!name) return "?";
  const trimmed = name.trim();
  // Phone numbers / handles with no letters → a neutral glyph.
  if (!/[a-z]/i.test(trimmed)) return "#";
  const parts = trimmed.split(/\s+/).filter(Boolean);
  const first = parts[0]?.[0] ?? "";
  const last = parts.length > 1 ? parts[parts.length - 1][0] : "";
  return (first + last).toUpperCase();
}
