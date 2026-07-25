# CLAUDE.md

**Read [AGENTS.md](AGENTS.md) before doing anything in this repo.** It holds the
ground rules for the multi-agent setup — most importantly:

> **Multiple agents share this one clone. Work in your OWN git worktree, never in
> the shared main checkout, or you will collide with (and can lose the
> uncommitted work of) other agents.**

Quick start for a new task:

```bash
scripts/agent-worktree.sh <slug>   # creates .claude/worktrees/<slug> on branch <slug>
cd .claude/worktrees/<slug>
git push -u origin <slug>          # back it up immediately
```

The branch name always equals the worktree directory name. See AGENTS.md for the
naming rules, build/verify commands, and cleanup steps.

Two more rules from AGENTS.md that are easy to violate without noticing:

> **The user's real backup is private and off-limits.** Never read it (including
> the `~/.traceloupe-dev/backup-mirror` and any `caches/<backup_id>/`) and never
> record anything derived from it. Validate with `tools/make_fixture_backup.py`
> or public DFIR fixtures instead. See "Never touch the user's real backup data".

> **Done means shipped** — implemented, tested, CI-green, pushed, PR open. Decide
> judgment calls yourself and document them in the PR; answer empirical questions
> with a fixture and a measurement rather than asking. Stop short only when
> externally blocked or when proceeding would be unsafe. See "Done means shipped".

> **Background work must outlive the UI.** Anything that outlives a single
> command call needs a status snapshot and a mount-time re-attach — the UI must
> never be the only place its state exists, or a reload leaves an idle-looking
> screen over work that is still running. See "Background work must outlive the
> UI".

> **Watch the shell's working directory.** It is not pinned to your worktree —
> after a `cd` elsewhere, the next command can run in the shared main checkout on
> a *different branch*, so relative-path reads/edits hit the wrong files. Use
> absolute paths rooted at your worktree, and `pwd` after any `cd` away. See
> "Don't trust the shell's working directory" in AGENTS.md.
