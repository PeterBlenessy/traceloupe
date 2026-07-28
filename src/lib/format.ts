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
let dayTime = buildDayTime();
let dayTimeYear = buildDayTimeYear();
let dateOnly = buildDateOnly();
let dateYear = buildDateYear();
let count = buildCount();

function rebuild() {
  time = buildTime();
  dayTime = buildDayTime();
  dayTimeYear = buildDayTimeYear();
  dateOnly = buildDateOnly();
  dateYear = buildDateYear();
  dateHeader = buildDateHeader();
  dateHeaderYear = buildDateHeaderYear();
  count = buildCount();
}

function buildTime() {
  return new Intl.DateTimeFormat(locale, { hour: "numeric", minute: "2-digit", hour12 });
}
function buildDayTime() {
  return new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    hour12,
  });
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

function buildDateOnly() {
  return new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" });
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
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) return time.format(d);
  if (d.getFullYear() === now.getFullYear()) return dateOnly.format(d);
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
  return dayTime.format(new Date(epochSeconds * 1000));
}

/** Like {@link formatMessageTime} but includes the year for dates outside the
 *  current year — used in the Timeline, where rows span many years and the day
 *  separators alone don't tell you which year a given row is in. */
export function formatTimelineTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  const d = new Date(epochSeconds * 1000);
  const fmt = d.getFullYear() === new Date().getFullYear() ? dayTime : dayTimeYear;
  return fmt.format(d);
}

/** Full date + time for a row (calls, history). */
export function formatDateTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  return dayTime.format(new Date(epochSeconds * 1000));
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

let dateHeader = buildDateHeader();
let dateHeaderYear = buildDateHeaderYear();

function buildDateHeader() {
  return new Intl.DateTimeFormat(locale, {
  weekday: "short",
  month: "short",
  day: "numeric",
});
}
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
