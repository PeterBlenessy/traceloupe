import { useEffect, useMemo, useRef, useState } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  ArrowDownToLine,
  ArrowDownWideNarrow,
  ArrowLeft,
  ArrowRight,
  ArrowUpNarrowWide,
  ArrowUpToLine,
  Copy,
  ExternalLink,
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
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { BadgeFilter } from "@/components/badge-filter";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { MediaLightbox } from "@/components/media-lightbox";
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
  formatTimelineTime,
  formatDateTime,
  formatListTime,
  formatMessageTime,
} from "@/lib/format";
import { usePersistedState } from "@/lib/use-persisted-state";
import { MediaCacheKeyBoundary, useMediaCacheKey } from "@/lib/use-media-cache-key";
import {
  TimeFilterBar,
  makeYearPresets,
  useTimePresets,
} from "@/components/time-filter";
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
  type LinkPreview,
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

/** Imperative top/bottom scrolling for a LazyVirtualList: the `scrollEnd` value
 *  to pass to it, plus handlers that bump its token. */
function useScrollEnds() {
  const [scrollEnd, setScrollEnd] =
    useState<{ dir: "top" | "bottom"; token: number }>();
  const token = useRef(0);
  return {
    scrollEnd,
    toTop: () => setScrollEnd({ dir: "top", token: (token.current += 1) }),
    toBottom: () => setScrollEnd({ dir: "bottom", token: (token.current += 1) }),
  };
}

