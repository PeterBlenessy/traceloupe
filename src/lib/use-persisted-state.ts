import { useEffect, useState } from "react";

/**
 * `useState` that persists to localStorage — so per-view UI choices (which tab,
 * which filter, sort order) survive navigation and app restarts. Keys are
 * namespaced (`traceloupe-ui:<key>`) to avoid collisions.
 *
 * The stored value is JSON. A malformed/absent entry falls back to `initial`.
 * Only JSON-serializable state (strings, numbers, booleans, plain objects) —
 * which is all our view state is.
 */
export function usePersistedState<T>(
  key: string,
  initial: T,
): [T, (v: T | ((prev: T) => T)) => void] {
  const storageKey = `traceloupe-ui:${key}`;
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(storageKey);
      return raw != null ? (JSON.parse(raw) as T) : initial;
    } catch {
      return initial;
    }
  });

  useEffect(() => {
    try {
      localStorage.setItem(storageKey, JSON.stringify(value));
    } catch {
      // Storage full / unavailable — persistence is best-effort.
    }
  }, [storageKey, value]);

  return [value, setValue];
}
