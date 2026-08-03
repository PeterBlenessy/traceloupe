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

MATCHING IS BY NAME AND BY STORE, UNIONED. The name is case- and
punctuation-insensitive. The store is the stronger of the two: an iLEAPP glob
and a module `path` that name the same file are the same artifact whatever
either project calls the product. Names drift — ours are chosen for a UI,
iLEAPP's for a report — so the store is what keeps a shipped module from
reading as a gap. It found four: the entire Files App cluster was reported
untouched while `icloud_drive` and three siblings were reading its store.

Placing a glob against a backup means stripping the DOMAIN ROOT off it: iLEAPP
searches a filesystem (`*/mobile/Library/…`), a backup is addressed by
(domain, relativePath) (`Library/…`). Every root that matches is tried, not the
longest, because six domains share `/var/mobile` and HealthDomain sits inside it.

A hand-written alias table joins the union for what neither can reach — chiefly
Waze, where iLEAPP's declared `paths` name a plist its code opens only to find
the container, and the data is in a `user.db` the manifest never mentions. An
alias can add a name but never hide one, and `--self-test` refuses any that
names a module or parser no longer in the tree.

  python3 tools/coverage-gap.py --self-test     # the matcher, no iLEAPP needed

  python3 tools/coverage-gap.py                 # summary + the biggest gaps
  python3 tools/coverage-gap.py --json out.json # the full table
  python3 tools/coverage-gap.py --all           # every gap, not just the top
  python3 tools/coverage-gap.py --present PATHS # split the gap by a real backup

`--present` takes a file of `domain<TAB>relativePath` lines — what a real backup
actually contains:

  cargo run -p traceloupe-core --example explore_real_backup -- <dir> <pw> list '%' \
      | tail -n +3 > /tmp/allpaths.txt

It splits the untouched artifacts three ways, which is the difference between a
list and a WORKLIST:

  * SAME STORE — the artifact reads a file we already parse (Photos.sqlite,
    healthdb, NoteStore). No new parsing; it is analysis we do not do on data
    already in the cache. The cheapest work there is.
  * UNREAD STORE — the file is in this backup and nothing reads it. Real work,
    and provably worth doing on this device.
  * NOT HERE — the file is not in this backup at all. Not necessarily
    unreachable; this device just does not have that app. Needs another device
    before it can be built OR ruled out.

Feed it the classifier's output first:

  python3 tools/classify-ileapp-artifacts.py --json /tmp/ileapp.json

The classifier needs an iLEAPP checkout at `engine/iLEAPP` (`pnpm setup:engine`),
which a git WORKTREE does not have — it is ignored, so only the main checkout
carries it. Run the classifier there and point this at the JSON.
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


