/**
 * Design lint: the visual invariants, measured instead of remembered.
 *
 * Every visual bug this project shipped was looked at in a screenshot and
 * accepted — hover actions covering a count pill, toolbar controls 2px apart,
 * three island heights in one toolbar, a control that changed height once a
 * filter was applied. The eye accepts a 4px mismatch that a measurement rejects
 * instantly, and each one looks defensible alone, which is how they accumulate.
 *
 * So this reads the DOM rather than trusting the render, in the states an idle
 * screenshot never shows: hovered, filtered, search expanded, at the smallest
 * and largest text size.
 *
 * Five rules, each one a bug that actually happened:
 *   type      every rendered font size is a step of the ramp        (13 sizes, 7 ad-hoc)
 *   control   every control height comes from --control-h*          (#91: 34 of 60 hand-tuned)
 *   island    islands are one height, segments another              (#131: 30/36/38 in one bar)
 *   overlap   no interactive element sits on top of another         (#92: actions over the pill)
 *   clipping  no label is cut off by its own box
 *
 * Usage: pnpm dev (or vite --port N), then:
 *   node scripts/check-design.mjs
 *   BASE=http://localhost:1440 node scripts/check-design.mjs
 * Exits non-zero and names every offender.
 */
import { chromium } from "@playwright/test";

const BASE = process.env.BASE || "http://localhost:1420";
const VIEWS = ["Messages", "Notes", "Safety", "Photos", "Contacts"];

/** The type ramp, in px at scale 1 — macOS's text styles (see index.css). */
const RAMP = [10, 11, 12, 13, 15, 17, 22, 26];
/** Control heights from --control-h*. */
const CONTROLS = [20, 24, 28, 32];
const ISLAND = 30;
const SEGMENT = 24;

/**
 * Sizes that are deliberately off the ramp because they are fitted to something
 * rather than chosen typographically. Each is commented at its site; keeping the
 * list here means a NEW off-ramp size fails even though these don't.
 */
const FITTED_TYPE = [
  { px: 11.2, why: "A− glyph, drawn to the stepper icon" },
  { px: 15.2, why: "A+ glyph, drawn to the stepper icon" },
  { px: 8, why: "initials in a half-size group avatar" },
  { px: 16, why: "browser default on non-text nodes (title/style)" },
];

const near = (a, b) => Math.abs(a - b) <= 0.6;
const failures = [];
const fail = (rule, state, msg) => failures.push(`[${rule}] ${state}: ${msg}`);

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1300, height: 900 },
  colorScheme: "dark",
});
const page = await ctx.newPage();

