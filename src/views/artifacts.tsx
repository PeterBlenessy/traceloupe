/**
 * The generic artifact view — one screen for every declarative module.
 *
 * It knows no artifact by name. The backend describes each one (label, columns,
 * row count) and this renders whatever arrives, the same way the home dashboard
 * takes its tiles from `METRIC_SOURCES` as data. That is the property that makes
 * the ~360-artifact tail affordable: a new TOML module appears here with no
 * frontend change at all.
 *
 * Deliberately a table. Under the earning-a-bespoke-view test (#194) a flat
 * row-shaped artifact renders as well in a table as in anything hand-built, and
 * hand-building 360 of them is exactly what this avoids. Artifacts that are
 * genuinely not row-shaped (media, threads, routes) fold into the view that can
 * show them instead.
 */
import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, Table2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SortControl, type SortState } from "@/components/sort-control";
import { useViewToolbar } from "@/components/toolbar-context";
import { useDebounced } from "@/lib/use-debounced";
import { NoBackupState, ListSearch, VirtualListView } from "@/components/view";
import { useEncryptedOnlyEmpty } from "@/lib/use-encrypted-only";
import { formatDateTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { client, type ArtifactRow, type ArtifactSummary } from "@/lib/ipc";

/** How many rows to pull. Artifacts are small next to Messages or the camera
 *  roll; when one turns out not to be, it earns real paging rather than this
 *  view growing a guess about which one needs it. */
const PAGE = 5000;

/** A value as text.
 *
 *  A column declared `timestamp` arrives as Unix seconds, so it is rendered as a
 *  date — the whole reason the module declares an epoch. Everything else is
 *  shown as the module produced it: this view must not second-guess a value,
 *  because it cannot know what the artifact meant by it. */
function cellText(value: ArtifactRow[string], isDate: boolean): string {
  if (value === null || value === undefined) return "—";
  if (isDate && typeof value === "number") return formatDateTime(value);
  if (typeof value === "boolean") return value ? "Yes" : "No";
  return String(value);
}

/** Columns whose values look like Unix-second timestamps.
 *
 *  The backend does not currently say which columns are dates — only their
 *  names — so this infers it from the values. Inference is the wrong long-term
 *  answer and is marked as such: the module already knows, and the summary
 *  should carry the column kind. Kept narrow (a plausible-date range on a
 *  number) so a count or an id cannot be mistaken for a date. */
function dateColumns(columns: string[], rows: ArtifactRow[]): Set<string> {
  const out = new Set<string>();
  for (const col of columns) {
    const values = rows.map((r) => r[col]).filter((v) => v !== null && v !== undefined);
    if (values.length === 0) continue;
    const allPlausibleDates = values.every(
      (v) => typeof v === "number" && v > 946_684_800 && v < 4_102_444_800,
    );
    if (allPlausibleDates) out.add(col);
  }
  return out;
}

function ArtifactTable({
  artifact,
  rows,
  search,
}: {
  artifact: ArtifactSummary;
  rows: ArtifactRow[];
  search: string;
}) {
  const dates = useMemo(() => dateColumns(artifact.columns, rows), [artifact.columns, rows]);
  return (
    <div className="min-w-full">
      <div
        data-slot="artifact-header"
        className="sticky top-0 z-10 flex gap-3 border-b bg-background/95 px-3 py-2 text-xs font-medium text-muted-foreground backdrop-blur"
      >
        {artifact.columns.map((c) => (
          <span key={c} className="min-w-0 flex-1 truncate">
            {c}
          </span>
        ))}
      </div>
      <VirtualListView<ArtifactRow>
        title={artifact.name}
        count={rows.length}
        estimateSize={36}
        items={rows}
        getKey={(_row, i) => i}
        emptyIcon={Table2}
        emptyMessage={search ? "No matches." : `No rows in ${artifact.name}.`}
        renderItem={(row) => (
          <div
            data-slot="list-row"
            className="flex gap-3 rounded-md px-3 py-1.5 text-[13px]"
          >
            {/* No per-cell tooltip. A native `title=` is banned (it looks
                nothing like the rest of the app), and wrapping four cells ×
                thousands of virtualized rows in the shared Tooltip is a lot of
                machinery for a hover nobody asked for. Cells are selectable
                instead, so a long value can be copied out. */}
            {artifact.columns.map((c) => (
              <span
                key={c}
                className={cn(
                  "min-w-0 flex-1 select-text truncate",
                  row[c] === null && "text-muted-foreground",
                )}
              >
                {cellText(row[c], dates.has(c))}
              </span>
            ))}
          </div>
        )}
      />
    </div>
  );
}

export function ArtifactsView() {
  const queryClient = useQueryClient();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: artifacts, isPending: listPending } = useQuery({
    queryKey: ["artifacts"],
    queryFn: () => client.listArtifacts(),
    enabled: active === true,
  });
  const { data: extraction } = useQuery({
    queryKey: ["artifactsExtractionState"],
    queryFn: () => client.artifactsExtractionState(),
    enabled: active === true,
  });
  const [extracting, setExtracting] = useState(false);
  const [extractError, setExtractError] = useState<string | null>(null);

  async function runExtraction() {
    setExtracting(true);
    setExtractError(null);
    try {
      await client.extractArtifacts();
      await queryClient.invalidateQueries({ queryKey: ["artifacts"] });
      await queryClient.invalidateQueries({ queryKey: ["artifactsExtractionState"] });
    } catch (e) {
      setExtractError(String(e));
    } finally {
      setExtracting(false);
    }
  }

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [rawSearch, setRawSearch] = useState("");
  const search = useDebounced(rawSearch, 200);
  const [sort, setSort] = useState<SortState>({ by: "none", desc: false });

  const list = artifacts ?? [];
  const selected = list.find((a) => a.id === selectedId) ?? list[0] ?? null;

  const { data: rows, isPending: rowsPending } = useQuery({
    queryKey: ["artifactRows", selected?.id],
    queryFn: () => client.getArtifactRows(selected!.id, 0, PAGE),
    enabled: active === true && !!selected && selected.rowCount > 0,
  });

  const filtered = useMemo(() => {
    let out = rows ?? [];
    if (search) {
      const needle = search.toLowerCase();
      out = out.filter((r) =>
        Object.values(r).some((v) => v !== null && String(v).toLowerCase().includes(needle)),
      );
    }
    if (sort.by !== "none") {
      const col = sort.by;
      out = [...out].sort((a, b) => {
        const x = a[col];
        const y = b[col];
        if (x === null) return 1;
        if (y === null) return -1;
        const cmp = typeof x === "number" && typeof y === "number"
          ? x - y
          : String(x).localeCompare(String(y));
        return sort.desc ? -cmp : cmp;
      });
    }
    return out;
  }, [rows, search, sort]);

  const searchNode = (
    <ListSearch value={rawSearch} onChange={setRawSearch} placeholder="Search rows" />
  );
  // Sort fields come from the artifact's own columns — the view has no idea
  // what they are, which is the point.
  const sortNode = selected ? (
    <SortControl
      fields={selected.columns.map((c) => ({ value: c, label: c }))}
      value={sort}
      onChange={setSort}
    />
  ) : null;

  // The title is the DESTINATION, not the selected artifact. Showing "App
  // permissions" here read better with one artifact but left no way to tell you
  // were in Artifacts at all — the design lint caught exactly that. The
  // artifact's own name belongs on its chip, which is why the chip row is always
  // rendered even for a single artifact.
  const toolbar = useMemo(
    () =>
      active === true
        ? {
            title: "Artifacts",
            count: selected ? filtered.length : 0,
            search: selected ? searchNode : undefined,
            sort: selected ? sortNode : undefined,
          }
        : null,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [active, selected, filtered.length, rawSearch, sort],
  );
  useViewToolbar(toolbar);

  // An artifact gated on encryption is listed even when empty, so it can say
  // why rather than vanish (#197) — an absent artifact and an impossible one
  // are different facts.
  const gatedMessage = useEncryptedOnlyEmpty(
    selected?.name ?? "This data",
    `No rows in ${selected?.name ?? "this artifact"}.`,
  );

  if (active === false) {
    return (
      <NoBackupState
        icon={Boxes}
        title="Browse everything else"
        lead="The long tail of what a backup holds — app permissions, alarms, known networks and more — each read by a small declarative module."
        features={[
          { label: "Pick an artifact", detail: "Only the ones this backup actually contains are listed." },
          { label: "Search", detail: "Search across every column at once." },
          { label: "Sort", detail: "Order by any column." },
        ]}
        note="Read locally from the backup on this Mac."
      />
    );
  }

  // Three different truths, and only one of them is "the device had none".
  //
  // A backup imported before a module existed has no rows, and saying "contained
  // none" there is a claim the user cannot check — it was the first thing the
  // owner hit on the first real run. `stale` is the same problem one update
  // later: rows exist, but from a smaller module set.
  const needsExtraction = extraction === "never-run" || extraction === "stale";
  if (!listPending && (needsExtraction || list.length === 0)) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
        <p className="max-w-md text-sm text-muted-foreground">
          {extraction === "never-run"
            ? "This backup was imported before these artifacts could be read, so nothing has been extracted from it yet."
            : extraction === "stale"
              ? "TraceLoupe can read more artifacts than were extracted from this backup."
              : "This backup contained none of the artifacts TraceLoupe can read yet."}
        </p>
        {needsExtraction && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button onClick={runExtraction} disabled={extracting} size="sm">
                {extracting ? "Extracting…" : "Extract artifacts"}
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              Reads them from the backup on disk — no need to re-import it
            </TooltipContent>
          </Tooltip>
        )}
        {extractError && (
          <p className="max-w-md select-text text-xs text-destructive">{extractError}</p>
        )}
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {/* One chip per artifact — always shown, including for a single one,
          because this is where the artifact's name lives now that the toolbar
          says where you are instead. It grows into the navigation question
          (#195) rather than pre-empting it. */}
      {list.length > 0 && (
        <div className="flex flex-wrap gap-1.5 border-b px-3 py-2">
          {list.map((a) => (
            <Tooltip key={a.id}>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  onClick={() => setSelectedId(a.id)}
                  className={cn(
                    "rounded-md px-2 py-1 text-xs",
                    a.id === selected?.id
                      ? "bg-accent text-accent-foreground"
                      : "text-muted-foreground hover:bg-muted",
                  )}
                >
                  {a.name}
                  <Badge variant="secondary" className="ml-1.5">
                    {a.rowCount}
                  </Badge>
                </button>
              </TooltipTrigger>
              <TooltipContent>
                {a.category ? `${a.category} · ` : ""}
                {a.rowCount} {a.rowCount === 1 ? "row" : "rows"}
              </TooltipContent>
            </Tooltip>
          ))}
        </div>
      )}

      <div className="min-h-0 flex-1">
        {/* Three different facts, three different messages. An artifact absent
            because the app was never installed is not in `list` at all; one
            that CANNOT exist in this backup explains that; one that exists and
            is simply empty says so. Collapsing any two is the failure this
            whole thread exists to prevent — so the gated case is decided by
            asking the artifact, not by inferring it from a zero row count. */}
        {selected && selected.rowCount === 0 ? (
          <div className="flex h-full items-center justify-center p-8 text-center">
            <p className="max-w-md text-sm text-muted-foreground">
              {selected.requiresEncryptedBackup
                ? gatedMessage
                : `No rows in ${selected.name}.`}
            </p>
          </div>
        ) : selected && !rowsPending ? (
          <ArtifactTable artifact={selected} rows={filtered} search={search} />
        ) : null}
      </div>
    </div>
  );
}
