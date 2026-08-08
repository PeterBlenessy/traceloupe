# Media discovery — finding an app's photos without knowing its schema

**Setting:** Settings → Apps → *"Find app media by inspecting the data"*. **On by default.**
**Code:** `crates/traceloupe-core/src/parsers/apps/discovery.rs`, applied in
`import.rs::discover_app_media`.

## The problem it solves

Every app module names the columns its media lives in. That works until the app
ships a new schema — and then the column is gone, the hand-written query finds
nothing, and the app's photos silently stop appearing.

That failure is invisible. An empty gallery looks exactly like a device with no
photos. It is not hypothetical:

- **WhatsApp imported nothing at all** for months, because the query named
  `m.ZPARTNERNAME` and that column belongs to `ZWACHATSESSION`. A statement that
  cannot be prepared aborts the whole parse. The unit fixture declared the column
  in the wrong place, so the test agreed with the bug (#360).
- Notes' hashtags were read from `ZTYPEUTI1`/`ZNOTE1`. Core Data's numeric suffix
  depends on the schema version, so on a differently-numbered device that returns
  **zero tags** and reports success (#355).
- Four `ZASSET` columns were inlined unguarded, so a device missing any one of
  them lost the entire Photos enrichment pass (#354).

Every one of those is the same shape: the code asserted a schema, the device
disagreed, and nothing said so.

## What it does instead

Rather than ask *"is `ZMEDIALOCALPATH` there?"*, the pass asks **"which column in
this database holds values that are actually media in this backup?"**

Every column of every table is sampled (200 rows), and scored two ways:

| Shape | Test | Verified by |
| --- | --- | --- |
| **Path** | Text ending in a media extension | The **Manifest** — is there a file by that name in this backup? |
| **Inline** | Bytes carrying a JPEG/PNG/HEIC/GIF/MP4 signature | The signature itself |

A column has to be *mostly* media (≥50% of sampled values) to count, so a column
of message bodies is not mistaken for media because one row mentions a filename.

**Verification against the Manifest is what makes this safe to run
automatically.** A guess that resolves to nothing scores nothing. A path column
whose files are not in the backup — Threema's `ZMESSAGE.ZFILENAME` is exactly
this — is deliberately *not* reported, because surfacing it would promise the
gallery something it cannot show.

## Where it runs, and where it does not

Discovery **fills a gap**; it does not replace a module. Whatever a module
produced stands.

The gap is measured per app as *"messages that say they carry media, and did not
get any"* — **not** "the module produced nothing at all". An all-or-nothing test
lets a half-broken module through: WhatsApp reads `ZMEDIALOCALPATH`, and if a
future schema moved that column while leaving the thumbnail path behind, it would
still produce *some* attachments and discovery would never look at what it lost.

Measured on the public iOS 17 backup, every app with a database present:

```
WhatsApp   flagged=19  produced=4   discovery runs
Viber      flagged=10  produced=5   discovery runs
Kik         flagged=4  produced=0   discovery runs
imo         flagged=7  produced=0   discovery runs
Threema     flagged=4  produced=0   discovery runs
TeleGuard   flagged=8  produced=0   discovery runs
LINE       flagged=19  produced=0   discovery runs
MEGA       flagged=32  produced=0   discovery runs
Gettr       flagged=4  produced=0   discovery runs
```

Because a module and discovery can now reach the same file, a discovered path is
skipped when that app already has a gallery item for it — verified as zero
duplicate `(source, relative_path)` groups after a full import.

**TikTok is imported outside the app-module registry**, through its own path, so
it sat outside this loop entirely and was the one app the pass silently did not
cover. It is wired in with the same gap test.

A module that names its own columns always wins, because it knows things a
scanner cannot infer: which message a file belongs to, which of several blobs is
the full image rather than the thumbnail, whether a row is a photo or a sticker.

## It is never silent

Everything discovery finds is written to `module_status` and shown against the
app:

```
Threema: Media located by schema discovery:
  ZIMAGEDATA.ZDATA holds inline bytes (8 of 8 sampled verified) -> 8 item(s);
  ZFILEDATA.ZDATA holds inline bytes (3 of 4 sampled verified) -> 3 item(s);
  ZCONTACT.ZIMAGEDATA holds inline bytes (1 of 1 sampled verified) -> 1 item(s)
```

This is a forensics tool. A file that appears because a scanner inferred which
column held it has to be explainable — an examiner needs to know a photo was
attributed by inference, and on what evidence, not merely that it showed up.

## Measured behaviour

Against the public iOS 17 backup, the same import with the setting off and on:

```
OFF   Photos 520   WhatsApp 4   Viber 2
ON    Photos 520   WhatsApp 4   Viber 2   Threema 12   TeleGuard 2
```

Threema's twelve are the interesting ones: its photos are stored **inline in the
database**, not on disk, so there was no path for any parser to find. They are
written out to `<cache>/media/discovered/` and land in the gallery as ordinary
items. Verified as real files — `JPEG image data … 512x512`, EXIF intact.

Run blind as a prototype against every app module, the same pass independently
rediscovered `ZWAMEDIAITEM.ZMEDIALOCALPATH` (WhatsApp) and `ZATTACHMENT.ZNAME`
(Viber) — the two columns that had been found by hand — with matching resolve
counts.

## Putting it back in the conversation

A photo in the gallery but not in the thread it was sent in reads as "no image
was sent". So discovery also tries to attach what it finds to the right message.

Core Data records a relationship as a plain integer column with no metadata
saying it is one, so the link is **inferred from the values**: which integer
column of the media table holds ids of messages we actually imported. Messages
carry `source_id`, the row id they had in the app's own database, so there is
something to match against.

This is the part that can go confidently wrong, so it is deliberately hard to
satisfy. A candidate column is rejected unless:

- it is not Core Data bookkeeping (`Z_PK`, `Z_ENT`, `Z_OPT`). **`Z_ENT` is the
  same number on every row of a table** — when that number happened to equal a
  message id, all eight of Threema's photos were attached to one message.
- **at least 75%** of its non-null values are ids of real messages;
- the mapping does not **collapse** — a column putting twenty rows onto two ids
  is a flag that shares values with message ids, not a relationship;
- it demonstrates the link across **at least three distinct messages**. One row
  of evidence is not evidence: a single-row table whose key happened to equal a
  message id is how a *contact's avatar* came to be attached to "Are you here
  yet?".

When no column passes, nothing is claimed. The media still reaches the gallery
tagged with the app; it simply is not asserted to belong to a conversation.

Measured on the public iOS 17 backup, Threema:

```
12 images in the gallery
 3 attached to their own messages (ZFILEDATA — each an attachment-only message)
 8 unattached (ZIMAGEDATA — the database's ZMESSAGE column is NULL on every row,
   so nothing says which message they belong to)
 1 unattached (a contact avatar, which is not a message attachment at all)
```

Discovery runs **after** the messages are inserted, not before — it can only
match against messages that exist. Running it first left every discovered image
in the gallery and none of them in a conversation.

## Deliberate limits
- **Unique matches only.** A path is used only when exactly one file in the
  Manifest carries that basename. On the iOS 17 backup, 1456 of 6734 basenames
  are shared by more than one file, so "first match wins" would eventually
  attach the wrong photo — a confident error, which is worse than a missing one.
- **Top four columns per database**, best evidence first, so a pathological
  schema cannot turn one import into thousands of inserts.
- **Sampling is bounded** at 200 rows per column; a column that is media only
  after row 200 will be missed.

## The Core Data prefix

Core Data stores a binary attribute with a leading byte, so Threema's photos are
a plain JPEG whose signature begins at **byte 1**, not byte 0:

```
ZIMAGEDATA pk=1 len=22663 firstbytes=01FFD8FFE000104A46494600
                           ^^ prefix
```

`media_magic` therefore scans a short window rather than only offset 0, and the
writer skips to the signature so what lands on disk is a valid file. An early
version checked offset 0 alone and reported "not an image" for every one of
them — the test `magic_is_found_behind_a_core_data_prefix` pins this.
