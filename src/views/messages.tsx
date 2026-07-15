import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  ArrowDownWideNarrow,
  ArrowLeft,
  ArrowRight,
  ArrowUpNarrowWide,
  FileText,
  GalleryVerticalEnd,
  ImageIcon,
  MessageSquare,
  MessagesSquare,
  Paperclip,
  Users,
} from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { BadgeFilter } from "@/components/badge-filter";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  Message as MessageRow,
  MessageContent,
  MessageHeader,
} from "@/components/ui/message";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import {
  EmptyView,
  ErrorState,
  ListDetail,
  ListSearch,
  ViewHeader,
} from "@/components/view";
import { LazyVirtualList } from "@/components/lazy-virtual-list";
import { VirtualList } from "@/components/virtual-list";
import {
  SortControl,
  sortItems,
  type SortState,
} from "@/components/sort-control";
import { useSettings } from "@/components/settings-provider";
import { cn } from "@/lib/utils";
import {
  formatCount,
  formatDateHeader,
  formatListTime,
  formatMessageTime,
} from "@/lib/format";
import { usePersistedState } from "@/lib/use-persisted-state";
import { TimeFilterBar, useTimePresets } from "@/components/time-filter";
import { initials } from "@/lib/contact";
import { useDebounced } from "@/lib/use-debounced";
import { serviceSlug } from "@/lib/apps";
import { BrandIcon, hasBrandIcon } from "@/lib/brand-icon";
import {
  useContactResolver,
  type ResolvedContact,
} from "@/lib/use-contact-resolver";
import {
  client,
  type Attachment,
  type Message,
  type ThreadSummary,
  type TimeRange,
  type TimelineMessage,
} from "@/lib/ipc";

type Mode = "conversations" | "timeline";

// Grouped content-filter buckets (order + labels for the pills). Kinds not present
// in the current scope are hidden, so the pills only ever show what's available.
const KIND_LABELS: Record<string, string> = {
  text: "Text",
  media: "Photos & Videos",
  link: "Links",
  shared: "Shared",
  sticker: "Stickers",
  system: "System",
};
const KIND_ORDER = ["text", "media", "link", "shared", "sticker", "system"];

/** Clickable content-kind badges (same pill component as the Apps "Native"/"Coming
 *  soon" tags). Shows only the kinds present in `available`, and hides itself unless
 *  there are at least two to choose between. */
function MessageKindFilter({
  available,
  value,
  onChange,
}: {
  available: string[];
  value: string;
  onChange: (v: string) => void;
}) {
  const kinds = KIND_ORDER.filter((k) => available.includes(k));
  if (kinds.length < 2) return null; // one (or no) kind → nothing to filter
  return (
    <BadgeFilter
      value={kinds.includes(value) ? value : "all"}
      onChange={onChange}
      options={[
        { value: "all", label: "All" },
        ...kinds.map((k) => ({ value: k, label: KIND_LABELS[k] })),
      ]}
    />
  );
}

/** A compact oldest/newest toggle — replaces the single-field "Time" sort picker. */
function OrderToggle({ desc, onToggle }: { desc: boolean; onToggle: () => void }) {
  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={onToggle}
      className="h-8 shrink-0 gap-1.5 px-2 text-xs text-muted-foreground"
      title={desc ? "Newest first — click for oldest" : "Oldest first — click for newest"}
    >
      {desc ? (
        <ArrowDownWideNarrow className="size-4" />
      ) : (
        <ArrowUpNarrowWide className="size-4" />
      )}
      {desc ? "Newest" : "Oldest"}
    </Button>
  );
}

