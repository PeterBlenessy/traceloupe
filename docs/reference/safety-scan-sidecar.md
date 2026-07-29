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

## How it dies (the teardown contract)

**TraceLoupe ends the sidecar with SIGKILL, and never asks it to shut down.**
That is not bluntness for its own sake — on the pinned build a graceful signal
is the *broken* path. llama-server's SIGTERM/SIGINT handler calls `exit()`,
which runs the ggml Metal teardown; with residency sets active (macOS 15+,
the default) that teardown does not survive being entered while inference is in
flight. Both outcomes are reproducible:

| outcome | what you see |
|---|---|
| **abort** | `ggml_metal_rsets_free` fails `GGML_ASSERT([rsets->data count] == 0)` → SIGABRT → macOS files a crash report against our sidecar |
| **wedge** | `exit()` blocks forever in `std::thread::join()` while a dispatch block spins in `__ggml_metal_rsets_init_block_invoke → usleep` — measured still stuck 5 minutes later |

The wedge is the worse half: the process keeps the GPU and the model's several
GB and never exits. So two mechanisms keep SIGKILL the only way in
(`safety_scan/reaper.rs`):

1. **Its own process group** (`process_group(0)` in `spawn`) — a signal aimed
   at *our* group (Ctrl-C in `tauri dev`, a closing terminal, a logout SIGTERM)
   cannot reach the sidecar, so it never has a graceful signal to wedge on.
2. **A process-wide reaper** — every live sidecar pid is registered and
   SIGKILLed from a SIGINT/SIGTERM/SIGHUP handler *and* from `atexit`.

Neither works alone. Isolation without the reaper turns every Ctrl-C into an
orphaned GPU server; the reaper without isolation still loses the race to the
group signal. `Drop` is the normal path but cannot be the guarantee: a scan
holds the server on a background thread, and nothing unwinds that stack when
the process is signalled or exits. That is what issue #31 actually recorded —
a sidecar reparented to `launchd`, still generating tokens 32 minutes after its
parent died.

Not fixable from here: `SIGKILL` on TraceLoupe itself, and `panic = "abort"`.

Upstream is aware (ggml-org/llama.cpp [#22593](https://github.com/ggml-org/llama.cpp/issues/22593),
[#19137](https://github.com/ggml-org/llama.cpp/issues/19137)); the proposed fix
([PR #22595](https://github.com/ggml-org/llama.cpp/pull/22595)) has been open
since May 2026 and addresses a missing `rsets_rm` pairing, not this
teardown-ordering case. **Bumping `LLAMA_CPP_VERSION` does not retire any of
this** — keep the contract whatever the pin says.

Verify it on hardware after a pin bump or any change to `spawn`/`shutdown`:

```bash
scripts/verify-sidecar-teardown.sh    # needs a staged binary + a real model
```

It drives both teardown shapes (group signal; app quits under a live scan
thread) and checks the sidecar is gone, in its own process group, and left no
crash report. The mechanism itself is covered in CI by the
`safety_scan::{server,reaper}` unit tests.

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
