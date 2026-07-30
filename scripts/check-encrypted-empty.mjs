/**
 * Guard: views whose data is encrypted-backup-only must say so when the backup
 * is not encrypted — instead of rendering a bare "nothing here".
 *
 * iOS puts `interactionC.db`, `Health`/`MedicalID` and `SafariTabs.db` on
 * `Domains.plist`'s `RelativePathsToOnlyBackupEncrypted` list, so an
 * unencrypted backup genuinely cannot carry them (see
 * docs/reference/backup-coverage-audit.md). Saying only "No interaction data in
 * this backup" claims the person contacted nobody, which is a different and far
 * stronger statement than "this kind of backup cannot hold it". Telling those
 * two apart is the app's whole job.
 *
 * The check *states what it observed* and fails when it observed too little —
 * a check that silently finds no views is indistinguishable from a passing one.
 *
 *   node scripts/check-encrypted-empty.mjs [baseUrl]
 */
import { chromium } from "@playwright/test";

const BASE = process.argv[2] ?? "http://localhost:5173";

// Each view, and the wording that proves it explained itself rather than
// shrugging. `mustNotSay` is the generic message it used to fall back to.
const CASES = [
  { view: "Interactions", mustSay: /only included in encrypted backups/i },
  { view: "Health", mustSay: /only included in encrypted backups/i },
  // A declarative artifact declaring `requires = "encrypted-backup"` must
  // explain itself exactly as the hand-built views do — same wording, from the
  // same hook, so there is one sentence for this situation rather than two.
  { view: "Artifacts", mustSay: /only included in encrypted backups/i, select: "Focus modes" },
];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const failures = [];
let observed = 0;

// The mock starts with no backup open, and a view with no backup shows its
// marketing state rather than an empty list — which would sail past this check
// while proving nothing. So open the mock backup first, exactly as the design
// lint does, and refuse to measure a view that is still showing "No backup".
await page.goto(`${BASE}/?mock=unencrypted`, { waitUntil: "networkidle" });
await page.waitForTimeout(800);
const open = page.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) {
  await open.click().catch(() => {});
  await page.waitForTimeout(1500);
}

// Navigate by clicking, never by page.goto: the mock's "a backup is open" flag
// lives in page memory, so a full navigation resets it and every view falls
// back to its no-backup state — which has no empty list to check at all.
for (const { view, mustSay, select } of CASES) {
  await page.getByText(view, { exact: true }).first().click().catch(() => {});
  await page.waitForTimeout(900);

  // Artifacts are only present once they have been extracted from the backup —
  // a cache imported before a module existed has none. The guard has to walk
  // through that, or it measures the "nothing extracted yet" screen and reports
  // a missing explanation that is not actually missing.
  const extract = page.getByRole("button", { name: /Extract artifacts/ });
  if (await extract.count()) {
    await extract.first().click().catch(() => {});
    await page.waitForTimeout(1200);
  }

  // Some views need a selection before the state under test is on screen.
  if (select) {
    await page.getByText(select, { exact: false }).first().click().catch(() => {});
    await page.waitForTimeout(600);
  }

  // Prove we arrived. A silent navigation failure reads exactly like a view
  // that explained itself.
  const heading = await page.evaluate(
    () => document.querySelector("h1, [data-slot='toolbar'] span")?.textContent ?? "",
  );
  if (!heading.includes(view)) {
    failures.push(`${view}: clicking it left the app showing "${heading}" — wrong view measured`);
    continue;
  }


  const body = (await page.locator("body").innerText()).replace(/\s+/g, " ");
  if (/No backup open/i.test(body)) {
    failures.push(`${view}: no backup was open, so the empty state was never reached`);
    continue;
  }
  observed += 1;
  if (mustSay.test(body)) {
    console.log(`  ok    ${view} explains why it is empty`);
  } else {
    const shown = body.slice(0, 160);
    failures.push(`${view}: no encryption explanation. Showed: "${shown}…"`);
    console.log(`  FAIL  ${view} did not explain why it is empty`);
  }
}

await browser.close();

if (observed !== CASES.length) {
  failures.push(
    `observed ${observed} of ${CASES.length} views — a check that sees nothing must not pass`,
  );
}

if (failures.length) {
  console.error(`\nencrypted-empty check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `\nencrypted-empty OK — ${observed} encrypted-only views explain themselves on an unencrypted backup.`,
);
