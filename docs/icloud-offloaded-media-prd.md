# iCloud-Offloaded Media

**Product Requirements Document**

*Status: Draft for review · Target: post-0.2.0 feature · Platform: macOS (TraceLoupe, Tauri v2)*

Companion docs: research in [`icloud-offloaded-media-research.md`](icloud-offloaded-media-research.md);
domain language in [`../CONTEXT.md`](../CONTEXT.md); the architecture decision in
[`adr/0001-icloud-offloaded-media-two-tier.md`](adr/0001-icloud-offloaded-media-two-tier.md).

---

## 1. Executive summary

iOS backups only *reference* much of their media. A note's embedded images, a
Messages attachment, a camera-roll original — the metadata row is in the backup,
but the blob was **offloaded to iCloud** ("Optimize Storage") and never written to
the device. On one real backup, Notes referenced **2,698** images and **0** were
present. TraceLoupe already reports this honestly (the `image_count` vs
`available_image_count` gap in `notes.rs`/`notes.tsx`; the "· not in backup" label
in `messages.tsx`), but it cannot **recover** the missing blob.

This feature adds recovery, in a deliberately layered way:

- **Phase 1 (local, zero-risk):** read iOS's own download-state flags so we can
  say *offloaded (in iCloud)* vs *deleted* instead of inferring purely from blob
  absence.
- **Tier 1 — Sanctioned Export (default):** import the archive a user requests
  from Apple's official Data & Privacy portal. ToS-compliant, no credentials.
- **Tier 2 — Live Fetch (opt-in):** retrieve offloaded blobs on demand via Apple's
  private authenticated iCloud protocol using the account owner's own credentials
  (the pyicloud/icloudpd model), implemented natively in Rust in a **separate
  repository**. Ships **Notes first** (Photos falls out for free).

The core research finding shapes everything: **Apple's sanctioned CloudKit Web
Services cannot reach the first-party Notes/Photos/Messages containers** — the only
programmatic routes are the sanctioned *export* (T1) or the *private* protocol
(T2). And the real risk of T2 is **automated account lockout** (recoverable), not
legal action — which is why T1 is the default and T2 is consent-gated.

## 2. Problem & opportunity

- Offloading is the norm, not the exception. Any user with "Optimize iPhone
  Storage" on (the default once storage fills) has a backup whose Notes images,
  Photos originals, and older Messages attachments are cloud-only. Today TraceLoupe
  honestly shows a hole where that media should be.
- TraceLoupe's pitch is *complete, local, auditable access to your own backup.* An
  honest hole is good; a way to **fill** the hole — with clear provenance and
  without pretending — is materially better, and no other local tool does this
  openly.
- The two audiences differ in what they can do: someone analyzing **their own**
  device can authenticate and Live Fetch; someone analyzing a **consenting other's**
  backup can ask that person to run a Data & Privacy export. The two tiers map
  cleanly onto the two audiences.
