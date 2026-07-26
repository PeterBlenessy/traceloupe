/**
 * Verifies that the app's unbounded lists really are virtualized (#67).
 *
 * The failure this guards against is not slowness: in #61 one list rendering
 * ~8000 rows drove the WebKit render process to 99% CPU and 3.1 GB and froze the
 * laptop. A 5-row fixture cannot catch that, so this inflates the mock client's
 * fixtures to thousands of rows (via the `traceloupe-mock-bulk` localStorage
 * knob) and asserts each list mounts only a windowful.
 *
 * Mounted-row count alone is not proof — it looks identical when the fixture
 * failed to inflate. So it also reads the virtualizer's scroll height: a few
 * mounted rows against a scroll height of thousands is the actual evidence.
 *
 * Usage: pnpm dev  (or vite --port N), then:
 *   node scripts/check-virtualization.mjs           # expects localhost:1420
 *   BASE=http://localhost:1427 node scripts/check-virtualization.mjs
 * Exits non-zero if any audited list stops virtualizing.
 */
import { chromium } from "@playwright/test";
const BULK = Number(process.env.BULK || 4000);
const BASE = process.env.BASE || "http://localhost:1420";
const b = await chromium.launch();
const ctx = await b.newContext({ viewport: { width: 1300, height: 880 }, colorScheme: "dark" });
const p = await ctx.newPage();
const errs = [];
p.on("console", (m) => { if (m.type() === "error") errs.push(m.text().slice(0, 140)); });
await p.addInitScript((n) => localStorage.setItem("traceloupe-mock-bulk", String(n)), BULK);
await p.goto(BASE + "/", { waitUntil: "networkidle" });
await p.waitForTimeout(700);
const open = p.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) { await open.click().catch(()=>{}); await p.waitForTimeout(1100); }

// Mounted-row count alone can't tell "virtualized" from "the fixture didn't
// inflate". The virtualizer sizes its spacer to the FULL list, so a scrollHeight
// of thousands of rows next to a handful of mounted rows is the actual proof.
const stats = () => p.evaluate(() => {
  const rows = [...document.querySelectorAll('[data-slot="list-row"], [role="button"][aria-current]')];
  let scrollH = 0;
  for (const r of rows) {
    for (let el = r.parentElement; el; el = el.parentElement) {
      if (el.scrollHeight > el.clientHeight + 40) { scrollH = Math.max(scrollH, el.scrollHeight); break; }
    }
  }
  return { rows: rows.length, total: document.querySelectorAll("*").length, scrollH };
});

// --- Safety Scan history rail ---
await p.getByText("Safety", { exact: true }).first().click().catch(()=>{});
await p.waitForSelector('[role="button"][aria-current]', { timeout: 15000 });
await p.waitForTimeout(500);
const safety = await stats();

// --- Security: consent (which runs the first check), then runs + findings ---
await p.getByText("Security", { exact: true }).first().click().catch(()=>{});
await p.waitForTimeout(1200);
// Dismiss whatever gate is up, loudly — a silent catch here is how this check
// spent three runs "passing" against an empty view.
for (let round = 0; round < 3; round++) {
  const btns = await p.evaluate(() => {
    const scope = document.querySelector('[role="dialog"]') || document.querySelector("main") || document.body;
    return [...scope.querySelectorAll("button")].map((b) => (b.textContent || "").trim()).filter(Boolean);
  });
  if (!btns.length) break;
  console.log(`  dialog buttons: ${btns.join(" | ")}`);
  const label = btns.find((t) => /save|run|continue|enable|got it|ok/i.test(t)) ?? btns[btns.length - 1];
  await p.getByRole("button", { name: label, exact: true }).first().click();
  await p.waitForTimeout(1500);
}
for (let i = 0; i < 15; i++) {
  await p.waitForTimeout(1000);
  const st = await p.evaluate(() => ({
    rows: document.querySelectorAll('[data-slot="list-row"]').length,
    aria: document.querySelectorAll('[role="button"][aria-current]').length,
    txt: (document.querySelector("main")?.innerText || "").replace(/\s+/g, " ").slice(0, 120),
  }));
  console.log(`  security t+${i + 1}s listRows=${st.rows} runRows=${st.aria} :: ${st.txt}`);
  if (st.rows > 0) break;
}
const security = await stats();

const line = (l, s) =>
  `${l.padEnd(10)} rows mounted=${String(s.rows).padEnd(4)} of ${BULK} fixture rows` +
  `   virtual scroll height=${s.scrollH}px (~${Math.round(s.scrollH / 60)} rows)   total DOM nodes=${s.total}`;
console.log(line("safety", safety));
console.log(line("security", security));
console.log(errs.length ? `console errors: ${errs.slice(0,3).join(" | ")}` : "no console errors");
const ok =
  safety.rows > 0 && safety.rows < 200 && safety.scrollH > 50000 &&
  security.rows > 0 && security.rows < 200 && security.scrollH > 50000;
console.log(
  `VERDICT: ${ok ? "both lists virtualize — a few rows mounted against a scroll height of thousands" : "FAILED"}`,
);
await b.close();
process.exit(ok ? 0 : 1);
