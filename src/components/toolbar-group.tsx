import { cn } from "@/lib/utils";

/**
 * A macOS-Notes-style segmented control cluster: a subtly bordered, rounded
 * container that groups related icon buttons into one unit. Buttons placed
 * inside should be borderless/ghost and `size="icon-sm"` so they read as segments
 * of the group rather than separate controls — at one notch below the default
 * control height, the island as a whole stands exactly as tall as a normal
 * button beside it, which is what makes it read as a single unit.
 *
 * The rhythm inside has TWO levels, and they carry meaning. Distinct controls
 * (text size, density, theme) get a normal gap. Only the two halves of ONE
 * control sit flush — A−/A+ are a single stepper, so they render inside their own
 * gapless span. Applying the tight spacing to everything, as this did at
 * `gap-0.5`, made three unrelated controls read as one five-part widget. Use it for toolbar chrome (the top-bar
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
        "inline-flex h-(--island-h) items-center gap-2 rounded-lg border border-border/70 bg-muted/40 px-0.5",
        className,
      )}
    >
      {children}
    </div>
  );
}
