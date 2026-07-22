# Local iOS Backup Browser — Architecture

*Working codename: TraceLoupe · Companion to the Product & Architecture Description*

> **Current as of v0.29.0.** Imports are fully native Rust (~35s per backup,
> offline) and decryption is native, manifest-indexed, and on-demand. iLEAPP was
> retired as a runtime engine at v0.7.0 and is now a *development-time reference
> only* — cross-checking native parsers, never run, downloaded, or bundled. The
> **iLEAPP-sidecar import** described in §6 and the sidecar-acquisition machinery
> in §9 were the **retired 0.1–0.2 MVP** and are kept here as history; they are
> clearly marked. The only sidecar that runs today is the Safety Scan
> `llama-server` (§9). The parser-provenance rules in §10 remain the governing
> policy for how iLEAPP may inform a native parser. For milestone history see the
> [CHANGELOG](../CHANGELOG.md).

---

## 1. Purpose & scope

This document describes the technical architecture: the components, their boundaries, and how data flows from an encrypted iOS backup to the screen. It assumes the product context from the Product & Architecture Description and does not repeat product rationale except where it drives a structural choice.

Three principles shape everything below:

1. **Native macOS only.** A single Tauri v2 app. No web tier, no server, no cross-platform abstraction. The frontend calls the Rust backend directly over Tauri IPC.
2. **Native parsing.** All artifacts are parsed by original Rust parsers. The 0.1–0.2 MVP reused iLEAPP as a headless parsing engine to bootstrap coverage; that engine was retired at v0.7.0 and the native-first path is now the only one. iLEAPP survives only as a development-time reference for reverse-engineered facts (§10).
3. **Decode on demand.** The native path never bulk-extracts: the manifest is indexed once; individual files are decrypted lazily on access and cached thereafter. This is the only decryption path today.

## 2. Architectural style

- **Single native app** — React UI in a Tauri webview, Rust backend, one IPC boundary between them.
- **UI-agnostic core** — all parsing/decryption logic sits in a standalone Rust crate with no Tauri or UI dependency, so it unit-tests on any CI. The Tauri command layer is a thin wrapper over it.
- **Native manifest-indexed parsing** — the core indexes the manifest once, then decrypts and parses files on demand into a per-backup cache DB. (Historically the 0.1–0.2 MVP populated that cache via a one-time iLEAPP sidecar import, §6; that path is retired.)
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

   Backup-derived data never leaves the machine (ADR 0001). Disclosed
   operational traffic is allowed: indicator-feed fetches (Security Check),
   the Safety Scan model download, and the opt-in de-shortener (§12).
```

## 4. Container view (C4 level 2)

```
┌───────────────────────────────────────────────────────────────────────────┐
│                            PRESENTATION                                     │
│                                                                             │
│   React 19 + shadcn/ui + Tailwind v4 · TanStack Router/Query — Tauri webview │
│   Views (one unified toolbar): Photos · Messages · Contacts · Calls · Safari │
│     · Notes · Recordings · Calendar · Reminders · Health · Interactions      │
│     · Apps · Device · Security · Safety                                      │
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
        │   Manifest Index · Decryptor · Native Parsers · Cache ·    │
        │   Search · Security Check (analyzer) · Safety Scan         │
        └───────────────────────────────────────────────────────────┘
                                 │
             ┌───────────────────┼───────────────────────┐
             ▼                   ▼                        ▼
   ┌──────────────────┐ ┌──────────────────┐  ┌──────────────────────────┐
   │ Encrypted backup │ │ Per-backup DBs   │  │  Safety Scan sidecar     │
   │ (read-only)      │ │ cache.db +       │  │  llama-server (Gemma,    │
   │                  │ │ analysis.db      │  │  Seatbelt-sandboxed)     │
   └──────────────────┘ └──────────────────┘  └──────────────────────────┘
