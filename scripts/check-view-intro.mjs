/**
 * Guard: every view must introduce itself to someone who has not opened a backup.
 *
 * `NoBackupState` is the app's main way of telling a person what it can read —
 * it is what fills every view before a backup is chosen. Nothing checked that it
 * exists or says anything useful, so a view could ship with none at all, or with
 * a placeholder, and the only way to find out was to click it with no backup
 * open. That is exactly how the Artifacts view turned out to be unintelligible
 * to the person who commissioned it: not because the copy was bad, but because
 * nothing was checking any view could introduce itself.
 *
 * The check is deliberately about SUBSTANCE, not wording: a title that names the
 * view, a lead long enough to be a sentence rather than a label, and at least one
 * concrete capability. What each view should *say* is per-view work; that it says
 * something is enforceable.
 *
 *   node scripts/check-view-intro.mjs [baseUrl]
 */
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const BASE = process.argv[2] ?? process.env.BASE ?? "http://localhost:5173";

/** Views taken from nav.ts, so a new destination is covered the day it lands
 *  rather than the day someone remembers to add it here. */
function navLabels() {
  const src = readFileSync(new URL("../src/lib/nav.ts", import.meta.url), "utf8");
  // `standaloneArtifactsNav` is conditional — it only appears when a module
  // declares it fits nowhere, so it is not in the sidebar to click.
  const navOnly = src.slice(src.indexOf("export const nav"));
  const labels = [...navOnly.matchAll(/label:\s*"([^"]+)"/g)].map((m) => m[1]);
  if (labels.length < 10) {
    throw new Error(
      `only found ${labels.length} nav labels in nav.ts — the parse is wrong, ` +
        `and a check that silently covers 2 views reports the same OK as one that covers 16`,
    );
  }
  return labels;
}

/** A lead has to be a sentence, not a label. Short enough to be a heading is not
 *  an introduction. */
const MIN_LEAD = 40;

const VIEWS = navLabels();
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
const failures = [];
let measured = 0;

// No backup open, deliberately: this is the state under test.
await page.goto(`${BASE}/`, { waitUntil: "networkidle" });
await page.waitForTimeout(800);

for (const view of VIEWS) {
  await page.getByText(view, { exact: true }).first().click().catch(() => {});
  await page.waitForTimeout(600);

  const seen = await page.evaluate(() => {
    // The intro lives in the view body; the sidebar and title bar are chrome.
    const main = document.querySelector("main") ?? document.body;
    const text = (main.innerText || "").replace(/\s+/g, " ").trim();
    // NoBackupState renders its features as label/detail pairs.
    const intro = main.querySelector("[data-slot='view-intro']");
    const lead = intro?.querySelector("[data-slot='view-intro-lead']");
    const features = [...(intro?.querySelectorAll("[data-slot='view-intro-feature']") ?? [])];
    return {
      text,
      chars: text.length,
      hasIntro: !!intro,
      lead: (lead?.textContent || "").trim(),
      features: features.length,
    };
  });

  measured += 1;

  if (!seen.hasIntro) {
    failures.push(
      `${view}: no NoBackupState with no backup open — nothing tells a newcomer what this view is`,
    );
    continue;
  }
  if (seen.lead.length < MIN_LEAD) {
    failures.push(
      `${view}: its lead is ${seen.lead.length} characters — a label, not an introduction`,
    );
    continue;
  }
  if (/coming soon|todo|tbd|placeholder|lorem/i.test(seen.text)) {
    failures.push(`${view}: its no-backup state is a placeholder`);
    continue;
  }
  if (seen.features === 0) {
    failures.push(
      `${view}: names no concrete capability — NoBackupState's \`features\` say what a ` +
        `person can actually do here, and without them the screen is only prose`,
    );
    continue;
  }
  console.log(
    `  ok    ${view} — introduces itself (${seen.lead.length}-char lead, ${seen.features} capabilities)`,
  );
}

await browser.close();

// A check that quietly measured nothing reports the same OK as one that passed.
if (measured !== VIEWS.length) {
  failures.push(`measured ${measured} of ${VIEWS.length} views from nav.ts`);
}

if (failures.length) {
  console.error(`\nview-intro check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`\nview-intro OK — all ${measured} views introduce themselves with no backup open.`);
