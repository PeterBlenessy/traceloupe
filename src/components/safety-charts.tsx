/**
 * The Safety Scan report's charts (#66).
 *
 * Three deliberate constraints, all of them decisions rather than defaults:
 *
 * 1. **Counts, never proportions.** No pie chart, no "41% coercive control".
 *    A percentage of an unvalidated classifier's output reads like a diagnosis.
 * 2. **Every bar splits confirmed from unconfirmed.** The cascade re-checks only
 *    some findings with the strong tier; the rest are hatched, so a chart cannot
 *    lend authority the model never gave it.
 * 3. **Inline SVG, sized in percentages.** The report prints, and print is where
 *    canvas rasterizes badly and CSS background gradients get dropped. Shapes
 *    are content; percentage geometry means no measurement and no distortion.
 *
 * Colour carries severity, texture carries confidence — so the charts survive a
 * greyscale print and stay readable under "Differentiate without colour".
 */
import { useId } from "react";

import type { FindingAnalytics, ChartBucket } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** Severity 1..3, drawn low-to-high so the serious band sits on top. */
const SEVERITY = [
  { label: "Concerning", color: "var(--muted-foreground)" },
  { label: "Harmful", color: "var(--status-warning)" },
  { label: "Serious", color: "var(--status-danger)" },
] as const;

type Unit = FindingAnalytics["unit"];

/** One drawable segment of a bar. */
type Segment = { color: string; hatch: boolean; n: number; label: string };

function segmentsOf(b: ChartBucket, hatchIds: string[]): Segment[] {
  const out: Segment[] = [];
  // Serious first so it anchors the base of a column and the left of a row —
  // the eye reads the most severe band without hunting for it.
  for (let i = 2; i >= 0; i--) {
    if (b.confirmed[i] > 0)
      out.push({
        color: SEVERITY[i].color,
        hatch: false,
        n: b.confirmed[i],
        label: `${SEVERITY[i].label}, confirmed`,
      });
    if (b.unconfirmed[i] > 0)
      out.push({
        color: `url(#${hatchIds[i]})`,
        hatch: true,
        n: b.unconfirmed[i],
        label: `${SEVERITY[i].label}, unconfirmed`,
      });
  }
  return out;
}

export function bucketTotal(b: ChartBucket): number {
  return (
    b.confirmed.reduce((a, n) => a + n, 0) +
    b.unconfirmed.reduce((a, n) => a + n, 0)
  );
}

/** Diagonal hatch per severity — the unconfirmed texture.
 *
 *  A pattern cannot inherit `fill` from the shape that uses it, so there is one
 *  per severity rather than one reusable overlay. The washed rect underneath
 *  keeps the band's colour readable; the stripes are what survive greyscale. */
function HatchDefs({ ids }: { ids: string[] }) {
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      className="absolute size-0"
      style={{ position: "absolute", width: 0, height: 0 }}
    >
      <defs>
        {SEVERITY.map((s, i) => (
          <pattern
            key={s.label}
            id={ids[i]}
            width="6"
            height="6"
            patternUnits="userSpaceOnUse"
            patternTransform="rotate(45)"
          >
            <rect
              width="6"
              height="6"
              style={{ fill: s.color, opacity: 0.3 }}
            />
            <line
              x1="0"
              y1="0"
              x2="0"
              y2="6"
              style={{ stroke: s.color }}
              strokeWidth="2.5"
            />
          </pattern>
        ))}
      </defs>
    </svg>
  );
}

function useHatchIds(): string[] {
  const base = useId();
  return SEVERITY.map((_, i) => `${base}-hatch-${i}`.replace(/:/g, ""));
}

// ---------------------------------------------------------------- time buckets

/** Parse a bucket key into a sortable, steppable position.
 *  Keys are `YYYY-MM-DD` (day and a week's Monday), `YYYY-MM`, `YYYY-Qn`, `YYYY`
 *  — built by the LOCAL calendar in SQL and formatted here as UTC, so no
 *  timestamp is re-interpreted in a second time zone on the way out. */
