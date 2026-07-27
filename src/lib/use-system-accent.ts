/**
 * useSystemAccent — keeps `--accent-system-value` on <html> in sync with the
 * macOS accent color, so the whole UI follows System Settings.
 *
 * The value is an oklch string from the `get_system_accent_color` command;
 * `index.css` consumes it via `--accent-color: var(--accent-system-value, …)`
 * with a baked-in blue fallback, so non-macOS hosts (or a failed invoke) simply
 * keep the default.
 *
 * macOS DOES push these changes — it posts distributed notifications any process
 * can observe, which is why every other app recolours the instant you move the
 * slider. This hook used to say otherwise and only re-fetch on focus, so the app
 * visibly lagged the rest of the system until you clicked into it. It now
 * subscribes (see src-tauri/src/system_watch.rs) and keeps the focus/visibility
 * refetch as a belt-and-braces path for anything the OS does not announce.
 *
 * It also carries the accessibility TEXT SIZE, because macOS's Text Size setting
 * reaches neither AppKit metrics nor WebKit's `-apple-system-*` fonts — measured
 * with scripts/font-probe.swift at category XL, where every text style still
 * reported its default size. An app that wants to honour it has to read the
 * category and apply it, which is what `--system-text-scale` does.
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
        // Transient IPC failure ≠ "host has no accent": keep the current value
        // (and the warm-start cache) rather than flashing the fallback blue.
        // Only a successful `null` result — a host without a readable accent —
        // clears it.
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

    // The type ramp follows the system text size, and unlike the in-app A+/-
    // control this one scales the FRAME too: someone who enlarged system text
    // needs the toolbar and sidebar legible as well, which is exactly the gap
    // freezing the frame against A+/- left open.
    const applyTextScale = async () => {
      try {
        const scale = await client.systemTextScale();
        if (cancelled) return;
        const root = document.documentElement;
        if (scale && Math.abs(scale - 1) > 0.001)
          root.style.setProperty("--system-text-scale", String(scale));
        else root.style.removeProperty("--system-text-scale");
      } catch {
        // Leave whatever is applied; a failed read is not "no preference".
      }
    };
    void applyTextScale();

    // Stamp macOS's own "Keyboard navigation" setting on <html>. Nothing reads
    // it yet — this is the hook the keyboard-focus work needs, and having the
    // value present makes the current behaviour measurable: with the setting
    // OFF, a native app's Tab visits text fields and lists only, while ours
    // visits every button and row.
    const applyKeyboardAccess = async () => {
      try {
        const on = await client.fullKeyboardAccess();
        if (!cancelled)
          document.documentElement.setAttribute(
            "data-full-keyboard-access",
            on ? "on" : "off",
          );
      } catch {
        // Non-Tauri host: leave the attribute absent.
      }
    };
    void applyKeyboardAccess();

    let unlisten: (() => void) | undefined;
    void client
      .onSystemChange((c) => {
        if (c.kind === "accent" || c.kind === "appearance") void fetchAndApply();
        if (c.kind === "textSize") void applyTextScale();
        if (c.kind === "keyboardAccess") void applyKeyboardAccess();
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // No subscription (non-Tauri host): the focus refetch below still runs.
      });

    const onFocusOrVisible = () => {
      if (document.visibilityState === "hidden") return;
      void fetchAndApply();
      void applyTextScale();
      void applyKeyboardAccess();
    };

    window.addEventListener("focus", onFocusOrVisible);
    document.addEventListener("visibilitychange", onFocusOrVisible);
    return () => {
      cancelled = true;
      unlisten?.();
      window.removeEventListener("focus", onFocusOrVisible);
      document.removeEventListener("visibilitychange", onFocusOrVisible);
    };
  }, []);
}
