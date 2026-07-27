/**
 * Keyboard navigation that follows macOS instead of inventing its own rules.
 *
 * macOS has a system setting for this — System Settings → Keyboard → "Keyboard
 * navigation" (`AppleKeyboardUIMode`). With it OFF, Tab visits only text fields
 * and lists; buttons, checkboxes and rows are skipped. With it ON, Tab visits
 * everything. Every native app behaves that way, and the setting is how a user
 * says which they want.
 *
 * We ignored it, so Tab had 46 stops in Messages, 39 in Contacts and 58 in
 * Safety Scan — every button and every row — on a machine where the setting was
 * off. That is the whole reason keyboard focus felt noisy: the app was acting as
 * if full keyboard access had been asked for when it had not.
 *
 * The value arrives from `system_watch.rs` as `data-full-keyboard-access` on
 * <html> and updates live when the setting changes, so this hook watches the
 * attribute rather than re-invoking.
 */
import { useCallback, useEffect, useRef, useState } from "react";

/** Whether macOS Full Keyboard Access is on. */
export function useFullKeyboardAccess(): boolean {
  const read = () =>
    document.documentElement.getAttribute("data-full-keyboard-access") === "on";
  const [on, setOn] = useState(read);
  useEffect(() => {
    setOn(read());
    // The attribute is (re)written when the OS announces a change, so observing
    // it keeps this in step without a second IPC path.
    const observer = new MutationObserver(() => setOn(read()));
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-full-keyboard-access"],
    });
    return () => observer.disconnect();
  }, []);
  return on;
}

/**
 * `tabIndex` for a control that is NOT a text field or a list.
 *
 * Returns -1 when Full Keyboard Access is off, which takes buttons out of the
 * tab order exactly as macOS does — the control stays clickable and stays
 * reachable through the list it belongs to.
 *
 * Dialogs are deliberately exempt (see `Button`): a modal must always be
 * completable from the keyboard, and unlike a toolbar there is nowhere else to
 * reach its buttons from.
 */
export function useControlTabIndex(): 0 | -1 {
  return useFullKeyboardAccess() ? 0 : -1;
}

/**
 * Arrow-key navigation for a selection list — the half that makes honouring the
 * system setting an improvement rather than a removal.
 *
 * With Full Keyboard Access off, macOS still lets Tab reach a LIST; you then
 * move within it using the arrows, and that is what this provides. The list is
 * one tab stop, ↑/↓ move the selection, Home/End jump to the ends. Selection
 * moves rather than focus, so it works with virtualised rows that are not
 * mounted — which focus-based roving `tabindex` cannot do.
 */
export function useListNavigation<T>({
  items,
  selectedId,
  onSelect,
  getId,
}: {
  items: readonly T[];
  selectedId: number | string | null | undefined;
  onSelect: (id: never) => void;
  getId: (item: T) => number | string;
}) {
  const ref = useRef<HTMLDivElement>(null);

  const move = useCallback(
    (delta: number | "first" | "last") => {
      if (items.length === 0) return;
      const current = items.findIndex((i) => getId(i) === selectedId);
      const next =
        delta === "first"
          ? 0
          : delta === "last"
            ? items.length - 1
            : // From nothing selected, ↓ starts at the top and ↑ at the bottom.
              current < 0
              ? delta > 0
                ? 0
                : items.length - 1
              : Math.min(items.length - 1, Math.max(0, current + delta));
      const id = getId(items[next]);
      if (id !== selectedId) onSelect(id as never);
      // Keep the moved-to row on screen. The rows are virtualised, so the
      // element may not exist yet — the query runs after the selection paints.
      requestAnimationFrame(() => {
        ref.current
          ?.querySelector('[aria-current="true"]')
          ?.scrollIntoView({ block: "nearest" });
      });
    },
    [items, selectedId, onSelect, getId],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Let a control inside the list keep its own key handling.
      if (e.target !== e.currentTarget && (e.target as HTMLElement).closest("input, textarea"))
        return;
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          move(1);
          break;
        case "ArrowUp":
          e.preventDefault();
          move(-1);
          break;
        case "Home":
          e.preventDefault();
          move("first");
          break;
        case "End":
          e.preventDefault();
          move("last");
          break;
      }
    },
    [move],
  );

  return {
    /** Spread onto the scrolling list container. */
    listProps: {
      ref,
      tabIndex: 0,
      role: "listbox" as const,
      "aria-label": undefined as string | undefined,
      onKeyDown,
    },
  };
}
