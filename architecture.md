# Local iOS Backup Browser — Architecture

*Working codename: TraceLoupe · Status: Draft v0.2 · Companion to the Product & Architecture Description*

---

## 1. Purpose & scope

This document describes the technical architecture: the components, their boundaries, and how data flows from an encrypted iOS backup to the screen. It assumes the product context from the Product & Architecture Description and does not repeat product rationale except where it drives a structural choice.

Three principles shape everything below:

1. **Native macOS only.** A single Tauri v2 app. No web tier, no server, no cross-platform abstraction. The frontend calls the Rust backend directly over Tauri IPC.
2. **Engine first, then native.** The MVP reuses iLEAPP as a headless parsing engine (broad coverage, zero parsers to write). A native lazy-decode core is added later for the artifacts people open most.
3. **Decode on demand (Phase 2).** The native path never bulk-extracts: the manifest is indexed once; individual files are decrypted lazily on access and cached thereafter.

## 2. Architectural style

- **Single native app** — React UI in a Tauri webview, Rust backend, one IPC boundary between them.
- **UI-agnostic core** — all parsing/decryption logic sits in a standalone Rust crate with no Tauri or UI dependency, so it unit-tests on any CI. The Tauri command layer is a thin wrapper over it.
- **Two-phase parsing** — Phase 1 imports via the iLEAPP sidecar into a cache DB; Phase 2 adds native manifest-indexed, on-demand decryption for hot artifacts. Both feed the same cache and UI.
- **Lazy pipeline with a cache** — an index/cache layer sits between the backup and the UI so repeat access is a query, not a re-parse or re-decrypt.

## 3. System context (C4 level 1)

```
                          ┌───────────────────────────────────┐
                          │              The user             │
                          │   (owns the phone & the backup)   │
                          └───────────────┬───────────────────┘
                                          │ opens, browses, searches, exports
                                          ▼
        ┌─────────────────────────────────────────────────────────────────┐
        │            Local iOS Backup Browser  (native macOS app)          │
        │                        Tauri v2 · one binary                     │
        └───────┬─────────────────────────────────────────────┬───────────┘
                │ reads (decrypts locally)                     │ optional, once
                ▼                                              ▼
   ┌──────────────────────────┐                  ┌──────────────────────────┐
   │  Encrypted iOS backup    │                  │  Acquisition helper CLI  │
   │  (Finder / MobileSync    │◀── creates ──────│  pymobiledevice3         │
   │   or CLI-produced)       │                  │  (libimobiledevice f/b)  │
   └──────────────────────────┘                  └───────────┬──────────────┘
                                                             │ USB + Trust
                                                             ▼
                                                  ┌──────────────────────────┐
                                                  │        iPhone            │
                                                  │  (broken screen OK)      │
                                                  └──────────────────────────┘

   No network dependency for operation. Nothing leaves the machine.
```

## 4. Container view (C4 level 2)

```
┌───────────────────────────────────────────────────────────────────────────┐
│                            PRESENTATION                                     │
│                                                                             │
│   React + shadcn/ui + Tailwind v4 (Vite, TypeScript) — in Tauri webview     │
│   Views: Gallery │ Messages │ Contacts │ Calls │ Safari │ Notes │ Browser   │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                │ @tauri-apps/api  invoke(command)
                                ▼
                     ┌─────────────────────────┐
                     │  Tauri command layer    │   (thin Rust wrapper)
                     │  (Rust)                 │
                     └───────────┬─────────────┘
                                 ▼
        ┌───────────────────────────────────────────────────────────┐
        │                    CORE (Rust crate)                       │
        │              no UI / no shell dependencies                 │
        │                                                            │
        │   Manifest Index · Decryptor · Parsers · Cache · Search    │
        └───────────────────────────────────────────────────────────┘
                                 │
             ┌───────────────────┼───────────────────────┐
             ▼                   ▼                        ▼
   ┌──────────────────┐ ┌──────────────────┐  ┌──────────────────────────┐
   │ Encrypted backup │ │ Local cache DB   │  │  Sidecar processes       │
   │ (read-only)      │ │ (SQLite: index + │  │  iLEAPP (MVP engine),    │
   │                  │ │  parsed cache)   │  │  Notes parser (later)    │
   └──────────────────┘ └──────────────────┘  └──────────────────────────┘
```

