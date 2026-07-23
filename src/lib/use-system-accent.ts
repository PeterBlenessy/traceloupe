/**
 * useSystemAccent — keeps `--accent-system-value` on <html> in sync with the
 * macOS accent color, so the whole UI follows System Settings.
 *
 * The value is an oklch string from the `get_system_accent_color` command;
 * `index.css` consumes it via `--accent-color: var(--accent-system-value, …)`
 * with a baked-in blue fallback, so non-macOS hosts (or a failed invoke) simply
 * keep the default.
 *
 * macOS doesn't push accent changes into a running process without an
 * NSDistributedNotificationCenter observer, so we re-fetch on window focus and
 * visibility change — changing the accent in System Settings and switching back
 * to TraceLoupe picks it up without native plumbing.
 */
import { useEffect } from "react";
import { client } from "@/lib/ipc";

/** Last accent seen, reapplied synchronously on mount so the first paint
 *  doesn't flash the baked-in fallback blue before the invoke round-trips. */
const STORAGE_KEY = "traceloupe-system-accent";

/** Only the shape theme.rs emits. An INVALID custom-property value is worse
 *  than an absent one: `var(--x, fallback)` falls back only when the var is
 *  unset, so a corrupt cached string would leave --primary/--ring computing
 *  to nothing (colorless buttons and rings) until the next successful invoke. */
function isValidAccent(value: string): boolean {
  return value.startsWith("oklch(") && CSS.supports("color", value);
}

export function useSystemAccent() {
  useEffect(() => {
    let cancelled = false;

    const apply = (value: string | null) => {
      if (value && !isValidAccent(value)) value = null;
      const root = document.documentElement;
      if (value) root.style.setProperty("--accent-system-value", value);
      else root.style.removeProperty("--accent-system-value");
      try {
        if (value) localStorage.setItem(STORAGE_KEY, value);
        else localStorage.removeItem(STORAGE_KEY);
      } catch {
        // Storage unavailable — the accent still applies for this session.
      }
    };

    const fetchAndApply = async () => {
      try {
        const value = await client.systemAccentColor();
        if (!cancelled) apply(value ?? null);
      } catch {
        if (!cancelled) apply(null);
      }
    };

    try {
      const cached = localStorage.getItem(STORAGE_KEY);
      if (cached && isValidAccent(cached)) {
        document.documentElement.style.setProperty(
          "--accent-system-value",
          cached,
        );
      }
    } catch {
      // Storage unavailable — fall through to the fetch.
    }
    void fetchAndApply();

    const onFocusOrVisible = () => {
      if (document.visibilityState === "hidden") return;
      void fetchAndApply();
    };

    window.addEventListener("focus", onFocusOrVisible);
    document.addEventListener("visibilitychange", onFocusOrVisible);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", onFocusOrVisible);
      document.removeEventListener("visibilitychange", onFocusOrVisible);
    };
  }, []);
}
