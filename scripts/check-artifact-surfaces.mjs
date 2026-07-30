/**
 * Guard: every surface a shipped artifact module declares must have a host view.
 *
 * `useHostedArtifacts` is generic — a view asks for the artifacts assigned to it
 * and gets them keyed by `join_column`. Nothing connected the two ends: the
 * `Surface` enum lives in Rust, the hosts are `useHostedArtifacts("…")` calls in
 * TSX, and no check crossed that boundary. So a module could declare
 * `surface = "device"` and then load, validate, run, decrypt its store and write
 * its rows to `artifact_rows` while rendering **nowhere at all** — there was no
 * Device view (#231). That is a worse version of the failure the `join_column`
 * requirement exists to prevent, because the artifact is not even floating in
 * the wrong view; there is no view.
 *
 * The rule is deliberately about DECLARED surfaces, not every enum variant. An
 * unused surface needs no host — `Surface::Contacts` can sit in the enum
 * unclaimed. The moment a module claims one, the host has to exist.
 *
 *   node scripts/check-artifact-surfaces.mjs
 */
import { readFileSync, readdirSync } from "node:fs";

const root = new URL("..", import.meta.url);
const read = (p) => readFileSync(new URL(p, root), "utf8");

/** Surfaces the Rust enum allows, so a typo in a TOML is a different failure
 *  from a missing host and does not get reported as one. */
function knownSurfaces() {
  const src = read("crates/traceloupe-core/src/artifacts.rs");
  const block = src.match(/pub enum Surface \{([\s\S]*?)\n\}/);
  if (!block) throw new Error("could not find `pub enum Surface` in artifacts.rs");
  // Variants are CamelCase in Rust and kebab-case in TOML (serde
  // rename_all = "kebab-case").
  const variants = [...block[1].matchAll(/^\s{4}([A-Z][A-Za-z]*),/gm)].map((m) =>
    m[1].replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase(),
  );
  if (variants.length < 2) {
    throw new Error(
      `parsed only ${variants.length} Surface variants — the parse is wrong, and a check ` +
        `that knows no surfaces reports the same OK as one that verified them all`,
    );
  }
  return variants;
}

/** What each shipped module declares. */
function declaredSurfaces() {
  const dir = new URL("crates/traceloupe-core/modules/", root);
  const out = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".toml"))) {
    const toml = readFileSync(new URL(file, dir), "utf8");
    // Only top-level `surface = "…"`, not a word inside a SQL string or comment.
    const m = toml.match(/^surface\s*=\s*"([^"]+)"/m);
    if (!m) {
      out.push({ file, surface: null });
      continue;
    }
    out.push({ file, surface: m[1] });
  }
  if (out.length === 0) {
    throw new Error("found no module TOMLs — the path is wrong, so this check verified nothing");
  }
  return out;
}

/** Comments removed, so a view that only *mentions* the call does not count as
 *  making it. device.tsx's own docstring explains the bug by quoting
 *  `useHostedArtifacts("device")`, and that quote alone satisfied this check —
 *  which would have let a view document a host it never actually became. */
function withoutComments(src) {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}

/** Which surfaces a view actually asks for. */
function hostedSurfaces() {
  const dir = new URL("src/views/", root);
  const hosts = new Map();
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".tsx"))) {
    const src = withoutComments(readFileSync(new URL(file, dir), "utf8"));
    for (const m of src.matchAll(/useHostedArtifacts\(\s*"([^"]+)"/g)) {
      const list = hosts.get(m[1]) ?? [];
      list.push(file);
      hosts.set(m[1], list);
    }
  }
  return hosts;
}

const surfaces = knownSurfaces();
const modules = declaredSurfaces();
const hosts = hostedSurfaces();
const failures = [];

for (const { file, surface } of modules) {
  if (surface === null) {
    // The loader rejects this too; saying so here names the file, which the
    // loader's error does not.
    failures.push(`${file}: declares no \`surface\` — every module must state a home`);
    continue;
  }
  if (!surfaces.includes(surface)) {
    failures.push(
      `${file}: surface "${surface}" is not one of the Surface enum's variants ` +
        `(${surfaces.join(", ")})`,
    );
    continue;
  }
  // Standalone is its own destination and needs no host view.
  if (surface === "standalone") {
    console.log(`  ok    ${file} — standalone, its own destination`);
    continue;
  }
  const where = hosts.get(surface);
  if (!where || where.length === 0) {
    failures.push(
      `${file}: declares surface "${surface}" but NO view calls ` +
        `useHostedArtifacts("${surface}") — its rows would be extracted and stored, ` +
        `and then rendered nowhere`,
    );
    continue;
  }
  console.log(`  ok    ${file} — surface "${surface}" hosted by ${where.join(", ")}`);
}

// A check that measured nothing reports the same OK as one that passed.
if (modules.length === 0) failures.push("no modules were examined");

if (failures.length) {
  console.error(`\nartifact-surfaces check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `\nartifact-surfaces OK — all ${modules.length} shipped module(s) declare a surface ` +
    `that something actually hosts.`,
);
