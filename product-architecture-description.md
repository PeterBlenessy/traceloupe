# Local iOS Backup Browser

**Product & Architecture Description**

*Working codename: TraceLoupe · Status: 0.2.0 (native lazy-decode core wired in alongside iLEAPP) · Platform: macOS (desktop, Tauri v2)*

---

## 1. Executive summary

A privacy-first application that lets people open, decrypt, and read the contents of their own iPhone backup — photos, messages, contacts, call history, Safari history, and Notes — through a clean, native-feeling interface. All processing happens locally; no data leaves the machine.

It is a native macOS desktop app built on Tauri v2, with all parsing/decryption logic in a UI-agnostic Rust core reached over Tauri IPC. The MVP is powered by iLEAPP as a headless parsing engine — reusing its hundreds of maintained artifact parsers — with a native lazy-decryption core introduced as a later performance evolution. The product targets the gap between forensic command-line tools (powerful but unfriendly and investigator-oriented) and commercial closed-source utilities (convenient but subscription-based and requiring you to hand device data to a third party).

## 2. Problem & opportunity

When an iPhone's screen breaks or a device is retired, its data survives in an encrypted local backup — but that backup is opaque. The payload is a directory of hash-named files indexed by a SQLite manifest. App data lives in SQLite databases and binary property lists, and some content — notably Apple Notes — is stored as gzip-compressed protobuf blobs that are unreadable without a dedicated parser.

Existing options fall into two camps: forensic tools (e.g. iLEAPP) that are thorough but produce static reports for investigators, and commercial GUIs that are polished but closed-source, subscription-based, and require trusting a third-party product with device data. There is room for an open, local, well-designed consumer tool.

## 3. Product overview

The application ingests an encrypted iOS backup (created by Finder or by an included command-line helper), decrypts it locally using the backup password, and presents its contents through a browsable interface: a photo gallery, message threads, a contacts list, call history, Safari history, and fully rendered Notes. Users can search and export. It is a self-contained native macOS app; all processing is on-device.

## 4. Target users

- Individuals recovering data from a broken or retired iPhone.
- Privacy-conscious users who prefer local, open, auditable tooling over cloud services.
- Technically literate power users who want direct access to their own device data.

## 5. Goals & non-goals

**Goals**

- Read the substantive contents of an encrypted iOS backup through a friendly UI.
- **Ship broad coverage fast** by reusing an existing parsing engine (iLEAPP) rather than hand-writing parsers for the MVP.
- **Feel fast:** after a one-time import, browsing is instant; the later native core adds on-demand decryption so first-open of high-traffic artifacts is quick too.
- Keep 100% of processing local — no telemetry, no cloud.
- A single, self-contained native macOS app (Tauri v2) — no web/server tier.
- Reuse proven, auditable parsing rather than reinventing fragile format logic.

**Non-goals**

- Not a forensic chain-of-custody tool.
- Not a device-management or sync utility.
- Not an attempt to recover data that iOS never places in a backup.
- Not a web or cross-platform app — macOS-native only, deliberately.

## 6. Key features

- Open and decrypt an encrypted backup (password-based), or point at an already-extracted tree.
- Photo and video gallery from the media domains.
- Message threads (SMS/iMessage) from `sms.db`.
- Contacts, call history, and Safari history.
- Notes rendered with formatting and embedded media (protobuf-aware).
- Broad third-party app coverage via the iLEAPP engine (see §8).
- Per-app data browser for ad-hoc inspection of arbitrary SQLite/plist files.
- Search across artifacts and export to standard formats.
- Fully local, offline operation.

## 7. How it works — data pipeline

The product evolves through two phases that share the same UI and Rust core.

**Phase 1 (MVP) — iLEAPP engine + one-time import**

