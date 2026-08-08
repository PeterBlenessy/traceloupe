/**
 * The raw database view: an app's tables and rows exactly as stored.
 *
 * Every parser in this app is an opinion about a schema, and an opinion can be
 * wrong without saying so — WhatsApp imported *nothing* for months because one
 * column was read off the wrong table, and an app with no messages looks exactly
 * like a device with no messages (#362).
 *
 * This is the fallback that makes such a thing visible. Nothing here interprets:
 * the table list is whatever SQLite reports, the rows are the stored values, and
 * the only additions are described rather than substituted — a blob says what it
 * is instead of dumping bytes, and a timestamp shows its decoded date *beside*
 * the raw number, never instead of it.
 *
 * The table is generated from whatever columns arrive, so an app whose schema
 * nobody has ever seen needs no work here to be readable.
 */
import { useEffect, useMemo, useState } from "react";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Database, Table2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  EmptyView,
  ErrorState,
  ListSearch,
  ListSkeleton,
  NoBackupState,
} from "@/components/view";
import { useViewToolbar } from "@/components/toolbar-context";
import { formatCount, formatDateTime } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { cn } from "@/lib/utils";
import { client, type RawCell } from "@/lib/ipc";

/** Rows per page. Matches the backend's own ceiling. */
const PAGE = 200;

