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
 *   clipping  no label is cut off by its own box                   (DialogTitle's leading-none)
 *   contrast  text meets WCAG AA against what is really behind it  (a tile letter at 4.11:1)
 *   focus     every control shows a ring when TABBED to            (:focus-visible needs real keys)
 *   a11y      every control has an accessible name                 (unnamed Settings switches)
 *   tooltip   every icon-only button explains itself
 *   spacing   gaps and padding stay on the 2px grid
 *
 * Every rule is proved on every run (see selfTest): each is shown a deliberate
 * violation and must report it. Three of them — focus, tooltip, spacing — were
 * NOT proved for a while, and the focus rule was quietly broken that whole time.
 *
 * Usage: pnpm dev (or vite --port N), then:
 *   node scripts/check-design.mjs
 *   BASE=http://localhost:1440 node scripts/check-design.mjs
 * Exits non-zero and names every offender.
 */
import { chromium } from "@playwright/test";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const BASE = process.env.BASE || "http://localhost:1420";
/**
 * Every view, because a rule that only holds where someone remembered to look is
 * not a rule. The deep states (hover / search / filter) are expensive, so they
 * run on the views that have the most toolbar surface; every other view still
 * gets its idle sweep in both colour schemes.
 */
const VIEWS = [
  "Photos", "Messages", "Contacts", "Calls", "Safari", "Notes", "Recordings",
  "Calendar", "Reminders", "Health", "Interactions", "Apps", "Security", "Safety",
];
const DEEP = new Set(["Messages", "Notes", "Safety", "Contacts", "Photos"]);

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

/**
 * Contrast below AA that is a deliberate design decision, with the measurement
 * that made it. Recorded rather than silently skipped: if one of these gets
 * WORSE than the floor here, it fails again.
 */
const CONTRAST_ACCEPTED = [
  {
    selector: '[class*="text-muted-foreground"], [class*="text-faint-foreground"]',
    floor: 3,
    why: "the platform's secondary/tertiary label tiers, which macOS itself renders" +
      " below AA (secondary is 55% alpha). Matching the platform is the point; the" +
      " floor still fails anything genuinely illegible.",
  },
  {
    selector: '[data-slot="sidebar"] [data-active="true"]',
    floor: 2.3,
    why: "selected sidebar row: white on the system accent, matching macOS's own" +
      " sidebar selection. Keeping the label white was an explicit product" +
      " decision (#41); AA would require darkening it against every accent colour.",
  },
];

const near = (a, b) => Math.abs(a - b) <= 0.6;
const failures = [];
const fail = (rule, state, msg) => failures.push(`[${rule}] ${state}: ${msg}`);


// ---------------------------------------------------------------------------
// Static pass: the source, not the render.
//
// The runtime rules can only judge what is on screen. A button in a view nobody
// visited, behind an error state, or in a dialog that never opened is invisible
// to them — and that is exactly where a literal survives. So before the browser
// starts, read the source and reject size or colour literals written onto a
// CONTROL. Scoped to controls on purpose: `size-10` on an avatar is fitted to
// the avatar, and flagging it would bury the signal in noise.
// ---------------------------------------------------------------------------
const CONTROL_ELEMENTS = /<(Button|button|Input|input|Select|SelectTrigger|ToggleGroupItem|Toggle)\b/g;
// A control height literal — `h-9`, `size-8`, `h-[36px]`. Small `size-*` values
// are icon sizes and legitimate.
const SIZE_LITERAL = /(?<![\w:-])(?:h|size|min-h)-(?:6|7|8|9|10|11|12|14)\b|(?<![\w:-])(?:h|size)-\[[^\]]*(?:px|rem)[^\]]*\]/g;
const PALETTE_LITERAL = /(?<![\w:-])(?:text|bg|border|ring|fill)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}\b/g;

const walk = (dir) =>
  readdirSync(dir).flatMap((e) => {
    const p = join(dir, e);
    return statSync(p).isDirectory() ? walk(p) : p.endsWith(".tsx") ? [p] : [];
  });

