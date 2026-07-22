---
name: create-native-app-parser
description: >-
  Add native support for ONE iOS app's chat data to TraceLoupe, replacing the
  iLEAPP fallback. Runs a disciplined process: scout the iLEAPP reference →
  design against known pitfalls → implement → validate against REAL data
  (diff vs iLEAPP + a public test-image fixture) → commit + MINOR release →
  correctness review-loop until clean (each fix round a PATCH release) →
  completeness review (measure surfaced-vs-available fields into the coverage doc)
  → self-improve the checklist when review bites. Picks the next app from
  docs/reference/app-support.md when
  none is named. Invoke as `/create-native-app-parser <app>` (e.g. `discord`). To
  work through many apps toward the top-50 list, wrap it with the built-in `/loop`
  (this skill does one app end-to-end; `/loop` handles the repetition).
---

# Create a native app parser

TraceLoupe reads iPhone backups natively instead of via the iLEAPP sidecar. Each
third-party chat app is a small module under
`crates/traceloupe-core/src/parsers/apps/`. This skill adds one app, correctly,
following a process that has repeatedly caught real bugs.

**Golden rule:** iLEAPP is the *reference*, not the *source*. Learn the schema
facts from its module, write fresh Rust (provenance: reference, architecture §10).
Never paste iLEAPP code.

**Correctness matters more than breadth.** This is a forensics tool — a wrong
sender, timestamp, or direction is a serious defect. There is **no runtime iLEAPP
fallback** (the app never runs iLEAPP — it's a dev reference only; see the
`ileapp-dev-reference-only` memory), so a wrong parser silently ships wrong data.
If a schema fact can't be resolved from the reference, validate against real data
(step 3) before trusting it.

---

## The loop

### 0a. Select the app
- **If an app was named** (`/create-native-app-parser discord`), use it.
- **If not** (e.g. running under `/loop`), pick the next one from
  **`docs/reference/app-support.md`** — the worklist and single source of truth for status.
  Choose the highest-value app that is **not yet ✅ native** and has a **clean
  groupable store** (an iLEAPP module + SQLite with a thread key + author column):
  prefer higher tiers (Top 10 → 25 → 50) and clean SQLite over heavy-machinery
  apps. **Skip** apps marked ⚪ (no recoverable local store) and defer ones needing
  new machinery (cached-JSON/`Cache.db`, YapDatabase, Matrix-JSON) unless building
  that machinery is the explicit task. If every remaining app needs machinery,
  build the highest-leverage decoder as its own sub-skill instead.
- Mark it in the tracker (status → in progress) so a resumed loop doesn't repeat it.

### 0b. Scout
- Read the app's iLEAPP module: `engine/iLEAPP/scripts/artifacts/<app>*.py`.
- Confirm it has a **clean, groupable local store** (SQLite with a real
  conversation/thread key + a per-message author column). If messages live in
  cached JSON (`fsCachedData`/`Cache.db`), serialized blobs (YapDatabase), or a
  binary format, that's a heavier effort — note it and pick a cleaner app, or
  build the needed decoder as its own sub-skill first.
- Note from the module's `__artifacts_v2__.sample_data` **which public test image
  contains this app and roughly how many rows** (e.g. `otto_ios17: 1803 rows`) —
  this tells you exactly what to validate against in step 3.

### 1. Pitfall-check (design first)
Design the parse against the checklist below *before* writing it. Most review
findings historically were the SAME few mistakes — design them out.

### 2. Implement + synthetic test
- New module `parsers/apps/<app>.rs`: `MODULE` (`AppChatModule`), `locate`,
  `parse(&Path, &str) -> Result<Vec<AppMessage>>`. Register in
  `parsers/apps/mod.rs` (`pub mod` + `APP_CHAT_MODULES`).
- Reuse the framework: `col_string`/`col_i64` (tolerant reads),
  `insert_app_conversation` (grouping, group detection, per-thread counters).
- No runtime iLEAPP coupling: the native parser writes straight to the cache. (A
  legacy `NativeSkips` path in `normalize.rs` exists only for the dormant engine,
  which is never run.)
- Write a synthetic unit test (1:1 **and** a group, incl. per-author attribution).
- `cargo test -p traceloupe-core --lib <app>` + `cargo clippy` + `cargo fmt`.

### 3. Validate against REAL data (the step that used to be missing)
Synthetic fixtures are circular — they only prove your encoder matches your
decoder. Break the circularity:

- **Cheap tier (always):** run the native parser and **iLEAPP** against the *same*
  extracted DB and diff the output (thread names, senders, timestamps, counts). A
  disagreement means one side is wrong — reconcile before shipping.
- **Real tier (per app, once):** obtain the DB from a **public DFIR iOS test
  image** — the same corpus iLEAPP's `sample_data` names refer to (Josh Hickman /
  CTF images, e.g. `dexter_ios18`, `otto_ios17`, `iphone11_ios17`). These are
  full public backups with dozens of apps installed, so **you don't need the app
  in your own backup**. Pick the image that `sample_data` says contains this app,
  extract just this app's DB, run native + iLEAPP, and diff. Keep a trimmed real
  DB as a committed fixture (or, if too large, document the validation result and
  the image/row-count in the module doc).