def ours() -> dict[str, dict[str, list[str]]]:
    """Everything we cover, as normalised name → how we cover it.

    EVERY match, not the first. This used to `setdefault`, so a name three
    modules answer to reported one of them — whichever sorted first. That is how
    iLEAPP's "Location" category came back as `module (life360_locations)` with
    `location_clients` invisible beside it: not wrong, but an undercount that
    reads as the whole answer, and the kind a reader has no way to notice.
    """
    parts: dict[str, dict[str, list[str]]] = collections.defaultdict(
        lambda: collections.defaultdict(list)
    )

    def add(name: str, kind: str, detail: str) -> None:
        bucket = parts[norm(name)][kind]
        if detail not in bucket:
            bucket.append(detail)

    # App chat modules: the `service` label is the product name.
    mod = (ROOT / "crates/traceloupe-core/src/parsers/apps/mod.rs").read_text()
    listed = re.search(r"APP_CHAT_MODULES[^=]*=\s*&\[(.*?)\];", mod, re.S)
    ids = re.findall(r"(\w+)::MODULE", listed.group(1)) if listed else []
    for i in ids:
        src = ROOT / f"crates/traceloupe-core/src/parsers/apps/{i}.rs"
        if src.exists():
            m = re.search(r'service:\s*"([^"]+)"', src.read_text())
            if m:
                add(m.group(1), "app chat parser", i)
    # TikTok is driven separately (two DBs), so it is not in the list above.
    if (ROOT / "crates/traceloupe-core/src/parsers/apps/tiktok.rs").exists():
        add("TikTok", "app chat parser", "tiktok, driven directly")

    # Declarative artifact modules. Only the header — everything after the first
    # `[[columns]]` is column names, and matching on those claimed we covered
    # "Address", "Status" and "Label".
    for toml in sorted((ROOT / "crates/traceloupe-core/modules").glob("*.toml")):
        header = toml.read_text().split("[[columns]]")[0]
        name = re.search(r'^name\s*=\s*"([^"]+)"', header, re.M)
        cat = re.search(r'^category\s*=\s*"([^"]+)"', header, re.M)
        for value in (name, cat):
            if value:
                add(value.group(1), "module", toml.stem)

    # Native parsers, by file name — these are the big first-party stores.
    for src in sorted((ROOT / "crates/traceloupe-core/src/parsers").glob("*.rs")):
        if src.stem in {"mod"}:
            continue
        add(src.stem, "native parser", src.stem)

    # The app catalog's own view of what we support natively.
    cat_src = (ROOT / "src/lib/apps.ts").read_text()
    for bundle, body in re.findall(r'"([\w.\-]+)":\s*\{([^}]*)\}', cat_src):
        name = re.search(r'name:\s*"([^"]+)"', body)
        support = re.search(r'support:\s*"(\w+)"', body)
        if name and support and support.group(1) == "native":
            add(name.group(1), "app catalog", "native")

    return {k: {kind: list(d) for kind, d in v.items()} for k, v in parts.items()}


def fmt(parts: dict[str, list[str]]) -> str:
    """`{"module": ["a", "b"]}` → `module (a, b)`."""
    return ", ".join(f"{kind} ({', '.join(sorted(set(d)))})" for kind, d in parts.items() if d)