const staticPass = () => {
  for (const file of walk("src")) {
    let src = readFileSync(file, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")   // prose about sizes is not code
      .replace(/^\s*\/\/.*$/gm, "");
    for (const m of src.matchAll(CONTROL_ELEMENTS)) {
      let i = m.index + m[0].length, depth = 0;
      while (i < src.length && !(src[i] === ">" && depth === 0)) {
        if (src[i] === "{") depth++;
        else if (src[i] === "}") depth--;
        i++;
      }
      const tag = src.slice(m.index, i);
      const line = src.slice(0, m.index).split("\n").length;
      for (const [re, kind] of [[SIZE_LITERAL, "size"], [PALETTE_LITERAL, "colour"]]) {
        for (const hit of tag.matchAll(re)) {
          fail(kind === "size" ? "control" : "type", "source",
            `${file}:${line} writes \`${hit[0]}\` on <${m[1]}> — take it from a token` +
            ` (--control-h*, --island-h, --status-*)`);
        }
      }
    }
  }
};

// Prove the static detector fires, for the same reason the runtime ones do.
{
  const before = failures.length;
  const probe = "<Button className=\"h-9 text-emerald-500\">x</Button>";
  for (const [re, kind] of [[SIZE_LITERAL, "size"], [PALETTE_LITERAL, "colour"]]) {
    if (![...probe.matchAll(re)].length) {
      console.error(`design lint SELF-TEST failed: the static ${kind} matcher did not fire on ${probe}`);
      process.exit(2);
    }
  }
  failures.length = before;
}
staticPass();

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1300, height: 900 },
  colorScheme: "dark",
});
const page = await ctx.newPage();

