import { cn } from "@/lib/utils";

/**
 * A macOS-Notes-style segmented control cluster: a subtly bordered, rounded
 * container that groups related icon buttons into one unit. Buttons placed
 * inside should be borderless/ghost and `size-7` so they read as segments of the
 * group rather than separate controls. Use it for toolbar chrome (the top-bar
 * app controls, per-view header actions) to give them the grouped, bordered look
 * Apple uses instead of a row of floating bare icons.
 */
export function ToolbarGroup({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-0.5 rounded-lg border border-border/70 bg-muted/40 p-0.5",
        className,
      )}
    >
      {children}
    </div>
  );
}
