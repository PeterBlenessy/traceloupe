# Spike: iLEAPP as the MVP parsing engine

*Milestone 1 · de-risk the engine boundary before building the import pipeline.*

This records what we verified by running iLEAPP against a synthetic encrypted
backup, and the decisions that follow. Reproduce it with the commands in
[§ Reproducing](#reproducing).

## Questions this spike had to answer

1. Can iLEAPP run **headless** and decrypt an **encrypted** iOS backup itself,
   or do we need our own decryptor for the MVP?
2. What does its `_lava_artifacts.db` output look like for the artifacts we
   surface — is it a stable thing our normalizer can read?
3. Does the download-on-first-use delivery model hold — do the GitHub releases
   ship a macOS Apple Silicon binary that runs?

## Findings

### 1. iLEAPP decrypts encrypted backups itself — no MVP decryptor needed ✅

iLEAPP takes `-t itunes --itunes_password <pw>` and does the full keybag →
class-key → `Manifest.db` → per-file decryption internally
(`scripts/search_files.py`). Against our fixture it logged:

```
Detected encrypted iTunes backup
Manifest.db was successfully decrypted with passcode salvage-test
```

and extracted + parsed the encrypted `sms.db`. **Consequence:** the Phase 1
MVP does not need the native Decryptor at all — that work moves entirely to
Phase 2, where we decrypt individual files on demand. The MVP just shells out
to iLEAPP with the password.

### 2. `_lava_artifacts.db` is a clean per-artifact SQLite — good normalizer source ✅

All 5 seeded messages arrived intact in a per-module `sms` table:

```sql
CREATE TABLE sms (
  message_timestamp INTEGER, read_timestamp INTEGER, message TEXT,
  service TEXT, message_direction TEXT, message_sent TEXT,
  message_delivered TEXT, message_read TEXT, account TEXT,
  account_login TEXT, chat_contact_id TEXT, attachment_name TEXT,
  attachment_file TEXT, attachment_timestamp INTEGER,
  attachment_mimetype TEXT, attachment_size_bytes TEXT,
  message_row_id TEXT, chat_id TEXT, from_me TEXT
);
```

Key structural facts for the normalizer (salvage-core, Milestone 2):

- **One table per artifact module**, named after the module (`sms`). Column
  names are the module's `data_headers` lowercased with spaces → underscores.
  So the lava schema is determined by the iLEAPP module version — which is
  exactly why we **pin** the iLEAPP release (see architecture §9).
- **Timestamps are already normalized to Unix epoch seconds** in lava
  (`1717840800` = 2024-06-08 10:00 UTC). We do not need to redo Cocoa/Core-Data
  time conversion for iLEAPP-sourced artifacts.
- **Direction** is a rendered string (`Incoming`/`Outgoing`), not the raw
  `is_from_me` int; `from_me` is also present. Our cache stores the boolean.
- **Media** is catalogued in three engine tables:
  `_lava_media_items(id, source_path, extraction_path, type, metadata, …)` and
  `_lava_media_references(media_item_id, module_name, artifact_name, …)`. An
  artifact row references media by id; the bytes live at `extraction_path`
  under the output folder. **Now verified end-to-end** by seeding an image
  attachment: iLEAPP extracted it, wrote the bytes to `media/<hash>.png`
  (matching the seeded PNG's size), set `_lava_media_items.type` to the MIME
  type (`image/png`), `.source_path` to the original backup path, and
  `.extraction_path` relative to the output dir; the `sms` row's
  `attachment_file` column holds the media-item id. **Normalizer contract:**
  resolve an artifact row's media-ref id → `_lava_media_items.extraction_path`
  → real bytes on disk. This is what the Gallery and Messages views consume.
- Other engine tables — `itunes_backup_info(property, property_value)`,
  `_file_path_list`, `_artifact_search_patterns`, `_artifact_pattern_to_file`
  — are metadata we can read opportunistically (device info) or ignore.

### 3. The frozen macOS release is currently BROKEN ⚠️ — spike's most important risk

`ileapp-v2026.1.0-macOS_Apple_Silicon.zip` (49 MB, from the official releases)
is a single-file arm64 Mach-O that runs `--help`, but **crashes on startup**
during plugin loading:

```
ImportError: cannot import name 'ImageDraw' from 'PIL'
  (…/scripts/artifacts/springboard.py)
[PYI-9043:ERROR] Failed to execute script 'ileapp'
```

This is a PyInstaller freeze bug (Pillow not fully bundled), not a bug in our
setup — it fails before touching any input. **The same run from an iLEAPP
source checkout with `requirements.txt` installed works perfectly.**

Implication for the download-on-first-use plan: we cannot assume the upstream
frozen macOS binary is usable. Options, in preference order:

1. **Build and host our own frozen iLEAPP binary** as a pinned asset (our own
   PyInstaller/`uv` build with Pillow correctly bundled), downloaded on first
   use. Keeps the delivery model; puts the freeze under our control — which we
   wanted anyway for version pinning.
2. Ship a managed Python environment (e.g. a pinned `uv` + iLEAPP source) and
   run from source. Larger, but avoids freezing entirely.
3. File/track the upstream release bug and fall back to (1) until fixed.

Either way, **the download source becomes our own release, not upstream's** —
which is strictly better for the SHA-pinning story in architecture §9. The
first-import download stays the same size ballpark (~50 MB).

### 3c. Contacts: iLEAPP's `addressbook` lava table is lossy ⚠️

Calls (`callhistory`) and Safari (`safarihistory`) produce clean, complete lava
tables — column-for-column what the module's headers describe, epoch
timestamps, decoded phone numbers and call-type/direction strings. Both are
normalized and validated end-to-end (see the normalizer's integration check).

**Contacts is different.** Running iLEAPP against a real `AddressBook.sqlitedb`
(iLEAPP's own `belkasoft_ctf6` addressBook test fixture), the `addressBook`
module's HTML report has the full contact, but the **lava** table
(`addressbook`) came out with only 6 columns —
`creation_date, prefix, middle_name, phone_numbers, group_name, storage_place`
— **missing `first_name`, `last_name`, `company`, `display_name`, and
`email_addresses` entirely**. The lava schema also looks data-dependent (a
sparse contact yields a sparse column set), which a fixed-column normalizer
can't rely on.

So Contacts can't be sourced from iLEAPP's lava output the way the other
artifacts are. Options for a follow-up:
1. Read iLEAPP's per-artifact **TSV/HTML export** for contacts instead of lava.
2. Bring the **native AddressBook parser forward from Phase 2** — the schema is
   stable and well-understood, and this removes the iLEAPP dependency for the
   one artifact where its lava output fails us.

Recommendation: (2). Contacts is a good first native parser — bounded scope,
stable format, and it sidesteps a real iLEAPP limitation. Tracked as the next
piece of work; Calls and Safari shipped without it.

### 3b. iLEAPP is dependency-version-sensitive — pin its deps when we re-freeze ⚠️

Running from source with **unpinned** deps installed pandas 3.0, whose default
string dtype coerces `None` → `NaN` (a truthy float). iLEAPP's SMS chat
renderer assumes `None` stays falsy, so with pandas 3.0 it enters the
attachment branch for *every* text row and crashes (`'float' object has no
attribute 'split'`) — taking the whole `sms` artifact (and its lava table)
down with it, but only when a thread mixes attachment and non-attachment
messages. Pinning `pandas==2.2.3` (with `numpy==1.26.4`, which the upstream
release already pins) fixes it. **Consequence:** our re-frozen build must pin
iLEAPP's exact dependency set, not just its source — a `requirements` lock, not
`pip install ileapp-latest`.

## Decisions taken from this spike

- **MVP drops the native Decryptor** — iLEAPP handles decryption; the Decryptor
  and Manifest Index are Phase-2-only, as the architecture already scoped them.
- **Normalizer reads per-module lava tables** with epoch timestamps; the cache
  schema built in Milestone 2 already matches (threads/messages/etc.).
- **We host our own frozen iLEAPP**, pinned by SHA-256, rather than depending on
  upstream's macOS release. Update architecture §9 wording from "download the
  exact pinned release binary from iLEAPP's official GitHub releases" to
  "download our pinned, re-frozen iLEAPP build" (upstream source, our freeze).
- **Next spike step:** seed the fixture with a photo/attachment and confirm the
  media path (`_lava_media_items.extraction_path` → real bytes) before building
  the Gallery. This is the only MVP artifact class not yet proven end-to-end.

## The fixture

`tools/make_fixture_backup.py` builds a ~48 KB, fully valid **encrypted**
backup (real keybag, two-stage PBKDF2 KEK, AES-key-wrapped class/manifest/file
keys, AES-CBC file blobs) with a seeded `sms.db`. Regenerating is ~instant, so
tests build it on the fly rather than committing a binary blob.

**Independently validated** two ways, so a green result isn't just iLEAPP
agreeing with itself:

- The third-party **`iOSbackup`** library (unrelated to iLEAPP) unlocks the
  keybag and decrypts `Manifest.db` from the password alone → the crypto layer
  is correct. (It stops at the per-file blob because it wants a full
  NSKeyedArchiver `MBFile`; iLEAPP tolerates the simpler plist we emit, and
  covers that layer instead.)
- **iLEAPP** extracts and parses the encrypted `sms.db` end-to-end → the
  file-blob decryption layer is correct.

The seeded `sms.db` schema matches what iLEAPP's SMS module queries
(`message`, `chat`, `chat_message_join`, with `date` in nanoseconds). Add more
artifact classes by extending `seed_files()` with the schema each iLEAPP module
expects.

## Reproducing

```sh
# 1. Build the fixture (needs `cryptography`)
python tools/make_fixture_backup.py /tmp/fixture-backup --password salvage-test

# 2a. Independent crypto check (needs `iOSbackup`)
python -c "from iOSbackup import iOSbackup; import os; \
  b=iOSbackup(udid='fixture-backup', cleartextpassword='salvage-test', backuproot='/tmp'); \
  print('manifest decrypted, files:', len(b.getBackupFilesList()))"

# 2b. End-to-end via iLEAPP from source
python /path/to/iLEAPP/ileapp.py -t itunes -i /tmp/fixture-backup \
  -o /tmp/ileapp-out --itunes_password salvage-test

# 3. Inspect the engine output
sqlite3 /tmp/ileapp-out/iLEAPP_Output_*/_lava_artifacts.db \
  "SELECT message, message_direction FROM sms;"
```