/** Everything the rules need, read in one pass so the states stay consistent. */
const probe = () =>
  page.evaluate(() => {
    const visible = (el) => {
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.opacity !== "0";
    };
    const box = (el) => {
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    };
    // The frame (toolbar, sidebar, dialog action rows) opts out of the text-size
    // control by design (#122), so its sizes must be compared at scale 1 — not
    // multiplied like content. Without this the lint reports the frame as broken
    // at every text size except the default.
    const fixed = (el) => !!el.closest('[data-text-scale="fixed"]');
    const label = (el) =>
      (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.textContent || "")
        .trim().replace(/\s+/g, " ").slice(0, 28) || "(icon)";

    const type = [];
    for (const el of document.querySelectorAll("body *")) {
      if (["STYLE", "SCRIPT", "TITLE", "SVG", "PATH"].includes(el.tagName)) continue;
      if (el.children.length || !el.textContent?.trim() || !visible(el)) continue;
      type.push({ px: parseFloat(getComputedStyle(el).fontSize), text: label(el), fixed: fixed(el) });
    }

    const controls = [];
    for (const el of document.querySelectorAll('[data-slot="button"], input:not([type=checkbox]), select')) {
      if (!visible(el)) continue;
      // Buttons that wrap content (asChild rows, full-width cards) size to their
      // content by design; the scale applies to actual controls.
      const cls = (el.className || "").toString();
      if (/h-auto|w-full/.test(cls) || el.getAttribute("data-slot") === "list-row") continue;
      controls.push({ h: el.getBoundingClientRect().height, name: label(el), cls: cls.slice(0, 40), fixed: fixed(el) });
    }

    const islands = [];
    for (const el of document.querySelectorAll("div")) {
      const cls = (el.className || "").toString();
      if (!/rounded-lg/.test(cls) || !/border/.test(cls) || !/bg-muted/.test(cls)) continue;
      if (!visible(el)) continue;
      islands.push({ h: el.getBoundingClientRect().height, name: label(el), fixed: fixed(el) });
      for (const seg of el.querySelectorAll("button")) {
        if (visible(seg)) islands.push({ h: seg.getBoundingClientRect().height, name: label(seg), segment: true, fixed: fixed(el) });
      }
    }

    // Interactive things that can cover each other. Restricted to siblings-ish
    // scopes (a card, a toolbar) so a popover legitimately over the page is not
    // reported.
    const interactive = [];
    for (const scope of document.querySelectorAll('[data-slot="card"], [data-tauri-drag-region]')) {
      const els = [...scope.querySelectorAll('button, [role="button"], a[href], [data-slot="badge"]')]
        .filter((e) => visible(e) && !e.closest('[role="dialog"]'));
      interactive.push(els.map((e) => ({ ...box(e), name: label(e) })));
    }

    const clipped = [];
    for (const el of document.querySelectorAll("body *")) {
      if (el.children.length || !el.textContent?.trim() || !visible(el)) continue;
      const cs = getComputedStyle(el);
      // truncate/line-clamp cut text ON PURPOSE; this is about boxes too small
      // for text that was meant to fit.
      if (cs.textOverflow === "ellipsis" || cs.overflow !== "visible") continue;
      const el2 = el;
      if (el2.scrollHeight > el2.clientHeight + 1 && el2.clientHeight > 0)
        clipped.push(`${label(el)} (${el2.scrollHeight}px of text in ${el2.clientHeight}px)`);
    }

    return { type, controls, islands, interactive, clipped };
  });

const check = async (state, scale) => {
  const { type, controls, islands, interactive, clipped } = await probe();

  for (const t of type) {
    const s0 = t.fixed ? 1 : scale;
    const want = RAMP.map((px) => px * s0);
    if (want.some((w) => near(t.px, w))) continue;
    if (FITTED_TYPE.some((f) => near(t.px, f.px * s0) || near(t.px, f.px))) continue;
    fail("type", state, `"${t.text}" renders at ${t.px}px — not a step of the ramp`);
  }

  for (const c of controls) {
    if (CONTROLS.some((h) => near(c.h, h * (c.fixed ? 1 : scale)))) continue;
    fail("control", state, `"${c.name}" is ${Math.round(c.h * 10) / 10}px — not a --control-h step (class: ${c.cls})`);
  }

  for (const i of islands) {
    const want = (i.segment ? SEGMENT : ISLAND) * (i.fixed ? 1 : scale);
    if (!near(i.h, want))
      fail("island", state, `${i.segment ? "segment" : "island"} "${i.name}" is ${Math.round(i.h * 10) / 10}px, expected ${Math.round(want * 10) / 10}px`);
  }

  for (const group of interactive) {
    for (let a = 0; a < group.length; a++) {
      for (let b = a + 1; b < group.length; b++) {
        const p = group[a], q = group[b];
        const ox = Math.min(p.x + p.w, q.x + q.w) - Math.max(p.x, q.x);
        const oy = Math.min(p.y + p.h, q.y + q.h) - Math.max(p.y, q.y);
        // Nested elements (a badge inside a button) legitimately overlap.
        const nested = ox >= Math.min(p.w, q.w) - 1 && oy >= Math.min(p.h, q.h) - 1;
        if (ox > 2 && oy > 2 && !nested)
          fail("overlap", state, `"${p.name}" and "${q.name}" overlap by ${Math.round(ox)}×${Math.round(oy)}px`);
      }
    }
  }

  for (const c of clipped) fail("clipping", state, c);
};


/**
 * Prove every detector can fire, on every run.
 *
 * A lint whose rules silently stop matching is worse than no lint: it reports
 * "OK" forever and reads as coverage. So before trusting a clean result, inject
 * one known-bad element per rule and require each rule to report it. If a
 * detector goes quiet — a class name changes, a selector drifts — this fails
 * loudly instead of the suite passing by accident.
 */
