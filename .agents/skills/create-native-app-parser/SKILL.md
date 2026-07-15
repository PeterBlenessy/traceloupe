---
name: create-native-app-parser
description: >-
  Add native support for ONE iOS app's chat data to TraceLoupe, replacing the
  iLEAPP fallback. Runs a disciplined process: scout the iLEAPP reference →
  design against known pitfalls → implement → validate against REAL data
  (diff vs iLEAPP + a public test-image fixture) → commit → subagent review →
  fix → re-review → push → cut a version when substantial. Invoke as
  `/create-native-app-parser <app>` (e.g. `discord`). To work through many apps
  toward the top-50 list, wrap it with the built-in `/loop` (this skill does one
  app; `/loop` handles the repetition).
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
sender, timestamp, or direction is a serious defect. A wrong parser is worse than
no parser (iLEAPP is the fallback). If a schema fact can't be resolved from the
reference, validate against real data (step 3) before trusting it.

---

## The loop

### 0. Scout
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
- If the app has an iLEAPP normalize stage in `normalize.rs`, it's already skipped
  when the native path succeeds (`NativeSkips.app_services`); otherwise it's
  purely additive.
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

### 4. Commit
Commit the module (message: what it reads, schema facts from `<app>.py`,
provenance reference §10, and the validation status).

### 5. Review → fix → re-review
- Launch a `general-purpose` subagent to correctness-review the module against
  the checklist (see the review prompt template below). Give it the schema facts
  and tell it to skim the iLEAPP module.
- Fix every real finding in the same pass (standing rule: fix bugs you find).
- Re-review substantial fixes with a second subagent focused on the fix.

### 6. Push; version when substantial
- Push after review is clean.
- When a batch of apps is substantial (~2+), cut a version: bump `package.json`,
  workspace `Cargo.toml`, `Cargo.lock` (via `cargo check`), `tauri.conf.json`;
  add a CHANGELOG entry + milestone-table row; update `docs/app-support.md`,
  `docs/app-data-coverage.md`, and the `traceloupe-versioning` memory; tag
  `vX.Y.Z`; push the tag.

That completes one app. To continue app-by-app toward the top-50 list (see
`docs/app-support.md`), run this skill under the built-in **`/loop`** — it handles
the repetition, so **don't stop to ask whether to continue**. Stop only when the
list is covered or the next app genuinely needs new machinery (a generic
`Cache.db` reader, a YapDatabase decoder) that warrants a fresh, focused effort —
which is a good moment to split that machinery out as its own sub-skill.

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