The only platform-specific code on the read path is the Tauri command layer. Everything of substance is in the UI-agnostic core.

## 5. The core (C4 level 3)

The core crate has no knowledge of Tauri or the UI. It exposes use-cases (`import_backup`, `open_backup`, `list_threads`, `get_note`, `get_media`, `search`, `export`) and is organized into components:

```
┌──────────────────────────────────────────────────────────────────────┐
│                            CORE CRATE                                  │
│                                                                        │
│  ┌────────────────┐   unlocks    ┌──────────────────┐                  │
│  │ Manifest Index │◀─────────────│    Decryptor     │                  │
│  │ (Phase 2)      │  file keys   │  (keybag / AES)  │                  │
│  │ domain+path →  │─────────────▶│  decrypt 1 file  │                  │
│  │ fileID + key   │   locate     └───────┬──────────┘                  │
│  └───────┬────────┘                      │ plaintext bytes             │
│          │                               ▼                             │
│          │                       ┌──────────────────┐                  │
│          │                       │     Parsers      │                  │
│          │                       │  native: SQLite, │                  │
│          │                       │  plist, Notes,   │                  │
│          │                       │  media/thumbs    │                  │
│          │                       └───────┬──────────┘                  │
│          │                               │                             │
│          │        ┌──────────────────────┘                             │
│          │        │   iLEAPP sidecar (MVP): whole-backup parse         │
│          │        │   ──▶ _lava_artifacts.db ──┐                       │
│          ▼        ▼                            ▼                       │
│  ┌──────────────────────────────────────────────────┐                 │
│  │                   Cache / Index                    │                │
│  │   SQLite: file index, parsed artifacts, thumbs     │                │
│  │   populated once · read on every access            │                │
│  └───────────────────────┬────────────────────────────┘                │
│                          │ feeds                                       │
│                          ▼                                             │
│                  ┌──────────────────┐                                  │
│                  │   Search (FTS)   │                                  │
│                  └──────────────────┘                                  │
└──────────────────────────────────────────────────────────────────────┘
```

**Component responsibilities**

- **Manifest Index** *(Phase 2)* — decrypts only `Manifest.db` once; maps every `domain/relativePath` to its `fileID` and per-file key. The backbone of lazy access.
- **Decryptor** *(Phase 2)* — unwraps the keybag with the backup password and decrypts a *single* requested file to bytes. Never walks the whole backup.
- **Parsers** — native parsers turn plaintext bytes into structured records (SQLite via `rusqlite`, plist, Notes protobuf, media/thumbnails). In the MVP most parsing is delegated to the iLEAPP sidecar instead.
- **Cache / Index** — a local SQLite store holding the file index, parsed artifacts, and thumbnails. Populated by the iLEAPP import (MVP) or by native lazy parsing (Phase 2); read on every access.
- **Search** — full-text index built over cached artifacts.

## 6. MVP flow — iLEAPP import, then instant browse

```
 First open of a backup (one time):

 UI ──invoke(import_backup)──▶ Core ──spawn──▶ iLEAPP sidecar (headless)
                                                   │  decrypt + parse
                                                   │  (selected modules)
                                                   ▼
                                            _lava_artifacts.db
                                                   │
                              Core reads/normalizes into Cache DB
                                                   │
                                          progress ▶ UI (progress bar)

 Every browse afterward:

 UI ──invoke(list_threads / get_note / ...)──▶ Core ──▶ Cache DB query ──▶ UI
                                                              ⟵ instant
```

The import is the one eager, whole-backup pass. It is bounded by running only the modules the product surfaces. After import, every view is a cache query.

## 7. Phase 2 flow — on-demand decode (native hot path)

Example: user opens the **Messages** view, native path enabled.

```
 UI ──invoke(list_threads)──▶ Core: list_threads
   │
   ├─▶ Cache hit?  ──yes──▶ return cached threads ──▶ UI   ⟵ instant
   │
   └─no─▶ Manifest Index: locate HomeDomain/Library/SMS/sms.db
             │
             ▼
          Decryptor: decrypt just sms.db  (one file)
             │
             ▼
          Parser: query messages / handles / chats
             │
             ▼
          Cache: persist parsed threads + build FTS
             │
             ▼
          return threads ──▶ UI    ⟵ first time only
```

Only `Manifest.db` + `sms.db` are decrypted for this view; media is untouched; the second visit is a cache hit. This removes the MVP's one-time import wait for the artifacts people open most.

