#!/usr/bin/env node
/**
 * Every chat parser must either extract media, or say why it does not.
 *
 * Thirteen of fifteen parsers shipped returning `attachments: Vec::new()`. Each
 * omission was individually reasonable — get messages working, do media later —
 * and collectively it meant a photo sent in most supported apps was invisible.
 * Nothing objected, because nothing checked. WhatsApp's media parse was in fact
 * DEAD ENTIRELY until #360 and no test noticed.
 *
 * So the rule stops being advice. A parser that hands back no attachments fails
 * this check until its source carries a `MEDIA:` note recording what was looked
 * for and what was found — a measurement, not an intention:
 *
 *     // MEDIA: none. backup-coverage shows this container holds no file over
 *     // 100 KB on the iOS 17 image; its photos are CDN references. See #421.
 *
 * That is a claim someone can check and disagree with, which is the point. The
 * note is not a way to opt out — it is the finding, written down. An app whose
 * photos genuinely live on a server needs no extractor, and saying so is worth
 * as much as building one.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const DIR = "crates/traceloupe-core/src/parsers/apps";
// Not parsers: the shared types, and the schema-blind fallback that exists
// precisely because parsers drift.
const NOT_A_PARSER = new Set(["mod.rs", "discovery.rs"]);

const offenders = [];
const debt = [];
for (const file of readdirSync(DIR).sort()) {
  if (!file.endsWith(".rs") || NOT_A_PARSER.has(file)) continue;
  const text = readFileSync(join(DIR, file), "utf8");

  // Does it ever build an attachment? `AppAttachment` is the type every
  // extractor constructs, so its absence is the signal.
  const extracts =
    /AppAttachment\s*\{/.test(text) || /attachments\s*\.\s*push\s*\(/.test(text);
  if (extracts) continue;

  // No extraction — then it owes an explanation. Two are acceptable:
  //
  //   MEDIA: none. <what was measured>   — settled; the app keeps nothing local
  //   MEDIA: TODO #123                   — known debt, with a ticket
  //
  // Silence is not. The second form is deliberately allowed: pretending the
  // thirteen parsers that shipped without media were already measured would be
  // the same dishonesty this guard exists to stop. Debt that names a ticket is
  // visible and shrinks; debt that is invisible is what got us here.
  const settled = /\/\/\s*MEDIA:\s*none\./.test(text);
  const tracked = /\/\/\s*MEDIA:\s*TODO\s*#\d+/.test(text);
  if (settled) continue;
  if (tracked) {
    debt.push(file.replace(/\.rs$/, ""));
    continue;
  }
  offenders.push(file.replace(/\.rs$/, ""));
}

if (offenders.length > 0) {
  console.error(
    "✗ chat parsers that neither extract media nor say why:\n" +
      "  Run `backup-coverage` against a backup with the app installed and read\n" +
      "  'media inside app containers'. Then either extract what is there, or\n" +
      "  record the measurement in the parser as a `// MEDIA:` note.\n" +
      "  A photo the OWNER SENT was theirs first and is often still on the\n" +
      "  device, even when received ones are only CDN links — so a low count is\n" +
      "  a finding to write down, not a reason to skip.\n",
  );
  for (const o of offenders) console.error(`  ${o}`);
  process.exit(1);
}
if (debt.length > 0) {
  console.log(
    `app-parser-coverage OK — ${debt.length} parser(s) carry a tracked MEDIA TODO: ` +
      debt.join(", "),
  );
} else {
  console.log("app-parser-coverage OK — every parser extracts media or explains");
}
