# Live Fetch (Tier 2) — pyicloud → native Rust porting spec

**Status:** research complete; implementation not started (blocked only on a
separate repo + a test Apple ID for end-to-end validation, **not** on knowledge).
**Scope:** the M2 "Live Fetch" crate from
[`docs/icloud-offloaded-media-prd.md`](./icloud-offloaded-media-prd.md) §6.3.
**Source of truth:** the maintained `icloudpd` fork's vendored `pyicloud_ipd`,
cross-checked against `picklepete/pyicloud`, `rclone`, and `steilerDev/icloud-photos-sync`.

This document exists so the Rust port can be written against a **proven reference
implementation** without an Apple account on hand. Every protocol claim below is
traceable to a primary-source file. Photos is **proven**; Notes is an **open gap**.

---

## 0. The one distinction to keep straight

There are **two unrelated Apple auth systems**. Porting the wrong one is the most
likely way to waste weeks:

| System | Endpoints | Used by | Needs anisette/GSA? | Reaches `ckdatabasews`? |
|---|---|---|---|---|
| **Web / idmsa SRP** ← *our target* | `idmsa.apple.com/appleauth/auth` → `setup.icloud.com` | iCloud.com, pyicloud, icloudpd, rclone | **No** | **Yes** |
| GrandSlam / GSA | `gsa.apple.com/grandslam/GsService2` | SideStore (app sideloading) | **Yes** | No — token scoped to dev/GrandSlam |

`SideStore/apple-private-apis` is the GSA system. **Do not port its auth.** Harvest
only its Rust crate choices (SRP wiring), documented in §5.

---

## 1. Authentication — SRP-6a web flow (PROVEN)

Port `pyicloud_ipd/base.py` (the SRP fork), **not** picklepete's legacy plaintext
`/signin` POST — Apple forced SRP and the plaintext path is dead.

**Endpoints**
- `AUTH_ROOT = https://idmsa.apple.com`
- `AUTH = {AUTH_ROOT}/appleauth/auth`
- `SETUP = https://setup.icloud.com/setup/ws/1`
- `HOME = https://www.icloud.com`
- China: `.com` → `.com.cn`.

**SRP config** (Python `srp` package): `srp.SHA256`, group `srp.NG_2048`, with
`srp.rfc5054_enable()` **and** `srp.no_username_in_x()`. Both flags matter — the
Rust `srp` crate must match this padding/username behavior or M1/M2 won't verify.

**Step 1 — `POST {AUTH}/signin/init`**
```json
{ "a": "<base64 SRP public A>", "accountName": "<appleid>",
  "protocols": ["s2k", "s2k_fo"] }
```
Response: `salt`, `b`, `c`, `iteration`, `protocol`.

**Password derivation** (`SrpPassword` class):
1. SHA-256 the plaintext password.
2. protocol `s2k_fo` → use the **hexdigest**; `s2k` → use the **raw digest**.
3. PBKDF2-HMAC-SHA256(that, salt, iteration) → SRP password entropy `x`.

**Step 2 — `POST {AUTH}/signin/complete?isRememberMeEnabled=true`**
```json
{ "accountName": "...", "c": "<from init>",
  "m1": "<base64 M1>", "m2": "<base64 M2>",
  "rememberMe": true, "trustTokens": ["<prior trust token, or empty>"] }
```

**Required OAuth/client headers** (`_get_auth_headers`; identical across both
libs — these are the public iCloud.com web-widget constants, reuse verbatim):
- `X-Apple-OAuth-Client-Id: d39ba9916b7251055b22c7f910e2ea796ee65e98b2ddecea8f5dde8d9d1a815d`
- `X-Apple-Widget-Key:` *(same value)*
- `X-Apple-OAuth-Client-Type: firstPartyAuth`
- `X-Apple-OAuth-Response-Type: code`
- `X-Apple-OAuth-State: <client UUID>`
- `X-Apple-OAuth-Require-Grant-Code: true`
- After a session exists: echo back `scnt` and `X-Apple-ID-Session-Id`.

**Session capture** (`PyiCloudSession.request`): scrape every response for
`X-Apple-ID-Account-Country`, `X-Apple-ID-Session-Id`, `X-Apple-Session-Token`,
`X-Apple-TwoSV-Trust-Token`, `scnt` → persist to a `{username}.session` JSON blob
+ a cookiejar.

**2FA / trusted device**
- `request_2fa_code()` triggers delivery.
- `validate_2fa_code(code)` → `POST {AUTH}/verify/trusteddevice/securitycode`,
  body `{"securityCode": {"code": "<code>"}}`. Error `-21669` = wrong code.
