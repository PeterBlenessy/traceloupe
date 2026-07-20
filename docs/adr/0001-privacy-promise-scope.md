# Privacy promise covers backup data, not app-operational traffic

TraceLoupe's public positioning said "fully local, offline, no cloud." With the Spyware Analyzer we need fresh indicator feeds, which means network fetches. Decided: the privacy promise is scoped to backup-derived data — nothing read from or derived from a user's backup ever leaves the machine — while disclosed, setting-governed operational traffic (fetching indicator feeds, checking for updates over HTTPS to public repositories) is permitted. The alternative (staying strictly offline and shipping indicators only inside app updates) was rejected because stale indicators quietly hollow out a security feature; visible freshness beats absolute airgap.

## Consequences

- product-architecture-description.md §5 ("no telemetry, no cloud", "fully local, offline") and the Spyware Analyzer PRD §8 must be reworded to the scoped promise.
- Any future operational fetch must be disclosed and controllable in Settings; backup-derived data in a request is forbidden by default. Narrow exceptions require explicit, informed, per-feature opt-in (default off, separately explained). Currently the only sanctioned exception is shortened-URL expansion during an Explicit Scan, which sends URLs found in the backup to resolver hosts.
- Disclosure for indicator updates is: first-run consent dialog, permanent inline note on the scan screen, Settings toggle (default on).
