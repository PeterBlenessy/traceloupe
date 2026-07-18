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
const setTheme = () =>
  page.evaluate((t) => {
    document.documentElement.classList.remove("light", "dark");
    document.documentElement.classList.add(t);
  }, THEME);

// Route -> sidebar nav label, so ACTIVATE mode can navigate CLIENT-SIDE (a full
// page.goto would reload the SPA and reset the mock's in-memory "active" flag).
const NAV = {
  "/photos": "Photos", "/messages": "Messages", "/contacts": "Contacts",
  "/calls": "Calls", "/safari": "Safari", "/notes": "Notes",
  "/recordings": "Recordings", "/calendar": "Calendar", "/reminders": "Reminders",
  "/health": "Health", "/interactions": "Interactions", "/apps": "Apps", "/device": "Device",
};

if (process.env.ACTIVATE) {
  // Open a backup once (mock flips to "active"), then stay in the SPA.
  await page.goto(BASE + "/", { waitUntil: "networkidle" }).catch(() => {});
  await page.waitForTimeout(400);
  // Open the UNENCRYPTED backup (last "Read & open") so no password dialog blocks
  // activation; the mock flips to "active" on open.
  const openers = page.getByText(/Read & open/);
  const n = await openers.count();
  await openers.nth(n > 1 ? n - 1 : 0).click({ timeout: 4000 }).catch(() => {});
  await page.waitForTimeout(1400);
  for (const route of routes) {
    if (NAV[route]) await page.getByRole("link", { name: NAV[route], exact: true }).click().catch(() => {});
    await setTheme();
    await page.waitForTimeout(700);
    const name = route.replace(/\//g, "_").replace(/^_/, "") + `.${THEME}.png`;
    await page.screenshot({ path: path.join(OUT, name) });
    console.log("[shot]", route, "->", path.join(OUT, name));
  }
} else {
  for (const route of routes) {
    await page.goto(BASE + route, { waitUntil: "networkidle" }).catch(() => {});
    await setTheme();
    await page.waitForTimeout(600);
    const name = (route === "/" ? "root" : route.replace(/\//g, "_").replace(/^_/, "")) + `.${THEME}.png`;
    await page.screenshot({ path: path.join(OUT, name) });
    console.log("[shot]", route, "->", path.join(OUT, name));
  }
}

await browser.close();
