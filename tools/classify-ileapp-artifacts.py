#!/usr/bin/env python3
"""Classify every iLEAPP artifact by whether an iTunes/Finder backup can reach it.

This is the machine half of `docs/reference/backup-coverage-audit.md`. The doc
records the conclusions; this script is how they were reached, so a later
session can re-run it against a newer iLEAPP checkout instead of trusting a
frozen table.

    pnpm setup:engine                                      # once
    python3 tools/classify-ileapp-artifacts.py             # summary
    python3 tools/classify-ileapp-artifacts.py --self-test # the guard
    python3 tools/classify-ileapp-artifacts.py --json out.json

# The rule — Apple's, not ours

Backup membership is decided by `Domains.plist`, which iOS ships on the device
and `backupd` reads. `tools/data/ios-backup-domains.json` holds that file's
contents (provenance below). Each domain declares a `RootPath` and several path
lists; the ones that matter for a **local** (iTunes/Finder) backup are:

| Key | Effect |
|---|---|
| `RelativePathsToBackupAndRestore` | included |
| `RelativePathsToBackupToDriveAndStandardAccount` | included — *local backups specifically* |
| `RelativePathsToBackupIgnoringProtectionClass` | included |
| `RelativePathsToOnlyBackupEncrypted` | included **only if the backup is encrypted** |
| `RelativePathsNotToBackup` | excluded |
| `RelativePathsNotToBackupToDrive` | excluded from local backups |

Keys naming *Service* or *MegaBackup* are iCloud concerns and are ignored here;
`*Restore*` keys describe the restore side, not what lands in the backup.

Three things this makes visible that guesswork did not:

- `Library/Safari/History.db`, `Library/Safari/BrowserState.db` and
  `Library/CallHistoryDB` are **not** in `RelativePathsToBackupAndRestore`. They
  sit in `RelativePathsToBackupToDriveAndStandardAccount` — in a local backup,
  absent from iCloud. Reading only the base list calls them unreachable, and we
  parse all three today.
- `Library/CoreDuet/People/interactionC.db` and `Health` are
  **encrypted-backup-only**, as is `Library/Safari/SafariTabs.db`. We parse all
  of them, so an unencrypted backup silently loses those views.
- `HomeDomain` is an **allowlist**: `/var/mobile` is not backed up wholesale,
  only the listed subpaths. That is why `Library/Biome` and
  `Library/CoreDuet/Knowledge` appear in no exclusion list — they are absent by
  not being included, which no denylist could have told us.

App containers are not in `Domains.plist`; they follow the documented
`AppDomain` rule — an app's `Documents` and `Library` are backed up except
`Library/Caches` and `tmp`.

# Provenance and its limits

The domain data is transcribed from **iOS 16.4** (iPhone SE 3) by leminlimez —
https://gist.github.com/leminlimez/c602c067349140fe979410ef69d39c28 — a
third-party transcription, not a file we extracted ourselves. Apple **moved**
the file in iOS 17.0, from `/System/Library/Backup/Domains.plist` to
`/System/Library/PrivateFrameworks/MobileBackup.framework/Domains.plist`, and
its contents may have changed with it. Treat results as authoritative for
iOS 16.4 and strongly indicative for later; verifying against a real iOS 17+
copy is tracked on the coverage map.
"""
from __future__ import annotations

import argparse
import ast
import collections
import fnmatch
import glob
import io
import json
import os
import re
import sys
import zipfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ILEAPP = os.path.join(REPO, "engine", "iLEAPP", "scripts", "artifacts")
DOMAINS_JSON = os.path.join(REPO, "tools", "data", "ios-backup-domains.json")
PATH_LISTS = os.path.join(REPO, "engine", "iLEAPP", "admin", "data", "filepath-lists")

INCLUDE_KEYS = (
    "RelativePathsToBackupAndRestore",
    "RelativePathsToBackupToDriveAndStandardAccount",
    "RelativePathsToBackupIgnoringProtectionClass",
)
ENCRYPTED_ONLY_KEY = "RelativePathsToOnlyBackupEncrypted"
EXCLUDE_KEYS = ("RelativePathsNotToBackup", "RelativePathsNotToBackupToDrive")