export function MessagesView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [mode, setMode] = usePersistedState<Mode>(
    "messages:mode",
    "conversations",
  );
  // Which conversation is open in the master-detail view. Lifted here so that
  // clicking a row in the Timeline view can jump to its conversation.
  const [selectedId, setSelectedId] = useState<number | null>(null);
  // Where a jump into a conversation came from, so the conversation view can
  // offer a "back" button to return to that overview (null = opened normally
  // from the conversation list, so no back button).
  const [openedFrom, setOpenedFrom] = useState<Mode | null>(null);
  const openThread = (threadId: number, from: Mode | null = null) => {
    setOpenedFrom(from);
    setSelectedId(threadId);
    setMode("conversations");
  };
  // Switching mode via the top toggle is a fresh navigation — drop any "back".
  const switchMode = (next: Mode) => {
    setOpenedFrom(null);
    setMode(next);
  };

  // Filter by source app (iMessage / SMS / TikTok / …), shared across all three
  // modes so it applies to both Conversations and Timeline.
  const [serviceFilter, setServiceFilter] = usePersistedState<string>(
    "messages:service",
    "all",
  );
  const { data: threadsForServices } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
    enabled: active === true,
  });
  // Distinct services present + the total message count per service (for the
  // filter chips, e.g. "SMS 200", "TikTok 30 000").
  const { services, serviceCounts, totalCount } = useMemo(() => {
    const counts = new Map<string, number>();
    let total = 0;
    for (const t of threadsForServices ?? []) {
      total += t.messageCount;
      if (t.service)
        counts.set(t.service, (counts.get(t.service) ?? 0) + t.messageCount);
    }
    return {
      services: [...counts.keys()].sort(),
      serviceCounts: counts,
      totalCount: total,
    };
  }, [threadsForServices]);
  // A persisted service that isn't in this backup falls back to "all".
  const service =
    serviceFilter === "all" || !services.includes(serviceFilter)
      ? null
      : serviceFilter;

  // Content-kind filter (grouped buckets). The pill control lives in each view's
  // toolbar; the selection is shared + persisted across Timeline and conversations.
  const [contentKind, setContentKind] = usePersistedState<string>(
    "messages:kind",
    "all",
  );

  // Deep link from elsewhere (e.g. a contact's "Conversations"): ?thread=<id>,
  // or ?service=<label> from the Apps view to preselect that app's chats.
  const search = useSearch({ strict: false }) as {
    thread?: number;
    service?: string;
  };
  useEffect(() => {
    if (search.thread != null) openThread(search.thread);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search.thread]);
  // Apply a `?service=` deep-link ONCE per distinct value — not on every `services`
  // refetch, which would otherwise snap the filter back after the user changed it.
  const appliedServiceRef = useRef<string | null>(null);
  useEffect(() => {
    if (
      search.service &&
      search.service !== appliedServiceRef.current &&
      services.includes(search.service)
    ) {
      setServiceFilter(search.service);
      appliedServiceRef.current = search.service;
    }
  }, [search.service, services, setServiceFilter]);

  if (active === false) {
    return (
      <EmptyView
        icon={MessageSquare}
        title="No backup open"
        description="Import a backup to read its messages."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {/* Match ViewHeader's height/padding (h-14 px-4) so this bar aligns with
          every other view's header; the mode toggle sits beside the title. */}
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
        <h1 className="text-base font-semibold">Messages</h1>
        <ToggleGroup
          type="single"
          value={mode}
          onValueChange={(v) => v && switchMode(v as Mode)}
          variant="outline"
          size="sm"
          className="ml-1"
        >
          <ToggleGroupItem
            value="conversations"
            aria-label="Conversations"
            title="Conversations"
          >
            <MessagesSquare className="size-4" />
          </ToggleGroupItem>
          <ToggleGroupItem value="timeline" aria-label="Timeline" title="Timeline">
            <GalleryVerticalEnd className="size-4" />
          </ToggleGroupItem>
        </ToggleGroup>
        {services.length > 1 && (
          <BadgeFilter
            className="ml-auto"
            // Display the clamped value so a stale persisted service (absent from
            // this backup) highlights "All" rather than no pill at all.
            value={service ?? "all"}
            onChange={setServiceFilter}
            options={[
              { value: "all", label: "All", count: totalCount },
              ...services.map((s) => {
                const slug = serviceSlug(s);
                return {
                  value: s,
                  label: s,
                  count: serviceCounts.get(s) ?? 0,
                  icon: hasBrandIcon(slug) ? (
                    <BrandIcon slug={slug} name={s} className="size-3.5" />
                  ) : undefined,
                };
              }),
            ]}
          />
        )}
      </header>
      <div className="min-h-0 flex-1">
        {mode === "conversations" ? (
          <Conversations
            selectedId={selectedId}
            onSelect={setSelectedId}
            service={service}
            kindValue={contentKind}
            onKindChange={setContentKind}
            onBack={openedFrom ? () => setMode(openedFrom) : undefined}
            backLabel="Timeline"
          />
        ) : (
          <Timeline
            onOpenThread={(id) => openThread(id, "timeline")}
            service={service}
            kindValue={contentKind}
            onKindChange={setContentKind}
          />
        )}
      </div>
    </div>
  );
}