- Doing this *carelessly* is easy and harmful (store the password, hammer the API,
  get the account locked, blur what's real). The opportunity is to do it the way
  the rest of TraceLoupe is done: honest, provenance-tracked, read-only over the
  original backup, credentials handled like the crown jewels.

## 3. Reference research — how the ecosystem does it

Full detail and citations in [`icloud-offloaded-media-research.md`](icloud-offloaded-media-research.md);
verified via a multi-source, adversarially-checked research pass.

**Sanctioned CloudKit Web Services is a dead end.** It is scoped to a developer's
*own* app containers plus discoverable contacts, gated by a per-container API
token. Apple never issues tokens for its first-party Notes/Photos/Messages system
containers. Rust/Go ports of CloudKit Web Services (`meszmate/apple-rs`,
`lukasmalkmus/icloud-go`) inherit this limit.

**Tier 1 — Apple Data & Privacy export** (privacy.apple.com → "Request a copy of
your data"). Official and ToS-clean. Includes iCloud **Notes and Photos** in
**original formats** ("Download and Keep Originals"). Apple fulfils in **~7 days**,
gives a **14-day** download window, and splits the archive into multiple files.
It is asynchronous and whole-account, not per-item.

**Tier 2 — the private authenticated protocol** (what iCloud.com uses under the
hood; reimplemented by `pyicloud` and its maintained `icloudpd` fork):

1. Authenticate with Apple ID + password.
2. Complete interactive **2FA/HSA2** (code to a trusted device), then persist a
   **trusted-session token** (Apple expires it ~2 months).
3. Query the CloudKit-backed web endpoints, e.g.
   `ckdatabasews.icloud.com/database/1/com.apple.photos.cloud/production`
   (shard-prefixed in practice), zone-based (`PrimarySync` for the personal Photos
   library). **Correction:** neither `pyicloud` nor `icloudpd` exposes a **Notes**
   service — Photos is the only proven media path; Notes must be reverse-engineered
   (see §6.3 and the porting spec).
4. Download the **full-resolution original**, not a thumbnail.

**Available Rust building blocks.** The auth target is the **idmsa web SRP-6a
flow** (`idmsa.apple.com` → `setup.icloud.com/.../accountLogin` + trust token) —
**no anisette required** — assembled from crypto crates (`srp`, `sha2`, `pbkdf2`,
`hmac`, `reqwest` + `reqwest_cookie_store`, `serde_json`, `base64`, `uuid`).
`SideStore/apple-private-apis` (+ `omnisette`) implements a **different** Apple-ID
system (GrandSlam/GSA + anisette) whose token does **not** reach `ckdatabasews`;
we reuse only its crate choices, not its auth. `opendal`'s iCloud **Drive** service
is a secondary reference. No crate does end-to-end Photos/Notes — we port
`icloudpd`'s `pyicloud_ipd` data layer. Full cited spec:
`docs/icloud-live-fetch-porting-spec.md`.

**Messages is the hardest case and is out of scope for T2 v1.** `pyicloud` has no
Messages service; Messages in iCloud is **end-to-end encrypted** (its CloudKit
Service Key lives in iCloud Keychain / iCloud-Backup escrow); and it is **not
exposed on iCloud.com**. Only commercial tools (Elcomsoft) do it. See §6.6 for the
research spike.

**Enforcement reality.** Apple has no documented record of *punitive* action
against individuals for own-account use. What it does, commonly, is an **automated
security lock** ("usually leads to accounts being locked" per Reincubate), triggered
by burst/unusual access — recoverable via normal account recovery. This is the
concrete risk T2 must be designed around.

## 4. Goals & non-goals

**Goals**

- Turn today's honest "not in backup" holes into recoverable media, with explicit
  provenance, without ever mutating the original backup.
- **Phase 1:** distinguish *offloaded* from *deleted* using iOS's own
  download-state flags — a purely local, no-network improvement.
- **Tier 1:** import an Apple Data & Privacy archive and reconcile its Notes/Photos
  media against already-parsed backup rows. Fully ToS-compliant, no credentials.
- **Tier 2:** opt-in, consent-gated Live Fetch of offloaded **Notes** media (Photos
  for free), native Rust, in a **separate repo/crate** consumed as an optional
  feature-flagged dependency.
- Handle credentials like the existing backup password: OS Keychain + Touch ID
  (`secret.rs`/`biometric.rs`), never plaintext; persist only the trusted-session
  token.
- Design the fetch path to *minimize* request bursts (the lockout trigger):
  per-item by default, bulk only on explicit opt-in with a warning.
- Honest UX throughout: what each tier can/can't do, the account-lockout risk of
  T2, ADP incompatibility, and clear provenance badges ("recovered via Export" /
  "via Live Fetch").

**Non-goals**

- Not Messages recovery in v1 (separate research spike — §6.6).
- Not Photos-gallery-first — Notes motivates the work; Photos is a T2 by-product,
  not a headline.
- Not browser automation of iCloud.com (does not escape the ToS, and the Notes web
  app has no per-image export — see research doc).
- Not a background/continuous sync daemon. Fetch is user-initiated.
- No credential or fetch telemetry of any kind; there is no backend.
- Not supported under **Advanced Data Protection** for T2 (T1 still works) — detect
  and message, don't fail opaquely.

## 5. Users & safety framing

Primary: (a) an individual analyzing **their own** device who can authenticate;
(b) someone analyzing a **consenting other's** backup, who can ask that person to
run an export (T1) or authenticate at fetch time (T2). T2 is designed for *whoever
can pass Apple ID + 2FA at fetch time* — no assumption is baked in.

Safety/honesty requirements (product requirements, not copy polish):

- **Consent gate before T2 is ever enabled**, stating plainly: it uses the
  account owner's own credentials against Apple's *private, unsanctioned* API; it
  may violate Apple's iCloud ToS; and it can trigger an **automated account lock**
  (recoverable, but disruptive). Enable is an explicit, per-account action.
- **Provenance is never hidden.** A recovered blob is visibly marked as recovered
  and by which tier; it is never presented as backup-native.
- **ADP detection** short-circuits T2 with a clear "use Tier 1 export instead"
  message.
- **Credentials never leave the machine** except to Apple's own auth endpoints over
  HTTPS.

## 6. Feature description

### 6.1 Phase 1 — download-state flags (local, no network)

Today "offloaded" is inferred purely from blob absence (`resolve_note_image`
returns `None`, `resolve_attachment` returns `None`). iOS actually records download
state; reading it lets us label *offloaded (fetchable)* vs *deleted/never-present*
**before** any network call.

> **Design correction (2026-07-21) — `transfer_state` is NOT an offload flag;
> M0's premise was wrong, and its research question is now resolved
> authoritatively.** See the citations in
> [`docs/icloud-offload-flags-findings.md`](./icloud-offload-flags-findings.md).
> The short version:
> - `attachment.transfer_state` is Apple's `IMFileTransferState` enum (`5 =
>   finished`, `6/7 = error/recoverableError`, `0 = waitingForAccept`, …). It is a
>   **completion latch** recording how a transfer *once* ended; iOS does **not**
>   rewrite it when the blob is later offloaded to Messages-in-iCloud or thinned.
>   So `transfer_state = 5` + missing blob is *consistent with* offloading but is
>   not proof of it — it carries the same information our blob-absence check
>   already has. iLEAPP itself never reads the column.
> - **Deletion removes the whole `attachment` row** (recoverable only via
>   ROWID-gap / WAL / Biome), so "deleted" normally presents as *no row*, while
>   "offloaded" presents as *a full metadata row with no blob*. That asymmetry —
>   **row-present + blob-absent = offloaded/thinned; row-absent = deleted** — is
>   the correct, authoritative model, and it is essentially today's heuristic.

**Revised local labelling (what M0 actually ships).** Do *not* present
`transfer_state` as an "offloaded" oracle. Instead:

| Source | Column | What we can honestly say (locally, no network) |
|---|---|---|
| Messages (`sms.db`) | `transfer_state ∈ {6,7,8}` | Transfer **failed / was never completed** — surface as "failed to download," *not* "offloaded." |
| Messages (`sms.db`) | `transfer_state = 5` + blob absent | Was successfully local once → now offloaded/thinned → **recoverable candidate**. |
| Messages (`sms.db`) | `ck_record_id` populated / `ck_sync_state` ≥ 1 | Attachment is (or was) synced to CloudKit → stronger "recoverable from iCloud" hint. **Semantics undocumented — label as heuristic until validated against a non-offloaded backup.** |
| Notes (`ZICCLOUDSYNCINGOBJECT`) | `ZFILESIZE` > 0 + `ZSERVERRECORDDATA` present, no blob on disk | Media object exists & synced to iCloud but not in backup → **recoverable candidate** (no dedicated "needs-fetch" boolean exists). |

Surfaced on the existing honest UI (`notes.tsx`, `messages.tsx`): the "N image(s)
not included in this backup" line splits into "…offloaded to iCloud (recoverable)"
vs "…failed to download / not available." Schema: add a nullable
`download_state` to the relevant cache rows (bump `SCHEMA_VERSION` in
`crates/traceloupe-core/src/cache.rs`; needs a re-import to populate).

> **Ground-truth status (empirical, `~/.traceloupe-dev/backup-mirror`).** This
> backup has Messages-in-iCloud on and is **fully offloaded** — the entire
> `MediaDomain/.../SMS/Attachments` tree holds 7 `.pvt/metadata.plist` files and
> **zero media blobs**; 0 of 8 465 attachment rows have a blob on disk, across
> *every* `transfer_state` and `ck_sync_state` value. This confirms the "latch"
> finding (no column predicts presence when nothing is present) but means the one
> *positive* signal we'd still like to confirm — that `ck_record_id`-populated
> rows are the recoverable ones — **cannot be validated here**. That validation
> (correlating `ck_record_id`/`ck_sync_state` against present blobs on a backup
> taken with "Optimize Storage" **off**) is now a *nice-to-have refinement*, no
> longer a blocker: the row-present/row-absent model above ships without it.

### 6.2 Tier 1 — Sanctioned Export importer (default)

- **Input:** the folder/zip set a user downloads from privacy.apple.com. A guided
  screen explains how to request it (and that Apple takes ~7 days).
- **Parse:** a new `parsers/data_export.rs` reads the export's Notes and Photos
  payloads (original-format media + the per-note structure Apple emits) into an
  intermediate form.
- **Reconcile:** match exported media to already-parsed backup rows so a recovered
  image lands on the *right* note/asset. Matching keys: note identifier / title +
  timestamps for Notes; asset UUID / filename + capture time for Photos (reusing
  the filename+time approach already in `recover_attachment_media`, `query.rs`).
  Unmatched export items are still importable as standalone recovered media.
- **Store:** blobs go into the **augmentation store** (§6.4) with
  `tier = 'export'`. The backup mirror is never touched.
- **No credentials, no network** (beyond the user's own browser download).

### 6.3 Tier 2 — Live Fetch (opt-in, separate Rust crate)

Lives in a **new repository** (working name `icloud-fetch`), a standalone Rust
crate that TraceLoupe depends on behind a `live-fetch` Cargo feature. Rationale
(see ADR-0001): isolates fragile, ToS-adjacent, network-bound, frequently-breaking
code from the offline core; independent release cadence to chase Apple's changes;
clean legal/licensing boundary; independently testable.

The full, cited, port-ready protocol is in
[`docs/icloud-live-fetch-porting-spec.md`](./icloud-live-fetch-porting-spec.md)
(SRP endpoints, headers, request bodies, download fields, Rust crate stack). Two
corrections it forces on the earlier plan:

- **Photos is the proven, free part; Notes is the hard part** — the reverse of
  "Notes first." A maintained reference (`icloudpd`'s `pyicloud_ipd`) implements
  the whole Photos path; **no open-source project implements Notes over the
  private protocol**, so Notes is a traffic-capture reverse-engineering spike, not
  a transcription. Ship/validate **Photos first**.
- **Auth building block was mis-scoped.** `SideStore/apple-private-apis` is a
  *different* Apple auth system (GrandSlam/GSA + anisette; its token doesn't reach
  `ckdatabasews`). The route we need is the **idmsa web SRP-6a flow** used by
  iCloud.com/pyicloud/icloudpd — **no anisette required**. We assemble it from
  Rust crypto crates (`srp`, `sha2`, `pbkdf2`, `hmac`, `reqwest` +
  `reqwest_cookie_store`, `serde_json`, `base64`, `uuid`); we harvest only the
  *crate choices* from apple-private-apis, not its GSA logic.

**Crate shape (initial):**

- `auth`: idmsa SRP-6a login → 2FA → trust-token persistence →
  `setup.icloud.com/accountLogin` bootstrap → `webservices`/`dsid` discovery.
  Returns a `Session` (cookiejar + trust token + discovered service endpoints).
- `photos` **(v1 headline — proven)**: `PrimarySync`-zone `records/query` + full-res
  `resOriginalRes.downloadURL` download (direct port of `pyicloud_ipd/photos.py`);
  validate byte-for-byte against real originals.
- `notes` **(spike — unproven)**: reverse-engineer the `com.apple.notes` container /
  zone / record types by capturing iCloud.com Notes web traffic; retrieve embedded
  attachment assets. No reference impl to copy; scope conservatively.
- Deliberate rate-limiting / backoff and a single-session reuse policy (reuse the
  30-day trust token) to reduce the account-lockout surface.

**TraceLoupe integration.** The `resolve_*` seam that currently returns `None` on
absence (`resolve_note_image` `notes.rs:503`, `resolve_attachment` `messages.rs:727`)
is where a "recoverable" record carries the iCloud identifier a later fetch needs.
Credentials/session persist via the existing Keychain infra (`secret.rs`,
`biometric.rs`); only the trusted-session token is stored, never the password.
**ADP is detected and blocks T2 with a message.**

**Trigger (lockout-aware).** Per-item **"Fetch from iCloud"** on the
referenced-vs-present gap (e.g. a note's missing images) is the default. An opt-in
**"fetch all offloaded"** bulk run exists for power users, gated behind a
rate-limit/lockout warning. Keeping typical volume low is a deliberate mitigation.

### 6.4 Augmentation store & persistence

Recovered blobs from **both** tiers live in a sidecar store keyed to the backup —
never in the read-only backup mirror. New cache tables (bump `SCHEMA_VERSION`):

- `recovered_media(id, backup_id, tier /* 'export' | 'live-fetch' */, kind /* NoteImage | Attachment | PhotoAsset */, owner_ref /* note_id / attachment_id / asset id */, blob_path, sha256, byte_len, source_detail, fetched_at)`
- `fetch_runs(id, started_at, finished_at, scope, item_count, status, error)` — T2 run log for progress/history and diagnosing lockouts.

Serving reuses the existing async URI-scheme pattern (`traceloupe-note-image` et
al. in `src-tauri/src/lib.rs`): a recovered blob resolves from `recovered_media`
when the Manifest has no entry. Query paths add a `recovered_*` count alongside the
existing `available_image_count` so the UI can distinguish backup-native from
recovered.

### 6.5 Tauri commands & IPC

Added to `src-tauri/src/lib.rs` and `src/lib/ipc.ts`, following existing patterns:

- Tier 1: `import_data_export(path)`, `get_export_import_status`.
- Tier 2: `icloud_auth_begin(apple_id)`, `icloud_submit_2fa(code)`,
  `icloud_trust_session()`, `icloud_auth_status()`, `fetch_offloaded(owner_ref)`,
  `fetch_all_offloaded(scope)`, `cancel_fetch()`.
- Shared: `list_recovered_media(owner_ref)`, `get_download_state(...)`.
- Event `fetch://progress` (mirrors `ImportPhase`, honors the existing
  `CancelToken`).

### 6.6 UI

- Recovery is surfaced **in place** on the existing honest states, not as a
  separate view: the Notes "N images not included" panel (`notes.tsx`) and the
  Messages "· not in backup" label (`messages.tsx`) gain a **"Recover"** affordance
  when Phase-1 flags say the item is *offloaded* (not *deleted*).
- A **Settings → iCloud recovery** pane: choose/inform Tier 1 (how to request an
  export) and Tier 2 (the consent gate, Apple-ID sign-in, session status, ADP
  warning, bulk-fetch toggle). shadcn components (check the registry first, per
  house rules).
- **Provenance badges** on recovered media ("Recovered · Export" / "Recovered ·
  Live Fetch") so it is never confused with backup-native data.
- Bulk-fetch progress reuses the import-provider pattern so navigation doesn't kill
  the job.

## 7. Milestones

**M0 — Offloaded-vs-deleted labelling (Phase 1, local).** Research question now
**resolved** (§6.1, `docs/icloud-offload-flags-findings.md`): the model is
structural (row-present+blob-absent ⇒ offloaded; row-absent ⇒ deleted), refined by
`transfer_state ∈ {6,7,8}` ⇒ "failed download" and `ck_record_id`/`ZSERVERRECORDDATA`
⇒ "synced-to-iCloud" hint. Add nullable `download_state` to the relevant cache rows,
compute it in the parsers, relabel the honest UI as *offloaded (recoverable)* vs
*failed / not available*. No network. *DoD: implemented → verified against a real
backup → screenshots → review → pushed.* (Positive-ground-truth validation of the
`ck_record_id` hint awaits a non-offloaded backup and is a follow-up refinement, not
a gate.)

**M1 — Tier 1 Sanctioned Export importer.** `parsers/data_export.rs`; Notes/Photos
reconcile against backup rows; `recovered_media` table + augmentation store;
`import_data_export` command; in-place "Recover from export" UX + provenance
badges. Fully offline/ToS-clean — the safe default ships first.

**M2 — Tier 2 Live Fetch, Photos-first.** Stand up the separate `icloud-fetch`
crate per `docs/icloud-live-fetch-porting-spec.md`: idmsa SRP-6a auth (+ trust-token
persistence) → `ckdatabasews`/`dsid` discovery → **Photos** `PrimarySync` query +
`resOriginalRes` download (proven port). TraceLoupe `live-fetch` feature; Keychain
session; consent gate; ADP detection; per-item fetch + opt-in bulk; `fetch_runs`
log. Ship as **experimental**. **Notes is a follow-on spike** (M2.5) — no reference
impl exists; reverse-engineer `com.apple.notes` container/zone from captured
iCloud.com traffic before scoping.

**M3 — Messages research spike + polish.** Feasibility spike on Messages in iCloud:
can TraceLoupe's existing encrypted-backup decryptor (crypto ladder + Keychain
keys) recover the Messages **CloudKit Service Key** from the iCloud-Backup escrow,
and is the chat-attachment CloudKit zone reachable? Commit to implementation only
if the spike proves feasible. Plus: session-expiry handling, bulk backoff tuning,
export-freshness reminders.

**Validation strategy** (per the research-authoritatively rule). Phase 1 flags:
diff our *offloaded vs deleted* labels against `mvt-ios`/iLEAPP interpretation of
the same columns on a real backup. Tier 1: run a real Data & Privacy export of a
test account and confirm reconciliation lands media on the right notes/assets.
Tier 2: test against a **disposable test Apple ID** only (never a primary account),
measuring how quickly bulk fetch trips a lockout to calibrate backoff; compare
fetched **Photos** byte-for-byte against `icloudpd` output for the same account
(Notes has no reference tool — validate it by round-tripping known offloaded note
images end-to-end).

## 8. Risks & open questions

- **M0 flag semantics — RESOLVED (2026-07-21).** The research question is
  answered authoritatively (§6.1, `docs/icloud-offload-flags-findings.md`):
  `transfer_state` is a non-updated completion latch, *not* an offload flag; the
  correct model is structural (row-present+blob-absent ⇒ offloaded; row-absent ⇒
  deleted). M0 is **buildable now** without new test data. **Residual, non-blocking:**
  the one *positive* signal we'd still like to confirm — that `ck_record_id`-populated
  rows are the recoverable ones — can't be validated on the current mirror (100%
  offloaded, no present blobs); it needs a backup with "Optimize Storage" **off**.
  M0 ships the structural model and labels that hint "heuristic" until confirmed.
