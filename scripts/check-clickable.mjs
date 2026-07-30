/**
 * Guard: a control that LOOKS clickable must actually receive the click.
 *
 * Being visible is not the same as being hittable. The app's toolbar is an
 * absolutely-positioned translucent overlay at `z-20` and views deliberately
 * scroll *underneath* it, so a control placed at the top of a view can be
 * perfectly visible and completely dead — which is exactly what happened to
 * "Back to Safety Scan" (#224). Nothing caught it, because nothing was looking.
 *
 * It also drives **deep-linked states**, which is the second half of that gap.
 * The design lint reaches views by clicking the sidebar and so only ever sees
 * their ordinary states; a screen that exists only via `?from=safety` had never
 * been rendered by any check at all.
 *
 * The test is `document.elementFromPoint` at each control's centre: if the
 * topmost element there is not the control or inside it, a real click would land
 * somewhere else.
 *
 *   node scripts/check-clickable.mjs [baseUrl]
 */
import { chromium } from "@playwright/test";

const BASE = process.argv[2] ?? process.env.BASE ?? "http://localhost:5173";

/**
 * States to check. `nav` is a sidebar label to click; `deep` is a URL pushed
 * client-side, which is the only way to reach a deep-link state — a full
 * page.goto resets the mock's in-memory "a backup is open" flag and every view
 * falls back to its no-backup screen, where none of these controls exist.
 */
const STATES = [
  { name: "Messages", nav: "Messages" },
  { name: "Notes", nav: "Notes" },
  { name: "Safety", nav: "Safety" },
  { name: "Security", nav: "Security" },
  { name: "Apps", nav: "Apps" },
  { name: "Photos", nav: "Photos" },
  // The states #224 lives in — reachable only by deep link, never measured
  // before. Both views render a "Back to Safety Scan" bar at the very top,
  // which is precisely where the toolbar overlay sits.
  {
    name: "Messages?from=safety",
    deep: "/messages?from=safety",
    expect: "Back to Safety Scan",
  },
  { name: "Notes?from=safety", deep: "/notes?from=safety", expect: "Back to Safety Scan" },
  // The state a user actually lands in: a finding opens the CONVERSATION, not
  // the thread list. Without a `thread` the view shows the list, so the
  // conversation's own list — with its own `underlap` — went unchecked. A
  // mutation reintroducing the bug there survived until this state was added.
  {
    name: "Messages?thread&from=safety",
    deep: "/messages?thread=1&from=safety",
    expect: "Back to Safety Scan",
  },
  // Messages has two modes and each renders its OWN list with its own
  // `underlap`. A mutation reintroducing the bug in the Timeline list survived
  // both states above, because neither switches mode — so the state is reached
  // by clicking the mode control, as a user would.
  {
    name: "Messages timeline?from=safety",
    deep: "/messages?from=safety",
    then: "Timeline",
    expect: "Back to Safety Scan",
  },
];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1400 } });
const failures = [];
let measured = 0;
let controlsChecked = 0;

await page.goto(`${BASE}/`, { waitUntil: "networkidle" });
await page.waitForTimeout(800);
const open = page.getByRole("button", { name: /Read & open/ }).last();
if (await open.count()) {
  await open.click().catch(() => {});
  await page.waitForTimeout(1500);
}

