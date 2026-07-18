/**
 * Headless screenshot harness for visual iteration. Renders the app in Chromium
 * against the running vite dev server (which uses the mock IPC client, so views
 * are populated). Captures the webview content — NOT the native macOS chrome
 * (traffic lights / Overlay titlebar), which only a real-window capture shows.
 *
 * Usage: node scripts/shot.mjs [route ...]   (default: a spread of views)
 *   node scripts/shot.mjs / /calendar /messages
 * Env: BASE=http://localhost:1420  THEME=dark|light  OUT=/path/dir
 */
import { chromium } from "@playwright/test";
import { mkdirSync } from "node:fs";
import path from "node:path";

const BASE = process.env.BASE || "http://localhost:1420";
const THEME = process.env.THEME || "dark";
const OUT = process.env.OUT || "/tmp/traceloupe-shots";
const routes = process.argv.slice(2).length ? process.argv.slice(2) : ["/", "/calendar", "/messages", "/photos"];
mkdirSync(OUT, { recursive: true });

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1100, height: 750 },
  deviceScaleFactor: 2,
  colorScheme: THEME === "light" ? "light" : "dark",
});
const page = await ctx.newPage();

// Optionally open a backup first (the mock client flips to "active" after an
// open), so artifact views render populated instead of the no-backup state.
if (process.env.ACTIVATE) {
  await page.goto(BASE + "/", { waitUntil: "networkidle" }).catch(() => {});
  await page.waitForTimeout(400);
  // Click the first backup card's open affordance.
  const opener = page.getByText(/Read & open|^Open$/).first();
  await opener.click({ timeout: 4000 }).catch(() => {});
  await page.waitForTimeout(1200);
}

for (const route of routes) {
  const url = BASE + route;
  await page.goto(url, { waitUntil: "networkidle" }).catch(() => {});
  // Force the theme class the app toggles on <html>.
  await page.evaluate((t) => {
    document.documentElement.classList.remove("light", "dark");
    document.documentElement.classList.add(t);
  }, THEME);
  await page.waitForTimeout(600);
  const name = (route === "/" ? "root" : route.replace(/\//g, "_").replace(/^_/, "")) + `.${THEME}.png`;
  const file = path.join(OUT, name);
  await page.screenshot({ path: file });
  console.log("[shot]", route, "->", file);
}

await browser.close();