- `trust_session()` → `GET {AUTH}/2sv/trust` → returns the **trust token**
  (`X-Apple-TwoSV-Trust-Token`). Persist it; future logins skip 2FA. Validity ≈ 30 days.

**Token exchange → bootstrap** (`_authenticate_with_token`):
`POST {SETUP}/accountLogin`
```json
{ "accountCountryCode": "<from session>", "dsWebAuthToken": "<session token>",
  "extended_login": true, "trustToken": "<trust token or ''>" }
```
(`_validate_token` = `POST {SETUP}/validate` with literal body `"null"` to test a
live session.)

---

## 2. Session / webservice discovery (PROVEN)

The `accountLogin` response contains:
- `dsInfo.dsid` — the account **DSID**, appended as `?dsid=<dsid>` on every
  subsequent CloudKit call.
- `webservices` — a **map of per-service hosts**:
  `webservices["ckdatabasews"]["url"]` (typically `https://p*-ckdatabasews.icloud.com`),
  plus `drivews`, `findme`, etc. `_get_webservice_url("ckdatabasews")` reads it;
  raises `ServiceNotActivated` if absent.
- `hsaVersion`, `hsaChallengeRequired`, `hsaTrustedBrowser` — drive the 2FA branch.

**The Photos host is not hardcoded** — it comes from `webservices["ckdatabasews"]`,
and every CloudKit request carries `?dsid=<dsid>`.

---

## 3. Photos — full-res original retrieval (PROVEN)

Source: `pyicloud_ipd/services/photos.py` / `pyicloud/services/photos.py`
(`PhotosService`, `PhotoAsset`, `SMART_FOLDERS`).

**Endpoint (POST)**
```
{ckdatabasews}/database/1/com.apple.photos.cloud/production/{library}/records/query?dsid=<dsid>
```
- `{library}` = `private` for your own library (`shared`/`SharedSync-<UUID>` exist).
- Container `com.apple.photos.cloud`, env `production`.
- **Zone**: `zoneID = {"zoneName": "PrimarySync"}`.

**List-assets query** (rank-paged)
```json
{ "query": {
    "recordType": "CPLAssetAndMasterByAssetDateWithoutHiddenOrDeleted",
    "filterBy": [
      {"fieldName": "startRank", "comparator": "EQUALS", "fieldValue": {"type": "INT64", "value": <offset>}},
      {"fieldName": "direction", "comparator": "EQUALS", "fieldValue": {"type": "STRING", "value": "ASCENDING"}}
    ] },
  "resultsLimit": <page_size * 2>,
  "desiredKeys": [ /* ~85 fields */ ],
  "zoneID": {"zoneName": "PrimarySync"} }
```
- `recordType` varies by album/smart-folder (`CPLAssetAndMasterByAddedDate`,
  `CPLAssetAndMasterInSmartAlbumByAssetDate:<Videos|Favorites|…>`).
- **Pagination is rank-based**: bump `startRank` by the page size each round (no
  opaque continuation token in the photos path). Each logical asset returns **two**
  records (a `CPLMaster` + a `CPLAsset`), hence `resultsLimit = page_size * 2`.

**Record model**
- `CPLMaster` = immutable original + resource fields; `CPLAsset` = per-user current
  version/edits/favorite/date, linked to its master via `masterRef`.
- **Version lookup** (`PHOTO_VERSION_LOOKUP`/`VIDEO_VERSION_LOOKUP`) maps a size
  enum → field prefix: `ORIGINAL → resOriginal`, `ALTERNATIVE → resOriginalAlt`,
  `MEDIUM → resJPEGMed`, `THUMB → resJPEGThumb`, `ADJUSTED → resJPEGFull`; video:
  `ORIGINAL → resOriginal`, `MEDIUM → resVidMed`, `THUMB → resVidSmall`.
- **Full-res download URL**: resource lives in field `"<prefix>Res"` (e.g.
  `resOriginalRes`), a CloudKit asset dict →
  `size_entry["value"]["downloadURL"]` → GET with the same session cookies to
  stream the true original.
- **Filename**: field `filenameEnc`, type `ENCRYPTED_BYTES` but actually plain
  base64: `base64.b64decode(value).decode("utf-8")`.
- **Checksum**: `"<prefix>Fingerprint"` + `size_entry["value"]["fileChecksum"]` —
  use these to dedup/verify against blobs the backup already references, and to key
  the augmentation store.

**Complete proven path:** SRP login → `webservices["ckdatabasews"]` + `dsid` →
`records/query` on `PrimarySync` → `resOriginalRes.downloadURL` → bytes.

---

