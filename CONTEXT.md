# TraceLoupe

Desktop (Tauri + Rust + React) analyzer for iOS device backups: opens,
decrypts, and renders the contents of a user's own iPhone backup locally. This
glossary captures the domain language we've sharpened; it grows as decisions
crystallize.

## Language

### Security Check

**Security Check**:
The user-facing detection feature (sidebar entry "Security"): Explicit Scans plus Passive Checks against spyware and stalkerware Indicators.
_Avoid_: Spyware Analyzer (superseded), virus scan, antivirus

**Indicator**:
A known-bad value (domain, URL, email, process name, file path, bundle ID, phone number) attributed to a named threat.
_Avoid_: IOC (in UI copy), signature, rule

**Feed**:
An external, publicly maintained source of Indicators (Amnesty investigations, Echap stalkerware-indicators, iMazing IOC repo).
_Avoid_: database, source list

**Explicit Scan**:
A user-initiated run that evaluates every Indicator class against the whole imported backup.
_Avoid_: analysis, audit, check-up

**Passive Check**:
A restricted detection pass that runs automatically at import when the user has consented; limited by default to app matching.
_Avoid_: background scan, auto-scan (it is part of import, not a background job)

**Finding**:
A single Indicator match in the user's data, carrying severity, threat attribution, and a reference to the source artifact.
_Avoid_: alert, detection, hit

**Privacy promise**:
The guarantee that backup-derived data never leaves the machine by default; disclosed app-operational traffic is permitted, and backup-derived exceptions exist only behind explicit per-feature opt-in (see ADR 0001).
_Avoid_: "fully offline", "no network" (both overstate the promise)

**Indicator update check**:
A setting-governed check against the Feeds that surfaces (and, at Explicit Scan start, applies) newer Indicators; sends nothing.
_Avoid_: sync, telemetry

**Mercenary spyware**:
State-grade commercial spyware sold to government actors (Pegasus, Predator, KingsPawn, …), detected chiefly via domain/process/path Indicators.

**Stalkerware**:
Consumer-grade covert monitoring apps installed by someone with physical access, detected chiefly via bundle-ID Indicators.

**Watchware**:
Monitoring apps that do not hide themselves (marketed as parental control); same detection surface as Stalkerware but lower implied severity.

### Offloaded media

**Offloaded media**:
Media that a backup only *references* — the metadata row exists, but the blob is
absent locally because iOS evicted it to iCloud ("Optimize Storage").
_Avoid_: "missing file", "cloud photo", "deleted".

**Referenced vs present**:
Two distinct counts for the same item. *Referenced* comes from the app's own
metadata (e.g. a note's `image_count`). *Present* means the blob actually
resolves in the backup `Manifest.db` (e.g. `available_image_count`). The gap
between them **is** the offloaded media.
_Avoid_: treating "referenced" as "available".

**Sanctioned Export (Tier 1 / T1)**:
Recovering offloaded media by importing the archive Apple produces via its
official Data & Privacy portal (privacy.apple.com → "Request a copy of your
data"). ToS-compliant, no credentials handled, but **asynchronous** (Apple
fulfils in ~7 days) and **bulk** (whole account, not per-item).
_Avoid_: "the API", "official API" (it is a file export, not a live API).

**Live Fetch (Tier 2 / T2)**:
Recovering offloaded media on demand by authenticating to Apple's **private**
iCloud protocol with the account owner's own Apple ID credentials (the
pyicloud/icloudpd model). Opt-in and consent-gated. Covers **Notes and Photos**;
**not** Messages.
_Avoid_: "the sanctioned path" (it is explicitly unsanctioned), "scraping".

**Account lockout**:
Apple's *automated security* lock triggered when tool-driven access looks like
unusual activity. Recoverable via normal account recovery — it is **not** a
legal or punitive ToS penalty. This, not litigation, is the real risk of Live
Fetch.
_Avoid_: "ban", "ToS enforcement action".

**Recovered blob**:
A media blob that was offloaded and later retrieved (via T1 or T2). It lives in
the **augmentation store** with explicit provenance and is **never** written back
into the read-only backup mirror.
_Avoid_: treating a recovered blob as backup-native.

