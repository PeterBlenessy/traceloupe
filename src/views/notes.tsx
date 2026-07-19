import { useMemo, useState, type FormEvent } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderTree,
  Image as ImageIcon,
  List,
  ListChecks,
  Lock,
  LockOpen,
  NotebookText,
  Paperclip,
  Pin,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemTitle,
} from "@/components/ui/item";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useTimePresets } from "@/components/time-filter";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, multiBadgeGroup, timeGroup, type FilterGroup } from "@/components/filter-groups";
import {
  EmptyView,
  ErrorState,
  ListDetail,
  ListSearch,
  ListSkeleton,
  ViewHeader,
} from "@/components/view";
import { formatDateTime, formatListTime } from "@/lib/format";
import { useDebounced } from "@/lib/use-debounced";
import { cn } from "@/lib/utils";
import { client, type Note, type TimeRange } from "@/lib/ipc";

/** A flattened list row for the virtualized master: a date/section header (flat
 *  view), a folder node (tree view), or a note. */
type NoteRowItem =
  | { kind: "header"; key: string; label: string }
  | { kind: "folder"; key: string; folder: string; count: number; collapsed: boolean }
  | { kind: "note"; key: number; note: Note; indent?: boolean };

/** Label for notes with no folder (e.g. "Recently Deleted" items were labelled
 *  in the parser; a genuine null falls back here). */
const NO_FOLDER = "Notes";

/** Flatten notes into a folder tree: each folder node followed by its notes (when
 *  expanded). Notes keep their incoming (sorted) order within a folder. */
