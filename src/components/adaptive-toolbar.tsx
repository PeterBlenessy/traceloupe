import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * The app's single toolbar row. Fixed `leading` (sidebar toggle + view title) on
 * the left, a right-aligned `middle` cluster (the view's Filter · Sort · Search
 * controls, published via the toolbar context), and fixed `trailing` app-wide
 * controls. Empty areas carry `data-tauri-drag-region` so the merged titlebar
 * drags the window; interactive children click normally.
 */
export function AdaptiveToolbar({
  leading,
  middle,
  trailing,
  className,
}: {
  /** Fixed content pinned to the left (sidebar toggle + view title). */
  leading?: ReactNode;
  /** The view's right-aligned controls cluster (Filter · Sort · Search). */
  middle?: ReactNode;
  /** Fixed content pinned to the right (app-wide controls). */
  trailing?: ReactNode;
  className?: string;
}) {
  return (
    <div
      data-tauri-drag-region
      className={cn("relative flex min-w-0 flex-1 items-center gap-2", className)}
    >
      {leading && (
        <div data-tauri-drag-region className="flex shrink-0 items-center gap-2">
          {leading}
        </div>
      )}
      <div
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center justify-end gap-2"
      >
        {middle}
      </div>
      {trailing && (
        <div className="flex shrink-0 items-center gap-2">{trailing}</div>
      )}
    </div>
  );
}
