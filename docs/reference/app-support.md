# App support tracker

A **living table** of what TraceLoupe reads, and the version each source gains
**native** support — i.e. an in-house Rust parser reading the backup directly,
with no dependency on the iLEAPP sidecar. Update a row the moment its status
changes; this file is the single source of truth for coverage.

Companion to `../CHANGELOG.md` (milestones) and
`../product-overview.md` §13 (roadmap rationale). For *field-level*
coverage within each app — everything in its DB and what we surface — see
[`app-data-coverage.md`](app-data-coverage.md).

**Last updated:** 2026-07-18 · current release **0.12.0**

## Status legend

| Mark | Meaning |
|------|---------|
| ✅ **Native** | Own Rust parser, no iLEAPP. The "Native since" column gives the version. |
| 🟡 **Via iLEAPP** | Surfaced today, but through the iLEAPP engine — not yet native. |
| ⬜ **Planned** | Not surfaced in any path yet; a target version is listed when scheduled. |
| ⚪ **Little local data** | The app deliberately stores little/nothing recoverable locally (encrypted or server-side), so there may be nothing to parse regardless of engine. |

> "Native since" is filled only when the **native** parser ships. An app can be
> 🟡 (readable via iLEAPP) for several releases before its ✅ native row lands.

## First-party (device data)

| Data | Source file | Status | Native since |
|------|-------------|--------|--------------|
| Messages (iMessage/SMS) | `sms.db` | ✅ Native | 0.2.0 |
| Group-chat names/members | `sms.db` (`chat.db` schema) | ✅ Native | 0.2.0 |
| Notes (incl. locked & pinned) | `NoteStore.sqlite` | ✅ Native | 0.2.0 |
| Voice recordings | `CloudRecordings.db` + `.m4a` | ✅ Native | 0.2.0 |
| Camera roll (photos/videos) | `Photos.sqlite` / DCIM | ✅ Native | 0.1.0 |
| Photo people (face tags) | `Photos.sqlite` (ZDETECTEDFACE→ZPERSON) | ✅ Native | 0.7.0 |
| Call history | `CallHistory.storedata` | ✅ Native | 0.3.0 |
| Safari history | `History.db` | ✅ Native | 0.3.0 |
| Safari bookmarks / reading list | `Bookmarks.db` | ✅ Native | 0.7.0 |
| Safari open tabs / tab groups | `SafariTabs.db` | ✅ Native | 0.7.0 |
| Contacts | `AddressBook.sqlitedb` | ✅ Native | 0.3.0 |
| Installed apps | `Info.plist` (Installed Applications) | ✅ Native | 0.1.0 |

**iLEAPP is no longer run at all.** Every artifact TraceLoupe surfaces —
first-party *and* third-party (TikTok/WhatsApp/Telegram messages, TikTok
contacts, …) — is parsed natively, so a default import launches no iLEAPP
subprocess and doesn't even require the engine to be installed (import ~35s vs.
minutes). iLEAPP is kept only as a **development-time reference** — a source
checkout to read how it extracts artifacts we can't inspect in our own backup.
The sidecar/normalize code path remains but dormant (no catalog module carries an
iLEAPP key); it would only re-activate if a future long-tail module opted in.

## Third-party apps

Tiers mirror the roadmap (`docs/product-overview.md` §13.1), ordered by
prevalence × local-data richness × parse feasibility. Native rollout is by
**scheduled batch, not strict tier order** — an app can be pulled forward.

**Native app-module framework** (`parsers/apps/`): each app is a small module that
locates its own DB and parses it into a shared message stream; one shared inserter
writes the threads/messages. Adding an app is one module file + a registry entry.
**WhatsApp is the first module** (cleanest schema — used to validate the framework).

**Batch 1 (0.3.0) — native third-party wave.** Which apps are feasible is set by
what's actually in the backup (confirmed by whether iLEAPP even has a module):

> ✅ done: **WhatsApp**, **Facebook Messenger** (clean SQLite), **Instagram** (DMs
> via a native NSKeyedArchiver decoder), **TikTok** — messages
> (`ChatFiles/<uid>/db.sqlite` `TIMMessageORM`, JSON `content` classified into text
> + typed markers for shared video/sticker/profile) AND contacts/social-graph
> (`AwemeIM.db` `TTKIMContactBaseUser*`/`AwemeContacts*`). Both TikTok DBs are now
> **validated against a real backup** (263k messages, 145 contacts). Instagram is
> *unvalidated* against a real backup.
> Investigate (data exists but not open-source-documented — see Research notes):
> **Snapchat**, **X/Twitter**, **Facebook** (main app).

**Telegram** — ✅ native as of 0.4.0 (dev): a native reader for its binary
"postbox" format (`t7` messages, `t2` peers). Messages only; unvalidated.

