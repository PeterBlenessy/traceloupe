import * as React from "react"
import { createContext, useContext } from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"
import { useFullKeyboardAccess } from "@/lib/use-keyboard-nav"

/** Set inside a modal, where buttons keep their place in the tab order. */
export const DialogKeyboardContext = createContext(false)

const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-(--control-radius) text-sm font-medium whitespace-nowrap transition-all outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        destructive:
          "bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:bg-destructive/60 dark:focus-visible:ring-destructive/40",
        outline:
          "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:border-input dark:bg-input/30 dark:hover:bg-input/50",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost:
          "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50",
        link: "text-primary underline-offset-4 hover:underline",
      },
      // Heights come from the shared --control-h* scale (see index.css) so a
      // button always matches the input or select beside it, and so every
      // control tracks the text-size knob together.
      size: {
        default: "h-(--control-h) px-3 has-[>svg]:px-2.5",
        xs: "h-(--control-h-xs) gap-1 px-2 text-xs has-[>svg]:px-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-(--control-h-sm) gap-1.5 px-2.5 has-[>svg]:px-2",
        lg: "h-(--control-h-lg) px-4",
        icon: "size-(--control-h)",
        "icon-xs": "size-(--control-h-xs) [&_svg:not([class*='size-'])]:size-3",
        "icon-sm": "size-(--control-h-sm)",
        "icon-lg": "size-(--control-h-lg)",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"
  // macOS skips buttons when Keyboard navigation is off (System Settings →
  // Keyboard); a native app's Tab goes to text fields and lists only. Following
  // that took Tab in Messages from 46 stops to a handful. Buttons stay clickable
  // and stay reachable once the user turns the setting on.
  //
  // Dialogs are exempt: a modal has to be completable from the keyboard, and
  // unlike a toolbar there is nowhere else its buttons can be reached from.
  const fullKeyboard = useFullKeyboardAccess()
  const inDialog = useContext(DialogKeyboardContext)
  //
  // A tabIndex of 0 arriving in props is NOT an app decision — Radix's
  // TooltipTrigger injects it when it wraps a button, and 15 of Messages' 24
  // buttons are tooltip-wrapped. Treating that as explicit left them in the tab
  // order and the setting appeared to do nothing. Only a non-zero tabIndex is
  // treated as deliberate.
  const explicit = props.tabIndex && props.tabIndex !== 0 ? props.tabIndex : undefined
  const tabIndex = explicit ?? (fullKeyboard || inDialog ? undefined : -1)

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
      tabIndex={tabIndex}
    />
  )
}

export { Button, buttonVariants }