## 4. Notes — OPEN GAP (not proven, do not promise)

**No open-source private-protocol implementation of Notes retrieval exists.**
- `picklepete/pyicloud/services/` has **no `notes.py`** (only account, calendar,
  contacts, drive, findmyiphone, photos, reminders, ubiquity). icloudpd vendors
  only Photos; rclone only Drive + Photos.
- What is *believed* (unverified): Notes rides the same `ckdatabasews` /
  `records/query` machinery but in a **different container** (`com.apple.notes`)
  and zone (likely `Notes`), with record types around `Note` / `Folder` /
  `Media`, attachments stored as CloudKit assets (same `downloadURL` mechanism).
  The **transport is identical to Photos**; only container/zone/recordType/
  `desiredKeys` differ — and whether note bodies/attachments are CloudKit-field-
  **encrypted** is unknown.
- **Plan:** ship/validate Photos first. Treat Notes as a **reverse-engineering
  spike** — capture real `ckdatabasews` traffic from iCloud.com's Notes web app
  (browser network tab) to recover the container id, zone, record types, and
  `desiredKeys`. There is no `.py` to copy. **Do not scope Notes as certain on the
  strength of the Photos code.**

> This sharpens PRD §6.3 and ADR-0001: "Notes first (Photos free)" was backwards —
> **Photos is free and proven; Notes is the hard part.** See the correction note
> appended to ADR-0001.

---

## 5. Fragility & the Rust crate stack

**Advanced Data Protection = hard stop.** ADP disables iCloud.com web access
entirely; since this route *is* the iCloud.com web stack, ADP kills it and makes
Photos/Notes end-to-end encrypted. T1 sanctioned export is the only ADP-compatible
path.

**Moving target.** The auth handshake has already broken once (SRP migration; 503
rate-limiting at `/signin/init`). The **CloudKit `records/query` shapes are far
more stable** than the auth layer — expect to chase `idmsa` changes, not the query.

**Account-lockout triggers** (the real operational risk): rapid `/signin/*`
retries, wrong-2FA loops (`-21669`), missing/rotating trust token (forces fresh
2FA every run), non-browser-like download velocity. Mitigations: **persist & reuse
the 30-day trust token + cookiejar** so SRP login is rare; single session,
browser-like pacing.

**Rust building blocks** — you assemble the web SRP flow from crypto crates (no
high-level crate covers it):

| Crate | Covers |
|---|---|
| `srp` | SRP-6a client, `SrpClient<Sha256>`, 2048-bit `G_2048` (match `no_username_in_x`/RFC5054 padding) |
| `sha2`, `pbkdf2`, `hmac` | `SrpPassword` s2k/s2k_fo derivation |
| `reqwest` + `reqwest_cookie_store` | session, cookiejar, `scnt`/session-id round-trip |
| `serde` / `serde_json` | all bodies are JSON |
| `base64` | `filenameEnc`, SRP `a`/`m1`/`m2` |
| `uuid` | `X-Apple-OAuth-State` / client id |

**Explicitly NOT usable:** `SideStore/apple-private-apis` (wrong auth system — GSA/
anisette; harvest crate list only); `icloud-album-rs` (public shared albums only).
No crate implements the idmsa SRP handshake, `accountLogin` bootstrap,
`webservices`/`dsid` discovery, or the `com.apple.photos.cloud` query. **That layer
is the port.**

**Suggested order:** (1) SRP handshake + trust-token persistence + `accountLogin`;
(2) `ckdatabasews` + `dsid` plumbing; (3) Photos `PrimarySync` query +
`resOriginalRes` download (validate byte-for-byte vs real originals); (4) Notes
reverse-engineering spike. Keep it all behind the opt-in / ADP-aware seam from
ADR-0001.

---

## Primary sources
- `icloud-photos-downloader/icloud_photos_downloader` — `src/pyicloud_ipd/base.py`
  (SRP auth, endpoints, headers, trust token); `src/pyicloud_ipd/services/photos.py`
  (version lookup, download field, query shapes). **Maintained reference.**
- `picklepete/pyicloud` — `pyicloud/base.py` (2FA/trust, webservices/dsid);
  `pyicloud/services/photos.py`; `services/` listing (**no notes.py**).
- `SideStore/apple-private-apis` — `icloud-auth/src/client.rs`, `omnisette/`
  (GSA+anisette = *different* system; Rust crate reference only).
- `rclone` `iclouddrive.md`; `steilerDev/icloud-photos-sync` #363; icloudpd issues
  #1062/#1120/#1256 (auth-churn/503 evidence).
- Apple Support, "Advanced Data Protection for iCloud" (ADP disables web access).