APP_CONTAINER = re.compile(r"containers/(?:data/application|shared/appgroup)/[^/]+/(.*)$")

# Sources TraceLoupe parses out of real backups today, with the verdict the
# classifier must produce for each. Ground truth: we demonstrably read these, so
# `excluded` is always a bug. `unknown` is the honest answer for the two whose
# iLEAPP glob is a bare filename (`*/NoteStore.sqlite*`, `*/Calendar.sqlitedb`)
# with no directory context to resolve — but it is pinned here, so a matcher
# regression that degrades a resolved source to `unknown` fails too.
#
# This guard has already caught the classifier being wrong three times: six
# domains share the root `/var/mobile` and only one was being tested; rootless
# globs were reported `excluded` rather than `unknown`, which would have quietly
# deleted real work from the coverage list; and fragment globs that overlap a
# domain entry (`Reminders/Container_v1/...` under `Library/Reminders`) did not
# match at all.
KNOWN_REACHABLE = {
    "sms": ("Messages", "backup"),
    "callHistory": ("Call history", "backup"),
    "addressBook": ("Contacts", "backup"),
    "safariHistory": ("Safari history", "backup"),
    "safariBookmarks": ("Safari bookmarks", "backup"),
    "reminders": ("Reminders", "backup"),
    "voiceRecordings": ("Voice recordings", "backup"),
    "photosMetadata": ("Photos", "backup"),
    "interactionCcontacts": ("CoreDuet interactions", "encrypted-only"),
    "health": ("Health", "encrypted-only"),
    # These two were pinned `unknown` while a bare-filename glob had no
    # directory context. Resolving globs against the device path-lists gives
    # them one, so they now classify properly — Calendar under HomeDomain's
    # `Library/Calendar`, NoteStore inside its app group.
    "notes": ("Notes", "backup"),
    "calendarAll": ("Calendar", "backup"),
}


def parse_artifacts() -> list[dict]:
    """Pull name/category/paths out of each module's __artifacts_v2__ literal."""
    out: list[dict] = []
    for path in sorted(glob.glob(os.path.join(ILEAPP, "*.py"))):
        base = os.path.basename(path)
        src = open(path, encoding="utf-8", errors="replace").read()
        m = re.search(r"__artifacts_v2__\s*=\s*\{", src)
        if not m:
            out.append({"file": base, "status": "no-v2-block"})
            continue
        start = src.index("{", m.start())
        depth, end = 0, None
        for i in range(start, len(src)):
            if src[i] == "{":
                depth += 1
            elif src[i] == "}":
                depth -= 1
                if depth == 0:
                    end = i
                    break
        if end is None:
            out.append({"file": base, "status": "unbalanced-braces"})
            continue
        try:
            block = ast.literal_eval(src[start : end + 1])
        except Exception as exc:  # a module we cannot read is reported, not skipped
            out.append({"file": base, "status": f"parse-error: {exc!s:.60}"})
            continue
        for key, meta in block.items():
            if not isinstance(meta, dict):
                continue
            paths = meta.get("paths") or ()
            if isinstance(paths, str):
                paths = (paths,)
            out.append(
                {
                    "file": base, "key": key, "status": "ok",
                    "name": meta.get("name", ""), "category": meta.get("category", ""),
                    "paths": [str(p) for p in paths],
                }
            )
    return out


def normalise(glob_path: str) -> str:
    """An iLEAPP glob → a device path relative to /var, that domains can match."""
    p = glob_path.replace("\\", "/").lstrip("*")
    p = re.sub(r"^/+", "", p)
    p = re.sub(r"^filesystem\d*/", "", p)
    p = re.sub(r"^(private/)?var/", "", p)
    return p.lower()


