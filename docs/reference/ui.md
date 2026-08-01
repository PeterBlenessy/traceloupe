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
- **Colors & theming:** one token layer in `src/index.css`. **Surfaces are ours**
  — tinted oklch neutrals, the app's identity. **Text is the platform's**: the
  three label tiers (`--foreground`, `--muted-foreground`, `--faint-foreground`)
  resolve to `-apple-system-label` and friends inside an
  `@supports (color: -apple-system-label)` block, with our oklch values as the
  fallback. macOS ships them as *alpha* over black/white, so one value is right
  on a card, the sidebar and a popover alike, and they follow the "Increase
  contrast" accessibility setting.
  **Status colours are the platform's too** — five roles (`danger` · `warning` ·
  `ok` · `info` · `note`) mapped to `-apple-system-red/orange/green/blue/purple`,
  replacing 56 hardcoded palette classes. Each has three tiers:
  `--status-X` (icons, fills), `--status-X-text` (lightness-clamped for
  contrast — systemOrange on white is ~2:1, unreadable), and
  `--status-X-soft`/`-line` (15 % / 30 % `color-mix` tints). Note what
  disappears: `text-emerald-600 dark:text-emerald-400` becomes
  `text-status-ok-text`, because a system colour already flips with the
  appearance. `--destructive` is the danger role — one red in the app.
  **The `@supports` guard is load-bearing:** WKWebView (what we ship) supports
  those keywords, Chromium (what `scripts/shot.mjs` renders with) does not — and
  a custom property holding an unsupported keyword is invalid *at use time*, so
  without the guard Chromium would drop to plain black/white and the screenshot
  harness would stop representing the app. Verified with `CSS.supports()` in
  both engines. `color-scheme` on `:root`/`.dark` is what ties the platform's
  appearance to *our* theme class. Use semantic tokens
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
- **Two steps below `text-xs`: `text-3xs` (10px) and `text-2xs` (11px).** They
  exist because the app kept reinventing them — 24 of 31 arbitrary type sizes
  were already exactly these. A timestamp, a duration badge and a NEW chip are a
  role; `text-[calc(0.625rem*var(--text-scale))]` written 17 times is that role
  without a name. An arbitrary size is still right when it is *fitted to
  something* rather than chosen typographically (initials inside a half-size
  avatar, a monogram at `0.65em`, the A−/A+ glyphs) — those say so at the site.
- **The type ramp IS the platform's.** `--ramp-*` carries macOS's text-style
  sizes — 10/11/12/13/15/17/22/26 (caption · subheadline · callout · body ·
  title3 · title2 · title1 · largeTitle) — not Tailwind's web ramp of
  12/14/16/18/20/24. They're numbers rather than the `-apple-system-*` keywords
  because a keyword sets an *absolute* size and would break A+/−; this way the
  sizes are native at scale 1 and still scale. `body` defaults to the body step,
  so text without an explicit class can't silently inherit the browser's 16px. `scripts/font-probe.swift` prints
  both sides (AppKit metrics, AppKit text styles, and what a WKWebView resolves
  `font: -apple-system-body` and friends to, since that needs no native bridge).
  Run it before changing anything about type, and run it again after changing
  System Settings → Accessibility → Display → Text Size to see whether the
  platform's sizes follow that setting.
- **Two scales, and they are not the same thing.** `--system-text-scale` carries
  the macOS accessibility Text Size and multiplies the ramp **everywhere,
  including the frame** — someone who enlarged system text needs the toolbar and
  sidebar legible too. `--text-scale` is the in-app A+/− reading preference and
  scales **content only**. macOS's setting reaches neither AppKit metrics nor
  WebKit's `-apple-system-*` fonts (measured at category XL, where every text
  style still reported its default size), so `system_watch.rs` reads the category
  from `com.apple.universalaccess` and maps it to a multiplier. Note the stored
  value is the SHORT name (`XL`), not `UICTContentSizeCategoryXL` — matching only
  the long form returns 1.0 on a machine that is actually set to XL, which looks
  exactly like the feature not working.
- **The text-size control (A+/−) is a READING preference, so the app frame opts
  out.** The top toolbar, the sidebar and a dialog's *action row* carry
  `data-text-scale="fixed"` and keep their size at every step; content, list
  rows, pills, content-adjacent buttons and a dialog's title/body all scale. The
  reason is not tidiness: a modal action row is a decision point, often
  destructive, and the button under the cursor must not depend on a font
  setting — and a toolbar that reflows the window on a text change reads as
  unstable. If the frame ever needs its own legibility control, that is a
  separate setting (or the macOS text-size setting), never this one.