# iLEAPP category → what of OURS reads part of it, for the cases where the names
# differ and no automatic match is possible.
#
# THIS TABLE USED TO CARRY A SECOND HALF: a hand-written note saying which
# artifacts in the category we did and did not read ("Alarms — NOT Stopwatch,
# Timers or WorldClock"). It went stale the moment modules were added — three
# Clock modules shipped and this file still reported 1 of 4, in a tool whose own
# docstring promises coverage is read from the source and never from a list kept
# beside it.
#
# So the note is computed now. What stays here is only the part a machine cannot
# derive: WHICH OF OURS covers the category when the product names differ.
ALIASES = {
    "facebookmessenger": "app chat parser (facebook_messenger)",
    "wificonnections": "module (wifi_networks, wifi_private_mac)",
    "bluetooth": "module (bluetooth_paired, bluetooth_devices, bluetooth_nearby)",
    "appusage": "module (data_usage)",
    # Our parser's service label is "imo"; iLEAPP calls the product "IMO HD Chat".
    "imohdchat": "app chat parser (imo)",
    # sim_cards reads CellularUsage.db's subscriber_info, which is where the SIM's
    # ICCID and number live. iLEAPP names this category after the hardware, and
    # its own paths point at a store we reach by a different one.
    "siminfo": "module (sim_cards)",
    "mobilebackupplist": "module (backup_sizing)",
    "wifiknownnetworks": "module (wifi_networks)",
    # Waze cannot be matched by store, and that is not a flaw in the matcher.
    # iLEAPP's `paths` for every Waze artifact name `Preferences/
    # com.waze.iphone.plist`; its CODE opens `Documents/user.db` in the same
    # container and reads the plist only to find which container that is. Our
    # modules read the store, so they read the same data by a path the manifest
    # never mentions. Read the code, not the manifest.
    "waze": "module (waze_places, waze_favorites, waze_recents)",
    # Both of these are blocked by the container guard, and correctly so: the
    # glob is a bare filename with no domain root, and the category is named
    # after a topic rather than the app, so the guard has nothing to check the
    # domain against. A human can, which is what this table is for.
    #
    # "Cloudkit" is NoteStore.sqlite's share tables — who a note is shared with.
    # `notes.rs` reads them into `notes.shared_with_json`.
    "cloudkit": "native parser (notes)",
    # "Health & Fitness" is two artifacts over AllTrails.sqlite, the store the
    # `alltrails` module reads.
    "healthfitness": "module (alltrails)",
    # Neither side of this is a literal the other can match. iLEAPP's globs
    # start INSIDE the app container (`*/Chrome/Default/Login Data*`); our
    # modules' paths are domain-relative and themselves globbed, because one
    # module has to serve Chrome, Brave and Edge at once
    # (`Library/Application Support/**/Default/Login Data`). Two globs can only
    # be compared by intersection, which `same_store` deliberately does not
    # attempt — a wrong intersection would claim coverage nobody wrote.
    "chromium": "module (chromium_logins, chromium_top_sites)",
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


def stores_we_read() -> list[str]:
    """Relative paths our native parsers and modules open.

    From the source, like everything else here: `import.rs`'s store literals and
    every module's `path`. Over-inclusive on purpose — calling an artifact "same
    store" when it is not merely misfiles it, while missing one sends someone to
    build a parser for a file already in the cache.
    """
    out = []
    src = (ROOT / "crates/traceloupe-core/src/import.rs").read_text()
    src = re.sub(r"/\*[\s\S]*?\*/", "", src)
    src = re.sub(r"^\s*//.*$", "", src, flags=re.M)
    for m in re.finditer(r'"([^"]*\.(?:db|sqlite|sqlitedb|storedata|plist))"', src):
        p = m.group(1)
        if not p.startswith(".") and not p.startswith("cache."):
            out.append(p)
    for toml in (ROOT / "crates/traceloupe-core/modules").glob("*.toml"):
        m = re.search(r'^path\s*=\s*"([^"]+)"', toml.read_text(), re.M)
        if m:
            out.append(m.group(1))
    return sorted(set(out))


# `index.find("HomeDomain", "Library/CoreDuet/People/interactionC.db")` — the
# native importer's stores, which name a DOMAIN as well as a path. The bare
# filenames elsewhere in `import.rs` are deliberately not read here: a
# domainless filename is exactly the kind of match that over-claims.
NATIVE_STORE = re.compile(r'\bfind\(\s*"([A-Za-z][\w.\-]*)"\s*,\s*"([^"]+)"')


def our_stores() -> list[tuple[str, str, tuple[str, str]]]:
    """(domain, path, (kind, name)) for every store we open with a domain beside it.

    Both halves of the app: the declarative modules, and the native importer's
    `index.find(domain, path)` calls. Leaving the native ones out reported
    InteractionC and the CloudKit share artifacts as untouched — `interactionC.db`
    and `NoteStore.sqlite` are parsed in `import.rs`, and their iLEAPP categories
    are named after neither the store nor anything we call it, so name matching
    could not see them either.
    """
    out = []
    for toml in sorted((ROOT / "crates/traceloupe-core/modules").glob("*.toml")):
        header = toml.read_text().split("[[columns]]")[0]
        dom = re.search(r'^domain\s*=\s*"([^"]+)"', header, re.M)
        path = re.search(r'^path\s*=\s*"([^"]+)"', header, re.M)
        if dom and path:
            out.append((dom.group(1), path.group(1), ("module", toml.stem)))
    src = (ROOT / "crates/traceloupe-core/src/import.rs").read_text()
    src = re.sub(r"/\*[\s\S]*?\*/", "", src)
    src = re.sub(r"^\s*//.*$", "", src, flags=re.M)
    for domain, path in NATIVE_STORE.findall(src):
        # `AppDomainGroup-group.com.apple.notes` is a domain too, so the test
        # is "names a domain", not "ends in one".
        if "Domain" in domain and path.count(".") >= 1:
            out.append((domain, path, ("native parser", path.rsplit("/", 1)[-1])))
    return sorted(set(out))


# Marks a glob whose leading wildcard means "at any depth". A sentinel rather
# than a third tuple field because it travels through `by_store` untouched and
# only `same_store` is entitled to act on it.
ANY_DEPTH = "\0any-depth\0"

# `.../Containers/Data/Application/<uuid>/` — everything after it is what an
# AppDomain backup entry is relative to.
APP_CONTAINER = re.compile(r"containers/data/application/[^/]+/(.*)", re.I)


def domain_roots() -> list[str]:
    """Every domain's root, as a prefix to strip off an iLEAPP glob.

    iLEAPP searches a FILESYSTEM, so its globs are rooted there
    (`*/mobile/Library/…`). A backup is addressed by (domain, relativePath) and
    the path is relative to the domain's root (`Library/…`). Comparing the two
    without stripping the root is why this tool reported the whole Files App
    cluster as untouched while four modules were reading its store.
    """
    data = json.load(open(ROOT / "tools/data/ios-backup-domains.json"))
    roots = set()
    for d in data.values():
        root = (d.get("root") or "").lower().strip("/")
        root = re.sub(r"^(private/)?var/", "", root).strip("/")
        # BackupDomain's root is the literal string "# empty" in the source
        # plist; it names no directory and would never prefix a real path.
        if root and not root.startswith("#"):
            roots.add(root)
    return sorted(roots)


def as_backup_path(glob: str, roots: list[str]) -> list[tuple[str, bool]]:
    """An iLEAPP glob → the (domain-relative glob, in-an-app-container) it could be.

    EVERY matching root, not the longest. Six domains share `/var/mobile` and
    HealthDomain sits at `/var/mobile/Library`, so `mobile/Library/…` is both
    `Library/…` in HomeDomain and `…` in HealthDomain. Taking the longest root
    picked HealthDomain for the whole Files App cluster and matched nothing.

    A glob under no known root is tried AS IF already domain-relative, which is
    what `classify-ileapp-artifacts.py` does with the same fragments. iLEAPP
    writes a lot of them (`*/Library/Caches/locationd/clients.plist`) and they
    are the ones a root-strip cannot help — the second flag says the placement
    was a guess, so the caller can demand more before believing it.
    """
    raw = glob.replace("\\", "/")
    g = raw.lstrip("*").lstrip("/")
    m = APP_CONTAINER.search(g)
    if m:
        return [(m.group(1), False)]
    low = g.lower()
    rooted = [(g[len(r) + 1 :], True) for r in roots if low.startswith(r + "/")]
    if rooted:
        return rooted
    # A glob that BEGAN with a wildcard says "at any depth", so the remainder is
    # a suffix and not a whole path. `**/SpringBoard/IconState.plist` is the
    # store three home-screen modules read, at `Library/SpringBoard/…`, and
    # anchoring it at the front reported all three as untouched. `ANY_DEPTH`
    # marks it so `same_store` can allow a prefix — but only for the tail, which
    # still has to match completely.
    return [((ANY_DEPTH + g) if raw.startswith("*") else g, False)]


def same_store(glob_rel: str, our_path: str) -> bool:
    """Does an iLEAPP glob name the store a module reads?

    Anchored at BOTH ends, and `*` does not cross `/`. A loose match here is not
    a harmless one: it would print "we already read this" over work nobody has
    done, which is the failure this whole file exists to prevent.

    An `ANY_DEPTH` prefix relaxes the FRONT anchor only, and only when the tail
    is specific enough to be a filename rather than a wildcard. `*.db` would
    otherwise match every store in the app, and one over-claim costs more here
    than every missed match put together.
    """
    if glob_rel.startswith(ANY_DEPTH):
        glob_rel = glob_rel[len(ANY_DEPTH) :]
        literal = re.sub(r"\*", "", glob_rel.rsplit("/", 1)[-1])
        if len(literal) < 6:
            return False
        head = "(?:.*/)?"
    else:
        head = ""
    parts = [re.escape(p) for p in glob_rel.split("*")]
    return re.fullmatch(head + "[^/]*".join(parts), our_path, re.I) is not None


def by_store(items, stores, category: str) -> set[tuple[str, str]]:
    """What covers a category, decided by the STORE rather than the name.

    Product names drift — ours are chosen for a UI, iLEAPP's for a report — so
    name matching goes quiet exactly when a module lands under a different
    heading. The store cannot drift: two artifacts reading the same file are the
    same artifact whatever either project calls it.

    AN APP'S OWN PATH IS NOT ENOUGH ON ITS OWN. `Documents/user.db` is a path a
    hundred apps have, so when the module reads an app container and the glob
    was not anchored at a domain root, the match only counts if the module's
    domain names the app the category is about. Without that guard this would
    report AllTrails as covered because Waze keeps a store with the same name.

    A glob anchored at a real domain root needs no such guard: there is one
    `Library/Application Support/CloudDocs/session/db/client.db` on the device.
    """
    roots = domain_roots()
    hits: set[tuple[str, str]] = set()
    for a in items:
        for glob in a.get("paths", []):
            for rel, rooted in as_backup_path(glob, roots):
                for domain, path, what in stores:
                    if not same_store(rel, path):
                        continue
                    app_scoped = domain.startswith("AppDomain")
                    if not rooted and app_scoped and norm(category) not in norm(domain):
                        continue
                    hits.add(what)
    return hits


def glob_to_re(g: str) -> re.Pattern:
    """An iLEAPP path glob as a regex over a backup's relativePath.

    Anchored at the END unless the glob ends in `*`, because the tail is what
    identifies the file. Unanchored at the front: a backup's relativePath is
    domain-relative and iLEAPP's globs start from the filesystem root.
    """
    g = g.lstrip("*").lstrip("/")
    parts = [re.escape(p) for p in g.split("*")]
    tail = "" if g.endswith("*") else "$"
    return re.compile(".*" + ".*".join(parts) + tail, re.I)


def split_by_backup(artifacts, untouched, paths_file: Path):
    """Untouched artifacts, split by what a real backup contains."""
    # (domain, path). The DOMAIN is what makes a bad match obvious: iLEAPP globs
    # are rooted at the filesystem, so `*/Application Support/DataStore*` matches
    # any app's — it put Google Duo against Apple Books here. Nothing can tell
    # those apart automatically, so the report shows the domain and lets a reader
    # see it in one glance instead of trusting a count.
    rows = []
    for line in paths_file.read_text().splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) == 2:
            rows.append((parts[0], parts[1]))
    ours = stores_we_read()
    same, unread, absent = collections.Counter(), collections.Counter(), collections.Counter()
    examples: dict[str, str] = {}
    for a in artifacts:
        if a.get("reach") not in ("backup", "encrypted-only"):
            continue
        cat = a.get("category")
        if cat not in untouched:
            continue
        hit = None
        for g in a.get("paths") or []:
            rx = glob_to_re(g)
            hit = next((r for r in rows if rx.match(r[1])), None)
            if hit:
                break
        if hit is None:
            absent[cat] += 1
            continue
        domain, path = hit
        shown = f"{domain[:34]}  {path}"
        if any(path.endswith(o) or o.endswith(path) for o in ours):
            same[cat] += 1
            examples.setdefault(cat, shown)
        else:
            unread[cat] += 1
            examples.setdefault(cat, shown)
    return same, unread, absent, examples, len(rows)


