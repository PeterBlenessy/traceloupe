/**
 * Guard: an empty list must blame the FILTER when a filter is what emptied it —
 * and must still blame the backup when the backup really is empty.
 *
 * `emptyListMessage` decides between "No calls in this backup" and "No calls in
 * this time range". Getting that wrong is not cosmetic: the first is a false
 * statement about someone's device, and telling those apart is the distinction
 * this app exists to make. It shipped untested because the repo has no
 * TypeScript test runner, so the guard belongs here with the other browser
 * checks rather than as a reason to add one (#278).
 *
 * BOTH halves matter:
 *   1. filtered to nothing  -> must NOT say "in this backup"
 *   2. genuinely empty      -> must say "in this backup"
 *
 * Without (2) the cheapest way to pass would be to delete the honest sentence
 * everywhere, which is the opposite failure and just as wrong.
 *
 *   node scripts/check-filtered-empty.mjs [baseUrl]
 */
import { chromium } from "@playwright/test";

// Calls and Safari are the two that were wrong. Notes already distinguished
// correctly and is here so a future refactor cannot regress it.
const VIEWS = ["Calls", "Safari", "Notes"];

const BASE = process.argv[2] ?? "http://localhost:5173";
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const failures = [];
let observed = 0;

async function openMock(query) {
  await page.goto(`${BASE}/${query}`, { waitUntil: "networkidle" });
  await page.waitForTimeout(800);
  const open = page.getByRole("button", { name: /Read & open/ }).last();
  if (await open.count()) {
    await open.click().catch(() => {});
    await page.waitForTimeout(1500);
  }
}

// Click, never goto: the mock's "a backup is open" flag lives in page memory,
// so a full navigation resets it and every view falls back to its no-backup
// state — which has no empty list to measure at all.
async function go(view) {
  await page.getByText(view, { exact: true }).first().click().catch(() => {});
  await page.waitForTimeout(900);
  const heading = await page.evaluate(
    () => document.querySelector("h1, [data-slot='toolbar'] span")?.textContent ?? "",
  );
  if (!heading.includes(view)) {
    failures.push(`${view}: clicking it left the app showing "${heading}" — wrong view measured`);
    return false;
  }
  return true;
}

const body = async () =>
  (await page.locator("body").innerText()).replace(/\s+/g, " ");

/**
 * Type `term` into the view's search, or clear it when `term` is "".
 *
 * Search, not a time preset — the time chips DISABLE themselves when their
 * facet count is zero, which is good UX and makes them useless here: the one
 * range guaranteed to be empty is the one that cannot be clicked. Search is
 * always enabled, and is the same kind of narrowing as far as the empty
 * wording is concerned.
 */
async function setSearch(term) {
  // ListSearch is one bordered box that animates from a w-8 icon button to an
  // open input; the <input> is always in the DOM and carries the placeholder as
  // its aria-label, so this finds it in either state.
  const box = page.getByRole("textbox", { name: /search/i }).first();
  if (!(await box.count())) return false;
  await box.click().catch(() => {});
  await page.waitForTimeout(300);
  await box.fill(term).catch(() => {});
  // The views debounce the term, and the count query round-trips after that.
  await page.waitForTimeout(1100);
  return true;
}

// ---- half 1: filtered to nothing ------------------------------------------
await openMock("");
for (const view of VIEWS) {
  if (!(await go(view))) continue;
  if (!(await setSearch("zzqqxnotathinganywhere"))) {
    failures.push(`${view}: no search to drive — the guard could not reach a filtered-empty state`);
    continue;
  }
  observed += 1;
  const text = await body();
  if (/in this backup/i.test(text)) {
    failures.push(
      `${view}: narrowed to nothing by a search and still said "in this backup" — a false claim about the device`,
    );
    console.log(`  FAIL  ${view} blamed the backup for a filter`);
  } else if (!/in this time range|match these filters|match this search/i.test(text)) {
    failures.push(`${view}: filtered to empty but named no reason. Showed: "${text.slice(0, 160)}…"`);
    console.log(`  FAIL  ${view} gave no reason`);
  } else {
    console.log(`  ok    ${view} blames the filter, not the backup`);
  }
  // Clearing it must bring the rows back, or the "empty" above proved nothing.
  await setSearch("");
  const back = await body();
  if (/match this search|match these filters/i.test(back)) {
    failures.push(`${view}: clearing the search left the filtered message on screen`);
  }
  await page.keyboard.press("Escape").catch(() => {});
  await page.waitForTimeout(300);
}

// ---- half 2: genuinely empty ----------------------------------------------
// Without this, deleting the honest sentence entirely would pass half 1.
await openMock("?mock=no-data");
for (const view of VIEWS) {
  if (!(await go(view))) continue;
  observed += 1;
  const text = await body();
  if (/in this backup/i.test(text)) {
    console.log(`  ok    ${view} still says so when the backup really is empty`);
  } else {
    failures.push(
      `${view}: an empty backup did not say "in this backup" — the honest message is gone. Showed: "${text.slice(0, 160)}…"`,
    );
    console.log(`  FAIL  ${view} lost the honest empty message`);
  }
}

await browser.close();

const expected = VIEWS.length * 2;
if (observed !== expected) {
  failures.push(
    `observed ${observed} of ${expected} states — a check that sees nothing must not pass`,
  );
}

if (failures.length) {
  console.error(`\nfiltered-empty check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `\nfiltered-empty OK — ${VIEWS.length} views blame the filter when filtered, and the backup when empty.`,
);