function buildTreeRows(notes: Note[], collapsed: Set<string>): NoteRowItem[] {
  const groups = new Map<string, Note[]>();
  for (const n of notes) {
    const f = n.folder ?? NO_FOLDER;
    const list = groups.get(f);
    if (list) list.push(n);
    else groups.set(f, [n]);
  }
  const rows: NoteRowItem[] = [];
  for (const folder of [...groups.keys()].sort((a, b) => a.localeCompare(b))) {
    const items = groups.get(folder)!;
    const isCollapsed = collapsed.has(folder);
    rows.push({
      kind: "folder",
      key: `folder-${folder}`,
      folder,
      count: items.length,
      collapsed: isCollapsed,
    });
    if (!isCollapsed) {
      for (const n of items) rows.push({ kind: "note", key: n.id, note: n, indent: true });
    }
  }
  return rows;
}

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
  // Flat list (date sections) vs a folder tree of the notes.
  const [viewMode, setViewMode] = usePersistedState<"flat" | "tree">("notes:view", "flat");
  // Collapsed folders in tree view (a folder is expanded unless listed here).
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const toggleFolder = (f: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (!next.delete(f)) next.add(f);
      return next;
    });
  const [sort, setSort] = usePersistedState<SortState>("notes:sort", { by: "modified", desc: true });
  // Filters — all derived client-side from the note metadata we already hold.
  const [folder, setFolder] = usePersistedState<string>("notes:folder", "all");
  const [lockState, setLockState] = usePersistedState<string>("notes:lock", "all");
  const [selectedTags, setSelectedTags] = usePersistedState<string[]>("notes:tags", []);
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
  // Stable reference (memoized) so the filter memos below don't rerun each render.
  const effTags = useMemo(
    () => selectedTags.filter((t) => tags.includes(t)),
    [selectedTags, tags],
  );

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
      if (effTags.length > 0 && !effTags.some((t) => n.tags.includes(t))) return false;
      if (search) {
        const hay = [n.title, n.snippet, n.folder]
          .filter(Boolean)
          .join(" ")
          .toLowerCase();
        if (!hay.includes(search)) return false;
      }
      return true;
    });
  }, [notes, effFolder, effLock, effTags, search]);

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

  // The virtualized master rows: a folder tree in "tree" view, else Pinned +
  // date sections (flat view).
  const rows = useMemo(() => {
    if (!sortedNotes) return [];
    return viewMode === "tree"
      ? buildTreeRows(sortedNotes, collapsed)
      : groupNotes(sortedNotes, sort, new Date());
  }, [sortedNotes, sort, viewMode, collapsed]);

  const hasNotes = (notes?.length ?? 0) > 0;
  // Faceted filters for the single Filter control. Only groups this backup
  // actually has are included — no locked notes → no Lock group at all.
  const filterGroups = useMemo(() => {
    if (!hasNotes) return [];
    const out: FilterGroup[] = [];
    // Folder facet is redundant in tree view (the tree already groups by folder).
    if (viewMode === "flat" && folders.length > 1) {
      const folderOptions: BadgeFilterOption[] = [
        { value: "all", label: "All folders" },
        ...folders.map((f) => ({ value: f, label: f, count: (notes ?? []).filter((n) => n.folder === f).length })),
      ];
      out.push(badgeGroup({ key: "folder", label: "Folder", description: "Which folder the note lives in", options: folderOptions, value: effFolder, onChange: setFolder }));
    }
    if (hasLocked)
      out.push(badgeGroup({
        key: "lock",
        label: "Lock",
        description: "Password-protected notes",
        options: [
          { value: "all", label: "All" },
          { value: "unlocked", label: "Unlocked", icon: <LockOpen className="size-3.5" /> },
          { value: "locked", label: "Locked", icon: <Lock className="size-3.5" /> },
        ],
        value: effLock,
        onChange: setLockState,
      }));
    if (tags.length > 0) {
      const tagOptions: BadgeFilterOption[] = tags.map((t) => ({
        value: t,
        label: t,
        count: (notes ?? []).filter((n) => n.tags.includes(t)).length,
      }));
      out.push(
        multiBadgeGroup({
          key: "tag",
          label: "Tags",
          description: "Hashtags used inside the note (pick any)",
          options: tagOptions,
          selected: effTags,
          onToggle: (t) =>
            setSelectedTags((prev) =>
              prev.includes(t) ? prev.filter((x) => x !== t) : [...prev, t],
            ),
        }),
      );
    }
    out.push(timeGroup({ description: "When the note was last modified", presets, counts: presetCounts, value: range, onChange: setRange }));
    return out;
  }, [hasNotes, viewMode, folders, notes, effFolder, hasLocked, effLock, tags, effTags, presets, presetCounts, range, setFolder, setLockState, setSelectedTags, setRange]);

  // Always-visible controls beside the Filter button.
  const modesNode = useMemo(
    () =>
      hasNotes ? (
        <ToggleGroup
          type="single"
          value={viewMode}
          onValueChange={(v) => v && setViewMode(v as "flat" | "tree")}
          variant="outline"
          size="sm"
        >
          <ToggleGroupItem value="flat" aria-label="List" title="List">
            <List className="size-4" />
          </ToggleGroupItem>
          <ToggleGroupItem value="tree" aria-label="Folders" title="Folder tree">
            <FolderTree className="size-4" />
          </ToggleGroupItem>
        </ToggleGroup>
      ) : undefined,
    [hasNotes, viewMode, setViewMode],
  );
  const sortNode = useMemo(
    () =>
      hasNotes ? (
        <SortControl
          fields={[
            { value: "modified", label: "Modified" },
            { value: "title", label: "Title" },
          ]}
          value={sort}
          onChange={setSort}
        />
      ) : undefined,
    [hasNotes, sort, setSort],
  );
  const searchNode = useMemo(
    () => (hasNotes ? <ListSearch value={q} onChange={setQ} placeholder="Search notes" /> : undefined),
    [hasNotes, q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? {
            title: "Notes",
            count: sortedNotes?.length,
            islands: [],
            filter: filterGroups,
            modes: modesNode,
            sort: sortNode,
            search: searchNode,
          }
        : null,
    [active, sortedNotes?.length, filterGroups, modesNode, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

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

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1">
        <ListDetail
          master={
            error ? (
              <ErrorState error={error} />
            ) : isPending ? (
              <ListSkeleton rows={6} />
            ) : (notes?.length ?? 0) === 0 ? (
              <EmptyView title="No notes in this backup." />
            ) : (sortedNotes?.length ?? 0) === 0 ? (
              <EmptyView title="No notes match these filters." />
            ) : (
              <VirtualList
                key={clockFormat}
                items={rows}
                getKey={(r) => r.key}
                estimateSize={64}
                renderItem={(r) =>
                  r.kind === "header" ? (
                    <SectionHeader label={r.label} />
                  ) : r.kind === "folder" ? (
                    <FolderRow
                      folder={r.folder}
                      count={r.count}
                      collapsed={r.collapsed}
                      onToggle={() => toggleFolder(r.folder)}
                    />
                  ) : (
                    <div className={cn("py-0.5", r.indent ? "pl-6 pr-2" : "px-2")}>
                      <NoteRow
                        note={r.note}
                        active={selected?.id === r.note.id}
                        showFolder={viewMode === "flat"}
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

/** An expandable folder node in tree view, with its child-note count as a chip. */
function FolderRow({
  folder,
  count,
  collapsed,
  onToggle,
}: {
  folder: string;
  count: number;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={!collapsed}
      data-slot="list-row"
      className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-sm font-medium hover:bg-accent/50"
    >
      {collapsed ? (
        <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
      ) : (
        <ChevronDown className="size-4 shrink-0 text-muted-foreground" />
      )}
      <Folder className="size-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate">{folder}</span>
      <Badge variant="secondary" className="shrink-0 tabular-nums">
        {count}
      </Badge>
    </button>
  );
}

function NoteRow({
  note,
  active,
  onClick,
  showFolder = false,
}: {
  note: Note;
  active: boolean;
  onClick: () => void;
  /** Show the note's folder as a chip (flat view; redundant under a folder node). */
  showFolder?: boolean;
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
          <ItemDescription className="flex items-center gap-1.5 truncate">
            {showFolder && note.folder && (
              <Badge
                variant="outline"
                className="shrink-0 gap-1 px-1.5 py-0 text-[10px] font-normal"
              >
                <Folder className="size-2.5" />
                {note.folder}
              </Badge>
            )}
            <span className="truncate">
              {note.locked ? "Password protected" : (note.snippet ?? "")}
            </span>
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
          ) : note.bodyRich ? (
            // Rich HTML is generated by our own parser from a fixed, escaped tag
            // set (headings/lists/checklists/formatting) — safe to render. Links
            // are intercepted so they open externally instead of navigating the
            // WKWebView (which would replace the whole SPA with no way back).
            <div
              className="note-rich select-text break-words text-sm leading-relaxed"
              onClick={(e) => {
                const a = (e.target as HTMLElement).closest("a");
                const href = a?.getAttribute("href");
                if (href) {
                  e.preventDefault();
                  client.openExternal(href);
                }
              }}
              dangerouslySetInnerHTML={{ __html: note.bodyRich }}
            />
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