/** Master-detail view: the thread list on the left, one conversation on the right. */
function Conversations({
  selectedId,
  onSelect,
  service,
  kindValue,
  onKindChange,
  onBack,
  backLabel,
}: {
  selectedId: number | null;
  onSelect: (id: number) => void;
  service: string | null;
  kindValue: string;
  onKindChange: (v: string) => void;
  onBack?: () => void;
  backLabel?: string;
}) {
  // Gate on an open backup (React Query dedups this with the parent's copy), so
  // list_threads isn't fired while `hasActiveBackup` is still resolving.
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: threads,
    isPending,
    error,
  } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
    enabled: active === true,
  });
  const resolve = useContactResolver();
  const { showContactNames, showAvatars } = useSettings();

  const [sort, setSort] = usePersistedState<SortState>("messages:sort", {
    by: "recent",
    desc: true,
  });

  // The app filter lives in the shared header; here we just apply it, then sort.
  const visibleThreads = useMemo(() => {
    const list = service
      ? threads?.filter((t) => t.service === service)
      : threads;
    if (!list) return list;
    return sortItems(
      list,
      (t) =>
        sort.by === "name"
          ? (t.displayName || t.identifier || "").toLowerCase()
          : sort.by === "count"
            ? t.messageCount
            : t.lastMessageAt,
      sort.desc,
    );
  }, [threads, service, sort]);

  const selected =
    visibleThreads?.find((t) => t.id === selectedId) ??
    visibleThreads?.[0] ??
    null;

  return (
    <ListDetail
      master={
        <>
          {/* No "Conversations" title here — the mode toggle above already says
              it. Just the count + sort in one slim row. */}
          {(threads?.length ?? 0) > 0 && (
            // h-14 px-4 matches the detail pane's ViewHeader so the two top rows
            // line up in height.
            <div className="flex h-14 shrink-0 items-center justify-between gap-2 border-b px-4">
              <span className="text-xs tabular-nums text-muted-foreground/60">
                {formatCount(visibleThreads?.length ?? 0)}
              </span>
              <SortControl
                fields={[
                  { value: "recent", label: "Recent" },
                  { value: "name", label: "Name" },
                  { value: "count", label: "Messages" },
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
          ) : (visibleThreads?.length ?? 0) === 0 ? (
            <p className="px-4 py-6 text-sm text-muted-foreground">
              {(threads?.length ?? 0) === 0
                ? "No messages in this backup."
                : "No conversations for this app."}
            </p>
          ) : (
            <VirtualList
              items={visibleThreads!}
              getKey={(t) => t.id}
              estimateSize={64}
              renderItem={(t) => (
                <div className="px-2 py-0.5">
                  <ThreadRow
                    thread={t}
                    resolve={resolve}
                    showContactNames={showContactNames}
                    showAvatars={showAvatars}
                    active={selected?.id === t.id}
                    onClick={() => onSelect(t.id)}
                  />
                </div>
              )}
            />
          )}
        </>
      }
      detail={
        selected ? (
          <Conversation
            thread={selected}
            resolve={resolve}
            showContactNames={showContactNames}
            kindValue={kindValue}
            onKindChange={onKindChange}
            onBack={onBack}
            backLabel={backLabel}
          />
        ) : (
          !isPending && (
            <EmptyView
              icon={MessageSquare}
              title="No conversation selected"
              description="Pick a thread on the left."
            />
          )
        )
      }
    />
  );
}

/**
 * The handle for a thread — a phone number or email. iLEAPP puts the chat's
 * ROWID in `identifier` and the actual contact handle in `display_name`, so the
 * handle we resolve/show comes from `displayName`.
 */
function threadHandle(thread: ThreadSummary): string {
  return thread.displayName ?? thread.identifier;
}

type Resolver = (handle: string | null | undefined) => ResolvedContact | null;

function isGroup(thread: ThreadSummary): boolean {
  return thread.participants.length > 1;
}

/** Resolve a single handle to a display string (contact name or the handle). */
function handleLabel(
  handle: string,
  resolve: Resolver,
  showContactNames: boolean,
): string {
  if (!showContactNames) return handle;
  return resolve(handle)?.name ?? handle;
}

/**
 * How a thread is labelled. Group chats show their name if set, otherwise the
 * members' names joined; 1:1 chats show the contact name or the raw handle.
 */
function threadLabel(
  thread: ThreadSummary,
  resolve: Resolver,
  showContactNames: boolean,
): string {
  if (isGroup(thread)) {
    if (thread.displayName?.trim()) return thread.displayName;
    return thread.participants
      .map((h) => handleLabel(h, resolve, showContactNames))
      .join(", ");
  }
  const handle = threadHandle(thread);
  if (!showContactNames) return handle;
  return resolve(handle)?.name ?? handle;
}

/** Every message from every conversation, in one chronological stream. */
function Timeline({
  onOpenThread,
  service,
  kindValue,
  onKindChange,
}: {
  onOpenThread: (threadId: number) => void;
  service: string | null;
  kindValue: string;
  onKindChange: (v: string) => void;
}) {
  const resolve = useContactResolver();
  const { showContactNames, showAvatars } = useSettings();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  // Content kinds present across the timeline (scoped to the service filter).
  const { data: kindsData } = useQuery({
    queryKey: ["messageKinds", null, service],
    queryFn: () => client.messageKinds(null, service),
    enabled: active === true,
  });
  const available = (kindsData ?? []).map(([k]) => k);
  // A selection that isn't present in this scope filters nothing.
  const kind = kindValue !== "all" && available.includes(kindValue) ? kindValue : null;
  // Anchor "now" once so preset bounds and query keys stay stable.
  const { now, presets } = useTimePresets();
  // Oldest-first by default (newest at the bottom); toggle flips to newest-first.
  const [order, setOrder] = useState<SortState>({ by: "time", desc: false });
  // The active time filter as a half-open [lo, hi) range; {null,null} = all time.
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Free-text search over message body / sender / conversation (debounced).
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;

  // Per-preset message counts for the chip labels (e.g. "7d · 812").
  const { data: presetCounts } = useQuery({
    queryKey: ["messageRanges", now, service, search, kind, "presets"],
    queryFn: () =>
      client.countMessageRanges(
        presets.map((p) => ({ lo: p.lo, hi: p.hi })),
        service,
        search,
        kind,
      ),
    enabled: active === true,
  });
  // Count for the active range — sizes the virtual scroller.
  const { data: total } = useQuery({
    queryKey: ["timelineRangeCount", range.lo, range.hi, service, search, kind],
    queryFn: async () =>
      (await client.countMessageRanges([range], service, search, kind))[0] ?? 0,
    enabled: active === true,
  });

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b px-3 py-1.5">
        <ListSearch
          value={q}
          onChange={setQ}
          placeholder="Search messages, sender…"
        />
      </div>
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-1.5">
        <TimeFilterBar
          className="flex-1"
          presets={presets}
          value={range}
          onChange={setRange}
          counts={presetCounts}
        />
        <MessageKindFilter
          available={available}
          value={kindValue}
          onChange={onKindChange}
        />
        <OrderToggle
          desc={order.desc}
          onToggle={() => setOrder({ by: "time", desc: !order.desc })}
        />
      </div>
      <LazyVirtualList<TimelineMessage>
        count={total ?? 0}
        startAtBottom={!order.desc}
        resetKey={`timeline:${service ?? "all"}:${kind ?? "all"}:${range.lo}:${range.hi}:${search}:${order.desc}`}
        estimateSize={56}
        windowKey={(page) => [
          "timelineWindow",
          service,
          kind,
          range.lo,
          range.hi,
          search,
          order.desc,
          page,
        ]}
        fetchWindow={(offset, limit) =>
          client.getRangeWindow(
            range.lo,
            range.hi,
            offset,
            limit,
            service,
            search,
            order.desc,
            kind,
          )
        }
        renderItem={(item, _i, prev) => (
          <TimelineRow
            item={item}
            showDate={dayChanged(prev, item)}
            onOpen={() => onOpenThread(item.threadId)}
            resolve={resolve}
            showContactNames={showContactNames}
            showAvatars={showAvatars}
          />
        )}
      />
    </div>
  );
}