for (const state of STATES) {
  if (state.nav) {
    await page.getByText(state.nav, { exact: true }).first().click().catch(() => {});
  } else {
    // Client-side navigation, so the mock's open backup survives. A popstate is
    // what makes the router notice.
    await page.evaluate((url) => {
      window.history.pushState({}, "", url);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, state.deep);
  }
  await page.waitForTimeout(900);

  // Some states need a control clicked after arriving — a mode switch, say.
  if (state.then) {
    await page.getByLabel(state.then, { exact: true }).first().click().catch(() => {});
    await page.waitForTimeout(900);
  }

  // Prove we are somewhere. A silent navigation failure would otherwise read as
  // a state with no problems.
  const url = page.url();
  if (state.deep && !url.includes(state.deep.split("?")[1] ?? "")) {
    failures.push(`${state.name}: never reached (url is ${url})`);
    continue;
  }

  const { out: results, skipped, scrolled } = await page.evaluate(() => {
    const out = [];
    const skipped = [];
    const scrolled = [];
    const controls = document.querySelectorAll(
      'button, [role="button"], a[href], [role="tab"], [role="menuitem"]',
    );
    for (const el of controls) {
      const r = el.getBoundingClientRect();
      if (r.width < 4 || r.height < 4) continue;
      // A control whose CENTRE is off-viewport cannot be hit-tested — and
      // reporting it as "covered by nothing" is the check being naive, not a
      // bug. The first version of this did exactly that and blamed a sidebar
      // item below the fold. Those are counted and reported separately, so the
      // check is honest about what it did not look at.
      const cxRaw = r.left + r.width / 2;
      const cyRaw = r.top + r.height / 2;
      if (cxRaw < 0 || cxRaw > window.innerWidth || cyRaw < 0 || cyRaw > window.innerHeight) {
        skipped.push((el.textContent || el.getAttribute("aria-label") || "?").trim().slice(0, 40));
        continue;
      }
      const cs = getComputedStyle(el);
      if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") continue;
      if (el.hasAttribute("disabled") || el.getAttribute("aria-disabled") === "true") continue;
      if (cs.pointerEvents === "none") continue;

      // Only PINNED controls are checked — chrome, bars, toolbars. Content
      // inside a scrollable region is a different case: lists deliberately
      // scroll beneath the translucent title bar, so their topmost rows are
      // always partly covered and the user scrolls. Flagging those is the check
      // being wrong, not the app: it reported message bubbles at the top of a
      // timeline. Every bug this exists for — #224's return bar, the Apps
      // extraction prompt — is a pinned control outside the scroller.
      let inScroller = false;
      for (let a = el.parentElement; a; a = a.parentElement) {
        const as = getComputedStyle(a);
        if (as.overflowY === "auto" || as.overflowY === "scroll") {
          inScroller = true;
          break;
        }
      }
      if (inScroller) {
        scrolled.push((el.textContent || "?").trim().slice(0, 40));
        continue;
      }

      const cx = Math.round(r.left + r.width / 2);
      const cy = Math.round(r.top + r.height / 2);
      const top = document.elementFromPoint(cx, cy);
      const reachable = top && (el === top || el.contains(top) || top.contains(el));
      if (!reachable) {
        const label = (el.textContent || el.getAttribute("aria-label") || "?")
          .trim()
          .slice(0, 44);
        const blocker = top
          ? `${top.tagName.toLowerCase()}${top.className ? "." + String(top.className).split(" ")[0] : ""}`
          : "nothing";
        out.push({ label, blocker, cx, cy });
      }
    }
    return { out, skipped, scrolled };
  });

  // A deep-linked state that renders none of the control it exists to show is a
  // navigation failure dressed up as a pass.
  if (state.expect) {
    const found = await page.evaluate(
      (needle) =>
        [...document.querySelectorAll("button")].some((b) =>
          (b.textContent || "").includes(needle),
        ),
      state.expect,
    );
    if (!found) {
      failures.push(`${state.name}: expected a "${state.expect}" control and found none`);
    }
  }

  measured += 1;
  const total = await page.evaluate(
    () => document.querySelectorAll('button, [role="button"], a[href]').length,
  );
  controlsChecked += total;

  const notes = [];
  if (skipped.length) notes.push(`${skipped.length} off-screen`);
  if (scrolled.length) notes.push(`${scrolled.length} inside a scroller`);
  const note = notes.length ? ` — ${notes.join(", ")}, not checked` : "";
  if (results.length === 0) {
    console.log(
      `  ok    ${state.name} — every on-screen control is hittable (${total} controls)${note}`,
    );
  } else {
    for (const r of results) {
      console.log(`  FAIL  ${state.name}: "${r.label}" is covered by <${r.blocker}>`);
      failures.push(
        `${state.name}: control "${r.label}" is visible but a click at (${r.cx},${r.cy}) ` +
          `would hit <${r.blocker}> instead`,
      );
    }
  }
}

await browser.close();

// A check that quietly measured nothing reports the same OK as one that passed.
if (measured !== STATES.length) {
  failures.push(`measured ${measured} of ${STATES.length} states`);
}
if (controlsChecked < STATES.length) {
  failures.push(`found only ${controlsChecked} controls across all states — did the app render?`);
}

if (failures.length) {
  console.error(`\nclickable check FAILED (${failures.length}):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `\nclickable OK — ${controlsChecked} controls across ${measured} states, ` +
    `including deep-linked ones, are all reachable by a real click.`,
);
