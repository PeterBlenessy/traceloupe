/**
 * Guard: a view whose store was in the backup and could NOT be read must say
 * so — not "there is none of this in your backup".
 *
 * This is the fifth empty-state reason (#288), and the only one where the data
 * exists and the shortfall is ours. #268 is why it has a guard: `sms.db`
 * decrypted truncated, would not open, and Messages read as empty for months.
 * Nothing on screen could have told anyone otherwise, and nothing in CI could
 * have either.
 *
 * Driven by `?mock=parse-failed`, which reports `calls` as failed and
 * `messages` as parsed — so this also proves the wording is SCOPED, and a
 * healthy view beside a broken one still says the ordinary thing.
 *
 *   node scripts/check-parse-failed.mjs [baseUrl]
 */
import { chromium } from "@playwright/test";

const BASE = process.argv[2] ?? "http://localhost:5173";

const CASES = [
  {
    view: "Calls",
    // The store was there and would not open.
    mustSay: /couldn't read the call history/i,
    mustNotSay: /No calls in this backup/i,
  },
  {
    // Parsed fine in the same mock: the failure must not leak across modules.
    view: "Messages",
    mustNotSay: /couldn't read/i,
  },
];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const failures = [];
let observed = 0;

await page.goto(`${BASE}/?mock=parse-failed`, { waitUntil: "networkidle" });
await page.waitForTimeout(800);
const open = page.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) {
  await open.click().catch(() => {});
  await page.waitForTimeout(1500);
}

// Click, never goto: the mock's "a backup is open" flag lives in page memory,
// so a full navigation resets it and every view falls back to its no-backup
// state — which has no empty list to measure.
for (const { view, mustSay, mustNotSay } of CASES) {
  await page.getByText(view, { exact: true }).first().click().catch(() => {});
  await page.waitForTimeout(900);

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

  if (mustSay && !mustSay.test(body)) {
    failures.push(`${view}: never said the store was unreadable. Showed: "${body.slice(0, 160)}…"`);
    console.log(`  FAIL  ${view} blamed the backup for a failed parse`);
    continue;
  }
  if (mustNotSay && mustNotSay.test(body)) {
    failures.push(`${view}: said "${body.match(mustNotSay)[0]}" when that is not the reason`);
    console.log(`  FAIL  ${view} gave the wrong empty reason`);
    continue;
  }
  console.log(`  ok    ${view} ${mustSay ? "names the failed parse" : "is unaffected by another module's failure"}`);
}

await browser.close();

if (observed !== CASES.length) {
  failures.push(
    `observed ${observed} of ${CASES.length} views — a check that sees nothing must not pass`,
  );
}

if (failures.length) {
  console.error(`\nparse-failed check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `\nparse-failed OK — ${observed} views tell an unreadable store apart from an empty one.`,
);