function TimelineRow({
  item,
  showDate,
  onOpen,
  resolve,
  showContactNames,
  showAvatars,
}: {
  item: TimelineMessage;
  showDate: boolean;
  onOpen: () => void;
  resolve: Resolver;
  showContactNames: boolean;
  showAvatars: boolean;
}) {
  const m = item.message;
  // The avatar ALWAYS shows the conversation partner (the other party), so a row
  // makes clear which chat it belongs to regardless of who sent it: for incoming
  // that's the actual sender; for your own outgoing messages it's the thread's
  // counterpart handle. A direction arrow (← received, → sent) marks who sent.
  const partnerHandle = m.isFromMe
    ? item.threadHandle
    : (m.sender ?? item.threadHandle);
  const resolved = partnerHandle ? resolve(partnerHandle) : null;
  const partnerName =
    (showContactNames && resolved?.name) || partnerHandle || item.threadTitle;
  const slug = item.service ? serviceSlug(item.service) : null;
  return (
    <div className="px-2 py-0.5">
      {showDate && m.sentAt && (
        <div className="px-4 py-1.5 text-center text-xs font-medium text-muted-foreground">
          {formatDateHeader(m.sentAt)}
        </div>
      )}
      {/* One flat row — avatar | ↔ | message | app icon | time. The avatar is the
          conversation partner; the arrow marks direction (your own messages are
          also tinted). The message wraps; the icon and time stay top-right. */}
      <button
        onClick={onOpen}
        data-slot="list-row"
        className={cn(
          "flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left transition-colors hover:bg-accent/50",
          m.isFromMe && "bg-primary/5 hover:bg-primary/10",
        )}
      >
        {showAvatars && (
          <Avatar className="size-8 shrink-0">
            {resolved?.hasImage && (
              <AvatarImage src={client.contactAvatarUrl(resolved.id)} alt="" />
            )}
            <AvatarFallback>{initials(partnerName)}</AvatarFallback>
          </Avatar>
        )}
        {/* Direction: → you sent it, ← you received it. */}
        <span
          className="shrink-0"
          title={m.isFromMe ? "You sent this" : "Received"}
        >
          {m.isFromMe ? (
            <ArrowRight className="size-3.5 text-primary" />
          ) : (
            <ArrowLeft className="size-3.5 text-muted-foreground" />
          )}
        </span>
        <div className="min-w-0 flex-1 whitespace-pre-wrap break-words text-sm text-foreground/90">
          {/* Without avatars, name the partner inline so the row still says who. */}
          {!showAvatars && (
            <span className="mr-1.5 font-medium text-foreground/70">
              {partnerName}
            </span>
          )}
          {m.body ? (
            m.body
          ) : m.attachments.length ? (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
              <Paperclip className="size-3.5" />
              {m.attachments[0].filename ?? "attachment"}
            </span>
          ) : (
            <span className="text-muted-foreground">—</span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
          {slug && hasBrandIcon(slug) && (
            <BrandIcon
              slug={slug}
              name={item.service ?? ""}
              className="size-3.5"
            />
          )}
          <span className="tabular-nums">{formatMessageTime(m.sentAt)}</span>
        </div>
      </button>
    </div>
  );
}

/** A day separator is shown when the calendar day changes between rows. */
function dayChanged(
  prev: TimelineMessage | undefined,
  cur: TimelineMessage,
): boolean {
  if (!prev) return true;
  const p = prev.message.sentAt;
  const c = cur.message.sentAt;
  if (!p || !c) return false;
  return (
    new Date(p * 1000).toDateString() !== new Date(c * 1000).toDateString()
  );
}

function ThreadRow({
  thread,
  resolve,
  showContactNames,
  showAvatars,
  active,
  onClick,
}: {
  thread: ThreadSummary;
  resolve: Resolver;
  showContactNames: boolean;
  showAvatars: boolean;
  active: boolean;
  onClick: () => void;
}) {
  const name = threadLabel(thread, resolve, showContactNames);
  const resolved = isGroup(thread) ? null : resolve(threadHandle(thread));
  return (
    <Item
      asChild
      size="sm"
      data-active={active}
      className="rounded-md transition-colors hover:bg-accent/50 data-[active=true]:bg-accent data-[active=true]:hover:bg-accent"
    >
      <button onClick={onClick} className="w-full text-left">
        {showAvatars && (
          <ItemMedia>
            {isGroup(thread) ? (
              <GroupAvatar thread={thread} resolve={resolve} />
            ) : (
              <Avatar>
                {resolved?.hasImage && (
                  <AvatarImage
                    src={client.contactAvatarUrl(resolved.id)}
                    alt=""
                  />
                )}
                <AvatarFallback>{initials(name)}</AvatarFallback>
              </Avatar>
            )}
          </ItemMedia>
        )}
        <ItemContent className="gap-0.5">
          <div className="flex items-baseline justify-between gap-2">
            <ItemTitle className="flex min-w-0 items-center gap-1.5">
              {hasBrandIcon(serviceSlug(thread.service)) && (
                <BrandIcon
                  slug={serviceSlug(thread.service)}
                  name={thread.service ?? ""}
                  className="size-3.5 shrink-0 self-center"
                />
              )}
              <span className="truncate">{name}</span>
            </ItemTitle>
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatListTime(thread.lastMessageAt)}
            </span>
          </div>
          <span className="truncate text-xs text-muted-foreground">
            {thread.snippet ?? "No messages"}
          </span>
        </ItemContent>
      </button>
    </Item>
  );
}

/** A group chat avatar: up to two members' photos stacked, else a group icon. */
function GroupAvatar({
  thread,
  resolve,
}: {
  thread: ThreadSummary;
  resolve: Resolver;
}) {
  const members = thread.participants.slice(0, 2).map((h) => resolve(h));
  return (
    <div className="relative size-8 shrink-0">
      {members.map((m, i) => (
        <Avatar
          key={i}
          className={cn(
            "absolute size-5 border border-background",
            i === 0 ? "left-0 top-0" : "bottom-0 right-0",
          )}
        >
          {m?.hasImage && (
            <AvatarImage src={client.contactAvatarUrl(m.id)} alt="" />
          )}
          <AvatarFallback className="text-[8px]">
            {m ? initials(m.name) : <Users className="size-2.5" />}
          </AvatarFallback>
        </Avatar>
      ))}
    </div>
  );
}

function Conversation({
  thread,
  resolve,
  showContactNames,
  kindValue,
  onKindChange,
  onBack,
  backLabel,
}: {
  thread: ThreadSummary;
  resolve: Resolver;
  showContactNames: boolean;
  kindValue: string;
  onKindChange: (v: string) => void;
  onBack?: () => void;
  backLabel?: string;
}) {
  const name = threadLabel(thread, resolve, showContactNames);
  const group = isGroup(thread);
  // Message order: oldest-first by default (chat-like, newest at the bottom).
  // Toggling to newest-first flips the query and pins the list to the top.
  const [order, setOrder] = useState<SortState>({ by: "time", desc: false });
  // Content kinds present in THIS conversation (drives the pills below).
  const { data: kindsData } = useQuery({
    queryKey: ["messageKinds", thread.id, null],
    queryFn: () => client.messageKinds(thread.id, null),
  });
  const available = (kindsData ?? []).map(([k]) => k);
  const kind = kindValue !== "all" && available.includes(kindValue) ? kindValue : null;
  // For a group, list the members under the header.
  const members = group
    ? thread.participants
        .map((h) => handleLabel(h, resolve, showContactNames))
        .join(", ")
    : null;

  // A thread can hold tens of thousands of messages; the count sizes the virtual
  // scroller and LazyVirtualList fetches only the windows it renders.
  const { data: total } = useQuery({
    queryKey: ["messageCount", thread.id, kind],
    queryFn: () => client.countThreadMessages(thread.id, kind),
  });

  const brandIcon = hasBrandIcon(serviceSlug(thread.service)) ? (
    <BrandIcon
      slug={serviceSlug(thread.service)}
      name={thread.service ?? ""}
      className="size-4 shrink-0"
    />
  ) : null;
  // A back button appears only when this conversation was jumped into from the
  // Timeline/Periods overview, returning the user to where they came from.
  const headerIcon =
    onBack || brandIcon ? (
      <div className="flex items-center gap-2">
        {onBack && (
          <Button
            variant="ghost"
            size="sm"
            className="-ml-2 h-7 gap-1 px-2 text-muted-foreground"
            onClick={onBack}
          >
            <ArrowLeft className="size-4" />
            {backLabel ?? "Back"}
          </Button>
        )}
        {brandIcon}
      </div>
    ) : undefined;

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={name} icon={headerIcon}>
        {group ? (
          <span
            className="max-w-[60%] truncate text-xs text-muted-foreground"
            title={members ?? ""}
          >
            {thread.participants.length} people · {members}
          </span>
        ) : (
          // App threads (e.g. TikTok) store the peer's @handle as the sole
          // participant — show it. The service name is intentionally omitted: it's
          // already in the conversation list and implied by the active filter.
          (() => {
            const handle = thread.participants.find((p) => p.startsWith("@"));
            return handle ? (
              <span className="text-xs text-muted-foreground">{handle}</span>
            ) : null;
          })()
        )}
        <MessageKindFilter
          available={available}
          value={kindValue}
          onChange={onKindChange}
        />
        <OrderToggle
          desc={order.desc}
          onToggle={() => setOrder({ by: "time", desc: !order.desc })}
        />
      </ViewHeader>
      <LazyVirtualList<Message>
        count={total ?? 0}
        startAtBottom={!order.desc}
        resetKey={`${thread.id}:${kind ?? "all"}:${order.desc}`}
        windowKey={(page) => ["messageWindow", thread.id, kind, order.desc, page]}
        fetchWindow={(offset, limit) =>
          client.getThreadMessageWindow(thread.id, offset, limit, order.desc, kind)
        }
        renderItem={(message, _i, prev) => {
          // In a group, label an incoming message with its sender — but only
          // when the sender changes, so runs of messages aren't repetitive.
          const newSender =
            !prev || prev.isFromMe || prev.sender !== message.sender;
          const senderLabel =
            group && !message.isFromMe && message.sender && newSender
              ? showContactNames
                ? (resolve(message.sender)?.name ?? message.sender)
                : message.sender
              : null;
          return (
            <div className="px-4 pb-1">
              <MessageBubble
                message={message}
                showTime={showTimeBetween(prev, message)}
                senderLabel={senderLabel}
              />
            </div>
          );
        }}
      />
    </div>
  );
}

