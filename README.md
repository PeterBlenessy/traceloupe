# Salvage — Local iOS Backup Browser

A privacy-first macOS app that opens, decrypts, and lets you browse the
contents of your own iPhone backup — photos, messages, contacts, calls,
Safari history, and Notes. All processing is local; nothing leaves your Mac.

- **Product description:** [product-architecture-description.md](product-architecture-description.md)
- **Architecture:** [architecture.md](architecture.md)

## Layout

- `crates/salvage-core` — UI-agnostic Rust core: discovery, cache, import pipeline
- `src-tauri` — Tauri v2 shell (thin command layer over the core)
- `src` — React frontend (shadcn/ui, Tailwind v4, TanStack Router/Query)

## Development

```sh
pnpm install
pnpm tauri dev        # run the app
cargo test -p salvage-core   # core tests
pnpm dev              # frontend only, in a browser with mocked IPC
```
