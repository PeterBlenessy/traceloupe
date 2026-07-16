import { useMemo, useState, type FormEvent } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
  Image as ImageIcon,
  ListChecks,
  Lock,
  LockOpen,
  NotebookText,
  Paperclip,
  Pin,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemTitle,
} from "@/components/ui/item";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import {
  SortControl,
  sortItems,
  type SortState,
} from "@/components/sort-control";
import { TimeFilterBar, useTimePresets } from "@/components/time-filter";
import { BadgeFilter } from "@/components/badge-filter";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  EmptyView,
  ErrorState,
  ListDetail,
  ListSearch,
  PanelHeader,
  ViewHeader,
} from "@/components/view";
import { formatDateTime, formatListTime } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { client, type Note, type TimeRange } from "@/lib/ipc";

/** A flattened list row: either a section header or a note (so the virtualized
 *  list can render Apple Notes-style date groups inline). */
type NoteRowItem =
  | { kind: "header"; key: string; label: string }
  | { kind: "note"; key: number; note: Note };

const MS_DAY = 86_400_000;

/**
 * The recency bucket a note falls in, keyed off its modified date — matching the
 * Notes app: Today, Yesterday, Previous 7/30 Days, then by month within the
 * current year and by year before that.
 */
function dateBucket(
  modifiedAt: number | null,
  now: Date,
): { key: string; label: string } {
  if (modifiedAt == null) return { key: "none", label: "No date" };
  const t = modifiedAt * 1000;
  const startOfToday = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
  ).getTime();
  if (t >= startOfToday) return { key: "today", label: "Today" };
  if (t >= startOfToday - MS_DAY)
    return { key: "yesterday", label: "Yesterday" };
  if (t >= startOfToday - 7 * MS_DAY)
    return { key: "prev7", label: "Previous 7 Days" };
  if (t >= startOfToday - 30 * MS_DAY)
    return { key: "prev30", label: "Previous 30 Days" };
  const d = new Date(t);
  if (d.getFullYear() === now.getFullYear()) {
    return {
      key: `m-${d.getMonth()}`,
      label: d.toLocaleString(undefined, { month: "long" }),
    };
  }
  return { key: `y-${d.getFullYear()}`, label: String(d.getFullYear()) };
}

/**
 * Flatten already-filtered+sorted notes into header/note rows. When sorted by
 * modified date, notes are grouped into a Pinned section (always first) followed
 * by recency date sections; any other sort stays a flat list (headers would be
 * meaningless). Section order follows the sort direction; within a section the
 * incoming note order is preserved.
 */
function groupNotes(notes: Note[], sort: SortState, now: Date): NoteRowItem[] {
  if (sort.by !== "modified") {
    return notes.map((n) => ({ kind: "note", key: n.id, note: n }));
  }
  type Section = { key: string; label: string; order: number; notes: Note[] };
  const sections = new Map<string, Section>();
  for (const n of notes) {
    const { key, label } = n.pinned
      ? { key: "pinned", label: "Pinned" }
      : dateBucket(n.modifiedAt, now);
    // Order sections by their most-recent note; null dates sort oldest.
    const order = n.modifiedAt ?? -Infinity;
    const s = sections.get(key);
    if (s) {
      s.notes.push(n);
      s.order = Math.max(s.order, order);
    } else {
      sections.set(key, { key, label, order, notes: [n] });
    }
  }
  const pinned = sections.get("pinned");
  sections.delete("pinned");
  const dated = [...sections.values()].sort((a, b) =>
    sort.desc ? b.order - a.order : a.order - b.order,
  );
  const ordered = pinned ? [pinned, ...dated] : dated;

  const rows: NoteRowItem[] = [];
  for (const s of ordered) {
    rows.push({ kind: "header", key: `h-${s.key}`, label: s.label });
    for (const n of s.notes) rows.push({ kind: "note", key: n.id, note: n });
  }
  return rows;
}

