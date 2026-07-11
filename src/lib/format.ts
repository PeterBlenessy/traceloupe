/** Shared formatting helpers for epoch-seconds timestamps from the cache. */

const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayTime = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});
const dateOnly = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const dateYear = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
});

/** Compact relative-ish label for a thread-list row. */
export function formatListTime(epochSeconds: number | null): string {
  if (!epochSeconds) return "";
  const d = new Date(epochSeconds * 1000);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) return time.format(d);
  if (d.getFullYear() === now.getFullYear()) return dateOnly.format(d);
  return dateYear.format(d);
}

/** Full timestamp for a message separator. */
export function formatMessageTime(epochSeconds: number | null): string {
  if (!epochSeconds) return "";
  return dayTime.format(new Date(epochSeconds * 1000));
}
