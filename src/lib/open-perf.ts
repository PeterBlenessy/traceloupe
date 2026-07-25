/**
 * Open-a-backup timing, for devtools (#40).
 *
 * Opening felt slow (~4 s) with no way to see where the time went. The Rust side
 * logs its own per-phase timings (they reach the console as `[traceloupe]` lines
 * at **debug** level — Settings → Developer → log level), but those only cover
 * the `open_backup` command. Measured on this machine with generated fixtures,
 * the backend work is milliseconds: backup discovery ~7.5 ms per backup and the
 * PBKDF2 key ladder ≤65 ms even at DPIC=200k. So the remaining time is on this
 * side — the IPC round-trip, the query refetch round, and the first render of the
 * landing view. This module makes that half visible too.
 *
 * Always on: one open emits a handful of lines, and a latency regression here is
 * a UX regression. Filter the console by `[open-perf]`. Each phase also emits a
 * `performance.measure`, so the Performance panel shows the same spans.
 */

const PREFIX = "%c[open-perf]%c";
const STYLE = "color:#38bdf8;font-weight:600";

let startedAt: number | null = null;
let lastAt: number | null = null;
let label = "";

function ms(n: number): string {
  return `${n.toFixed(0)} ms`;
}

/** Begin timing an open. Call as early as the user's intent is known. */
export function openPerfStart(backupLabel: string): void {
  startedAt = performance.now();
  lastAt = startedAt;
  label = backupLabel;
  performance.mark("open-backup:start");
  console.info(
    `${PREFIX} opening ${label} —`,
    STYLE,
    "color:inherit",
    "timing phases below (backend phases log as [traceloupe] at debug level)",
  );
}

/**
 * Record one phase, timed from the previous phase (or from the start). No-op if
 * `openPerfStart` wasn't called, so this can be sprinkled safely.
 */
export function openPerfPhase(phase: string): void {
  if (startedAt === null || lastAt === null) return;
  const now = performance.now();
  const since = now - lastAt;
  const total = now - startedAt;
  lastAt = now;
  try {
    performance.measure(`open-backup: ${phase}`, { start: now - since, end: now });
  } catch {
    // performance.measure with a start/end object needs a modern engine; the
    // console line is the point, so never let timing break the open path.
  }
  console.info(
    `${PREFIX} ${phase}: ${ms(since)} (total ${ms(total)})`,
    STYLE,
    "color:inherit",
  );
}

/** Close out the open. Subsequent phase calls are ignored until the next start. */
export function openPerfEnd(phase = "first paint of landing view"): void {
  if (startedAt === null) return;
  openPerfPhase(phase);
  const total = performance.now() - startedAt;
  performance.mark("open-backup:end");
  try {
    performance.measure("open-backup: total", "open-backup:start", "open-backup:end");
  } catch {
    // See above — informational only.
  }
  console.info(
    `${PREFIX} ${label} usable after ${ms(total)}`,
    STYLE,
    "color:inherit",
  );
  startedAt = null;
  lastAt = null;
}

/** Whether an open is currently being timed (so views only report once). */
export function openPerfInFlight(): boolean {
  return startedAt !== null;
}