/** Jump-to-top / jump-to-bottom icon pair (grouped tight). */
function JumpButtons({
  onTop,
  onBottom,
  disabled,
}: {
  onTop: () => void;
  onBottom: () => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex shrink-0 items-center">
      <Button
        variant="ghost"
        size="sm"
        className="size-7 px-0 text-muted-foreground"
        title="Jump to top"
        disabled={disabled}
        onClick={onTop}
      >
        <ArrowUpToLine className="size-4" />
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="-ml-1 size-7 px-0 text-muted-foreground"
        title="Jump to bottom"
        disabled={disabled}
        onClick={onBottom}
      >
        <ArrowDownToLine className="size-4" />
      </Button>
    </div>
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
  // One media cache key per mount of this view, shared by every image below —
  // so a view-switch remount busts WebKit's cached-failed scheme tasks while
  // scrolling reuses URLs. See use-media-cache-key.
  return (
    <MediaCacheKeyBoundary>
      <MessagesViewInner />
    </MediaCacheKeyBoundary>
  );
}

function MessagesViewInner() {
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
  // clicking a row in the Timeline view can jump to its conversation. Persisted
  // so leaving Messages (e.g. to Photos) and returning keeps the same
  // conversation selected instead of snapping back to the top one.
  const [selectedId, setSelectedId] = usePersistedState<number | null>(
    "messages:selected",
    null,
  );
  // Where a jump into a conversation came from, so the conversation view can
  // offer a "back" button to return to that overview (null = opened normally
  // from the conversation list, so no back button).
  const [openedFrom, setOpenedFrom] = useState<Mode | null>(null);
  // A message to scroll to after opening a conversation (e.g. the Timeline row
  // that was clicked). Cleared once the conversation has scrolled to it.
  const [scrollToMessage, setScrollToMessage] = useState<number | null>(null);
  const openThread = (
    threadId: number,
    from: Mode | null = null,
    messageId: number | null = null,
  ) => {
    setOpenedFrom(from);
    setSelectedId(threadId);
    setScrollToMessage(messageId);
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
            scrollToMessage={scrollToMessage}
            onScrolledToMessage={() => setScrollToMessage(null)}
          />
        ) : (
          <Timeline
            onOpenThread={(id, messageId) =>
              openThread(id, "timeline", messageId ?? null)
            }
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
  scrollToMessage,
  onScrolledToMessage,
}: {
  selectedId: number | null;
  onSelect: (id: number) => void;
  service: string | null;
  kindValue: string;
  onKindChange: (v: string) => void;
  onBack?: () => void;
  backLabel?: string;
  /** A message id to scroll the open conversation to (from a Timeline jump). */
  scrollToMessage?: number | null;
  onScrolledToMessage?: () => void;
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
            scrollToMessage={
              selected.id === selectedId ? scrollToMessage : null
            }
            onScrolledToMessage={onScrolledToMessage}
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
  onOpenThread: (threadId: number, messageId?: number) => void;
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
  const { now, presets: basePresets } = useTimePresets();
  // The message date range → per-year quick-filter chips (so you can jump to a
  // specific year without fiddling the custom range). Replaces the cumulative
  // "this year" chip with disjoint year chips when the backup spans years.
  const { data: dateBounds } = useQuery({
    queryKey: ["messageDateBounds"],
    queryFn: () => client.messageDateBounds(),
    enabled: active === true,
  });
  const presets = useMemo(() => {
    if (!dateBounds) return basePresets;
    const minYear = new Date(dateBounds[0] * 1000).getFullYear();
    const maxYear = new Date(now * 1000).getFullYear();
    return [
      ...basePresets.filter((p) => p.key !== "year"),
      ...makeYearPresets(minYear, maxYear),
    ];
  }, [basePresets, dateBounds, now]);
  // Oldest-first by default (newest at the bottom); toggle flips to newest-first.
  // Persisted so it survives leaving Messages and returning.
  const [order, setOrder] = usePersistedState<SortState>("messages:timeline-order", {
    by: "time",
    desc: false,
  });
  // The active time filter as a half-open [lo, hi) range; {null,null} = all time.
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });
  // Free-text search over message body / sender / conversation (debounced).
  const [q, setQ] = useState("");
  const search = useDebounced(q.trim()) || null;
  // Image lightbox opened by tapping a thumbnail in a row (kept here so it
  // survives row virtualization). Holds the tapped message's image set.
  const [lb, setLb] = useState<{
    images: Attachment[];
    index: number;
    sentAt: number | null;
    from: string | null;
  } | null>(null);

  // Per-preset message counts for the chip labels (e.g. "7d · 812").
  const { data: presetCounts } = useQuery({
    queryKey: ["messageRanges", now, service, search, kind, presets.length],
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
  const { scrollEnd, toTop, toBottom } = useScrollEnds();

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
        <div className="flex shrink-0 items-center">
          <OrderToggle
            desc={order.desc}
            onToggle={() => setOrder({ by: "time", desc: !order.desc })}
          />
          <JumpButtons onTop={toTop} onBottom={toBottom} disabled={!total} />
        </div>
      </div>
      <LazyVirtualList<TimelineMessage>
        count={total ?? 0}
        startAtBottom={!order.desc}
        resetKey={`timeline:${service ?? "all"}:${kind ?? "all"}:${range.lo}:${range.hi}:${search}:${order.desc}`}
        scrollEnd={scrollEnd}
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
            onOpen={() => onOpenThread(item.threadId, item.message.id)}
            onOpenImage={(images, index, sentAt, from) =>
              setLb({ images, index, sentAt, from })
            }
            resolve={resolve}
            showContactNames={showContactNames}
            showAvatars={showAvatars}
          />
        )}
      />
      <MessageImageLightbox
        images={lb?.images ?? []}
        index={lb?.index ?? null}
        onClose={() => setLb(null)}
        onIndex={(i) => setLb((v) => (v ? { ...v, index: i } : v))}
        sentAt={lb?.sentAt ?? null}
        from={lb?.from ?? null}
      />
    </div>
  );
}

