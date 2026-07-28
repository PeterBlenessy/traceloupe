# Changelog

All notable changes to **TraceLoupe** are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

Pre-1.0, the **minor** version marks a milestone; the per-version entries below give the detail.

> The single source of truth for the version is `package.json`; keep the
> workspace `Cargo.toml` and `src-tauri/tauri.conf.json` in step when it changes.

## [Unreleased]

_Nothing yet._

## [0.35.0] — 2026-07-28

**Dates and numbers follow your Region, the dashboard follows your sidebar, and
Health is one timeline** instead of one section at a time.

### Added

- **Each dashboard tile shows what is inside it** — the messaging services in
  Messages, the categories in Health — as small icons above the count.
- **Health appears on the dashboard** whatever kind of Health data a backup
  holds. It was counting workouts alone, so a backup with steps and sleep but no
  workouts showed no Health tile at all.
- **Security Check and Safety Scan get a full-width tile each**, with the split
  by severity, what changed since the run before, how old the threat feeds were,
  and what the run covered.
- **Health is one list.** Workouts, daily activity, sleep, awards, timezones and
  cycle tracking can now be shown together, newest first — and you can pick any
  combination. Previously it was one section at a time with no way back to
  seeing everything.

### Fixed

- **Dates, times and numbers follow your Region**, not the app's language.
  macOS lets those differ — English with Region: Sweden — and the app was
  formatting everything for the language: `Jun 8, 12:40 AM` where you set
  `8 Jun, 0:40`, and the wrong thousands separator everywhere.
- **The dashboard uses the sidebar's names, icons and order.** Six tiles had
  drifted to their own — "Voice memos" for Recordings, "Workouts" for Health —
  and the tiles were sorted by size, so nothing was ever in the same place
  twice.
- **A tile for a small number of items shows its chart.** A handful of voice
  memos got no chart at all.
- **A filter you have not touched no longer looks like an active filter** — it
  was showing a chip per option in the toolbar.

## [0.34.0] — 2026-07-28

**Opening a backup now shows you what's in it** — a tile for every kind of data
TraceLoupe found, with how much, the years it covers, and when it clusters.

### Added

- **A dashboard on the home view.** One tile per kind of data the backup
  actually yielded — messages, photos, contacts, calls, Safari history, notes,
  voice memos, calendar, reminders, workouts, interactions, apps — each showing
  the count, the period it covers and a small chart of when that data clusters.
  Click a tile to go straight there.
- **Security Check and Safety Scan appear as tiles too**, showing when they last
  ran and what they found, with a Run action when a backup has never been
  scanned.
- **Kinds of data with nothing in them are left out**, so no tile leads to an
  empty screen.

### Changed

- **The "What you can do with TraceLoupe" cards now appear only before a backup
  is open.** Once one is open, its actual contents are the more useful thing to
  show — and you can click them.

## [0.33.2] — 2026-07-28

### Fixed

- **Chart hovers look like the rest of the app.** Pointing at a bar showed a
  plain browser tooltip after a delay, in the browser's styling rather than
  TraceLoupe's — and it described only the slice under the pointer. Hovering now
  covers the whole column or row and gives the full breakdown.
- **The same plain tooltips are gone from Calls, Interactions, Notes and
  Messages** — a country flag, channel totals, a group marker and an image
  count. The Messages avatar was showing a plain tooltip on top of the contact
  card it already opens.

## [0.33.1] — 2026-07-28

### Fixed

- **The note beside the charts told the truth in every state.** It claimed
  dismissed findings had been left out even when "Show dismissed" was putting
  them on the chart, and counted dismissals of every severity when you had
  filtered to one.
- **Charts covering more than a year name the year.** A two-year view read
  "Jan Feb … Dec Jan Feb" with nothing to say which January.
- **One message with an unreadable date no longer ruins the timeline.** A
  timestamp that doesn't decode reads as 1970, which stretched the chart across
  fifty years and squashed every real finding into the last bar. Dates outside
  what a backup can cover now count as "no usable date" — included in every
  other chart, and said so beside them.
- **The report's totals come from one count**, so the number in the header can't
  drift from the one below it.

## [0.33.0] — 2026-07-27

**The Safety Scan report analyses your findings instead of just listing them** —
charts for when, what and where, honest about what a local model can and cannot
tell you.

### Added

- **Analysis charts in the scan report and beside the findings list.** Findings
  over time, by category, and by conversation — in the printable report and, at
  the click of the chart button, above the findings list, where they follow
  whatever filter you have applied.
- **Every bar says how sure the model is.** Solid means a second, stronger model
  confirmed the finding; a diagonal hatch means only the fast pass ever saw it.
  A scan run without the confirmation pass now *looks* less certain rather than
  quietly being so.
- **The charts say what they leave out**: how many findings they cover, how many
  have no date and so can't appear on the timeline, and how many you dismissed
  as false positives.
- **Time buckets fit the scan.** A three-week scan reads by day and a ten-year
  one by quarter, the axis names which, and quiet periods stay visible as gaps
  rather than being closed up.

### Fixed

- **The report's totals now add up.** It printed a findings count that left out
  findings whose original message is gone, beside a severity breakdown that
  didn't — so the three severities could total more than the number above them.
- **The report's category counts covered the whole scan**, not just the 500
  findings it lists.

## [0.32.0] — 2026-07-27

**TraceLoupe follows your Mac instead of approximating it** — text, colour,
motion, contrast and keyboard behaviour all come from your system settings and
change the moment you do, and the app no longer slows to a crawl on a large
backup.

### Added

- **Eleven macOS settings are followed, live.** Accent and selection colour,
  light/dark, text size, keyboard navigation, reduce motion, reduce
  transparency, increase contrast, differentiate without colour, sidebar icon
  size and scroll bars — each applied the moment you change it, rather than the
  next time the app is clicked into. Changing your region re-formats dates and
  times immediately too.
- **The macOS Text Size setting works** (System Settings → Accessibility →
  Display). macOS doesn't hand that setting to apps built like this one, so
  TraceLoupe reads it and applies it — and unlike the in-app A+/− buttons, which
  resize what you're reading, it enlarges the toolbar and sidebar as well.
- **Arrow-key navigation in lists.** Tab reaches a list, then ↑/↓ move the
  selection and Home/End jump to the ends.

### Changed

- **Text and colour come from macOS.** Sizes follow the system's own text styles
  rather than a web scale — the app's body text had been sitting a step larger
  than every native window beside it — and text and status colours use the
  platform's, so they match other Mac apps and follow "Increase contrast".
- **Controls match native macOS**: buttons, fields and dropdowns at native
  height with tighter corners, one shared scale so a button can never be a
  different height than the field beside it.
- **Keyboard focus follows your Mac's setting** instead of stopping at every
  button and row.

### Fixed

- **Large scans stay fast.** The findings list is filtered, sorted and fetched a
  screenful at a time instead of handing the whole list to the interface — it
  used to send several megabytes on every change. Scan history, security runs
  and security findings render only the rows on screen, so a long history can no
  longer freeze the app.
- **Findings survive a re-import**, keeping their link to the message or note
  they came from.
- **Progress is honest**: bars report in order, a re-scan says how many findings
  it actually made rather than counting earlier ones, and a failed model
  download says so.
- **Safety Scan reads messages sent with an attachment**, which were being
  skipped entirely — text and all.
- **Accessibility and polish**: every control in Settings is named for screen
  readers, dialog titles no longer clip at larger text sizes, the search box
  shows a focus ring when tabbed to, and severity is readable without relying on
  colour.

## [0.31.27] — 2026-07-27

### Added

- **Selected rows use your Mac's highlight colour.** macOS lets you pick a
  highlight colour separately from the accent; the app was using the accent for
  both.
- **Severity is readable without colour.** With "Differentiate without colour"
  on, a finding count marks its severity with a symbol instead of relying on red
  versus amber alone.
- **Scroll bars follow your setting.** Choosing "Always" gives permanent scroll
  bars with room reserved for them.
- **Changing your region re-formats dates and times immediately**, rather than
  on the next restart.

## [0.31.26] — 2026-07-27

### Added

- **The app follows four more macOS settings**, and picks them up the moment you
  change them:
  - **Reduce motion** — animations and transitions stop moving.
  - **Reduce transparency** — the frosted title bar becomes solid, and lists no
    longer scroll beneath it.
  - **Increase contrast** — borders firm up and secondary text darkens to full
    strength.
  - **Sidebar icon size** (Appearance) — small, medium or large icons, with the
    row height to match.

## [0.31.25] — 2026-07-27

### Fixed

- **Reloading the app no longer puts a keyboard highlight on the sidebar
  toggle.** The app moves focus to that button when you collapse or expand the
  sidebar from the keyboard, so it doesn't get lost — but it couldn't tell that
  apart from the app simply starting up.

## [0.31.24] — 2026-07-27

### Changed

- **Keyboard navigation follows your Mac's setting.** macOS decides how far the
  Tab key reaches (System Settings → Keyboard → "Keyboard navigation"), and the
  app ignored it — tabbing through a view stopped at every button and every row.
  It now matches what native apps do: with the setting off, Tab reaches text
  fields and lists, and **arrow keys move the selection within a list**, with
  Home and End jumping to the ends. Turn the setting on and everything is
  reachable again.

### Fixed

- **Opening Settings no longer highlights the "General" tab** as though you had
  tabbed to it.
- **Clicking a dialog's close button no longer draws a keyboard focus ring.**

## [0.31.23] — 2026-07-27

### Added

- **The app follows the macOS Text Size setting** (System Settings →
  Accessibility → Display → Text Size). macOS doesn't hand that setting to apps
  like ours, so TraceLoupe reads it and applies it — and unlike the in-app A+/−
  buttons, which resize what you're reading, this one also enlarges the toolbar
  and sidebar, because someone who needs bigger text needs it everywhere.

### Fixed

- **System changes apply immediately instead of when the app regains focus.**
  Changing the accent colour recoloured every other app at once and TraceLoupe
  only after you clicked into it — it wasn't listening for the change. Accent,
  light/dark and text size are all picked up live now.
- **Every control in Settings has a name for screen readers.** The switches
  announced themselves as an unnamed button.
- **Dialog titles no longer clip their descenders** at larger text sizes, and
  generated app tiles keep their letter readable at every colour.

## [0.31.22] — 2026-07-26

### Changed

- **Severity and status colours now come from macOS.** Reds, ambers, greens and
  blues throughout the app — severity badges, "Clean" and "NEW" markers, warning
  banners, destructive buttons — use the system's own colours instead of a
  hand-picked palette, so they match other Mac apps and shift correctly between
  light and dark. Text on tinted badges is darkened or lightened as needed to
  stay readable.

## [0.31.21] — 2026-07-26

### Changed

- **Text now matches macOS.** Sizes follow the system's own text styles rather
  than a web scale — the app's body text had been sitting a step larger than
  every native window beside it. Text colours come from macOS too, which means
  they also follow the system's "Increase contrast" accessibility setting.
- **Consistent small text.** Timestamps, badges and chips were being sized by
  hand in 31 places across 7 slightly different values; they now share two named
  sizes, so the same kind of label looks the same everywhere.

## [0.31.20] — 2026-07-26

### Changed

- **The text-size buttons now resize what you're reading, not the app around
  it.** The toolbar, the sidebar and a dialog's OK/Cancel row keep their size at
  every step; messages, notes, findings, list rows and the buttons that sit among
  them scale as before. A dialog's title and body still scale — only its action
  row holds still, so a destructive button can't shift under your cursor because
  you changed a reading preference.

## [0.31.19] — 2026-07-26

### Changed

- **The buttons on a backup card are back to full size.** Open, Read & open,
  Re-import and Forget had been pinned to the small size — a leftover from when
  the standard button was too tall and everything was shrunk by hand.

## [0.31.18] — 2026-07-26

### Changed