1. **Acquire** — an encrypted full backup via Finder or the bundled CLI helper (pymobiledevice3 preferred, libimobiledevice fallback). Encryption is required to capture keychain, Health, and the fullest per-app data.
2. **Import** — the app runs iLEAPP as a headless sidecar against the backup (only the needed modules, to limit time), producing its structured `_lava_artifacts.db`. This is a one-time pass shown with a progress bar.
3. **Index the report** — the core reads iLEAPP's output DB into its own cache/index store.
4. **Present** — the native UI renders artifacts by querying the cached DB. Because parsing already happened at import, browsing is instant.

**Phase 2 — native lazy-decode core (performance evolution)**

For high-traffic artifacts (messages, media, Notes), a native Rust path replaces the eager import so first access is fast without a full upfront pass:

1. **Index** — decrypt only the small `Manifest.db` once into an index of every file's domain, path, and per-file key.
2. **Decode on demand** — when a view is opened, decrypt just the file(s) it needs (e.g. `sms.db`) using the index. The full backup is never bulk-extracted.
3. **Parse** — SQLite queried directly; media read as-is (thumbnails on demand, full-resolution only on open); Notes decoded by a protobuf-aware parser.
4. **Cache** — parsed results persist locally, so re-opening a view is a millisecond query.

iLEAPP remains the engine for the long tail of third-party app artifacts not yet hand-written in the native core, running as a background indexer.

> **Access model.** Phase 1 pays a one-time import cost, then browses instantly. Phase 2 removes even that for the artifacts people open most. See §8.6 and §8.7.

> **iOS extraction ceiling.** App binaries (`.ipa`), `Caches/` directories, files an app flagged do-not-back-up, and some Secure Enclave–bound secrets are absent from *any* backup by design. No tool can recover them, and the product does not claim to.

## 8. Architecture choices

Each decision below is stated with its rationale and its main tradeoff.

### 8.1 Desktop shell — Tauri v2

**Decision.** Tauri v2 (Rust backend + system webview), not Electron.

**Rationale.** Small binaries, low memory footprint, a memory-safe Rust backend, the platform's native webview, and first-class mobile targets. This aligns with a privacy-first, resource-light product.

**Tradeoff.** Tauri uses the platform's native webview (WKWebView on macOS), which complicates browser-based end-to-end testing and means cross-webview rendering differences must be tested deliberately. See §9.

### 8.2 Frontend — React + shadcn/ui + Tailwind v4 + Vite + TypeScript

**Decision.** React with shadcn/ui components on Tailwind v4, built with Vite.

**Rationale.** shadcn/ui's "own-the-code" model — components copied into the repo, built on accessible Radix primitives — gives full design control for a polished, non-templated, Mac-grade UI, with a large ecosystem and strong TypeScript support. Tauri is frontend-agnostic, so the React app builds to static assets it loads directly.

**Tradeoff.** shadcn is a component set, not a batteries-included framework. Routing (React Router / TanStack Router) and data fetching (TanStack Query) are assembled explicitly. This is a deliberate trade of all-in-one convenience for control. (This choice replaces an earlier Vue 3 / Quasar 2 direction.)

### 8.3 Application core — standalone Rust crate

**Decision.** Business and parsing logic lives in a standalone Rust crate with no UI or shell dependencies, reached from the UI over Tauri IPC commands.

**Rationale.** A single source of truth for parsing, and a dependency-free crate that is trivially unit-testable on any CI without a GUI. Keeping it shell-agnostic (rather than hard-wiring it into the Tauri command layer) preserves clean testing and future portability, at effectively no cost.

**Tradeoff.** A thin interface boundary between the crate and the Tauri command layer — minimal overhead for the testing and clarity benefit.

### 8.4 Native macOS only — no web tier

**Decision.** Ship a single native macOS app. No web app, no server, no cross-platform abstraction layer.

**Rationale.** The product's core capability is direct local access to an encrypted backup on disk — precisely what a browser sandbox is worst at. Supporting web mode would have forced a ports-and-adapters indirection layer, a second (HTTP) transport, and File-System-Access/backend workarounds for the `MobileSync` folder, all to enable the one environment least suited to the task. Dropping it removes that entire layer: the frontend calls `invoke()` directly, there is one transport, and the app reads the disk natively.