function TimelineRow({
  item,
  showDate,
  onOpen,
  onOpenImage,
  resolve,
  showContactNames,
  showAvatars,
}: {
  item: TimelineMessage;
  showDate: boolean;
  onOpen: () => void;
  /** Open one of this row's images in a lightbox (with metadata context). */
  onOpenImage: (
    images: Attachment[],
    index: number,
    sentAt: number | null,
    from: string | null,
  ) => void;
  resolve: Resolver;
  showContactNames: boolean;
  showAvatars: boolean;
}) {
  const m = item.message;
  // Hide iMessage plugin/app payloads (see isInternalAttachment) from the row's
  // attachment fallback and thumbnails.
  const atts = m.attachments.filter((a) => !isInternalAttachment(a));
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
  // Same link-preview behaviour as the conversation bubble: "inline" mode
  // unfurls links into cards and strips a previewed link from the row text (a
  // URL-only row collapses to the card); "hover" mode is handled in MessageBody.
  const { urls: previewUrls, rich: richUrls } = useInlinePreviews(m.body);
  const trimmedBody = (m.body ?? "").trim();
  const replaceUrlWithCard =
    previewUrls.length === 1 && !/\s/.test(trimmedBody);
  const omitUrls = replaceUrlWithCard ? [] : richUrls;
  const showBodyText =
    !!m.body && !replaceUrlWithCard && hasNonUrlText(m.body, new Set(omitUrls));
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
      {/* A div (not a button) so the image thumbnails below can be real nested
          buttons — a button inside a button is invalid HTML. role/tabIndex/key
          handling keep it keyboard-accessible like the original button row. */}
      <div
        role="button"
        tabIndex={0}
        onClick={onOpen}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onOpen();
          }
        }}
        data-slot="list-row"
        className={cn(
          "flex w-full cursor-pointer gap-2.5 rounded-md px-3 py-2 text-left transition-colors hover:bg-accent/50",
          // Top-align when a preview card makes the row tall; otherwise center.
          previewUrls.length ? "items-start" : "items-center",
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
            showBodyText && <MessageBody text={m.body} omit={omitUrls} />
          ) : atts.some(
              (a) => a.localPath && isImageAttachment(a.mimeType ?? "", a.filename),
            ) ? null : atts.length ? (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
              <Paperclip className="size-3.5" />
              {atts[0].filename ?? "attachment"}
            </span>
          ) : (
            <span className="text-muted-foreground">—</span>
          )}
          <TimelineThumbs
            attachments={atts}
            onOpenImage={(images, index) =>
              onOpenImage(
                images,
                index,
                m.sentAt,
                m.isFromMe ? "You" : partnerName,
              )
            }
          />
          {previewUrls.map((u) => (
            <LinkPreviewCard key={u} url={u} placeholder={replaceUrlWithCard} />
          ))}
        </div>
        <div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
          {slug && hasBrandIcon(slug) && (
            <BrandIcon
              slug={slug}
              name={item.service ?? ""}
              className="size-3.5"
            />
          )}
          <span className="tabular-nums">{formatTimelineTime(m.sentAt)}</span>
        </div>
      </div>
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
  scrollToMessage,
  onScrolledToMessage,
}: {
  thread: ThreadSummary;
  resolve: Resolver;
  showContactNames: boolean;
  kindValue: string;
  onKindChange: (v: string) => void;
  onBack?: () => void;
  backLabel?: string;
  scrollToMessage?: number | null;
  onScrolledToMessage?: () => void;
}) {
  const name = threadLabel(thread, resolve, showContactNames);
  const group = isGroup(thread);
  // Message order: oldest-first by default (chat-like, newest at the bottom).
  // Toggling to newest-first flips the query and pins the list to the top.
  // Persisted so it survives leaving Messages and returning.
  const [order, setOrder] = usePersistedState<SortState>("messages:conversation-order", {
    by: "time",
    desc: false,
  });
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

  // Scroll-to-message (from a Timeline jump): resolve the target's row index in
  // the current order/filter, then hand the virtual list a one-shot jump token.
  const { data: jumpIndex } = useQuery({
    queryKey: ["messageIndex", thread.id, scrollToMessage, kind, order.desc],
    queryFn: () =>
      client.threadMessageIndex(thread.id, scrollToMessage!, kind, order.desc),
    enabled: scrollToMessage != null,
  });
  const [jumpTo, setJumpTo] = useState<{ index: number; token: number } | undefined>();
  const jumpToken = useRef(0);
  const { scrollEnd, toTop, toBottom } = useScrollEnds();
  useEffect(() => {
    if (scrollToMessage == null || jumpIndex === undefined) return;
    if (jumpIndex != null && jumpIndex >= 0) {
      jumpToken.current += 1;
      setJumpTo({ index: jumpIndex, token: jumpToken.current });
    }
    onScrolledToMessage?.(); // consume the request (found or not) so it fires once
  }, [scrollToMessage, jumpIndex, onScrolledToMessage]);

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
        <JumpButtons onTop={toTop} onBottom={toBottom} disabled={!total} />
      </ViewHeader>
      <LazyVirtualList<Message>
        count={total ?? 0}
        startAtBottom={!order.desc}
        resetKey={`${thread.id}:${kind ?? "all"}:${order.desc}`}
        jumpTo={jumpTo}
        scrollEnd={scrollEnd}
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
  // In "inline" mode every link unfurls into a card (capped). A link whose
  // preview loads is dropped from the text; a message that is *only* one link
  // collapses to the card entirely (iMessage-style).
  const { urls: previewUrls, rich: richUrls } = useInlinePreviews(message.body);
  const trimmedBody = (message.body ?? "").trim();
  const replaceUrlWithCard =
    previewUrls.length === 1 && !/\s/.test(trimmedBody);
  const omitUrls = replaceUrlWithCard ? [] : richUrls;
  const showBodyText =
    !!message.body &&
    !replaceUrlWithCard &&
    hasNonUrlText(message.body, new Set(omitUrls));
  // Drop iMessage app/plugin payloads (`.pluginPayloadAttachment`) — binary
  // typedstream blobs behind rich links/app messages, not openable user files.
  const attachments = useMemo(
    () => message.attachments.filter((a) => !isInternalAttachment(a)),
    [message.attachments],
  );
  // Available image attachments open in an in-app lightbox (with prev/next).
  const imageAtts = useMemo(
    () =>
      attachments.filter(
        (a) => a.localPath && isImageAttachment(a.mimeType ?? "", a.filename),
      ),
    [attachments],
  );
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
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
              {showBodyText && (
                <p className="whitespace-pre-wrap break-words">
                  <MessageBody text={message.body!} omit={omitUrls} />
                </p>
              )}
              {attachments.map((a) => {
                const imgIdx = imageAtts.indexOf(a);
                return (
                  <div key={a.id} className={cn(message.body && "mt-1.5")}>
                    <AttachmentView
                      att={a}
                      onOpenImage={
                        imgIdx >= 0 ? () => setLightboxIndex(imgIdx) : undefined
                      }
                    />
                  </div>
                );
              })}
              {previewUrls.map((u) => (
                <LinkPreviewCard
                  key={u}
                  url={u}
                  placeholder={replaceUrlWithCard}
                />
              ))}
              <MessageImageLightbox
                images={imageAtts}
                index={lightboxIndex}
                onClose={() => setLightboxIndex(null)}
                onIndex={setLightboxIndex}
                sentAt={message.sentAt}
                from={message.isFromMe ? "You" : (senderLabel ?? message.sender ?? null)}
              />
              {message.edited && (
                <span className="mt-0.5 block text-[10px] italic opacity-60">
                  Edited
                </span>
              )}
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

/** iMessage app/plugin payloads (`.pluginPayloadAttachment`) are binary plists
 *  behind rich links, stickers and app messages — internal data, not user files.
 *  They can't be opened, so they're hidden rather than shown as dead rows. */
function isInternalAttachment(att: Attachment): boolean {
  return (att.filename ?? "")
    .toLowerCase()
    .endsWith(".pluginpayloadattachment");
}

/** Message text with URLs rendered as clickable links that open in the default
 *  browser (user-initiated; the app never loads remote content itself). */
const URL_RE = /(https?:\/\/[^\s<>()]+|www\.[^\s<>()]+)/gi;

/** Normalize a URL token the same way `allUrls` does, for matching against the
 *  omit set. */
function normalizeUrlToken(part: string): string {
  const raw = part.replace(/[.,!?;:]+$/, "");
  return /^www\./i.test(raw) ? `https://${raw}` : raw;
}

/** Whether `text` has any content besides the URLs in `omit` — used to decide
 *  whether a bubble/row still needs its text line once previewed links are
 *  stripped. */
function hasNonUrlText(text: string, omit: Set<string>): boolean {
  return text.split(URL_RE).some((part) => {
    if (!part) return false;
    if (/^(https?:\/\/|www\.)/i.test(part)) return !omit.has(normalizeUrlToken(part));
    return part.trim().length > 0;
  });
}

function MessageBody({ text, omit }: { text: string; omit?: string[] }) {
  // In "hover" mode each link gets an on-demand preview card; "off"/"inline"
  // leave the link plain (inline mode shows a card in the bubble instead).
  const hover = useSettings().linkPreviewMode === "hover";
  // Links whose preview card is shown are dropped from the text (iMessage-style).
  const omitSet = omit && omit.length ? new Set(omit) : null;
  return text.split(URL_RE).map((part, i) => {
    if (!part) return null;
    if (/^(https?:\/\/|www\.)/i.test(part)) {
      // Trailing sentence punctuation isn't part of the URL — keep it as text.
      const m = part.match(/^(.*?)([.,!?;:]+)$/);
      const urlText = m ? m[1] : part;
      const trailing = m ? m[2] : "";
      const href = /^www\./i.test(urlText) ? `https://${urlText}` : urlText;
      // Dropped because it's shown as a card — keep only trailing punctuation.
      if (omitSet?.has(href)) {
        return trailing ? <span key={i}>{trailing}</span> : null;
      }
      const link = (
        <a
          href={href}
          onClick={(e) => {
            e.stopPropagation();
            e.preventDefault();
            client.openExternal(href);
          }}
          // Inherit the bubble's text colour (never `text-primary`): the
          // outgoing bubble is `bg-primary`, so a primary-coloured link there is
          // invisible. `underline` keeps it recognizable as a link. No `title`
          // (the hover card already shows the URL — a native tooltip would
          // double up).
          className="cursor-pointer break-all font-medium underline underline-offset-2"
        >
          {urlText}
        </a>
      );
      return (
        <span key={i}>
          {hover ? (
            <LinkHoverPreview href={href}>{link}</LinkHoverPreview>
          ) : (
            link
          )}
          {trailing}
        </span>
      );
    }
    return <span key={i}>{part}</span>;
  });
}

/** Wraps a link so hovering it reveals an OpenGraph preview card. The preview is
 *  fetched lazily — the hover-card content only mounts (and so only fires the
 *  query) once the pointer rests on the link. */
function LinkHoverPreview({
  href,
  children,
}: {
  href: string;
  children: React.ReactNode;
}) {
  return (
    <HoverCard openDelay={300} closeDelay={120}>
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent
        side="top"
        className="w-72 overflow-hidden p-0"
        // Keep clicks inside the card from bubbling to the row/bubble.
        onClick={(e) => e.stopPropagation()}
      >
        <LinkPreviewContent url={href} />
      </HoverCardContent>
    </HoverCard>
  );
}

/** Compact inline thumbnails of a message's available image attachments, for the
 *  Timeline (clicking the row navigates to the conversation). */
function TimelineThumbs({
  attachments,
  onOpenImage,
}: {
  attachments: Attachment[];
  /** Open image `index` (into the filtered image list) in a lightbox. */
  onOpenImage?: (images: Attachment[], index: number) => void;
}) {
  const cacheKey = useMediaCacheKey();
  const imgs = attachments.filter(
    (a) => a.localPath && isImageAttachment(a.mimeType ?? "", a.filename),
  );
  if (!imgs.length) return null;
  return (
    <span className="ml-2 inline-flex shrink-0 gap-1 align-middle">
      {imgs.slice(0, 3).map((a, i) => (
        <button
          key={a.id}
          type="button"
          // Stop the row's click (which opens the conversation) so tapping a
          // thumbnail opens the image in the lightbox instead.
          onClick={(e) => {
            e.stopPropagation();
            onOpenImage?.(imgs, i);
          }}
          className="block overflow-hidden rounded"
          title={a.filename ?? "image"}
        >
          <img
            src={client.attachmentUrl(a.id, { thumb: true, cacheKey })}
            alt=""
            className="size-9 rounded object-cover"
            onError={(e) => {
              e.currentTarget.style.display = "none";
            }}
          />
        </button>
      ))}
    </span>
  );
}

/** Up to this many preview cards per message, so a link-heavy message (e.g. a
 *  shopping list) doesn't unfurl into a wall of cards. */
const MAX_PREVIEW_CARDS = 4;

/** Every distinct URL in `text` (normalized to https, trailing punctuation
 *  trimmed, de-duplicated in order) — so a message with several links previews
 *  each of them, not just the first. */
function allUrls(text: string): string[] {
  const re = /(https?:\/\/[^\s<>()]+|www\.[^\s<>()]+)/gi;
  const out: string[] = [];
  for (const m of text.matchAll(re)) {
    const raw = m[0].replace(/[.,!?;:]+$/, "");
    out.push(/^www\./i.test(raw) ? `https://${raw}` : raw);
  }
  return [...new Set(out)];
}

/** Inline link previews for a message body: the links to unfurl (`urls`, capped)
 *  and the subset that resolved to a real preview (`rich`) — used both to render
 *  cards and to strip previewed links from the shown text. Empty unless the
 *  user picked "inline" mode. The queries share the card's cache by URL, so a
 *  link is fetched once. */
function useInlinePreviews(body: string | null): { urls: string[]; rich: string[] } {
  const inline = useSettings().linkPreviewMode === "inline";
  const urls = useMemo(
    () => (inline && body ? allUrls(body).slice(0, MAX_PREVIEW_CARDS) : []),
    [inline, body],
  );
  const results = useQueries({
    queries: urls.map((u) => ({
      queryKey: ["linkPreview", u],
      queryFn: () => client.fetchLinkPreview(u),
      staleTime: Infinity,
      retry: false,
    })),
  });
  const rich = urls.filter((_, i) => {
    const d = results[i]?.data;
    return !!d && (!!d.title || !!d.image);
  });
  return { urls, rich };
}

/** The host of a URL without a leading `www.`, for a compact domain label. */
function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** An inline OpenGraph card shown in the bubble for a link (image on top, then
 *  site/title/description, then a footer with the domain and a copy button) —
 *  the whole card opens the link. Shares the hover card's query cache by URL.
 *
 *  `placeholder`: when the message is only this link, the card replaces the raw
 *  URL, so it always renders (falling back to the domain) rather than returning
 *  null while loading or when the site offers no preview. */
function LinkPreviewCard({
  url,
  placeholder = false,
}: {
  url: string;
  placeholder?: boolean;
}) {
  const { data } = useQuery<LinkPreview>({
    queryKey: ["linkPreview", url],
    queryFn: () => client.fetchLinkPreview(url),
    staleTime: Infinity,
    retry: false,
  });
  const rich = !!data && (!!data.title || !!data.image);
  // For an inline card that sits beside message text, stay quiet until there's
  // something worth showing; as a URL replacement, always render.
  if (!rich && !placeholder) return null;

  const host = hostOf(url);
  const copy = (e: React.MouseEvent) => {
    e.stopPropagation();
    navigator.clipboard
      ?.writeText(url)
      .then(() => toast.success("Link copied"))
      .catch(() => {});
  };
  return (
    <div className="mt-1.5 w-full max-w-[280px] overflow-hidden rounded-xl border">
      <button
        onClick={(e) => {
          e.stopPropagation();
          client.openExternal(url);
        }}
        className="block w-full text-left transition-colors hover:bg-accent/50"
        title={url}
      >
        {data?.image && (
          <img
            src={data.image}
            alt=""
            className="h-32 w-full bg-muted object-cover"
            onError={(e) => {
              e.currentTarget.style.display = "none";
            }}
          />
        )}
        <div className="min-w-0 px-2.5 py-2">
          <div className="truncate text-[10px] uppercase tracking-wide text-muted-foreground">
            {data?.siteName ?? host}
          </div>
          <div className="line-clamp-2 text-xs font-medium leading-snug">
            {data?.title ?? host}
          </div>
          {data?.description && (
            <div className="mt-0.5 line-clamp-2 text-[11px] leading-snug text-muted-foreground">
              {data.description}
            </div>
          )}
        </div>
      </button>
      <div className="flex items-center gap-2 border-t px-2.5 py-1">
        <span className="min-w-0 flex-1 truncate text-[10px] text-muted-foreground/70">
          {host}
        </span>
        <button
          onClick={copy}
          className="-mr-1 inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground"
          title="Copy link"
        >
          <Copy className="size-3" />
          Copy
        </button>
      </div>
    </div>
  );
}

/** The OpenGraph preview shown inside a link's hover card. Fetches title/image/
 *  description from the linked site (cached forever by URL); clicking opens it
 *  externally. Renders a loading line first, and falls back to the bare URL when
 *  the site offers no preview or the fetch fails. */
function LinkPreviewContent({ url }: { url: string }) {
  const { data, isLoading, isError } = useQuery<LinkPreview>({
    queryKey: ["linkPreview", url],
    queryFn: () => client.fetchLinkPreview(url),
    staleTime: Infinity,
    retry: false,
  });
  if (isLoading) {
    return (
      <div className="p-3 text-xs text-muted-foreground">Loading preview…</div>
    );
  }
  const empty = !data || (!data.title && !data.image && !data.description);
  if (isError || empty) {
    return (
      <div className="break-all p-3 text-xs text-muted-foreground">{url}</div>
    );
  }
  return (
    <button
      onClick={(e) => {
        e.stopPropagation();
        client.openExternal(url);
      }}
      className="flex w-full flex-col text-left transition-colors hover:bg-accent/50"
      title={url}
    >
      {data.image && (
        <img
          src={data.image}
          alt=""
          className="h-32 w-full bg-muted object-cover"
          onError={(e) => {
            e.currentTarget.style.display = "none";
          }}
        />
      )}
      <div className="min-w-0 p-2.5">
        {data.siteName && (
          <div className="truncate text-[10px] uppercase tracking-wide text-muted-foreground">
            {data.siteName}
          </div>
        )}
        {data.title && (
          <div className="line-clamp-2 text-sm font-medium leading-snug">
            {data.title}
          </div>
        )}
        {data.description && (
          <div className="mt-0.5 line-clamp-3 text-xs text-muted-foreground">
            {data.description}
          </div>
        )}
        <div className="mt-1 truncate text-[10px] text-muted-foreground/70">
          {url}
        </div>
      </div>
    </button>
  );
}

/** In-app viewer for a message's image attachments, via the shared MediaLightbox
 *  (windowed/fullscreen per settings), with prev/next among the same message's
 *  images, all available metadata, and an "open externally" escape hatch. */
function MessageImageLightbox({
  images,
  index,
  onClose,
  onIndex,
  sentAt,
  from,
}: {
  images: Attachment[];
  index: number | null;
  onClose: () => void;
  onIndex: (i: number) => void;
  sentAt: number | null;
  from: string | null;
}) {
  const { lightboxStyle, showMediaMetadata } = useSettings();
  const cacheKey = useMediaCacheKey();
  const att = index != null ? images[index] : null;
  const meta =
    att && showMediaMetadata ? (
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-3">
          <span className="select-text truncate">{att.filename ?? "image"}</span>
          {att.mimeType && <span className="text-neutral-400">{att.mimeType}</span>}
          {from && <span className="truncate text-neutral-400">{from}</span>}
          {sentAt != null && (
            <span className="shrink-0 text-neutral-400">{formatDateTime(sentAt)}</span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-3">
          {images.length > 1 && index != null && (
            <span className="tabular-nums">
              {index + 1} / {images.length}
            </span>
          )}
          <button
            onClick={() => openAttachmentFile(att.id)}
            className="inline-flex items-center gap-1 hover:text-white"
            title="Open in the default app"
          >
            <ExternalLink className="size-3.5" />
            Open externally
          </button>
        </div>
      </div>
    ) : undefined;

  return (
    <MediaLightbox
      open={index != null}
      onClose={onClose}
      style={lightboxStyle}
      title={att?.filename ?? "Image"}
      hasPrev={index != null && index > 0}
      hasNext={index != null && index < images.length - 1}
      onPrev={() => index != null && index > 0 && onIndex(index - 1)}
      onNext={() =>
        index != null && index < images.length - 1 && onIndex(index + 1)
      }
      media={
        att ? (
          <img
            key={att.id}
            src={client.attachmentUrl(att.id, { cacheKey })}
            alt={att.filename ?? ""}
            className="max-h-full max-w-full object-contain"
          />
        ) : null
      }
      meta={meta}
    />
  );
}

function AttachmentView({
  att,
  onOpenImage,
}: {
  att: Attachment;
  /** Open an available image in the in-app lightbox instead of externally. */
  onOpenImage?: () => void;
}) {
  const mime = att.mimeType ?? "";
  const available = !!att.localPath;
  const cacheKey = useMediaCacheKey();

  if (available && isImageAttachment(mime, att.filename)) {
    return (
      <button
        onClick={() => (onOpenImage ? onOpenImage() : openAttachmentFile(att.id))}
        className="block max-w-[240px] overflow-hidden rounded-lg"
        title={att.filename ?? "image"}
      >
        <img
          src={client.attachmentUrl(att.id, { thumb: true, cacheKey })}
          alt={att.filename ?? ""}
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
        // The `#t=0.1` media fragment makes WebKit seek to and paint the first
        // frame, so the player shows a still instead of a black rectangle before
        // playback (no server-side frame extraction needed).
        src={`${client.attachmentUrl(att.id, { cacheKey })}#t=0.1`}
        className="max-h-64 max-w-[240px] rounded-lg"
      />
    );
  }
  if (available && mime.startsWith("audio/")) {
    return (
      <audio
        controls
        preload="none"
        src={client.attachmentUrl(att.id, { cacheKey })}
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
      title={available ? "Open attachment" : "This attachment isn't in the backup"}
    >
      <Icon className="size-4 shrink-0" />
      <span className="truncate">{att.filename ?? "attachment"}</span>
      {!available && (
        <span className="shrink-0 text-[10px] italic opacity-70">
          · not in backup
        </span>
      )}
    </button>
  );
}