- Only after this does the module lose its "unvalidated" caveat in the docs.

### 4. Commit + MINOR release (the app lands)
Commit the validated module (message: what it reads, schema facts from `<app>.py`,
provenance reference §10, validation status). Then cut the app's minor release:
- Mark it ✅ native in `docs/reference/app-support.md`; add its row to
  `docs/reference/app-data-coverage.md`.
- **Bump the MINOR version** (`0.5.0 → 0.6.0`): `package.json`, workspace
  `Cargo.toml`, `src-tauri/tauri.conf.json`, `Cargo.lock` (via `cargo check`).
- CHANGELOG entry + milestone row; update the `traceloupe-versioning` memory.
- Commit, `git tag -a vX.Y.0`, `git push origin main && git push origin vX.Y.0`.

**Minor vs patch — an OBJECTIVE rule (don't judge "size").** Whether a change
"feels big" is subjective and inconsistent, so don't decide the bump that way.
Decide by *kind*:
- **A new app / new message store / new conversation source** → **MINOR**, always,
  regardless of how few fields it has. Every distinct app is a user-visible
  feature.
- **Extending an app already native** — a new field, a second table, or a
  sub-artifact of the same app (e.g. TikTok *contacts* for the already-native
  TikTok chats) → **PATCH**, folded into that app's line.
- **Reusable machinery** (a decoder like `nska`, a `Cache.db` reader) → ships with
  the first app that needs it; call it out in the changelog. It doesn't get its
  own version.

This keeps the loop deterministic. Only deviate — batching a couple of apps into
one minor — if there's an explicit reason (a coordinated release); the default is
one app per minor.

### 5. Review loop → PATCH release per fix round (iterate until clean)
Harden the app through **repeated** review rounds — not just twice:
- Launch a `general-purpose` subagent to correctness-review the module against the
  checklist (prompt template below). Give it the schema facts; tell it to skim the
  iLEAPP module. **Vary the lens each round** (round 1: schema/attribution; round
  2: timestamps/direction/types; round 3: adversarial/edge cases) so fresh eyes
  catch what the last round didn't.
- Fix every real (correctness) finding in the same pass — ignore style/nits.
- Each fix round is a **PATCH release**: bump the patch (`vX.Y.0 → vX.Y.1`), add a
  CHANGELOG line under the app's entry, commit, tag `vX.Y.Z`, push main + tag.
- **Keep looping** until a full review round returns **no real findings**
  (minimum 2 rounds; if a round finds something, do another after fixing). Only
  then is the app done.

### 5b. Completeness review (are we surfacing everything?)
Correctness asks "is what we extract right?"; completeness asks "did we extract
**all** the app stores?" Measure it **objectively** against two references, not by
feel:
- the app's real **schema** (tables/columns actually present), and
- **iLEAPP's module** (the fields a mature tool surfaces — the practical superset).

Run a completeness pass (a `general-purpose` subagent, or an inline field-by-field
walk): enumerate every message-relevant field the app persists, and mark each
**surfaced ✅ / present-but-not-surfaced ⬜ / not-in-backup —**. Then:
- **Record the result in `docs/reference/app-data-coverage.md`** — this review *is* what
  fills that table honestly (reactions, edits, read receipts, replies, media
  payloads, location, forwarded-from, etc.).
- **Implement a gap now** only if it's **high-value AND cheap** (another column on
  the same query) — that's a PATCH. Don't gold-plate: an expensive or niche field
  stays a ⬜ row (a logged follow-up), so the loop keeps moving.
- Completeness gaps **do not block** the app's release — correctness does. The
  point is an *honest, measured* coverage record, not 100% coverage per app.

### 5c. UI/UX check (conditional + periodic — usually a no-op)
These modules add **no new UI** — every app flows into the shared Messages view
(threads list + conversation), tagged by service. So there's nothing to review per
app *unless*:
- **New rendering shape (per app, only if it applies):** the app introduces
  something the view hasn't handled — no sender name, missing timestamp, HTML/
  emoji-only body, an **unnamed group**, an empty body with only an attachment, a
  very long or RTL name. Verify it **degrades gracefully** in the Messages view.
  Cheap tier: reason about it / add a mock thread for the service in the mock
  client (`src/lib/ipc.ts`) and check it renders. Real tier: load it in the
  Messages view (mock or a backup) and eyeball via browser automation.
- **Roster growth (periodic, ~every few apps / at a version milestone):** with
  many services now present, sanity-check the *shared* UI — the service filter
  (overflow, ordering, the "All" option, chip legibility), service labels/icons,
  empty/degraded states, and thread-list virtualization at scale. This is a
  batched pass, **not** something to run every turn.

A UI/UX finding is usually a fix to the shared view (a patch), not to the module.

### 6. Self-improve (make the loop learn) — do this whenever a review bites
If a review round surfaced a **correctness bug**, ask: *would the pitfall
checklist have prevented it at design time?*
- **Not on the checklist** → add a new checklist item (concrete: the signal to
  look for + the fix), so no future app repeats it.