export function RawDbView() {
  const navigate = useNavigate();
  const { app, db, table } = useSearch({ strict: false }) as {
    app?: string;
    db?: string;
    table?: string;
  };
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);
  const debounced = useDebounced(search, 250);

  const { data: hasBackup } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });

  const databases = useQuery({
    queryKey: ["rawDatabases", app],
    queryFn: () => client.rawDatabases(app!),
    enabled: !!app && !!hasBackup,
  });

  // Land on the first database rather than an empty pane: opening this view
  // already says which app is being inspected.
  const activeDb = db ?? databases.data?.[0]?.relativePath;

  const tables = useQuery({
    queryKey: ["rawTables", activeDb],
    queryFn: () => client.rawTables(activeDb!),
    enabled: !!activeDb && !!hasBackup,
  });

  // Biggest table first — it is nearly always the one worth reading, and an
  // alphabetical list buries it among Core Data bookkeeping.
  const orderedTables = useMemo(
    () => [...(tables.data ?? [])].sort((a, b) => b.rows - a.rows),
    [tables.data],
  );
  const activeTable = table ?? orderedTables[0]?.name;

  const rows = useQuery({
    queryKey: ["rawRows", activeDb, activeTable, page, debounced],
    queryFn: () =>
      client.rawRows({
        relativePath: activeDb!,
        table: activeTable!,
        offset: page * PAGE,
        limit: PAGE,
        search: debounced || null,
      }),
    enabled: !!activeDb && !!activeTable && !!hasBackup,
  });

  // A search or a change of table invalidates the page number; keeping it would
  // show "no rows" for a table that has plenty.
  useEffect(() => {
    setPage(0);
  }, [debounced, activeTable, activeDb]);

  const go = (next: { db?: string; table?: string }) =>
    navigate({
      to: "/raw-db",
      search: { app, db: next.db ?? activeDb, table: next.table },
    });

  // Publishes into the ONE app toolbar — `search`/`title` are the contract's own
  // slots. An earlier version passed `left`/`right`, which TypeScript accepted
  // (excess-property checks do not reach a value passed through useMemo) and the
  // toolbar silently ignored, so the search box simply never appeared.
  useViewToolbar(
    useMemo(
      () => ({
        title: app ? `Raw data — ${app}` : "Raw data",
        count: rows.data?.total,
        search: (
          <ListSearch
            value={search}
            onChange={setSearch}
            placeholder="Search this table"
          />
        ),
      }),
      [app, search, rows.data?.total],
    ),
  );

  if (!hasBackup) {
    return (
      <NoBackupState
        icon={Database}
        title="Read the raw data"
        lead="An app's own database, table by table and row by row, exactly as it is stored — no parsing in the way."
        features={[
          { label: "Every table", detail: "With row counts, including ones nothing else reads." },
          { label: "Search", detail: "Across every column, without knowing which holds what." },
          { label: "Honest cells", detail: "A blob says what it is; a timestamp shows its date beside the stored number." },
        ]}
      />
    );
  }
  if (!app) {
    return (
      <EmptyView
        icon={Database}
        title="No app chosen"
        description="Open this from an app in the Apps view."
      />
    );
  }
  if (databases.error) return <ErrorState error={databases.error} />;
  if (databases.isPending) return <ListSkeleton />;
  if ((databases.data?.length ?? 0) === 0) {
    return (
      <EmptyView
        icon={Database}
        title="No database in this backup"
        description={`${app} has no SQLite store the backup contains. Many apps keep their data server-side, and iOS lets an app exclude its own files from a backup.`}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 border-b px-3 py-1.5">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 text-xs text-muted-foreground"
              onClick={() => navigate({ to: "/apps" })}
            >
              <ArrowLeft className="size-3.5" />
              Apps
            </Button>
          </TooltipTrigger>
          <TooltipContent>Back to the app list</TooltipContent>
        </Tooltip>
        <span className="truncate font-mono text-2xs text-muted-foreground/70">
          {activeDb}
        </span>
      </div>
      <div className="flex min-h-0 flex-1">
      {/* Databases + tables. One rail, because a database with one table should
          not cost two panes. */}
      <div className="flex w-64 shrink-0 flex-col border-r">
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-2">
            {(databases.data ?? []).map((d) => (
              <div key={d.relativePath} className="mb-3">
                <button
                  type="button"
                  data-slot="list-row"
                  onClick={() => go({ db: d.relativePath, table: undefined })}
                  className={cn(
                    "flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-left text-xs font-medium hover:bg-accent/50",
                    d.relativePath === activeDb && "bg-accent",
                  )}
                >
                  <Database className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="truncate">{d.name}</span>
                </button>
                {d.relativePath === activeDb &&
                  orderedTables.map((t) => (
                    <button
                      key={t.name}
                      type="button"
                      data-slot="list-row"
                      onClick={() => go({ table: t.name })}
                      className={cn(
                        "flex w-full items-center gap-1.5 rounded-md py-1 pr-2 pl-6 text-left text-xs hover:bg-accent/50",
                        t.name === activeTable && "bg-accent font-medium",
                      )}
                    >
                      <Table2 className="size-3 shrink-0 text-muted-foreground" />
                      <span className="min-w-0 flex-1 truncate">{t.name}</span>
                      {/* -1 means present but uncountable. Saying so beats a
                          confident wrong number or a silent omission. */}
                      <span className="shrink-0 text-3xs text-muted-foreground">
                        {t.rows < 0 ? "?" : formatCount(t.rows)}
                      </span>
                    </button>
                  ))}
              </div>
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* The table itself. */}
      <div className="flex min-w-0 flex-1 flex-col">
        {rows.error ? (
          <ErrorState error={rows.error} />
        ) : rows.isPending ? (
          <ListSkeleton />
        ) : (rows.data?.total ?? 0) === 0 ? (
          <EmptyView
            icon={Table2}
            title={debounced ? "Nothing matches" : "This table is empty"}
            description={
              debounced
                ? `No row in ${activeTable} contains "${debounced}".`
                : `${activeTable} has no rows in this backup.`
            }
          />
        ) : (
          <>
            <div className="flex items-baseline justify-between gap-4 border-b px-4 py-2">
              <span className="truncate text-sm font-medium">{activeTable}</span>
              <span className="shrink-0 text-xs text-muted-foreground">
                {debounced
                  ? `${formatCount(rows.data!.total)} matching`
                  : `${formatCount(rows.data!.total)} rows`}
              </span>
            </div>
            {/* Wide schemas are normal here, so the table scrolls in both
                directions inside its own box rather than widening the window. */}
            <div className="min-h-0 flex-1 overflow-auto">
              <table className="w-max min-w-full border-collapse text-xs">
                <thead className="sticky top-0 z-10 bg-background/95 backdrop-blur">
                  <tr>
                    {rows.data!.columns.map((c) => (
                      <th
                        key={c}
                        className="border-b border-r px-2 py-1.5 text-left font-semibold whitespace-nowrap last:border-r-0"
                      >
                        {c}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {rows.data!.rows.map((row, i) => (
                    <tr key={i} className="hover:bg-accent/40">
                      {row.map((cell, j) => (
                        <td
                          key={j}
                          className="max-w-96 border-b border-r px-2 py-1 align-top last:border-r-0"
                        >
                          <Cell cell={cell} />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {rows.data!.total > PAGE && (
              <div className="flex items-center justify-between gap-4 border-t px-4 py-2">
                <span className="text-xs text-muted-foreground">
                  {formatCount(page * PAGE + 1)}–
                  {formatCount(Math.min((page + 1) * PAGE, rows.data!.total))} of{" "}
                  {formatCount(rows.data!.total)}
                </span>
                <div className="flex gap-1">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page === 0}
                    onClick={() => setPage((p) => Math.max(0, p - 1))}
                  >
                    Previous
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={(page + 1) * PAGE >= rows.data!.total}
                    onClick={() => setPage((p) => p + 1)}
                  >
                    Next
                  </Button>
                </div>
              </div>
            )}
          </>
        )}
      </div>
      </div>
    </div>
  );
}

/** One cell. NULL is shown as a marker, not as an empty string that could be
 *  mistaken for an empty text value — the difference matters in an examination. */
function Cell({ cell }: { cell: RawCell }) {
  if (cell.kind === "null") {
    return <span className="text-faint-foreground italic">NULL</span>;
  }
  if (cell.kind === "blob") {
    return (
      <Badge variant="outline" className="gap-1 px-1.5 py-0 text-3xs font-normal">
        {cell.text}
      </Badge>
    );
  }
  return (
    <span className="font-mono break-words whitespace-pre-wrap">
      {cell.text}
      {cell.decodedUnix != null && (
        /* Beside the stored value, never instead of it. Apple writes time three
           ways in one database, so the raw number alone is unreadable — but the
           raw number is what a raw view exists to show. */
        <span className="ml-2 font-sans text-muted-foreground">
          {formatDateTime(cell.decodedUnix)}
        </span>
      )}
    </span>
  );
}