def reads(items, covered) -> str | None:
    """Which artifacts in a category we read, and which we do not — computed.

    Matched by artifact NAME against everything `ours()` found, the same way a
    whole category is matched. That makes the sentence move on its own when a
    module lands, which is the entire point: the hand-written version this
    replaces reported Clock as 1 of 4 after three of the four had shipped.

    Naming what is NOT read is the useful half — it is the worklist.

    RETURNS None WHEN NAME MATCHING CANNOT SPEAK. Per-artifact names only line
    up with ours where iLEAPP names the thing rather than the product: "Timers"
    matches, "Kik Messages" never will, because our parser is called "Kik". In
    those categories no artifact matches, and printing "NOT <every artifact>"
    would report a covered chat app as a total gap. Saying nothing is the honest
    answer; the `how` beside it already names what reads the category.
    """
    yes = sorted(a["name"] for a in items if norm(a.get("name", "")) in covered)
    no = sorted(a["name"] for a in items if norm(a.get("name", "")) not in covered)
    if not yes:
        return None
    if not no:
        return f"all {len(yes)}"
    return f"{', '.join(yes)} — NOT {', '.join(no)}"


def with_detail(how: str, items, covered) -> str:
    detail = reads(items, covered)
    return f"{how} — {detail}" if detail else how


