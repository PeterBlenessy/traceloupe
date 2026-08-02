#!/usr/bin/env python3
"""Which declarative modules are IMPLEMENTED, and which have been VERIFIED.

These are different things and the audit used to conflate them.

A module can be written from iLEAPP's definition, load, run, and pass its
fixture while reading the wrong key — the fixture was written from the same
reading of the store, so it agrees with the module by construction. Only a real
device settles it. Both states are worth shipping; only one of them is proof.

  IMPLEMENTED  the module exists, loads, runs, and produced rows from a fixture
  VERIFIED     its output has been checked against a named real backup

The verified note lives in each module's own `verified` field, so this table is
read from the modules and cannot drift from them. A module with no `verified`
field is implemented and unverified, which is reported rather than hidden.

    python3 tools/module-status.py            # the table, as Markdown
    python3 tools/module-status.py --summary  # just the counts
"""
import re
import sys
from pathlib import Path

MODULES = Path(__file__).resolve().parent.parent / "crates/traceloupe-core/modules"


def scalar(text: str, key: str) -> str | None:
    m = re.search(rf'^{key}\s*=\s*"(.*)"\s*$', text, re.M)
    return m.group(1) if m else None


def main() -> int:
    rows = []
    for f in sorted(MODULES.glob("*.toml")):
        t = f.read_text()
        rows.append(
            (
                scalar(t, "name") or f.stem,
                scalar(t, "category") or "—",
                scalar(t, "domain") or "—",
                scalar(t, "verified"),
            )
        )
    if not rows:
        print("no modules found — did the directory move?", file=sys.stderr)
        return 1

    verified = [r for r in rows if r[3]]
    if "--summary" in sys.argv:
        print(f"{len(rows)} modules implemented, {len(verified)} verified "
              f"against a real backup, {len(rows) - len(verified)} not yet")
        return 0

    print(f"| Module | Category | Implemented | Verified against |")
    print(f"|---|---|:-:|---|")
    for name, category, _domain, v in rows:
        print(f"| {name} | {category} | ✅ | {v or '— not yet —'} |")
    print()
    print(f"**{len(rows)} implemented · {len(verified)} verified · "
          f"{len(rows) - len(verified)} awaiting a real backup.**")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
