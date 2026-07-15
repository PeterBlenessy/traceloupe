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
  themes for free. The `ModeToggle` in the top bar is a single button that
  **cycles** System → Light → Dark on click (no menu; `SunMoon`/`Sun`/`Moon`
  icon reflects the current theme); the same choice is also in Settings →
  General → Appearance.
- **Icons:** `lucide-react`, sized with `size-4` / `size-5`.
- **Density:** a user setting (`SettingsProvider.density`, Comfortable/Cozy/
  Compact) stamped as `data-density` on `<html>`; CSS in `index.css` tightens
  list-row `padding-block` and chat-bubble line-height per level. It is **True
  Density** — fonts, icons and controls keep their size; only spacing tightens.
  A rows-icon toggle in the top bar (`DensityToggle`) cycles the levels. Custom
  (non-`Item`) rows opt in with `data-slot="list-row"`.
- **Persisted view state:** per-view UI choices (active tab, filter, sort) use
  `usePersistedState(key, initial)` (`src/lib/use-persisted-state.ts`) — a
  `useState` backed by localStorage under `traceloupe-ui:<key>` — so they
  survive navigation and restarts. Only JSON-serializable state; guard a stale
  persisted value that no longer exists in the current backup (fall back to
  "all"/first).
- **Spacing/sizing:** Tailwind scale utilities. Arbitrary values (`w-[70%]`)
  are allowed only where no scale step fits (e.g. chat-bubble max width).

## App frame

`AppShell` uses the shadcn **Sidebar** block (`SidebarProvider` + `Sidebar` +
`SidebarInset`). Nav items are `SidebarMenuButton` with `isActive` bound to the
route. The top bar carries the `SidebarTrigger` and `ModeToggle`; each view
renders its own header below it.

## View primitives (`src/components/view.tsx`)

Build every artifact view from these, not from raw flex/grid scaffolding:

- **`PanelHeader`** — the canonical header shared by **every** list view: a
  title row (`ViewHeader`: title + count + inline filter `actions`), an optional
  full-width `search` row, and an optional `toolbar` row (time filter + sort).
  Use this — don't hand-roll header rows — so all headers stay identical.
- **`ViewHeader`** — the title/count/actions strip; used inside `PanelHeader`
  and directly for a **detail pane's** own header (a selected note/recording/
  conversation), which is not a list and has no search/toolbar.
- **`VirtualListView`** / **`LazyListView`** — a single scrolling column whose
  rows are virtualized. `VirtualListView` takes an in-memory array; `LazyListView`
  fetches windows (`count` + `fetchWindow`) for lists of tens of thousands of
  rows. Both render `PanelHeader` internally (`header`→actions, `search`,
  `toolbar` slots). Used by Photos, Safari, Calls, Apps.
- **`ListDetail`** — master list + detail pane (Contacts, Recordings, Notes,
  Messages). **Layout:** the `PanelHeader` spans the **full width across the top**
  and the `ListDetail` (list | detail) sits **below** it — never a header trapped
  inside the narrow master column:
  `<div className="flex h-full flex-col"><PanelHeader …/><div className="min-h-0 flex-1"><ListDetail …/></div></div>`.
- **`ListSearch`** — the standard search input (goes in `PanelHeader`'s `search`).
- **`EmptyView`** — the standard empty / no-selection state (wraps shadcn
  `Empty`). Used for "no backup open", "nothing selected", etc.

**Filters & sort** (go in `PanelHeader`'s `actions`/`toolbar`):

- **`BadgeFilter`** (`components/badge-filter.tsx`) — the one control for every
  single-select list filter (service, source, Safari type, note lock, message
  content-kind). Clickable `Badge` pills: selected = filled, others muted, with
  optional icon + count. **Never wraps** — scrolls horizontally when narrow
  (`flex-nowrap` + `overflow-x-auto`), so filters can't push the header taller.
  Don't build filter chips out of raw `ToggleGroup` anymore.
- **`TimeFilterBar`** (`components/time-filter.tsx`) — preset period chips
  (All/24h/7d/30d/year) + custom range, over any date field. Same no-wrap
  horizontal-scroll behavior as `BadgeFilter`. Add it to any dated view (Photos,
  Safari, Calls, Recordings, Notes, Messages timeline).
- **`SortControl`** (`components/sort-control.tsx`) — field + direction. When a
  view has a single sort field (time), use a plain direction toggle instead (see
  Messages' `OrderToggle`) rather than a one-item picker.

List rows use shadcn **`Item`** (`ItemMedia`/`ItemContent`/`ItemTitle`/…) — which
carries the `data-slot="item"`/`data-size` the density CSS targets; people use
**`Avatar`** with initials from `lib/contact.ts`. Timestamps go through
`lib/format.ts`. Selected/hovered rows show an inset, rounded, full-width
highlight (`w-full` button inside a `px-2` gutter), matching the sidebar.

## What may stay custom

A few things have no shadcn primitive and are legitimately bespoke — keep them
as single, documented components, not inlined markup:

- **Message bubbles** (`views/messages.tsx`) — chat bubbles are app-specific.
- **Messages is the one view not fully on `PanelHeader`.** It's a chat view: the
  top mode + service/content-filter bar is full-width, but the **conversation
  detail pane keeps its own header** (the selected conversation's name + per-chat
  controls), like every messaging app. That is intentional, not drift. Its
  Timeline mode does follow the single-column `PanelHeader` pattern.
- **Settings dialog** (`app-shell.tsx`) — a fixed-height dialog with a
  macOS-System-Settings-style **vertical** tab rail (a `bg-muted/30` sidebar,
  active row filled) beside a scrolling content pane.

When you reach for custom markup, first check whether a `view.tsx` primitive or
a shadcn component already covers it; if it recurs, promote it into `view.tsx`.