## 8. Media flow — deferred resolution (Phase 2)

```
 Gallery view                 open one item
   │ request thumbnails            │ request full-res
   ▼                               ▼
 Core: get_media(thumb)        Core: get_media(full)
   │                               │
   ├─ cache hit ─▶ return          ├─ cache hit ─▶ return
   │                               │
   └─ miss ─▶ decrypt file ─▶ downscale ─▶ cache ─▶ return
                                   └─ miss ─▶ decrypt file ─▶ cache ─▶ return
```

Thumbnails are generated and cached on first gallery paint; full-resolution bytes are decrypted only when an item is opened. Media — the bulk of a backup's size — is never decrypted en masse.

## 9. Sidecar boundary

Some parsing is cleanest to reuse rather than reimplement. In the MVP this is the whole parsing engine; later it narrows to the long tail and the Notes protobuf. Sidecars run as **separate processes** invoked by the core, exchanging files/JSON/SQLite — never linked into the binary.

```
   Core ──spawn──▶ Sidecar (frozen binary, headless)
        ◀─SQLite/JSON──  iLEAPP (MVP engine) · apple-notes-parser (later)
```

Rationale: keeps licensing clean (no copyleft linking), isolates crashes, and lets iLEAPP act as a headless engine whose `_lava_artifacts.db` the core reads like any other SQLite source. The roadmap progressively replaces sidecar parsing with native Rust where instant first-open matters, and replaces the Notes sidecar with a pure-Rust parser.

**Sidecar acquisition — download-on-first-use, not bundled.** The iLEAPP sidecar is not shipped inside the app bundle. On first import, the app downloads a **pinned, re-frozen iLEAPP build** (hosted as our own release asset — see note), verifies its SHA-256 against a checksum pinned in the app, stores it under Application Support, and runs it from there. The download is shown to the user with version and source information before it happens. Rationale: keeps the .app small, avoids notarizing a frozen Python blob inside our bundle, and lets the pinned iLEAPP version be bumped without an app release. The pin matters because `_lava_artifacts.db` is not a stable public API — the core's normalizer is written against a specific iLEAPP version. A settings escape hatch lets power users point at their own iLEAPP binary instead. Cost: the first import requires network access — a one-time, user-visible exception to the otherwise fully-offline operation.

> **Note (Milestone 1 spike finding).** The upstream `ileapp-…-macOS_Apple_Silicon` release binary for v2026.1.0 is broken — it crashes on startup with a Pillow `ImageDraw` import error from its PyInstaller freeze, before parsing anything. Running iLEAPP from a source checkout works. We therefore host our **own** re-frozen iLEAPP build (upstream source, our freeze with Pillow correctly bundled) as the pinned download, rather than depending on upstream's macOS asset. This also strengthens the SHA-pinning story: the download source is under our control. See `docs/spike-ileapp.md`.

> **MVP decryption (spike finding).** iLEAPP decrypts encrypted backups itself via `--itunes_password` (full keybag → per-file AES path). So the MVP needs **no** native Decryptor; the Decryptor and Manifest Index (§5) are Phase-2-only.

## 10. Parser provenance

"Hand-written native parser" (Phase 2) means original Rust that reads an artifact — **not** a copy of iLEAPP's source pasted into this codebase. There are three distinct ways iLEAPP can contribute to a native parser, with different legal weight. Contributors must be explicit about which one applies to each parser.

```
  ┌────────────────────────────────────────────────────────────────┐
  │  (1) REFERENCE  → learn the format, write fresh Rust            │
  │      what's used: the reverse-engineered *facts*                │
  │        (domain/path, tables, columns, timestamp encoding)       │
  │      copyright: facts aren't copyrightable → clean, no notice   │
  │      → this is the DEFAULT and preferred path                   │
  ├────────────────────────────────────────────────────────────────┤
  │  (2) PORT  → translate iLEAPP's code line-by-line to Rust       │
  │      what's used: the *expression* (structure, logic, names)    │
  │      copyright: derivative work → MIT applies                   │
  │      obligation: include iLEAPP copyright + MIT text in NOTICES  │
  │      → allowed, but the file carries their attribution          │
  ├────────────────────────────────────────────────────────────────┤
  │  (3) SIDECAR  → run iLEAPP as a separate process, read output   │
  │      what's used: nothing of theirs is *in* the binary          │
  │      copyright: no combination, no derivative-work question     │
  │      → this is the MVP model                                    │
  └────────────────────────────────────────────────────────────────┘
```