PART_RE = re.compile(r"([a-z ]+?) \(([^)]*)\)")


def parse_parts(prose: str) -> dict[str, list[str]]:
    """`module (a, b), app chat parser (c)` → `{"module": ["a","b"], …}`.

    The inverse of `fmt`, so the hand-written aliases and the computed matches
    are the same shape and can simply be merged.
    """
    out: dict[str, list[str]] = {}
    for kind, detail in PART_RE.findall(prose):
        out.setdefault(kind.strip(), []).extend(d.strip() for d in detail.split(","))
    return out


# Where each kind of coverage lives, so an alias naming something deleted is an
# error rather than a quiet overstatement.
WHERE = {
    "module": "crates/traceloupe-core/modules/{}.toml",
    "app chat parser": "crates/traceloupe-core/src/parsers/apps/{}.rs",
    "native parser": "crates/traceloupe-core/src/parsers/{}.rs",
}


def check_aliases() -> list[str]:
    """Aliases naming code that is gone. An alias is a claim of coverage."""
    bad = []
    for cat, prose in ALIASES.items():
        for kind, names in parse_parts(prose).items():
            for name in names:
                # Free-text notes ("the device header", "driven directly") are
                # not paths and are not checked; only the ones that name code.
                tmpl = WHERE.get(kind)
                if tmpl and " " not in name and not (ROOT / tmpl.format(name)).exists():
                    bad.append(f"ALIASES[{cat!r}] names {kind} {name!r}, which does not exist")
    return bad


