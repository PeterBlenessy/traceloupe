import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { NotebookText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { Item, ItemContent, ItemDescription, ItemTitle } from "@/components/ui/item";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { EmptyView, ErrorState, ListDetail, ViewHeader } from "@/components/view";
import { formatDateTime, formatListTime } from "@/lib/format";
import { client, type Note } from "@/lib/ipc";

export function NotesView() {
  const navigate = useNavigate();
  // Subscribe to the clock preference so times re-render on change.
  const { clockFormat } = useSettings();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: notes, isPending, error } = useQuery({
    queryKey: ["notes"],
    queryFn: () => client.listNotes(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [sort, setSort] = useState<SortState>({ by: "modified", desc: true });
  const sortedNotes = useMemo(
    () =>
      notes
        ? sortItems(
            notes,
            (n) => (sort.by === "title" ? (n.title ?? "").toLowerCase() : n.modifiedAt),
            sort.desc,
          )
        : notes,
    [notes, sort],
  );

  if (active === false) {
    return (
      <EmptyView icon={NotebookText} title="No backup open" description="Import a backup to see notes.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const selected = sortedNotes?.find((n) => n.id === selectedId) ?? sortedNotes?.[0] ?? null;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Notes" count={notes?.length} />
          {(notes?.length ?? 0) > 0 && (
            <div className="flex shrink-0 justify-end border-b px-2 py-1.5">
              <SortControl
                fields={[
                  { value: "modified", label: "Modified" },
                  { value: "title", label: "Title" },
                ]}
                value={sort}
                onChange={setSort}
              />
            </div>
          )}
          {error ? (
            <ErrorState error={error} />
          ) : isPending ? (
            <div className="min-h-0 flex-1 overflow-auto">
              {Array.from({ length: 6 }).map((_, i) => (
                <div key={i} className="px-3 py-2">
                  <Skeleton className="h-12 w-full" />
                </div>
              ))}
            </div>
          ) : (notes?.length ?? 0) === 0 ? (
            <p className="px-4 py-6 text-sm text-muted-foreground">No notes in this backup.</p>
          ) : (
            <VirtualList
              key={clockFormat}
              items={sortedNotes!}
              getKey={(n) => n.id}
              estimateSize={64}
              renderItem={(n) => (
                <NoteRow
                  note={n}
                  active={selected?.id === n.id}
                  onClick={() => setSelectedId(n.id)}
                />
              )}
            />
          )}
        </>
      }
      detail={
        selected ? (
          <NoteDetail note={selected} />
        ) : (
          !isPending && (
            <EmptyView
              icon={NotebookText}
              title="No note selected"
              description="Pick a note on the left."
            />
          )
        )
      }
    />
  );
}

/** A note's display title: its title, else the first line of the snippet. */
function noteTitle(n: Note): string {
  return n.title?.trim() || n.snippet?.trim() || "Untitled note";
}

function NoteRow({
  note,
  active,
  onClick,
}: {
  note: Note;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button onClick={onClick} className="w-full text-left">
        <ItemContent className="gap-0.5">
          <div className="flex items-baseline justify-between gap-2">
            <ItemTitle className="truncate">{noteTitle(note)}</ItemTitle>
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatListTime(note.modifiedAt)}
            </span>
          </div>
          <ItemDescription className="truncate">
            {note.snippet ?? note.folder ?? ""}
          </ItemDescription>
        </ItemContent>
      </button>
    </Item>
  );
}

function NoteDetail({ note }: { note: Note }) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={noteTitle(note)}>
        {note.folder && <span className="text-xs text-muted-foreground">{note.folder}</span>}
      </ViewHeader>
      <ScrollArea className="flex-1">
        <div className="mx-auto max-w-2xl p-6">
          {note.modifiedAt && (
            <p className="mb-4 text-xs text-muted-foreground">{formatDateTime(note.modifiedAt)}</p>
          )}
          <div className="select-text whitespace-pre-wrap break-words text-sm leading-relaxed">
            {note.body ?? note.snippet ?? "(empty note)"}
          </div>
        </div>
      </ScrollArea>
    </div>
  );
}
