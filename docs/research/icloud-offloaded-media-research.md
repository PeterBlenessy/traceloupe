# Accessing iCloud-offloaded media — research & options

> **Superseded in part (as of v0.29.0).** This spike's claim that "there is no
> networking code… the app is otherwise fully offline" was true on 2026-07-20 but
> no longer holds: Security Check fetches indicator feeds, Safety Scan downloads a
> local model, and an opt-in URL de-shortener exists. Per ADR 0001 the privacy
> promise is scoped to *backup-derived* data. The iCloud-media analysis below
> still stands.

**Status:** research spike (branch `research/icloud-offloaded-media`, 2026-07-20)
**Question:** iOS backup data references media that the device never stored
locally — Notes embedded images, Messages attachments, iCloud Photos originals.
The metadata is in the backup; the blob was offloaded to iCloud. *Can a desktop
app fetch those blobs, and how?*

This document is the authoritative answer we researched to, plus the options and a
recommendation for TraceLoupe. Findings were produced by a multi-source web
research pass with adversarial verification (24/25 claims confirmed); citations
are at the end.

---

## TL;DR

- **Yes, it is technically possible** for a self-hosted desktop tool to fetch a
  user's *own* offloaded **Photos** and **Notes** media using only their Apple ID
  credentials — but **only via Apple's private, undocumented iCloud protocol**
  (the same one `iCloud.com` and the open-source `pyicloud`/`icloudpd` use), **not
  via any sanctioned Apple API.**
- **Apple's official CloudKit Web Services cannot do this.** They are
  architecturally scoped to *your own app's* containers, never Apple's first-party
  Notes/Photos/Messages system containers. This is a hard wall, not a
  configuration gap.
- **Messages attachments have no known self-hosted path.** The mature open-source
  ecosystem (`pyicloud`) has no Messages/iMessage service at all. Fetching
  offloaded iMessage attachments is a genuine open problem.
- The private-protocol route carries real costs: it **breaks under Advanced Data
  Protection (ADP)**, it **violates Apple's iCloud Terms of Service** (a
  contractual, automated-access prohibition), and it is a **moving target** that
  periodically breaks when Apple changes auth.
