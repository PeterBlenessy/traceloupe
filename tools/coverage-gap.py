#!/usr/bin/env python3
"""What iLEAPP reads from a backup that TraceLoupe does not.

`classify-ileapp-artifacts.py` answers "which of iLEAPP's artifacts are even in a
backup". This answers the next question: of those, which do we already cover, and
what is left. Together they turn "full iLEAPP parity" from a slogan into a number
that goes down.

Coverage is read from the source, never from a list kept beside it, so a parser
added tomorrow is counted without anyone remembering this file:

  * app chat modules   — `APP_CHAT_MODULES` in parsers/apps/mod.rs
  * declarative modules — the `name`/`category` of every modules/*.toml
  * native parsers     — the files in parsers/ and the app catalog's `native`
                          entries in src/lib/apps.ts

GRANULARITY IS THE HONEST PART. iLEAPP groups artifacts into product categories,
and a category is rarely all-or-nothing: "Clock" is Alarms, Stopwatch, Timers and
WorldClock, and we read one of the four. So this reports a category as TOUCHED —
we read something in it — and never as finished. Anything stronger would be a
claim this tool cannot support, and "we support Clock" is exactly the sort of
thing that stops someone looking.

Matching is by name, case- and punctuation-insensitively, plus an explicit alias
table for the cases where iLEAPP's product name and ours differ ("Facebook
Messenger" vs our "Messenger"). Aliases are listed with the artifact that
justifies them, because an alias is a claim of coverage and an unjustified one
overstates what we do.

  python3 tools/coverage-gap.py                 # summary + the biggest gaps
  python3 tools/coverage-gap.py --json out.json # the full table
  python3 tools/coverage-gap.py --all           # every gap, not just the top

Feed it the classifier's output first:

  python3 tools/classify-ileapp-artifacts.py --json /tmp/ileapp.json
"""

from __future__ import annotations

import argparse
import collections
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def norm(s: str) -> str:
    """Product names, comparable. 'Facebook Messenger' == 'facebook_messenger'."""
    return re.sub(r"[^a-z0-9]", "", s.lower())


def ours() -> dict[str, str]:
    """Everything we cover, as normalised name → how we cover it."""
    covered: dict[str, str] = {}

    # App chat modules: the `service` label is the product name.
    mod = (ROOT / "crates/traceloupe-core/src/parsers/apps/mod.rs").read_text()
    listed = re.search(r"APP_CHAT_MODULES[^=]*=\s*&\[(.*?)\];", mod, re.S)
    ids = re.findall(r"(\w+)::MODULE", listed.group(1)) if listed else []
    for i in ids:
        src = ROOT / f"crates/traceloupe-core/src/parsers/apps/{i}.rs"
        if src.exists():
            m = re.search(r'service:\s*"([^"]+)"', src.read_text())
            if m:
                covered[norm(m.group(1))] = f"app chat parser ({i})"
    # TikTok is driven separately (two DBs), so it is not in the list above.
    if (ROOT / "crates/traceloupe-core/src/parsers/apps/tiktok.rs").exists():
        covered.setdefault(norm("TikTok"), "app chat parser (tiktok, driven directly)")

    # Declarative artifact modules. Only the header — everything after the first
    # `[[columns]]` is column names, and matching on those claimed we covered
    # "Address", "Status" and "Label".
    for toml in sorted((ROOT / "crates/traceloupe-core/modules").glob("*.toml")):
        header = toml.read_text().split("[[columns]]")[0]
        name = re.search(r'^name\s*=\s*"([^"]+)"', header, re.M)
        cat = re.search(r'^category\s*=\s*"([^"]+)"', header, re.M)
        for value in (name, cat):
            if value:
                covered.setdefault(norm(value.group(1)), f"module ({toml.stem})")

    # Native parsers, by file name — these are the big first-party stores.
    for src in sorted((ROOT / "crates/traceloupe-core/src/parsers").glob("*.rs")):
        if src.stem in {"mod"}:
            continue
        covered.setdefault(norm(src.stem), f"native parser ({src.stem})")

    # The app catalog's own view of what we support natively.
    cat_src = (ROOT / "src/lib/apps.ts").read_text()
    for bundle, body in re.findall(r'"([\w.\-]+)":\s*\{([^}]*)\}', cat_src):
        name = re.search(r'name:\s*"([^"]+)"', body)
        support = re.search(r'support:\s*"(\w+)"', body)
        if name and support and support.group(1) == "native":
            covered.setdefault(norm(name.group(1)), "app catalog (native)")

    return covered