**Augmentation store**:
A separate sidecar store (blob dir + index) keyed to a backup, holding recovered
blobs and fetched metadata alongside the existing SQLite cache. Exists so the
decrypted backup mirror stays strictly read-only.
_Avoid_: "the cache" (that is the parse cache), "the mirror".

## Relationships

Security Check:

- A **Feed** provides many **Indicators**
- An **Explicit Scan** evaluates all **Indicators** and produces zero or more **Findings**
- A **Passive Check** is a scope-restricted scan triggered by import (consented, configurable), producing **Findings** the same way
- A **Finding** references exactly one **Indicator** and one source artifact (message, app, history row, …)

Offloaded media:

- **Offloaded media** is recovered by either **Sanctioned Export** (T1, default)
  or **Live Fetch** (T2, opt-in).
- **Live Fetch** covers **Notes** and **Photos** (ported from pyicloud). It does
  **not** cover **Messages** — Messages in iCloud is end-to-end encrypted (its
  CloudKit Service Key lives in iCloud Keychain / backup escrow) and has no
  open-source reference; that is a separate research spike.
- **Live Fetch** risks **Account lockout**; **Sanctioned Export** does not.

## Example dialogue

> **Dev:** "Import finished and flagged an app — was that an **Explicit Scan**?"
> **Domain expert:** "No, that's a **Passive Check**: apps-only by default, and only because the user consented. An **Explicit Scan** is the full pass the user starts from the Security view, and it's the only place weak Indicators like visited domains may surface as **Findings**."

> **Dev:** "The note says 4 images but the gallery shows 1 — is that a parser bug?"
> **Domain:** "No — `image_count` is 4 (**referenced**), `available_image_count`
> is 1 (**present**). The other 3 are **offloaded media**. To get them you'd
> either import a **Sanctioned Export** or turn on **Live Fetch** — and Live Fetch
> can lock the account, so it's opt-in."

## Flagged ambiguities

- "Fully local / offline" was used to mean both "backup data stays local" and "the app never touches the network" — resolved: the **Privacy promise** covers backup-derived data only; disclosed operational traffic is allowed. Existing docs (product-architecture-description.md §5, PRD §8) use the overstated phrasing and should be updated.
- "iCloud access" was used to mean both the sanctioned Data & Privacy export and
  the private authenticated protocol — resolved: these are **Sanctioned Export**
  (T1) and **Live Fetch** (T2), materially different in legality, risk, and UX.

## Decisions log (pending ADRs)

- Detection runs in both modes — Explicit Scan and Passive Check — with a settings toggle (2026-07-20).
- Passive consent is gathered by a one-time question at the first app launch after the feature ships; on acceptance the apps-only Passive Check runs immediately against the existing cache (no re-import needed). The choice is changeable in Settings (2026-07-20).
- Passive scope defaults to apps-only (bundle IDs; configuration profiles once available); scope is user-configurable in Settings (2026-07-20).
- Findings and scan_runs are cache-DB tables; they share the cache lifecycle and are rebuilt after re-import (Passive Check repopulates; UI prompts to re-run the Explicit Scan) (2026-07-20).
- Indicator update checks are governed by one setting (default on): Explicit Scan fetches and applies fresh Feeds at start (offline fallback to local snapshot); other flows surface that updates are available rather than silently fetching (2026-07-20).
- Disclosure is both: a first-run consent dialog before the first fetch, plus a permanent inline note on the scan screen and the Settings toggle (2026-07-20).
- Findings carry the 3-level iMazing severity taxonomy: Critical / Warning / Info (2026-07-20).
- Echap bundle-ID/app indicators move into M1 so the Passive Check works at launch; the rest of the stalkerware surface (C2 domains, TCC cross-checks, profiles) stays M2 (2026-07-20).
- User-facing name is **Security Check** (sidebar "Security"); "spyware" remains prominent in view copy for findability. Internal artifacts (branch, PRD filename) keep the spyware-analyzer codename (2026-07-20).
- Shortened-URL expansion stays on the roadmap (M3) as an explicit, informed, per-feature opt-in (default off) — the sole sanctioned exception to the backup-data rule; ADR 0001 amended accordingly (2026-07-20).
