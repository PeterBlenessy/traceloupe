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
Anything a detection feature surfaces to the user; comes in two kinds — **Indicator Finding** and **Content Finding**.
_Avoid_: alert, detection, hit

**Indicator Finding**:
A single deterministic Indicator match in the user's data (Security Check), carrying severity, threat attribution, and a reference to the source artifact.
_Avoid_: treating it as probabilistic — it either matched or it didn't

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

### Safety Scan

**Safety Scan**:
The user-facing content-analysis feature (sidebar sibling of Security Check): a manually started, local-only run that classifies backup text (messages, notes) into the Forensic 9, optionally restricted to a time range.
_Avoid_: "AI scan", "moderation", "the agent" (there is no agent — see Stateless classifier)

**Forensic 9**:
The fixed classification taxonomy: threat-violence, harassment-bullying, sexual-content, grooming-exploitation, self-harm, hate-identity, coercive-control, scam-fraud, drugs-illegal.
_Avoid_: "toxicity categories" (moderation framing; this is forensic review of history, not live moderation)

**Content Finding**:
A model-produced classification attached to one message or note: a Forensic 9 category, severity 1–3, and a one-line rationale.
_Avoid_: treating it as deterministic — it is a probabilistic verdict from a local model and can be dismissed as a false positive

**Stateless classifier**:
The role the local model plays: deterministic Rust code selects text and feeds it one Chunk at a time; the model has no tools, no queries, no memory across calls — it sees the Chunk, returns verdicts, forgets.
_Avoid_: "agent", "the AI reads the backup" (it is only ever handed Chunks)

**Chunk**:
A contiguous window of ~25 messages from one conversation (with sender labels and timestamps), sent as a single classification unit; verdicts still attach to individual messages. Notes are classified individually.
_Avoid_: "batch" (a batch of unrelated messages would destroy the context that pattern categories need)

**Analysis store**:
A per-backup sidecar SQLite DB (analysis.db) holding Content Findings, scan progress, and summaries, keyed by stable identifiers plus text fingerprints. Survives re-import — unlike cache-DB tables — because a full scan costs hours of local compute.
_Avoid_: "the cache" (that is the parse cache, rebuilt at will)

**Content-free audit log**:
A record of what a Safety Scan did — which identifier ranges were classified, when, with which model, and verdict counts — that never contains message text.
_Avoid_: "scan log" (ambiguous about whether content is logged; it never is)

**Scan report**:
The natural-language summary the model writes at the end of a run from the verdict list: counts per category/severity, most serious Content Findings, notable patterns. Flagged threads additionally get a short per-thread summary.

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
- A **Passive Check** is a scope-restricted scan triggered by import (consented, configurable), producing **Indicator Findings** the same way
- An **Indicator Finding** references exactly one **Indicator** and one source artifact (message, app, history row, …)

Safety Scan:

- A **Safety Scan** feeds **Chunks** to the **Stateless classifier** and produces zero or more **Content Findings**, a **Scan report**, and a **Content-free audit log**
- A **Content Finding** references exactly one message or note and one **Forensic 9** category; several can attach to the same message
- **Content Findings** live in the **Analysis store**, never in the parse cache — the deliberate opposite of Security Check's cache-DB findings, because model verdicts are hours-expensive to recompute and Indicator Findings are cheap
- **Safety Scan** and **Security Check** are sibling detection features; both surface **Findings**, of different kinds and trust levels (probabilistic vs deterministic)

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

> **Dev:** "Can the Safety Scan agent pull in the rest of a thread when it needs more context?"
> **Domain expert:** "There is no agent — the model is a **Stateless classifier**. If a category needs context, the context is already in the **Chunk** we hand it; it can't ask for more. And an 'ok, come over' flagged as **coercive-control** is a **Content Finding** — probabilistic, dismissible — not an **Indicator Finding** like a Pegasus domain match."

> **Dev:** "The note says 4 images but the gallery shows 1 — is that a parser bug?"
> **Domain:** "No — `image_count` is 4 (**referenced**), `available_image_count`
> is 1 (**present**). The other 3 are **offloaded media**. To get them you'd
> either import a **Sanctioned Export** or turn on **Live Fetch** — and Live Fetch
> can lock the account, so it's opt-in."

## Flagged ambiguities

- "Fully local / offline" was used to mean both "backup data stays local" and "the app never touches the network" — resolved: the **Privacy promise** covers backup-derived data only; disclosed operational traffic is allowed. Existing docs (docs/product-overview.md §5, PRD §8) use the overstated phrasing and should be updated.
- "iCloud access" was used to mean both the sanctioned Data & Privacy export and
  the private authenticated protocol — resolved: these are **Sanctioned Export**
  (T1) and **Live Fetch** (T2), materially different in legality, risk, and UX.

- "Finding" was defined as an Indicator match, then Safety Scan needed the same word for model verdicts — resolved: **Finding** is the umbrella; **Indicator Finding** (deterministic) and **Content Finding** (probabilistic) are the kinds.
- "Agent" was used loosely for the Safety Scan worker — resolved: it is a **Stateless classifier** driven by a deterministic Rust pipeline; "agent" implies model-driven control flow, which was explicitly rejected (ADR 0002).

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

Safety Scan (2026-07-21, grill session; architecture in ADR 0002):

- Serves all four jobs — safety review, abuse evidence, investigator triage, self-audit — hence the unified Forensic 9 taxonomy rather than a moderation lens.
- Local-only inference: Gemma 4 E4B (Q4_K_M) via a sandboxed llama-server sidecar; Gemma 4 E2B offered as the low-RAM fallback; GGUFs fetched on first use through the existing verified-download path.
- Deterministic pipeline, no agent framework; one closing summary pass (Scan report + per-flagged-thread summaries) is the only non-classification LLM use.
- Unit of classification is the Chunk (~25 messages with context); v1 sources are messages (all chat apps) and notes; Safari history and calendar deferred.
- Scans are manual-only, cover everything by default (newest conversations first), are pause/crash-resumable per Chunk, incremental on re-scan via text fingerprints, and accept a user-chosen time range (year or month span).
- Verdicts persist in the per-backup Analysis store (analysis.db), not cache.db, so re-import cannot destroy hours of classification work.
- llama-server runs under a macOS Seatbelt profile denying all network except its loopback listen socket and all file reads outside the model directory; server logging off; prompt text never written to app logs; every run leaves a Content-free audit log.
- Findings surface on a new Safety Scan page (run controls, progress, findings by category/severity, Scan report, false-positive dismissal) plus inline badges in Messages/Notes deep-linking into threads.
- Validation is hybrid: public datasets where distribution fits (Jigsaw → hate/harassment, PAN12 → grooming, threat corpora) plus a hand-labeled synthetic fixture set in-repo for coercive-control, scam-fraud, self-harm, and hard negatives; the fixture harness gates prompt changes in CI.