- **On the checklist but you still shipped it** → sharpen that item (a clearer
  rule / a "watch for" note) and add it as a mandatory pre-commit self-check.
- Recurring across apps → promote it to the top of the checklist.
Commit the `SKILL.md` change with the app's fixes. This is the step that stops the
same mistake (e.g. group attribution — found 4×) from recurring: the skill's
checklist should get *stronger* every time review catches something.

### 7. Next app
That completes one app. Under the built-in **`/loop`**, go straight to the next
(step 0a) — **don't stop to ask whether to continue**. Stop only when the top-50
list (`docs/reference/app-support.md`) is covered or the next app needs new machinery (a
generic `Cache.db` reader, a YapDatabase decoder) that warrants a fresh, focused
effort — split that machinery out as its own sub-skill.

---

## Known-pitfalls checklist (design against ALL of these)

1. **Sender = the per-message AUTHOR column**, never the conversation/group name.
   In a group, attributing every message to the group title is the recurring bug
   (hit Kik/imo/Threema/Viber). Find the author column (`ZALIAS`, `ZSENDER`,
   `ZFROM`, `senderPk`, …) and use it; set `sender_id` from a stable **id** (not a
   display name — same names collide) so the framework can label groups.
2. **Timestamp epoch AND unit.** Core-Data/Cocoa = seconds since 2001 (`+978307200`);
   others are Unix seconds / ms (`/1000`) / ns (`/1e9`). Read large-integer times
   via `col_i64`, **never `f64`** (loses precision past 2^53 — corrupts ns times).
3. **Direction default.** Map the known received/sent states explicitly; for an
   unknown/NULL state, don't silently attribute an owner's message to the peer —
   infer from a signal (e.g. presence of a member-sender) or leave unattributed.
   Remember failed/sending are the owner's actions (outgoing).
4. **Groups: named vs unnamed.** Author attribution must key off the sender
   column, not off whether the group has a name. An unnamed group must still
   attribute authors and be detectable as a group (distinct `sender_id`s).
   Exclude system/service messages (e.g. `Z_PRIMARYKEY.Z_NAME = 'SystemMessage'`).
5. **Type-tolerant reads.** Use `col_string`/`col_i64` so one oddly-typed row
   (INTEGER id read as String, TEXT timestamp, BLOB body) can't `?`-abort the
   whole DB parse.
6. **No JOIN fan-out.** A message with several attachments/media rows must not
   duplicate the message — use `EXISTS(...)` / a subquery for flags, not a join.
7. **`locate` path boundary.** Match the DB as a whole path component
   (`== "x.sqlite" || ends_with("/x.sqlite")`), and exclude `-wal`/`-shm`. The
   `table_exists` guard makes a non-message DB return empty (safe).
8. **Content decoding.** HTML → text (strip tags, recover emoji `alt`, decode
   entities); NSKeyedArchiver blobs → `crate::nska`; protobuf/JSON → decode the
   documented fields. Don't emit raw markup or empty bodies for emoji-only rows.
9. **Count after commit.** If you tally into `report`, do it after `tx.commit()`
   so a rolled-back parse doesn't leave phantom counts (which double up if iLEAPP
   re-runs). The shared `insert_app_conversation` already does this.

---

## Framework quick-reference

- `AppMessage { chat_key, chat_name, timestamp(unix s), body, is_from_me,
  sender_name, sender_handle, sender_id, has_attachment }`.
- `chat_name = Some` → the framework titles the thread with it and skips group
  inference. `chat_name = None` → it derives a 1:1 name from the peer's
  `sender_name`, or labels a group when it sees >1 distinct incoming `sender_id`.
- `numeric_id_groups: true` ONLY when a bare-numeric `chat_key` means a group
  (TikTok). For apps whose 1:1s use numeric ids, it MUST be false.
- iLEAPP fallback is automatic: `parse` returns `Ok(vec![])` for an
  unrecognized/absent DB; the driver only claims the service (skips iLEAPP) when
  it produced messages.

## Review prompt template (step 5)

> Correctness review of `crates/traceloupe-core/src/parsers/apps/<app>.rs`
> (repo: …). Driven by the shared framework (`insert_app_conversation`: groups by
> `chat_key`; `chat_name` Some → titles + skips group inference; else derives from
> distinct incoming `sender_id`s). Schema (from iLEAPP `<app>.py`): <paste facts>.
> Hunt for REAL bugs, prioritizing the known-pitfalls checklist: per-message
> sender vs conversation name (group attribution), timestamp epoch/unit, direction
> default for unknown/NULL, named-vs-unnamed groups + system-message exclusion,
> type-tolerant reads, JOIN fan-out, locate boundary, content decoding. Read the
> file + its test AND skim the iLEAPP module for the timestamp/sender/direction
> questions. For each finding: severity, file:line, concrete input → wrong output,
> covered-by-test or missed. If correct, say so. Be concise. Do not modify files.

## Provenance & licensing
Every module header records `provenance: reference (own implementation) …` and
names the iLEAPP module the schema facts came from (architecture §10). Facts
(paths, table/column names, encodings) aren't copyrightable; the Rust is ours.