- **Account lockout (T2).** The defining risk. Mitigations: per-item default,
  bulk-only-on-opt-in with warning, single-session reuse, backoff, and never
  storing the password. The consent copy states it plainly.
- **Moving target (T2).** The private protocol is undocumented and breaks without
  notice (SRP/anisette/endpoint/session changes). The separate-repo cadence exists
  precisely to absorb this; TraceLoupe pins a known-good crate version.
- **Auth path — RESOLVED (2026-07-21).** Earlier worry was "Rust crates target
  GrandSlam, not iCloud web login." Resolved: the target is the **idmsa web SRP-6a
  flow** (no anisette), fully specified from `icloudpd` in
  `docs/icloud-live-fetch-porting-spec.md`; `apple-private-apis` (GrandSlam/GSA) is
  the *wrong* system and is not used for auth. Residual risk is churn (below), not
  feasibility.
- **Notes has no reference implementation (open gap).** No open-source project
  fetches Notes over the private protocol — pyicloud has no `notes.py`. Notes is a
  reverse-engineering spike (capture `com.apple.notes` CloudKit traffic), not a
  port. **Photos leads M2**; Notes scope stays conservative until the spike lands.
- **Tier 1 fidelity.** The Data & Privacy export's Notes format may not reconstruct
  embedded images the way `NoteStore.sqlite` does; reconciliation may be
  approximate (title/time rather than exact identifier). Unmatched items still
  import as standalone recovered media.
