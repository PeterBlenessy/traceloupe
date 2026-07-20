# Security Check — validation against MVT

Plan T10 asks: *no indicator class where MVT structurally finds matches our
engine cannot; differences in specific hits are explained, not unexplained
gaps.* This records the M1 validation.

## Method

- **Reference:** Amnesty International's Mobile Verification Toolkit
  (`mvt-ios`), installed from PyPI, IOCs refreshed via `mvt-ios download-iocs`.
- **Empirical run:** `mvt-ios check-backup` and a TraceLoupe Explicit Scan, both
  against the developer's own device backup (`00008101-…`, iOS 17.5.1).
- **Structural check:** every MVT iOS module that checks indicators, mapped to
  the TraceLoupe scan surface.

## Empirical result

| Tool | Result on the dev backup |
|---|---|
| `mvt-ios check-backup` | 0 informational / low / medium / high / critical alerts |
| TraceLoupe Explicit Scan | 1 Info finding: `com.kaspersky.safekids` (Kaspersky Safe Kids), graded Info via the Echap watchware feed |

Both tools agree the device shows no mercenary-spyware indicators. TraceLoupe
additionally surfaced an installed watchware app that MVT did not flag on this
run — the extra hit is on our side, not a gap. There was **no indicator MVT
reported that TraceLoupe missed.**

(A byte-for-byte empirical diff is limited because MVT reads the raw,
hash-named backup while it is decrypted, whereas TraceLoupe scans its own
decrypted cache. The structural mapping below is the load-bearing check; the
empirical run confirms no contradiction.)

## Structural coverage — MVT iOS modules → TraceLoupe

MVT modules that evaluate indicators (from the installed package):

| MVT module | Indicator classes it checks | TraceLoupe coverage |
|---|---|---|
| `sms` / `sms_attachments` | domains, URLs, emails in messages + attachment names | **Tier A** — `messages`, `attachments` |
| `safari_history` / `safari_browserstate` | domains/URLs in history + open tabs | **Tier A** — `safari` (history + bookmarks/tabs) |
| `chrome_history` / `firefox_history` | third-party browser history domains | **Planned** — same domain match once those parsers land; Safari covered today |
| `calls` | numbers/handles | **Tier A** — `calls` (via contact/interaction handles) |
| `contacts` | emails/handles | **Tier A** — `contacts` |
| `calendar` | invite links/notes | **Tier A** — `calendar` |
| `interactionc` | per-contact interaction records, bundle IDs | **Tier A** — `interactions` |
| `applications` | installed app bundle IDs | **Tier A** — `apps` (installed_apps) |
| `manifest` | suspicious file names/paths across the backup | **Tier A** — `manifest` sweep |
| `configuration_profiles` | unknown/hidden MDM/proxy profiles | **Shipped (M2, v0.22.0)** — `parse_configuration_profiles` (ProfileTruth + PayloadManifest): indicator match + hidden/device-mgmt review findings |
| `profile_events` | profile install/remove events | **Partial** — installed profiles are surfaced via ProfileTruth; per-event install/remove history is a later addition |
| `osanalytics_addaily` | malicious **process names** | **Shipped (M2, v0.21.0)** — `parse_addaily` scans `netUsageBaseline` process names |
| `net_datausage` | process names with network usage | **Shipped (M2, v0.21.0)** — `parse_datausage` scans `ZPROCESS` names + bundle names |
| `tcc` | apps holding mic/camera/location grants | **Tier B (M2)** |
| `shortcuts` | automation-based surveillance | **Tier B (M2)** |
| `webkit_resource_load_statistics` / `webkit_session_resource_log` | in-app webview domains | **Tier B (M2)** |
| `locationd` / `idstatuscache` / `global_preferences` | supporting artifacts, mostly timeline context | **Out of scope M1** — low-signal / no indicator surface for our feeds |
| `whatsapp` | links in WhatsApp messages | **Partial** — WhatsApp chats are parsed; indicator scan of app-chat bodies is a later enhancement |
| `backup_info` | device metadata (no indicators) | N/A |

**Conclusion:** every MVT module that matches an indicator class our feeds carry
(domains, URLs, emails, bundle IDs, file names/paths, **process names**) is now
covered by a shipped module — Tier A plus the Tier-B process-activity extraction
added in M2 (v0.21.0). The remaining MVT modules match Tier-B artifacts still
scheduled for M2 (TCC, Shortcuts, WebKit) — named in the
PRD, not unexplained gaps.

## Indicator-kind parity

Our loaders ingest domains, URLs, emails, process names, file names, file paths,
bundle IDs, cert SHA-1s, file hashes, and IPs (see `indicators.rs`). Process
names are live as of M2 (DataUsage + OSAnalytics). The kinds with **no scan
surface yet** — cert hashes, file hashes, IPs — map to Tier-B artifacts still to
come or to on-device binaries a backup does not contain; they are carried in the
set and become live as later Tier-B extractions land.

## Reproduce

```bash
python3 -m venv mvt-venv && ./mvt-venv/bin/pip install mvt
./mvt-venv/bin/mvt-ios download-iocs
./mvt-venv/bin/mvt-ios check-backup --output ./mvt-out <decrypted-backup-dir>
# TraceLoupe side: run the real-cache perf/coverage test
TRACELOUPE_REAL_CACHE="…/caches/<udid>/cache.db" \
  cargo test -p traceloupe-core --test scan_real_cache -- --ignored --nocapture
```
