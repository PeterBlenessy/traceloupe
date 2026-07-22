# Recover iCloud-offloaded media via two tiers; Live Fetch as a separate repo

**Status:** accepted (2026-07-20)

## Decision

iOS backups only *reference* much of their media (Notes images, Photos originals,
Messages attachments); the blobs are offloaded to iCloud. To recover them we will
offer **two tiers**, on top of a risk-free local Phase 1:

- **Phase 1 (local, no network):** read iOS native download-state flags
  (`transfer_state`, Notes/Photos download-state columns) so we can honestly
  distinguish *offloaded (in iCloud)* from *deleted* — sharpening today's
  blob-absence heuristic.
- **Tier 1 — Sanctioned Export (default):** import the archive from Apple's
  official Data & Privacy portal. ToS-compliant, no credential handling; async
  (~7 days) and bulk.
- **Tier 2 — Live Fetch (opt-in, consent-gated):** authenticate to Apple's
  private iCloud protocol with the account owner's own credentials (pyicloud
  model), re-implemented natively in **Rust** in a **separate repository/crate**
  that TraceLoupe consumes as an optional, feature-flagged dependency. Ships
  **Notes** first (Photos falls out for free).

## Why

- **Sanctioned CloudKit Web Services cannot reach Apple's first-party
  Notes/Photos/Messages containers** — verified dead end. The only programmatic
  routes to this data are the sanctioned *export* (T1) or the *private* protocol
  (T2). See `docs/research/icloud-offloaded-media-research.md`.
- **The real risk of T2 is automated account lockout, not legal action.** Apple
  has no track record of pursuing individuals for own-account use, but tool-driven
  access "usually" trips an automated security lock (recoverable). That concrete,
  user-facing risk — not an abstract ToS clause — is why T2 is opt-in with an
  explicit consent screen, and why T1 is the default.
- **Browser automation does not escape the ToS** (automated-vs-human is the line)
  and fails the motivating Notes case (no per-image export on iCloud.com), so it
  was rejected as the T2 engine.
- **Separate repo** isolates fragile, ToS-adjacent, network-bound, inherently
  unstable code from TraceLoupe's clean offline core, and lets it ship on its own
  cadence to chase Apple's frequent breakages.
- **Native Rust over vendoring Python icloudpd** to match the project's
  fully-native direction; no complete Rust iCloud-Photos/Notes crate exists, so
  the plan is to build on `SideStore/apple-private-apis` for the Apple-ID auth
  layer and port pyicloud's data layer. We accept owning that data-layer
  maintenance.

## Implementation shape

- **Storage:** recovered blobs (both tiers) go into a separate **augmentation
  store** keyed to the backup, with provenance. The decrypted backup mirror stays
  strictly **read-only** and is never mutated.
- **Trigger (T2):** per-item "Fetch from iCloud" on the referenced-vs-present gap,
  plus an opt-in bulk "fetch all offloaded" run (with a lockout warning). Keeping
  typical request volume low is a deliberate lockout-mitigation.
- **Credentials (T2):** reuse the existing Keychain + biometric infra; never
  persist the Apple ID password in plaintext — store only the trusted-session
  token.
- **Persona:** serves both self-analysis and consenting-subject analysis; T2 is
  designed for whoever can authenticate (Apple ID + 2FA) at fetch time.

## Consequences / no-s

- **Messages is explicitly out of Tier 2 v1.** Messages in iCloud is
  end-to-end encrypted (CloudKit Service Key in iCloud Keychain / backup escrow),
  absent from pyicloud, and not on iCloud.com. It is a separate research spike —
  with a promising lead: TraceLoupe's existing encrypted-backup decryptor may be
  able to recover the service key from the backup keychain escrow.
- Tier 2 is blocked for accounts with **Advanced Data Protection** (which
  disables the web-access mechanism it simulates); T1 still works.
