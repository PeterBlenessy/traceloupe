import { useCallback, useRef, useState } from "react";
import { cn } from "@/lib/utils";

/**
 * A pointer-draggable width, persisted to localStorage. `side` says which edge
 * the handle sits on: "right" for a left-docked panel (drag right = wider),
 * "left" for a right-docked one.
 */
export function useResizableWidth(
  key: string,
  initial: number,
  min: number,
  max: number,
) {
  const clamp = useCallback((n: number) => Math.min(max, Math.max(min, n)), [min, max]);
  const [width, setWidth] = useState(() => {
    const saved = Number(localStorage.getItem(key));
    return Number.isFinite(saved) && saved > 0 ? clamp(saved) : initial;
  });
  const widthRef = useRef(width);
  widthRef.current = width;

  const startResize = useCallback(
    (e: React.PointerEvent, side: "left" | "right" = "right") => {
      e.preventDefault();
      const startX = e.clientX;
      const startW = widthRef.current;
      const sign = side === "right" ? 1 : -1;
      // Suppress text selection / iframe pointer capture while dragging.
      document.body.style.userSelect = "none";
      const onMove = (ev: PointerEvent) => setWidth(clamp(startW + sign * (ev.clientX - startX)));
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        document.body.style.userSelect = "";
        localStorage.setItem(key, String(Math.round(widthRef.current)));
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [key, clamp],
  );

  return { width, startResize };
}

/** A thin vertical drag handle for resizing an adjacent panel. */
export function ResizeHandle({
  onPointerDown,
  className,
}: {
  onPointerDown: (e: React.PointerEvent) => void;
  className?: string;
}) {
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      onPointerDown={onPointerDown}
      className={cn(
        "w-1 shrink-0 cursor-col-resize bg-border transition-colors hover:bg-primary/40 active:bg-primary/60",
        className,
      )}
    />
  );
}
