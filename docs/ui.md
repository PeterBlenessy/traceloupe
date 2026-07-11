# UI design base

Conventions for the frontend, so the artifact views stay consistent as they
grow. The rule of thumb: **compose shadcn/ui primitives and the shared
`components/view.tsx` helpers; don't hand-roll structure or inline bespoke CSS.**

## Foundations

- **Components:** shadcn/ui, "own-the-code" under `src/components/ui/`. Add new
  ones with `pnpm dlx shadcn@latest add <name>` rather than writing custom
  equivalents. Prefer an existing primitive (Item, Empty, Avatar, Card,
  Dialog…) over new markup.
- **Colors & theming:** one token layer in `src/index.css`, all on the neutral
  **oklch** scale (no mixed color systems). Use semantic tokens
  (`bg-background`, `text-muted-foreground`, `bg-accent`, `border`) — never raw
  hex or `oklch(...)` literals in components. Sidebar tokens match the same
  scale.
- **Light/dark:** `ThemeProvider` toggles a `light`/`dark` class on `<html>`;
  the token layer does the rest. Every component must read from tokens so it
  themes for free. The `ModeToggle` (light/dark/system) lives in the top bar.
- **Icons:** `lucide-react`, sized with `size-4` / `size-5`.
- **Spacing/sizing:** Tailwind scale utilities. Arbitrary values (`w-[70%]`)
  are allowed only where no scale step fits (e.g. chat-bubble max width).

## App frame

`AppShell` uses the shadcn **Sidebar** block (`SidebarProvider` + `Sidebar` +
`SidebarInset`). Nav items are `SidebarMenuButton` with `isActive` bound to the
route. The top bar carries the `SidebarTrigger` and `ModeToggle`; each view
renders its own header below it.

## View primitives (`src/components/view.tsx`)

Build every artifact view from these, not from raw flex/grid scaffolding:

- **`ViewHeader`** — the title/count/actions strip at the top of a view.
- **`ListView`** — a single scrolling column (Calls, Safari).
- **`ListDetail`** — master list + detail pane (Messages, Contacts).
- **`ListSearch`** — a standard filter input.
- **`EmptyView`** — the standard empty / no-selection state (wraps shadcn
  `Empty`). Used for "no backup open", "nothing selected", etc.

List rows use shadcn **`Item`** (`ItemMedia`/`ItemContent`/`ItemTitle`/…);
people use **`Avatar`** with initials from `lib/contact.ts`. Timestamps go
through `lib/format.ts`.

## What may stay custom

A few things have no shadcn primitive and are legitimately bespoke — keep them
as single, documented components, not inlined markup:

- **Message bubbles** (`views/messages.tsx`) — chat bubbles are app-specific.

When you reach for custom markup, first check whether a `view.tsx` primitive or
a shadcn component already covers it; if it recurs, promote it into `view.tsx`.
