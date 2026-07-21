# Safety Scan — the llama-server sidecar

Safety Scan runs inference in llama.cpp's `llama-server`, bundled as a Tauri
sidecar and always spawned inside a TraceLoupe-controlled Seatbelt sandbox. See
ADR 0002 for the threat model; this is the operational setup.

## Why it's bundled

A *shipped* build runs **only** the binary TraceLoupe bundles — `resolve_binary`
compiles out the env-override and `$PATH` fallbacks in release, so a packaged
app cannot be pointed at an external llama-server. The bundle exists so the
sandbox has a known, controlled binary to run. In `tauri dev` / `cargo test`
(debug builds) the fallbacks are available, so `brew install llama.cpp` on your
`$PATH` is enough for local runs without bundling.

## One-time bundling (before a packaged `tauri build`)

`externalBin` is **not** committed to `tauri.conf.json`, because Tauri's build
script requires the referenced binary to exist even for `cargo check`, and we
don't commit a ~30 MB arch-specific binary (it would break CI). To produce a
distributable `.app`:

1. Fetch the pinned binary + dylibs:
   ```bash
   scripts/download-llama-server.sh          # uses src-tauri/binaries/LLAMA_CPP_VERSION
   ```
   (Set `LLAMA_CPP_VERSION` to a real llama.cpp release tag, e.g. `b6510`.)
2. Add to `src-tauri/tauri.conf.json` under `bundle`:
   ```json
   "externalBin": ["binaries/llama-server"],
   "resources": {
     "../crates/traceloupe-core/resources/indicators/": "indicators/",
     "binaries/lib/": "lib/"
   }
   ```
3. `pnpm tauri build`.

**Unverified:** the exact on-disk name/location of the sidecar in the packaged
`.app`, the dylib rpath, and whether the sandbox write-deny leaves Metal enough
room, all need a real packaged run on Apple Silicon to confirm. `resolve_binary`
accepts both `llama-server-<triple>` and `llama-server` next to the executable
to be robust to Tauri's naming.

## The sandbox (what protects your data)

Every scan spawns the binary under `sandbox-exec` with a profile that:

- denies all network except the loopback listen socket;
- denies `file-write*` everywhere except a per-run, TraceLoupe-owned scratch dir
  (`<app-data>/models/sidecar-scratch`, wiped before and after each run) — so
  the prompt text (your messages/notes) has nowhere on disk to land;
- denies reads of user data outside the model, the binary, and scratch;
- redirects Metal's shader cache + temp files into scratch via
  `MTL_SHADER_CACHE_PATH` / `TMPDIR`, so GPU init still has a place to write.

A live `sandbox-exec` test (`server.rs`, `denies_writes_except_scratch`) asserts
the OS actually refuses a write outside scratch.
