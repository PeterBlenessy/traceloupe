# Safety Scan — the llama-server sidecar

Safety Scan runs inference in llama.cpp's `llama-server`, bundled as a Tauri
sidecar (`bundle.externalBin` → `binaries/llama-server`) and always spawned
inside a TraceLoupe-controlled Seatbelt sandbox. See ADR 0002 for the threat
model. This mirrors NoteSage's setup, including its **dev vs. prod split**.

## Dev vs. prod (the two ways to get the binary)

The binary is git-ignored (`src-tauri/binaries/llama-server-*`, `lib/`); only
`LLAMA_CPP_VERSION` is committed. You stage it one of two ways:

| | Dev — `pnpm setup:llama` | Prod — `pnpm build:llama` |
|---|---|---|
| script | `download-llama-server.sh` | `build-llama-server.sh` |
| source | pre-built GitHub release | **compiled from source** |
| linking | dynamic (ships `lib/` dylibs) | **static** (`BUILD_SHARED_LIBS=OFF`, `GGML_STATIC=ON`) |
| Metal | external `.metal` file | **embedded** (`GGML_METAL_EMBED_LIBRARY=ON`) |
| speed | seconds | a few minutes (cmake build) |
| use | local `tauri dev` | the `.app` you ship |

**Why two:** the pre-built release is dynamically linked (`@rpath` dylibs),
which is fine for dev but **breaks macOS code signing** — a shipped `.app` needs
a static, self-contained binary. `build-llama-server.sh` produces exactly that
(it fails if `otool -L` shows any `@rpath`/homebrew dep) so `externalBin` bundles
one signable file with no `lib/` to stage.

## How it resolves at runtime

`server.rs::resolve_binary`:

- **Release build:** ONLY the bundled sidecar next to the app executable — the
  env-override and `$PATH` fallbacks are `#[cfg(debug_assertions)]`, compiled
  out, so a shipped app can never run an external, unsandboxed binary.
- **Dev build:** the bundled sidecar, then the staged `src-tauri/binaries/`
  binary (found by walking up from the dev exe — so `tauri dev` "just works"
  after `pnpm setup:llama`), then `$TRACELOUPE_LLAMA_SERVER`, then `$PATH`.

CI stages the binary via `download-llama-server.sh` before `cargo check`
(Tauri validates `externalBin` at check time).

## Building a release `.app`

```bash
pnpm build:llama    # static binary into src-tauri/binaries/
pnpm app:build      # tauri build — bundles + signs the sidecar
```

**Not yet verified on hardware:** a full packaged `pnpm app:build` on Apple
Silicon (sidecar placement, signing, and that the sandbox write-deny leaves
Metal enough room on a real model run). The static binary removes the dylib
staging problem, but the packaged run still wants a smoke test.

## The sandbox (what protects your data)

Every scan spawns the binary under `sandbox-exec` with a profile that:

- denies all network except the loopback listen socket;
- denies `file-write*` everywhere except a per-run, TraceLoupe-owned scratch dir
  (`<app-data>/models/sidecar-scratch`, wiped before/after each run) — so the
  prompt text (your messages/notes) has nowhere on disk to land;
- denies reads of user data outside the model, the binary, and scratch;
- redirects Metal's shader cache + temp into scratch via `MTL_SHADER_CACHE_PATH`
  / `TMPDIR`.

A live `sandbox-exec` test (`server.rs`, `denies_writes_except_scratch`) asserts
the OS actually refuses a write outside scratch.
