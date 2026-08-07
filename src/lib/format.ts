/**
 * Shared formatting for the timestamps and counts the cache produces.
 *
 * **Every formatter here takes an explicit locale.** Passing `undefined` — the
 * obvious thing, and what this file did — resolves to the app's LANGUAGE, and
 * macOS lets language and Region differ: English on a Mac set to Sweden reports
 * `AppleLocale = en_US@rg=sezzzz`, the webview answers `en-US`, and every date
 * in the app read `Jun 8, 12:40 AM` on a machine that writes `8 juni` and keeps
 * a 24-hour clock (#161).
 *
 * `formatCount` was worse: it inserted a space with a regex and used no locale
 * at all, so it was right for Sweden by accident and wrong for every US reader.
 *
 * The resolved locale comes from the backend (`get_system_locale`), which folds
 * the Region override into the locale itself because Intl ignores the `rg`
 * extension. See `system_watch.rs`.
 */

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

/** The locale every formatter in this file uses.
 *
 *  `undefined` until the backend answers, which means the first render or two
 *  may format in the webview's default. That is the same window the app already
 *  accepts for the accent colour, and far better than formatting in the wrong
 *  region for the whole session. */
let locale: string | undefined;

/**
 * Adopt the system locale. Rebuilds every formatter, including the date-only
 * ones — they do not depend on the clock preference but they very much depend
 * on the region.
 */
export function setFormatLocale(next: string) {
  if (next === locale) return;
  locale = next;
  rebuild();
}

// Rebuilt whenever the clock preference OR the locale changes.
let hour12 = hour12For(readClockFormat());
let time = buildTime();
let dayTimeYear = buildDayTimeYear();
let dateYear = buildDateYear();
let count = buildCount();
let decimals = new Map<number, Intl.NumberFormat>();

function rebuild() {
  time = buildTime();
  dayTimeYear = buildDayTimeYear();
  dateYear = buildDateYear();
  dateHeaderYear = buildDateHeaderYear();
  count = buildCount();
  decimals = new Map();
}

function buildTime() {
  return new Intl.DateTimeFormat(locale, { hour: "numeric", minute: "2-digit", hour12 });
}
function buildDayTimeYear() {
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
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
  rebuild();
}

function buildDateYear() {
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
function buildCount() {
  return new Intl.NumberFormat(locale);
}

/** A date/time formatter in the system locale, for the handful of places that
 *  need options this file does not already cover. Exported so no caller has to
 *  reach for `Intl` — and therefore for `undefined` — themselves. */
export function dateFormat(options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  return new Intl.DateTimeFormat(locale, options);
}

/** The resolved locale, for the rare caller that needs to pass it on. */
export function formatLocale(): string | undefined {
  return locale;
}

/** Compact relative-ish label for a thread-list row. */
export function formatListTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  const d = new Date(epochSeconds * 1000);
  const now = new Date();
  // Today shows a clock time and no date at all, so there is no year to omit.
  // Any other day shows a DATE, and a date without its year is ambiguous the
  // moment a backup spans more than one year — which every backup does.
  if (d.toDateString() === now.toDateString()) return time.format(d);
  return dateYear.format(d);
}

/** Time of day only, e.g. "3:00 PM" / "15:00" — respects the clock preference
 *  (unlike a locale-default formatter, which would ignore the 12/24h setting). */
export function formatTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return time.format(new Date(epochSeconds * 1000));
}

/** Full timestamp for a message separator. */
export function formatMessageTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTimeYear.format(new Date(epochSeconds * 1000));
}

/** Like {@link formatMessageTime} but includes the year for dates outside the
 *  current year — used in the Timeline, where rows span many years and the day
 *  separators alone don't tell you which year a given row is in. */
export function formatTimelineTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTimeYear.format(new Date(epochSeconds * 1000));
}

/** Full date + time for a row (calls, history). */
export function formatDateTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTimeYear.format(new Date(epochSeconds * 1000));
}

/** Full date + time that ALWAYS includes the year, e.g. "Jun 3, 2024, 2:04 PM"
 *  — for detail views (the finding popover) where the exact date must be
 *  unambiguous regardless of how long ago it was. */
export function formatDateTimeYear(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTimeYear.format(new Date(epochSeconds * 1000));
}

/** A date without a time, e.g. "May 15, 1990" — used for birthdays. */
export function formatDate(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dateYear.format(new Date(epochSeconds * 1000));
}

let dateHeaderYear = buildDateHeaderYear();

function buildDateHeaderYear() {
  return new Intl.DateTimeFormat(locale, {
  weekday: "short",
  year: "numeric",
  month: "short",
  day: "numeric",
});
}

/** A day separator label for the timeline, e.g. "Sat, Jun 8". */
export function formatDateHeader(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dateHeaderYear.format(new Date(epochSeconds * 1000));
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
 * A count with the separator the user's REGION uses — "450,897" in the US,
 * "450 897" in Sweden, "450.897" in Germany.
 *
 * This used to insert a space with a regex and consult no locale at all, which
 * made it right for Sweden by accident and wrong for every US reader. Returns
 * "" for null/undefined so callers can show their own placeholder.
 */
export function formatCount(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "";
  return count.format(Math.trunc(n));
}

/**
 * A fractional number at a fixed number of decimals — "5.20 km", "1,3 m/s".
 *
 * `toFixed` hard-codes a period. Health printed `formatCount(steps)` (which is
 * region-aware) and `distance.toFixed(2)` (which is not) in the SAME sentence,
 * so a Swedish reader got "8 549 steg · 5.20 km" — one separator per half. That
 * is the split-brain this file was written to end (#161).
 */
export function formatDecimal(
  n: number | null | undefined,
  digits: number,
): string {
  if (n == null || !Number.isFinite(n)) return "";
  let f = decimals.get(digits);
  if (!f) {
    f = new Intl.NumberFormat(locale, {
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    });
    decimals.set(digits, f);
  }
  return f.format(n);
}

/**
 * A byte count as a human size — "2.77 GB", "512 kB".
 *
 * Decimal units (kB/MB/GB), because that is what iOS itself reports for cellular
 * data, and a module's numbers should match the figure the user can check in
 * Settings rather than being 7% smaller for a reason nobody can see.
 *
 * Lives here because this is the only file allowed to build an `Intl` formatter —
 * a `toLocaleString()` elsewhere silently opts out of the resolved locale, and
 * the design lint fails on it. Raw bytes are unreadable at this scale
 * (`2766679954`), which is the whole point: an artifact nobody can read has not
 * shown anything.
 */
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "";
  const bytes = Math.trunc(n);
  if (Math.abs(bytes) < 1000) return `${count.format(bytes)} B`;
  const units = ["kB", "MB", "GB", "TB", "PB"];
  let value = bytes / 1000;
  let unit = 0;
  while (Math.abs(value) >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  // Two decimals below 10, one below 100, none above — so the column stays
  // narrow without throwing away significant digits on small values.
  const digits = Math.abs(value) < 10 ? 2 : Math.abs(value) < 100 ? 1 : 0;
  return `${new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value)} ${units[unit]}`;
}

/** "1 finding" / "3 findings", without every caller re-deriving the ternary.
 *
 *  Written out because three views were doing it inline and the scan tiles were
 *  not doing it at all — "1 modules covered" is the kind of detail that makes a
 *  careful tool look careless. */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${formatCount(n)} ${n === 1 ? one : many}`;
}
