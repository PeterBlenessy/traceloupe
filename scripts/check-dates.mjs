#!/usr/bin/env node
/**
 * Every DATE the app shows must carry its year.
 *
 * A backup spans years by definition, so "Jun 3" is ambiguous in every view
 * that has one — the conversation list, the timeline separators, note and call
 * rows, chart axes. The app used to drop the year whenever the date fell in the
 * CURRENT year, which reads fine on the day it is written and is wrong forever
 * after (#345).
 *
 * This is a static rule over the source, not a runtime one, because the failure
 * is invisible in a screenshot taken this year — exactly how it survived so
 * long. It checks every `Intl.DateTimeFormat` options object in the frontend:
 * if it asks for a `day` or a `month`, it must also ask for a `year`.
 *
 * A time-of-day formatter (`hour`/`minute` only) is not a date and is exempt —
 * "15:04" on a row that means today has no year to omit.
 *
 * Usage: node scripts/check-dates.mjs
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname;
const SRC = join(ROOT, "src");

/** Every .ts/.tsx under src/. */
function walk(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (/\.tsx?$/.test(e)) out.push(p);
  }
  return out;
}

/** The `{...}` immediately following each `Intl.DateTimeFormat(` call, brace-matched
 *  so nested objects and the trailing args don't confuse it. */
function optionObjects(src) {
  const out = [];
  const re = /Intl\.DateTimeFormat\s*\(/g;
  let m;
  while ((m = re.exec(src))) {
    const open = src.indexOf("{", m.index);
    if (open === -1) continue;
    // Only look inside this call's argument list.
    if (src.slice(m.index, open).includes(")")) continue;
    let depth = 0;
    for (let i = open; i < src.length; i++) {
      if (src[i] === "{") depth++;
      else if (src[i] === "}") {
        depth--;
        if (depth === 0) {
          out.push({
            text: src.slice(open, i + 1),
            line: src.slice(0, open).split("\n").length,
          });
          break;
        }
      }
    }
  }
  return out;
}

const asks = (o, key) => new RegExp(`\\b${key}\\s*:`).test(o);

const failures = [];
for (const file of walk(SRC)) {
  const src = readFileSync(file, "utf8");
  for (const o of optionObjects(src)) {
    const isDate = asks(o.text, "day") || asks(o.text, "month");
    // A spread (`...year`) can supply the year from a variable, so a literal
    // check would report a false positive; treat any spread as "may carry it".
    const spreads = /\.\.\./.test(o.text);
    if (isDate && !asks(o.text, "year") && !spreads)
      failures.push(
        `${file.replace(ROOT, "")}:${o.line} formats a date without a year — ` +
          `${o.text.replace(/\s+/g, " ").slice(0, 70)}`,
      );
  }
}

// Prove the detector fires, for the same reason the other checks self-test:
// a rule nobody has seen fail is a rule nobody can trust.
{
  const probe = optionObjects(
    `new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" })`,
  );
  const caught =
    probe.length === 1 &&
    (asks(probe[0].text, "day") || asks(probe[0].text, "month")) &&
    !asks(probe[0].text, "year");
  if (!caught) {
    console.error(
      "check-dates SELF-TEST failed: the detector did not flag a year-less date format",
    );
    process.exit(2);
  }
}

if (failures.length) {
  console.error(`dates: ${failures.length} year-less date format(s):\n`);
  for (const f of failures) console.error(`  ${f}`);
  console.error(
    "\nEvery displayed date needs its year — a backup spans years (#345).\n" +
      "Add `year` to the options, or use a formatter in src/lib/format.ts.",
  );
  process.exit(1);
}
console.log("dates OK — every date format carries its year.");