def covered_by(rel: str, entries: list[str]) -> bool:
    """Could a domain entry and this glob name the same file?

    A domain entry names a file or a directory, and a directory covers its
    subtree. iLEAPP globs, though, are frequently *fragments* carrying no domain
    context — `**/Safari/History.db*`, `**/Reminders/Container_v1/Stores/*` — so
    a plain prefix test misses most of them. All four relationships below are
    checked, always on path-segment boundaries so `.../Safari` cannot match
    `.../SafariSomethingElse`.
    """
    stem = rel.rstrip("*").rstrip("/")
    if not stem:
        return False
    for e in entries:
        e = e.lower().rstrip("/")
        # 1. The glob sits at or under the entry.
        #    entry `Library/Notes`, glob `Library/Notes/notes.sqlite`
        if stem == e or stem.startswith(e + "/"):
            return True
        # 2. The glob is shallower than the entry.
        #    glob `Library/Safari/*`, entry `Library/Safari/History.db`
        if rel.endswith("*") and e.startswith(stem + "/"):
            return True
        # 3. The fragment overlaps the entry's tail, whether it stops there or
        #    continues deeper. Covers both `Safari/History.db` inside
        #    `Library/Safari/History.db`, and `Reminders/Container_v1/Stores/*`
        #    under `Library/Reminders`.
        #
        #    A separate "fragment is a suffix of the entry" rule used to sit
        #    here; mutation testing showed removing it changed nothing, because
        #    this loop already subsumes it at k == len(fs). Dead branches read
        #    exactly like live ones, so it is gone rather than kept "for
        #    clarity".
        es, fs = e.split("/"), stem.split("/")
        for k in range(1, min(len(es), len(fs)) + 1):
            if es[-k:] == fs[:k]:
                return True
    return False


def load_device_paths() -> dict[str, list[str]]:
    """Real device paths, indexed by basename, from the lists iLEAPP ships.

    These come from full-filesystem images, which is fine for this purpose and
    only this purpose: the image says *where a file lives*, and `Domains.plist`
    still decides whether that location is backed up. An FFS image can never be
    evidence that something is in a backup — it contains everything, including
    what backups exclude.
    """
    by_base: dict[str, list[str]] = collections.defaultdict(list)
    if not os.path.isdir(PATH_LISTS):
        return by_base
    for name in sorted(os.listdir(PATH_LISTS)):
        if not name.endswith(".zip"):
            continue
        try:
            z = zipfile.ZipFile(os.path.join(PATH_LISTS, name))
        except Exception:
            continue
        for inner in z.namelist():
            with z.open(inner) as fh:
                for i, line in enumerate(io.TextIOWrapper(fh, encoding="utf-8", errors="replace")):
                    if i == 0 and line.startswith("path"):
                        continue
                    path = line.split(",", 1)[0].strip().lower()
                    if path:
                        by_base[path.rsplit("/", 1)[-1]].append(path)
    return by_base


def resolve_on_device(globs: list[str], by_base: dict[str, list[str]]) -> str | None:
    """Find a real device path one of these globs matches, or None."""
    for g in globs:
        pat = re.sub(r"^/+", "", g.replace("\\", "/").lower().lstrip("*"))
        base_pat = pat.rsplit("/", 1)[-1]
        want = pat if pat.startswith("*") else "*" + pat
        for base, candidates in by_base.items():
            if not fnmatch.fnmatch(base, base_pat):
                continue
            for c in candidates:
                if fnmatch.fnmatch(c, want) or pat in c:
                    return c
    return None


