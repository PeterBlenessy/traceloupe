import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { client } from "@/lib/ipc";
import { contactName } from "@/lib/contact";

export interface ResolvedContact {
  id: number;
  name: string;
  hasImage: boolean;
}

/**
 * A match key for a phone number: its last 8 significant digits. Message handles
 * store the full international form (e.g. +46701234567) while contacts are often
 * saved nationally (070-123 45 67); comparing a suffix ignores the country code
 * and trunk zero so both forms resolve to the same contact. 8 digits is long
 * enough to avoid collisions within one person's address book.
 */
function normalizePhone(raw: string): string {
  const digits = raw.replace(/\D/g, "");
  return digits.length > 8 ? digits.slice(-8) : digits;
}

/**
 * A comparable key for a phone number or email, so the same value in different
 * formats matches. Returns "" for values that can't be keyed. Shared by the
 * resolver and by callers that match the other direction (contact → threads).
 */
export function phoneOrEmailKey(value: string): string {
  const v = value.trim();
  if (!v) return "";
  if (v.includes("@")) return `e:${v.toLowerCase()}`;
  const k = normalizePhone(v);
  return k ? `p:${k}` : "";
}

/**
 * Resolves a message handle (phone number or email) to a saved contact, so
 * surfaces can show a name and photo instead of a raw identifier. Contacts are
 * loaded once (shared React Query cache) and indexed by normalized phone/email.
 */
export function useContactResolver(): (handle: string | null | undefined) => ResolvedContact | null {
  // Gate on an open backup so this doesn't fire `list_contacts` before/without
  // one (it's called by Calls/Messages, which may mount pre-backup). The
  // hasActiveBackup query is shared + cached, so this adds no extra round-trip.
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: contacts } = useQuery({
    queryKey: ["contacts"],
    queryFn: () => client.listContacts(),
    enabled: active === true,
  });

  return useMemo(() => {
    const byKey = new Map<string, ResolvedContact>();
    for (const c of contacts ?? []) {
      const resolved: ResolvedContact = {
        id: c.id,
        name: contactName(c),
        hasImage: c.hasImage,
      };
      for (const p of c.phones) {
        const k = phoneOrEmailKey(p.value);
        if (k) byKey.set(k, resolved);
      }
      for (const e of c.emails) {
        const k = phoneOrEmailKey(e.value);
        if (k) byKey.set(k, resolved);
      }
    }
    return (handle) => {
      if (!handle) return null;
      const k = phoneOrEmailKey(handle);
      return k ? (byKey.get(k) ?? null) : null;
    };
  }, [contacts]);
}
