/**
 * Every toolbar island is one height, and every segment inside one is another.
 *
 * This exists because the rule kept being broken by things that looked fine in
 * isolation: the Filter control was `size-9` (36px) when idle and a 38px island
 * once a filter was applied — so it was both taller than its neighbours AND
 * changed height as you used it. The search box was `h-9`. Two view-mode toggles
 * defaulted to 28px segments where a third passed `size="sm"`. Each was a
 * literal written at a call site, which is exactly what
 * docs/reference/ui.md forbids and what nobody notices in review.
 *
 * Islands are measured in their real states — idle, with a filter applied, with
 * search expanded — because the first bug only appeared in one of them.
 *
 * Usage: pnpm dev (or vite --port N), then:
 *   node scripts/check-toolbar-geometry.mjs
 *   BASE=http://localhost:1438 node scripts/check-toolbar-geometry.mjs
 * Exits non-zero listing anything off-scale.
 */
import { chromium } from "@playwright/test";

const BASE = process.env.BASE || "http://localhost:1420";
const ISLAND = 30; // one segment + p-0.5 + 1px border, per --island-h
const SEGMENT = 24; // --control-h-sm

const browser = await chromium.launch();
const page = await (
  await browser.newContext({ viewport: { width: 1300, height: 880 }, colorScheme: "dark" })
).newPage();
await page.goto(BASE + "/", { waitUntil: "networkidle" });
await page.waitForTimeout(800);
const open = page.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) {
  await open.click().catch(() => {});
  await page.waitForTimeout(1100);
}

/** Measure every island and segment currently on screen. */
const measure = () =>
  page.evaluate(() => {
    const rows = [];
    const scopes = [
      ...document.querySelectorAll('[data-tauri-drag-region]'),
      ...document.querySelectorAll('[data-slot="card-header"]'),
    ];
    for (const scope of scopes) {
      for (const el of scope.querySelectorAll("button, div, input")) {
        const r = el.getBoundingClientRect();
        if (!r.height || r.height > 60 || r.width < 12) continue;
        const cls = (el.className || "").toString();
        const isIsland = /rounded-lg/.test(cls) && /border/.test(cls) && /bg-muted/.test(cls);
        const isSegment = el.tagName === "BUTTON" && !!el.closest("[class*='bg-muted']");
        if (!isIsland && !isSegment) continue;
        const name =
          (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.textContent || "")
            .trim().replace(/\s+/g, " ").slice(0, 24) || "(icon)";
        rows.push({ kind: isIsland ? "island" : "segment", name, h: Math.round(r.height * 10) / 10 });
      }
    }
    return rows;
  });

const bad = [];
const check = async (state) => {
  for (const row of await measure()) {
    const want = row.kind === "island" ? ISLAND : SEGMENT;
    if (Math.abs(row.h - want) > 0.6) bad.push(`${state}: ${row.kind} "${row.name}" is ${row.h}px, expected ${want}px`);
  }
};

for (const view of ["Messages", "Notes", "Safety", "Photos"]) {
  await page.getByText(view, { exact: true }).first().click().catch(() => {});
  await page.waitForTimeout(1200);
  const dismiss = page.getByRole("button", { name: /^Got it$/ });
  if (await dismiss.count()) { await dismiss.first().click().catch(() => {}); await page.waitForTimeout(300); }
  await check(view);

  // Expanded search — a state the idle measurement never sees.
  const search = page.getByRole("button", { name: /search/i }).first();
  if (await search.count()) {
    await search.click().catch(() => {});
    await page.waitForTimeout(500);
    await check(`${view}+search`);
    await page.keyboard.press("Escape").catch(() => {});
    await page.waitForTimeout(300);
  }

  // With a filter applied — where the Filter control used to change height.
  const funnel = page.getByRole("button", { name: "Filter" }).first();
  if (await funnel.count()) {
    await funnel.click().catch(() => {});
    await page.waitForTimeout(500);
    const opt = page.getByRole("button", { name: /iMessage|SMS|With photos|Serious|Harmful|Folders/i }).first();
    if (await opt.count()) {
      await opt.click().catch(() => {});
      await page.waitForTimeout(600);
      await page.keyboard.press("Escape").catch(() => {});
      await page.waitForTimeout(500);
      await check(`${view}+filter`);
    } else {
      await page.keyboard.press("Escape").catch(() => {});
    }
  }
}
await browser.close();

if (bad.length) {
  console.error(`toolbar geometry is off in ${bad.length} place(s):\n  ` + [...new Set(bad)].join("\n  "));
  console.error(`\nIslands are ${ISLAND}px and their segments ${SEGMENT}px — take the height from` +
    ` --island-h / --control-h-sm rather than writing a literal (see docs/reference/ui.md).`);
  process.exit(1);
}
console.log(`toolbar geometry OK — every island ${ISLAND}px, every segment ${SEGMENT}px, across` +
  ` Messages / Notes / Safety / Photos including expanded search and an applied filter.`);
