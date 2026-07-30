/**
 * Guard: no artifact module may read a store the native importer already parses.
 *
 * The two halves of this app read backups in different ways. `import.rs` has
 * hand-written parsers for the big stores — Messages, Photos, Safari, Calls,
 * Health — that produce real typed views. The declarative modules read the long
 * tail into a generic table. Both are right for their half, and NEITHER is right
 * for the same file.
 *
 * A module pointed at an already-parsed store would show the same data twice, in
 * two places, parsed two different ways, with no indication which one to believe.
 * That is worse than not having it at all, because a reader who finds a
 * disagreement has no way to resolve it.
 *
 * This is not hypothetical: I was one commit away from adding a `safari_tabs`
 * module before noticing that `SafariTabs.db` AND `BrowserState.db` are already
 * imported natively — including a private-browsing flag the module would have
 * lost. Nothing would have failed. The Safari view would simply have grown a
 * second, worse copy of its own tabs.
 *
 * Against the NATIVE importer the check is per-file: if `import.rs` opens a store,
 * no module may. Between MODULES it is per-collection, because one file really can
 * hold two unrelated artifacts — `com.apple.mobiletimerd.plist` has `MTAlarms` and
 * `MTSleepAlarms` side by side, and forbidding that would be forbidding the data
 * rather than the duplication.
 *
 *   node scripts/check-artifact-overlap.mjs
 */
import { readFileSync, readdirSync } from "node:fs";

const root = new URL("..", import.meta.url);
const read = (p) => readFileSync(new URL(p, root), "utf8");

/** Relative paths the native importer reads.
 *
 *  Taken from the source rather than a list kept beside it, so a parser added
 *  tomorrow is covered without anyone remembering this file. Deliberately
 *  over-inclusive: it collects every store-shaped string literal in import.rs,
 *  because a false positive here costs one conversation and a false negative
 *  costs a duplicated artifact nobody notices. */
function nativelyParsedPaths() {
  const src = read("crates/traceloupe-core/src/import.rs")
    // Comments mention paths while discussing them; only real literals count.
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
  const paths = new Set();
  for (const m of src.matchAll(/"([^"]*\.(?:db|sqlite|sqlitedb|storedata|plist))"/g)) {
    const p = m[1];
    // Working files of the importer itself, and the manifest every reader opens.
    if (p.startsWith(".") || p.startsWith("cache.") || p === "Manifest.db") continue;
    paths.add(p);
  }
  if (paths.size < 5) {
    throw new Error(
      `found only ${paths.size} natively-parsed paths in import.rs — the parse is wrong, ` +
        `and a check that knows of no native parsers can never find an overlap`,
    );
  }
  return paths;
}

/** What each shipped module reads. */
function modulePaths() {
  const dir = new URL("crates/traceloupe-core/modules/", root);
  const out = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".toml"))) {
    const toml = readFileSync(new URL(file, dir), "utf8");
    const path = toml.match(/^path\s*=\s*"([^"]+)"/m)?.[1];
    const domain = toml.match(/^domain\s*=\s*"([^"]+)"/m)?.[1];
    // What it reads WITHIN the store, so two modules taking different
    // collections out of one file are not mistaken for duplicates.
    const rows = toml.match(/^rows\s*=\s*(\[.*\]|"[^"]*")/m)?.[1] ?? "";
    const sql = toml.match(/^sql\s*=\s*[\s\S]*?\n(?=\[|\w|$)/m)?.[0] ?? "";
    if (path) out.push({ file, domain, path, within: `${rows}${sql}`.replace(/\s+/g, " ").trim() });
  }
  if (out.length === 0) {
    throw new Error("found no module TOMLs — the path is wrong, so this check verified nothing");
  }
  return out;
}

const native = nativelyParsedPaths();
const modules = modulePaths();
const failures = [];

for (const { file, domain, path } of modules) {
  if (native.has(path)) {
    failures.push(
      `${file}: reads ${domain}:${path}, which import.rs already parses natively. ` +
        `The same store read twice shows the same data in two places, parsed two ` +
        `different ways, with nothing to say which to believe — and the native ` +
        `parser is the one with a real view behind it. Fold into that instead.`,
    );
    continue;
  }
  console.log(`  ok    ${file} — ${path} is not natively parsed`);
}

// Two modules must not read the same ROWS. Sharing a FILE is legitimate and
// happens for real: com.apple.mobiletimerd.plist holds `MTAlarms` and
// `MTSleepAlarms` side by side, which are two different artifacts that happen to
// live together. What must not repeat is the same collection read twice.
const seen = new Map();
for (const { file, domain, path, within } of modules) {
  const key = `${domain}:${path}::${within}`;
  if (seen.has(key)) {
    failures.push(
      `${file} and ${seen.get(key)} read the SAME rows out of ${domain}:${path} — ` +
        `sharing a file is fine, reading the same collection twice is not`,
    );
  }
  seen.set(key, file);
}

if (failures.length) {
  console.error(`\nartifact-overlap check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `\nartifact-overlap OK — all ${modules.length} module(s) read stores the native ` +
    `importer does not, and no two read the same one.`,
);