- **Control height — one scale, never a literal.** Every inline control
  (button, input, select, toggle, tab trigger) takes its height from the
  `--control-h*` tokens in `index.css`: `--control-h` is the default at **28 px
  to match a native macOS push button**, with `-xs` / `-sm` / `-lg` at 20 / 24 /
  32 px. Write `h-(--control-h)`, never `h-9` or `size-8` — a literal is how the
  app ends up with a 36 px button next to a 32 px field, which is exactly the
  drift [#91](https://github.com/PeterBlenessy/traceloupe/issues/91) had to undo
  (34 of 60 buttons had been hand-shrunk with `size="sm"` to fight a too-tall
  default). Heights ride `--text-scale` so a control grows with the text it
  contains; **padding does not**, because horizontal room is furniture.
- **The visual rules are enforced, not remembered.** `scripts/check-design.mjs`
  runs in CI and measures five invariants across five views, in the states an
  idle screenshot never shows (hovered, filtered, search expanded, smallest and
  largest text size):

  | rule | what it catches | the bug it comes from |
  |---|---|---|
  | `type` | a font size that is not a ramp step | 31 hand-written sizes across 7 values |
  | `control` | a height that is not a `--control-h*` step | 34 of 60 buttons hand-tuned (#91) |
  | `island` | islands/segments off their one height | 30 / 36 / 38 px in one toolbar (#131) |
  | `overlap` | one interactive element covering another | hover actions over the count pill (#92) |
  | `clipping` | a label cut off by its own box | `DialogTitle`'s `leading-none` |
  | `contrast` | text under WCAG AA against what is *really* behind it | a generated app tile at 4.11:1 |
  | `focus` | a control with no ring when **tabbed** to | — |
  | `a11y` | a control with no accessible name | unnamed Settings switches |
  | `tooltip` | an icon-only button that explains nothing | — |
  | `spacing` | a gap or padding off the 2px grid | — |

  Two contrast exceptions are recorded *with their measurement* rather than
  hidden: the selected sidebar row (white on the accent, 2.34:1 — an explicit
  product decision, and what macOS does), and the secondary/tertiary label tiers
  (which macOS itself renders below AA). Each has a floor, so if one gets worse
  it fails again.

  It runs **two passes**. The *static* pass reads the source and rejects a size
  or colour literal written onto a control — that is the only way to cover a
  button in a view nobody visited or a state nobody opened, which is exactly
  where literals survive. The *runtime* pass then measures every view (plus the
  Settings dialog) in both colour schemes.

  It **self-tests its detectors on every run**: it injects one deliberate
  violation per rule and fails if any rule stays quiet, because a lint whose
  matcher drifts reports OK forever and reads as coverage. Sizes that are
  legitimately off-ramp (initials fitted to an avatar, the A−/A+ glyphs) are
  listed in the script with their reason — so a *new* off-ramp size still fails.
  Take heights from `--island-h` / `--control-h-sm`, never a literal.
- **Three more browser checks gate merges alongside it**, all sharing the one dev
  server in CI's `Browser checks` step:

  | check | what it catches | the bug it comes from |
  |---|---|---|
  | `check-encrypted-empty.mjs` | claiming a backup has no data when nobody has looked yet | #216 |
  | `check-clickable.mjs` | a control that looks clickable but a real click lands elsewhere | #224 |
  | `check-view-intro.mjs` | a view that cannot introduce itself with no backup open | #221 |
  | `check-artifact-surfaces.mjs` | a module whose `surface` no view hosts, so its rows render nowhere | #231 |
  | `check-artifact-overlap.mjs` | a module reading a store the native importer already parses | — |

  A host view must never know which artifact it is showing. Two things a module
  declares rather than the view inferring: **column kinds** (`timestamp`, `bytes`)
  so a value is formatted from the fact rather than from a range guess, and
  **`[highlight]`** — what may be shown on the host's own row before anything is
  expanded. The Apps view had that hard-coded to TCC's shape (a literal
  `"Decision"` column, the values `Allowed`/`Limited`, the phrase "none granted"),
  which only became visible when a second apps-surface module shipped and the row
  read "Data usage: none granted". A module with no `[highlight]` gets a record
  count and nothing invented for it.

  `check-view-intro.mjs` walks every `{ to, label }` pair in `nav.ts` — so a **new
  destination is covered the day it lands**, not the day someone remembers the
  list — and for each one requires a `NoBackupState` that is **on screen**, with a
  heading, a lead of at least 40 characters, and at least one named capability.

  Two things it does that a first draft of it did not, both found by review:
  it asserts **arrival** (`location.pathname` must equal the entry's `to`) rather
  than trusting the click, because a swallowed click left the *previous* view on
  screen and it got measured and reported as this one's pass — so every view but
  the first inherited its predecessor's intro. That is not hypothetical: the
  sidebar's Scans group hardcodes its two labels in `app-shell.tsx`, so renaming
  `scanNav`'s `label` makes the click miss forever. And it asserts **geometry**,
  because an intro can sit in the DOM off-screen and `querySelector` cannot tell
  the difference — the same class as #224. It asserts on the `data-slot`s that `NoBackupState`
  renders (`view-intro`, `view-intro-lead`, `view-intro-feature`) rather than on
  Tailwind classes, which change for visual reasons and would fail the check for
  the wrong cause. It deliberately checks *substance, not wording*: what a view
  should say is per-view work, but **that** it says something is enforceable.
  The Artifacts view shipped unintelligible to the person who asked for it — not
  because the copy was bad, but because nothing was checking any view could
  explain itself before a backup was open.
- **A control island stands level with a button.** `ToolbarGroup` and
  `SortControl` wrap segments in a bordered island with `p-0.5`; segments are
  therefore `size="icon-sm"` (24 px), which puts the island at **30 px** (24 +
  2 px padding + 1 px border, each side) against a 28 px button — a hair taller,
  the way a macOS segmented control is. Segments at the default 28 px push the
  island to 34 px, and it stops reading as one unit and starts reading as a
  second row of chrome.

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

## Every list is virtualized, bounded, or provably small

**Any list whose length is not bounded by a constant must use the shared
virtualization** — `VirtualList` / `VirtualListView` / `LazyListView`, never a
hand-rolled windowing scheme. This is not a performance nicety: in
[#61](https://github.com/PeterBlenessy/traceloupe/issues/61) one list rendering
~8000 rows drove the WebKit render process to 99 % CPU and 3.1 GB and froze the
whole laptop. That list looked harmless when it was written, because findings
were expected to be few.

So every list must fall into one of three buckets, and which one is a decision
you record — not something a reader has to reverse-engineer:

1. **Virtualized** — the default for anything that grows with the backup, with a
   scan, or with a run. Scan-history rails and findings lists included: they gain
   a row per scan and never shed one.
2. **Bounded with disclosure** — render the first N and *say* what is not shown.
   For lists that cannot be virtualized: a printable document (every row must
   exist in the DOM for print/PDF — the Safety Scan report caps at 500 most
   severe), or a gallery inside another scroller (a note's images cap at 50).
   Never truncate silently; a list that quietly stops is worse than a slow one.
3. **Provably small** — a fixed set (nav items, sort fields, the import catalog).
   Declare it with `useBoundedList(name, count, bound)`
   (`src/lib/bounded-list.ts`), which logs in dev if the bound is ever exceeded.
   A comment claiming "this is always short" ages silently; the hook does not.

When in doubt, virtualize. The cost of virtualizing a list that stays short is a
few lines; the cost of not virtualizing one that grows is the user's machine.

**Virtualization needs a bounded ancestor, or it does the opposite.** The app
once froze on any large list, and the cause was neither the virtualizer nor the
queries — it was layout height. shadcn's `SidebarProvider` uses `min-h-svh`, a
*minimum*. With no fixed-height ancestor the whole flex chain is content-driven,
so `flex-1`, `min-h-0` and `overflow-auto` constrain nothing; the virtualizer's
spacer inflates the document, its scroll container grows to content height, and
it concludes every row is visible and mounts all of them. Small lists never
tripped it because they stayed under the viewport floor.

`app-shell.tsx` therefore passes `h-svh overflow-hidden`. **Keep it.** The
diagnostic signal is a window query sweeping thousands of rows in one go — that
means the scroller's measured `clientHeight` is huge, i.e. unbounded.

**Check it, don't assert it.** `scripts/check-virtualization.mjs` inflates the
mock fixtures to thousands of rows (the `traceloupe-mock-bulk` localStorage knob,
which the mock client reads) and fails if an audited list stops mounting only a
windowful. It reads the virtualizer's scroll height as well as the mounted-row
count — a low row count on its own looks identical to a fixture that never
inflated, which is exactly how a broken check passes.

Current classification:

| List | Bucket |
|---|---|
| Photos, Safari, Calls, Apps, Health, Notes, Messages, Recordings, Reminders, Contacts, Calendar | virtualized (`VirtualListView` / `LazyListView`) |
| Safety Scan history rail · Safety Scan findings · Security run rail · Security findings | virtualized (`VirtualList`) |
| Safety Scan report findings (500) · a note's image gallery (50) · a finding's shortened links (25) | bounded with disclosure |
| Sidebar nav · sort fields · density & theme options · filter pills (`OverflowRow` caps them) · per-contact fields · per-message attachments · severity/category breakdowns | provably small |
| Settings import catalog · activity-indicator entries · a contact's conversations · backups in a folder · Safety Scan chart buckets · home dashboard tiles | provably small, declared via `useBoundedList` |

### A chart is never drawn from a bounded list

The corollary that is easy to miss: once a list is capped, anything *summarising*
it must go back to the database. The Safety Scan report renders at most 500
findings and its narrative sees at most 100 — a chart built from either would
describe a subset while looking like it described the scan.

So `AnalysisDb::finding_analytics` aggregates in SQL over every finding the
filter matches, and it builds its `WHERE` from the same `filtered_scope()` the
page query uses. A chart and the rows beneath it therefore describe one
population by construction, not by two authors keeping two queries in step —
which is exactly what drifted in #59.

The same rule caught a live defect: the report's totals row printed
"15 findings" beside a severity split of 3 / 9 / 4. The total excluded stale
findings and the split (read off the scan row) did not.

### Charts: what they are allowed to claim

Charts in this app describe an *unvalidated local classifier's* output, so the
form is constrained on purpose (#66):

- **Counts and time, never proportions.** No pie charts, no "41% coercive
  control" — a percentage of a model's opinion reads like a diagnosis.
- **Every bar splits confirmed from unconfirmed.** Colour carries severity,
  a diagonal hatch carries "the cascade's strong tier never saw this". A scan run
  without the cascade then *looks* less certain instead of being silently so.
- **Disclosures sit next to the chart**, not in a footer: how many findings are
  charted, how many have no date (and so cannot be on a timeline), how many were
  dismissed as false positives and left out.
- **The x-axis is the content's timeline**, never a series of scan runs. Runs are
  not comparable to one another — the chunker, the model tier and the scope have
  all changed between them.
- **A timestamp outside 2007..now is not a date.** Apple stores seconds since
  2001; read as Unix time that lands in 1970, and a zeroed column lands there
  too. The bucket unit is chosen from the span and `year` is the coarsest unit
  there is, so *one* such finding turned a year of real data into a single bar
  with fifty empty ones beside it. `TIMELINE_START` closes the window, and those
  findings join the ones with no timestamp at all under "no usable date" —
  counted everywhere except the axis they cannot sit on. That is also what
  *bounds* the axis: the widest possible span is ~20 years of `year` buckets, so
  there is nothing left to truncate.
- **Hover uses the app's tooltip, never an SVG `<title>`.** A `<title>` is a
  native browser tooltip: it ignores the type ramp and the theme, waits about a
  second, and can only describe the one shape the pointer is over. Charts point
  at the whole bucket — a column or a row — and say what is in it.
- **Inline SVG, geometry in percentages.** The report prints; canvas rasterizes
  badly and CSS background gradients get dropped. No measurement, no distortion,
  and the hatch survives a greyscale print. Bucket width adapts (day → year) so
  the axis holds ~10–30 bars at any range, and the axis names the unit.

`scripts/check-design.mjs` opens the Safety view's analysis panel as one of its
measured states — a section behind a toggle is otherwise invisible to the lint,
which is where an off-ramp size survives.

### The home dashboard extends itself

The tiles on the home view are driven by `dashboard::METRIC_SOURCES` — id, label,
route, icon, table, timestamp column. The command iterates that list and sends
each tile's **label, route and icon as data**, so `dashboard-tiles.tsx` knows
nothing about which modules exist. Adding a kind of data is one row in Rust and
no frontend change; an icon name the UI does not recognise falls back to a
generic glyph rather than dropping the tile, so a new module works immediately
and merely looks generic until someone picks one.

Full table introspection was rejected: a tile needs a label, a route and an icon
that do not exist in the schema, and the cache holds plenty of tables that are
not modules (`attachments`, `note_media`, the FTS shadow tables).

**What makes "add a parser and it appears" true rather than intended** is
`every_content_table_is_accounted_for`. It lists the cache's real tables and
fails on any that has neither a `MetricSource` nor an entry in `NOT_A_TILE` with
a reason. Adding `CREATE TABLE podcasts` fails the build until someone decides
what it is. It found the six FTS shadow tables on its first run.

Two tiles, two implementations, is how their heights diverged — 119.8px and
96.5px in one grid, then 144 and 122 after they were split again. Every tile
renders through one `TileShell` with fixed row heights now, data and scan alike:
whatever goes in the slots, the rows are the rows.

**A ToggleGroup in a toolbar is an island**, not a control. It sits in the same
row as `FilterControl` and `SortControl` and must read as their equal, so it
takes `size="island"` (`--island-h`) rather than `size="sm"`. At `sm` it rendered
24px against their 30 — in Notes, Messages and Safety alike, for as long as it
had existed — because the island rule only looked for the bordered
`div.rounded-lg.bg-muted` shape and a ToggleGroup does not have it. The rule
measures both now, and knows the difference: a `FilterControl`'s button is a true
*segment*, inset inside a taller island, while a ToggleGroup's items **are** the
island, filling it. Same appearance, different geometry, so they cannot share one
expectation.

**Label, icon and order come from the sidebar**, not from the dashboard. Six of
fourteen tiles had drifted to their own names and icons — "Voice memos" for
Recordings, "Workouts" for Health, a message bubble for Safety — because the
dashboard carried a second list of names for destinations that already had them.
`src/lib/nav.ts` is now that one list; the tile looks itself up by route and
falls back to the backend's values only for a module the nav has never heard of.
Order follows it too: busiest-first changed with every backup, so no tile's
position was ever learnable.

A tile shows **facets** where a module has parts worth naming — the services in
Messages, the categories in Health — drawn as brand icons, or as words when
there is no icon to draw. Unresolvable ones are dropped rather than rendered as
`BrandIcon`'s text fallback, which turned a row of bundle ids into "COCOCOCO".

## WebKit animates less than Chromium does

The app ships in a WKWebView; the shot harness runs headless Chromium. **Animation
bugs therefore do not reproduce in the harness** — they only appear in the real
app, so an animation is not verified until it has been seen there.

- **WebKit does not animate the individual `scale` and `translate` properties**,
  which is what Tailwind's `scale-*` / `-translate-*` utilities compile to. The
  symptom is an element that fades but does not move or grow — it reads as
  "instant". Animate the `transform` shorthand, or better, animate `width` and
  `height`: a size morph is reliable. The filter popover works exactly this way —
  one persistent node, `transition-all`, swapping width/height/radius, with
  `overflow-hidden` to clip.
- **`animate-in`, `zoom-in-95` and friends do not exist here** — no
  `tailwindcss-animate` package is installed.
- **Arbitrary transition properties containing a comma** —
  `transition-[opacity,transform]` — silently fail to compile, giving no
  transition at all. Use the built-in `transition` utility.
- `index.css` zeroes all durations under `prefers-reduced-motion` inside
  `@layer base`. An unlayered `!important` does **not** beat a layered one; that
  is the cascade-layer rule, not a bug.

Verify by reading `getComputedStyle(el).animationDuration` rather than watching.

## macOS display preferences are respected

> Full reference: [`macos-integration.md`](macos-integration.md) — every setting
> we follow, how the bridge works, and the measured reasons CSS media queries
> cannot do this job. The decision behind it is
> [ADR 0004](../adr/0004-follow-macos-settings-not-app-preferences.md).

Four settings are read by `system_watch.rs`, stamped on `<html>`, and consumed by
rules in `index.css`:

| setting | attribute | what changes |
|---|---|---|
| Reduce motion | `data-reduce-motion` | transitions and animations collapse to ~0 |
| Reduce transparency | `data-reduce-transparency` | the frosted title bar goes solid, and lists stop rising beneath it |
| Increase contrast | `data-increase-contrast` | borders firm up; the secondary/tertiary text tiers lift to the primary one |
| Sidebar icon size | `data-sidebar-icon-size` | 16 / 20 / 24 px icons, row height to match |
| Differentiate without colour | `data-differentiate-without-color` | severity carried by hue alone gains a glyph (`data-severity` marks those spots) |
| Show scroll bars | `data-scroll-bars` | "always" reserves the gutter and draws a thumb instead of an overlay |

Two more system values ride the same bridge but are not attributes:
**`--selected-system-value`** is the colour macOS paints a selected row —
deliberately separate from the accent, since Appearance offers both and we were
painting selection with the wrong one — and a **locale change** re-keys the view
subtree so every date, time and number is formatted afresh (they are produced
during render from the webview's locale, which is otherwise fixed at launch).

**Why attributes and not media queries.** WebKit *supports*
`prefers-reduced-motion`, `prefers-contrast` and `prefers-reduced-transparency`
— and a WKWebView never resolves them to the system values. Measured: all report
false on a machine where AppKit reports them correctly. They are free syntax, not
free behaviour, so the values come over the bridge.

Increase contrast is also what earns back the contrast exception the design lint
records for the secondary label tiers: with it on they are no longer the
platform's low-contrast greys.

## Keyboard navigation follows macOS

**macOS decides how much Tab reaches, not us.** System Settings → Keyboard →
"Keyboard navigation" (`AppleKeyboardUIMode`) is the user's statement about it:
with it OFF, native Tab visits text fields and lists only; with it ON, every
control. `system_watch.rs` reads it and stamps `data-full-keyboard-access` on
`<html>`; `useControlTabIndex()` turns that into a `tabIndex` for any control
that is not a text field or a list.

Ignoring it is what made keyboard focus feel noisy — Tab had 46 stops in
Messages and 58 in Safety Scan, every button and every row, on a machine where
the setting was off. Following it gives 34 and 27, and 46/54 when the user turns
it on.

Two consequences worth knowing:

- **A list is one tab stop, and ↑/↓ move the selection** (`useListNavigation`).
  That is what makes this an improvement rather than a removal — Tab reaches the
  list, arrows move within it, Home/End jump. Selection moves rather than focus,
  which is what lets it work with virtualised rows that are not mounted.
- **Dialogs are exempt.** A modal must be completable from the keyboard and,
  unlike a toolbar, there is nowhere else to reach its buttons from.

Also: **a dialog does not hand focus to its first control.** Radix does that by
default, which put a focus ring on Settings' "General" tab every time it was
opened with the mouse. `DialogContent` focuses itself instead — the trap and
Escape still work, and nothing looks pre-chosen.

## Buttons always have a tooltip

**Every button gets a tooltip. No exceptions** — text buttons and icon-only
buttons alike. Icon-only buttons are unreadable without one; even labelled
buttons benefit from a one-line "what this does", which doubles as the
accessible name.

- Use the shadcn `Tooltip` (`components/ui/tooltip.tsx`), not a bare `title=`:

  ```tsx
  <Tooltip>
    <TooltipTrigger asChild>
      <Button variant="ghost" size="icon" aria-label="Delete this scan">
        <Trash2 className="size-3.5" />
      </Button>
    </TooltipTrigger>
    <TooltipContent>Delete this scan</TooltipContent>
  </Tooltip>
  ```

- No provider wiring needed inside views: the whole app already sits inside a
  `TooltipProvider` (mounted by `SidebarProvider` in `ui/sidebar.tsx`).
- Icon-only buttons keep an `aria-label` **as well as** the tooltip.
- A **disabled** button keeps its tooltip — and it should explain *why* it's
  disabled (e.g. "Exporting reports is coming soon").
- Existing `title="…"` attributes are legacy; prefer the `Tooltip` component for
  anything new, and upgrade `title=` to it when you touch a button.

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
4. **Every button gets a tooltip.** Wrap it in the shadcn `Tooltip` (see
   "Buttons always have a tooltip" above) — icon-only buttons especially, and
   disabled buttons must say why they're disabled.
5. **Build on what's in flight.** These shared components arrived via a large
   migration. Before starting UI work on a branch, `git fetch` and skim
   `origin/main` and open PRs (`gh pr list`) for related UI changes, so you adopt
   the current pattern and migrate alongside it — not around it. (The scan views
   drifted precisely because they were built while the toolbar migration was still
   on a separate branch.)
