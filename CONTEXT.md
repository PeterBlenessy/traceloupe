# TraceLoupe

Desktop (Tauri + Rust + React) analyzer for iOS device backups. This glossary
captures the domain language we've sharpened; it grows as decisions crystallize.
Seeded 2026-07-20 from the iCloud-offloaded-media design session.

## Language

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

## Relationships

- **Offloaded media** is recovered by either **Sanctioned Export** (T1, default)
  or **Live Fetch** (T2, opt-in).
- **Live Fetch** covers **Notes** and **Photos** (ported from pyicloud). It does
  **not** cover **Messages** — Messages in iCloud is end-to-end encrypted (its
  CloudKit Service Key lives in iCloud Keychain / backup escrow) and has no
  open-source reference; that is a separate research spike.
- **Live Fetch** risks **Account lockout**; **Sanctioned Export** does not.

## Example dialogue

> **Dev:** "The note says 4 images but the gallery shows 1 — is that a parser bug?"
> **Domain:** "No — `image_count` is 4 (**referenced**), `available_image_count`
> is 1 (**present**). The other 3 are **offloaded media**. To get them you'd
> either import a **Sanctioned Export** or turn on **Live Fetch** — and Live Fetch
> can lock the account, so it's opt-in."

## Flagged ambiguities

- "iCloud access" was used to mean both the sanctioned Data & Privacy export and
  the private authenticated protocol — resolved: these are **Sanctioned Export**
  (T1) and **Live Fetch** (T2), materially different in legality, risk, and UX.