```

The only platform-specific code on the read path is the Tauri command layer. Everything of substance is in the UI-agnostic core.

## 5. The core (C4 level 3)

The core crate has no knowledge of Tauri or the UI. It exposes use-cases (`open_backup`, `list_threads`, `get_note`, `get_media`, `search`, `export`, plus the Security Check scan and Safety Scan use-cases) and is organized into components:

```
┌──────────────────────────────────────────────────────────────────────┐
│                            CORE CRATE                                  │
│                                                                        │
│  ┌────────────────┐   unlocks    ┌──────────────────┐                  │
│  │ Manifest Index │◀─────────────│    Decryptor     │                  │
│  │ (indexed once) │  file keys   │  (keybag / AES)  │                  │
│  │ domain+path →  │─────────────▶│  decrypt 1 file  │                  │
│  │ fileID + key   │   locate     └───────┬──────────┘                  │
│  └───────┬────────┘                      │ plaintext bytes             │
│          │                               ▼                             │
│          │                       ┌──────────────────┐                  │
│          │                       │  Native Parsers  │                  │
│          │                       │  SQLite, plist,  │                  │
│          │                       │  Notes protobuf, │                  │
│          │                       │  media/thumbs,   │                  │
│          │                       │  app-chat plugins│                  │
│          │                       └───────┬──────────┘                  │
│          │                               │                             │
│          ▼                               ▼                             │
│  ┌──────────────────────────────────────────────────┐                 │
│  │                   Cache / Index                    │                │
│  │   cache.db: file index, parsed artifacts, thumbs,  │                │
│  │   Security Check findings + scan-runs · read often │                │
│  └───────────────────────┬────────────────────────────┘                │
│                          │ feeds                                       │
│                          ▼                                             │
│                  ┌──────────────────┐                                  │
│                  │   Search (FTS)   │                                  │
│                  └──────────────────┘                                  │
└──────────────────────────────────────────────────────────────────────┘
```

**Component responsibilities**

- **Manifest Index** — decrypts only `Manifest.db` once; maps every `domain/relativePath` to its `fileID` and per-file key. The backbone of lazy access.
- **Decryptor** — unwraps the keybag with the backup password and decrypts a *single* requested file to bytes on demand, caching the result. Never walks the whole backup. Locked Apple Notes are decrypted on demand the same way.
- **Native Parsers** — original Rust parsers turn plaintext bytes into structured records: first-party artifacts (Messages, Notes, Contacts, Calls, Safari, Recordings, Photos/camera-roll, Calendar, Reminders, Health, Interactions, Apps) plus a pluggable app-chat framework for third-party chats (WhatsApp, Messenger, Instagram, TikTok, Telegram, Kik, imo, Threema, Viber, Teams, LinkedIn). SQLite via `rusqlite`, plist, Notes protobuf, media/thumbnails. (The retired MVP delegated parsing to an iLEAPP sidecar; see §6.)
- **Cache / Index** — the per-backup `cache.db` (SQLite) holding the file index, parsed artifacts, thumbnails, and Security Check findings/scan-runs; populated by native lazy parsing and read on every access.
- **Search** — full-text index built over cached artifacts.
- **Security Check (`analyzer`)** — native Rust spyware/stalkerware indicator engine (v0.20.0–0.28.0). Runs Explicit Scans and a consent-gated Passive Check over messages/Safari/apps/contacts/notes/calendar/interactions, a Manifest file sweep, and Tier-B artifacts; matches bundled + refreshable STIX2/Échap indicator feeds; writes severity-graded findings and scan-runs to `cache.db`.
- **Safety Scan** — local-AI content review of Messages & Notes (v0.29.0, Beta). Drives the sandboxed `llama-server` sidecar (§9) and persists findings, chunk progress, and summaries to the per-backup `analysis.db`.

## 6. MVP flow — iLEAPP import, then instant browse *(retired at v0.7.0)*

> **Historical.** This describes the 0.1–0.2 MVP. It was replaced by the native
> on-demand path in §7, which is the only import path today. iLEAPP is no longer
> run, downloaded, or bundled (see the status note at the top).

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

## 7. Native flow — on-demand decode (the current path)

Example: user opens the **Messages** view.

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

Only `Manifest.db` + `sms.db` are decrypted for this view; media is untouched; the second visit is a cache hit. This is what let the native path replace the MVP's one-time whole-backup import.

## 8. Media flow — deferred resolution

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

Sidecars run as **separate processes** invoked by the core, exchanging files/JSON/SQLite over a controlled boundary — never linked into the binary. The rationale is unchanged: clean licensing (no copyleft linking), crash isolation, and — for Safety Scan — the ability to confine the process in an OS sandbox. **Today the only sidecar is the Safety Scan `llama-server`.** The iLEAPP parsing sidecar below is historical.

```
   Core ──spawn──▶ Safety Scan sidecar: llama-server (Gemma GGUF)
        ◀─loopback HTTP──  under a macOS Seatbelt (sandbox-exec) profile
