#!/usr/bin/env node
/**
 * Fail if a real backup's own row counts get committed.
 *
 * AGENTS.md: never record anything derived from the user's real backup. It has
 * been violated twice — once by a coverage doc that opened with a device's
 * message/photo/contact totals, and once by parser doc-comments quoting a
 * library's asset counts. Both read as harmless engineering detail; both are
 * personal data about one person, in a public repo. A rule that is only in a
 * document gets broken, so this makes it a build failure.
 *
 * The `backup-coverage` tool that PRODUCES these figures is deliberately kept —
 * measuring a backup is the whole point. What must never happen is committing
 * its output.
 *
 * What trips it: a number that reads as a tally of someone's data — a
 * thousands-separated figure next to a data noun ("95,334 camera-roll assets"),
 * or a bare count of one ("3,842 notes"). Version numbers, issue references,
 * byte sizes, dates, and schema constants are not counts of anybody's content
 * and are left alone.
 */
import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";

// Nouns that only ever pluralize a person's own records.
const NOUNS =
  "messages|assets|photos|videos|contacts|calls|notes|memos|recordings|reminders|" +
  "samples|workouts|visits|bookmarks|threads|conversations|attachments|events|" +
  "chats|sessions|ring days|quantity samples|camera-roll assets|history items";

// "95,334 camera-roll assets" / "3842 notes" / "24k GPS points". A bare
// four-digit year is not a tally ("2021 Photos" labels a fixture row), so
// 19xx/20xx without a thousands separator is excluded.
const COUNTED = new RegExp(
  String.raw`\b\d{1,3}(?:,\d{3})+\s*(?:${NOUNS})\b|\b(?!(?:19|20)\d{2}\b)\d{3,}\s*(?:${NOUNS})\b|\b\d+k\s+(?:GPS points|samples)\b`,
  "i",
);

// Counts from a PUBLIC research image or a third-party feed are not anybody's
// private data — they are citable facts, and stating them is how a parser
// documents what it was validated against. Only the user's own device is off
// limits.
const PUBLIC_SOURCE =
  /public|hickman|stalkerware-indicators|validation device|fixture|iLEAPP/i;

// Only text we author. Source, docs and agent instructions — not lockfiles,
// fixtures, or the tool's own README examples, which describe shape not people.
const TRACKED = execSync("git ls-files", { encoding: "utf8" })
  .split("\n")
  .filter(Boolean)
  .filter((f) => /\.(md|rs|ts|tsx|mjs|toml)$/.test(f))
  .filter((f) => !f.startsWith("crates/traceloupe-core/tests/"))
  // Fixtures assert on numbers they themselves planted; that is not a person.
  .filter((f) => !/fixtures?\//.test(f));

const hits = [];
for (const file of TRACKED) {
  let text;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    continue; // deleted between ls-files and read
  }
  const lines = text.split("\n");
  lines.forEach((line, i) => {
    // An explicit opt-out for the rare legitimate case, so the guard can be
    // satisfied deliberately rather than by deleting it.
    const near = lines.slice(Math.max(0, i - 4), i + 3).join(" ");
    // The opt-out reads on the line or just beside it, because the natural place
    // to explain a number is the comment under it, not crammed into the string.
    if (near.includes("not-a-backup-count")) return;
    // Attribution usually sits a sentence away from the figure it licenses —
    // a doc-comment says "Josh Hickman's public iOS 17 image" on one line and
    // gives the counts two lines later. Judge the paragraph, not the line.
    if (PUBLIC_SOURCE.test(near)) return;
    const m = line.match(COUNTED);
    if (!m) return;
    if (new RegExp(String.raw`at\s+` + m[0].trim().replace(/\s+/g, String.raw`\s*`)).test(line))
      return; // projection, not an observation
    hits.push({ file, line: i + 1, text: m[0].trim() });
  });
}

if (hits.length > 0) {
  console.error(
    "✗ backup statistics found in committed text.\n" +
      "  These read as counts of one person's own data. Describe the SHAPE\n" +
      "  instead, and let `backup-coverage` report figures per backup.\n" +
      "  (Genuinely not a backup count? Add `not-a-backup-count` to the line.)\n",
  );
  for (const h of hits) console.error(`  ${h.file}:${h.line}  ${h.text}`);
  process.exit(1);
}
console.log(`no-backup-stats OK — scanned ${TRACKED.length} files`);
