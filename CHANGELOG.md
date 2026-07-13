# Changelog

All notable changes to **TraceLoupe** are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

While pre-1.0, the **minor** version tracks major milestones:

| Version | Milestone |
|---------|-----------|
| `0.1.0` | **MVP** — iLEAPP-powered import into a local cache, with a native browsing UI. |
| `0.2.0` | **Phase 2 complete** — native lazy-decode core (Manifest Index, on-demand decryption, native Messages/Notes parsers) replacing the eager import for the hot artifacts. |

> The single source of truth for the version is `package.json`; keep the
> workspace `Cargo.toml` and `src-tauri/tauri.conf.json` in step when it changes.

## [Unreleased]

### Added
- **Phase 2 — native Messages, wired in.** The import now materializes Messages
  natively from the backup's `sms.db` via a reusable `ManifestIndex`
  (decrypt-on-demand: resolve `domain/relativePath` → file + key, read one file),
  skipping iLEAPP's `sms` normalize step. iLEAPP remains the automatic fallback
  when `sms.db` is absent or the native parse fails.
- **Phase 2 — native Notes, wired in.** Notes are now read natively from
  `NoteStore.sqlite` (via the same `ManifestIndex`): each note's body is
  gzip-inflated from `ZICNOTEDATA.ZDATA` and its text walked out of the
  `NoteStoreProto` wire format, with folder/title/snippet/timestamps from the
  Core Data columns (schema introspected so version-suffixed column names still
  resolve). iLEAPP's `notes` step is skipped on success and remains the fallback.

### Remaining for 0.2.0
- iLEAPP still *runs* its `sms` and `notes` modules, so no import time is saved
  yet — dropping them is next, gated on real-backup validation of the native
  output.

## [0.1.0] — 2026-07-13

Initial baseline. Open, decrypt, and browse iPhone backups entirely on-device.

### Added
- Discover and open encrypted or unencrypted iPhone backups; first-time import
  via a bundled, checksum-pinned iLEAPP engine, then instant re-open from cache.
- Native, hardware-accelerated backup decryption (keybag → class keys → AES-CBC);
  camera roll read natively with on-demand full-image decryption and cache-once.
- Views: Messages (conversations, cross-conversation timeline, per-year periods),
  Photos (virtualized gallery + full-viewport lightbox with keyboard nav),
  Contacts, Calls, Safari, Notes, and installed Apps.
- Third-party chats surfaced in Messages (TikTok, WhatsApp, Telegram).
- Per-list sorting (field + direction), a 24-hour clock option, resizable and
  icon-rail sidebar, always-visible scrollbars.
- Security: key zeroization, a "forget backup" flow, `backup_id` validation, and
  hardened media serving.