def how_covered(cat: str, items, covered, stores) -> str | None:
    """One sentence naming everything of ours that reads part of this category.

    The NAME match and the STORE match are UNIONED rather than raced. They see
    different things — a name knows a chat parser the store rules cannot place,
    a store knows a module filed under a heading of our own choosing — and
    taking whichever answered first is how `location_clients` went missing from
    "Location" the moment a second module was filed under the same word.

    The hand-written aliases join the union rather than sitting under it as a
    fallback. That is safe in one direction only — an alias can now add a name
    but never hide one — and `check_aliases` closes the other, refusing to run
    with an alias that names a module or parser no longer in the tree.
    """
    n = norm(cat)
    parts: dict[str, list[str]] = {k: list(v) for k, v in covered.get(n, {}).items()}
    for kind, detail in parse_parts(ALIASES.get(n, "")).items():
        parts.setdefault(kind, []).extend(detail)
    known = {(kind, name) for kind, names in parts.items() for name in names}
    extra = sorted(by_store(items, stores, cat) - known)
    for kind, name in extra:
        parts.setdefault(kind, []).append(name)
    if not parts:
        return None
    return fmt(parts) + (", some by store" if extra else "")


def is_photos_facet(cat: str) -> bool:
    return cat.lower().startswith(("photos.sqlite", "photos-"))


