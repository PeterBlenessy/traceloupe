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

export function useSystemAccent() {
  useEffect(() => {
    let cancelled = false;

    const apply = (value: string | null) => {
      const root = document.documentElement;
      if (value) root.style.setProperty("--accent-system-value", value);
      else root.style.removeProperty("--accent-system-value");
    };

    const fetchAndApply = async () => {
      try {
        const value = await client.systemAccentColor();
        if (!cancelled) apply(value ?? null);
      } catch {
        if (!cancelled) apply(null);
      }
    };

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