- **The findings list stays quick no matter how much a scan flagged.** It used to
  hand the whole list to the interface at once — several megabytes at eight
  thousand findings, resent every time anything changed, including each time you
  dismissed something. Findings are now filtered, sorted and grouped in the
  database and fetched a screenful at a time.
  ([#65](https://github.com/PeterBlenessy/traceloupe/issues/65))
- **The severity counts beside the filters can no longer disagree with the list.**
  Both are now counted the same way, so a filter promising twelve findings
  always shows twelve.
  ([#65](https://github.com/PeterBlenessy/traceloupe/issues/65))

## [0.31.17] — 2026-07-26

### Fixed

- **Scan progress no longer credits this run with findings it didn't make.**
  Restarting a scan of a range that had been scanned before read
  "0% · 8823 findings so far" — the count includes everything the range already
  held, which is what the Findings panel shows, but the wording claimed it for
  the run just started. It now reads "12% · 17 new · «redacted» from earlier scans of
  this range".
  ([#65](https://github.com/PeterBlenessy/traceloupe/issues/65))

### Note

- **A scan started after 0.31.12 can't reuse work from before it.** That release
  fixed messages sent with an attachment being skipped entirely, which changes
  what a scan reads — so previously-scanned stretches containing an attachment
  have to be looked at again. It is a one-time cost, and only for ranges scanned
  before 0.31.12.

## [0.31.16] — 2026-07-26

### Changed

- **Progress bars now report in order.** Import, Security Check, Safety Scan and
  the model download sent their progress over a transport whose own
  documentation warns that rapid updates can arrive out of order — so a bar could
  jump backwards. They now use an ordered channel.
  ([#65](https://github.com/PeterBlenessy/traceloupe/issues/65))

### Fixed

- **A failed model download says so again.** Its error was sent on a path that
  the progress rework had left behind, so a failure could look like a download
  that simply stopped moving.
  ([#65](https://github.com/PeterBlenessy/traceloupe/issues/65))

## [0.31.15] — 2026-07-26

### Fixed

- **Long lists can no longer freeze the app.** Scan history, security runs and
  security findings now render only the rows on screen. Each of these grows
  forever — a row per scan, never removed — and a long one used to put every row
  in the document at once, which is what locked up the machine in an earlier
  release. Verified against «redacted»-row lists.
  ([#67](https://github.com/PeterBlenessy/traceloupe/issues/67))
- **A note packed with photos, and a finding with many shortened links, stay
  responsive.** Both now show a sensible number and say plainly how many aren't
  shown, instead of rendering everything.
  ([#67](https://github.com/PeterBlenessy/traceloupe/issues/67))
- **The findings count on a scan history row can be inspected again.** Its
  breakdown (serious / harmful / concerning) appeared on hover — but the row's
  hover actions landed on top of the count, so reaching for it was the very thing
  that covered it, and the breakdown could never be seen. The count now sits
  beside the scan's date and the actions have the right edge to themselves.
  ([#92](https://github.com/PeterBlenessy/traceloupe/issues/92))
- **Row actions no longer jump around.** They stay visible (dimmed until you
  hover), and line up in straight columns whether or not a row offers Resume — so
  Delete is always in the same place.
  ([#92](https://github.com/PeterBlenessy/traceloupe/issues/92))

### Changed

- **Buttons, fields and dropdowns have tighter corners**, closer to native macOS
  than the softer web-style rounding they had. One value controls it, so the whole
  control family stays consistent.
- **The toolbar cluster is spaced properly again.** Text size, density and theme
  were packed together as if they were one control; only the A− / A+ pair is a
  single control, and only that pair sits flush now.

## [0.31.14] — 2026-07-26

### Changed

- **Controls match native macOS.** Buttons, fields, dropdowns and toolbar
  clusters were noticeably chunkier than the macOS controls beside them — the
  standard button stood 36px tall where a native one is 28px. Everything now
  reads at native height, and every control takes that height from one shared
  scale, so a button can no longer end up a different height than the field next
  to it. Controls still grow with the text-size setting.
  ([#91](https://github.com/PeterBlenessy/traceloupe/issues/91))
- **The toolbar and sort clusters no longer tower over everything else.** Both
  stood 38px tall and read as a second row of chrome; they now sit level with the
  controls around them.
  ([#91](https://github.com/PeterBlenessy/traceloupe/issues/91))

### Fixed

- **Live reload works again for developers running the app from a git
  worktree.** The file watcher ignored the very directory such a checkout lives
  in, so edits kept serving a stale bundle with no error to explain it.

## [0.31.13] — 2026-07-26

### Fixed

- **Findings kept their link to the message or note they came from across a
  re-import.** Re-importing rebuilds the cache and renumbers every row, so
  findings from an earlier scan pointed at nothing and "View flagged text" said
  the source was no longer available — for content that was still right there.
  Findings are now re-attached by content, so past scans stay readable.
  ([#96](https://github.com/PeterBlenessy/traceloupe/issues/96))
- **A finding whose content really is gone now says so, instead of showing
  someone else's.** The dangling row id is cleared rather than left to point at
  whichever row inherited the number. And "gone" isn't permanent: re-importing a
  backup that has the content again brings the finding back.
  ([#96](https://github.com/PeterBlenessy/traceloupe/issues/96))

## [0.31.12] — 2026-07-26

### Fixed

- **Safety Scan was skipping every message sent with an attachment — including
  its text.** A message with a photo attached was never examined at all, and the
  scan still reported clean. Those messages are now scanned for what they say.
  ([#97](https://github.com/PeterBlenessy/traceloupe/issues/97))
- **The scan now says when an attachment was there.** Previously "look at this"
  read the same whether or not a photo came with it, and a note made mostly of
  photos looked almost empty. Both now note the attachment — and say plainly that
  it was not itself examined.
  ([#97](https://github.com/PeterBlenessy/traceloupe/issues/97))
- **Flagged text keeps its line breaks.** Notes shown in "View flagged text"
  arrived as one run-on block; paragraphs, headings and lists now read as written.
  Dismissals are unaffected — a note's identity no longer depends on how its text
  is laid out.
- **Settings tabs match each other again.** Safety and Security were built from
  their own components, so their spacing drifted from the rest; every tab now uses
  the same rows, and those rows are tighter than before.
  ([#93](https://github.com/PeterBlenessy/traceloupe/issues/93))

### Note

Notes re-classify once on the next scan — the text sent for classification
changed, so previous results for them are recomputed. Findings, dismissals and
message results are unaffected.

## [0.31.11] — 2026-07-25

### Removed

- **An unused command that loaded the whole photo library at once.** Nothing
  called it any more — the Photos view fetches in windows — so it was dead weight
  on every build. ([#65](https://github.com/PeterBlenessy/traceloupe/issues/65))

## [0.31.10] — 2026-07-25

### Fixed

- **The Security view fills the window like every other view.** Its scan history
  and findings table grew the page instead of scrolling in place, so with a few
  scans behind you everything below them was pushed out of reach. Both now scroll
  independently and a taller window simply shows more rows.
  ([#79](https://github.com/PeterBlenessy/traceloupe/issues/79),
  [#67](https://github.com/PeterBlenessy/traceloupe/issues/67))

## [0.31.9] — 2026-07-25

### Fixed

- **The Safety Scan history list scrolls in place** instead of growing the page.
  It gains a row per scan and never loses one, so over time it pushed everything
  below it out of reach. ([#67](https://github.com/PeterBlenessy/traceloupe/issues/67))

## [0.31.8] — 2026-07-25

### Fixed

- **Density now applies to the Safety Scan and Security views.** Both hand-rolled
  their list rows without the shared markup the setting keys off, so changing
  Density appeared to do nothing there while every other view responded.
  ([#78](https://github.com/PeterBlenessy/traceloupe/issues/78))
- **The Safety Scan findings list fills the window** instead of a fixed height. A
  tall window now shows more rows, and the list scrolls in place rather than
  making the whole page scroll to reach the rest of it.
  ([#79](https://github.com/PeterBlenessy/traceloupe/issues/79))

## [0.31.7] — 2026-07-25

### Changed

- **The activity indicator shows a Safety Scan's progress as a percentage**
  rather than a chunk count, matching what the Safety Scan view itself shows.
  ([#73](https://github.com/PeterBlenessy/traceloupe/issues/73))

## [0.31.6] — 2026-07-25

### Added

- **One place to see everything the app is doing.** The toolbar now shows a
  single indicator: it names the task when one thing is running, and reads
  "N ongoing" when several are. Clicking it lists each — Safety Scan, Security
  Check, an import, a re-import, a model download — with its own progress and a
  link to the view that owns it. Previously each kind had its own pill, so
  several at once crowded out the view's own title and controls.
  ([#73](https://github.com/PeterBlenessy/traceloupe/issues/73))

### Fixed

- **A running Security Check is no longer invisible.** Starting one and
  navigating away left nothing on screen to say it was running, and no way back
  to it. It now appears in the activity indicator like everything else.
  ([#73](https://github.com/PeterBlenessy/traceloupe/issues/73))
- **Imports, re-imports and Security Checks survive the window reloading.** Their
  progress only existed in the interface, so a reload left them looking idle
  while they carried on underneath — and starting them again then failed, because
  the first was still running. Each now re-attaches to whatever is in flight.
  ([#72](https://github.com/PeterBlenessy/traceloupe/issues/72))

## [0.31.5] — 2026-07-25

### Fixed

- **A running scan is no longer lost by the window reloading.** Scan progress
  lived only in the interface, so anything that reloaded it — a crash, a manual
  refresh, the window recovering itself — left an idle "Start safety scan" over a
  scan that was still running for hours underneath. The interface now re-attaches
  to whatever is in flight and picks its progress back up, the same way an
  in-progress model download already did.
  ([#69](https://github.com/PeterBlenessy/traceloupe/issues/69))

## [0.31.4] — 2026-07-25

### Fixed

- **A scan with thousands of findings no longer freezes the app.** The Safety
  Scan findings list rendered every finding at once; with ~8000 it drove the
  renderer to 99% CPU and 3.1 GB and locked up the whole machine. It is now
  virtualized like every other list in the app, so only visible rows are built.
  The report — which must stay whole to print and export — lists the 500 most
  serious instead, states how many it left out, and keeps counting all of them in
  its totals. ([#61](https://github.com/PeterBlenessy/traceloupe/issues/61))
- **Turning on debug logging no longer makes the app unresponsive.** Every log
  line was sent to the interface as its own message, and a running scan produces
  hundreds a second — enough to drown out the scan's own progress updates. Logs
  now arrive in batches over a transport built for streams, so they stay live and
  readable without competing with the rest of the app. If the app ever produces
  faster than it can show, it says how many lines it skipped rather than quietly
  dropping them. ([#60](https://github.com/PeterBlenessy/traceloupe/issues/60))
- **The scan's finding count matches the Findings list.** The progress counter
  showed only findings that run had newly written, while the list showed
  everything in the scan's scope — so a scan over already-checked content could
  read 84 while the list read 251. Both now show the same number, from the first
  moment of the scan. ([#59](https://github.com/PeterBlenessy/traceloupe/issues/59))

### Added

- **Optional log file.** Settings → Developer can also write logs to disk, shows
  where they go, and reveals the file in Finder. Useful for handing over a log
  after a long scan, or reading one after a crash.
  ([#60](https://github.com/PeterBlenessy/traceloupe/issues/60))

## [0.31.3] — 2026-07-25

### Fixed

- **Safety Scan's content filters are in the filter popover**, grouped under
  **Content** the same way periods are grouped under **Time** — one pill per
  message service in the backup (iMessage, SMS, TikTok, …) plus Notes. They were
  a separate row of toggle chips beside the filter button, which is not where the
  rest of the app puts its filters; selections now surface as removable chips on
  the filter island like everywhere else.
  ([#57](https://github.com/PeterBlenessy/traceloupe/issues/57))

## [0.31.2] — 2026-07-25

### Fixed

- **Opening a backup crashed the view** to the error boundary with "Rendered
  fewer hooks than expected". The backup picker returned the Device home early,
  above two of its own hooks, so the render right after a backup opened called
  fewer hooks than the one before it. Introduced in 0.31.1 with the unified
  Device home.
  ([#54](https://github.com/PeterBlenessy/traceloupe/pull/54))
- **Opening a backup is now ~100 ms instead of ~4 s.** The open was waiting on
  the backup's decryption keys — which is mostly a macOS Keychain dialog, i.e.
  unbounded human time. Browsing only needs the parsed cache, so keys now warm up
  in the background (routed through the existing lock, so nothing derives twice
  or prompts for Touch ID twice). The picker also stopped awaiting a full query
  refetch round before navigating. Per-phase timings are logged permanently —
  `[traceloupe]` for the Rust phases (debug level), `[open-perf]` for the
  frontend — with any phase that can block on a person labelled as user time.
  ([#40](https://github.com/PeterBlenessy/traceloupe/issues/40))
- **The Safety Scan report matches the scan it belongs to.** It described every
  live finding in the store, so a notes-only re-scan could narrate message
  findings from an earlier run; it now covers the scan's own sources and time
  range, the same scope its card and findings list use.
  ([#43](https://github.com/PeterBlenessy/traceloupe/issues/43))

### Changed

- **Re-scanning no longer re-writes summaries that haven't changed.** Each
  summary is keyed by a digest of the findings behind it, so a scan that added
  nothing reuses the text and spends no model calls at all.
  ([#43](https://github.com/PeterBlenessy/traceloupe/issues/43))
- **Per-conversation summaries are written on demand.** A scan used to spend one
  model call per flagged conversation at the end — 40 flagged conversations meant
  40 calls, most never read. Now the most severe few are written while the model
  is still warm, and the rest are summarized when you open them, with a
  "Summarize this conversation" action. With no model loaded you still get an
  immediate factual summary built from the findings, labelled as such rather than
  passed off as the classifier's own wording.
  ([#18](https://github.com/PeterBlenessy/traceloupe/issues/18))

## [0.31.1] — 2026-07-24

### Changed

- The Device view and the home view are now **one view**. `/` is the app's only
  home: the backup picker before a backup is open, and the full device detail
  once one is — laid out densely in two columns instead of a tall centred list,
  with the re-import / open-another / close actions alongside it. The separate
  `/device` route is gone, and the redundant phone icon with it (the sidebar
  hero already shows the device icon and name). Opening a backup lands here.
  ([#39](https://github.com/PeterBlenessy/traceloupe/issues/39))
- The selected sidebar and Settings item is now a **solid accent fill** instead
  of a low-opacity tint, matching a native macOS selection. The label keeps its
  normal color, so a selected row reads like its neighbours — only the
  background changes.
  ([#41](https://github.com/PeterBlenessy/traceloupe/issues/41))

### Fixed

- A Safety Scan now **keeps the Mac awake** while it runs (a macOS
  `PreventUserIdleSystemSleep` assertion held for the scan's lifetime and
  released on every exit path). Long unattended scans no longer stall when the
  machine idle-sleeps mid-chunk. The display still sleeps as usual.
  ([#32](https://github.com/PeterBlenessy/traceloupe/issues/32))
- A scan with findings never shows "this scan didn't produce a written report"
  again: when the model returns empty prose, a deterministic overview built from
  the findings themselves is stored instead — same guard for per-thread
  summaries. ([#43](https://github.com/PeterBlenessy/traceloupe/issues/43))

## [0.31.0] — 2026-07-24

**Safety Scan, end to end** — bounded classification you can trust, a styled
report you can export to PDF, scanning by source (iMessage/SMS/TikTok/Notes),
findings scoped correctly across re-scans, and a rebuilt findings + history
experience.

### Fixed

- Safety Scan output is bounded by a hand-written GBNF grammar passed to the
  model server, replacing the JSON-schema `response_format` (whose `maxItems`
  the pinned server silently ignores). Verdict arrays could grow until they hit
  the token budget and truncated into unparseable JSON — the whole chunk was
  then skipped (~15–45% of chunks failing on real scans). The grammar closes the
  array deterministically, and keeps bounded whitespace so the weak sweep tier
  still detects reliably (compact JSON collapsed it to empty). Verified against
  the pinned server with a synthetic classification-fixture eval. (#43)
- Safety Scan no longer chokes on very long notes: a long note is windowed
  into overlapping segments that each fit the model's context (previously one
  oversized chunk ran ~10× longer than normal, then failed unclassified —
  ~45% of a real scan's wall time). A single giant pasted message is truncated
  for the model with an explicit marker instead of sinking its whole window,
  and the per-chunk output budget is tightened so runaways cost less. (#33)

### Changed

- Safety Scan classifies several chunks concurrently (2 slots on 16 GB
  machines, 4 on 32 GB+; sequential below that) — Apple Silicon serves small
  batches at near-linear throughput, so scans finish roughly 2× faster on
  typical hardware. Per-chunk checkpointing, resume, and cancellation behave
  exactly as before. (#34)
- Safety Scan now runs as a two-tier cascade when both models are installed:
  the fast E2B model sweeps everything, then E4B re-checks only the chunks
  that got flagged (most content is clean, so most of the slow tier's work
  was confirming cleanliness). E4B's verdict wins on re-checked items — if it
  clears a chunk E2B flagged, the finding is removed. Each re-check is applied
  atomically (clear + confirm + checkpoint in one transaction), so an
  interrupted cascade never drops a finding and resumes exactly where it
  stopped; if the E4B model can't load, the E2B verdicts stand and the scan
  still completes. Near-E4B precision at close to E2B speed; the report footer
  names both tiers. Single-model machines are unchanged. (#35)
- Security Check and Safety Scan are now master–detail views: a scan-history
  rail (date-led titles, outcome filter, sorting) on the left, and the
  selected scan's report **and its findings** on the right — a past scan is
  browsed exactly like the latest one, no report-only dialog. (#25 follow-up,
  scan-views redesign; mockups in docs/design/scan-views-mockups.html)
- The Safety Scan report is a structured frame — severity stats, narrative,
  per-conversation summaries as links into Messages, and a provenance footer
  (period · model · on-device) — instead of one text block.
- Safety findings are compact one-line rows (severity · category · source ·
  date · rationale) with severity filter, sorting, a group-by-conversation
  toggle, an inline dismiss button, and a full detail sheet naming the scan
  each finding came from.
- Jumping from a Safety finding to its note now selects that note in Notes
  and shows a "Back to Safety Scan" return chip.
- Safety findings are counted and listed by **scope** (a scan's sources + time
  range) rather than by which run first classified each chunk. Classification is
  cached per chunk across scans, so re-scanning already-covered data used to
  report "Clean"; now every scan surfaces the findings that fall within it and
  overlapping scans agree. (#42)
- Findings show who and where: the sender, the timestamp (with year), and the
  source app's real brand icon (iMessage/TikTok/Notes), with phone/handles
  resolved to contact names; a flagged own-message reads "Me → recipient".
  Opening a finding jumps to the exact flagged message in the conversation (with
  a "Back to Safety Scan" chip), and the flagged text peeks in a scrollable
  popover. The report's per-conversation list shows resolved names too.
- The scan-history rail is denser and clearer: date-led titles no longer clip,
  the action buttons (report/resume/delete) hide until hover and float over the
  card, the findings pill shows just the count with a severity breakdown on
  hover, an interrupted scan keeps its findings count next to an "Interrupted"
  label, and Resume lives on the card of the scan it continues.
- A resumed scan shows its true state (already-scanned chunks and existing
  findings) from the first frame instead of counting up from zero.

### Added

- Security Check runs now carry a per-run feed receipt: the result cards and
  the exported CSV cite the exact indicator feeds the scan ran against
  ("Checked against Pegasus «redacted» · … — feeds updated 2026-07-20"), even
  after the installed feeds have since been updated. Runs recorded before
  this change list their feeds without the updated-date. (#25)
- Scan by source: the Content picker is a multi-select of the actual sources in
  the backup — each message service (iMessage, SMS, TikTok, …) plus Notes — so a
  scan can target just TikTok, or iMessage + Notes, or any mix (all picked =
  everything). Findings record their service, so a scoped scan counts and lists
  exactly what it covered.
- The Safety Scan report is a styled, printable document opened from each
  history card's document icon: a mostly-deterministic frame (scope · model ·
  duration, severity totals, category counts, findings grouped by conversation
  with resolved contact names) with the model's narrative and per-conversation
  prose spliced in. It's also the export source — Export renders the same
  document to PDF. (#43)
- Report privacy setting (Settings → Safety → Report): include the verbatim
  flagged message text in the report and its export, **off by default** — the
  report shows structured findings only unless you opt in, since an export is a
  shareable file. (#38)
- In-conversation search: search within a single conversation by message text or
  sender, from the conversation header.
- A developer setting (Settings → Developer) surfaces the cascade's confidence
  signal — a "Confirmed" badge on findings the strong tier (E4B) re-checked and
  kept — off by default.

## [0.30.1] — 2026-07-23

**Review round on the 0.30.0 UI refresh** — fixes from a three-lens code
review (frontend correctness, Rust/IPC robustness, UX/accessibility).

### Fixed

- Toolbar legibility: the view-count label reads at full muted strength (was
  2.4:1 on the solid bar), and the translucent bar's glass densified (65% +
  40 px blur) so text stays readable over extreme content.
- Graphite system accent now gets dark button text (~5:1; white text on
  Graphite measured 3.4:1). No other stock accent changes.
- Feed updates are refused while a scan is running (the running scan's
  stamped counts would otherwise describe replaced files), and a failed
  update now shows its error instead of failing silently.
- A corrupt cached accent value can no longer blank `--primary`/`--ring`
  (values are validated before applying), and a transient IPC failure keeps
  the last-known accent instead of flashing the fallback blue.
- Keyboard focus survives the sidebar-trigger swap between sidebar and
  title bar instead of dropping to the top of the tab order.
- Every review-flagged button gained its tooltip (Run scan, Update now,
  Clear/Choose folder, the Choose-a-backup CTAs, the expanded device hero),
  and the raw link-buttons gained focus-visible rings.
- Scrollbar thumbs also appear on hover, not only mid-scroll.
- Consistency sweep: Notes/Recordings lists scroll under the translucent
  bar like their siblings; Messages/Notes toolbar islands join the new
  control scale; the device hero's active state uses the accent pill; the
  Messages conversations empty state got its icon tile.
- `objc2-app-kit` now builds only the NSColor bindings instead of the whole
  AppKit surface (missing `default-features = false`).

## [0.30.0] — 2026-07-23

**UI refresh — device hero, Scans group, system accent, layered surfaces.**

### Added

- **Device hero**: the sidebar's top item is now a large phone-under-the-loupe
  illustration showing the open backup's identity (device name, model, iOS
  version, Encrypted chip) — or a dashed ghost phone with a Choose-a-backup
  action when nothing is open. Collapses to a compact mark on the icon rail.
- **System accent color**: the UI follows the macOS accent set in System
  Settings (read via `NSColor.controlAccentColor`, re-checked on window focus).
  Active nav, selection, sent bubbles, primary buttons, and focus rings all
  tint with it; scan verdicts and destructive actions stay semantic.

- **Translucent toolbar** (on by default; Settings → General to opt out): the
  title bar goes see-through with a backdrop blur, and list content — Calls,
  Safari, Apps, Calendar, Reminders, Photos, and the Contacts/Messages list
  panes — scrolls visibly beneath it.
- "Settings → Safety" in the Safety view's model prompt is now a real link
  that opens the Settings dialog on the Safety tab (new `SettingsLink` /
  `useSettingsDialog` deep-linking).
- Security view now separates evidence from configuration: the view keeps a
  read-only provenance line ("N indicators from M feeds · updated …"), the
  scan action, and a stale-feed nudge that deep-links to Settings; updating
  feeds, the per-feed source list, and the custom STIX/YAML folder moved to
  Settings → Security.

### Changed

- The sidebar toggle now lives inside the sidebar while it's open (top-right,
  the native macOS pattern) and moves into the title bar only when the sidebar
  is hidden — so it no longer reads as belonging to the view title. It also
  gained a Hide/Show sidebar tooltip.
- Scrollbars only paint their thumb while actually scrolling; the 12 px gutter
  stays reserved, so layout never shifts.
- Security and Safety moved from the sidebar header into their own labeled
  **Scans** group; the content views sit under a **Content** label.
- Sidebar icons grew from 16 px to 20 px with slightly taller rows.
- The title bar / toolbar grew from 44 px to 52 px with 20 px icons and
  matching larger controls (sidebar trigger, filter, sort, search, density
  and theme toggles), so the top chrome breathes like the new sidebar.
- The Settings dialog's nav pane now mirrors the app sidebar: same surface,
  row height, icon size, and accent-tinted active pill.
- The surface palette moved from pure white / near-black to layered tinted
  neutrals: light mode gets an off-white canvas with a deeper sidebar pane;
  dark mode lifts off near-black with the sidebar as the darkest layer.
- Every empty state now carries an accent-tinted icon tile, including the
  previously text-only ones (Calls, Safari, Apps, Notes, Recordings, Contacts,
  Calendar, Reminders, Interactions).
- Apps view rows are now bordered cards with a clear type hierarchy: app name,
  then App Store metadata, then the download receipt (emphasized date), then
  the bundle id in small monospace — one size and voice per class of info.

## [0.29.1] — 2026-07-22

**Housekeeping — repository, documentation, and release process.** No
app-behavior changes; the built app is unchanged from 0.29.0.

### Release process
- Version bumps are scripted across all manifests (`scripts/release.sh`), and the
  `vX.Y.Z` tag is now created automatically when a release lands on main
  (`.github/workflows/release-tag.yml`). A CI job (`scripts/check-releases.sh`)
  fails any PR that bumps the version without a CHANGELOG entry, whose manifests
  disagree, or whose history lost a tag. Backfilled the git tags `v0.1.0`–`v0.28.0`
  (including `v0.6.2`) and the missing `0.6.2` CHANGELOG entry.

### Documentation
- Reorganized `docs/` into `adr/ plans/ research/ reference/ validation/` and moved
  the architecture docs out of the repo root; renamed the product doc to
  `docs/product-overview.md`. Removed the redundant CHANGELOG milestone table.
  Brought `architecture.md`, `product-overview.md`, and the README current with
  0.29.0 — native-first imports, Security Check and Safety Scan, and privacy
  claims scoped to backup-derived data. New `scripts/check-doc-links.sh`
  (CI-enforced) verifies every relative Markdown link.

### Repository
- Deleted a duplicate `THIRD-PARTY-NOTICES` file; renumbered a duplicated ADR; and
  gitignore the whole `.claude/` local-runtime folder.

## [0.29.0] — 2026-07-22

**Safety Scan — on-device AI content review.** A new capability alongside
Security Check: a local large-language model reads your Messages and Notes and
flags conversations worth a human look — threats and violence, harassment,
grooming, self-harm, coercive control, scams, and more. It runs entirely on this
Mac.

- **Local and sandboxed by construction.** Classification runs a Gemma model
  through a bundled `llama-server` sidecar under a Seatbelt sandbox: no network
  except loopback, no filesystem writes outside a scratch dir, and message/note
  text lives only in the prompt — never written to disk. Release builds run only
  the bundled binary, never one from `PATH`.
- **Deterministic pipeline, resumable.** Messages are windowed and Notes chunked
  with stable keys and fingerprints; each chunk is classified against a fixed
  Forensic-9 taxonomy and validated. Findings, per-chunk progress, and summaries
  persist in a per-backup `analysis.db` that survives re-import, so a re-scan
  skips unchanged content.
- **The scan you can steer.** Choose what to scan (Messages, Notes, or both) and
  the time range, with live item counts that match the Messages and Notes views.
  Progress flips to "Scanning" the moment the model is ready; Stop aborts the
  in-flight request within about a second.
- **Results you can act on.** A scan report names the period scanned and the
  finding count; findings are severity-graded with the model's one-line
  rationale and can be dismissed as false positives (dismissals persist across
  re-scans). A scan history lists past runs — view any run's report, or delete a
  run.
- **Model provisioning + health.** A two-entry Gemma catalog with a RAM gate and
  a verified background download, plus an on-demand health check that proves the
  local model actually runs on this Mac.
- **Shipped experimental.** A Beta badge and a disclaimer make clear the
  classifier's accuracy is not yet validated on real hardware — every finding is
  a prompt to review the actual conversation, not a verdict. A labeled-fixture
  validation harness and scorer gate this in CI.

### Also in this release

- **Unified toolbar, everywhere.** Messages (both Chats and Timeline) and the
  scan views publish their title and filters into the one shared top toolbar;
  the dead in-view headers and the old `TimeFilterBar` are gone.
- **Capability-forward empty states.** Every view's "no backup" state now leads
  with what the view can do and moves the "open a backup" ask onto the button.
- **Apps view.** The App Store install receipt (download date, installing Apple
  ID, age rating, subgenre), colored per-app icon tiles, and opt-in real App
  Store artwork (off by default).
- **Security Check polish.** The external threat feeds are explained (who
  they're from, what STIX/YAML are, with links); the de-shortener risk reads as
  a warning callout; a dead setting was removed.
- **Every button has a tooltip.** A new project rule (AGENTS.md + `docs/reference/ui.md`),
  with the existing `title=` buttons swept onto the shadcn Tooltip.

### Fixed

- **Scan Delete did nothing.** Deleting a scan left its `audit_log` rows behind;
  with foreign keys on, the `scans` delete failed and the confirm dialog just
  sat there. It now clears every child table (regression-tested), and a failed
  delete surfaces a toast.
- **Sidebar scrolled horizontally.** The group separator, inset with `mx-2` but
  100% wide, overflowed its container by 16px; the divider now auto-sizes to fit.
- **Safety Scan dev-run crashes.** Fixed a SIGABRT from Tauri's dylib-less dev
  sidecar copy, and captured `llama-server` output to the logs with errors
  surfaced as toasts.

## [0.28.0] — 2026-07-21

**Security Check M3 complete — opt-in de-shortener.** Reveal where a shortened
link in a finding points. Resolving a link contacts a remote host with a URL
from the backup — the sole sanctioned exception to "nothing leaves the machine"
(ADR 0001) — so it is strictly opt-in and deliberate.

- **Per-link, user-approved:** never automatic and never during a Passive Check.
  A "Reveal destination" button on a finding's shortened links opens an approval
  dialog that names the real risk (resolving can signal that the device is being
  examined). Every use prompts by default.
- **Per-backup opt-out (not global):** the dialog carries a "don't ask again for
  this backup" switch, stored in that backup's own cache — it never applies to
  other backups, resets on re-import, and clears when the backup is forgotten.
- **Safe resolution:** only known-shortener hosts (an allowlist) are ever
  contacted; the destination is read from the redirect `Location` **without
  visiting it**, so the final target is never contacted. SSRF-guarded by the same
  public-only DNS resolver as link previews (private/loopback/metadata addresses
  refused, rebind-proof).
- New `shorteners` core module; `expand_short_url` / `find_shortener_urls` /
  `deshorten_auto_approve_get`/`set` commands.

With the de-shortener, **M3 — and the Security Check as a whole — is complete.**

## [0.27.1] — 2026-07-21

**Sidebar: Security grouped with Device.** The Security entry moves up next to
Device — both are whole-backup operations (its identity, and an audit of it),
distinct from the content views — with a separator dividing that pair from the
content list. Gives the security feature fitting prominence.

## [0.27.0] — 2026-07-21

**Security Check M3 — scan-history diffing.** A re-scan (e.g. after updating
indicators) now shows what's new.

- `query::list_findings` computes an `is_new` flag per finding by diffing against
  the previous completed scan of the same backup (matching on module + matched
  value + source artifact); `previous_completed_run` finds the baseline. The
  first scan has no baseline, so nothing is marked new.
- **Security view:** findings new since the last scan carry a **NEW** badge, and
  the results header shows a "N new since last scan" count.

## [0.26.0] — 2026-07-21

**Security Check M3 — custom indicators.** Researchers can point a scan at their
own indicator files, merged with the bundled feeds.

- **New loaders** (`indicators::load_custom_dir`, `load_indicators`,
  `IndicatorSet::merged_with`): a folder is scanned by extension —
  `.stix`/`.stix2`/`.json` as STIX2, `.yaml`/`.yml` as Echap YAML — with no
  manifest required; a malformed file is reported and skipped, a missing folder
  degrades to empty. Custom indicators are re-deduplicated against the snapshot.
- **Setting** `custom_indicator_dir` on `DetectionSettings`, applied to every
  scan (Explicit, Passive) and reflected in the indicator-feed counts.
- **Security view:** a "Custom indicators" row with a folder picker and Clear.

## [0.25.0] — 2026-07-21

**Security Check M2 complete — WebKit resource-load statistics.** Adds the last
Tier-B surface: the domains an app's in-app browser (WebKit) contacted.

- **New parsers** (`analyzer::parse_webkit_observations`,
  `parse_webkit_session_log`): read each app's
  `Library/WebKit/WebsiteData/ResourceLoadStatistics/observations.db`
  (`ObservedDomains.registrableDomain`) and the older
  `full_browsing_session_resourceLog.plist` (`browsingStatistics` origins).
- **New `webkit` scan module:** aggregates observed domains across all apps and
  matches them against domain/URL indicators; a matched domain is surfaced once,
  naming the apps whose webviews contacted it — evidence of in-app spyware C2 or
  exfiltration traffic.
- **On-demand extraction** during an Explicit Scan: every per-app
  `observations.db` is located via the Manifest index and parsed. Passive Check
  unaffected.
- Validated against the real dev backup: «redacted» observed domains extracted across
  34 apps, zero indicator matches (clean). See `docs/validation/security-check-validation.md`.

With WebKit, every MVT iOS module that matches an indicator class our feeds
carry is now covered by a shipped module — **M2 Tier-B is complete.**

## [0.24.0] — 2026-07-21

**Security Check M2 — Shortcuts.** Shortcuts can call out to arbitrary URLs; a
shortcut quietly posting to a malicious endpoint is an exfiltration/automation
vector.

- **New parser** (`analyzer::parse_shortcuts`): reads `Shortcuts.sqlite`
  (HomeDomain, `Library/Shortcuts/Shortcuts.sqlite`) — each
  `ZSHORTCUTACTIONS.ZDATA` is a binary plist of workflow actions whose string
  parameters (e.g. an `openurl` action's `WFInput`) carry the URLs, matched
  against domain/URL indicators.
- **New `shortcuts` scan module:** matches each shortcut's referenced hosts/URLs
  against indicators (feed-graded), on-demand during an Explicit Scan.
- **Refactor:** `run_scan`'s four Tier-B inputs (manifest, processes, profiles,
  grants — now plus shortcuts) are grouped into one `ScanInputs` struct,
  replacing a growing positional-argument list.
- Validated against the real dev backup: 46 shortcuts extracted (44 reference a
  host), zero indicator matches (clean). See `docs/validation/security-check-validation.md`.

## [0.23.0] — 2026-07-21

**Security Check M2 — TCC permissions.** Cross-checks which apps hold sensitive
permissions against the stalkerware bundle-id lists.

- **New parser** (`analyzer::parse_tcc`): reads granted rows from `TCC.db`
  (HomeDomain, `Library/TCC/TCC.db`) — `auth_value` 2/3 (or the legacy `allowed`
  column), mapping each `kTCCService*` to a friendly name and a
  surveillance-relevant flag (microphone, camera, screen, photos, contacts,
  location, speech, motion).
- **New `tcc` scan module:** aggregates grants per app; a client that matches a
  stalkerware/watchware bundle-id indicator is surfaced as one feed-graded
  finding listing the sensitive permissions it holds ("holds Camera, Microphone
  access") — turning a bare bundle-id match into concrete capability evidence.
  A normal app holding camera access is not flagged; only a *known monitoring
  app* holding it is.
- **On-demand extraction** during an Explicit Scan via the Manifest index,
  best-effort. Passive Check unaffected.
- Validated against the real dev backup: 116 grants across 67 apps extracted,
  zero stalkerware matches (clean); the positive path is covered by unit tests.
  See `docs/validation/security-check-validation.md`.

## [0.22.0] — 2026-07-20

**Security Check M2 — configuration profiles.** Surfaces installed configuration
profiles, the classic stalkerware install vector (an unexpected or hidden
profile can grant broad control over the device).

- **New parser** (`analyzer::parse_configuration_profiles`): reads
  `ProfileTruth.plist` (the authoritative installed-profile list, keyed by
  `Name from Org (UUID)`) and `PayloadManifest.plist` (the `HiddenProfiles`
  set), extracting each profile's name, organization, UUID, referenced hosts,
  device-management capabilities, and hidden flag.
- **New `profiles` scan module:** matches profile hosts/names/orgs against
  indicators (feed-graded), and adds one structural review finding per profile —
  **Warning** for a hidden profile (invisible in Settings), **Info** for a
  device-management profile (MDM/proxy/VPN/content-filter), else a plain
  "review if unexpected" **Info**.
- **On-demand extraction** during an Explicit Scan via the Manifest index
  (SysSharedContainer configuration-profiles domain), best-effort. Passive Check
  unaffected.
- Validated against the real dev backup: the one installed profile (a legitimate
  university printer profile) is parsed correctly and surfaced as a single Info
  review finding — no false alarm. See `docs/validation/security-check-validation.md`.

## [0.21.0] — 2026-07-20

**Security Check M2 — process-name detection (first Tier-B surface).** Adds the
artifact class that originally exposed Pegasus: process activity, matched
against process-name indicators.

- **New parsers** (`analyzer::parse_datausage`, `parse_addaily`): DataUsage.sqlite
  `ZPROCESS` (process name, bundle name, Mac-absolute timestamp → Unix) and the
  OSAnalytics `com.apple.osanalytics.addaily.plist` `netUsageBaseline` dictionary
  (keyed by process name).
- **New `process_names` scan module:** matches each observed process name (and
  its basename) against process-name indicators, and DataUsage bundle names
  against bundle-id indicators, graded Critical.
- **On-demand extraction:** an Explicit Scan locates and extracts both files via
  the Manifest index (WirelessDomain / HomeDomain), best-effort — a missing or
  unreadable file just yields fewer processes, never fails the scan. The Passive
  Check stays apps-only.
- Validated against the real dev backup: «redacted» processes extracted, no mercenary
  process-name matches; the bundle-name cross-check independently re-surfaced the
  known Kaspersky Safe Kids watchware (Info) found in M1. See
  `docs/validation/security-check-validation.md`.

## [0.20.0] — 2026-07-20

**Security Check (M1).** A local scan that checks an imported iPhone backup for
indicators of compromise from known mercenary spyware (Pegasus, Predator,
KingsPawn, Operation Triangulation, NoviSpy, Wintego, EagleMsgSpy, Candiru,
Coruna, DarkSword) and commercial stalkerware/watchware — modeled on iMazing's
Spyware Analyzer and Amnesty International's MVT methodology, implemented
natively in Rust (no MVT code; only its CC-BY indicator data and the public
Echap stalkerware feed).

- **Indicator feeds.** STIX2 bundles + Echap `ioc.yaml`/`watchware.yaml`
  normalized into one indicator set (domains, URLs, emails, process names,
  file names/paths, bundle IDs, cert hashes, IPs), with evidence-graded
  severity. A snapshot of ~«redacted» indicators is bundled for offline use and can
  be refreshed over HTTPS from the public feed repos.
- **Scan engine.** Evaluates the set against the cache (messages, attachments,
  Safari history/bookmarks, notes, calendar, contacts, interactions, installed
  apps) plus a `Manifest.db` file-name/path/app-domain sweep. Conservative host
  tokenizer and exact-or-subdomain matching to limit false positives. Full scan
  of a real backup completes in well under a second.
- **Two modes.** User-initiated **Explicit Scan** (all modules, optional fresh
  feed fetch) and a consent-gated **Passive Check** at import (apps-only by
  default, configurable). Both governed by persisted detection settings with
  one-time consent dialogs.
- **Security view.** Severity-graded findings table with per-finding detail and
  deep links into the source artifact, a persistent false-positive/clean-≠-safe
  disclaimer, a stalkerware-victim safety panel, indicator-freshness display,
  and CSV report export with CC-BY attribution.
- **Privacy (ADR 0001).** Backup-derived data never leaves the machine; feed
  fetches are disclosed, setting-governed, send-nothing operational traffic.

Validated against `mvt-ios check-backup` (see `docs/validation/security-check-validation.md`):
every indicator class MVT matches from these feeds is covered by a shipped
module; the rest maps to the named M2 (Tier B) scope.

## [0.19.0] — 2026-07-20

**Data-coverage close-out.** The last field-level items, and a line drawn under
the coverage effort (see `docs/reference/app-data-coverage.md`). Requires a re-import to
populate for existing caches.

### Added

- **Safari — local open tabs.** The Safari "Tabs" list now comes from
  `BrowserState.db` (this device's actual open tabs, 201 here) instead of the
  thinner iCloud-synced `SafariTabs.db` (44), and each tab shows its last-viewed
  time and a **Private** badge for private-browsing tabs (schema v46).
- **Calls — number country.** A call number's ISO country
  (`ZISO_COUNTRY_CODE`) shows as a flag on the call row (schema v45), so
  international calls stand out at a glance.
- **Photos — added-to-library date.** When a camera-roll asset was added to the
  library (`ZASSET.ZADDEDDATE`) is surfaced in the lightbox as "Added &lt;date&gt;"
  whenever it differs from the capture date by more than a day — flagging media
  that was received, saved, or imported rather than shot on this device (schema
  v44). ~«redacted» such assets in the reference backup.

## [0.18.0] — 2026-07-20

**Data-coverage pass.** More fields already present in a backup, now surfaced —
with a forensic "recover what was deleted or hidden" throughline (Recently
Deleted photos and messages). Requires a re-import to populate for existing
caches.

### Added

- **Interactions — per-app channels.** A "Channels" summary strip above the
  interaction list shows which apps CoreDuet interactions flowed through
  (Messages, Phone, FaceTime, Snapchat, Gmail, …) with in/out totals, read from
  the raw `ZINTERACTIONS` table (the person-level `ZCONTACTS` graph has no app
  dimension). Bundle ids sharing a name merge; zero-total channels drop.
- **Health — Cycle Tracking.** A menstrual-flow + symptoms section from the
  HealthKit category samples.
- **Health — Awards.** Earned Apple Fitness achievements.
- **Contacts — social / IM profiles.** AddressBook property 46 (Twitter,
  Instagram, and other service handles).
- **Apps — App Store metadata.** Name, seller, version, genre, and release year
  parsed from each app's `iTunesMetadata` bplist.
- **Messages — sticker classification.** Sticker attachments
  (`attachment.is_sticker`) are now classified as their own content kind,
  lighting up the Stickers content-filter (which previously never matched).
- **Messages — Recently Deleted.** Deleted-but-recoverable iMessages
  (`chat_recoverable_message_join`, never read before) are recovered and shown
  with a red "Deleted &lt;date&gt;" badge — messages that had vanished from the
  conversation reappear, forensic.
- **Messages — expressive send effects.** "Sent with Confetti / Slam / Invisible
  Ink / …" from `expressive_send_style_id`.
- **Messages — app-bubble messages.** iMessage-app bubbles (`balloon_bundle_id`)
  surfaced as a distinct message kind instead of empty rows.
- **Photos — Live Photo & burst badges.** Distinguishes Live Photos
  (`ZPLAYBACKSTYLE`) and burst stacks (`ZAVALANCHEUUID`).
- **Photos — Recently Deleted.** Trashed camera-roll assets (`ZTRASHEDSTATE`)
  are now surfaced with a red trash badge and a lightbox indicator instead of
  being dropped at ingest — forensic, matching the Hidden-album treatment.
- **Notes — image filter.** Filter for notes that carry embedded images.
- **Device — toolbar.** Close the backup, re-import, or open another backup
  without leaving the view.

### Fixed

- **Photos — panorama mislabel.** `ZKINDSUBTYPE == 2` is a Live Photo
  still-frame, not a panorama (which is `== 1`); panoramas are now counted
  correctly.
- **Notes — honest image availability.** Notes whose images were offloaded to
  iCloud no longer pretend to display them; they state the images aren't in the
  backup instead.

## [0.17.0] — 2026-07-20

**Health rings, mobility metrics & the timezone timeline.** Requires a
re-import to populate for existing caches.

### Added

- **Activity rings** — `activity_caches` (the pre-aggregated Move/Exercise/
  Stand rings with their goals) becomes an `activity_rings` table (schema
  v34); daily rows show goal progress ("Move 412/500 kcal"). Rings a device
  never tracked stay blank (phone-only stores have Move only).
- **Mobility + audio-exposure metrics** — five more quantity types in the
  daily aggregation: headphone audio exposure (dB, loudest sample), walking
  speed, step length, double-support and walking-asymmetry fractions. Spread
  metrics merge min/avg/max across sources instead of summing.
- **Timezone timeline** — every sample's recording timezone
  (`data_provenances.tz_name`) aggregated per zone and device model (schema
  v35) into a new **Timezones** section: zone, sample count, devices,
  first–last span. A travel history hiding in the Health store.

### Changed

- **Health view sections are descriptor-driven** — the per-section machinery
  (sort state, windowing, counts, empty states, rendering) collapsed into one
  pipeline; workouts share the same virtualized list as the other sections.
  Adding the Timezones section was one descriptor entry. (Review finding.)
- The `health_daily` metric names are shared constants between parser and
  query — a rename can no longer silently drop a metric. (Review finding.)
- The device-model → marketing-name mapping moved to a shared module (used
  by Device and the timezone timeline) and covers more models.

## [0.16.1] — 2026-07-20

**Review-fix pass over 0.16.0** (multi-agent code review; 10 findings, 8 fixed).

### Fixed

- **Health daily aggregates no longer double-count multi-device days** — samples
  are aggregated per source (`data_provenances.source_id`); cumulative metrics
  keep the largest source's daily total, heart rate merges across sources.
  Requires a re-import to correct existing caches.
- The Sleep section's sort control reflected stale state after changing the
  sort field; daily rows now window against local-midnight preset bounds
  (was off by one day at range edges outside UTC); the Workouts section no
  longer claims "No health data" when daily/sleep data exists.
- Contact groups ignore non-person `ABGroupMembers` rows (`member_type != 0`),
  which could mis-tag a contact via a ROWID collision.
- The daily/sleep lists are fetched only when their section is visible.

## [0.16.0] — 2026-07-19

**Health deep-dive + contact relationships.** Requires a re-import to populate
for existing caches (the migrations create the structures; the parsers fill
them).

### Added

- **Health: daily activity** — the raw HealthKit quantity samples (steps,
  distance, resting/active energy, flights, heart rate) are aggregated per UTC
  day into a new `health_daily` cache table (schema v30) and shown in a new
  **Daily activity** section: one row per day with totals and the heart-rate
  min/avg/max (canonical count/sec scaled to bpm), time-filterable and sortable
  by date/steps/distance. «redacted» days in the reference backup.
- **Health: sleep sessions** — sleep-analysis category samples (data type 63)
  become a `sleep_sessions` table (schema v31) with friendly stage names
  (In Bed / Asleep / Awake / Core / Deep / REM) and a **Sleep** section
  (start–end, duration, date/duration sort).
- **Health: workout GPS routes** — each workout's location series
  (`associations` → `data_series` → `location_series_data`, tombstoned links
  skipped) is stored downsampled to ≤«redacted» points (schema v32). Workout rows
  with a route expand to an inline SVG route preview (equirectangular
  projection, start/end markers, altitude-range caption). The Health view now
  has a **Section** filter (Workouts | Daily activity | Sleep).
- **Contacts: relationships + groups** — related names (`ABMultiValue`
  property 23; label = relationship, iOS magic labels cleaned, custom labels
  kept) and address-book group memberships (`ABGroup` ⋈ `ABGroupMembers`) are
  parsed into `related_json`/`groups_json` (schema v33) and shown as a
  **Related** field group and **Groups** chips in the contact detail.

## [0.15.0] — 2026-07-19

**Data-coverage pass — surfacing fields already in the backup.** Requires a
re-import of the affected module to populate for existing caches (the migrations
create the structures; the parsers fill them).

### Added

- **Notes: all embedded images** — the detail pane now shows every image in a
  note (gallery), not just the list thumbnail. New `note_media` cache table
  (schema v29) holds each resolved image with its on-demand decrypt fields; the
  note-image protocol takes an optional index. True inline-at-position rendering
  remains future work.
- **Messages: group actions** — iMessage system rows (rename, add/remove
  participant, leave, group-photo change; `item_type` 1–4) were dropped because
  they carry no text/attachment. They're now surfaced as centered `system`
  messages ("‹actor› ‹action›"). ~370 in the reference backup.
- **Safari: reading list** — reading-list rows show their last-viewed date
  ("Read ‹date›") or an "Unread" badge.

### Fixed

- The Messages parser's new columns are schema-guarded, so older `sms.db`
  versions (and the test fixtures) are unaffected.

## [0.14.0] — 2026-07-19

**The "islands" toolbar (0.13.0) becomes a Filter · Sort · Search paradigm.** One
funnel **Filter** button morphs open into a grouped panel; sort and search stand
on their own. Plus a state-aware title bar and a reorganised sidebar.

### Added

- **Filter panel** (`FilterControl`): the funnel button *morphs* into the panel —
  its width/height/radius animate from the button's footprint to the full panel
  (the NoteSage command-bar technique; width/height animate reliably in the macOS
  webview, unlike the `scale`/`translate` properties). Each facet is a **labelled,
  described group**; facets the backup lacks are omitted, empty time presets are
  disabled. Selected filters show as **removable chips** on the closed island that
  collapse-animate away on removal. **Multi-select** where it fits (e.g. Notes
  tags); single-select for Time and single-choice facets.
- **Standalone Sort** (a bordered island) and an **expanding Search** box whose
  width animates open.
- **State-aware title bar**: when the sidebar is **collapsed** the title bar spans
  the full window width above the icon rail; when **expanded** the sidebar runs
  full height and the bar only covers the content area to its right. Traffic
  lights vertically centred; the sidebar toggle is its own island.
- **Sidebar reorg**: the top entry merges with the **Device** view (shows the
  open device's name); **Settings** moved to the sidebar footer.

### Changed

- **All views** migrated off the islands engine to the new paradigm; the adaptive
  islands fit-engine (`toolbar-islands`, the measurement logic in
  `AdaptiveToolbar`) and the now-unused `PanelHeader` were removed.
- Filter chips use discrete borders (no louder than the island border).

### Fixed

- WebKit wouldn't animate the individual `scale`/`translate` CSS properties, so
  all toolbar animations use `transform`/`width`/`height` instead.
- The dev window config now applies the merged-titlebar settings (it previously
  replaced the base window, losing `titleBarStyle`/`hiddenTitle`).
- A review pass hardened `FilterControl`: guards a toggle-based multi-select clear
  against double-firing, cancels pending timers on unmount, adds focus
  management, and re-measures the panel with a `ResizeObserver`.

## [0.13.0] — 2026-07-19

**Adaptive "islands" toolbar.** Every view's controls now live in one unified
top bar as segmented, bordered **islands** of related actions — a facet, the
time range, sort, an expanding search — verified across all views with the
screenshot harness.

### Added

- **Adaptive islands toolbar** (`AdaptiveToolbar` + `toolbar-context` +
  `toolbar-islands`). Islands share the available width evenly so none
  dominates; each reveals as many chips as fit and tucks the rest behind a
  single expand chevron. Clicking a chevron **expands** that island and collapses
  the others — to their two-item minimum where there's room, otherwise to a
  single representative icon. A greedy fit reveals more chips into any slack and
  only collapses the widest island to an icon when the minimums don't fit, so
  nothing overflows and the app-wide controls never clip.
- **Data-aware time island** — presets whose range holds no items fold away
  first, so the visible chips are the ranges that actually contain data.
- **Expanding search island** — a magnifier that opens into an inline input,
  bordered like the other collapsed islands.

### Changed

- **All thirteen views migrated** to publish their filters/sort/search as islands
  via `useViewToolbar`, dropping their in-panel `PanelHeader`/toolbar rows:
  Safari, Photos, Calls, Interactions, Calendar, Reminders, Health, Apps,
  Contacts, Notes, Recordings, Messages, and Device. Master-detail views
  (Contacts, Notes, Recordings, Messages) keep their detail sub-headers; Messages
  lifts its top-level view (Chats/Timeline) and app-service controls into the
  unified bar while its per-pane order/jump/kind controls stay contextual.

## [0.12.2] — 2026-07-18

Toolbar polish follow-ups (verified against Apple's apps via the native-window
screenshot harness).

### Changed

- **Custom date-range button rejoined the time-preset chips** — it was being
  stretched over next to the sort control; now `All / 24h / 7d / 30d / <year> /
  Range` read as one time-filter unit, with sort on its own at the right.
- **Header filter chips no longer collapse into "⋮" when there's room** — the
  source/type filter (Safari's History/Bookmarks/Reading List/Tabs, Photos'
  source chips) now claims the free header width and shows every chip that fits,
  overflowing only when the row is genuinely narrow.

### Added

- Onboarding polish: the primary **"Read & open" / "Open"** actions are real
  buttons; the encrypted-backup password step shows a **Keychain trust line**;
  and a wrong password keeps the field with an inline "That password didn't work"
  instead of a dead-end error card.

## [0.12.1] — 2026-07-18

A UX consistency-and-polish pass, verified against Apple's apps via a new
screenshot harness (headless Chromium + real-window `screencapture`).

### Added

- **Native macOS title bar** — the window now uses `titleBarStyle: Overlay` with
  a hidden title and positioned traffic lights, so the app content runs to the
  top as one unified bar instead of a native titlebar stacked over a separate app
  bar. The sidebar reads as its own panel beneath the lights.
- **Notes-style toolbar button group** — the top-bar controls (sidebar toggle,
  density, theme, settings) are grouped into one subtly bordered segmented
  cluster (`ToolbarGroup`).
- **Dynamic, minimal view toolbars** — a macOS-style **expanding search** (an
  icon that opens into an inline input, with a clear button) replaces the
  full-width search row, and a **Filter toggle** reveals the filter/sort row on
  demand (state persisted per view). Three header rows collapse to one.
- **Reduced-motion** support (`prefers-reduced-motion`).

### Changed

- **Density now applies to every list view** — Calendar/Reminders/Health/
  Interactions rows moved onto the shared `Item`/`list-row` slots, so the
  Comfortable/Cozy/Compact setting affects them (and they share one borderless
  row style instead of bordered "cards").
- **One filter-chip language** — badge filters and time-preset chips share a
  single tinted-pill treatment (`filterPillClass`).
- **One empty & loading system** — zero-result states use the richer `EmptyView`
  (icon + title) and loading uses a single shared `ListSkeleton` everywhere.
- All date/number formatting routes through `lib/format.ts` (fixes a Calendar
  12/24-hour bug; Safari visit counts get thousands separators).

### Removed

- The dead "Parsing engine needed" card (leaked iLEAPP / `pnpm setup:engine`
  developer text) and the unused `engine-setup`/`placeholder` views.

## [0.12.0] — 2026-07-18

A large Messages- and media-focused UX release, capped by a pre-release code
review that hardened the new link-fetching and scroll-persistence code.

### Added

- **Link previews for URLs in messages.** OpenGraph "unfurl" cards behind a
  single **3-way setting — Off · Hover · Inline**: *Inline* renders the card in
  the bubble (iMessage-style, replacing the raw URL when the message is only a
  link); *Hover* shows it in a popover. Every link in a message is unfurled (up
  to a cap), in both the conversation and the Timeline. Rich links from iMessage
  plugin payloads (e.g. Apple Maps) are decoded offline from the typedstream;
  TikTok uses its oEmbed endpoint (it serves no OpenGraph to bots). Preview
  images are proxied to `data:` URLs so the webview never contacts the host, and
  a crawler-style User-Agent is used (a browser UA regresses Spotify/Instagram).
- **In-app image & video viewer.** Message images/videos open in a shared,
  full-viewport lightbox with selectable styles, an opaque metadata overlay, and
  a dedicated **Media** settings tab. Videos show a first-frame poster instead of
  a black rectangle.
- **Recover missing attachments from the camera roll** (opt-in). When a
  message's image/video isn't in the backup, TraceLoupe can match it to a
  `Photos.sqlite` asset and display it, badged as *recovered*; the Timeline flags
  genuinely-missing attachments with a "not in backup" note.
- **Notes rich text** — formatting, lists, and checklists are now *rendered*
  (not just counted), first-image thumbnails appear in the Notes list, a
  hashtag-tag filter (iOS 15+) is available, and a flat/folder-tree view toggle.
- **Contact-aware Timeline avatars** — hover shows the contact; clicking opens
  them in Contacts. Added year quick-filters, jump-to-top/bottom, and the year in
  row times.
- **Persisted UI state** — Timeline & conversation scroll position (index-based),
  the Timeline time range, message time-order toggles, sidebar open/closed state,
  and window size/position all survive navigation and app restarts.
- **Overflow "⋮" menus** for the time-range and badge filters, so filters never
  wrap or push the header taller; jump-to-top/bottom added to conversations too.

### Changed

- Settings rows stack their (now full-width) description below the label + control.
- Timeline direction arrows read relative to the shown party, and outgoing rows
  resolve the contact (fixing the "#" placeholder avatar).
- Toolbar layout unified — time range on the left, facets + sort on the right;
  the new Calendar/Reminders/Health/Interactions views gained filters and
  surfaced metadata; list content left-aligned.

### Fixed

- **WAL-mode databases dropped data** (Safari history came up empty) — each DB's
  `-wal` sidecar is now replayed so unflushed rows are read.
- **Encrypted media no longer 404s** in a fresh session — the custom-scheme
  protocol handlers lazily reload the backup keys, and a *cancelled* Touch ID
  unlock no longer re-prompts once per media item (a photo grid could storm).
- Media no longer vanishes when switching views (per-mount cache key); opening an
  attachment no longer launches TextEdit on binary garbage.
- Jump-to-message and scroll restore reworked to be index-based and reliable —
  wait for the row count, re-issue across frames, and let an explicit jump win
  over position restore.

### Security

- **Closed a DNS-rebind SSRF in link-preview fetching.** URLs come from
  third-party messages in a backup (potentially of a compromised phone), i.e.
  attacker-controllable input, and the static private-host pre-check was
  bypassable by rebinding the domain between the check and ureq's connect. Fetches
  now pin the vetted address via a resolver that yields only globally-routable
  IPs, re-checked on every redirect hop and failing closed. Also folded in
  earlier link-preview/locked-note review hardening.

## [0.11.4] — 2026-07-16

A review of the Tauri media-serving/backend layer. No security hole (path
traversal is closed, the frontend can only ever supply a numeric id, secrets stay
out of logs); these fix the resource and secret-at-rest items it surfaced.

### Fixed

- **Scrubbing an encrypted video/audio no longer re-decrypts the whole file per
  request** — Range requests reused to decrypt the entire attachment into memory
  and a fresh temp on every seek (an OOM/disk-thrash path on a large video). The
  plaintext is now decrypted once to a temp cached by id (unique-temp + atomic
  rename, so concurrent requests can't read a half-written file) and reused across
  seeks.
- **Concurrent thumbnail renders can't serve a half-written JPEG** — `sips` now
  writes to a unique temp and atomically renames into the cache (owner-only before
  it's visible), fixing a race between two requests for the same image.
- **Decrypted-plaintext temps are cleared when a backup is closed or switched**,
  not only on forget — full-plaintext originals and externally-opened attachments
  no longer linger past the session.
- **Forgetting or switching a backup can't race an in-flight import** — both now
  take the import lock before touching cache files.

### Security

- The backup password is now held in zeroized buffers, wiped from memory on drop.

## [0.11.3] — 2026-07-16

A broad frontend + UI/UX review. Fixes real interaction bugs and tightens
consistency across the newer views.

### Fixed

- **Calendar / Reminders / Interactions were unscrollable and un-virtualized** —
  they wrapped the virtual list in a plain block, so it had no bounded height:
  the list couldn't scroll and every row mounted at once (rows past the fold were
  clipped and unreachable). They now use the shared `VirtualListView`, which also
  gives them a loading skeleton, an error state, and the same row width as every
  other list.
- **Health** gains loading and error states.
- **Device** shows an error state instead of a blank panel when its query fails.
- **Re-import didn't refresh some counts** — the Messages Timeline total (a
  query-key typo) and the Photos time-chip counts stayed stale after a re-import;
  both now invalidate correctly.
- **Contacts weren't requested before a backup was open** — the shared contact
  resolver now gates on an active backup.

### Changed

- Calendar/Reminders list-name pills use the shadcn `Badge`; the Reminders header
  count is now the total (matching every other view); Photos grid thumbnails have
  an accessible label and a keyboard focus ring.

## [0.11.2] — 2026-07-16

A broad whole-crate review of `traceloupe-core`. The security-critical surface —
keybag/AES decryption, the Manifest path guards, and all dynamic SQL — verified
clean (no reachable panics from adversarial keybag/plist/typedstream/postbox
bytes, no SQL injection, no path traversal). This releases the data-integrity
hardening it surfaced.

### Fixed

- **Timestamp overflow across 13 parsers** — converting a Core Data date did
  `d as i64 + MAC_EPOCH`, which saturates the float→int cast and then overflows
  the integer add on a corrupt/absurd date (~1e19): a panic in debug builds, a
  wrapped-negative time in release. Now the epoch is added in floating point
  before the cast, so it saturates cleanly. (safari, calls, address book, photos,
  reminders, health, interactions, calendar, and the WhatsApp/Viber/Kik/Threema
  chat parsers.)
- **Safari bookmarks: one bad row no longer wipes the whole import** — a NULL
  `type` or `id` was read strictly and aborted the entire bookmarks/reading-list/
  tabs load; such rows are now tolerated (NULL type → not a folder) or skipped.
- **WhatsApp / Facebook Messenger: a mistyped cell no longer drops all messages**
  — message body/timestamp now go through the same tolerant column readers the
  other app-chat parsers use.
- **Recordings re-import keeps the folder name** — a recordings-only re-import
  hardcoded the Voice-Memos folder to NULL; it now matches the full import.

## [0.11.1] — 2026-07-16

A code-review pass over the 0.9.0→0.11.0 work. The reviewed surface (Notes
decryption/crypto ladder, the five new parsers/views, the import/IPC/frontend
wiring) verified correct; this releases the handful of real fixes it found.

### Fixed

- **Messages import no longer aborts on a NULL-dated row** — `message.date` was
  read as a required integer, so one NULL date (the column is `INTEGER DEFAULT 0`,
  not `NOT NULL`) would fail the entire Messages parse. Now read optionally.
- **Attachment-only messages no longer dropped on a stale flag** — a message with
  no text was skipped whenever the denormalized `cache_has_attachments` flag was
  stale (0 despite real `message_attachment_join` rows). Selection and the
  has-attachment flag now consult the actual join table.
- **Health workouts pick their activity deterministically** — a multi-activity
  (multi-sport / all-NULL-primary) workout previously showed an arbitrary
  activity's type/duration; now it deterministically prefers the explicit primary,
  else the longest, and aggregates sample dates for the true span.
- **Locked notes are unlockable even without an iteration count** — `note_crypto`
  no longer requires `ZCRYPTOITERATIONCOUNT` (decryption already defaults 0/absent
  to 20000), so a schema that omits it still gets a password prompt.
- Hardening: `aes_ecb_decrypt_block` is panic-safe in isolation; corrected stale
  doc/comments (Notes ciphertext column, import step count).

## [0.11.0] — 2026-07-16

Closes the last gap from the 0.9.0/0.10.0 coverage audit: **password-protected
(locked) Apple Notes can now be unlocked**. The note password is entered in the
app and never leaves it; nothing is decrypted at rest — only the crypto
parameters are cached, and the plaintext is derived on demand and discarded.

### Added

- **Locked-note decryption** — unlocking a protected note runs Apple's crypto
  ladder: `PBKDF2-HMAC-SHA256(password, salt)` → AES key-unwrap (RFC 3394) of the
  per-note key → `AES-128-GCM` over the note body (IV/tag/ciphertext from
  `ZICNOTEDATA`) → gunzip → protobuf → text. Salt/iterations/wrapped-key are read
  from the note object, matching Apple's real table layout.

### Fixed

- **Locked-note decryption was broken** — the parser read the ciphertext from a
  nonexistent `ZENCRYPTEDDATA` column, took the GCM IV/tag from the wrong table,
  and ignored `ZCRYPTOWRAPPEDKEY` (skipping the key-unwrap step), so `unlockNote`
  always failed. All three are corrected. The decryptor is also resilient to an
  anomalous on-device variant (iteration count `0` → 20000 default; a 16-byte
  wrapped key) by trying multiple key candidates and letting the GCM tag select
  the right one.

### Internal

- Cache schema **v23 → v24** (adds `notes.crypto_wrapped_key`).

## [0.10.0] — 2026-07-16

Follows the 0.9.0 coverage audit by **surfacing the untapped stores** it flagged —
five new views plus deeper decoding of Messages and Notes. See
[`docs/reference/app-data-coverage.md`](docs/reference/app-data-coverage.md) for the field-level
inventory.

### Added

- **Device view** — the active backup's device/backup metadata (name, model
  mapped to a marketing name, iOS version, serial, last-backup date, encryption).
- **Calendar view** — events from `Calendar.sqlitedb` (title · when · location ·
  notes · calendar).
- **Reminders view** — from the reminders store (title/notes · completion · flag ·
  list · due date).
- **Health view** — a workout log (activity, date, duration, distance) plus a
  sample-count + date-range summary, without materializing the raw samples.
- **Interactions view** — CoreDuet's pre-aggregated cross-app communication graph:
  who you've talked to, incoming/outgoing counts, and the span, most-contacted
  first.
- **Messages: `attributedBody` decoded** — recovers the body of modern text-less
  messages (streamtyped NSString extractor, validated 3000/3000 against the `text`
  column), and flags **edited** messages (`date_edited`) with an "Edited" tag.
- **Notes: rich-content indicators** — checklist badge (`ZHASCHECKLIST`) and
  per-note embedded image / attachment counts.
- **App-chat attachment media framework** — the shared inserter now resolves an
  `AppMessage`'s attachments to backup files (`attachments` table + gallery
  mirror), closing the audit's cross-cutting gap. Per-app emission lands when a
  backup with app media is available to validate against.

### Notes

- **Locked-note decryption** remains unfixed and is a **known defect** — iLEAPP
  doesn't decode encrypted notes and the on-disk crypto is ambiguous, so a correct
  fix needs a validated reference/known-answer vector.

## [0.9.0] — 2026-07-15

A **data-coverage pass**: a field-level audit of the real backup (parser →
cache → query → UI) followed by filling every high-value, tractable gap it
found. Each item below is verified end-to-end. See
[`docs/reference/app-data-coverage.md`](docs/reference/app-data-coverage.md) for the full inventory
and the remaining (large-feature / password-blocked) gaps.

### Added

- **Calls: FaceTime audio vs video + call location.** `ZCALLTYPE` distinguishes
  FaceTime Audio from Video (only video gets the video icon); `ZLOCATION`
  (carrier/geo) shows in the call row.
- **Photos: EXIF, dimensions, file size, video duration.** Camera make/model,
  lens, and a compact "ISO · ƒ · shutter · mm" exposure summary in the lightbox,
  plus pixel dimensions, original file size, and video length.
- **Photos: hidden-album flag** — hidden assets are badged (eye-off), shown and
  flagged rather than silently mixed in (forensic stance).
- **Photos: screenshot / panorama subtype badges.**
- **Contacts detail** — birthday, note, job title, department, nickname, middle
  name, and structured postal addresses.
- **Voice Memos folder** — recordings show their containing folder.
- **Messages: read/delivered receipts** ("Read <time>" / "Delivered" under sent
  bubbles), **tapbacks/reactions** (add/remove folded into a "❤️×2 👍" badge,
  incl. custom emoji), and **inline replies** (a quoted preview above the reply).
- **Safari: deleted-history tombstones** — cleared URLs surface in the History
  list flagged deleted (trash icon + strikethrough).

### Fixed

- **Voice-memo titles** — read `ZENCRYPTEDTITLE` (plaintext locally, on every
  row) so all memos show their real name, not just the ~276 with a composition
  manifest.
- **Notes creation dates** — COALESCE the suffixed Core Data date columns so a
  present-but-NULL `ZCREATIONDATE1` no longer shadows the populated
  `ZCREATIONDATE3` (was NULL on every note).
- **`safari_bookmarks.rs`** items-after-test-module and a `manual_is_multiple_of`
  lint (pre-existing, blocked `clippy -D warnings`).

### Known

- **Locked-note decryption is broken** and unfixed: the ciphertext is read from a
  nonexistent column and the AES-key-unwrap step is missing. A correct fix needs
  validation with a real note password.

## [0.8.0] — 2026-07-15

### Added

- **Native TikTok DM messages**, validated on a real backup («redacted» messages). Parsed
  from `ChatFiles/<uid>/db.sqlite` (`TIMMessageORM`) — a *separate* DB from the
  `AwemeIM.db` social graph — with sender names resolved from `AwemeContacts*`. The
  `-wal` sidecar is extracted alongside each DB so unflushed rows are replayed.
- **Typed markers for non-text TikTok messages.** Shared videos, stickers, nudges
  and profile cards (whose payloads live only on TikTok's servers) surface as
  labelled markers instead of blank bubbles, and each carries a content `kind`.
- **Message content-kind filter** in the open conversation — clickable badges
  (text / link / media / shared / sticker / system) showing only the kinds actually
  present. Threaded through SQL, the Tauri commands and the cache (schema v11 adds
  `messages.kind`).
- **Friendlier voice-memo titles** — read from each recording's
  `.composition/manifest.plist` (`RCSavedRecordingTitle`), falling back to the DB
  label then the filename, instead of the cryptic folder name.
- **Message image/video attachments now appear in the Photos gallery** (mirrored
  into `media_items` with source `Messages`).
- **UI density setting** (Comfortable / Cozy / Compact) — "True Density": fonts and
  icons keep their size, only spacing tightens. A rows-icon toggle in the top bar
  cycles the levels; list rows, the Timeline and chat bubbles all respond.
- **Time-range + search filters** extended to Contacts, Calls and Recordings, matching
  Photos / Safari / Notes.

### Changed

- **Shared `PanelHeader` header** across every list view (title · count · filter
  badges / search / toolbar). Master-detail views (Contacts, Recordings, Notes,
  Messages) now put the full-width header across the top with the list+detail split
  below it, instead of a header trapped in the narrow master column.
- **All filter chips are now `BadgeFilter` badges** and **never wrap** — they scroll
  horizontally when the window is narrow, so filters can't push the header taller.
  The time-range period chips got the same no-wrap treatment.
- **Import progress** now separates the *Indexing* phase from import, restarts at 0%,
  shows a right-aligned `step n/N`, and uses Title-Case entity labels.
- **Appearance toggle** in the top bar is a single button that cycles
  System → Light → Dark (lucide `sun-moon` for system); also surfaced in Settings.
- **Settings dialog** redesigned to a fixed-size, macOS-System-Settings-style layout
  with a vertical tab rail.
- Selection, active filter and sort order now **persist across navigation and restarts**
  for every view (`usePersistedState`).
- Removed the redundant single-field "Time" sort picker in Messages (a direction
  toggle replaces it).

### Fixed

- **Stale persisted filters can no longer strand a view empty.** Photos' source and
  Notes' folder/lock filters are clamped to what the *current* backup actually has,
  so a choice carried over from another backup falls back to "all" instead of leaving
  an unrecoverable empty grid.
- A `?service=` deep-link into Messages now applies **once** per value instead of
  snapping the filter back on every refetch.
- Recordings show a distinct "no matches" message when a search/time filter excludes
  every recording (vs. "no recordings in this backup").
- TikTok message parsing reads `content`/`chat_key` tolerantly, so a single odd row
  (BLOB content, numeric group id) no longer aborts the whole account.

## [0.7.0] — 2026-07-15

### Fixed

- **Opening an encrypted backup no longer needs a second "Open" click.** After the
  password step the backup is now marked active optimistically, so the target
  view no longer reads a stale "no backup open" state and bounce back to the
  picker (queries use `staleTime: Infinity`).
- **Photos source filter no longer breaks on a narrow window** — the pills scroll
  horizontally within the title row instead of wrapping out of it. The long
  "iTunes Backup - Installed Applications" source is shortened to "iTunes Backup"
  (and its numbered variants collapse into one).
- Filter/header **item counts are now smaller and dimmer** across all views, so
  they read as secondary to the labels they annotate.

### Changed

- **Timeline rows redesigned** to a single flat line — avatar · direction · message ·
  app icon · time — with the message free to wrap over multiple lines. The
  always-the-owner conversation phone number is gone, and the source app is now
  just its brand icon (no "iMessage"/etc. text), pinned to the left of the time.
- **Timeline rows now show the conversation partner and direction.** The avatar is
  always the other party (so every row makes clear which chat it belongs to, even
  your own outgoing messages), and a direction arrow marks sent (→, tinted) vs
  received (←). Backed by a new `threadHandle` on each timeline row.
- **All large counts now use a thousands separator** (`450 897`, non-breaking) so
  they read clearly and never wrap mid-number.

### Added

- **Native TikTok contacts / social graph** (`AwemeIM.db`), the last artifact
  that had needed iLEAPP. A default import is now **fully native** — it launches
  no iLEAPP subprocess and doesn't require the engine installed, cutting a full
  import from minutes to ~35s. iLEAPP is kept only as a development reference for
  schemas we can't inspect in our own backup; the engine code path stays dormant.
- **Photo metadata from `Photos.sqlite`** — a native parser enriches each
  camera-roll photo with the **people** in it (face recognition), a precise
  **capture date**, **GPS location**, and its **favorite** flag. Photo search
  matches person names; the lightbox shows who's in a photo, its coordinates, and
  a favorite heart; tagged/favorited thumbnails carry small badges.
- **Search in Notes** — a full-width search row over title / snippet / folder,
  alongside the folder, lock, time, and sort filters.
- **Search rows for Photos, Messages, and Safari.** Photos gets a full-width
  filename search; the Messages timeline gets a full-width search over body /
  sender / conversation; Safari's search moved to its own full-width row. All
  compose with the existing time filter and sort. (Photos person/face tags aren't
  parsed yet — a future `Photos.sqlite` parser — so photo search is filename-only
  for now.)
- **Native Safari bookmarks, reading list, and open tabs.** New parsers read
  `Bookmarks.db` (bookmarks + reading-list items, with their added/viewed dates
  and preview text) and `SafariTabs.db` (open tabs, grouped by tab group). The
  Safari view gains a **type filter** on the title row — History · Bookmarks ·
  Reading List · Tabs — with the same search + time filter + sort across all of
  them.
- **Back button** in a conversation opened from the Timeline view, returning you
  to that overview.
- **Timeline time filters.** Merged the separate Periods view into Timeline: the
  toolbar now carries quick-filter chips (All · 24h · 7d · 30d · year, each with
  its message count) plus a custom from–to date range, left-aligned beside the
  sort control. Selecting a chip or range filters the stream (rather than the old
  jump-to-bucket behaviour).

- **Time filters on Photos, Notes, and Safari.** The same preset chips + custom
  from–to range as the Timeline now filter Photos (by capture date, server-side),
  Notes (by modified date), and Safari history (by visit date, server-side). On
  Photos the app/source filter moved up beside the title; on Notes the time chips
  replace the old year dropdown.
- **Notes layout** rebuilt into full-width rows: title + folder + lock state (now
  with lock/unlock icons) on the first row, time filters + sort on the second.
- **Brand icons on the Photos source filter** (same treatment as the message
  filter chips).

### Removed

- The standalone **Periods** view (folded into Timeline's filters, above).
- The Notes **year dropdown** (superseded by the time-filter chips + range).

## [0.6.3] — 2026-07-15

### Fixed

- **Message views no longer stick while scrolling.** The lazy virtual list was
  measuring not-yet-loaded placeholder rows as their true height, collapsing the
  total size and then re-expanding each row as its window resolved — the jump
  that made Timeline/Periods/conversation scrolling feel frozen. Unloaded rows
  now reserve their estimated height and are never measured; only real content
  is. Also disables the browser's own scroll-anchoring so it can't fight the
  virtualizer.
- **Timeline & Periods now show which conversation each message belongs to.**
  Rows led with the sender only; the conversation is now the primary label
  (making clear who a 1:1 was with / which group), with the sender shown as a
  prefix on the snippet for your own and group messages.

### Added

- **Sort messages by time direction** (oldest-first ↔ newest-first) in the
  Timeline, Periods, and conversation views — previously only the conversation
  *list* could be sorted. Newest-first pins the newest message to the top;
  oldest-first keeps the chat-like newest-at-bottom layout.

## [0.6.2] — 2026-07-15

**Real brand logos across all app and message surfaces.** Official brand marks
(simple-icons) replace the placeholder emoji everywhere an app or service is
shown — Apps rows, Messages filter chips, thread-list rows, the conversation
header, and per-message service badges — rendered as inline SVG so the
asset-blocking CSP is satisfied. Near-monochrome marks (X, TikTok, iMessage on
dark) fall back to `currentColor` to stay visible in both themes; apps without a
brand mark (imo, Teams, SMS) get a clean monogram tile.

## [0.6.1] — 2026-07-15

A review-and-hardening point release after LinkedIn (0.6.0).

### Changed

- **Faster imports** — iLEAPP no longer re-parses first-party data the native
  parsers already read.
- **Settings dialog split into tabs**, instead of one overloaded pane.

### Fixed

- The **"Extract" action is gated** for apps already parsed natively, and app /
  service rows show brand icons.
- Message-attachment images render correctly when the attachment has a **NULL
  mime type** or comes from an **encrypted backup**.

## [0.6.0] — 2026-07-15

### Added
- **LinkedIn** (`Documents/msg_database.sqlite`) — messages grouped by
  `conversationUrn`; sender, direction (`distance == "SELF"`), and body decoded
  from the `serializedMessage` JSON; the chat name from the non-owner participant
  in `serializedConversation`. Unvalidated against a real backup; behind the
  iLEAPP fallback.

## [0.5.0] — 2026-07-15

Two more native third-party chat apps via the app-chat framework, each
code-reviewed and hardened. Both unvalidated against a real backup; behind the
iLEAPP fallback.

### Added
- **Viber** (`com.viber/database/Contacts.data`) — messages, conversation
  grouping, per-author group attribution, direction, attachment flag. Uses
  `ZSTATEDATE` (creation) for the timestamp and infers direction robustly
  (including failed sends).
- **Microsoft Teams** (`SkypeSpacesDogfood/*/Skype*.sqlite`) — messages with
  per-author group attribution; HTML content reduced to plain text (recovering
  emoji `alt` text); `ZTHREADTOPIC` group titles.

## [0.4.0] — 2026-07-14

Four more native third-party chat apps via the app-chat framework. All are
unvalidated against a real backup and sit behind the automatic iLEAPP fallback.

### Added
- **Telegram** — a native reader for its binary "postbox" store
  (`postbox/db/db_sqlite`): a bounds-checked byte reader, the `t7` message parse
  (text/author/timestamp/direction), and a minimal `PostboxDecoder` for peer
  names from `t2`. Media payloads aren't decoded.
- **Kik** (`kik.sqlite`) — messages, direction (`ZTYPE`), and group detection via
  the group `ZJID`. Group per-author isn't in this schema, so a group is titled
  but its messages carry no author (as with iLEAPP).
- **imo** (`IMODb2.sqlite`) — messages with correct **per-author group
  attribution** via `ZALIAS`; nanosecond timestamps.
- **Threema** (`ThreemaData.sqlite`) — messages with per-member group attribution
  via `ZSENDER` (named and unnamed groups); system messages excluded.

### Fixed
- Each app was code-reviewed and hardened before release: group chats are no
  longer mislabeled as 1:1 or mis-attributed (Kik/imo/Threema), a new shared
  `col_i64` reads large integer timestamps without f64 precision loss, and
  storage-class-tolerant column reads prevent one odd row from aborting a parse.

### Notes & caveats
- Telegram/Kik/imo/Threema native output is unvalidated against real backups;
  all fall back to iLEAPP on any parse miss.
- iLEAPP remains required for the long tail (Viber, Discord, Slack, Teams, etc.).

## [0.3.0] — 2026-07-14

Native-first, Batch 1: every built-in view now materializes without iLEAPP, and
third-party chats gain native parsers behind a pluggable app-module framework.
iLEAPP still runs for what isn't native yet (Telegram, TikTok's contact graph,
and the long tail), so this is the first batch of the migration, not its end.

### Added
- **Native Calls, Safari & Contacts (no iLEAPP).** Call history
  (`CallHistory.storedata`), Safari history (`History.db`), and Contacts
  (`AddressBook.sqlitedb`, self-extracted) now materialize via native parsers
  through the ManifestIndex, with iLEAPP kept as automatic fallback. Calls and
  Safari also gained sidebar re-import actions. **All first-party views are now
  native.** (Apps was already native from `Info.plist`.)
- **Native third-party chat framework** (`parsers/apps/`). Each app is a small
  module — locate its DB, parse it into a shared message stream — and one shared
  inserter builds the same threads/messages the Messages view renders. Adding an
  app is one module file plus a registry entry.
  - **WhatsApp** (`ChatStorage.sqlite`) and **Facebook Messenger**
    (`lightspeed-userDatabases/*.db`) — native, validated by synthetic fixtures.
  - **Instagram** (`DirectSQLiteDatabase/*.db`) and **TikTok** (`AwemeIM.db`) —
    native but **not yet validated against a real backup**, so they stay behind
    the automatic iLEAPP fallback.
- **NSKeyedArchiver decoder** (`crate::nska`) — resolves Apple keyed-archive
  blobs (used by Instagram DMs); a reusable, standalone iOS-forensics primitive.
- **Living coverage docs** — `docs/reference/app-support.md` (native vs iLEAPP per app) and
  `docs/reference/app-data-coverage.md` (field-level: what each DB holds vs. what we
  surface). Includes research notes on Snapchat / X / Facebook local stores.

### Fixed
- Hardening from a multi-agent code review of the native work: the
  NSKeyedArchiver decoder no longer hangs or panics on a crafted/cyclic archive
  (memoized graph resolution, guarded date conversion); 1:1 Messenger/Instagram
  chats are no longer mislabeled "Group chat"; per-app import counts are folded in
  only after commit; a schema-drifted third-party DB falls back to iLEAPP instead
  of silently dropping messages; several column reads are storage-class-tolerant.

### Notes & caveats
- Instagram & TikTok native output is unvalidated against a real backup; both
  degrade to iLEAPP on any parse miss. TikTok's contact social-graph still comes
  from iLEAPP.
- iLEAPP remains required (Telegram, TikTok contacts, long-tail apps). Making it
  optional is a later batch.

## [0.2.0] — 2026-07-14

The native lazy-decode core, wired into the import — plus password-protected and
pinned Notes, richer Notes browsing, and a reworked re-import UX. iLEAPP still
runs (it supplies Calls, Safari, Apps, and third-party chats); replacing it is
the batched 0.3.0+ migration under "Planned" below.

### Added
- **Native Messages, Notes, Recordings & Camera roll.** The import materializes
  these natively from the backup via a reusable `ManifestIndex` (decrypt-on-
  demand: resolve `domain/relativePath` → file + key, read one file). Messages
  come from `sms.db`; Notes from `NoteStore.sqlite` (body gzip-inflated from
  `ZICNOTEDATA.ZDATA`, text walked out of the `NoteStoreProto` wire format,
  Core Data columns schema-introspected); Recordings from `CloudRecordings.db`
  with `.m4a` streamed over a `traceloupe-audio://` scheme (Range-seekable,
  decrypted at play time). iLEAPP stays the automatic fallback when a source DB
  is absent or a native parse fails.
- **Locked (password-protected) Notes.** Detected via `ZISPASSWORDPROTECTED` /
  `ZENCRYPTEDDATA`; shown with a lock icon and unlocked on demand with the note
  password (PBKDF2 → AES-128-GCM), the plaintext held only in session, never at
  rest.
- **Notes filters & date grouping.** Filter by folder, year, and locked state;
  the list groups into Pinned + recency sections (Today, Yesterday, Previous 7/30
  Days, months, years), matching the Notes app. Parses `ZISPINNED`.
- **Re-import moved to the sidebar.** Per-data-type re-import is now an action on
  each nav item, with a spinner that survives navigation (state lifted above the
  routes); a cancelled re-import no longer destroys the previous import (atomic
  temp-cache swap).
- **Touch ID (opt-in) + signing detection.** An encrypted backup's Keychain
  password can be gated behind Touch ID; the app detects whether it's stably
  signed and enables the toggle accordingly (see `docs/reference/signing.md`).

### Notes & caveats
- Native Messages/Notes/Recordings/Camera-roll run *in addition to* iLEAPP's
  passes, so import time isn't reduced yet — that lands with the 0.3.0
  first-party migration.
- Locked-note AES-GCM decryption and `ZISPINNED` parsing are unit-tested but
  pending validation against a real backup that contains such notes.

## Planned

The **native-first migration is complete** — every surfaced artifact, first- and
third-party, is parsed by an in-house Rust parser, and iLEAPP is no longer run at
all (kept only as a development-time schema reference; the sidecar path is
dormant). The earlier "make iLEAPP optional, in batches" plan has therefore been
fully delivered and superseded; the remaining backlog is about *depth*, not
removing iLEAPP. Tracked in detail in [`docs/reference/app-data-coverage.md`](docs/reference/app-data-coverage.md)
(field-level) and [`docs/reference/app-support.md`](docs/reference/app-support.md) (per-app).

- **Field-level coverage gaps** — the highest-value unsurfaced fields: Messages
  full per-edit history (`message_summary_info`) and group-action rows; Notes
  inline image/drawing rendering; Photos Live/burst grouping; the Contacts
  relationship graph and groups.
- **Untapped stores** — Keychain (presence + counts only, never values), the
  Apps-view install metadata (version / install date / seller), and Health raw
  samples + GPS routes (only workouts are surfaced today).
- **More third-party apps** — the ⬜ Planned tiers in `app-support.md` (YouTube,
  Gmail, WeChat, Discord, Reddit, Spotify, …), plus two that need a real backup to
  pin their schema (Snapchat, X/Twitter). A single generic **`Cache.db`** module
  could surface cached network content across many apps at once — a strong future
  addition.
- **App-chat attachment media** — the framework has landed; individual parsers
  must still *emit* their attachments (WhatsApp/Kik/Threema/TikTok media),
  deferred until a backup containing that media exists to validate against.
- **Validation debt** — several app parsers (Instagram, Telegram, Kik, imo,
  Threema, Viber, Teams, LinkedIn) are marked *unvalidated* pending a real backup
  with those apps installed.

## [0.1.0] — 2026-07-13

Initial baseline. Open, decrypt, and browse iPhone backups entirely on-device.

### Added
- Discover and open encrypted or unencrypted iPhone backups; first-time import
  via a bundled, checksum-pinned iLEAPP engine, then instant re-open from cache.
- Native, hardware-accelerated backup decryption (keybag → class keys → AES-CBC);
  camera roll read natively with on-demand full-image decryption and cache-once.
- Views: Messages (conversations, cross-conversation timeline, per-year periods),
  Photos (virtualized gallery + full-viewport lightbox with keyboard nav),
  Contacts, Calls, Safari, Notes, and installed Apps.
- Third-party chats surfaced in Messages (TikTok, WhatsApp, Telegram).
- Per-list sorting (field + direction), a 24-hour clock option, resizable and
  icon-rail sidebar, always-visible scrollbars.
- Security: key zeroization, a "forget backup" flow, `backup_id` validation, and
  hardened media serving.
