#!/usr/bin/env python3
"""Which iLEAPP artifacts are actually reachable in a REAL backup, and which
still carry data on modern iOS.

This is the tool behind the measurements posted to #192. It answers the question
that issue asks — "which of the unclassified artifacts are actually in a backup?"
— by enumerating a real backup's Manifest and testing every iLEAPP path glob
against it, rather than reasoning about path shape.

    # 1. dump every path in a real backup (never the owner's — see AGENTS.md)
    cargo run -p traceloupe-core --example explore_real_backup -- \\
        <backup-dir> <password> list '%' > backup-paths.txt

    # 2. get iLEAPP's artifact definitions
    curl -sL https://github.com/abrignoni/iLEAPP/archive/refs/heads/main.tar.gz \\
        | tar xz && mv iLEAPP-main ileapp-src

    # 3. measure
    python3 tools/ileapp-reachability.py backup-paths.txt ileapp-src

TWO TRAPS THIS TOOL EXISTS TO NOT FALL INTO AGAIN
-------------------------------------------------

1. iLEAPP declares `paths` as a BARE STRING for many artifacts. `list()` on a
   string splits it into characters, so the first "glob" becomes `*`, which
   matches every path in the backup. The first run of this analysis reported 194
   reachable artifacts; the truth was 79. It was caught only because one result
   contradicted a hand check. Hence `_globs()`.

2. iLEAPP globs are FULL-FILESYSTEM paths, while a backup's `relativePath` has
   no `/private/var/mobile` prefix. Matching therefore anchors at the END and
   allows any prefix. A naive `fnmatch` reports almost nothing.

WHAT "ABSENT" DOES AND DOES NOT MEAN
------------------------------------

Absent here means absent from THIS device's backup, which is a lower bound on
backup-reachability, not a verdict on it. An artifact can be missing because the
feature was never used rather than because iOS excludes it, and one device
cannot tell those apart. A result is only conclusive when the DOMAIN settles it
(Biome, Safari favicons), or when a second device agrees.
"""

from __future__ import annotations

import ast
import collections
import json
import pathlib
import re
import sys

# iLEAPP annotates each artifact with row counts per test image. These are the
# image keys whose names mark them as iOS 18 or 26 — "still has data on modern
# iOS" is a different question from "reachable", and both matter.
MODERN = re.compile(r"ios(18|26)", re.I)


def load_backup_paths(path: pathlib.Path) -> list[str]:
    """The `relativePath`s out of `explore_real_backup … list '%'` output."""
    out = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or " path(s)" in line:
            continue
        parts = line.split(None, 1)
        if len(parts) == 2 and "/" in parts[1]:
            out.append(parts[1].strip())
    return sorted(set(out))


def glob_to_re(glob: str) -> re.Pattern[str]:
    """An iLEAPP glob as a regex anchored at the end of a backup relativePath."""
    out, i = [], 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif glob[i] == "*":
            out.append("[^/]*")
            i += 1
        else:
            out.append(re.escape(glob[i]))
            i += 1
    return re.compile(".*" + "".join(out) + "$", re.I)


def _globs(raw) -> list[str]:
    """Normalise `paths`, which is a bare string for many artifacts.

    See trap 1 in the module docstring: getting this wrong silently turns one
    artifact into a wildcard that matches the whole backup.
    """
    if isinstance(raw, str):
        return [raw]
    return list(raw or ())


def artifacts(src: pathlib.Path):
    for f in sorted((src / "scripts" / "artifacts").glob("*.py")):
        m = re.search(r"__artifacts_v2__\s*=\s*(\{.*?\n\})\s*\n", f.read_text(errors="replace"), re.S)
        if not m:
            continue
        try:
            meta = ast.literal_eval(m.group(1))
        except Exception:
            continue  # a definition we cannot read is not a definition we can judge
        for key, a in meta.items():
            if isinstance(a, dict):
                yield key, a


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    paths = load_backup_paths(pathlib.Path(sys.argv[1]))
    src = pathlib.Path(sys.argv[2])
    if not paths:
        print("no backup paths parsed — is that the output of `list '%'`?")
        return 1

    rows = []
    for key, a in artifacts(src):
        globs = _globs(a.get("paths"))
        sample = a.get("sample_data", {}) or {}
        modern = {k: v for k, v in sample.items() if MODERN.search(k)}
        n = sum(
            int(m.group(1))
            for v in modern.values()
            if (m := re.search(r"(\d+)\s+rows?", str(v)))
        )
        reachable = any(
            rx.match(p) for g in globs for rx in [glob_to_re(g)] for p in paths
        )
        rows.append(
            {
                "key": key,
                "name": a.get("name", key),
                "category": a.get("category", "?"),
                "modern_rows": n,
                "paths": globs,
                "reachable": reachable,
            }
        )

    rows.sort(key=lambda r: (-r["modern_rows"], r["name"]))
    pathlib.Path("ileapp-reachability.json").write_text(json.dumps(rows, indent=1))

    reach = [r for r in rows if r["reachable"]]
    live = [r for r in reach if r["modern_rows"] > 0]
    print(f"artifact definitions parsed:   {len(rows)}")
    print(f"  present in this backup:      {len(reach)}")
    print(f"  ...with data on iOS 18/26:   {len(live)}")
    print(f"  absent despite modern data:  {len([r for r in rows if not r['reachable'] and r['modern_rows'] > 0])}")
    print("\nlive + reachable, by category:")
    for cat, k in collections.Counter(r["category"] for r in live).most_common():
        print(f"  {k:4}  {cat}")
    print("\nfull detail written to ileapp-reachability.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
