---
name: grill-me
description: Interview the user relentlessly about a plan or design until reaching shared understanding, resolving each branch of the decision tree. Use when the user wants to stress-test a plan, get grilled on a design, or says "grill me".
---

Interview the user relentlessly about every aspect of this plan until you reach a
shared understanding. Walk down each branch of the design tree, resolving
dependencies between decisions one at a time. For each question, give your
recommended answer.

Ask each question via the **AskUserQuestion** tool, waiting for the response
before continuing. Populate the options with your recommendation plus the most
plausible alternatives (max 4, each label ≤12 characters). Reserve plain-text
questions for genuinely free-form follow-ups ("what would you call this?"); the
structured tool is the default.

**If a question can be answered by exploring the codebase, explore the codebase
instead.**

That last rule is what makes grilling worth the round-trips. It turns questions
into verified facts — which column holds the value, whether a route exists,
whether the field you are about to render is even populated — rather than asking
the user to guess at their own schema. Grilling that skips it produces agreement
about something impossible.

## What to grill for

Chase the answers that would change what gets built:

- **The thing that does not exist.** Repeatedly the blocker here has been an
  absent field: a finding has no sender, so "dismiss everything from this person"
  cannot mean what it says; a tooltip promised a failure reason that was never
  stored. Check that the data behind a proposed feature is actually there.
- **The empty and the enormous case.** A report that vanishes when a scan finds
  nothing is broken for the scan whose report matters most. A list that is fine
  at ten rows freezes the app at eight thousand.
- **The undo.** If the feature creates state, ask how it is removed. A rule with
  no UI to delete it is a one-way door, and that has shipped here.
- **What is explicitly out.** The boundary is the half that keeps scope intact
  later.

## Then write it down

Grilling that stays in the conversation is lost by the next session. **File the
issue before writing code**, using the body in the `ship-a-change` skill —
Outcome, In Scope, Out of Scope, Acceptance Criteria, Background. The issue is
the contract; your memory of the conversation is not.

When the decisions are architectural rather than scoped-feature-sized, use
**`grill-with-docs`** instead: same interview, but it also sharpens
`docs/CONTEXT.md` and records the hard-to-reverse calls as ADRs.
