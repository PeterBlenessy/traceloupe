# iOS "offloaded-to-iCloud vs deleted" flags — authoritative findings

**Question (M0):** from a decrypted iTunes/Finder backup alone (no network), can we
tell whether a referenced media file is **offloaded to iCloud** (blob absent but
recoverable) vs **deleted** (gone)? We currently infer purely from blob absence.

**Verdict:** **No single column gives a clean "offloaded vs deleted" bit.** The
defensible, authoritative model is structural, not a flag:
**row present + blob absent ⇒ offloaded/thinned; row absent ⇒ deleted.** That is
essentially today's heuristic — the research validates it rather than replacing it.
`transfer_state` must **not** be presented as an offload oracle. Two secondary
signals sharpen the labelling (failed-transfer states, and CloudKit-sync columns),
one of them still unverified. Details + citations below.

---

## (A) Messages — `attachment.transfer_state`

`transfer_state` is the raw integer of Apple's **`IMFileTransferState`** enum
(private `IMSharedUtilities`/`IMCore` `IMFileTransfer` object, keyed by the
attachment `guid`). Recovered verbatim from Apple binaries by two mature
reverse-engineered iMessage clients:

| Value | Enum case | Meaning |
|------:|-----------|---------|
| -1 | `archiving` | being archived |
| 0 | `waitingForAccept` | awaiting accept / not yet started |
| 1 | `accepted` | accepted, not moving yet |
| 2 | `preparing` | preparing |
| 3 | `transferring` | actively up/downloading |
| 4 | `finalizing` | finalizing |
| **5** | **`finished`** | **transfer complete — file was present locally at completion** |
| 6 | `error` | failed (terminal) |
| 7 | `recoverableError` | failed, retryable |
| 8 | `rejected` | rejected (terminal) |
| 9 | `thumbnail` | thumbnail-only |

Source: `beeper/barcelona` → `Core/Barcelona/Attachments/IMFileTransfer+StateBound.swift`;
`beeper/platform-imessage` → `src/IMessage/Sources/IMDatabase/Models/Attachment.swift`
(`transferState: IMFileTransferState?` read directly from `attachment.transfer_state`;
`isTerminalFailure = error | recoverableError | rejected`). Framework existence
confirmed in Apple SDK/restore dumps (`IMSharedUtilities.framework`/`IMCore.framework`).

### Why it is NOT an offloaded-vs-deleted signal

1. **Completion latch, not liveness.** `5` means the transfer *once* finished;
   nothing rewrites it when the blob is later offloaded to Messages-in-iCloud or
   thinned. So `5` + missing blob is ambiguous and adds nothing over blob-absence.
2. **Deletion removes the row.** A deleted attachment normally leaves **no row**
   (recovery is via ROWID gaps / WAL / Biome, per Belkasoft & The Binary Hick), so
   "deleted" ≠ "row with a special state." The real discriminator is
   **row-present vs row-absent**.
3. **No forensic tool treats it as an offload oracle.** iLEAPP's
   `scripts/artifacts/sms.py` doesn't even select `transfer_state`; it decides
   "present" purely by file existence — identical to our heuristic. Beeper uses
   `transfer_state` **plus** an on-disk existence check, never the state alone.
4. **Backup-scope confound.** With Messages-in-iCloud on, blobs live in CloudKit
   and are legitimately absent from the backup while the row remains at
   `transfer_state = 5` — the exact "offloaded but recoverable" case, and it is
   indistinguishable from `5` alone.

> Contradiction to note: `kacos2000/Queries/sms_db.sql` annotates `5 = "temp
> folder"`, `6 = "Library"` — an empirical storage-location guess that **conflicts
> with** the Apple enum above. Prefer the enum (two independent RE codebases agree
> on names *and* integers).

### What `transfer_state` *can* honestly do

Distinguish **"failed / never completed"** (`6/7/8`, and arguably `0`) from
**"was successfully local once"** (`5`). A blob-absent row at `6/7` is a *failed
download*, not an offload — surfacing those differently is a real, local,
authoritative improvement over pure blob-absence.

### The stronger-in-principle signal: CloudKit columns

Modern `attachment` schema carries `ck_sync_state` (INTEGER), `ck_record_id`
(TEXT), `ck_server_change_token_blob` (BLOB) — Messages-in-iCloud participation. A
populated `ck_record_id` / non-zero `ck_sync_state` means the attachment is (or
was) synced to CloudKit → **recoverable**, a better "offloaded not deleted" hint
than `transfer_state`. **But** the integer semantics of `ck_sync_state` are
undocumented in every primary source found, and **no** mainstream tool reads them —
treat as a promising lead, validate empirically before trusting.

