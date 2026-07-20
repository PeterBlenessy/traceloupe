# TraceLoupe

A privacy-first macOS app that opens, decrypts, and renders the contents of a user's own iPhone backup locally. This context covers the domain language of the app, currently focused on the Spyware Analyzer feature.

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

## Relationships

- A **Feed** provides many **Indicators**
- An **Explicit Scan** evaluates all **Indicators** and produces zero or more **Findings**
- A **Passive Check** is a scope-restricted scan triggered by import (consented, configurable), producing **Findings** the same way
- A **Finding** references exactly one **Indicator** and one source artifact (message, app, history row, …)

## Example dialogue

> **Dev:** "Import finished and flagged an app — was that an **Explicit Scan**?"
> **Domain expert:** "No, that's a **Passive Check**: apps-only by default, and only because the user consented. An **Explicit Scan** is the full pass the user starts from the Security view, and it's the only place weak Indicators like visited domains may surface as **Findings**."

## Flagged ambiguities

- "Fully local / offline" was used to mean both "backup data stays local" and "the app never touches the network" — resolved: the **Privacy promise** covers backup-derived data only; disclosed operational traffic is allowed. Existing docs (product-architecture-description.md §5, PRD §8) use the overstated phrasing and should be updated.

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