**Tradeoff.** No browser-based access. Given the product is a local-disk tool, this is a net simplification rather than a real loss. (This reverses an earlier dual-target direction.)

### 8.5 Parsing & decryption — iLEAPP engine first, reuse over reinvent

**Decision.** For the MVP, use **iLEAPP as a headless parsing engine** rather than writing parsers: run it as a sidecar, then read its structured `_lava_artifacts.db` output. Over time, add native Rust parsers for high-traffic artifacts (SQLite via `rusqlite`, Notes via a protobuf-aware parser) while keeping iLEAPP for the long tail. Backup decryption is based on the iOSbackup approach in the native path.

**Rationale.** iLEAPP already parses hundreds of artifacts — messages, Safari, contacts, calls, Notes, and many third-party apps — and is actively maintained. Reusing it collapses the largest part of the build: the MVP writes no parsers and gets broad third-party app coverage (the Top-10/25/50 roadmap) essentially for free. The iOS Notes format in particular is a moving, complex protobuf target that forensic projects already track.

**Licensing posture.** iLEAPP is MIT-family (verify LICENSE); it is used here as a separate-process engine, which also keeps other components' licenses (e.g. LGPL iOSbackup) clean by avoiding static linking. Each source's LICENSE is verified before any code is incorporated.

**Sidecar delivery.** iLEAPP is not bundled into the app. On first import, the app downloads the exact pinned release binary from iLEAPP's official GitHub releases, verifies its checksum, and stores it locally — showing the user what is being downloaded, from where, and why before it happens. This keeps the app small, avoids shipping a frozen Python blob, and lets the pinned engine version be updated independently of app releases. Power users can point the app at their own iLEAPP install instead.

**Tradeoff.** iLEAPP's model is an eager, whole-backup report pass — the opposite of on-demand decoding — so the MVP accepts a one-time import cost (§8.6). The first import also requires network access for the one-time sidecar download (disclosed in-app; see §10). The native lazy path (§8.6) is introduced specifically to overcome the eager-import limitation for the artifacts that matter most.

### 8.6 On-demand decryption & caching (Phase 2)

**Decision.** After the iLEAPP-powered MVP, add a native path that never bulk-extracts: decrypt `Manifest.db` once to build a file index, decrypt individual files lazily — only when a view needs them — and cache parsed results so subsequent access is instant. This path targets high-traffic artifacts (messages, media, Notes) first.

**Rationale.** iLEAPP's import is an eager, whole-backup pass; most of that cost is spent on media the user never opens. The lazy path makes *first* access to the common artifacts fast, not just repeat access, closing the one gap the iLEAPP MVP leaves. The MVP ships first because breadth-fast matters more early; this is the follow-on optimization.

**Design points.**
- **Manifest-first indexing** — one small decrypt yields every file's location and per-file keys.
- **Lazy, targeted decryption** — decrypt only the file(s) a view needs, via the index; no whole-tree extraction.
- **Deferred media** — decode thumbnails on demand, full-resolution only on open.
- **Cache-once ingest** — persist parsed artifacts to a local SQLite store; re-access is a query, not a re-decrypt.
- **Parallelised native decryption** — Rust across cores for the initial per-artifact hit.
- **iLEAPP as background indexer** — retained for third-party app artifacts not yet natively parsed.

**Tradeoff.** More moving parts than reading iLEAPP's report alone — an index/cache layer and cache invalidation to manage. The first open of a natively-parsed artifact still costs its decryption; only repeat access is instant. There is no way to read an encrypted backup with zero upfront work.

### 8.7 Architecture decisions — summary

