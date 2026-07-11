/** Shared helpers for rendering people/handles across views. */

import type { Contact } from "@/lib/ipc";

/** A display name for a contact: full name, else organization, else "No Name". */
export function contactName(c: Contact): string {
  const name = [c.firstName, c.lastName].filter(Boolean).join(" ").trim();
  return name || c.organization || "No Name";
}

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
