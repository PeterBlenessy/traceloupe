/**
 * Wording for a list that came back empty.
 *
 * A list emptied by a FILTER has told you nothing about the backup, and saying
 * "not in this backup" there is this app's central claim stated wrongly. The
 * distinction between *absent*, *impossible here* and *filtered out* is the
 * whole job — see `use-encrypted-only.ts` for the other half of it.
 *
 * Calls and Safari both accounted for the search box and ignored the time
 * filter, so narrowing to a week with no matches announced that the device had
 * never made a call. Notes already got this right, which is why it does not use
 * this helper: its list query takes no arguments, so an empty result there
 * really does mean the backup holds none.
 */

/** What is currently narrowing a list, beyond the data itself. */
export type Narrowing = {
  /** The search box, when it has a term in it. */
  search?: string | null;
  /** A time range with at least one bound set. */
  timeFiltered?: boolean;
  /** Any other active filter — a kind pill, a service, a category. */
  otherFiltered?: boolean;
};

/**
 * Pick the honest empty message.
 *
 * `absent` is the wording for a genuinely empty source, and is only ever
 * returned when nothing is narrowing the list. `noun` is the plural thing being
 * listed, in the app's own words ("calls", "bookmarks").
 */
export function emptyListMessage(
  narrowing: Narrowing,
  absent: string,
  noun: string,
): string {
  const searching = Boolean(narrowing.search?.trim());
  const filtered = Boolean(narrowing.timeFiltered || narrowing.otherFiltered);

  if (searching && filtered) return `No ${noun} match this search in this time range.`;
  if (searching) return `No ${noun} match this search.`;
  // Deliberately not "no results" — naming the thing keeps the sentence about
  // the user's data rather than about the query.
  if (narrowing.timeFiltered) return `No ${noun} in this time range.`;
  if (filtered) return `No ${noun} match these filters.`;
  return absent;
}

/** Whether a `TimeRange`-shaped value is actually narrowing anything. */
export function isTimeFiltered(range: {
  lo: number | null;
  hi: number | null;
}): boolean {
  return range.lo != null || range.hi != null;
}