- **ADP prevalence.** ADP disables T2 entirely (and iCloud.com web access). We
  detect and route to T1; no way around it, by design.
- **Legal.** T2 uses an unsanctioned API; even own-account use is contractually
  prohibited (not statutorily settled). Ship behind explicit consent; a brief legal
  review before M2 leaves experimental.
- **Open:** Should recovered media survive a re-import (augmentation store keyed to
  backup identity, not cache lifetime)? Should Photos Live Fetch be exposed at all
  in v1, or kept as an internal by-product until Notes proves the path? Do we ever
  offer a Tier-1-style local-folder "import loose iCloud downloads" path for users
  who grab files manually from iCloud.com?

## 9. References

- Research + citations: [`icloud-offloaded-media-research.md`](icloud-offloaded-media-research.md)
- Decision record: [`adr/0001-icloud-offloaded-media-two-tier.md`](adr/0001-icloud-offloaded-media-two-tier.md)
- Apple Data & Privacy (Tier 1) — <https://privacy.apple.com>; format/timing — <https://support.apple.com/en-us/108306>
- `pyicloud` — <https://github.com/icloud-photos-downloader/pyicloud> · `icloudpd` — <https://github.com/icloud-photos-downloader/icloud_photos_downloader>
- Apple-ID auth primitives (Rust) — <https://github.com/SideStore/apple-private-apis>
- CloudKit Web Services (why the sanctioned path fails) — <https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/CloudKitWebServicesReference/index.html>
- Messages in iCloud E2E / extraction (Elcomsoft) — <https://blog.elcomsoft.com/2018/11/messages-in-icloud-how-to-extract-full-content-including-media-files-locations-and-documents/>
- Apple-ID lockout behavior (Reincubate) — <https://reincubate.com/support/how-to/apple-id-icloud-locked/>