def classify_path(p: str, domains: dict) -> tuple[str, str]:
    """(verdict, why) for one path: backup | encrypted-only | excluded | unknown."""
    rel_full = normalise(p)

    m = APP_CONTAINER.search(rel_full)
    if m:
        inner = m.group(1)
        if inner.startswith("library/caches/") or inner.startswith("tmp/"):
            return "excluded", "AppDomain: Library/Caches and tmp are excluded"
        return "backup", "AppDomain (app Documents/Library)"

    # Six domains share the root /var/mobile (Home, Media, CameraRoll, Keyboard,
    # Tones, HomeKit), so a path must be tested against EVERY domain whose root
    # matches — not the longest or the first. Rootless fragments are also tried
    # as already-domain-relative.
    rooted_candidates: list[tuple[str, str]] = []
    for name, d in domains.items():
        root = (d.get("root") or "").lower()
        root = re.sub(r"^/?(private/)?var/", "", root).rstrip("/")
        if root and (rel_full == root or rel_full.startswith(root + "/")):
            rooted_candidates.append((name, rel_full[len(root):].lstrip("/")))

    # A path that sits under a known domain root is NOT a fragment, and must be
    # judged only against the domains that actually own that root. Applying the
    # fragment fallback to it as well is how every Biome path came back
    # "backup": `mobile/library/biome/...` matched ManagedPreferencesDomain's
    # single-segment entry `mobile`, a domain rooted at /var/Managed Preferences
    # that has nothing to do with it.
    rooted = bool(rooted_candidates)
    candidates = rooted_candidates or [(name, rel_full) for name in domains]

    encrypted, excluded = None, None
    for name, rel in candidates:
        keys = domains[name]["keys"]
        if any(covered_by(rel, keys.get(k, [])) for k in EXCLUDE_KEYS):
            excluded = excluded or f"{name}.excluded"
            continue
        for k in INCLUDE_KEYS:
            if covered_by(rel, keys.get(k, [])):
                return "backup", f"{name}.{k}"
        if covered_by(rel, keys.get(ENCRYPTED_ONLY_KEY, [])):
            encrypted = encrypted or f"{name}.{ENCRYPTED_ONLY_KEY}"
    if encrypted:
        return "encrypted-only", encrypted
    if excluded:
        return "excluded", excluded
    if rooted:
        # Sits under a domain root and is on none of its lists. Every domain is
        # an allowlist, so that is a real answer, not a gap.
        return "excluded", "under a domain root but on no include list"
    # A rootless fragment that matched nothing. We cannot tell which domain it
    # would land in, and guessing "excluded" would silently delete real work.
    return "unknown", "rootless glob; no domain context to resolve it"


def classify(paths: list[str], domains: dict) -> tuple[str, list[str]]:
    verdicts = [classify_path(p, domains) for p in paths]
    kinds = {v for v, _ in verdicts}
    # Any reachable path makes the artifact reachable — modules routinely list a
    # store plus its -wal/-shm siblings, or several OS-version variants.
    for want in ("backup", "encrypted-only", "unknown"):
        if want in kinds:
            return want, sorted({w for v, w in verdicts if v == want})
    return "excluded", sorted({w for _, w in verdicts})


# The other half of the guard: things a backup provably cannot contain. Without
# this, a matcher that says "yes" too easily looks like a matcher that got
# better — the counts go up and everything reads like progress.
#
# It earned its place immediately. Resolving rootless globs to real device paths
# made every Biome artifact classify as *backup*, because
# `mobile/library/biome/...` matched ManagedPreferencesDomain's single-segment
# entry `mobile` — a domain rooted at /var/Managed Preferences with nothing to
# do with Biome. Only a negative expectation catches that shape of error.
KNOWN_UNREACHABLE = {
    "Biome": "Biome — Apple excludes it from backups entirely",
    "KnowledgeC": "knowledgeC — never in a backup",
}


