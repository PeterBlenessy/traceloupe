#!/usr/bin/env node
/**
 * No view may leave content clipped and unreachable.
 *
 * The app shell's content wrapper is `overflow-hidden` on purpose: it is the
 * clipping box that lets an opted-in list rise under the translucent bar. That
 * makes scrolling every view's OWN responsibility — and a view that forgets has
 * no local symptom. It looks correct until the window is short enough, and then
 * the bottom of the page simply does not exist: no scrollbar, no wheel
 * response, nothing to indicate anything is missing.
 *
 * That is what shipped in the Device view (#344), measured at 1089px of content
 * inside a 900px wrapper — the last 189px were unreachable by any means.
 *
 * The test is exactly that measurement: if the shell's overflow-hidden wrapper
 * has more content than it can show, the view under it is not scrolling and the
 * overflow is lost. A view that scrolls properly keeps its wrapper's
 * scrollHeight equal to its clientHeight, because the scrolling happens inside.
 *
 * Usage: pnpm dev (or vite --port N), then:
 *   node scripts/check-view-scroll.mjs [http://localhost:1420]
 * Exits non-zero if any view clips.
 */
import { readFileSync } from "node:fs";
import { chromium } from "@playwright/test";

const BASE = process.argv[2] || process.env.BASE || "http://localhost:1420";

/** Sidebar destinations, read from nav.ts so a new view is audited on arrival. */
function navDestinations() {
  const src = readFileSync(new URL("../src/lib/nav.ts", import.meta.url), "utf8");
  const navStart = src.indexOf("export const nav");
  if (navStart < 0) throw new Error("no `export const nav` in nav.ts — the parse is wrong");
  const items = [
    ...src
      .slice(navStart)
      .matchAll(/to:\s*['"`]([^'"`]+)['"`],\s*\n?\s*label:\s*['"`]([^'"`]+)['"`]/g),
  ].map((m) => ({ to: m[1], label: m[2] }));
  if (items.length < 10) {
    throw new Error(`only found ${items.length} nav destinations in nav.ts — the parse is wrong`);
  }
  return items;
}

// A short window, deliberately: clipping is a function of how much room the view
// has, and a tall viewport hides the bug the way it hid #344 until someone
// resized. Anything the app supports at all it must support here.
const VIEWPORT = { width: 1280, height: 720 };
// A couple of pixels of slack for sub-pixel layout; the real failures are large.
const SLACK = 4;

const b = await chromium.launch();
const ctx = await b.newContext({ viewport: VIEWPORT, colorScheme: "dark" });
const p = await ctx.newPage();
const failures = [];

await p.goto(`${BASE}/`, { waitUntil: "networkidle" });
await p.waitForTimeout(700);
const open = p.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) {
  await open.click().catch(() => {});
  // The mock import runs a progress dialog; wait it out rather than racing it.
  await p.waitForTimeout(12_000);
}

/** The shell's clipping wrapper, and how much it is hiding. */
const clipped = () =>
  p.evaluate(() => {
    const el = document.querySelector('[data-slot="view-frame"]');
    if (!el) return { found: false, over: 0 };
    return {
      found: true,
      over: el.scrollHeight - el.clientHeight,
      sh: el.scrollHeight,
      ch: el.clientHeight,
    };
  });

// Views gated behind a modal go LAST: their overlay swallows pointer events, so
// visiting one mid-list would strand every view after it for a reason that has
// nothing to do with scrolling.
const GATED = new Set(["/security", "/safety-scan"]);
const nav = navDestinations();
const routes = [
  { to: "/", label: "Device" },
  ...nav.filter((r) => !GATED.has(r.to)),
  ...nav.filter((r) => GATED.has(r.to)),
];
const skipped = [];
for (const { to, label } of routes) {
  // Some views raise a dialog on arrival (Security's consent gate). Its overlay
  // swallows pointer events, so dismiss whatever is open before navigating on,
  // or every later view fails for a reason that has nothing to do with scroll.
  await p.keyboard.press("Escape");
  await p.waitForTimeout(250);
  // Navigate in-app: the mock's active backup lives in memory, and a full page
  // load would drop it and audit a "No backup open" placeholder instead.
  if (to === "/") {
    // The sidebar's device hero links home. Reloading would drop the mock's
    // in-memory active backup and audit the picker twice instead of the
    // Device view once.
    const hero = p.locator('a[href="/"], a[href="#/"]').first();
    if ((await hero.count()) === 0) {
      failures.push("Device: no in-app link home");
      continue;
    }
    await hero.click();
  } else {
    // Match on the href, not the accessible name: a nav entry can carry a
    // suffix badge that fuses into its name ("SafetyBETA"), and an exact — or
    // even word-boundary — name match then silently skips that view instead of
    // auditing it.
    const link = p.locator(`a[href="${to}"], a[href="#${to}"]`).first();
    if ((await link.count()) === 0) {
      failures.push(`${label}: no sidebar link to click`);
      continue;
    }
    try {
      await link.click({ timeout: 4000 });
    } catch {
      // A consent gate is in front of it. Say so rather than passing quietly —
      // a guard that silently drops a view reads as "checked" when it wasn't.
      skipped.push(`${label} (${to}) — blocked by a modal, not audited`);
      continue;
    }
  }
  await p.waitForTimeout(900);
  const r = await clipped();
  if (!r.found) {
    failures.push(`${label}: could not find the shell's clipping wrapper`);
    continue;
  }
  if (r.over > SLACK) {
    failures.push(
      `${label} (${to}): ${r.over}px of content is clipped and unreachable ` +
        `(${r.sh}px inside a ${r.ch}px wrapper) — the view needs its own scroll container`,
    );
  }
}

await b.close();

for (const s of skipped) console.warn(`  SKIP  ${s}`);

if (failures.length) {
  console.error(`view-scroll check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  FAIL  ${f}`);
  process.exit(1);
}
console.log(
  `view-scroll check passed (${routes.length - skipped.length} of ${routes.length} views).`,
);