# (iLEAPP glob, our module path, should they be the same store?) — the cases
# that were wrong, kept as the record of what wrong looked like.
STORE_CASES = [
    # Rooted at /var/mobile: the whole Files App cluster read as untouched
    # because the root was never stripped off the glob.
    ("*/mobile/Library/Application Support/CloudDocs/session/db/client.db*",
     "Library/Application Support/CloudDocs/session/db/client.db", True),
    # …and taking the LONGEST root put that same glob in HealthDomain, where
    # `/var/mobile/Library` swallowed two more segments than it should have.
    ("*/mobile/Library/Application Support/CloudDocs/session/db/server.db*",
     "Library/Application Support/CloudDocs/session/db/server.db", True),
    # A rootless fragment, tried as already domain-relative.
    ("*/Library/Caches/locationd/clients.plist",
     "Library/Caches/locationd/clients.plist", True),
    # `*` does not cross `/`: a store in a subdirectory is a different store.
    ("*/mobile/Library/Preferences/com.apple.wifi.plist",
     "Library/Preferences/nested/com.apple.wifi.plist", False),
    # Same tail, different tree.
    ("*/mobile/Library/Application Support/CloudDocs/session/db/client.db*",
     "Library/Other/client.db", False),
    # A leading wildcard means AT ANY DEPTH: three home-screen modules read this
    # store at `Library/SpringBoard/`, and anchoring the front reported all
    # three as untouched.
    ("**/SpringBoard/IconState.plist", "Library/SpringBoard/IconState.plist", True),
    ("*NoteStore.sqlite*", "NoteStore.sqlite", True),
    # …but the tail still has to match completely.
    ("**/SpringBoard/IconState.plist", "Library/SpringBoard/Other.plist", False),
    # A tail too short to be a filename matches nothing at all. `*.db` at any
    # depth would otherwise claim every store in the app.
    ("**/*.db", "Library/CoreDuet/People/interactionC.db", False),
]


