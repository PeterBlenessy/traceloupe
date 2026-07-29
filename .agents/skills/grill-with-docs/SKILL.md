---
name: grill-with-docs
description: Grilling session that challenges a plan against TraceLoupe's domain model, sharpens terminology in docs/CONTEXT.md, and records hard-to-reverse decisions as ADRs (and a PRD for anything milestone-sized) as they crystallise. Use for architectural or feature-defining work, or when the user says "grill with docs".
---

Everything in **`grill-me`** applies: interview relentlessly, one branch of the
decision tree at a time, each question through **AskUserQuestion** with your
recommendation first and the most plausible alternatives beside it (max 4, labels
≤12 characters), and **if a question can be answered by exploring the codebase,
explore the codebase instead.**

What this skill adds: the documentation is updated *during* the session, not
after. Decisions captured while the reasoning is live are accurate; decisions
reconstructed afterwards are a summary of what you remember agreeing to.

## Where the docs live in this repo

This is a single-context repo. Do not go looking for a `CONTEXT-MAP.md` or create
per-module contexts.

| What | Where |
| --- | --- |
| Glossary | `docs/CONTEXT.md` |
| Decisions | `docs/adr/NNNN-slug.md` — sequential, scan for the highest and add one |
| Milestone-sized specs | `docs/plans/<slug>-prd.md`, `<slug>-plan.md` |

## During the session

**Challenge against the glossary.** When the user's term conflicts with
`docs/CONTEXT.md`, say so immediately rather than quietly adopting the new sense.
The glossary already records one such collision: "Spyware Analyzer" was renamed
to "Security Check" and the old name is listed as one to avoid. That entry only
exists because the conflict was raised at the time.

**Sharpen fuzzy language.** Propose a precise canonical term when one is
overloaded. Distinguishing *Explicit Scan* from *Passive Check*, or *Indicator*
from *Finding*, is what stops two views from describing the same thing
differently.

**Cross-reference with the code.** When the user states how something works,
check whether the code agrees, and surface contradictions. This is where
grilling earns its keep — a stated behaviour that the code contradicts is either
a bug or a misunderstanding, and both are cheaper to find now.

**Update `docs/CONTEXT.md` inline.** As each term resolves, not batched at the
end. One sentence per term, defining what it *is*; list the words to avoid.
Only domain terms — if a competent Rust or React developer would recognise it
without this project, it does not belong.

## ADRs — offered sparingly

Write one only when **all three** hold:

1. **Hard to reverse** — changing your mind later costs something real.
2. **Surprising without context** — a future reader will ask "why on earth is it
   done this way?"
3. **A genuine trade-off** — there were real alternatives and one was chosen for
   specific reasons.

Miss any one and skip it. An easily reversed decision will simply be reversed; an
unsurprising one prompts no questions; one with no alternative records nothing.

House style, set by
[`0004-follow-macos-settings-not-app-preferences.md`](../../../docs/adr/0004-follow-macos-settings-not-app-preferences.md):
a `**Status:** accepted (date)` line, then `## Decision`, `## Context`, and a
`## Why not <the obvious alternative>` section. **Argue from measurement, not
taste** — 0004 is persuasive because it counted the tab stops and named the
eleven ignored settings, which turned a preference debate into a fact. If your
Context section has no numbers in it, ask whether you actually verified the
premise.

What has qualified here: the scope of the privacy promise, running the safety
pipeline locally, the two-tier approach to iCloud-offloaded media, following
macOS settings over app preferences. All four are architectural shape, lock-in,
or a deliberate deviation from the obvious path.

## PRDs — for milestone-sized work only

When the outcome is a multi-milestone feature rather than a single change, write
`docs/plans/<slug>-prd.md` before building: executive summary, problem, goals and
**non-goals**, feature description, milestones, risks and open questions,
references. `spyware-analyzer-prd.md` is the model, including its section on how
a comparable commercial tool solves the same problem — knowing what the
established answer looks like is worth a section.

Anything smaller belongs in a GitHub issue, not a PRD. Use the issue body in
`ship-a-change`.

## Finish the loop

A grilling session ends with artefacts, not agreement: the glossary updated, any
ADR written, the issue or PRD filed. Then hand over to `ship-a-change` to build
it.
