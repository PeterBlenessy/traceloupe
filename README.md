# TraceLoupe — Local iOS Backup Browser

A privacy-first macOS app that opens, decrypts, and lets you browse the
contents of your own iPhone backup — photos, messages, contacts, calls,
Safari history, and Notes. All processing is local; nothing leaves your Mac.

- **Product description:** [product-architecture-description.md](product-architecture-description.md)
- **Architecture:** [architecture.md](architecture.md)

## Layout

- `crates/traceloupe-core` — UI-agnostic Rust core: discovery, cache, import pipeline
- `src-tauri` — Tauri v2 shell (thin command layer over the core)
- `src` — React frontend (shadcn/ui, Tailwind v4, TanStack Router/Query)

## Development

```sh
pnpm install
pnpm app:dev          # run the dev app (TraceLoupe Dev)
cargo test -p traceloupe-core   # core tests
pnpm dev              # frontend only, in a browser with mocked IPC
```

**Imports are fully native.** TraceLoupe parses backups with its own Rust
parsers (~35s for a full import) — no iLEAPP, no Python, no engine download,
and no network access required. iLEAPP is used only as a *development-time
reference* for cross-checking those native parsers: `pnpm setup:engine`
installs a pinned iLEAPP + Python venv into the git-ignored `./engine`, which
you only need if you're diffing a parser against iLEAPP's output. See
`docs/native-app-parser.md` for the parser workflow and `docs/spike-ileapp.md`
for the history.

## License

TraceLoupe is licensed under the [Apache License 2.0](LICENSE) — Copyright 2026
Peter Blenessy. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). TraceLoupe
ships no third-party parser code — all parsers are original Rust; iLEAPP (MIT)
is used only as a development reference and is neither bundled nor run by the
app.
