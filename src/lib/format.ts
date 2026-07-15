/** Shared formatting helpers for epoch-seconds timestamps from the cache. */

/** User clock preference: locale default, or force 12-/24-hour. */
export type ClockFormat = "system" | "12h" | "24h";
export const CLOCK_KEY = "traceloupe-clock";

/** `hour12` option for Intl: undefined = locale default, else forced. */
function hour12For(pref: ClockFormat): boolean | undefined {
  return pref === "system" ? undefined : pref === "12h";
}

// Read the persisted preference at module load so the very first render already
// uses the right clock, before the settings provider mounts.
export function readClockFormat(): ClockFormat {
  const raw = typeof localStorage !== "undefined" ? localStorage.getItem(CLOCK_KEY) : null;
  return raw === "12h" || raw === "24h" ? raw : "system";
}

// The time-bearing formatters are rebuilt whenever the clock preference changes
// (date-only formatters don't depend on it, so they stay constant).
let hour12 = hour12For(readClockFormat());
let time = buildTime();
let dayTime = buildDayTime();

function buildTime() {
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", hour12 });
}
function buildDayTime() {
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    hour12,
  });
}

/**
 * Switch the clock preference used by all time formatters. Called by the
 * settings provider; views re-render and pick up the new formatters on their
 * next render.
 */
export function setClockFormat(pref: ClockFormat) {
  hour12 = hour12For(pref);
  time = buildTime();
  dayTime = buildDayTime();
}

const dateOnly = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const dateYear = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
});

/** Compact relative-ish label for a thread-list row. */
export function formatListTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  const d = new Date(epochSeconds * 1000);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) return time.format(d);
  if (d.getFullYear() === now.getFullYear()) return dateOnly.format(d);
  return dateYear.format(d);
}

/** Full timestamp for a message separator. */
export function formatMessageTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTime.format(new Date(epochSeconds * 1000));
}

/** Full date + time for a row (calls, history). */
export function formatDateTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTime.format(new Date(epochSeconds * 1000));
}

const dateHeader = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
});
const dateHeaderYear = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  year: "numeric",
  month: "short",
  day: "numeric",
});

/** A day separator label for the timeline, e.g. "Sat, Jun 8". */
export function formatDateHeader(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  const d = new Date(epochSeconds * 1000);
  const fmt = d.getFullYear() === new Date().getFullYear() ? dateHeader : dateHeaderYear;
  return fmt.format(d);
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

/**
 * A count with a thousands separator, e.g. 450897 → "450 897". Uses a
 * non-breaking space (U+00A0) so a large count never wraps mid-number.
 * Returns "" for null/undefined so callers can show their own placeholder.
 */
export function formatCount(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "";
  const neg = n < 0;
  const digits = Math.abs(Math.trunc(n))
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, " ");
  return neg ? `-${digits}` : digits;
}
