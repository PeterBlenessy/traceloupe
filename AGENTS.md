# Agent ground rules

Multiple AI agents work on this repo **on the same machine, from the same
clone**. Branches alone do **not** isolate them — a git checkout swaps the whole
working tree, and `target/` / `node_modules` are shared, so two agents editing
or building at once collide and can lose uncommitted work. These rules keep each
agent in its own lane.

> **The one rule that prevents everything else: work in your own worktree, never
> in the shared main checkout.**

## Start every task in your own worktree

Before editing or building anything, work in an isolated worktree whose **branch
name equals its directory name**. One helper handles both cases:

```bash
scripts/agent-worktree.sh <name>
```

- If `<name>` is a **new** task → creates branch `<name>` off `origin/main` in
  `.claude/worktrees/<name>`.
- If `<name>` is an **existing** branch (local or on origin) → checks that branch
  out into `.claude/worktrees/<name>` instead of creating a new one.

`.claude/worktrees/` is gitignored, so worktrees never show up in `git status`.
Then `cd` into the worktree and do all your work there.

Claude Code users can also use the built-in `EnterWorktree` tool — the naming
convention still applies.

### New task (no branch yet)

```bash
scripts/agent-worktree.sh my-task-slug     # branch + dir both "my-task-slug", off origin/main
```

Wraps `git worktree add .claude/worktrees/<slug> -b <slug> origin/main`.

### Picking up an EXISTING branch

If the work is on a branch that already exists — one that was handed to you,
renamed, or left mid-flight (e.g. `feature/icloud-offloaded-media`) — do **not**
create a new branch. Check the existing one out into a matching worktree:

```bash
scripts/agent-worktree.sh feature/icloud-offloaded-media   # detects it exists, checks it out
# equivalently, by hand:
git fetch origin
git worktree add .claude/worktrees/feature/icloud-offloaded-media feature/icloud-offloaded-media
```

(A branch with slashes just nests the worktree dir — name and branch stay
identical.) Then get current before you start, and re-verify you're isolated:

```bash
git merge origin/main       # (or rebase, per the branch's convention)
git branch --show-current   # must be the branch you were handed
```

A branch can be checked out in only **one** worktree at a time, so if this errors
with "already checked out", another agent already owns it — coordinate, don't
force.

## Naming

- **`<slug>` is kebab-case and describes the task**: `messages-stickers`,
  `spyware-ioc-engine`.
- **Branch name == worktree directory name.** One slug, used for both. If you
  want a type prefix, put it in *both* (`feature/foo` → branch `feature/foo`,
  dir `.claude/worktrees/feature/foo`) so they stay identical.

## While you work

- **Stay on your branch in your worktree.** Never `git checkout <other-branch>`
  inside a worktree to peek at other work — that is the collision. One worktree,
  one branch, for its whole life.
- **Only touch your own worktree and branch.** Don't edit files, commit, rebase,
  reset, or delete branches that belong to another agent. Don't run `git clean`,
  `checkout -f`, or `reset --hard` outside your worktree.
- **Base off `origin/main`** (or the agreed integration branch), not off whatever
  the shared checkout happens to be sitting on.
- **Don't reinvent shared components; build on what's in flight.** Before writing
  a new view, a UI control, or a shared helper, check whether it already exists:
  grep `src/components/`, and read the relevant doc (**`docs/reference/ui.md`** for anything
  with a header, filter, sort, search, or a new view). Then `git fetch` and skim
  `origin/main` **and open PRs** (`gh pr list`) for related work — a big pattern
  may be mid-migration on another branch, and you want to adopt/extend it, not
  re-create the old thing beside it. (This is exactly how the two scan views ended
  up hand-rolling their own header bar while every other view moved to the shared
  toolbar: they were built while that migration was still on a separate branch.)
- **Rebase on `origin/main` before you finish** a longer-lived branch, and
  re-check that any shared pattern you touched hasn't changed on main since you
  branched — if it has, migrate onto it rather than shipping the stale shape.
- **Commit early and often**, and **push right after your first commit**
  (`git push -u origin <slug>`). A branch that lives only on this laptop is
  unbacked-up — if the folder is clobbered it's gone. Everything on GitHub is
  safe.
- **Builds are per-worktree.** Each worktree has its own `target/` and its own
  `node_modules` (run `pnpm install` once in it). Do **not** point multiple
  worktrees at a shared `CARGO_TARGET_DIR` — they'd contend on the build lock.