```

**Safety Scan `llama-server` (current).** Safety Scan runs local inference in llama.cpp's `llama-server`, bundled as a Tauri sidecar (`bundle.externalBin`) and always spawned inside a TraceLoupe-controlled **Seatbelt sandbox** (`sandbox-exec`). The profile denies all network except the loopback listen socket and denies `file-write*` everywhere except a per-run scratch dir that is wiped before/after each run — so message/note text has nowhere on disk to land. A shipped `.app` resolves *only* the bundled, statically-linked binary; env-override and `$PATH` fallbacks are compiled out of release builds. See [ADR 0002](adr/0002-safety-scan-local-pipeline.md) for the threat model and [the sidecar reference](reference/safety-scan-sidecar.md) for the dev/prod build split and the live sandbox test.

**iLEAPP parsing sidecar (retired at v0.7.0).** In the 0.1–0.2 MVP the core spawned a **pinned, re-frozen iLEAPP build**, downloaded on first import (SHA-256-pinned, stored under Application Support), and read its `_lava_artifacts.db` like any other SQLite source. That download was the one-time, user-visible network exception of the MVP. It has been fully replaced by native Rust parsers (§5): iLEAPP is no longer downloaded, bundled, or run — it remains a *development-time reference* only (§10). The historical spike findings (upstream macOS binary broken; iLEAPP self-decrypts via `--itunes_password`, so the MVP needed no native Decryptor) are recorded in `docs/research/spike-ileapp.md`.

## 10. Parser provenance

"Hand-written native parser" means original Rust that reads an artifact — **not** a copy of iLEAPP's source pasted into this codebase. There are three distinct ways iLEAPP can contribute to a native parser, with different legal weight. Contributors must be explicit about which one applies to each parser.

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
  │      → was the MVP model; retired at v0.7.0 (no longer used)     │
  └────────────────────────────────────────────────────────────────┘
```

**Practical test for (1) vs (2).** If iLEAPP's `.py` is open in one pane and the same variable names and control flow are being typed into Rust in the other, it is a port — treat it as (2) and attribute. If instead the *facts* are extracted into notes ("messages: `HomeDomain/Library/SMS/sms.db`; join `message`/`handle`/`chat_message_join`; dates = Mac absolute time × 10⁹") and implemented from those, it is original code — (1).

**Rules for this project.**
- Prefer (1). It keeps the codebase free of copied source and attribution obligations, and makes the eventual pure-Rust parsers genuinely ours.
- Where (2) is unavoidable, add the iLEAPP copyright notice and MIT permission text to a `THIRD-PARTY-NOTICES.md` file. MIT is permissive but **not** obligation-free — the notice must travel with distribution.
- Verify the license **per module**, not once. iLEAPP is MIT overall, but it is community-contributed; a specific parser could carry a different header or contain lifted logic. Check the actual file being learned from.
- Record the chosen path in each parser's source header (e.g. `// provenance: reference (own implementation)` or `// provenance: port of iLEAPP <module> — see THIRD-PARTY-NOTICES.md`).

