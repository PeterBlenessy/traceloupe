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
pnpm setup:engine     # one-time: install the local iLEAPP engine into ./engine
pnpm app:dev          # run the dev app (TraceLoupe Dev), wired to the local engine
cargo test -p traceloupe-core   # core tests
pnpm dev              # frontend only, in a browser with mocked IPC
```

**The iLEAPP engine.** Imports are powered by iLEAPP, which isn't bundled or
auto-downloaded yet. `pnpm setup:engine` installs a pinned iLEAPP + Python venv
into `./engine` (git-ignored, ~220 MB), and `pnpm app:dev` points the app at it
via the `TRACELOUPE_PYTHON` / `TRACELOUPE_ILEAPP_SOURCE` env vars. Without it, imports
report "engine not installed". See `docs/spike-ileapp.md` for why deps are
pinned and the plan to ship a re-frozen binary later.

## License

TraceLoupe is licensed under the [Apache License 2.0](LICENSE) — Copyright 2026
Peter Blenessy. It builds on third-party open-source software under permissive
licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). iLEAPP (MIT) runs
as a separate subprocess and is never linked into TraceLoupe.