| Area | Decision | Key reason | Main tradeoff |
|---|---|---|---|
| Desktop shell | Tauri v2 (not Electron) | Small, fast, memory-safe, native webview | Native-webview E2E testing is harder |
| Frontend | React + shadcn/ui + Tailwind v4 + Vite | Own-the-code polish, a11y, ecosystem | Assemble routing/data yourself |
| Core logic | Standalone Rust crate over Tauri IPC | One source of truth, testable anywhere | Thin interface boundary |
| Platform | macOS-native only, no web tier | Core job is local disk access; web fits worst | No browser access |
| Parsing (MVP) | iLEAPP as headless engine | Broad coverage with zero parsers to write | Eager whole-backup import |
| Access model | On-demand decryption + cache (Phase 2) | Fast first-open; no bulk extract | Index/cache layer to manage |
| Decryption | iOSbackup approach (native path) | Handles keybag/AES correctly | Encrypted backup + password required |
| Access model | On-demand decryption + cache | Instant re-access; no bulk extract | Index/cache layer to manage |

## 9. Testing strategy

- **Frontend E2E** — Playwright against the built frontend on the dev server with the Tauri IPC layer mocked, run in both Chromium and WebKit projects. WebKit matters because macOS renders the app in WKWebView, not Chromium.
- **Core logic** — Rust unit tests on the dependency-free crate, runnable on any CI (including Linux) without a GUI.
- **Native-shell E2E** — WebdriverIO + tauri-driver on Windows and Linux CI. On macOS, WKWebView exposes no WebDriver or CDP, so real-app automation relies on the emerging tauri-playwright plugin (a socket bridge to the native webview) or remains manual.

This split gives dependable automated coverage without depending on a bundled Chromium, and isolates the one gap — the macOS native shell — explicitly.

## 10. Privacy & security posture

- All processing is local; the application makes no network calls to operate on backup data. The single exception is a one-time, user-visible download of the pinned iLEAPP engine from its official GitHub releases on first import (checksum-verified; version and source shown to the user). After that, the app runs fully offline.
- The backup password is used only in memory to unwrap file keys; it is never persisted or transmitted.
- No telemetry or analytics.
- The tool operates on user-provided copies of a backup; the source device is never written to.
- Dependencies are open and auditable, and third-party parsers can be run with the network disabled to empirically confirm no exfiltration.

## 11. Known limitations & constraints

- **iOS extraction ceiling** (§7): app binaries, `Caches/`, backup-excluded files, and some hardware-bound secrets are unavailable from any backup.
- **Encrypted-backup requirement**: the fullest data set requires an encrypted backup and its password; a lost password renders a backup unreadable by any tool.
- **One-time import cost (MVP)** (§8.5): the iLEAPP-powered MVP runs an eager parse pass on first import of a backup; browsing is instant afterward. Phase 2 removes this for high-traffic artifacts.
- **First-import network requirement (MVP)** (§8.5, §10): the pinned iLEAPP engine is downloaded on first use, so the very first import needs network access (or a manually supplied iLEAPP binary). All subsequent operation is offline.
- **First-access decryption cost (Phase 2)** (§8.6): even in the native path, the first open of a given artifact must decrypt it; only subsequent (cached) access is instant. No tool can read an encrypted backup with zero upfront work.
- **macOS native-shell E2E gap** (§9): no WebDriver/CDP for WKWebView.
- **macOS Full Disk Access**: reading Finder's protected `MobileSync` location requires granting the host app Full Disk Access, or working from a copied backup.

## 12. Technology summary

| Layer | Technology |
|---|---|
| Desktop shell | Tauri v2 (Rust) |
| Frontend | React + TypeScript, shadcn/ui, Tailwind v4, Vite |
| UI ↔ core | Tauri IPC commands |
| Core logic | Rust crate (`rusqlite`, plist, decompression/crypto) |
| Local cache / index | SQLite (manifest file-index + parsed-artifact cache) |
| Parsing engine (MVP) | iLEAPP (headless sidecar; pinned GitHub release, downloaded on first use) → `_lava_artifacts.db` |
| Decryption (Phase 2) | iOSbackup approach |
| Notes parsing (Phase 2) | Maintained Apple Notes protobuf parser |
| Acquisition helper | pymobiledevice3 (fallback: libimobiledevice) |
| Testing | Playwright (Chromium + WebKit), Rust unit tests, WebdriverIO + tauri-driver, tauri-playwright |

