---
name: create-cache-db-app-parser
description: >-
  Add native support for an iOS app whose chat/social data lives in the CFURL
  network cache (`Cache.db` / `fsCachedData`) rather than a clean local store —
  Discord, Slack, X/Twitter, Facebook, and many others. Provides the schema facts
  and process for a shared generic `Cache.db` reader plus per-app parsers on top of
  it. A sibling to `create-native-app-parser`; follows the same loop (design →
  implement → validate against REAL data → commit + release → review → self-improve)
  but for cache-derived data. Invoke as `/create-cache-db-app-parser <app>`.
---

# Native parser for a Cache.db app

Some apps keep **no clean local message store** — their content only survives as
cached HTTP responses in the iOS **CFURL cache** (`Cache.db` + `fsCachedData`).
Discord, Slack, X/Twitter, and Facebook are the notable ones. This skill builds a
**shared generic `Cache.db` reader** once, then a thin per-app parser that filters
the cached responses for that app's API and decodes them.

**Read `create-native-app-parser` first** — the loop discipline (pitfalls
checklist, real-data validation, review rounds, versioning cadence, self-improve)
all apply here. This skill adds only the cache-specific parts.

## What Cache.db is (schema facts — provenance reference, §10)

Learned from iLEAPP `discord_cache.py` / `fsCachedData.py` / `cachev0.py`; write
fresh Rust from these facts.

`Cache.db` is a SQLite CFURL cache, typically at
`Library/Caches/<bundle-id>/Cache.db` (each app has its own):

- **`cfurl_cache_response(entry_ID, request_key, time_stamp, …)`** — one row per
  cached request. `request_key` is the **URL**; `time_stamp` the cache time.
- **`cfurl_cache_receiver_data(entry_ID, isDataOnFS, receiver_data)`** — the
  response **body**. If `isDataOnFS = 0`, `receiver_data` is the body inline
  (BLOB). If `isDataOnFS = 1`, `receiver_data` is a **filename** pointing into the
  sibling `fsCachedData/<name>` directory where the real body lives.
- **`cfurl_cache_blob_data(entry_ID, response_object, request_object, …)`** —
  `response_object` is a **binary plist** of the HTTP response (headers incl.
  `Content-Type`); `request_object` the request. Decode with `crate::nska`/`plist`
  if you need the content type or request headers.

Join all three on `entry_ID`. The unencrypted CFURL cache is readable without keys
(though `Cache.db` itself is a backed-up file resolved via the ManifestIndex).

## The shared generic reader (build once)

Create `crates/traceloupe-core/src/cache_db.rs` exposing something like:

```rust
pub struct CachedResponse {
    pub url: String,            // request_key
    pub timestamp: Option<i64>, // time_stamp (check epoch — often Mac/Core-Data)
    pub content_type: Option<String>, // from response_object bplist, best-effort
    pub body: Vec<u8>,          // inline receiver_data, or read from fsCachedData/<name>
}
/// Open a Cache.db and yield decoded cached responses, resolving isDataOnFS bodies
/// against the sibling fsCachedData dir. url_filter lets a caller pull only its
/// app's endpoints cheaply.
pub fn read_cache_db(cache_db: &Path, fs_cached_dir: &Path,
                     url_filter: impl Fn(&str) -> bool) -> Result<Vec<CachedResponse>>;
```

Design points: stream/iterate (a real `Cache.db` is large); resolve `isDataOnFS`;
decode the `response_object` bplist only when the content type is needed; be
tolerant of missing receiver rows (`LEFT JOIN`). Unit-test with a synthetic
`Cache.db` (three tables + an fsCachedData file) covering inline vs on-FS bodies.

## The per-app parser (thin, on top of the reader)

An app module locates its `Cache.db` (its bundle's `Library/Caches/<bundle>/Cache.db`
via the ManifestIndex), calls `read_cache_db` with a URL filter for that app's
message API, decodes each matching body (usually JSON), and emits `AppMessage`s
through the shared `insert_app_conversation` — exactly like a `create-native-app-parser`
module. Register it in `APP_CHAT_MODULES` the same way (the `parse` fn wraps the
reader instead of opening a plain SQLite chat DB).

Per-app URL/JSON facts come from the app's iLEAPP module (e.g. `discord_cache.py`,
`discordChats.py`). Example: Discord messages are cached `…/channels/<id>/messages`
JSON responses.

## Cache-specific pitfalls (add to the design-time checklist)

1. **Cache is PARTIAL and EPHEMERAL** — it holds only what was recently fetched
   and not yet evicted, not the full conversation. Never present it as complete.
   Note the coverage limit in `docs/app-data-coverage.md`.
2. **Dedup** — the same message can appear in multiple cached responses (paginated
   fetches overlap). Dedup by the app's message id, not by cache entry.
3. **`isDataOnFS`** — half the bodies are on disk in `fsCachedData`; a reader that
   only reads inline `receiver_data` silently misses them.
4. **Timestamp source** — the cache `time_stamp` is when it was *cached*, not when
   the message was *sent*. Prefer a timestamp from inside the decoded body; fall
   back to the cache time only if the body has none, and say which.
5. **Content-type via bplist** — don't trust the URL extension; the real type is in
   the `response_object` binary plist.
6. **Same shared pitfalls** — per-message author, direction, type-tolerant reads,
   JSON/blob decoding, etc. (see `create-native-app-parser`).

## Validation & the rest of the loop

Same as `create-native-app-parser`: **validate against a REAL `Cache.db`** from a
public DFIR iOS test image that has the app (diff native vs iLEAPP's cache module),
not synthetic-only. Then commit + MINOR release, correctness review-loop (PATCH per
fix), completeness review into the coverage doc, self-improve the checklist, UI/UX
check if it brings a new rendering shape. Because cache data is partial, be
especially clear in the docs that coverage is best-effort.

The generic reader is **reusable machinery** — it ships with the first cache app
(Discord is the natural first) and doesn't get its own version; subsequent cache
apps (Slack, X/Twitter, Facebook) are thin modules reusing it.