# iLEAPP category → what of ours reads part of it, when the names differ. Each is
# justified by the artifact that makes it true; without that a line here is just
# an assertion that we cover something.
ALIASES = {
    # iLEAPP category: (our thing, the artifact that justifies the claim)
    "facebookmessenger": ("app chat parser (facebook_messenger)", "Facebook Messenger - Chats"),
    "clock": ("module (alarms, sleep_schedule)", "Alarms — NOT Stopwatch, Timers or WorldClock"),
    "wificonnections": ("module (wifi_networks, wifi_private_mac)", "WiFi Known Networks Info / Times / Scanned (Private) — NOT BSS List"),
    "location": ("module (location_clients)", "LSC - clients.plist — NOT locationd.plist, routined, Maps Sync, Weather"),
    "identifiers": ("module (sim_cards) + the device header", "Subscriber Info, Serial Number — NOT AirDrop ID, IMEI, Find My, Timezone, Backup Settings"),
    "bluetooth": ("module (bluetooth_paired, bluetooth_devices, bluetooth_nearby)", "Bluetooth Paired LE / Paired / Other LE"),
    "appusage": ("module (data_usage)", "network usage per app — NOT foreground/screen time"),
    # Our parser's service label is "imo"; iLEAPP calls the product "IMO HD Chat".
    "imohdchat": ("app chat parser (imo)", "IMO HD Chat - Messages — NOT Contacts"),
    "networkusage": ("module (data_usage)", "Data Usage"),
    "apppermissions": ("module (tcc)", "Application Permissions"),
    # sim_cards reads CellularUsage.db's subscriber_info, which is where the SIM's
    # ICCID and number live. iLEAPP splits the same store across two categories.
    "siminfo": ("module (sim_cards)", "SIM - UUID — NOT the Unique Label Store"),
    "cellular": ("module (sim_cards, data_usage)", "Cellular Wireless — partially"),
}

# iLEAPP categories that are not products we could "support" — they name a store
# we already read in full, or a facet of one. Listed with the reason, because an
# exclusion nobody can argue with is an exclusion nobody checks.
NOT_A_GAP = {
    "photos": "Photos.sqlite — parsed natively; the -A-..-S- categories are facets of it",
    "health": "healthdb_secure.sqlite — parsed natively",
    "smsimessage": "sms.db — parsed natively",
    "callhistory": "CallHistory.storedata — parsed natively",
    "contacts": "AddressBook — parsed natively",
    "calendar": "Calendar.sqlitedb — parsed natively",
    "safaribrowser": "Safari History/Bookmarks/Tabs — parsed natively",
    "notes": "NoteStore.sqlite — parsed natively",
    "installedapps": "the Apps view lists these from the backup itself",
    "keychain": "encrypted-backup keychain — handled by the decryptor, not an artifact",
}


def is_photos_facet(cat: str) -> bool:
    return cat.lower().startswith(("photos.sqlite", "photos-"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--input", default="/tmp/ileapp.json", help="classify-ileapp-artifacts.py --json output")
    ap.add_argument("--json", metavar="FILE", help="write the full table")
    ap.add_argument("--all", action="store_true", help="every gap, not just the biggest")
    args = ap.parse_args()

    path = Path(args.input)
    if not path.exists():
        print(f"missing {path} — run:\n  python3 tools/classify-ileapp-artifacts.py --json {path}")
        return 2
    artifacts = json.load(open(path))
    reachable = [a for a in artifacts if a.get("reach") in ("backup", "encrypted-only")]

    covered = ours()
    by_cat: dict[str, list] = collections.defaultdict(list)
    for a in reachable:
        by_cat[a.get("category") or "?"].append(a)

    gaps, done, excused = [], [], []
    for cat, items in by_cat.items():
        n = norm(cat)
        if is_photos_facet(cat):
            excused.append((cat, len(items), NOT_A_GAP["photos"]))
        elif n in NOT_A_GAP:
            excused.append((cat, len(items), NOT_A_GAP[n]))
        elif n in ALIASES:
            how, why = ALIASES[n]
            done.append((cat, len(items), f"{how} — {why}"))
        elif n in covered:
            done.append((cat, len(items), covered[n]))
        else:
            gaps.append((cat, len(items)))

    gaps.sort(key=lambda x: (-x[1], x[0]))
    done.sort(key=lambda x: (-x[1], x[0]))
    total_gap = sum(n for _, n in gaps)

    print(f"iLEAPP artifacts that a backup can contain: {len(reachable)}")
    print(f"  {sum(n for _, n, _ in done):4} in {len(done)} categories we TOUCH (some read, not necessarily all)")
    print(f"  {sum(n for _, n, _ in excused):4} in {len(excused)} categories that are not gaps (see below)")
    print(f"  {total_gap:4} in {len(gaps)} categories NOT touched at all\n")

    print("── touched: we read SOMETHING here. Not a claim that we read it all ──")
    for cat, n, how in done:
        print(f"  {n:3}  {cat:28} {how}")

    print("\n── not a gap ──")
    seen = set()
    for cat, n, why in excused:
        if why not in seen:
            seen.add(why)
            print(f"       {why}")

    print(f"\n── untouched: {total_gap} artifacts in {len(gaps)} categories we read nothing from ──")
    shown = gaps if args.all else gaps[:30]
    for cat, n in shown:
        print(f"  {n:3}  {cat}")
    if len(shown) < len(gaps):
        print(f"  … and {len(gaps) - len(shown)} more categories ({sum(n for _, n in gaps[len(shown):])} artifacts) — pass --all")

    if args.json:
        Path(args.json).write_text(
            json.dumps(
                {
                    "reachable": len(reachable),
                    "covered": [{"category": c, "artifacts": n, "how": h} for c, n, h in done],
                    "not_a_gap": [{"category": c, "artifacts": n, "why": w} for c, n, w in excused],
                    "gap": [{"category": c, "artifacts": n} for c, n in gaps],
                },
                indent=1,
            )
        )
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