## 11. Data stores

| Store | Type | Access | Contents |
|---|---|---|---|
| Encrypted backup | On-disk bundle | Read-only | `Manifest.db`, per-file blobs |
| Manifest index | In-memory + cached | R/W | domain/path → fileID + key |
| `cache.db` (per backup) | SQLite | R/W | parsed artifacts, thumbnails, FTS, Security Check findings + scan-runs |
| `analysis.db` (per backup) | SQLite | R/W | Safety Scan findings, chunk progress, summaries |

All SQLite access is via `rusqlite`. The source backup is always read-only; the app never writes to it and never touches the source device. *(Historical: the retired MVP also read a transient `_lava_artifacts.db` produced by an iLEAPP sidecar run — §6.)*

## 12. Cross-cutting concerns

- **Security/privacy** — backup-derived data never leaves the machine by default ([ADR 0001](adr/0001-privacy-promise-scope.md)). The disclosed operational-traffic exceptions are: indicator-feed fetches for Security Check and the GGUF model download for Safety Scan (both setting-governed), plus one opt-in backup-data exception — the shortened-URL de-shortener (default off, per-use consent). The backup password lives only in memory to unwrap keys; the source backup is read-only; the Safety Scan sidecar runs Seatbelt-sandboxed with network denied except loopback (§9).
- **Performance** — the native path uses manifest-first indexing, single-file lazy decryption, deferred media, cache-once, and Rust parallelism, so cost falls on first access and re-access is a query. (A full native import of a backup runs ~35s.)
- **Testability** — the core crate is pure logic, unit-tested on any CI (incl. Linux) with no GUI; the frontend is E2E-tested with mocked IPC in Chromium **and** WebKit; native-shell E2E via WebdriverIO + tauri-driver (Win/Linux CI) and the tauri-playwright bridge (macOS).
- **Extensibility** — third-party chat coverage comes from native app-chat parser modules behind a pluggable framework; new parsers are added behind a common interface without touching the pipeline. Security Check indicator sources and the Safety Scan taxonomy extend independently of the parsers.
- **Error isolation** — a single unreadable/corrupt file is logged and skipped, never aborting a view or an import.

## 13. Key constraints (architecture-relevant)

- **First-access cost** — the first open of a natively-parsed artifact must decrypt and parse it; only cached re-access is instant. No zero-cost read of an encrypted backup exists.
- **iOS ceiling** — `.ipa` binaries, `Caches/`, backup-excluded files, and some Secure-Enclave secrets are absent from any backup.
- **macOS WKWebView** — no WebDriver/CDP for the native shell, shaping the testing approach.
- **Full Disk Access** — reading the protected `MobileSync` path requires granting the host app FDA, or working from a copied backup.

## 14. Component-to-technology map

| Component | Technology |
|---|---|
| Presentation | React 19, TypeScript, shadcn/ui, Tailwind v4, TanStack Router/Query, Vite |
| UI ↔ core | `@tauri-apps/api` invoke → Tauri v2 command layer (Rust) |
| Core | `crates/traceloupe-core` (Rust): Manifest Index, Decryptor, Native Parsers, Cache, Search, Security Check, Safety Scan |
| SQLite access | `rusqlite` |
| Data stores | `cache.db` (+ FTS) and `analysis.db`, per backup |
| Parsing | native Rust parsers (first-party artifacts + pluggable app-chat framework) |
| Decryption | native, manifest-indexed, on-demand, cached |
| Security Check | native Rust `analyzer` engine; bundled + refreshable STIX2 / Échap indicator feeds |
| Safety Scan | Gemma GGUF via `llama-server` Tauri sidecar under a Seatbelt (`sandbox-exec`) profile |
| Acquisition | pymobiledevice3 (fallback libimobiledevice) |
| Reference (dev only) | iLEAPP — cross-checking native parsers; never run/bundled (§10) |