/** Show a timestamp separator when >15 min elapsed since the previous message. */
function showTimeBetween(prev: Message | undefined, cur: Message): boolean {
  if (!prev) return true;
  return (cur.sentAt ?? 0) - (prev.sentAt ?? 0) > 15 * 60;
}

function MessageBubble({
  message,
  showTime,
  senderLabel,
}: {
  message: Message;
  showTime: boolean;
  senderLabel?: string | null;
}) {
  const align = message.isFromMe ? "end" : "start";
  return (
    <div>
      {showTime && message.sentAt && (
        <div className="py-2 text-center text-xs text-muted-foreground">
          {formatMessageTime(message.sentAt)}
        </div>
      )}
      <MessageRow align={align}>
        <MessageContent>
          {senderLabel && <MessageHeader>{senderLabel}</MessageHeader>}
          <Bubble variant={message.isFromMe ? "default" : "muted"}>
            <BubbleContent className="select-text">
              {message.replyToSnippet && (
                <p className="mb-1 border-l-2 border-current/30 pl-2 text-xs italic opacity-70">
                  {message.replyToSnippet}
                </p>
              )}
              {message.body && (
                <p className="whitespace-pre-wrap break-words">
                  {message.body}
                </p>
              )}
              {message.attachments.map((a) => (
                <div key={a.id} className={cn(message.body && "mt-1.5")}>
                  <AttachmentView att={a} />
                </div>
              ))}
            </BubbleContent>
          </Bubble>
          {message.reactions && (
            <span className="mt-0.5 w-fit rounded-full border bg-background px-1.5 py-0.5 text-xs leading-none shadow-sm">
              {message.reactions}
            </span>
          )}
          {message.isFromMe && (message.readAt || message.deliveredAt) && (
            <span className="mt-0.5 text-[10px] text-muted-foreground">
              {message.readAt
                ? `Read ${formatMessageTime(message.readAt)}`
                : "Delivered"}
            </span>
          )}
        </MessageContent>
      </MessageRow>
    </div>
  );
}

