# Native app parser — the process

How we add native support for one more app's chat data (replacing the iLEAPP
fallback). The runnable version is the `create-native-app-parser` skill (do one
app; wrap with the built-in `/loop` to work through many); this doc is the
contributor-facing summary. **iLEAPP is the reference, not the source** (learn
schema facts, write fresh Rust — provenance: reference, architecture §10).

## Steps

0. **Select + scout.** If no app is named (e.g. under `/loop`), pick the next one
   from `docs/app-support.md` — highest-value app not yet ✅ native with a clean
   groupable store; skip ⚪ (no local store) and defer machinery-heavy ones. Then
   read the iLEAPP module (`engine/iLEAPP/scripts/artifacts/<app>*.py`); confirm a
   thread key + per-message author; note from `sample_data` which public test
   image has the app.
1. **Pitfall-check** — design against the checklist below *before* writing.
2. **Implement** a `parsers/apps/<app>.rs` module (`AppChatModule` + `locate` +
   `parse`), register it, reuse `col_string`/`col_i64`/`insert_app_conversation`;
   write a synthetic test (1:1 **and** a group with per-author attribution).
3. **Validate against real data** — diff native vs iLEAPP on the *same* extracted
   DB; and, once per app, extract the DB from a **public DFIR iOS test image**
   (Hickman/CTF — no need for the app in your own backup), diff, and keep a real
   fixture. Only then drop the "unvalidated" caveat.
4. **Commit.**
4b. **Commit + MINOR release** — mark ✅ in `docs/app-support.md`, bump the minor
   version (`0.6.0`), CHANGELOG + tag `vX.Y.0`, push. One app = one minor release.
5. **Correctness review loop** — subagent review, varying the lens each round; fix
   real findings; each fix round is a **PATCH release** (`vX.Y.1`, `vX.Y.2`…). Keep
   iterating **until a round finds nothing** (min 2 rounds), not just twice.
5b. **Completeness review** — objectively (vs the schema + iLEAPP) list every field
   the app stores as surfaced ✅ / not-surfaced ⬜ / not-in-backup —, and record it
   in `docs/app-data-coverage.md`. Implement a gap now only if high-value + cheap
   (a patch); else leave a ⬜ follow-up. Doesn't block release — correctness does.
5c. **UI/UX check** — usually a no-op (apps add no UI; they flow into the shared
   Messages view). Only when the app brings a *new rendering shape* (no sender,
   no timestamp, HTML/emoji-only body, unnamed group, long/RTL name) verify it
   degrades gracefully; and *periodically* (roster growth) sanity-check the shared
   view (service filter overflow, labels/icons, empty states, virtualization).
   Findings are fixes to the shared view (a patch), not the module.
6. **Self-improve** — whenever correctness review catches a bug, add/sharpen a
   known-pitfalls checklist item in the skill so no future app repeats it, and
   commit that with the fix. Then `/loop` moves to the next app.

## Known-pitfalls checklist

1. **Sender = the per-message author column**, never the conversation/group name
   (the recurring group-attribution bug). Use a stable id for `sender_id`.
2. **Timestamp epoch + unit** — Core-Data secs-since-2001 (`+978307200`) vs Unix
   s/ms/ns. Read large ints via `col_i64`, never `f64`.
3. **Direction default** for unknown/NULL state — don't attribute owner→peer.
4. **Groups named vs unnamed** — author keys off the sender column, not the group
   name; exclude system messages.
5. **Type-tolerant reads** so one odd row can't abort the parse.
6. **No JOIN fan-out** — `EXISTS`/subquery for flags, not a join.
7. **`locate` whole-path-component boundary**; exclude `-wal`/`-shm`.
8. **Decode content** — HTML→text (recover emoji `alt`), NSKeyedArchiver via
   `crate::nska`, protobuf/JSON fields.
9. **Count after commit** (the shared inserter already does this).

## Framework

`AppMessage`, `AppChatModule`, `APP_CHAT_MODULES`, `insert_app_conversation`,
`col_string`/`col_i64` — see `parsers/apps/mod.rs`. `chat_name = Some` titles the
thread and skips group inference; `None` derives from the peer / distinct senders.
iLEAPP fallback is automatic (empty parse ⇒ iLEAPP runs).
