#!/usr/bin/env python3
"""Classify every iLEAPP artifact by whether an iTunes/Finder backup can reach it.

This is the machine half of `docs/reference/backup-coverage-audit.md`. The doc
records the conclusions; this script is how they were reached, so a later
session can re-run it against a newer iLEAPP checkout instead of trusting a
frozen table.

    pnpm setup:engine                       # once — clones iLEAPP into engine/
    python3 tools/classify-ileapp-artifacts.py            # summary
    python3 tools/classify-ileapp-artifacts.py --json out.json

**The rule, stated so it can be argued with.** An iOS backup stores files keyed
by *domain*: HomeDomain (`/var/mobile`, minus exclusions), MediaDomain and
CameraRollDomain, AppDomain*/AppDomainGroup* (an app's Documents + Library,
minus `Library/Caches` and `tmp`), KeychainDomain, WirelessDomain, and a handful
of system domains. Everything else on the device needs a full-filesystem
extraction (GrayKey/checkm8) and is permanently out of reach for a tool that
reads backups.

**Its known weakness.** Membership is a property of the domain, not of the path,
and the exclusions are invisible in a glob. `Library/Biome/` and
`Library/CoreDuet/Knowledge/` sit under the same `mobile/Library` prefix as
artifacts that *are* backed up — while `Library/CoreDuet/People/interactionC.db`
right beside them is backed up, and we already parse it. So the FFS_ONLY list
below is an explicit deny-list of known exclusions, and anything the rule cannot
place comes back `unknown` rather than being guessed at. Resolving those needs a
real backup Manifest — see the audit doc.
"""
from __future__ import annotations

import argparse
import ast
import collections
import glob
import json
import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ILEAPP = os.path.join(REPO, "engine", "iLEAPP", "scripts", "artifacts")

# Paths a backup never contains. Order matters only for the reason reported.
FFS_ONLY: list[tuple[str, str]] = [
    (r"biome", "Biome — excluded from backup"),
    (r"knowledgec|coreduet/knowledge", "knowledgeC — excluded from backup"),
    (r"sysdiagnose", "sysdiagnose bundle"),
    (r"logarchive|unifiedlogs|\.logarchive|/logs?/", "unified/system logs"),
    (r"(private/)?var/db/", "/var/db — system, not backed up"),
    (r"(private/)?var/log", "/var/log — not backed up"),
    (r"(private/)?var/(installd|root|preferences)/", "system dir — not backed up"),
    (r"/caches/|/library/caches", "Library/Caches — excluded from backup"),
    (r"mobile_installation\.log", "install logs — not backed up"),
    (r"\.ips$", "crash reports — not backed up"),
]

# Paths a backup does contain, and the domain that carries them.
BACKUP_DOMAINS: list[tuple[str, str]] = [
    (r"mobile/containers/data/application", "AppDomain"),
    (r"mobile/containers/shared/appgroup", "AppDomainGroup"),
    (r"mobile/media|photodata|dcim", "CameraRoll/MediaDomain"),
    (r"keychain", "KeychainDomain (encrypted backups only)"),
    (r"(^|/)health", "HealthDomain (encrypted backups only)"),
    (r"wireless", "WirelessDomain"),
    (r"mobile/library|/library/", "HomeDomain"),
    (r"mobile/documents|/documents/", "HomeDomain/AppDomain"),
]


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
                    "file": base,
                    "key": key,
                    "status": "ok",
                    "name": meta.get("name", ""),
                    "category": meta.get("category", ""),
                    "paths": [str(p) for p in paths],
                }
            )
    return out


def classify(paths: list[str]) -> tuple[str, list[str]]:
    domains: set[str] = set()
    excluded: set[str] = set()
    unplaced = False
    for raw in paths:
        p = raw.lower()
        for rx, why in FFS_ONLY:
            if re.search(rx, p):
                excluded.add(why)
                break
        else:
            for rx, domain in BACKUP_DOMAINS:
                if re.search(rx, p):
                    domains.add(domain)
                    break
            else:
                unplaced = True
    if domains:
        return "backup", sorted(domains)
    if excluded and not unplaced:
        return "ffs-only", sorted(excluded)
    return "unknown", sorted(excluded)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--json", metavar="FILE", help="write the full per-artifact table")
    args = ap.parse_args()

    if not os.path.isdir(ILEAPP):
        print(f"ERROR: no iLEAPP checkout at {ILEAPP} — run `pnpm setup:engine`", file=sys.stderr)
        return 1

    arts = parse_artifacts()
    broken = [a for a in arts if a["status"] != "ok"]
    ok = [a for a in arts if a["status"] == "ok"]
    for a in ok:
        a["reach"], a["why"] = classify(a["paths"])

    if args.json:
        json.dump(ok + broken, open(args.json, "w"), indent=1)

    counts = collections.Counter(a["reach"] for a in ok)
    print(f"iLEAPP artifacts parsed: {len(ok)}" + (f"  (unreadable: {len(broken)})" if broken else ""))
    for reach in ("backup", "ffs-only", "unknown"):
        print(f"  {counts[reach]:4}  {reach}")
    for b in broken:
        print(f"  !! {b['file']}: {b['status']}")

    bycat: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    for a in ok:
        bycat[a["category"]][a["reach"]] += 1
    dead = sorted(c for c, v in bycat.items() if v["backup"] == 0 and v["unknown"] == 0)
    print(f"\ncategories with nothing backup-reachable ({len(dead)}):")
    for c in dead:
        print(f"  {c}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
