---
name: wayfinder
description: Plan an effort too big for one agent session as a map of decision tickets on GitHub, then resolve them one at a time until the route is clear. Use for multi-session work wrapped in fog — where what to build is not yet decidable — not for a feature that could be built today.
---

Adapted from [Matt Pocock's wayfinder](https://github.com/mattpocock/skills/tree/main/skills/engineering/wayfinder),
wired to this repo's tracker and skills.

A loose idea arrives, too big for one session, and the way from here to the
**destination** is not visible. Wayfinding finds that way. It charts a **map** on
the issue tracker and works its **decision tickets** — questions whose resolution
is a decision, not slices of a build.

**The point is surviving the session.** An agent session ends; anything held only
in the conversation is gone. A map is state a fresh session can load in one
fetch: where this is going, what is settled, what is takeable now, and what was
deliberately ruled out. Before this existed, that state lived in a 200-line
memory file on one laptop — which is not state, it is a souvenir.

## When not to use it

**If the route is already clear, do not make a map.** Most work here is one
issue and one `ship-a-change` run. Charting is justified only when you cannot yet
say what to build — when the open questions are decisions, not tasks. If grilling
the idea surfaces no fog, say so and stop.

## Plan, don't do

Each ticket resolves a decision. The map is done when nothing is left to decide
before someone goes and builds. **The pull to just do the work is the signal you
have reached the edge of the map** — hand off to `ship-a-change` there.

## Refer to tickets by name

In anything the user reads, name the ticket — a wall of `#180, #181, #182` is
illegible. The number rides inside the link, never instead of it.

## The map

One issue labelled `wayfinder:map`, with the tickets as its **sub-issues**. It is
an **index, not a store**: each decision lives in exactly one place — its ticket —
and the map only gists it and links.

```markdown
## Destination
<what reaching the end looks like. One or two lines; every session orients here first.>

## Notes
<domain; which skills to consult; standing constraints for this effort>

## Decisions so far
- [<closed ticket name>](<link>) — <one-line gist>

## Not yet specified
<in-scope fog you cannot ticket yet; graduates as the frontier advances>

## Out of scope
<ruled beyond the destination; never graduates>
```

Open tickets are **not listed** — they are found by query, so the map cannot go
stale about them.

### Fog, or a ticket?

The test is whether you can state the question precisely **now** — not whether
you can answer it. Sharp enough to phrase, even if blocked, means ticket. Not yet
that sharp means **Not yet specified**. Do not pre-slice fog into ticket-shaped
pieces; one patch may graduate into several tickets, or none.

**Out of scope is a different thing from fog.** Fog gathers only *toward* the
destination. Work past the destination is scope, not sharpness — it goes in **Out
of scope** and never graduates. We already do this informally: the `⊘
won't-implement` markers in `docs/reference/app-data-coverage.md` are exactly
this list. When a ticket turns out to sit past the destination, **close it** and
leave one line saying why.

## Ticket types

Every ticket is **HITL** (worked with the user, live) or **AFK** (agent alone).
An agent never answers the user's side of a HITL ticket — a grilling that
answers its own questions has broken the point of grilling.

| Label | Mode | For |
| --- | --- | --- |
| `wayfinder:research` | AFK | A fact a decision waits on, findable outside this working directory. Resolve with a research subagent. |
| `wayfinder:grilling` | HITL | The default. Use `grill-me`, or `grill-with-docs` when the answer belongs in `docs/CONTEXT.md` or an ADR. |
| `wayfinder:prototype` | HITL | Something cheap and concrete to react to, when "how should it look or behave" is the question. |
| `wayfinder:task` | either | Manual work blocking a *decision* — provisioning access, requesting an export, moving data so its shape is visible. The one type that does rather than decides, and it earns that by unblocking a decision. |

## Tracker mechanics

GitHub sub-issues and issue dependencies are both enabled on this repo. These
commands are verified, not guessed — note `-F` for the integer id; `-f` sends a
string and returns HTTP 422.

```bash
R=repos/PeterBlenessy/traceloupe

# create (gh issue create has no --json; it prints the URL)
URL=$(gh issue create --label "wayfinder:grilling" --title "…" --body-file body.md)

# attach as a child of the map — sub_issue_id is the database id, not the number
id=$(gh api $R/issues/<child>  --jq .id)
gh api --method POST $R/issues/<map>/sub_issues -F sub_issue_id=$id

# blocking
id=$(gh api $R/issues/<blocker> --jq .id)
gh api --method POST $R/issues/<blocked>/dependencies/blocked_by -F issue_id=$id

# read them back
gh api $R/issues/<map>/sub_issues            --jq '.[]|"#\(.number) \(.title)"'
gh api $R/issues/<n>/dependencies/blocked_by --jq '.[]|"#\(.number) \(.title)"'
```

**Claim a ticket by assigning it to yourself before doing any work** — an open,
unassigned ticket is unclaimed, and that is what stops two sessions colliding.
The **frontier** is the open, unblocked, unassigned children.

## Chart a map

1. **Name the destination** — `grill-with-docs`. It fixes the scope, so it is
   settled first.
2. **Grill again, breadth-first** — fan out across the whole space rather than
   deep on one thread. **If no fog appears, stop and say so**: the work does not
   need a map.
3. **Create the map**, Decisions-so-far empty, fog sketched into Not yet
   specified.
4. **Create the tickets you can specify now**, then wire blocking in a **second
   pass** — issues need ids before they can reference each other.
5. Stop. Charting resolves nothing; that is the next session's work.

## Work through the map

1. Load the **map** — the low-resolution view, not every ticket body.
2. Take the ticket the user named, else the first on the frontier. **Claim it.**
3. Resolve it, zooming into related tickets on demand and using the skills the
   Notes block names.
4. **Record the resolution**: comment the answer on the ticket, close it, and add
   a one-line gist plus link to the map's Decisions so far.
5. Graduate any fog the answer sharpened into new tickets, clearing those patches
   from Not yet specified. If the answer put something past the destination, rule
   it out of scope rather than resolving it.

**Record the resolution as soon as you have it** — not at the end of the session.
A context window that runs out mid-write loses the decision and leaves the ticket
claimed, which is worse than never having started it. Resolving several tickets
in one session is fine here; leaving one half-recorded is not.