const selfTest = async () => {
  await page.evaluate(() => {
    const card = document.querySelector('[data-slot="card"]') || document.body;
    const host = document.createElement("div");
    host.id = "__design_lint_selftest";
    host.style.cssText = "position:relative;height:80px";
    host.innerHTML = `
      <span style="font-size:13.7px">off-ramp type</span>
      <button data-slot="button" style="height:37px">off-scale control</button>
      <div class="rounded-lg border bg-muted/40" style="height:44px">off-scale island</div>
      <button style="position:absolute;left:0;top:0;width:60px;height:20px">under</button>
      <button style="position:absolute;left:10px;top:4px;width:60px;height:20px">over</button>
      <span style="display:block;height:4px;overflow:visible;font-size:13px">clipped text</span>`;
    card.appendChild(host);
  });
  await page.waitForTimeout(150);

  const before = failures.length;
  await check("self-test", 1);
  const fired = new Set(
    failures.slice(before).map((f) => f.slice(1, f.indexOf("]"))),
  );
  failures.length = before; // the injected findings are not real findings

  await page.evaluate(() => document.getElementById("__design_lint_selftest")?.remove());

  const missing = ["type", "control", "island", "overlap", "clipping"].filter((r) => !fired.has(r));
  if (missing.length) {
    console.error(
      `design lint SELF-TEST failed: ${missing.join(", ")} did not fire on a deliberate` +
        ` violation. The detector is broken — a clean run would have meant nothing.`,
    );
    await browser.close();
    process.exit(2);
  }
};

// ---- drive the app through the states an idle screenshot never shows ----
await page.goto(BASE + "/", { waitUntil: "networkidle" });
await page.waitForTimeout(800);
const open = page.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) {
  await open.click().catch(() => {});
  await page.waitForTimeout(1200);
}

await selfTest();

for (const view of VIEWS) {
  await page.getByText(view, { exact: true }).first().click().catch(() => {});
  await page.waitForTimeout(1200);
  const dismiss = page.getByRole("button", { name: /^Got it$/ });
  if (await dismiss.count()) { await dismiss.first().click().catch(() => {}); await page.waitForTimeout(300); }

  await check(view, 1);

  // Hovered — where actions appear and can land on something.
  const row = page.locator('[data-slot="list-row"], [role="button"][aria-current]').first();
  if (await row.count()) {
    await row.hover().catch(() => {});
    await page.waitForTimeout(400);
    await check(`${view}+hover`, 1);
  }

  // Search expanded, and a filter applied.
  const search = page.getByRole("button", { name: /search/i }).first();
  if (await search.count()) {
    await search.click().catch(() => {});
    await page.waitForTimeout(500);
    await check(`${view}+search`, 1);
    await page.keyboard.press("Escape").catch(() => {});
    await page.waitForTimeout(300);
  }
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
      await check(`${view}+filter`, 1);
    } else {
      await page.keyboard.press("Escape").catch(() => {});
    }
  }
}

// The extremes of the text-size control: the ramp scales, the frame does not.
for (const [size, scale] of [["xs", 0.85], ["xl", 1.2]]) {
  await page.evaluate((s) => document.documentElement.setAttribute("data-text-size", s), size);
  await page.waitForTimeout(400);
  await check(`text-${size}`, scale);
}

await browser.close();

if (failures.length) {
  const byRule = failures.reduce((m, f) => {
    const r = f.slice(1, f.indexOf("]"));
    (m[r] ||= []).push(f);
    return m;
  }, {});
  console.error(`design lint failed — ${failures.length} finding(s):\n`);
  for (const [rule, list] of Object.entries(byRule)) {
    console.error(`  ${rule} (${list.length}):`);
    for (const f of [...new Set(list)].slice(0, 8)) console.error(`    ${f}`);
    if (new Set(list).size > 8) console.error(`    … and ${new Set(list).size - 8} more`);
  }
  console.error(
    `\nTake sizes from the tokens (--ramp-*, --control-h*, --island-h) rather than` +
      ` writing literals; see docs/reference/ui.md.`,
  );
  process.exit(1);
}
console.log(
  `design lint OK — type, control heights, island geometry, overlap and clipping,` +
    ` across ${VIEWS.join(" / ")} in idle / hover / search / filter states and at both text extremes.`,
);