**Practical test for (1) vs (2).** If iLEAPP's `.py` is open in one pane and the same variable names and control flow are being typed into Rust in the other, it is a port — treat it as (2) and attribute. If instead the *facts* are extracted into notes ("messages: `HomeDomain/Library/SMS/sms.db`; join `message`/`handle`/`chat_message_join`; dates = Mac absolute time × 10⁹") and implemented from those, it is original code — (1).

**Rules for this project.**
- Prefer (1). It keeps the codebase free of copied source and attribution obligations, and makes the eventual pure-Rust parsers genuinely ours.
- Where (2) is unavoidable, add the iLEAPP copyright notice and MIT permission text to a `THIRD-PARTY-NOTICES` file. MIT is permissive but **not** obligation-free — the notice must travel with distribution.
- Verify the license **per module**, not once. iLEAPP is MIT overall, but it is community-contributed; a specific parser could carry a different header or contain lifted logic. Check the actual file being learned from.
- Record the chosen path in each parser's source header (e.g. `// provenance: reference (own implementation)` or `// provenance: port of iLEAPP <module> — see THIRD-PARTY-NOTICES`).

## 11. Data stores

| Store | Type | Access | Contents |
|---|---|---|---|
| Encrypted backup | On-disk bundle | Read-only | `Manifest.db`, per-file blobs |
| iLEAPP report DB | SQLite (transient) | Read (MVP) | `_lava_artifacts.db` from a sidecar run |
| Manifest index | In-memory + cached | R/W (Phase 2) | domain/path → fileID + key |
| Cache DB | SQLite | R/W | parsed artifacts, thumbnails, FTS |

The source backup is always read-only; the app never writes to it and never touches the source device.

## 12. Cross-cutting concerns

- **Security/privacy** — no network on the read path; the one network exception is the one-time, user-visible, checksum-verified download of the pinned iLEAPP sidecar (§9); the backup password lives only in memory to unwrap keys; source backup is read-only; sidecars are auditable and can run network-disabled.
- **Performance** — MVP confines cost to a one-time import (limited to needed modules); Phase 2 adds manifest-first indexing, single-file lazy decryption, deferred media, cache-once, and Rust parallelism for the first-access hit.
- **Testability** — the core crate is pure logic, unit-tested on any CI (incl. Linux) with no GUI; the frontend is E2E-tested with mocked IPC in Chromium **and** WebKit; native-shell E2E via WebdriverIO + tauri-driver (Win/Linux CI) and the tauri-playwright bridge (macOS).
- **Extensibility** — third-party app coverage comes from iLEAPP modules in the MVP; native parsers are added behind a common parser interface without touching the pipeline.
- **Error isolation** — a single unreadable/corrupt file is logged and skipped, never aborting a view or an import.

## 13. Key constraints (architecture-relevant)

- **One-time import cost (MVP)** — the iLEAPP pass is eager and whole-backup; browsing is instant only after it completes.
- **First-access cost (Phase 2)** — the first open of a natively-parsed artifact must decrypt it; only cached re-access is instant. No zero-cost read of an encrypted backup exists.
- **iOS ceiling** — `.ipa` binaries, `Caches/`, backup-excluded files, and some Secure-Enclave secrets are absent from any backup.
- **macOS WKWebView** — no WebDriver/CDP for the native shell, shaping the testing approach.
- **Full Disk Access** — reading the protected `MobileSync` path requires granting the host app FDA, or working from a copied backup.

## 14. Component-to-technology map

| Component | Technology |
|---|---|
| Presentation | React, TypeScript, shadcn/ui, Tailwind v4, Vite |
| UI ↔ core | `@tauri-apps/api` invoke → Tauri v2 command layer (Rust) |
| Core | Rust crate: Manifest Index, Decryptor, Parsers, Cache, Search |
| SQLite access | `rusqlite` |
| Cache / index | SQLite (+ FTS) |
| Parsing engine (MVP) | iLEAPP (headless sidecar; pinned GitHub release, downloaded on first use) |
| Notes / native parsing (later) | apple-notes-parser → pure-Rust |
| Decryption (Phase 2) | iOSbackup approach |
| Acquisition | pymobiledevice3 (fallback libimobiledevice) |
