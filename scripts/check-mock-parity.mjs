/**
 * The browser mock must describe the same world the backend does.
 *
 * TypeScript already guarantees `mockClient` implements every method of
 * `TraceLoupeClient`. What it cannot guarantee is that the mock *returns the
 * same things* — and the mock is what `check-design.mjs` measures, what every
 * screenshot shows, and what any browser check exercises. When it drifts, all of
 * those quietly stop testing anything, while still reporting success.
 *
 * That is not hypothetical. Twice in one week:
 *
 *  - the mock's `moduleMetrics` omitted recordings, calendar, reminders,
 *    workouts and interactions, so the design lint had never once seen those
 *    five dashboard tiles;
 *  - the mock computed `dismissed` its own way, so a fix to the SQL appeared not
 *    to work, because every browser check was still asserting the old answer.
 *
 * So this reconciles the inventories that exist in both languages. It is
 * deliberately crude — it reads the source rather than running anything — and
 * crude is fine: a missing id is a missing id.
 *
 * Usage: node scripts/check-mock-parity.mjs
 */
import { readFileSync } from "node:fs";

const failures = [];
const fail = (what, detail) => failures.push(`[${what}] ${detail}`);

const read = (p) => readFileSync(p, "utf8");

// ---------------------------------------------------------------- inventories

/** `id: "messages",` inside METRIC_SOURCES. */
const backendModules = () => {
  const src = read("crates/traceloupe-core/src/dashboard.rs");
  const start = src.indexOf("pub const METRIC_SOURCES");
  const end = src.indexOf("\n];", start);
  if (start === -1 || end === -1) {
    fail("parity", "could not find METRIC_SOURCES in dashboard.rs — this check has gone blind");
    return [];
  }
  return [...src.slice(start, end).matchAll(/^\s+id:\s*"([^"]+)"/gm)].map((m) => m[1]);
};

/** `id: "messages", label: …` inside the mock's moduleMetrics. */
const mockModules = () => {
  const src = read("src/lib/ipc.ts");
  const start = src.indexOf("moduleMetrics: async ()");
  const end = src.indexOf("\n  messageDateBounds:", start);
  if (start === -1 || end === -1) {
    fail("parity", "could not find the mock's moduleMetrics in ipc.ts — this check has gone blind");
    return [];
  }
  return [...src.slice(start, end).matchAll(/\bid:\s*"([^"]+)"/g)].map((m) => m[1]);
};

/** Routes the app actually has. */
const routes = () => {
  const src = read("src/main.tsx");
  return [...src.matchAll(/path:\s*"([^"]+)"/g)].map((m) => m[1]);
};

/** `route: "/messages",` inside METRIC_SOURCES. */
const backendRoutes = () => {
  const src = read("crates/traceloupe-core/src/dashboard.rs");
  const start = src.indexOf("pub const METRIC_SOURCES");
  const end = src.indexOf("\n];", start);
  if (start === -1) return [];
  return [...src.slice(start, end).matchAll(/^\s+route:\s*"([^"]+)"/gm)].map((m) => m[1]);
};

// ---------------------------------------------------------------------- rules

const be = backendModules();
const mo = mockModules();

// The check must be able to fail: if either side reads as empty, the comparison
// below would pass trivially — which is the whole failure this file is about.
if (be.length === 0) fail("parity", "read zero modules from dashboard.rs");
if (mo.length === 0) fail("parity", "read zero modules from the mock");

for (const id of be) {
  if (!mo.includes(id))
    fail("parity",
      `the mock has no "${id}" module. The design lint and every screenshot run` +
      ` against the mock, so that tile is never seen by any check.`);
}
for (const id of mo) {
  if (!be.includes(id))
    fail("parity",
      `the mock invents a "${id}" module the backend does not have — the checks` +
      ` are exercising something that cannot happen in the app.`);
}

const known = routes();
for (const r of backendRoutes()) {
  if (!known.includes(r))
    fail("route",
      `METRIC_SOURCES points a tile at "${r}", which is not a route in main.tsx —` +
      ` clicking that tile would do nothing, silently.`);
}

// ------------------------------------------------------------------- self-test
//
// Same rule as the design lint: prove the matchers fire, or a clean run means
// nothing. A regex that stops matching after a refactor would otherwise report
// perfect parity forever.
if (backendModules().length < 5 || mockModules().length < 5 || routes().length < 5) {
  console.error(
    "mock-parity SELF-TEST failed: one of the extractors returned almost nothing," +
      " so the comparisons above are vacuous. Fix the matcher, not the app.",
  );
  process.exit(2);
}

if (failures.length) {
  console.error(`mock parity failed — ${failures.length} finding(s):\n`);
  for (const f of failures) console.error(`  ${f}`);
  console.error(
    "\nThe mock is what the browser checks measure. Keeping it in step with the" +
      " backend is what makes those checks mean anything.",
  );
  process.exit(1);
}
console.log(
  `mock parity OK — ${be.length} dashboard modules present in both, all tile routes exist.`,
);