---

## (B) Notes — `ZICCLOUDSYNCINGOBJECT` (`NoteStore.sqlite`)

iLEAPP's `scripts/artifacts/notes.py` joins `ZICCLOUDSYNCINGOBJECT` to itself and
decides image presence **purely by filesystem existence**
(`os.path.exists(Accounts/LocalAccount/Media/{ZIDENTIFIER}/{ZFILENAME})`) — there
is **no** "needs download from CloudKit" boolean consulted. Same heuristic we use.

Relevant columns:
- `ZFILENAME`, `ZIDENTIFIER` — on-disk path components.
- `ZFILESIZE` — declared media size; non-zero ⇒ a real media object exists in the
  record even if the blob isn't on disk.
- `ZSERVERRECORDDATA`, `ZSERVERSHAREDATA` — archived CloudKit record/share
  metadata; presence ⇒ synced to iCloud (ciofecaforensics, "Revisiting Apple Notes
  (7): CloudKit Data").
- `ZUNAPPLIEDENCRYPTEDRECORD` — encrypted CloudKit record pending application.
- `ZCRYPTO*` + `ZISPASSWORDPROTECTED` — locked-note encryption params.

**Verdict:** no confirmed dedicated "not yet downloaded from CloudKit" boolean.
Best inferential signal: media row with `ZFILESIZE > 0` + populated
`ZSERVERRECORDDATA` (proves it synced) but **no file on disk** ⇒ likely
offloaded/recoverable. Inference, not an authoritative flag.

---

## Empirical check (`~/.traceloupe-dev/backup-mirror`, 2026-07-21)

This backup has Messages-in-iCloud **on** and is **fully offloaded**:
- `MediaDomain/Library/SMS/Attachments/` holds **7 files, all `.pvt/metadata.plist`
  — zero media blobs**.
- **0 of 8 465** attachment rows have a blob on disk, across *every* value of
  `transfer_state` (0: 5 463, 5: 2 102, 6: 896, −1: 3, 7: 1) **and** `ck_sync_state`
  (0: 1 253, 1: 6 963, 2: 238, 4: 11). `ck_record_id` populated for 6 989 / empty
  for 1 476.

So: (a) confirms `transfer_state`/`ck_sync_state` do **not** predict presence when
nothing is present, and (b) this mirror has **no positive ground truth** — every
row is offloaded — so the one thing still worth confirming (that
`ck_record_id`-populated rows are the recoverable ones) needs a **different backup,
taken with "Optimize Storage" off**. That confirmation is a refinement, not a
prerequisite: the row-present/row-absent model ships now.

---

## Confidence

- **High:** the `IMFileTransferState` map; that `transfer_state` is a
  non-updated completion latch; that iLEAPP (sms + notes) and Beeper decide
  presence by file existence; that deletion removes the row.
- **Medium:** "row-present + blob-absent ⇒ offloaded/thinned" (strong under
  Messages-in-iCloud; weaker for orphaned/WAL-recovered rows).
- **Low / genuine gap:** exact `ck_sync_state` integer semantics and whether a
  value cleanly means "offloaded to CloudKit" — undocumented, unused by any tool;
  validate on a non-offloaded backup. No confirmed Notes "needs-fetch" boolean.

## Sources
- `beeper/barcelona` `IMFileTransfer+StateBound.swift`; `beeper/platform-imessage`
  `IMDatabase/Models/Attachment.swift` (authoritative enum).
- `abrignoni/iLEAPP` `scripts/artifacts/sms.py`, `scripts/artifacts/notes.py`
  (presence-by-file-existence).
- `kacos2000/Queries/sms_db.sql` (contradicting empirical labels — flagged).
- Elcomsoft ("Dude, Where Are My Messages?"), Belkasoft ("Lagging for the Win"),
  The Binary Hick (deletion = row removal → ROWID/WAL/Biome recovery).
- ciofecaforensics, "Revisiting Apple Notes (7): CloudKit Data"
  (`ZSERVERRECORDDATA`, `ZUNAPPLIEDENCRYPTEDRECORD`, `ZCRYPTO*`).
- Apple SDK/restore dumps confirming `IMSharedUtilities`/`IMCore` `IMFileTransfer`.