export function NotesView() {
  const navigate = useNavigate();
  // Subscribe to the clock preference so times re-render on change.
  const { clockFormat } = useSettings();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: notes,
    isPending,
    error,
  } = useQuery({
    queryKey: ["notes"],
    queryFn: () => client.listNotes(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [sort, setSort] = usePersistedState<SortState>("notes:sort", { by: "modified", desc: true });
  // Filters — all derived client-side from the note metadata we already hold.
  const [folder, setFolder] = usePersistedState<string>("notes:folder", "all");
  const [lockState, setLockState] = usePersistedState<string>("notes:lock", "all");
  const [tag, setTag] = usePersistedState<string>("notes:tag", "all");
  // Free-text search over title / snippet / folder.
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim().toLowerCase());
  // Time filter — same presets + custom range as Timeline/Photos, over the
  // note's modified date (replaces the old year dropdown).
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });

  // The distinct folders present, for the folder dropdown.
  const folders = useMemo(
    () =>
      Array.from(
        new Set(
          (notes ?? []).map((n) => n.folder).filter((f): f is string => !!f),
        ),
      ).sort((a, b) => a.localeCompare(b)),
    [notes],
  );
  const hasLocked = useMemo(() => (notes ?? []).some((n) => n.locked), [notes]);
  // The distinct hashtag tags present, for the tag facet.
  const tags = useMemo(
    () =>
      Array.from(new Set((notes ?? []).flatMap((n) => n.tags))).sort((a, b) =>
        a.localeCompare(b),
      ),
    [notes],
  );
  // Clamp persisted filters to what THIS backup actually has, so a stale
  // `notes:folder`/`notes:lock` from another backup can't silently empty the list
  // (its control may be hidden, leaving no way to reset).
  const effFolder = folder !== "all" && folders.includes(folder) ? folder : "all";
  const effLock = hasLocked ? lockState : "all";
  const effTag = tag !== "all" && tags.includes(tag) ? tag : "all";

  // Whether a note's modified date falls in a [lo, hi) window (undated notes
  // only pass the fully-open "All" window).
  const inWindow = (
    modifiedAt: number | null,
    lo: number | null,
    hi: number | null,
  ) => {
    if (lo == null && hi == null) return true;
    if (modifiedAt == null) return false;
    return (lo == null || modifiedAt >= lo) && (hi == null || modifiedAt < hi);
  };

  // Folder + lock filtered, before the time filter — the base for both the list
  // and the time-chip counts.
  const baseFiltered = useMemo(() => {
    return (notes ?? []).filter((n) => {
      if (effFolder !== "all" && n.folder !== effFolder) return false;
      if (effLock === "locked" && !n.locked) return false;
      if (effLock === "unlocked" && n.locked) return false;
      if (effTag !== "all" && !n.tags.includes(effTag)) return false;
      if (search) {
        const hay = [n.title, n.snippet, n.folder]
          .filter(Boolean)
          .join(" ")
          .toLowerCase();
        if (!hay.includes(search)) return false;
      }
      return true;
    });
  }, [notes, effFolder, effLock, effTag, search]);

  const presetCounts = useMemo(
    () =>
      presets.map(
        (p) => baseFiltered.filter((n) => inWindow(n.modifiedAt, p.lo, p.hi)).length,
      ),
    [presets, baseFiltered],
  );

  const sortedNotes = useMemo(() => {
    if (!notes) return notes;
    const filtered = baseFiltered.filter((n) =>
      inWindow(n.modifiedAt, range.lo, range.hi),
    );
    return sortItems(
      filtered,
      (n) =>
        sort.by === "title" ? (n.title ?? "").toLowerCase() : n.modifiedAt,
      sort.desc,
    );
  }, [notes, baseFiltered, sort, range]);

  // Header/note rows for the list: Pinned + date sections when sorted by date.
  const rows = useMemo(
    () => (sortedNotes ? groupNotes(sortedNotes, sort, new Date()) : []),
    [sortedNotes, sort],
  );

  if (active === false) {
    return (
      <EmptyView
        icon={NotebookText}
        title="No backup open"
        description="Import a backup to see notes."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const selected =
    sortedNotes?.find((n) => n.id === selectedId) ?? sortedNotes?.[0] ?? null;

  const hasNotes = (notes?.length ?? 0) > 0;

  return (
    <div className="flex h-full flex-col">
      <PanelHeader
        title="Notes"
        count={sortedNotes?.length}
        actions={
          hasNotes ? (
            <>
              {folders.length > 1 && (
                <Select value={effFolder} onValueChange={setFolder}>
                  <SelectTrigger size="sm" className="h-7 w-[9rem] text-xs">
                    <SelectValue placeholder="Folder" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">All folders</SelectItem>
                    {folders.map((f) => (
                      <SelectItem key={f} value={f}>
                        {f}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              {hasLocked && (
                <BadgeFilter
                  value={effLock}
                  onChange={setLockState}
                  options={[
                    { value: "all", label: "All" },
                    {
                      value: "unlocked",
                      label: "Unlocked",
                      icon: <LockOpen className="size-3.5" />,
                    },
                    {
                      value: "locked",
                      label: "Locked",
                      icon: <Lock className="size-3.5" />,
                    },
                  ]}
                />
              )}
            </>
          ) : undefined
        }
        search={
          hasNotes ? (
            <ListSearch value={q} onChange={setQ} placeholder="Search notes" />
          ) : undefined
        }
        toolbar={
          hasNotes ? (
            <>
              {tags.length > 0 && (
                <BadgeFilter
                  value={effTag}
                  onChange={setTag}
                  options={[
                    { value: "all", label: "All tags" },
                    ...tags.map((t) => ({
                      value: t,
                      label: t,
                      count: (notes ?? []).filter((n) => n.tags.includes(t)).length,
                    })),
                  ]}
                />
              )}
              <TimeFilterBar
                className="flex-1"
                presets={presets}
                value={range}
                onChange={setRange}
                counts={presetCounts}
              />
              <SortControl
                fields={[
                  { value: "modified", label: "Modified" },
                  { value: "title", label: "Title" },
                ]}
                value={sort}
                onChange={setSort}
              />
            </>
          ) : undefined
        }
      />
      {/* Then the note list + content panel. */}
      <div className="min-h-0 flex-1">
        <ListDetail
          master={
            error ? (
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
              <p className="px-4 py-6 text-sm text-muted-foreground">
                No notes in this backup.
              </p>
            ) : (sortedNotes?.length ?? 0) === 0 ? (
              <p className="px-4 py-6 text-sm text-muted-foreground">
                No notes match these filters.
              </p>
            ) : (
              <VirtualList
                key={clockFormat}
                items={rows}
                getKey={(r) => r.key}
                estimateSize={64}
                renderItem={(r) =>
                  r.kind === "header" ? (
                    <SectionHeader label={r.label} />
                  ) : (
                    <div className="px-2 py-0.5">
                      <NoteRow
                        note={r.note}
                        active={selected?.id === r.note.id}
                        onClick={() => setSelectedId(r.note.id)}
                      />
                    </div>
                  )
                }
              />
            )
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
      </div>
    </div>
  );
}

/** A note's display title: its title, else the first line of the snippet. */
function noteTitle(n: Note): string {
  return (
    n.title?.trim() ||
    n.snippet?.trim() ||
    (n.locked ? "Locked note" : "Untitled note")
  );
}

/** A date/Pinned group label between note rows. */
function SectionHeader({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-1.5 bg-background/95 px-3 pb-1 pt-3 text-xs font-semibold text-muted-foreground">
      {label === "Pinned" && <Pin className="size-3" />}
      {label}
    </div>
  );
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
    <Item
      asChild
      data-active={active}
      className="rounded-md transition-colors hover:bg-accent/50 data-[active=true]:bg-accent data-[active=true]:hover:bg-accent"
    >
      <button onClick={onClick} className="w-full text-left">
        <ItemContent className="gap-0.5">
          <div className="flex items-baseline justify-between gap-2">
            <ItemTitle className="flex min-w-0 items-center gap-1.5">
              {note.pinned && (
                <Pin className="size-3.5 shrink-0 text-muted-foreground" />
              )}
              {note.locked && (
                <Lock className="size-3.5 shrink-0 text-muted-foreground" />
              )}
              <span className="truncate">{noteTitle(note)}</span>
              {note.hasChecklist && (
                <ListChecks
                  className="size-3.5 shrink-0 text-muted-foreground"
                  aria-label="Checklist"
                />
              )}
              {note.imageCount > 0 && (
                <span
                  className="inline-flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground"
                  title={`${note.imageCount} image${note.imageCount > 1 ? "s" : ""}`}
                >
                  <ImageIcon className="size-3.5" />
                  {note.imageCount}
                </span>
              )}
              {note.attachmentCount > note.imageCount && (
                <span
                  className="inline-flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground"
                  title="Other attachments (tables, drawings, files)"
                >
                  <Paperclip className="size-3.5" />
                  {note.attachmentCount - note.imageCount}
                </span>
              )}
            </ItemTitle>
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatListTime(note.modifiedAt)}
            </span>
          </div>
          <ItemDescription className="truncate">
            {note.locked
              ? "Password protected"
              : (note.snippet ?? note.folder ?? "")}
          </ItemDescription>
        </ItemContent>
        {note.hasImage && (
          // A thumbnail of the note's first image (like Apple Notes). Hidden if
          // the image can't be served (resolution is best-effort).
          <img
            src={client.noteImageUrl(note.id)}
            alt=""
            loading="lazy"
            className="ml-2 size-11 shrink-0 self-center rounded-md bg-muted object-cover"
            onError={(e) => {
              e.currentTarget.style.display = "none";
            }}
          />
        )}
      </button>
    </Item>
  );
}

function NoteDetail({ note }: { note: Note }) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={noteTitle(note)}>
        {note.folder && (
          <span className="text-xs text-muted-foreground">{note.folder}</span>
        )}
      </ViewHeader>
      {/* min-h-0 lets this flex child shrink to the pane height so the ScrollArea
          actually clips + scrolls, instead of growing with the note body. */}
      <ScrollArea className="min-h-0 flex-1">
        <div className="max-w-2xl p-6">
          {note.modifiedAt && (
            <p className="mb-4 text-xs text-muted-foreground">
              {formatDateTime(note.modifiedAt)}
            </p>
          )}
          {note.tags.length > 0 && (
            <div className="mb-4 flex flex-wrap gap-1.5">
              {note.tags.map((t) => (
                <span
                  key={t}
                  className="rounded-full bg-accent px-2 py-0.5 text-xs text-muted-foreground"
                >
                  {t}
                </span>
              ))}
            </div>
          )}
          {note.locked ? (
            // Keyed by id so the password field / unlocked body reset per note.
            <LockedNote key={note.id} note={note} />
          ) : (
            <div className="select-text whitespace-pre-wrap break-words text-sm leading-relaxed">
              {note.body ?? note.snippet ?? "(empty note)"}
            </div>
          )}
        </div>
      </ScrollArea>
    </div>
  );
}

/** A locked note: prompt for the password, decrypt on demand, show the body. The
 *  decrypted text lives only in component state (session), never persisted. */
function LockedNote({ note }: { note: Note }) {
  const [password, setPassword] = useState("");
  const [body, setBody] = useState<string | null>(null);
  const [unlocking, setUnlocking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function unlock(e: FormEvent) {
    e.preventDefault();
    if (!password || unlocking) return;
    setUnlocking(true);
    setError(null);
    try {
      setBody(await client.unlockNote(note.id, password));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setUnlocking(false);
    }
  }

  if (body !== null) {
    return (
      <div className="select-text whitespace-pre-wrap break-words text-sm leading-relaxed">
        {body || "(empty note)"}
      </div>
    );
  }

  return (
    <form
      onSubmit={unlock}
      className="mx-auto max-w-sm space-y-3 pt-6 text-center"
    >
      <div className="flex flex-col items-center gap-2">
        <div className="flex size-12 items-center justify-center rounded-full bg-accent">
          <Lock className="size-5 text-muted-foreground" />
        </div>
        <p className="text-sm text-muted-foreground">
          This note is password protected.
        </p>
        {note.passwordHint && (
          <p className="text-xs text-muted-foreground">
            Hint: {note.passwordHint}
          </p>
        )}
      </div>
      <Input
        type="password"
        autoFocus
        placeholder="Note password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        className="text-center select-text"
      />
      {error && <p className="text-xs text-destructive">{error}</p>}
      <Button
        type="submit"
        disabled={!password || unlocking}
        className="w-full"
      >
        {unlocking ? "Unlocking…" : "Unlock"}
      </Button>
    </form>
  );
}