function keyToDate(unit: Unit, key: string): Date {
  const [y, rest] = [Number(key.slice(0, 4)), key.slice(5)];
  switch (unit) {
    case "day":
    case "week":
      return new Date(
        Date.UTC(y, Number(key.slice(5, 7)) - 1, Number(key.slice(8, 10))),
      );
    case "month":
      return new Date(Date.UTC(y, Number(rest) - 1, 1));
    case "quarter":
      return new Date(Date.UTC(y, (Number(rest.slice(1)) - 1) * 3, 1));
    case "year":
      return new Date(Date.UTC(y, 0, 1));
  }
}

function dateToKey(unit: Unit, d: Date): string {
  const y = d.getUTCFullYear();
  const m = d.getUTCMonth() + 1;
  const pad = (n: number) => String(n).padStart(2, "0");
  switch (unit) {
    case "day":
    case "week":
      return `${y}-${pad(m)}-${pad(d.getUTCDate())}`;
    case "month":
      return `${y}-${pad(m)}`;
    case "quarter":
      return `${y}-Q${Math.floor(d.getUTCMonth() / 3) + 1}`;
    case "year":
      return `${y}`;
  }
}

function step(unit: Unit, d: Date): Date {
  const n = new Date(d);
  switch (unit) {
    case "day":
      n.setUTCDate(n.getUTCDate() + 1);
      break;
    case "week":
      n.setUTCDate(n.getUTCDate() + 7);
      break;
    case "month":
      n.setUTCMonth(n.getUTCMonth() + 1);
      break;
    case "quarter":
      n.setUTCMonth(n.getUTCMonth() + 3);
      break;
    case "year":
      n.setUTCFullYear(n.getUTCFullYear() + 1);
      break;
  }
  return n;
}

/** SQL returns only the buckets that have findings. A quiet month is a fact
 *  about the data, so the gaps are filled with empty bars rather than closed up
 *  — otherwise an absence of findings arrives as an absence of *time*, and the
 *  axis silently compresses a two-year lull into nothing. */
export function fillTimeGaps(
  unit: Unit,
  buckets: ChartBucket[],
): ChartBucket[] {
  if (buckets.length < 2) return buckets;
  const byKey = new Map(buckets.map((b) => [b.key, b]));
  const last = keyToDate(unit, buckets[buckets.length - 1].key).getTime();
  const out: ChartBucket[] = [];
  // Guard against a malformed key producing a runaway loop; the unit is chosen
  // to keep this well under a hundred.
  for (
    let d = keyToDate(unit, buckets[0].key), i = 0;
    d.getTime() <= last && i < 600;
    d = step(unit, d), i++
  ) {
    const key = dateToKey(unit, d);
    out.push(
      byKey.get(key) ?? { key, confirmed: [0, 0, 0], unconfirmed: [0, 0, 0] },
    );
  }
  return out;
}

const UNIT_NOUN: Record<Unit, string> = {
  day: "day",
  week: "week",
  month: "month",
  quarter: "quarter",
  year: "year",
};

/** Does this chart cross a calendar year? A day/week/month label that omits the
 *  year is unreadable the moment it does: a 30-month axis reads
 *  "Jan Feb … Dec Jan Feb …" with nothing to say which January, and the hover
 *  title on the bar repeats the ambiguity. */
function spansYears(unit: Unit, buckets: ChartBucket[]): boolean {
  if (buckets.length < 2) return false;
  const y = (b: ChartBucket) => keyToDate(unit, b.key).getUTCFullYear();
  return y(buckets[0]) !== y(buckets[buckets.length - 1]);
}

function formatBucket(
  unit: Unit,
  key: string,
  withYear = false,
  locale?: string,
): string {
  const d = keyToDate(unit, key);
  const year: Intl.DateTimeFormatOptions = withYear ? { year: "2-digit" } : {};
  const fmt = (o: Intl.DateTimeFormatOptions) =>
    new Intl.DateTimeFormat(locale, { ...o, ...year, timeZone: "UTC" }).format(
      d,
    );
  switch (unit) {
    case "day":
      return fmt({ day: "numeric", month: "short" });
    case "week":
      return fmt({ day: "numeric", month: "short" });
    case "month":
      return fmt({ month: "short" });
    case "quarter":
      return `Q${Math.floor(d.getUTCMonth() / 3) + 1} ${String(d.getUTCFullYear()).slice(2)}`;
    case "year":
      return String(d.getUTCFullYear());
  }
}