def self_test(ok: list[dict]) -> int:
    by = {a["file"].removesuffix(".py"): a for a in ok}
    failures, missing = [], []
    for module, (label, expected) in sorted(KNOWN_REACHABLE.items()):
        a = by.get(module)
        if a is None:
            missing.append(f"{module} ({label})")
            continue
        got = a["reach"]
        if got == expected:
            print(f"  ok    {label:22} {got}")
        else:
            print(f"  FAIL  {label:22} {got}  (expected {expected})")
            why = a["why"][0] if a["why"] else "?"
            failures.append(f"{label}: expected {expected}, got {got} — {why}")
    for m in missing:
        print(f"  WARN  {m}: no such iLEAPP module — renamed upstream?")

    for category, why in sorted(KNOWN_UNREACHABLE.items()):
        rows = [a for a in ok if a["category"] == category]
        if not rows:
            print(f"  WARN  {category}: no artifacts found — renamed upstream?")
            continue
        wrong = [a for a in rows if a["reach"] in ("backup", "encrypted-only")]
        if wrong:
            print(f"  FAIL  {category:22} {len(wrong)}/{len(rows)} marked reachable")
            failures.append(
                f"{category}: {len(wrong)} of {len(rows)} classified reachable — {why}. "
                f"First: {wrong[0]['file']} via {wrong[0]['why'][0] if wrong[0]['why'] else '?'}"
            )
        else:
            print(f"  ok    {category:22} all {len(rows)} unreachable")

    if failures:
        print(f"\n{len(failures)} known source(s) classified wrongly:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(
        f"\nself-test ok — {len(KNOWN_REACHABLE)} known sources classify as expected, "
        f"and {len(KNOWN_UNREACHABLE)} known-unreachable categories stayed unreachable"
    )
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Classify iLEAPP artifacts by backup reachability")
    ap.add_argument("--json", metavar="FILE", help="write the full per-artifact table")
    ap.add_argument("--self-test", action="store_true",
                    help="assert the sources we already parse classify as expected")
    args = ap.parse_args()

    if not os.path.isdir(ILEAPP):
        print(f"ERROR: no iLEAPP checkout at {ILEAPP} — run `pnpm setup:engine`", file=sys.stderr)
        return 1
    if not os.path.isfile(DOMAINS_JSON):
        print(f"ERROR: missing domain rules at {DOMAINS_JSON}", file=sys.stderr)
        return 1
    domains = json.load(open(DOMAINS_JSON))

    arts = parse_artifacts()
    broken = [a for a in arts if a["status"] != "ok"]
    ok = [a for a in arts if a["status"] == "ok"]
    for a in ok:
        a["reach"], a["why"] = classify(a["paths"], domains)

    # Second pass. A rootless glob (`**/interactionC.db*`) carries no directory,
    # so the domain rules cannot place it — iLEAPP writes them that way because
    # it searches a filesystem, while backups are addressed by (domain, path).
    # The device path-lists iLEAPP ships supply the missing directory; the
    # domain rules then decide reachability exactly as before.
    resolved = 0
    by_base = load_device_paths()
    if by_base:
        for a in ok:
            if a["reach"] != "unknown":
                continue
            hit = resolve_on_device(a["paths"], by_base)
            if not hit:
                continue
            verdict, why = classify_path(hit, domains)
            if verdict == "unknown":
                continue
            a["reach"], a["why"] = verdict, [f"{why} (via device path {hit})"]
            a["resolved_path"] = hit
            resolved += 1

    if args.json:
        json.dump(ok + broken, open(args.json, "w"), indent=1)

    if args.self_test:
        return self_test(ok)

    counts = collections.Counter(a["reach"] for a in ok)
    print(f"iLEAPP artifacts parsed: {len(ok)}" + (f"  (unreadable: {len(broken)})" if broken else ""))
    if by_base:
        print(f"  ({len(by_base):,} device basenames loaded; {resolved} rootless globs resolved)")
    else:
        print("  (no device path-lists found — rootless globs left unresolved)")
    for reach in ("backup", "encrypted-only", "excluded", "unknown"):
        print(f"  {counts[reach]:4}  {reach}")
    for b in broken:
        print(f"  !! {b['file']}: {b['status']}")

    print("\nreachable artifacts by category (top 25):")
    bycat: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    for a in ok:
        bycat[a["category"]][a["reach"]] += 1
    rows = sorted(bycat.items(), key=lambda t: -(t[1]["backup"] + t[1]["encrypted-only"]))
    for cat, c in rows[:25]:
        if c["backup"] or c["encrypted-only"]:
            enc = f"  (+{c['encrypted-only']} encrypted-only)" if c["encrypted-only"] else ""
            print(f"  {c['backup']:4}  {cat}{enc}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
