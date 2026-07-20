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
