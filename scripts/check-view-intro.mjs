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
 * The check is deliberately about SUBSTANCE, not wording: a heading, a lead long
 * enough to be a sentence rather than a label, and at least one concrete
 * capability — all of it on screen, in the view the sidebar label actually leads
 * to. What each view should *say* is per-view work; that it says something is
 * enforceable.
 *
 *   node scripts/check-view-intro.mjs [baseUrl]
 */
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const BASE = process.argv[2] ?? process.env.BASE ?? "http://localhost:5173";

/** Destinations taken from nav.ts, so a new one is covered the day it lands
 *  rather than the day someone remembers to add it here.
 *
 *  `to` as well as `label`, and that is not a convenience: the label is what gets
 *  clicked and the route is how we prove the click *arrived*. Matching pairs also
 *  makes the parse specific — a stray `label:` in some future settings nav cannot
 *  become a phantom view, because it has no adjacent `to:`.
 *
 *  Any quote style, because nothing in this repo normalises them (no prettier, no
 *  eslint) — a view written with 'single' quotes was silently skipped. */
function navDestinations() {
  const src = readFileSync(new URL("../src/lib/nav.ts", import.meta.url), "utf8");
  // `standaloneArtifactsNav` is conditional — it only appears when a module
  // declares it fits nowhere, so it is not in the sidebar to click. Declared
  // above `nav`, and pinned below so moving it cannot quietly add it back.
  const navStart = src.indexOf("export const nav");
  if (navStart < 0) throw new Error("no `export const nav` in nav.ts — the parse is wrong");
  const navOnly = src.slice(navStart);
  if (/standaloneArtifactsNav/.test(navOnly)) {
    throw new Error(
      "`standaloneArtifactsNav` now appears after `export const nav`, so it is inside the " +
        "slice this parse reads. It has no sidebar entry, so it would be 'checked' by " +
        "measuring whichever view was on screen before it. Exclude it explicitly.",
    );
  }
  const items = [
    ...navOnly.matchAll(/to:\s*['"`]([^'"`]+)['"`],\s*\n?\s*label:\s*['"`]([^'"`]+)['"`]/g),
  ].map((m) => ({ to: m[1], label: m[2] }));
  if (items.length < 10) {
    throw new Error(
      `only found ${items.length} nav destinations in nav.ts — the parse is wrong, ` +
        `and a check that silently covers 2 views reports the same OK as one that covers 16`,
    );
  }
  return items;
}

/** A lead has to be a sentence, not a label. Short enough to be a heading is not
 *  an introduction. */
const MIN_LEAD = 40;

const VIEWS = navDestinations();
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
const failures = [];
let measured = 0;

// No backup open, deliberately: this is the state under test.
await page.goto(`${BASE}/`, { waitUntil: "networkidle" });
await page.waitForTimeout(800);

for (const { to, label } of VIEWS) {
  const view = label;
  await page.getByText(label, { exact: true }).first().click().catch(() => {});
  // Wait for ARRIVAL, not for a duration. A swallowed click used to leave the
  // previous view on screen, which then got measured and reported as this one's
  // pass — so every view but the first inherited its predecessor's intro and the
  // check could not tell "this view introduces itself" from "some view did".
  await page.waitForURL((u) => u.pathname === to, { timeout: 5000 }).catch(() => {});

  const arrivedAt = new URL(page.url()).pathname;
  if (arrivedAt !== to) {
    failures.push(
      `${view}: clicking "${label}" never reached ${to} (still at ${arrivedAt}) — ` +
        `its sidebar label and nav.ts have drifted apart, or the entry is not in the sidebar`,
    );
    continue;
  }

  const seen = await page.evaluate(() => {
    // The intro lives in the view body; the sidebar and title bar are chrome.
    const main = document.querySelector("main") ?? document.body;
    const text = (main.innerText || "").replace(/\s+/g, " ").trim();
    const intro = main.querySelector("[data-slot='view-intro']");
    const lead = intro?.querySelector("[data-slot='view-intro-lead']");
    const features = [...(intro?.querySelectorAll("[data-slot='view-intro-feature']") ?? [])];
    const title = intro?.querySelector("h1, h2, h3");
    // Being in the DOM is not being on screen. #224 was exactly a "visible but
    // not real" bug, and the intro's own `pt-16` exists to clear the
    // absolutely-positioned title bar — so geometry is the thing to assert.
    let onScreen = false;
    if (intro) {
      const r = intro.getBoundingClientRect();
      const cs = getComputedStyle(intro);
      onScreen =
        r.width > 8 &&
        r.height > 8 &&
        r.right > 0 &&
        r.left < window.innerWidth &&
        r.bottom > 0 &&
        r.top < window.innerHeight &&
        cs.visibility !== "hidden" &&
        cs.display !== "none" &&
        Number(cs.opacity) > 0.1;
    }
    return {
      text,
      chars: text.length,
      hasIntro: !!intro,
      onScreen,
      title: (title?.textContent || "").trim(),
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
  if (!seen.onScreen) {
    failures.push(
      `${view}: its NoBackupState is in the DOM but not on screen — an intro nobody ` +
        `can see is not an intro`,
    );
    continue;
  }
  if (seen.title.length === 0) {
    failures.push(`${view}: its no-backup state has no heading`);
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
    `  ok    ${view} — at ${to}, introduces itself ` +
      `(${seen.lead.length}-char lead, ${seen.features} capabilities)`,
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