- **Don't trust the shell's working directory to stay in your worktree.** The
  shell's cwd is **not** pinned to your worktree — a `cd` elsewhere (a temp dir,
  a `venv`, a build cache) can drop the *next* command back in the shared main
  checkout, which is sitting on a **different branch**. Commands that use
  relative paths then silently read — and `sed -i` / `>` silently edit — that
  other branch's files. Guard against it:
  - Prefer **absolute paths rooted at your worktree** for every read, edit,
    `grep`, and in-place `sed`/redirect. Don't lean on relative paths after
    you've `cd`'d away.
  - After any `cd` out of your worktree — or whenever unsure — run `pwd` (and
    `git branch --show-current`) before touching files again.
  - Treat a **content mismatch** as a location bug, not a real change: an
    unexpected version number, CHANGELOG entries you didn't write, or a file
    that looks "reverted" almost always means you're reading the shared checkout
    on another branch. Stop and re-check `pwd` before editing or committing.

## Verify your isolation before the first edit

```bash
git rev-parse --show-toplevel   # must be your .claude/worktrees/<slug>, NOT the shared main checkout
git branch --show-current       # must be your <slug>
git worktree list               # see who else is where
```

If `--show-toplevel` is the plain repo root, stop and make your worktree first.
Re-run these (or at least `pwd`) any time you return from a `cd` elsewhere — the
shell can silently land you back in the shared checkout on another branch (see
"Don't trust the shell's working directory" above).

## The shared main checkout

The top-level clone (`iphone-backup-analyzer/`) is the **canonical repo**, not a
dev sandbox. Don't develop directly in it. Leave whatever branch it's on alone;
create a worktree instead.

## Never touch the user's real backup data

The user's iPhone backup is **private data and off-limits.** Do not read it, and
do not record anything derived from it (counts, sizes, names, statistics) in
notes, commits, issues, or agent memory.

Off-limits, concretely:

- the backups Finder writes (`~/Library/Application Support/MobileSync/…`),
- the decrypted mirror at `~/.traceloupe-dev/backup-mirror` and
  `scripts/dump-backup.sh` that produces it,
- any `caches/<backup_id>/` (`cache.db`, `analysis.db`) built from their device.

**Validate with data you are allowed to have**, in this order:

1. **Generate a fixture.** `tools/make_fixture_backup.py` produces a valid
   *encrypted* iOS backup — real keybag → class-key → `Manifest.db` →
   per-file-blob crypto — in a few KB. Extend it with the artifact your task
   needs. For DB-level work, hand-seed the schema in an in-memory SQLite, as the
   `analysis.rs` / `cache.rs` tests already do.
2. **Public DFIR corpora.** iLEAPP's bundled per-artifact fixtures
   (`admin/test/cases/data/…`) and the public Hickman/CTF iOS test images are
   published research data — realistic *and* allowed. Keep what you use as a
   committed fixture.
3. **Never** the user's own backup, mirror, or caches.

Apple's *schema* (table/column layouts, enum meanings) is public DFIR knowledge
and fine to write down. The user's *data* is not. If a claim genuinely cannot be
checked without real data, say so and state what your synthetic coverage does
prove — don't quietly validate against their backup, and don't ask them to
eyeball it in place of a test you could have written.

## Done means shipped