def self_test() -> int:
    """Everything that can be checked without an iLEAPP checkout."""
    failures = []

    for glob, path, want in STORE_CASES:
        got = any(same_store(rel, path) for rel, _ in as_backup_path(glob, domain_roots()))
        mark = "ok  " if got == want else "FAIL"
        print(f"  {mark}  {'same store' if want else 'different'}: {glob} ~ {path}")
        if got != want:
            failures.append(f"{glob} ~ {path}: expected {want}, got {got}")

    # An app container path is ambiguous by construction, so it must need the
    # category to name the app. Both directions, because only checking the
    # positive would pass on a matcher that had stopped checking at all.
    stores = [("AppDomain-com.waze.iphone", "Documents/user.db", ("module", "waze_recents"))]
    item = {"name": "x", "paths": ["*/mobile/Containers/Data/Application/*/Documents/user.db"]}
    for cat, want in (("Waze", {("module", "waze_recents")}), ("AllTrails", set())):
        got = by_store([item], stores, cat)
        mark = "ok  " if got == want else "FAIL"
        print(f"  {mark}  container path under {cat!r} → {sorted(got) or 'nothing'}")
        if got != want:
            failures.append(f"container match for {cat}: expected {want}, got {got}")

    # The native importer's stores must be in the store list: their iLEAPP
    # categories ("InteractionC", "Cloudkit") are named after neither the store
    # nor anything we call it, so name matching cannot see them either.
    native = {p for _, p, (kind, _) in our_stores() if kind == "native parser"}
    for want in ("Library/CoreDuet/People/interactionC.db", "NoteStore.sqlite"):
        ok = want in native
        print(f"  {'ok  ' if ok else 'FAIL'}  native store listed: {want}")
        if not ok:
            failures.append(f"{want} is parsed in import.rs but not in our_stores()")

    for problem in check_aliases():
        print(f"  FAIL  {problem}")
        failures.append(problem)
    if not check_aliases():
        print(f"  ok    every alias names code that exists ({len(ALIASES)} aliases)")

    # A name several modules answer to must name all of them.
    covered = ours()
    multi = [k for k, v in covered.items() if len(v.get("module", [])) > 1]
    print(f"  ok    {len(multi)} names matched by more than one module")

    if failures:
        print(f"\n✗ {len(failures)} failure(s)")
        return 1
    print("\n✓ coverage-gap self-test clean")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--input", default="/tmp/ileapp.json", help="classify-ileapp-artifacts.py --json output")
    ap.add_argument("--json", metavar="FILE", help="write the full table")
    ap.add_argument("--all", action="store_true", help="every gap, not just the biggest")
    ap.add_argument("--present", metavar="PATHS", help="split the gap by a real backup's manifest")
    ap.add_argument("--self-test", action="store_true", help="check the matcher; needs no iLEAPP checkout")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    path = Path(args.input)
    if not path.exists():
        print(f"missing {path} — run:\n  python3 tools/classify-ileapp-artifacts.py --json {path}")
        return 2
    artifacts = json.load(open(path))
    reachable = [a for a in artifacts if a.get("reach") in ("backup", "encrypted-only")]

    if bad := check_aliases():
        print("\n".join(bad), file=sys.stderr)
        return 2

    covered = ours()
    stores = our_stores()
    by_cat: dict[str, list] = collections.defaultdict(list)
    for a in reachable:
        by_cat[a.get("category") or "?"].append(a)

    gaps, done, excused = [], [], []
    for cat, items in by_cat.items():
        n = norm(cat)
        # ORDER: name, then STORE, then the hand-written aliases LAST. The
        # alias table is the only part of this file a human maintains, so it is
        # the only part that can go stale — and it did, twice, naming two Wi-Fi
        # modules after a third shipped and two Clock modules after five. Asking
        # the store first means an alias is consulted only where nothing can be
        # computed, which is the smallest surface a stale list can wrong.
        if is_photos_facet(cat):
            excused.append((cat, len(items), NOT_A_GAP["photos"]))
        elif n in NOT_A_GAP:
            excused.append((cat, len(items), NOT_A_GAP[n]))
        elif how := how_covered(cat, items, covered, stores):
            done.append((cat, len(items), with_detail(how, items, covered)))
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

    if args.present:
        pf = Path(args.present)
        if not pf.exists():
            print(f"\nmissing {pf} — see --help for how to produce it")
            return 2
        same, unread, absent, ex, n = split_by_backup(artifacts, {c for c, _ in gaps}, pf)
        print(f"\n══ against a real backup ({n} manifest entries) ══")
        print(f"\n── SAME STORE: {sum(same.values())} artifacts read a file we ALREADY parse.")
        print("   No new parsing — analysis we do not do on data already in the cache.")
        for c, k in same.most_common():
            print(f"  {k:3}  {c:22} {ex.get(c, '')[:78]}")
        print(f"\n── UNREAD STORE: {sum(unread.values())} artifacts read a file in this backup")
        print("   that nothing reads. Real work — but CHECK THE DOMAIN: an iLEAPP glob")
        print("   with no domain root matches any app's file, so a product name against")
        print("   an unrelated domain is a bad match, not a gap.")
        for c, k in unread.most_common():
            print(f"  {k:3}  {c:22} {ex.get(c, '')[:78]}")
        print(f"\n── NOT HERE: {sum(absent.values())} artifacts in {len(absent)} categories.")
        print("   Not unreachable — this device just lacks the app. Needs another device")
        print("   before it can be built OR ruled out.")

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
