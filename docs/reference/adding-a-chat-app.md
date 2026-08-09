# Adding a chat app

What a new app parser owes, and why each item is on the list.

Every entry here exists because it was skipped once and shipped. This is not a
style guide — it is a list of defects that reached a release.

## Before writing a parser

**Measure what the backup holds.** Run `backup-coverage` against a backup with
the app installed and read *media inside app containers*.

```
cargo run -p traceloupe-core --bin backup-coverage -- <backup_dir> [password]
```

An app whose container holds no file over 100 KB keeps no photos on the device,
and knowing that stops effort going where there is nothing to find. Record the
number either way — it is the evidence behind whatever you do next.

Note the asymmetry that is easy to get wrong: a **received** photo may exist
only as a CDN reference, with nothing local. A photo the **owner sent** was
theirs first and is often still in the app's own container. Concluding "this app
is all remote" from received messages alone writes off half the evidence.

## Messages

- Read the app's own store; do not rely on the schema-blind discovery pass
  (`docs/reference/media-discovery.md`). Discovery is a fallback for when a
  parser is absent or has drifted, and media it finds is attributed *by
  inference* — weaker evidence, and the UI says so.
- Direction (`is_from_me`) must come from something the store actually records,
  not from a guess. Getting it backwards misattributes who said what.
- Group chats: the sender of each message, not just the thread's participants.
- Timestamps: check the epoch. Apple-absolute, Unix seconds and Unix
  milliseconds all appear across apps, and a wrong epoch is a plausible-looking
  date decades out.

## Media — the item most often skipped

**Thirteen of fifteen parsers shipped without it.** Each omission was
individually reasonable and collectively meant a photo sent in most supported
apps was invisible. `scripts/check-app-parser-coverage.mjs` now fails the build
on silence: a parser that extracts nothing must carry a note saying what was
measured, or a `MEDIA: TODO #123` naming its ticket.

- Find the column holding the path or filename and build `AppAttachment`s,
  resolving through the Manifest. `whatsapp.rs` and `viber.rs` are the working
  examples.
- Probe for columns rather than assuming them. `m.ZPARTNERNAME` was read off the
  wrong table and aborted WhatsApp's entire parse; a column that moved between
  releases is the normal case, not the exception.

## Tests

- **Fixture the real schema shape, not an idealised one.** WhatsApp's media
  parse was dead entirely because its fixture put a column on the table the code
  expected, so the test agreed with the bug (#360).
- Watch each new guard fail before trusting it. A test that has never failed has
  not been shown to test anything.
- Cross-check against a public image where one exists (Josh Hickman's iOS
  releases). Counts from a public image are citable; counts from the owner's own
  device must never be committed — see `scripts/check-no-backup-stats.mjs`.

## Surfacing

- Register the module so it appears in the import catalog and in
  `docs/reference/app-support.md`.
- An app with no data in this backup and an app that is not supported must not
  look the same. An empty view has to say which.

## Provenance

Anything shown must be traceable to where it came from — the store, the table,
the column. An examiner needs to know a photo was attributed by a parser reading
a named column rather than by a scanner inferring one, and the two carry
different weight.