**A task is finished when the PR is open, not when it's understood.** Definition
of done: implemented → tests written → the full [CI gate](#project-specific-notes)
green locally → committed → **pushed** → PR opened. Then start the next item;
don't stop to ask whether to continue.

**Decide, document, ship — don't escalate a judgment call.** If a task needs a
choice (which strategy, which trade-off), pick one, implement it, and write the
reasoning into the PR body or an ADR. A PR the user can reject costs them a
minute; a question blocks them until they next sit down.

**Answer empirical questions with tests, not prompts.** "Is it slow?", "does the
cache hold?", "does the migration work?" are all measurable — build the fixture
and measure. Before asking the user anything, check whether a fixture could have
answered it; if so, that's your job, not theirs.

**Only two things justify stopping short:**

1. **Externally blocked** — an upstream fix must land first, a credential or
   artifact you cannot synthesise is missing.
2. **Unsafe or destructive** — proceeding risks data loss or an irreversible,
   outward-facing action.

Anything else deferred is an unfinished task, not a status update. Say which of
the two applies, specifically. If scope must shrink, finish everything else in
full and state exactly what was left and why.

## Background work must outlive the UI

**Anything that outlives a single command call needs a status snapshot and a
mount-time re-attach. The UI must never be the only place its state exists.**

Work that runs in the Rust process — a scan, an import, a download — survives a
webview reload. The React state describing it does not. Without a way to
re-attach, a reload leaves an idle-looking UI over work that is still running,
and any gate around that work then rejects the user's retry while the original
is still going.

This was found the hard way five times (#69, #72): Safety Scan, import,
re-import and Security Check all shipped without it; only the model download had
it, and its own comment explained exactly why. So, for any new background job:

1. **Snapshot its last progress** in managed state (`Mutex<Option<Event>>`), and
   expose a `get_*_status` command.
2. **One helper owns both the emit and the snapshot.** They must not be updated
   separately — a snapshot maintained at only *some* emit sites is worse than
   none, because the UI then re-attaches to a stale phase. Security Check had
   four emit sites for one event; Safety Scan had six.
3. **Clear it on every exit path.** Use an RAII guard when the command has
   several `?` returns — scattered clear calls eventually miss one and strand
   finished work in the UI.
4. **Re-attach on mount** in the provider, then subscribe — sharing one
   subscribe function with the start path so both attach the same listener.
5. **Surface it in the toolbar activity indicator** (`activity-indicator.tsx`) by
   adding an entry to `useActivities` — not another toolbar pill.

## Measure the UI, don't eyeball it

**A screenshot tells you it rendered. It does not tell you it is right.** Every
visual inconsistency this project has shipped was looked at first and accepted:
hover actions covering a count pill, toolbar controls packed 2px apart, three
different island heights in one toolbar (30 / 36 / 38px). Each looked fine in
isolation, which is precisely how they accumulate.

So after any UI change, read the numbers:

- **Computed geometry, not the render.** `getBoundingClientRect().height` for
  every control in the region; the resolved value of the token that is supposed
  to drive it; the set of distinct font sizes on the page. Compare like with
  like — every island, every segment, every row.
- **The states an idle screenshot never shows.** Filter applied, search
  expanded, hovered, empty, at the smallest and largest text size. A control
  that *changes* size between states is worse than one that is merely wrong, and
  only a state sweep finds it.
- **When a rule breaks twice, make it executable.** `scripts/check-design.mjs`
  (in CI) and `check-virtualization.mjs` exist because writing the rule down a
  second time did not work. Run the design lint after any UI change; it measures
  type, control heights, island geometry, overlap and clipping across five views
  and both text extremes. Prove a new check can fail before trusting it — the
  design lint does this to itself on every run, injecting a violation per rule
  and failing if any detector stays quiet.

## Pick the right IPC primitive

**Events are for one-off notifications. Streams use a Channel. Bulk data is
paginated or raw bytes.** Two incidents came from getting this wrong (#60, #61),
and both were found in production rather than in review (#65).

Tauri's own guidance is that the event system "is not designed for low latency or
high throughput situations", and that rapid events delivered to an async listener
**may be processed out of order** — for a progress stream that is a correctness
bug, not a performance one.

| What you have | Use |
|---|---|
| A rare, small notification ("the theme changed") | `emit` |
| An ordered stream from a background job (progress, logs) | `tauri::ipc::Channel`, via `stream::ProgressStream` |
| A collection that grows with the backup | a command with `offset`/`limit`, plus a separate `count_*` |
| Bytes (media, exports) | the custom protocol or `ipc::Response`, never JSON |

Notes that are easy to get wrong:

- **A `ProgressStream` holds one channel slot.** These streams are
  single-producer, single-consumer: one job emits, one React provider consumes
  and fans out through context. Two components subscribing to the same stream
  means the second silently steals it — fan out in the provider instead.
- **A stream carries updates, not state.** Anything sent before the UI subscribes
  is dropped, which is why every stream pairs with the `get_*_status` snapshot
  described above.
- **Convert every emit site for a stream, including the error path.** A partial
  conversion leaves failures reaching nobody, which looks exactly like a hang.
- **A command returning `Vec<T>` must be windowed or declared bounded.**
  `src-tauri/tests/command_bounds.rs` fails the build otherwise, and its `BOUNDED`
  list is the inventory: every unwindowed collection command with the reason it
  cannot grow, and a measured size where it can. Write what *caps* the
  collection — "it's small" is not a bound.

## A check is not trusted until it has been seen failing

Almost every real defect found in this repo lately was found by making a check
fail on purpose — not by reading code and judging it correct. So when you add a
guard, a lint rule or a test, **break something and watch it fail before you
trust it**, and name the test after the defect it catches.

There are two ways a check goes quiet, and they need different answers:

- **A weak assertion** — the code could be wrong and nothing fails.
  `cargo mutants --file <path> -p traceloupe-core -- --lib` answers this
  directly: it changes the code and reports the changes no test noticed. A trial
  over `dashboard.rs` took 15 minutes and found seven, in a file written with
  deliberate guard tests. Too slow for every PR; run it when you add logic worth
  guarding, and write tests from its report rather than from imagination.
- **A blind check** — it runs, observes nothing, and reports success. This is the
  more common one here: the design lint reported "Safety" while a dialog kept it
  on Security; the home view was measured before its data arrived and a planted
  violation passed; the mock omitted five modules so those tiles were never on
  screen. Mutation testing cannot see any of that. The answer is that **a check
  states what it observed and fails when it observed too little** — see the
  `coverage` rule in `check-design.mjs` and `check-mock-parity.mjs`.

Both scripts prove their own matchers on every run. If a detector stops firing
on a deliberate violation, that is a failure in itself — a rule that cannot fail
is indistinguishable from a rule that passes.

## Finishing up

- Open a PR (or hand off) once CI-clean and the branch is pushed.
- When your branch is merged or abandoned, clean up **with the script**:
  ```bash
  scripts/agent-cleanup.sh <slug>            # refuses if anything would be lost
  scripts/agent-cleanup.sh <slug> --force    # …and this is how you say "lose it"
  scripts/agent-cleanup.sh --list            # every worktree: dirty? merged? unpushed?
  ```
  It removes the worktree, the local branch and the remote branch, is idempotent,
  and gates every destructive step on a check that names what it found. Doing it
  by hand is three commands whose failure modes are silent, and it has two ways
  to hurt in a repo several agents share:

  - **Removing the worktree your shell is standing in** leaves the shell with no
    working directory, and the next relative path resolves somewhere else or not
    at all. A script cannot chdir its caller, so this refuses up front and prints
    the command to run instead. (This one is not hypothetical — it happened while
    cutting 0.34.0.)
  - **`git worktree remove` discards uncommitted work and `git push --delete`
    discards unpushed commits**, both silently. Another agent's hours can be in
    there.

## Project-specific notes

- Stack: Tauri + Rust (`crates/traceloupe-core`, `src-tauri`) + React (`src/`).
- **System settings: read `docs/reference/macos-integration.md` before touching
  anything that follows the OS** (accent, appearance, text size, keyboard
  navigation, the accessibility display options). It records what macOS actually
  exposes, what WebKit does *not* give us for free, and the traps that make a
  setting look adopted when it is not. The decision to follow the system rather
  than add app preferences is ADR 0004.
- **UI / views: read `docs/reference/ui.md` before building or changing any view.** Every
  view surfaces its title, filters, sort and search through ONE shared top toolbar
  (`useViewToolbar`) — there are no per-view header bars. Don't hand-roll headers,
  filter popovers, time pickers, or pill rows: the shared components already cover
  it (`FilterControl` + `badgeGroup`/`timeGroup`/`multiBadgeGroup`, `SortControl`,
  `ListSearch`, `NoBackupState`, `VirtualListView`/`LazyListView`/`ListDetail`).
- **Every button gets a tooltip — no exceptions.** Wrap it in the shadcn
  `Tooltip` (`components/ui/tooltip.tsx`); icon-only buttons especially, and a
  disabled button's tooltip must say *why* it's disabled. The app is already
  inside a `TooltipProvider`, so no wiring is needed. See "Buttons always have a
  tooltip" in `docs/reference/ui.md`.
- Verify a change builds the **binary**, not just `cargo check`:
  `cargo test -p traceloupe-core && cargo build -p traceloupe && pnpm exec tsc --noEmit`.
- Parser changes need a **re-import** to populate existing caches (the cache
  migration only creates the empty structures; bump `SCHEMA_VERSION` in
  `crates/traceloupe-core/src/cache.rs`).
- Domain glossary: `docs/CONTEXT.md`. Field-level data-coverage roadmap:
  `docs/reference/app-data-coverage.md`.
- **Cutting a release: follow [`RELEASING.md`](RELEASING.md).** Never bump the
  version by hand-editing one manifest — run `scripts/release.sh X.Y.Z` (bumps
  `package.json` + workspace `Cargo.toml` + `tauri.conf.json` + `Cargo.lock`
  together), add the `## [X.Y.Z]` CHANGELOG section (a new minor opens with a
  one-line bold milestone summary), and run `scripts/check-releases.sh`. The
  `vX.Y.Z` tag is created
  automatically on merge to main — don't tag by hand. CI fails a bump that has
  no CHANGELOG entry.
