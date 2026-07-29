---
name: ship-a-change
description: The end-to-end loop for changing TraceLoupe — grill, file, worktree, build, prove the guards, preflight, PR, merge, clean up. Use when starting any non-trivial feature or fix in this repo, or when the user asks to build/implement/fix something.
---

The order below is not a style preference. Every step exists because skipping it
produced a specific defect that reached a release.

## 1. Grill before you build, and write the outcome down

**Interview the user before building.** Walk down each branch of the design tree,
resolving dependencies between decisions one at a time. Ask via the
**AskUserQuestion** tool: your recommended answer first, plus the most plausible
alternatives (max 4, labels ≤12 characters). Keep plain-text questions for
genuinely free-form ones ("what would you call this?").

**Where an answer is findable in the codebase, look instead of asking.** This is
the line that makes grilling pay: it turns questions into verified facts —
column names, which table a value lives in, whether a route exists — instead of
into the user's guesses about their own schema.

Then **file the issue before writing code**, with the decisions and their reasons
in it. The issue is the contract — not your memory of the conversation. It is what
keeps scope intact across a long session, and what lets a decision be revisited
without re-deriving it. Use this body:

```markdown
## Outcome          <!-- one sentence: what is true when this is done -->
## In Scope
## Out of Scope     <!-- the half that actually prevents drift -->
## Acceptance Criteria   <!-- checkable, not aspirational -->
## Background       <!-- the constraints found while grilling -->
```

`Out of Scope` and `Acceptance Criteria` are the load-bearing ones. Without them
an issue records enthusiasm rather than an agreement.

Grilling repeatedly surfaces blockers the code would otherwise hit halfway
through: a finding has no sender field, so "dismiss everything from this person"
cannot mean what it says; the report vanished for a clean scan, which is the scan
whose report matters most; the failure reason the tooltip promised did not exist
anywhere.

## 2. Work in your own worktree

```bash
scripts/agent-worktree.sh <slug>
cd .claude/worktrees/<slug>
git push -u origin <slug>
```

See AGENTS.md. Use absolute paths rooted at the worktree; `pwd` after any `cd`.

## 3. Edit with something that fails when it misses

**This is where most self-inflicted damage comes from.** Scripted string
replacement is silent when it is wrong:

- a regex with `.*?` and `re.S` deleted ~500 lines of `analysis.rs`
- `cd X && python3 …` short-circuited because the `cd` failed, so the edit never
  ran — and `pnpm build` afterwards still reported success, so it looked applied
- `s.replace(old, new, 1)` hit the *first* occurrence three separate times: the
  wrong `scan2`, a field added to the wrong struct, a component inserted into the
  wrong parent

So:

- **Prefer the Edit tool.** It fails loudly when `old_string` is not unique or
  not found. That is the whole point.
- If you must script an edit, **assert**: that the pattern was found, that it
  matched exactly once, and that the file actually changed. `assert old in s` is
  the minimum; counting occurrences is better.
- **Never run a formatter across a whole file** you are touching a few lines of.
  `prettier --write` reformatted four view files and buried the real change in
  ~500 lines of churn.
- After a scripted edit, **grep for the thing you expect** — the presence of the
  new string and the absence of the old.

## 4. Prove every new guard fails

A check that cannot fail reads exactly like a check that passes. Before trusting
a new test, lint rule or assertion, **break something and watch it fail.**

This caught, in one session: a lint reporting "Safety" while a dialog kept it on
Security; a self-test that passed with its own probe deleted; a view "measured"
before its data had loaded, where a planted violation sailed through; a rule
sharing a name with another so the requirement was satisfied twice over.

Two failure modes, needing different answers:

- **Weak assertion** — the code could be wrong and nothing fails.
  `cargo mutants --file <path> -p traceloupe-core -- --lib`. A trial over
  `dashboard.rs` took 15 minutes and found seven, in a file written with
  deliberate guard tests. Too slow for CI; run it when adding logic worth
  guarding, and write tests from its report rather than from imagination.
- **Blind check** — it runs, observes nothing, reports success. Mutation testing
  cannot see this. The answer is a check that **states what it observed and fails
  when it observed too little** — see the `coverage` rule in `check-design.mjs`
  and `check-mock-parity.mjs`.

## 5. Measure the UI, never eyeball it

`getBoundingClientRect().height`, computed styles, resolved locales, DOM counts.
"distinct heights: [30]" is a fact; "looks right" is not. See "Measure the UI,
don't eyeball it" in AGENTS.md.

Check the states an idle screenshot never shows: hovered, focused, filtered, both
text extremes, both themes. A focus-visible swap could not be tested at all until
macOS Full Keyboard Access was switched on in the page — the harness silently
could not reach the control, which looks identical to a broken feature.

## 6. Preflight before the PR

```bash
scripts/preflight.sh              # hygiene, rust, frontend
scripts/preflight.sh --with-ui    # …and the design lint
```

One command, CI's order, and a summary saying which checks actually **ran** —
because a gate that quietly skips a step reports the same "OK" as one that
passed it.

## 7. Ship it, and finish the loop

Open the PR, wait for CI, merge, clean up:

```bash
gh pr checks <n>                       # wait for green
gh pr merge <n> --merge
git pull && scripts/agent-cleanup.sh <slug>
```

**Do not announce an action you have not performed.** Saying "merging now" and
stopping happened twice in one session. If you say it, do it in the same turn, or
say what you are waiting for.

`agent-cleanup.sh` refuses rather than losing work, and refuses to run from
inside the worktree it is removing — a script cannot chdir its caller, and
removing the directory you are standing in leaves the shell with none.

## 8. Release only when asked

`scripts/release.sh X.Y.Z`, then a CHANGELOG entry written for someone who uses
the app: what changed for them, not which module moved. The tag is created
automatically when the release lands on main.

## When a guard catches something

Fix it in the same pass, and consider what it means. Twice in one session a guard
flagged a technical detail and the real finding was underneath it: an undeclared
`Vec` return turned out to be a feature with **no UI to undo it** — a one-way
door; a control-height literal sat on an input added minutes earlier. Read what
the guard found, then look one level past it.