**Research notes — apps with no iLEAPP module** (web research, July 2026; "no
iLEAPP module" ≠ "no data"). These need a real backup to pin exact schemas before
a parser is safe to write:
- **Snapchat** (`com.toyopagroup.picaboo`) — iOS *does* persist chats, contacts,
  best-friends, and memories in the app's `Data/Application/<uuid>/` container
  (SQLite under a `databases/`-style folder; message receipts too). Records
  survive "Clear Conversation" in WAL/freelist. **Caveat:** the store is often
  **encrypted** (Cellebrite/others decrypt it), so it may need key material — the
  reason iLEAPP has no module. Upgraded from "no data" to *investigate*.
- **X/Twitter** (`com.atebits.tweetie2`) — no documented clean message DB; DMs
  aren't in a well-known SQLite. Cached API responses live in the **generic
  `Cache.db`** (unencrypted CFURL cache: `cfurl_cache_response` +
  `cfurl_cache_blob_data`). Best-effort carving, not a clean store.
- **Facebook main app** (`com.facebook.Facebook`) — chats route through Messenger
  (already native); the rest is feed/media in `Cache.db`.
- **Generic `Cache.db` opportunity:** nearly every app has
  `Library/Caches/<bundle>/Cache.db` holding cached network content. One generic
  native module (iLEAPP does this via `fsCachedData`/`cachev0`/`parsecdCache`)
  could surface cached data across many apps at once — a strong future addition.

Sources: forensafe (iOS Snapchat/Messenger), Cellebrite & xperylab (Snapchat
decryption), TrustedSec (iOS `Cache.db`), SANS ISC / AboutDFIR (iOS app artifacts).

### Tier 1 — Top 10

| App | What's stored locally | Status | Native since |
|-----|-----------------------|--------|--------------|
| WhatsApp | Messages (rich local SQLite) | ✅ Native — messages | 0.3.0 |
| Facebook Messenger | Messages (`lightspeed` SQLite) | ✅ Native — messages | 0.3.0 |
| TikTok | Messages (JSON) + social-graph contacts | ✅ Native (validated) — messages + content kinds; 🟡 contacts | 0.3.0 |
| Instagram | DMs as archived plists | ✅ Native (unvalidated) — DMs | 0.3.0 |
| Telegram | Messages (binary "postbox" format) | ✅ Native (unvalidated) — messages | 0.4.0 |
| Facebook (main app) | Chats via Messenger (done); feed/media in generic `Cache.db` | ⬜ Investigate (Cache.db) | TBD |
| Snapchat | Chats/contacts DO persist on iOS (often encrypted) | ⬜ Investigate (real backup) | TBD |
| YouTube | Watch/search history, cache | ⬜ Planned | TBD |
| Gmail | Cached mail/metadata | ⬜ Planned | TBD |
| WeChat | Messages, media | ⬜ Planned | TBD |
| Signal | Encrypted local store | ⚪ Little local data | — |

### Tier 2 — Top 25 (adds)

| App | Status | Native since |
|-----|--------|--------------|
| X / Twitter | ⬜ Investigate — DMs not in a clean DB; cached in generic `Cache.db` | TBD |
| Discord | ⬜ Planned | TBD |
| Reddit | ⬜ Planned | TBD |
| Spotify | ⬜ Planned | TBD |
| LinkedIn | ✅ Native (unvalidated) — messages | 0.6.0 |
| Pinterest | ⬜ Planned | TBD |
| Threads | ⬜ Planned | TBD |
| Viber | ✅ Native (unvalidated) — messages, groups | 0.5.0 |
| LINE | ⬜ Planned — iLEAPP schema too thin (no conversation key) | TBD |
| Google Maps | ⬜ Planned | TBD |

### Tier 3 — Top 50 (adds)

| App | Status | Native since |
|-----|--------|--------------|
| Slack | ⬜ Planned | TBD |
| Microsoft Teams | ✅ Native (unvalidated) — messages, groups | 0.5.0 |
| Zoom | ⬜ Planned | TBD |
| Twitch | ⬜ Planned | TBD |
| Tinder / Bumble / Hinge | ⬜ Planned | TBD |
| PayPal / Venmo / Cash App | ⬜ Planned | TBD |
| Uber | ⬜ Planned | TBD |
| Amazon | ⬜ Planned | TBD |
| Strava | ⬜ Planned | TBD |
| Notion | ⬜ Planned | TBD |

### Additional messaging apps (native)

Popular messengers outside the strict tier lists, done via the app-chat framework:

| App | What's stored locally | Status | Native since |
|-----|-----------------------|--------|--------------|
| Kik | `kik.sqlite` (Core Data) | ✅ Native (unvalidated) — messages, groups | 0.4.0 |
| imo | `IMODb2.sqlite` | ✅ Native (unvalidated) — messages, group authors | 0.4.0 |
| Threema | `ThreemaData.sqlite` | ✅ Native (unvalidated) — messages, group authors | 0.4.0 |

Candidate next (harder — need heavier machinery): Discord/Slack (cached JSON /
generic `Cache.db`), Bumble (YapDatabase serialized blobs), Reddit (Matrix-
protocol JSON), LINE/Zangi (thin schemas, need real-backup schema work),
teleguard.

### Tier 4 — Top 100 (the long tail)

Regional messengers, banking/fintech, travel, fitness, productivity apps, and
popular games. Enumerated per-app as they're scheduled; add rows here when a
specific app is picked up.

### Priority: apps in the reference backup (grounded triage)

The apps actually installed on the test device (`00008101-…001E`), triaged
against their **real** on-disk containers via the decrypted `Manifest.db` — not
guessed. **Key finding: none are chat apps.** The WhatsApp/Telegram-style
message-parser loop does not apply to any of them; the real recoverable value
here is *creative media* plus a few thin web caches. Prioritise accordingly.

**A. Real local personal content — worth building (feeds the Gallery, not Messages)**

| App | Bundle | What's actually stored locally | Mechanism | Priority |
|-----|--------|-------------------------------|-----------|----------|
| PicCollage | `com.cardinalblue.PicCollage` | `Documents/PhotoCollage.sqlite` catalog + **486** project files (`collage_*/multi_page.json` layouts + embedded source photos) | New "creations" media path: read the sqlite catalog, surface each collage + its source images in the Gallery | **1 (richest)** |
| ibisPaintX | `jp.ne.ibis.ibisPaintX` | **79** `Documents/` files — drawings + `.thumbs/list_*.png` thumbnails, `.settings/*.dat` | Same creations path: enumerate artworks + thumbnails into the Gallery | 2 |

**B. Web-view apps — content is server-side, almost nothing cached (best-effort, low yield)**

These are WKWebView shells: their screens are fetched live and rendered from the
web, so there is no local message/store to parse. Confirmed thin: Edlevo's
`iOS_WKWebView_app.sqlite` and IndexedDB are near-empty; StudyBee has only 16
WebKit housekeeping files. Not worth a native parser until/unless a real backup
shows substantial `Library/WebKit/WebsiteData` content to carve.

| App | Bundle | Note |
|-----|--------|------|
| Edlevo (school↔guardian) | `com.tieto.se.edumobile` | Swedish school comms — but WKWebView; messages live server-side |
| StudyBee | `se.studybee.student` | WKWebView; only Firebase + WebKit stats locally |
| Mecenat (student ID) | `mecenat` | WKWebView + Safari extension; card/profile fetched live |
| Skånetrafiken (transit) | `se.skanetrafiken.prod-washington-ios` | Tickets/journeys server-side; no local DB |

**C. Public / low-value local data**

| App | Bundle | Note |
|-----|--------|------|
| Skolmat (lunch menus) | `se.yostudios.skolmat` | `Documents/Index.db` caches *public* school-lunch menus — little personal value |

**D. Nothing recoverable — encrypted, server-side, or no personal data** ⚪

Recorded so we don't re-investigate: **Swish** (`se.bankgirot.swish`),
**Mobilbanken/Sparbanken** (`se.sparbankerna.mobilbankenprivat`), **BankID**
(`com.bankidapp.BankID`) — financial, encrypted key-store only. **Bitdefender
iOSSecurity**, **Kaspersky SafeKids** — security agents. **WidgetSmith**
(`com.crossforward.WidgetSmith`) — widget config in prefs, no comms. Games
(2048, Magic Tiles, Roblox, Wow), **SoundHound**, and the Google suite
(Gmail/Drive/Docs/Classroom/Calendar — Google-server-side; Gmail already tracked
in Tier 1) carry nothing of forensic interest locally.

**Priority order (set by the project owner):**
1. **Support every app in this backup** — extract whatever each app stores
   locally (media, records, cached content), regardless of whether it's a chat
   app. The parser loop generalizes from "chat modules" to **app-artifact
   modules**: each app module locates its container files and emits whatever
   TraceLoupe can surface (Gallery media, structured records, etc.).
   Build order by data richness: **PicCollage → ibisPaintX** (real creative
   media) first, then the web-view/thin apps (best-effort), then record the
   ⚪ nothing-local apps with their reason so they're not re-investigated.
2. **Then** move on to other popular apps beyond this backup (the Tier 1–3 lists
   above), using public DFIR test images where we lack local data.

This means the app-chat framework (`parsers/apps/`) grows a sibling notion of a
**media/artifact module** that writes into the Gallery (and, later, other views)
instead of the message stream. PicCollage is the first such module and validates
the generalized path.

## How to update this file

1. When an app/source gains **native** support, flip its status to ✅ and set
   **Native since** to the shipping version.
2. When an app is first surfaced via iLEAPP, set it 🟡 and note which artifacts.
3. Keep the **Last updated** line and `CHANGELOG.md` in step.
4. Apps that store nothing usefully local stay ⚪ — record *why* so we don't keep
   re-investigating them.
