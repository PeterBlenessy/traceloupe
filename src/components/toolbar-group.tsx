import { cn } from "@/lib/utils";

/**
 * A macOS-Notes-style segmented control cluster: a subtly bordered, rounded
 * container that groups related icon buttons into one unit. Buttons placed
 * inside should be borderless/ghost and `size="icon-sm"` so they read as segments
 * of the group rather than separate controls — at one notch below the default
 * control height, the island as a whole stands exactly as tall as a normal
 * button beside it, which is what makes it read as a single unit. Use it for toolbar chrome (the top-bar
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