/** Open an attachment in the OS default app, toasting on failure. */
function openAttachmentFile(id: number) {
  client.openAttachment(id).catch((e) =>
    toast.error("Couldn't open attachment", {
      description: e instanceof Error ? e.message : String(e),
    }),
  );
}

/**
 * Renders one attachment by media type: images/audio/video play inline;
 * documents and everything else show an icon that opens the file on click.
 * Only materialized attachments (with extracted bytes) are interactive.
 */
/** An image by MIME, or by filename extension when the MIME is missing (sms.db
 *  often stores a NULL mime for image attachments — the backend transcodes them
 *  to JPEG regardless, so we should still render them inline). */
function isImageAttachment(mime: string, filename: string | null): boolean {
  if (mime.startsWith("image/")) return true;
  const f = (filename ?? "").toLowerCase();
  return /\.(jpe?g|png|gif|heic|heif|webp|tiff?|bmp)$/.test(f);
}

function AttachmentView({ att }: { att: Attachment }) {
  const mime = att.mimeType ?? "";
  const available = !!att.localPath;

  if (available && isImageAttachment(mime, att.filename)) {
    return (
      <button
        onClick={() => openAttachmentFile(att.id)}
        className="block max-w-[240px] overflow-hidden rounded-lg"
        title={att.filename ?? "image"}
      >
        <img
          src={client.attachmentUrl(att.id, { thumb: true })}
          alt={att.filename ?? ""}
          loading="lazy"
          className="max-h-64 w-full object-cover"
        />
      </button>
    );
  }
  if (available && mime.startsWith("video/")) {
    return (
      <video
        controls
        preload="metadata"
        src={client.attachmentUrl(att.id)}
        className="max-h-64 max-w-[240px] rounded-lg"
      />
    );
  }
  if (available && mime.startsWith("audio/")) {
    return (
      <audio
        controls
        preload="none"
        src={client.attachmentUrl(att.id)}
        className="max-w-[240px]"
      />
    );
  }

  // Documents and unknowns: an icon + filename that opens the file on click.
  const Icon = isImageAttachment(mime, att.filename)
    ? ImageIcon
    : mime
      ? FileText
      : Paperclip;
  return (
    <button
      onClick={() => available && openAttachmentFile(att.id)}
      disabled={!available}
      className={cn(
        "flex items-center gap-1.5 rounded-md text-xs",
        available ? "underline-offset-2 hover:underline" : "opacity-60",
      )}
      title={available ? "Open attachment" : "File not available"}
    >
      <Icon className="size-4 shrink-0" />
      <span className="truncate">{att.filename ?? "attachment"}</span>
    </button>
  );
}
