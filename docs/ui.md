# UI design base

Conventions for the frontend, so the artifact views stay consistent as they
grow. The rule of thumb: **compose shadcn/ui primitives and the shared
`components/view.tsx` helpers; don't hand-roll structure or inline bespoke CSS.**

> **Read the "View toolbar" section before building or changing any view.** Every
> view surfaces its title/filter/sort/search through ONE shared toolbar — there
> are no per-view header bars. Re-implementing a header, a filter popover, a time
> picker, or a pill row is the most common mistake here; all of it already exists.

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
`SidebarInset`). The top bar is a **single** shared `AdaptiveToolbar`
(`components/adaptive-toolbar.tsx`) with three regions:

- **`leading`** — sidebar toggle + the current view's **title & count**.
- **`middle`** — the current view's **mode toggle · Filter · Sort · Search**.
- **`trailing`** — app-wide controls (import/model indicators, density, theme).

The view itself renders **none** of this. There is no per-view header row.

## The view toolbar — how every view surfaces its controls

A view publishes its title, count and controls to the one shared toolbar with
**`useViewToolbar(config)`** (`components/toolbar-context.tsx`); the app shell
renders them (`app-shell.tsx` → `AdaptiveToolbar`). **Every** view works this way
— there is no per-view header/control bar, and you must not add one.

`ViewToolbar` slots:

- `title?: string`, `count?: number` — left, next to the sidebar toggle.
- `modes?: ReactNode` — an always-visible mode toggle (Notes' List/Folders,
  Messages' Chats/Timeline). A shadcn `ToggleGroup`.
- `filter?: FilterGroup[]` — faceted filters for the morphing **Filter** popover
  (see below). Omit / empty ⇒ no Filter button.
- `sort?: ReactNode` — the sort control (see below).
- `search?: ReactNode` — the animated search box (`ListSearch`).

Rules:

- Call `useViewToolbar` **exactly once** per view render (it does a single
  `setToolbar`). **Memoize** the config object and its node/array members so it
  doesn't republish every render.
- **Gate on the backup:** pass `null` when there's no active backup, so the
  `NoBackupState` shows with just the app controls. Clears on unmount.
- **Two-mode views** (Messages): each mode component calls `useViewToolbar`
  *itself* with the full config; the parent passes the shared bits (title, mode
  toggle, shared filter groups) down as props. Only one mode renders at a time,
  so only one `useViewToolbar` is ever live. Don't merge two calls in the parent.

```tsx
const filterGroups = useMemo<FilterGroup[]>(() => [
  badgeGroup({ key: "source", label: "Source", description: "…", options, value, onChange }),
  timeGroup({ description: "When it happened", presets, counts, value: range, onChange: setRange }),
], [options, value, presets, counts, range]);
const sortNode = useMemo(() => <SortControl fields={…} value={sort} onChange={setSort} />, [sort]);
const searchNode = useMemo(() => <ListSearch value={q} onChange={setQ} placeholder="Search…" />, [q]);
useViewToolbar(useMemo(() => active ? {
  title: "Calls", count, filter: filterGroups, sort: sortNode, search: searchNode,
} : null, [active, count, filterGroups, sortNode, searchNode]));
```

## Filters — the morphing Filter popover

The **Filter** button (a funnel) morphs into a popover of grouped facets
(`components/filter-control.tsx`, `FilterControl`). You never place `FilterControl`
yourself — publish `filter: FilterGroup[]` and the shell renders it. Build groups
with the helpers in `components/filter-groups.tsx`:

- **`badgeGroup({…})`** — a single-select facet (source, folder, Safari type,
  message app/kind). `options[0]` is the "all"/default; picking it clears the group.
- **`multiBadgeGroup({…})`** — multi-select (e.g. tags): empty selection = all;
  each selected value is its own removable chip.
- **`timeGroup({ presets, counts, value, onChange, description })`** — the time
  facet: a pill per preset plus the custom **Range** picker. This IS the time
  filter — not the older `TimeFilterBar` (legacy, superseded, do not use).

**Design choice — show all, disable empty.** Every option is always shown; an
option with a zero count is **disabled** (greyed), never hidden. Pass per-option
`counts` to get this. Hiding options because they're empty reads as a bug — don't.

Active selections surface as removable chips on the funnel's island when closed;
"Clear all" resets them. The popover animates width/height (WebKit-safe). By
default it anchors the funnel's right edge and morphs **leftward** (for the
right-aligned toolbar). If you reuse `FilterControl` inside content with the
funnel on the left (e.g. Safety Scan's run card), pass **`align="right"`** so it
morphs rightward into the content instead of over the sidebar.

**Time presets** live in `components/time-filter.tsx`: `useTimePresets()`
(All/24h/7d/30d/year, anchored to a stable `now`) and `makeYearPresets(min, max)`
(a chip per calendar year the data spans — replace the single "year" preset with
these for multi-year data; see the Messages timeline and Safety Scan). Counts for
message-dated views come from `client.countMessageRanges(...)`.

## Sort & search

- **`SortControl`** (`components/sort-control.tsx`) — field + direction, in the
  `sort` slot. For a single sort field (time), use a plain direction toggle
  instead of a one-item picker (see Messages' `OrderToggle`).
- **`ListSearch`** (`components/view.tsx`) — the standard search input, in the
  `search` slot (it animates open in the toolbar).

## View content (`src/components/view.tsx`)

The toolbar is global; a view's own return is just its content. Build it from
these — not raw flex/grid scaffolding:

- **`VirtualListView`** / **`LazyListView`** — a single virtualized scrolling
  column. `VirtualListView` takes an in-memory array; `LazyListView` fetches
  windows (`count` + `fetchWindow`) for tens of thousands of rows. Photos, Safari,
  Calls, Apps.
- **`ListDetail`** — master list + detail pane (Contacts, Recordings, Notes,
  Messages Chats). The detail pane keeps its **own** header for the selected item.
- **`ViewHeader`** — a title strip for a **detail pane only** (a selected note /
  recording / conversation). **NOT** for a view's top-level header — that is the
  toolbar's job (`useViewToolbar`).
- **`NoBackupState`** — the rich "open a backup" onboarding every content view
  shows before a backup is loaded (feature icon, action title, capability grid,
  privacy note, "Choose a backup" CTA). Return it — and publish `null` to the
  toolbar — when there's no active backup.
- **`EmptyView`** — the plain empty / no-selection state (wraps shadcn `Empty`):
  "no results", "nothing selected", "no X in this backup".

List rows use shadcn **`Item`** (`ItemMedia`/`ItemContent`/`ItemTitle`/…), avatars
from `lib/contact.ts`, timestamps via `lib/format.ts`. Selected/hovered rows show
an inset, rounded, full-width highlight, matching the sidebar.

## What may stay custom

A few things have no primitive and are legitimately bespoke — keep them as single,
documented components, not inlined markup:

- **Message bubbles** (`views/messages.tsx`) — chat bubbles are app-specific.
- **Scan / report views** (Safety Scan, Security Check) — action + report views,
  not browsable lists. They publish just their **title** to the toolbar; their
  controls stay in the content on purpose (the scan time range and Run buttons are
  **inputs to the action**, not filters over displayed content). The time filter
  still reuses `FilterControl` + `timeGroup` — just in the run card, right-aligned.
- **Settings dialog** (`app-shell.tsx`) — a fixed-height dialog with a
  macOS-System-Settings-style vertical tab rail; rows via `SettingsGroup` /
  `SettingsRow`.

## Before you build a view or a control

1. **It goes on the toolbar.** Publish title/filter/sort/search/modes via
   `useViewToolbar`. Do not add an in-view header or control bar.
2. **Reuse, don't re-implement.** About to write a pill row, a popover, a time
   picker, a search box, or a header strip? Stop — it exists (above). Grep
   `src/components/` and skim this doc first.
3. **Promote, don't inline.** If a genuinely new shared control is needed, add it
   under `components/` (or a shadcn primitive in `components/ui/`) and document it
   here — never inline it in one view.
4. **Build on what's in flight.** These shared components arrived via a large
   migration. Before starting UI work on a branch, `git fetch` and skim
   `origin/main` and open PRs (`gh pr list`) for related UI changes, so you adopt
   the current pattern and migrate alongside it — not around it. (The scan views
   drifted precisely because they were built while the toolbar migration was still
   on a separate branch.)
