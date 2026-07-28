import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Toggle as TogglePrimitive } from "radix-ui"

import { cn } from "@/lib/utils"
import { useControlTabIndex } from "@/lib/use-keyboard-nav"

const toggleVariants = cva(
  "inline-flex items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap transition-[color,box-shadow] outline-none hover:bg-muted hover:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 data-[state=on]:bg-accent data-[state=on]:text-accent-foreground dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-transparent",
        outline:
          "border border-input bg-transparent shadow-xs hover:bg-accent hover:text-accent-foreground",
      },
      size: {
        default: "h-(--control-h) min-w-(--control-h) px-2",
        sm: "h-(--control-h-sm) min-w-(--control-h-sm) px-1.5",
        // Toolbar segments are ISLAND-tall, not control-tall. A toggle group in
        // a toolbar sits beside FilterControl and SortControl, which are
        // islands — and at size="sm" it rendered 24px against their 30px in
        // Notes, Messages and Safety alike. Nobody noticed because the design
        // lint's island rule only measured `div.rounded-lg.bg-muted`, and a
        // ToggleGroup is not one.
        island: "h-(--island-h) min-w-(--island-h) px-1.5",
        lg: "h-(--control-h-lg) min-w-(--control-h-lg) px-2.5",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Toggle({
  className,
  variant,
  size,
  ...props
}: React.ComponentProps<typeof TogglePrimitive.Root> &
  VariantProps<typeof toggleVariants>) {
  return (
    <TogglePrimitive.Root
      data-slot="toggle"
      tabIndex={useControlTabIndex()}
      className={cn(toggleVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Toggle, toggleVariants }
