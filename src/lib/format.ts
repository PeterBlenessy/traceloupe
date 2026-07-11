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

/** Full date + time for a row (calls, history). */
export function formatDateTime(epochSeconds: number | null): string {
  if (!epochSeconds) return "";
  return dayTime.format(new Date(epochSeconds * 1000));
}

/** A call duration like "5:12" or "1:02:08"; empty for zero/none. */
export function formatDuration(seconds: number | null): string {
  if (!seconds || seconds <= 0) return "";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}