/** The range a chart covers, for its caption. */
function formatSpan(unit: Unit, buckets: ChartBucket[]): string {
  if (buckets.length === 0) return "";
  const opts: Intl.DateTimeFormatOptions =
    unit === "year" ? { year: "numeric" } : { month: "short", year: "numeric" };
  const at = (i: number) =>
    new Intl.DateTimeFormat(undefined, { ...opts, timeZone: "UTC" }).format(
      keyToDate(unit, buckets[i].key),
    );
  const first = at(0);
  const lastLabel = at(buckets.length - 1);
  return first === lastLabel ? first : `${first} – ${lastLabel}`;
}

// -------------------------------------------------------------------- charts

const PLOT_H = 116;
const AXIS_H = 20;
/** Room above the tallest bar for the scale label. Without a stated maximum a
 *  reader cannot tell whether the tall bar is three findings or thirty, and the
 *  hover titles that would have told them do not print. */
const TOP_PAD = 13;

/** Findings over the scanned content's own timeline.
 *
 *  The x-axis is `occurredAt` — when the messages were sent — never a series of
 *  scan runs. Runs are not comparable to one another: the chunker, the model
 *  tier and the scope have all changed between them, so a line through their
 *  totals would be a lie with a trend drawn on it. */
function TimeChart({
  unit,
  buckets,
  withYear,
  hatchIds,
}: {
  unit: Unit;
  buckets: ChartBucket[];
  withYear: boolean;
  hatchIds: string[];
}) {
  const max = Math.max(1, ...buckets.map(bucketTotal));
  const n = buckets.length;
  const slot = 100 / n;
  const barW = slot * 0.72;
  // At most ~12 tick labels, whatever the bucket count, so they never collide.
  const stride = Math.max(1, Math.ceil(n / 12));

  const base = TOP_PAD + PLOT_H;

  return (
    <svg
      width="100%"
      height={base + AXIS_H}
      aria-hidden="true"
      className="overflow-visible"
    >
      {/* The scale, stated once. */}
      <line
        x1="0"
        x2="100%"
        y1={TOP_PAD}
        y2={TOP_PAD}
        style={{ stroke: "var(--border)" }}
        strokeWidth="1"
        strokeDasharray="2 3"
      />
      <text x="0" y={TOP_PAD - 3} className="fill-muted-foreground text-3xs">
        {max}
      </text>
      {buckets.map((b, i) => {
        const total = bucketTotal(b);
        const x = i * slot + (slot - barW) / 2;
        let y = base;
        const segs = segmentsOf(b, hatchIds);
        return (
          <g key={b.key}>
            {segs.map((s, si) => {
              const h = (s.n / max) * PLOT_H;
              y -= h;
              return (
                <rect
                  key={si}
                  x={`${x}%`}
                  width={`${barW}%`}
                  y={y}
                  height={h}
                  style={{ fill: s.color }}
                >
                  <title>{`${formatBucket(unit, b.key, withYear)}: ${s.n} ${s.label}`}</title>
                </rect>
              );
            })}
            {total === 0 && (
              // A bar of zero still needs to exist, or a quiet period reads as
              // missing data rather than as quiet.
              <rect
                x={`${x}%`}
                width={`${barW}%`}
                y={base - 1}
                height={1}
                style={{ fill: "var(--border)" }}
              />
            )}
            {i % stride === 0 && (
              <text
                x={`${i * slot + slot / 2}%`}
                y={base + 13}
                textAnchor="middle"
                className="fill-muted-foreground text-3xs"
              >
                {formatBucket(unit, b.key, withYear)}
              </text>
            )}
          </g>
        );
      })}
      <line
        x1="0"
        x2="100%"
        y1={base}
        y2={base}
        style={{ stroke: "var(--border)" }}
        strokeWidth="1"
      />
    </svg>
  );
}