## 13. Roadmap & open questions

### 13.1 Third-party app artifact support

Beyond first-party data, the product will parse the locally stored artifacts of popular third-party apps — messages, media, timelines, contacts, and caches. Each app is delivered as an **independent artifact module** (mirroring the plugin model of tools like iLEAPP), so coverage grows additively from the core without touching it, and modules are community-extensible.

Rollout is tiered by app popularity and by recovery/forensic value:

| Tier | Coverage | Representative apps |
|---|---|---|
| 1 | Top 10 | WhatsApp, Instagram, Facebook Messenger, TikTok, Snapchat, Telegram, Signal, YouTube, Gmail, WeChat |
| 2 | Top 25 | *adds* X/Twitter, Discord, Reddit, Spotify, LinkedIn, Pinterest, Threads, Viber, LINE, Google Maps, and similar |
| 3 | Top 50 | *adds* Slack, Microsoft Teams, Zoom, Twitch, Tinder/Bumble/Hinge, PayPal/Venmo/Cash App, Uber, Amazon, Strava, Notion, and similar |
| 4 | Top 100 | *adds* the long tail: regional messengers, banking/fintech, travel, fitness, productivity apps, and popular games |

**Live coverage tracker.** Per-app status and the version each app gains native support are tracked in [`docs/app-support.md`](docs/app-support.md) — the living source of truth, updated as coverage lands.

**Prioritization criteria** — each tier is ordered by (1) user prevalence, (2) richness of locally stored artifacts, and (3) technical feasibility of parsing.

**Caveat.** Apps differ widely in what they persist locally. Some highly popular apps — Snapchat and Signal in particular — deliberately minimize or encrypt local storage, so recoverable artifacts may be limited to metadata or nothing at all. The roadmap targets what a backup actually contains, not what the app shows on-screen.

> The specific app lists above are representative examples, not a fixed ranking. The definitive per-tier list will be set from current app-store and usage rankings at planning time, since popularity shifts and varies by region.

### 13.2 Platform & core

- **`0.1.0` (MVP):** iLEAPP-powered import + native Tauri/shadcn UI over the cached report DB.
- **`0.2.0` (shipped):** native lazy-decode core for Messages, Notes, Recordings, and Camera roll (on-demand decryption, deferred media, cache-once), wired into the import **alongside** iLEAPP — which still supplies Calls, Safari, Apps, and third-party chats.
- **`0.3.0`+ native-first migration, in batches** — the plan for progressively replacing iLEAPP:
  - **Batch 1 (`0.3.0`):** native parsers for the remaining first-party views — Calls (`CallHistory.storedata`), Safari (`History.db`), Apps (app-state plist), and self-extracted Contacts (`AddressBook.sqlitedb`) via the Manifest Index; all built-in views then materialize natively and the redundant iLEAPP sms/notes passes are dropped so import time falls. Plus a first native third-party wave: TikTok (moved off iLEAPP), and Instagram, Facebook, Facebook Messenger, X/Twitter, Snapchat. WhatsApp and Telegram are deferred to `0.4.0` (they already read via iLEAPP). See [`docs/app-support.md`](docs/app-support.md) for per-app status.
  - **Batch 2 (`0.3.x`):** make iLEAPP **optional** — default install fully offline (no first-import download, no bundled ~222 MB engine); fetched on demand only for deeper third-party coverage. This keeps §8.5's breadth as opt-in rather than a hard dependency.
  - **Batch 3+ (`0.4.0`+):** native third-party app modules per the §13.1 tiers (Top 10 first), replacing iLEAPP coverage incrementally.
- Mobile targets via Tauri v2 (iOS/Android) — considered later, not part of the macOS-first scope.