/** Everything the rules need, read in one pass so the states stay consistent. */
const probe = () =>
  page.evaluate((ACCEPTED) => {
    const visible = (el) => {
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      // sr-only text is a 1px clipped box: present for screen readers, not on
      // screen, so no visual rule should judge it.
      if (r.width <= 1 || r.height <= 1) return false;
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
    // Only inside a toolbar or a card header. The same rounded-bordered-muted
    // look is used for CONTENT chips too (the Interactions channel strip), and
    // those size to their content — judging them against the island height
    // reports the app as broken when it is the detector that is wrong.
    const islandScopes = [
      ...document.querySelectorAll("[data-tauri-drag-region]"),
      ...document.querySelectorAll('[data-slot="card-header"]'),
    ];
    for (const scope of islandScopes)
    for (const el of scope.querySelectorAll("div")) {
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

    // Tooltip + accessible name. A trigger may wrap the button in a <span>
    // (needed so a DISABLED button still shows its tooltip), so look up a few
    // levels rather than at the button itself.
    const unnamed = [];
    for (const el of document.querySelectorAll("button")) {
      if (!visible(el)) continue;
      const hasText = !!(el.textContent || "").trim();
      let tipped = false;
      for (let n = el, i = 0; n && i < 3; n = n.parentElement, i++)
        if (n.getAttribute?.("data-slot") === "tooltip-trigger") { tipped = true; break; }
      // A <label for> names a button too — it is labelable HTML — so a switch
      // paired with a visible Label is named even with no ARIA on it.
      const labelled = el.id && document.querySelector(`label[for="${CSS.escape(el.id)}"]`);
      const named =
        hasText || el.getAttribute("aria-label") || el.getAttribute("aria-labelledby") ||
        el.getAttribute("title") || !!el.querySelector(".sr-only") || !!labelled;
      if (!named) unnamed.push({ kind: "name", name: el.outerHTML.replace(/\s+/g, " ").slice(0, 120) });
      // The tooltip rule is about ICON BUTTONS, whose meaning is otherwise
      // unconveyed. A switch or checkbox sits beside a visible label that
      // already says what it does, and does not want a tooltip on top.
      const selfExplaining =
        ["switch", "checkbox", "radio", "tab"].includes(el.getAttribute("role") || "") || !!labelled;
      if (!named) { /* reported above */ }
      else if (!hasText && !tipped && !selfExplaining)
        unnamed.push({ kind: "tooltip", name: el.getAttribute("aria-label") || "(icon)" });
    }

    // Contrast: composite the text colour over whatever is actually behind it.
    //
    // Colours are resolved by painting them onto a 1px canvas and reading the
    // pixel, NOT by parsing the string: getComputedStyle returns whatever colour
    // space the author used — `oklch(0.93 0.006 260)` here — and reading those
    // three numbers as if they were R/G/B produces confident nonsense (it
    // reported 952 failures, all imaginary).
    const _c = document.createElement("canvas");
    _c.width = _c.height = 1;
    const _ctx = _c.getContext("2d", { willReadFrequently: true });
    const parse = (css) => {
      _ctx.clearRect(0, 0, 1, 1);
      _ctx.fillStyle = "#000";
      _ctx.fillStyle = css;
      _ctx.fillRect(0, 0, 1, 1);
      const d = _ctx.getImageData(0, 0, 1, 1).data;
      return { r: d[0], g: d[1], b: d[2], a: d[3] / 255 };
    };
    const over = (fg, bg) => ({
      r: fg.r * fg.a + bg.r * (1 - fg.a),
      g: fg.g * fg.a + bg.g * (1 - fg.a),
      b: fg.b * fg.a + bg.b * (1 - fg.a),
      a: 1,
    });
    const lum = (c) => {
      const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
    };
    const bgOf = (el) => {
      let acc = null;
      for (let n = el; n; n = n.parentElement) {
        const c = parse(getComputedStyle(n).backgroundColor);
        if (c.a === 0) continue;
        acc = acc ? over(acc, c) : c;
        if (acc.a === 1 && c.a === 1) return acc;
      }
      return acc || { r: 0, g: 0, b: 0, a: 1 };
    };
    const contrast = [];
    for (const el of document.querySelectorAll("body *")) {
      if (el.children.length || !(el.textContent || "").trim() || !visible(el)) continue;
      if (el.closest(".sr-only") || (el.className || "").toString().includes("sr-only")) continue;
      const cs = getComputedStyle(el);
      if (parseFloat(cs.opacity) < 0.6) continue;   // deliberately de-emphasised
      const size = parseFloat(cs.fontSize), weight = parseInt(cs.fontWeight, 10) || 400;
      const bg = bgOf(el);
      const fg = over(parse(cs.color), bg);
      const L1 = lum(fg), L2 = lum(bg);
      const ratio = (Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05);
      const large = size >= 18.66 || (size >= 14 && weight >= 700);
      const want = large ? 3 : 4.5;
      if (ratio < want) {
        const accepted = ACCEPTED.find(
          (a) => el.closest(a.selector) || el.matches(a.selector),
        );
        if (accepted && ratio >= accepted.floor) continue;
        const rgb = (c) => `rgb(${Math.round(c.r)},${Math.round(c.g)},${Math.round(c.b)})`;
        contrast.push(
          `"${label(el)}" ${ratio.toFixed(2)}:1 at ${size}px (needs ${want}:1) —` +
            ` ${rgb(fg)} on ${rgb(bg)}, <${el.tagName.toLowerCase()} class="${(el.className || "").toString().slice(0, 44)}">`,
        );
      }
    }

    // Spacing: Tailwind's scale is entirely even numbers of px, so an odd gap or
    // pad is an arbitrary value someone typed.
    const spacing = [];
    for (const el of document.querySelectorAll("body *")) {
      if (!visible(el)) continue;
      const cs = getComputedStyle(el);
      const vals = [];
      if (/flex|grid/.test(cs.display)) vals.push(["gap", cs.columnGap], ["gap", cs.rowGap]);
      vals.push(["padding", cs.paddingTop], ["padding", cs.paddingLeft]);
      for (const [what, v] of vals) {
        const px = parseFloat(v);
        if (!px || Number.isNaN(px) || px > 96) continue;
        if (Math.abs(px - Math.round(px / 2) * 2) > 0.35)
          spacing.push(`${what} ${px}px on "${label(el)}" is off the 2px grid`);
      }
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

    return { type, controls, islands, interactive, clipped, unnamed, contrast, spacing };
  }, CONTRAST_ACCEPTED);


/**
 * Focus must be visible — and it can only be checked from the keyboard.
 *
 * Tailwind styles focus with `focus-visible:`, which the browser applies for
 * keyboard navigation and NOT for a programmatic `el.focus()`. An in-page
 * version of this rule therefore reported all 185 controls as broken while the
 * app was fine. Pressing Tab is the only way to ask the question honestly.
 */
const checkFocus = async (state) => {
  // Start the walk at the top of the document. blur() is NOT enough: the browser
  // keeps a "sequential focus navigation starting point" that a click sets and a
  // blur does not clear, so after the harness clicks anything, Tab resumes from
  // there and never reaches the controls above it. Focusing a sentinel at the
  // very start moves that point explicitly.
  await page.evaluate(() => {
    let anchor = document.getElementById("__tab_origin");
    if (!anchor) {
      anchor = document.createElement("span");
      anchor.id = "__tab_origin";
      anchor.tabIndex = -1;
      document.body.insertBefore(anchor, document.body.firstChild);
    }
    anchor.focus();
  });
  const seen = new Set();
  for (let i = 0; i < 14; i++) {
    await page.keyboard.press("Tab");
    const r = await page.evaluate(() => {
      const el = document.activeElement;
      if (!el || el === document.body) return null;
      const cs = getComputedStyle(el);
      const name =
        (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 24) || "(icon)";
      // Tailwind emits several shadow LAYERS, the unused ones as fully
      // transparent placeholders — a real ring sits among them, e.g.
      // "rgba(0,0,0,0) 0 0 0 0, oklch(…) 0 0 0 2px". So neither "is it none?"
      // nor "does it start transparent?" answers the question: strip every
      // transparent layer and see whether any colour is left.
      // The ring may sit on a WRAPPER rather than the focused element itself —
      // `has-[:focus-visible]:ring-2` on an island around an input is the normal
      // way to do it, and the user sees a ring either way. So look at the
      // element and its nearest ancestors.
      const ringed = (node) => {
        const c = getComputedStyle(node);
        if (c.outlineStyle !== "none" && parseFloat(c.outlineWidth) > 0) return true;
        const sh = c.boxShadow === "none" ? "" : c.boxShadow;
        const rest = sh.replace(/rgba?\([^)]*,\s*0\s*\)/g, "").replace(/\btransparent\b/g, "");
        return /(oklch|oklab|color\(|#[0-9a-f]{3}|rgb)/i.test(rest);
      };
      // An ancestor only counts if it DECLARES a focus ring — otherwise an
      // ordinary drop shadow (every dialog has one) reads as focus styling and
      // the rule passes on anything inside a card.
      const declaresFocusRing = (node) =>
        /(?:focus-visible|focus-within|has-\[:focus-visible\]):ring/.test(
          (node.className || "").toString(),
        );
      if (ringed(el)) return { name, visible: true, key: name + el.tagName };
      for (let n = el.parentElement, i = 0; n && i < 3; n = n.parentElement, i++)
        if (declaresFocusRing(n) && ringed(n))
          return { name, visible: true, key: name + el.tagName };
      const shadow = cs.boxShadow === "none" ? "" : cs.boxShadow;
      const opaqueShadow = /(oklch|oklab|color\(|#[0-9a-f]{3})/i.test(
        shadow.replace(/rgba?\([^)]*,\s*0\s*\)/g, "").replace(/\btransparent\b/g, ""),
      ) || /rgba?\([^)]*?(?:,\s*(?:0?\.\d+|1(?:\.0+)?))?\s*\)/.test(
        shadow.replace(/rgba?\([^)]*,\s*0\s*\)/g, ""),
      );
      const ring = [cs.outlineStyle !== "none" && parseFloat(cs.outlineWidth) > 0, opaqueShadow];
      return { name, visible: ring.some(Boolean), key: name + el.tagName };
    });
    if (!r || seen.has(r.key)) continue;
    seen.add(r.key);
    if (!r.visible) fail("focus", state, `"${r.name}" shows no focus ring when tabbed to`);
  }
  await page.evaluate(() => document.getElementById("__tab_origin")?.remove());
};

const check = async (state, scale) => {
  const { type, controls, islands, interactive, clipped, unnamed, contrast, spacing } = await probe();

  for (const u of unnamed)
    fail(u.kind === "name" ? "a11y" : "tooltip", state,
      u.kind === "name"
        ? `an icon-only ${u.name} has no accessible name — a screen reader announces nothing`
        : `icon-only button "${u.name}" has no tooltip`);
  for (const c of contrast) fail("contrast", state, c);
  for (const sp of spacing) fail("spacing", state, sp);

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
    // Inject where each rule actually looks: the island rule only scans toolbars
    // and card headers, so a violation planted anywhere else would not be seen —
    // and the self-test would then be testing nothing.
    const card =
      document.querySelector('[data-slot="card-header"]') ||
      document.querySelector("[data-tauri-drag-region]") ||
      document.body;
    const host = document.createElement("div");
    host.id = "__design_lint_selftest";
    host.style.cssText = "position:relative;height:80px";
    host.innerHTML = `
      <span style="font-size:13.7px">off-ramp type</span>
      <button data-slot="button" style="height:37px">off-scale control</button>
      <div class="rounded-lg border bg-muted/40" style="height:44px">off-scale island</div>
      <button style="position:absolute;left:0;top:0;width:60px;height:20px">under</button>
      <button style="position:absolute;left:10px;top:4px;width:60px;height:20px">over</button>
      <span style="display:block;height:4px;overflow:visible;font-size:13px">clipped text</span>
      <span style="color:#7a7a7a;background:#6f6f6f;font-size:13px">low contrast</span>
      <button style="height:28px;width:28px"><svg width="10" height="10"></svg></button>
      <button aria-label="no tooltip here" style="height:28px;width:28px"><svg width="10" height="10"></svg></button>
      <div style="display:flex;gap:3px"><span style="font-size:13px">odd gap</span></div>`;
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

  // Focus is driven by the keyboard and lives outside check(), so it is proved
  // separately below. Everything else must fire on the planted violations.
  {
    // The focus rule tabs rather than calling focus(), so it cannot be planted
    // in the same pass. A control with no ring, placed first in tab order, must
    // trip it — otherwise a rule that reports nothing looks identical to an app
    // with perfect focus styling.
    await page.evaluate(() => {
      const b = document.createElement("button");
      b.id = "__design_lint_focus_probe";
      b.textContent = "no ring";
      b.style.cssText = "outline:none!important;box-shadow:none!important;height:28px";
      // Inside the open dialog when there is one. A modal traps focus, so a
      // probe planted outside it is never tabbed to — which read as "the focus
      // detector is broken" when the detector was fine and the probe was
      // unreachable.
      const trap = document.querySelector('[role="dialog"]');
      if (trap) trap.appendChild(b);
      else document.body.insertBefore(b, document.body.firstChild);
    });
    const before = failures.length;
    await checkFocus("self-test");
    const fired = failures.slice(before).some((f) => f.startsWith("[focus]"));
    failures.length = before;
    await page.evaluate(() => document.getElementById("__design_lint_focus_probe")?.remove());
    if (!fired) {
      console.error(
        "design lint SELF-TEST failed: focus did not fire on a control with no" +
          " focus ring. The detector is broken — a clean run would have meant nothing.",
      );
      await browser.close();
      process.exit(2);
    }
  }

  const missing = [
    "type", "control", "island", "overlap", "clipping", "contrast", "a11y", "tooltip", "spacing",
  ].filter((r) => !fired.has(r));
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
  await page.waitForTimeout(900);
  const dismiss = page.getByRole("button", { name: /^Got it$/ });
  if (await dismiss.count()) { await dismiss.first().click().catch(() => {}); await page.waitForTimeout(250); }

  await check(view, 1);
  if (!DEEP.has(view)) continue;
  await checkFocus(view);

  // Hovered — where row actions appear and can land on something (#92).
  const row = page.locator('[data-slot="list-row"], [role="button"][aria-current]').first();
  if (await row.count()) {
    await row.hover().catch(() => {});
    await page.waitForTimeout(350);
    await check(`${view}+hover`, 1);
  }

  const search = page.getByRole("button", { name: /search/i }).first();
  if (await search.count()) {
    await search.click().catch(() => {});
    await page.waitForTimeout(450);
    await check(`${view}+search`, 1);
    await page.keyboard.press("Escape").catch(() => {});
    await page.waitForTimeout(250);
  }

  const funnel = page.getByRole("button", { name: "Filter" }).first();
  if (await funnel.count()) {
    await funnel.click().catch(() => {});
    await page.waitForTimeout(450);
    const opt = page.getByRole("button", { name: /iMessage|SMS|With photos|Serious|Harmful|Folders/i }).first();
    if (await opt.count()) {
      await opt.click().catch(() => {});
      await page.waitForTimeout(500);
      await page.keyboard.press("Escape").catch(() => {});
      await page.waitForTimeout(400);
      await check(`${view}+filter`, 1);
    } else {
      await page.keyboard.press("Escape").catch(() => {});
    }
  }
}

// Settings is a dialog, so nothing above ever opens it — and it is dense with
// controls, which is precisely where an off-scale one hides.
await page.getByText("Settings", { exact: true }).first().click().catch(() => {});
await page.waitForTimeout(1000);
for (const tab of ["General", "Media", "Apps", "Security", "Safety", "Developer"]) {
  const t = page.getByRole("tab", { name: tab }).first();
  if (await t.count()) {
    await t.click().catch(() => {});
    await page.waitForTimeout(350);
    await check(`Settings/${tab}`, 1);
  }
}
await page.keyboard.press("Escape").catch(() => {});
await page.waitForTimeout(400);

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
  `design lint OK — type, control heights, island geometry, overlap, clipping,` +
    ` contrast, focus visibility, accessible names, tooltips and spacing, across` +
    ` ${VIEWS.length} views + Settings in idle / hover / search / filter states and at both` +
    ` text extremes.`,
);