/** A ranked list of bars with room for a real label.
 *
 *  Horizontal because the labels are contact names and category names, and a
 *  rotated axis label is a chart apologising for its own layout. */
function RankChart({
  rows,
  hatchIds,
}: {
  rows: { key: string; label: string; bucket: ChartBucket }[];
  hatchIds: string[];
}) {
  const max = Math.max(1, ...rows.map((r) => bucketTotal(r.bucket)));
  return (
    <div className="space-y-1.5">
      {rows.map((r) => {
        const total = bucketTotal(r.bucket);
        const segs = segmentsOf(r.bucket, hatchIds);
        let x = 0;
        return (
          // Proportional, not a fixed label column: the same chart renders at
          // ~320px beside the findings list and at ~670px in the report, and a
          // 9rem label left the bar narrower than its own name.
          <div
            key={r.key}
            className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.6fr)_1.75rem] items-center gap-2"
          >
            <span
              className="truncate text-xs text-muted-foreground"
              title={r.label}
            >
              {r.label}
            </span>
            <svg width="100%" height="10" aria-hidden="true">
              {segs.map((s, si) => {
                const w = (s.n / max) * 100;
                const at = x;
                x += w;
                return (
                  <rect
                    key={si}
                    x={`${at}%`}
                    width={`${w}%`}
                    y="0"
                    height="10"
                    rx="1"
                    style={{ fill: s.color }}
                  >
                    <title>{`${r.label}: ${s.n} ${s.label}`}</title>
                  </rect>
                );
              })}
            </svg>
            <span className="text-right text-xs tabular-nums text-muted-foreground">
              {total}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function Legend({ hatchIds }: { hatchIds: string[] }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-2xs text-muted-foreground">
      {SEVERITY.map((s, i) => (
        <span key={s.label} className="inline-flex items-center gap-1">
          <svg width="9" height="9" aria-hidden="true">
            <rect
              width="9"
              height="9"
              rx="1"
              style={{ fill: SEVERITY[i].color }}
            />
          </svg>
          {s.label}
        </span>
      ))}
      <span className="inline-flex items-center gap-1">
        <svg width="9" height="9" aria-hidden="true">
          <rect
            width="9"
            height="9"
            rx="1"
            style={{ fill: `url(#${hatchIds[2]})` }}
          />
        </svg>
        Hatched: not confirmed by the second model
      </span>
    </div>
  );
}

/** The numbers behind a chart, for screen readers and for anyone who wants the
 *  values rather than the shape. Charts are `aria-hidden`; this is what they
 *  announce instead. */
function ChartTable({
  caption,
  rows,
}: {
  caption: string;
  rows: { label: string; bucket: ChartBucket }[];
}) {
  return (
    <table className="sr-only">
      <caption>{caption}</caption>
      <thead>
        <tr>
          <th>Bucket</th>
          {SEVERITY.map((s) => (
            <th key={s.label}>{s.label} confirmed</th>
          ))}
          {SEVERITY.map((s) => (
            <th key={s.label}>{s.label} unconfirmed</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.bucket.key}>
            <th scope="row">{r.label}</th>
            {r.bucket.confirmed.map((n, i) => (
              <td key={i}>{n}</td>
            ))}
            {r.bucket.unconfirmed.map((n, i) => (
              <td key={i}>{n}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Block({
  title,
  note,
  className,
  children,
}: {
  title: string;
  note?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <figure className={cn("space-y-2 break-inside-avoid", className)}>
      <figcaption className="text-xs font-medium">
        {title}
        {note && (
          <span className="ml-1.5 font-normal text-muted-foreground">
            {note}
          </span>
        )}
      </figcaption>
      {children}
    </figure>
  );
}

/**
 * The analysis section: what was flagged, when, and where.
 *
 * Every number here is aggregated in SQL over EVERY finding the filter matches
 * — not over the page the list renders. The report caps its list at 500 rows
 * and its narrative at 100; a chart drawn from either would describe a subset
 * while looking like it described the scan.
 */
export function FindingCharts({
  analytics,
  categoryLabel,
  conversationLabel,
  variant = "document",
  className,
}: {
  analytics: FindingAnalytics;
  categoryLabel: (slug: string) => string;
  /** Resolves a thread identifier to a contact/group name. */
  conversationLabel: (identifier: string) => string;
  /** `document` stacks for the printable report; `panel` pairs the two ranked
   *  charts side by side, because on screen the charts share their height with
   *  the findings list and a 400px-tall stack would leave no room for it. */
  variant?: "document" | "panel";
  className?: string;
}) {
  const hatchIds = useHatchIds();
  const {
    unit,
    byCategory,
    byConversation,
    otherConversations,
    otherConversationFindings,
    charted,
    undated,
    dismissed,
  } = analytics;
  const overTime = fillTimeGaps(unit, analytics.overTime);
  const multiYear = spansYears(unit, overTime);

  if (charted === 0) return null;

  const categoryRows = byCategory.map((b) => ({
    key: b.key,
    label: categoryLabel(b.key),
    bucket: b,
  }));
  const conversationRows = byConversation.map((b) => ({
    key: b.key || "__notes",
    label: b.key ? conversationLabel(b.key) : "Notes",
    bucket: b,
  }));

  const panel = variant === "panel";
  const full = panel ? "sm:col-span-2" : undefined;

  return (
    <section
      className={cn(
        panel ? "grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2" : "space-y-6",
        className,
      )}
    >
      {/* One copy of the patterns for every chart below: they were being emitted
          inside each rank-chart row, so a six-row chart repeated the same three
          ids six times. `url(#id)` resolves document-wide. */}
      <HatchDefs ids={hatchIds} />

      {!panel && (
        <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Analysis
        </h2>
      )}

      {overTime.length > 0 && (
        <Block
          title={`Findings by ${UNIT_NOUN[unit]}`}
          note={formatSpan(unit, overTime)}
          className={full}
        >
          <TimeChart
            unit={unit}
            buckets={overTime}
            withYear={multiYear}
            hatchIds={hatchIds}
          />
          <ChartTable
            caption={`Findings by ${UNIT_NOUN[unit]}`}
            rows={overTime.map((b) => ({
              label: formatBucket(unit, b.key, multiYear),
              bucket: b,
            }))}
          />
        </Block>
      )}

      {categoryRows.length > 0 && (
        <Block title="What was flagged">
          <RankChart rows={categoryRows} hatchIds={hatchIds} />
          <ChartTable caption="Findings by category" rows={categoryRows} />
        </Block>
      )}

      {conversationRows.length > 0 && (
        <Block
          title="Where it was flagged"
          note={
            otherConversations > 0
              ? `busiest ${conversationRows.length} of ${conversationRows.length + otherConversations}`
              : undefined
          }
        >
          <RankChart rows={conversationRows} hatchIds={hatchIds} />
          <ChartTable
            caption="Findings by conversation"
            rows={conversationRows}
          />
          {otherConversations > 0 && (
            <p className="text-2xs text-muted-foreground">
              {otherConversations} further conversation
              {otherConversations === 1 ? "" : "s"} account for{" "}
              {otherConversationFindings} finding
              {otherConversationFindings === 1 ? "" : "s"}, not charted above.
            </p>
          )}
        </Block>
      )}

      <div className={full}>
        <Legend hatchIds={hatchIds} />
      </div>

      {/* What the charts are NOT. Stated next to them, not buried in a footer:
          the disclosures are the reason a reader can trust the shapes at all. */}
      <p className={cn("text-2xs leading-relaxed text-muted-foreground", full)}>
        {charted} finding{charted === 1 ? "" : "s"} charted.
        {undated > 0 &&
          ` ${undated} ${undated === 1 ? "has" : "have"} no date and ${undated === 1 ? "is" : "are"} absent from the timeline (counted everywhere else).`}
        {dismissed > 0 &&
          ` ${dismissed} dismissed as false positive${dismissed === 1 ? "" : "s"} and left out of every chart.`}{" "}
        These are one local model's verdicts, not ground truth. Hatched portions
        were seen only by the fast tier and never confirmed.
      </p>
    </section>
  );
}