- **Recommendation for TraceLoupe:** keep the current honest "not in backup"
  posture as the default; treat cloud fetch as an **opt-in, clearly-labelled,
  user-credentials feature scoped to Photos + Notes first**, built behind a
  feature flag at the existing `resolve_*` seam. Do not promise Messages
  attachments. See [Recommendation](#recommendation).

---

## 1. Why this problem exists (grounded in our code)

TraceLoupe's entire notion of "offloaded" is currently **implicit**: a metadata
row exists in the app database, but the blob does not resolve in `Manifest.db`.

- **Notes** (`crates/traceloupe-core/src/parsers/notes.rs`): we count `image_count`
  (images the note *references*, from `ZICCLOUDSYNCINGOBJECT`) separately from
  `available_image_count` (rows in `note_media`, written only when
  `resolve_note_image()` finds a real blob in the Notes-domain Manifest,
  `notes.rs:503`). On one real backup: **2,698 referenced images, 0 present.**
  `src/views/notes.tsx` surfaces this honestly ("N image(s) not included in this
  backup … Notes media is often stored in iCloud and not downloaded").
- **Messages** (`messages.rs:727`): `resolve_attachment()` returns `None` when the
  file isn't in `MediaDomain`; the row is still written with `local_path = NULL`
  and the UI shows "· not in backup". There is an opt-in *filename* recovery from
  the camera roll (`query.rs:301`), but that only helps when the same file also
  survives in Photos.
- **Photos / Voice Memos** simply omit assets whose blob isn't present.

So today the app **correctly detects and honestly reports** offloading, but has
**no mechanism to retrieve** the missing blob. There is no networking/iCloud code
in the repo at all — the app is otherwise fully offline. The clean integration
seam for any fetch capability is exactly those `resolve_*` functions that
currently return `None`.

> Aside/gap: we infer "offloaded" purely from blob absence. iOS actually records
> download state (e.g. `attachment.transfer_state` in `sms.db`, and download-state
> columns in the Notes `ZICCLOUDSYNCINGOBJECT` schema). Reading those would let us
> distinguish *offloaded-to-iCloud* (fetchable) from *deleted/never-present* (not
> fetchable) **before** attempting any network call — worth doing regardless of
> which option below we pick.

---

## 2. The options

| # | Approach | Reaches Notes/Photos/Messages? | Self-hostable, user's own creds? | Verdict |
|---|----------|-------------------------------|----------------------------------|---------|
| A | **CloudKit Web Services / CloudKit JS** (sanctioned) | ❌ No — only *your* app containers | ✅ but useless here | **Dead end** |
| B | **Authenticated private iCloud protocol** (`pyicloud`/`icloudpd` model) | ◑ Photos ✅, Notes ✅, Messages ❌ | ✅ Yes | **Viable, with caveats** |
| C | **Commercial forensic API** (Reincubate ricloud; Elcomsoft-class) | ✅ Broad | ❌ Enterprise partnership, $10K+/mo | **Not self-hostable** |
| D | **Read iOS download-state flags only** (no fetch) | n/a | ✅ Fully local | **Complementary, do anyway** |
| E | **User re-downloads in Apple's apps, re-backs-up** (no code) | ✅ | ✅ | **Zero-eng fallback** |

### Option A — Sanctioned CloudKit Web Services: architecturally impossible

Apple's CloudKit Web Services / CloudKit JS give web/other-platform access to
CloudKit data, **but only to "Data stored in your app's CloudKit databases" and
"Discoverable users and contacts"**, gated by a per-container API token you
generate in the CloudKit Console. Apple's Notes and Photos *do* run on CloudKit —
but in **Apple's own private system containers**, for which Apple never issues a
third-party token. "The application is the only gateway to the database. The same
applies to access via CloudKit Web Services." This is precisely why every tool
that actually reaches this data bypasses CloudKit Web Services entirely. **No
amount of engineering on the sanctioned path reaches the target data.**

### Option B — Authenticated private iCloud protocol: the viable route

This is what `iCloud.com` does under the hood, and what the open-source
`pyicloud` library and its actively-maintained `icloudpd` fork (MIT, v1.32.x,
2025) reimplement:

1. **Authenticate** with Apple ID email + password
   (`PyiCloudService('user@example.com', 'password')`; password can come from a
   system keyring).
2. **Complete 2FA** interactively — a code delivered to a trusted device
   (`requires_2fa` → `request_2fa_code()` → `validate_2fa_code()`), then
   `trust_session()` persists a trusted-session token so you don't re-prompt.
   Apple expires the trusted session on its own interval (**~2 months**). (Note:
   modern 2FA/HSA2 and legacy two-step 2SA need materially different call
   sequences.)
3. **Query CloudKit-backed web endpoints** — the *same hosts iCloud.com uses*,
   e.g. `ckdatabasews.icloud.com/database/1/com.apple.photos.cloud/production`
   (shard-prefixed in practice, `p150-ckdatabasews…`), zone-based: the
   `PrimarySync` zone for the personal Photos library, server-discovered
   `SharedSync-{UUID}` zones for shared/family libraries.
4. **Download the full-resolution original** — `--size original` returns the
   complete offloaded blob, **not** a thumbnail/placeholder.

Crucially, this uses your *own* iCloud auth against Apple's *own* private
container — it is **not** a third-party developer CloudKit token, which is why
it works where Option A can't.

**Coverage within Option B:**
- **Photos** — well-supported and proven (`icloudpd` exists precisely for this).
- **Notes** — `pyicloud` lists a Notes service ("retrieving full notes with
  attachments, rendering to HTML, and exporting"). *Caveat:* that its attachment
  extraction specifically pulls **cloud-only** (offloaded) blobs — vs
  locally-cached ones — is documented but not independently benchmarked in our
  sources. **Verify before relying on it.**
- **Messages** — **not covered.** `pyicloud` has no iMessage service. No verified
  source shows any self-hosted path to offloaded Messages attachments. This is
  the biggest gap versus our data model.

### Option C — Commercial forensic API

Reincubate's `ricloud` is an enterprise **API-as-a-service** (their page lists
"Enterprise: POA, tiered volume pricing from $10K p/m"; a startup tier still
floors around ~$500/mo and is cloud-hosted). The public `reincubate/ricloud`
repo is only a *client* for their hosted service, not self-hostable. Elcomsoft
Phone Breaker is comparable commercial tooling. This buys broad, higher-assurance
coverage (including token-based auth flows) **at the cost of a paid partnership**
— fundamentally not a fit for a self-hosted desktop tool, but worth naming as the
"if we ever need turnkey breadth" escape hatch.

### Option D — Read iOS download-state flags (local, no fetch)

Independent of fetching, we can read the native "is this offloaded?" signals
already in the backup (`transfer_state`, Notes download-state columns, Photos
`Optimize Storage` thumbnail-vs-original indicators in `Photos.sqlite`). This
sharpens our honest reporting ("offloaded to iCloud — fetchable" vs "deleted")
and is a prerequisite for a good fetch UX. **Low-risk, do it regardless.**

### Option E — No-code fallback

Tell the user: in Apple's own Notes/Photos apps, disable "Optimize Storage" /
download the originals, wait for sync, then re-create the backup. Everything then
lands in the local backup and our existing parsers pick it up. Zero engineering,
zero ToS exposure — a legitimate answer for many users.

---

## 3. Constraints & risks (apply to Option B)

1. **Advanced Data Protection breaks it.** `icloudpd`'s own docs: ADP "is not
   supported because icloudpd simulates web access, which is disabled with ADP."
   Enabling ADP turns off iCloud.com data access — the exact mechanism this route
   depends on. (Nuance: a user can *temporarily* re-enable web access for ~1-hour
   windows from a trusted device.)
2. **Terms of Service.** Apple's iCloud ToS (§V.B.10) prohibits "accessing the
   Service through any automated means, like scripts or web crawlers." This is a
   **contractual** restriction (cf. *hiQ v. LinkedIn*), not automatically a
   statutory/CFAA violation, and it applies **even to your own account**.
   Sanctioned CloudKit Web Services is exempt; the private-protocol route is not.
   This is a product/legal decision, not just an engineering one — get a lawyer's
   read before shipping, and make it opt-in with clear disclosure.
3. **It's a moving target.** The protocol is private and undocumented — endpoint
   hosts, the SRP auth handshake, `anisette`/ADSP header requirements, and session
   lifetime all change without notice and periodically break `pyicloud`/`icloudpd`
   (recurring SRP/503 breakages). A Rust implementation would be reverse-
   engineering a shifting target, not consuming a stable API — **ongoing
   maintenance cost, not a one-time build.**
4. **Credential sensitivity.** We'd be handling the user's live Apple ID password
   and trusted-session token. Must use the OS keychain, never persist plaintext,
   and be explicit about what's stored and where.

---

## 4. What's actually in the local backup (context)

Pure-local parsers (e.g. `apple_cloud_notes_parser`) extract embedded
drawings/pictures **only "if the type of backup used has the original files
referenced in attached media."** When media is offloaded, the blob is simply
absent locally and *must* be fetched over the network — these parsers have no
cloud capability ("Cloud" in the name refers to Notes *syncing*, not fetching).
This matches exactly what we observe: metadata present, blob missing. So there is
no local trick that recovers a truly-offloaded blob; fetching is the only path
(Options B/C/E).

---

## 5. Recommendation

**Ship honesty by default; make fetch an opt-in, Photos+Notes-first, feature-
flagged capability at the `resolve_*` seam.**

Concretely, in priority order:

1. **Now (local, no risk) — Option D.** Read the native download-state flags so we
   can label offloaded-but-fetchable vs deleted. Improves the current honest UI
   immediately and is a prerequisite for good fetch UX. Touches `notes.rs` /
   `messages.rs` schema-read only.
2. **Prototype (opt-in) — Option B for Photos.** Photos is the proven,
   best-supported case and the highest-value one (95k assets in our reference
   backup). Build a small authenticated-iCloud client (or shell to a vendored
   `icloudpd`) behind a feature flag, driven from the camera-roll/attachment
   absence branch. Gate on: user opts in, no ADP, clear ToS disclosure,
   keychain-stored creds/session.
3. **Then — Option B for Notes**, *after* verifying it retrieves cloud-only note
   attachments (the one unverified claim). This directly closes the "2,698
   referenced, 0 present" gap that motivated this spike.
4. **Do not promise Messages attachments.** No self-hosted path exists today.
   Keep the current "not in backup" + camera-roll filename recovery. Track as an
   open research question.
5. **Keep Option E documented** as the zero-risk user workaround, and **Option C**
   noted as the commercial escape hatch if turnkey breadth is ever required.

Design note: everything hangs off the existing seam where presence is decided —
`resolve_note_image()` (`notes.rs:503`), `resolve_attachment()` (`messages.rs:727`),
and the camera-roll Manifest iteration. The `available_image_count < image_count`
gap already surfaced in the UI is the natural place to attach a "Fetch from
iCloud" action. No architectural change is required to *host* the feature — the
cost is entirely in the (maintenance-heavy) iCloud client and the
legal/UX framing.

---

## 6. Open questions (carried forward)

1. Is there **any** self-hosted, own-credentials path to offloaded **Messages/
   iMessage attachments**? `pyicloud` has none. What endpoint/zone would it use?
2. What exact low-level auth artifacts must a from-scratch (non-Python) Rust
   client reproduce — `anisette`/ADSP headers, SRP handshake specifics, the
   Mbksync/trusted-session token flow — and is any of it documented authoritatively
   vs only reverse-engineered?
3. Does `pyicloud`'s Notes service actually pull **cloud-only** (offloaded) image
   blobs, or only locally-cached ones? (Blocks Option B step 3.)
4. How do Elcomsoft Phone Breaker / `mvt` differ from the `pyicloud`/`icloudpd`
   approach — any genuinely self-hosted (non-partnership) path, and what do their
   whitepapers claim about token-based vs full-credential auth?

---

## 7. References

Primary / authoritative:
- Apple, *CloudKit Web Services Reference* — access scope & "application is the
  only gateway": https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/CloudKitWebServicesReference/index.html
- Apple, *Obtaining an API token for an iCloud container*: https://developer.apple.com/documentation/cloudkit/obtaining-an-api-token-for-an-icloud-container
- Apple, *CloudKit framework* (moves data between *your* app and *your* containers): https://developer.apple.com/documentation/cloudkit
- Apple, *iCloud Terms & Conditions* (§V.B.10 automated-access prohibition): https://www.apple.com/legal/internet-services/icloud/us-en/terms.html
- Apple Support, *Advanced Data Protection* (turns off iCloud.com data access): https://support.apple.com/en-us/102630
- `pyicloud` (PyPI) — auth flow, 2FA, services list incl. Notes: https://pypi.org/project/pyicloud/
- `pyicloud` fork (icloud-photos-downloader): https://github.com/icloud-photos-downloader/pyicloud
- `icloudpd` authentication docs — ADP unsupported, 2FA ~2-month expiry: https://icloud-photos-downloader.github.io/icloud_photos_downloader/authentication.html
- `icloudpd` PR #733 — 2FA vs 2SA call sequences: https://github.com/icloud-photos-downloader/icloud_photos_downloader/pull/733
- `apple_cloud_notes_parser` — local-only, extracts media "if referenced": https://github.com/threeplanetssoftware/apple_cloud_notes_parser

Secondary / vendor (lower confidence, useful context):
- DeepWiki, *icloudpd API integration* — `PrimarySync`/`SharedSync` zones, endpoints: https://deepwiki.com/icloud-photos-downloader/icloud_photos_downloader/8.3-api-integration
- Reincubate *ricloud* — commercial enterprise API pricing/model: https://reincubate.com/ricloud-api/
- Elcomsoft blog — iCloud auth tokens, cloud forensics data channels: https://blog.elcomsoft.com/2017/11/icloud-authentication-tokens-inside-out/ · https://blog.elcomsoft.com/2022/11/cloud-forensics-obtaining-icloud-backups-media-files-and-synchronized-data/
- The Forensic Scooter — full-size asset vs thumbnail (Optimize Storage) in `Photos.sqlite`: https://theforensicscooter.com/2022/12/05/do-you-have-a-full-sized-assetor-just-a-thumbnail-did-optimized-iphone-storage-process-occur/
