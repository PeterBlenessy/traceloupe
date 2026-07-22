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
  grep `src/components/`, and read the relevant doc (**`docs/ui.md`** for anything
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

## Finishing up

- Open a PR (or hand off) once CI-clean and the branch is pushed.
- When your branch is merged or abandoned, clean up:
  ```bash
  git worktree remove .claude/worktrees/<slug>
  git branch -d <slug>          # -d refuses unless merged; that's the safety net
  git worktree prune            # drop stale worktree admin entries
  ```

## Project-specific notes

- Stack: Tauri + Rust (`crates/traceloupe-core`, `src-tauri`) + React (`src/`).
- **UI / views: read `docs/ui.md` before building or changing any view.** Every
  view surfaces its title, filters, sort and search through ONE shared top toolbar
  (`useViewToolbar`) — there are no per-view header bars. Don't hand-roll headers,
  filter popovers, time pickers, or pill rows: the shared components already cover
  it (`FilterControl` + `badgeGroup`/`timeGroup`/`multiBadgeGroup`, `SortControl`,
  `ListSearch`, `NoBackupState`, `VirtualListView`/`LazyListView`/`ListDetail`).
- **Every button gets a tooltip — no exceptions.** Wrap it in the shadcn
  `Tooltip` (`components/ui/tooltip.tsx`); icon-only buttons especially, and a
  disabled button's tooltip must say *why* it's disabled. The app is already
  inside a `TooltipProvider`, so no wiring is needed. See "Buttons always have a
  tooltip" in `docs/ui.md`.
- Verify a change builds the **binary**, not just `cargo check`:
  `cargo test -p traceloupe-core && cargo build -p traceloupe && pnpm exec tsc --noEmit`.
- Parser changes need a **re-import** to populate existing caches (the cache
  migration only creates the empty structures; bump `SCHEMA_VERSION` in
  `crates/traceloupe-core/src/cache.rs`).
- Domain glossary: `CONTEXT.md`. Field-level data-coverage roadmap:
  `docs/app-data-coverage.md`.
- **Cutting a release: follow [`RELEASING.md`](RELEASING.md).** Never bump the
  version by hand-editing one manifest — run `scripts/release.sh X.Y.Z` (bumps
  `package.json` + workspace `Cargo.toml` + `tauri.conf.json` + `Cargo.lock`
  together), add the `## [X.Y.Z]` CHANGELOG section (+ a milestone-table row for
  a new minor), and run `scripts/check-releases.sh`. The `vX.Y.Z` tag is created
  automatically on merge to main — don't tag by hand. CI fails a bump that has
  no CHANGELOG entry.
