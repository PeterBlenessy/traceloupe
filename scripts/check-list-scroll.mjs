#!/usr/bin/env node
/**
 * A keyboard-nav list wrapper must be a FLEX COLUMN, or its list cannot scroll.
 *
 * `useListNavigation` returns props spread onto a wrapper div that sits between
 * the view's flex column and the `VirtualList`. `VirtualList`'s scroller is
 * `min-h-0 flex-1 overflow-auto` — and `flex-1`/`min-h-0` are FLEX-ITEM
 * properties: they do nothing unless the parent is `display:flex`.
 *
 * When that wrapper is a plain block (`min-h-0 flex-1` but no `flex flex-col`),
 * the scroller resolves to `height:auto`, grows to the virtualizer's full
 * spacer height, and `overflow-auto` never engages. The symptom is nasty and
 * indirect: the list ignores the wheel entirely, and arrow-key
 * `scrollIntoView` — finding nothing scrollable beneath it — walks up and
 * scrolls an `overflow-hidden` app-shell ancestor instead, dragging the whole
 * two-pane layout sideways so the detail pane leaves the screen.
 *
 * That is exactly what shipped in Messages, Notes and Contacts. Safety Scan and
 * Security got it right; the entire difference was the two words `flex
 * flex-col`, which is far too easy to omit for something with no local symptom.
 *
 * Usage: node scripts/check-list-scroll.mjs
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname;

function walk(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (/\.tsx$/.test(e)) out.push(p);
  }
  return out;
}

/** Every JSX opening tag that spreads keyboard-nav list props. */
function navWrappers(src) {
  const out = [];
  // `{...listProps}` / `{...threadListProps}` / `{...noteListProps}` …
  const re = /\{\.\.\.\w*[lL]istProps\}/g;
  let m;
  while ((m = re.exec(src))) {
    // Back up to this tag's `<`, forward to its closing `>`.
    const start = src.lastIndexOf("<", m.index);
    const end = src.indexOf(">", m.index);
    if (start === -1 || end === -1) continue;
    out.push({
      tag: src.slice(start, end + 1),
      line: src.slice(0, start).split("\n").length,
    });
  }
  return out;
}

const failures = [];
for (const file of walk(join(ROOT, "src"))) {
  const src = readFileSync(file, "utf8");
  for (const w of navWrappers(src)) {
    const cls = /className="([^"]*)"/.exec(w.tag)?.[1] ?? "";
    const isFlexItemSized = /\bflex-1\b/.test(cls) && /\bmin-h-0\b/.test(cls);
    const isFlexColumn = /\bflex\b/.test(cls) && /\bflex-col\b/.test(cls);
    if (isFlexItemSized && !isFlexColumn)
      failures.push(
        `${file.replace(ROOT, "")}:${w.line} keyboard-nav list wrapper is ` +
          `"min-h-0 flex-1" but not a flex column — the list inside it cannot ` +
          `scroll (add \`flex flex-col\`)`,
      );
  }
}

// Prove the detector fires — a rule nobody has seen fail is a rule nobody can
// trust. Both directions, so a matcher that flagged everything would fail too.
{
  const bad = navWrappers(
    `<div {...listProps} className="min-h-0 flex-1 outline-none">`,
  );
  const good = navWrappers(
    `<div {...listProps} className="flex min-h-0 flex-1 flex-col outline-none">`,
  );
  const cls = (w) => /className="([^"]*)"/.exec(w.tag)?.[1] ?? "";
  const flags = (w) =>
    /\bflex-1\b/.test(cls(w)) &&
    /\bmin-h-0\b/.test(cls(w)) &&
    !(/\bflex\b/.test(cls(w)) && /\bflex-col\b/.test(cls(w)));
  if (bad.length !== 1 || !flags(bad[0]) || good.length !== 1 || flags(good[0])) {
    console.error(
      "check-list-scroll SELF-TEST failed: the detector does not separate a " +
        "block wrapper from a flex column",
    );
    process.exit(2);
  }
}

if (failures.length) {
  console.error(`list-scroll: ${failures.length} unscrollable list wrapper(s):\n`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log("list-scroll OK — every keyboard-nav list wrapper is a flex column.");
