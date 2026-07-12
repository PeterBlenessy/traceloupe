import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { FileText, ImageIcon, MessageSquare, Paperclip, Users } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Message as MessageRow, MessageContent, MessageHeader } from "@/components/ui/message";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { EmptyView, ListDetail, ViewHeader } from "@/components/view";
import { LazyVirtualList } from "@/components/lazy-virtual-list";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import { cn } from "@/lib/utils";
import { formatDateHeader, formatListTime, formatMessageTime } from "@/lib/format";
import { initials } from "@/lib/contact";
import { useContactResolver, type ResolvedContact } from "@/lib/use-contact-resolver";
import {
  client,
  type Attachment,
  type Message,
  type ThreadSummary,
  type TimelineMessage,
} from "@/lib/ipc";

type Mode = "conversations" | "timeline" | "periods";

export function MessagesView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const [mode, setMode] = useState<Mode>("conversations");
  // Which conversation is open in the master-detail view. Lifted here so that
  // clicking a row in the Timeline or Periods view can jump to its conversation.
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const openThread = (threadId: number) => {
    setSelectedId(threadId);
    setMode("conversations");
  };

  // Deep link from elsewhere (e.g. a contact's "Conversations"): ?thread=<id>.
  const search = useSearch({ strict: false }) as { thread?: number };
  useEffect(() => {
    if (search.thread != null) openThread(search.thread);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search.thread]);

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
      <div className="flex shrink-0 items-center gap-2 border-b px-3 py-2">
        <ToggleGroup
          type="single"
          value={mode}
          onValueChange={(v) => v && setMode(v as Mode)}
          variant="outline"
          size="sm"
        >
          <ToggleGroupItem value="conversations">Conversations</ToggleGroupItem>
          <ToggleGroupItem value="timeline">Timeline</ToggleGroupItem>
          <ToggleGroupItem value="periods">Periods</ToggleGroupItem>
        </ToggleGroup>
      </div>
      <div className="min-h-0 flex-1">
        {mode === "conversations" ? (
          <Conversations selectedId={selectedId} onSelect={setSelectedId} />
        ) : mode === "timeline" ? (
          <Timeline onOpenThread={openThread} />
        ) : (
          <Periods onOpenThread={openThread} />
        )}
      </div>
    </div>
  );
}

/** Master-detail view: the thread list on the left, one conversation on the right. */
function Conversations({
  selectedId,
  onSelect,
}: {
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  const { data: threads, isPending } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
  });
  const resolve = useContactResolver();
  const { showContactNames, showAvatars } = useSettings();

  // Filter by source app (iMessage / SMS / TikTok / …). Only meaningful when a
  // backup holds more than one; app DMs (e.g. TikTok) carry their own service.
  const [serviceFilter, setServiceFilter] = useState<string>("all");
  const services = useMemo(() => {
    const set = new Set<string>();
    for (const t of threads ?? []) if (t.service) set.add(t.service);
    return [...set].sort();
  }, [threads]);
  const visibleThreads = useMemo(
    () =>
      serviceFilter === "all"
        ? threads
        : threads?.filter((t) => t.service === serviceFilter),
    [threads, serviceFilter],
  );

  const selected =
    visibleThreads?.find((t) => t.id === selectedId) ?? visibleThreads?.[0] ?? null;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Conversations" count={visibleThreads?.length} />
          {services.length > 1 && (
            <div className="border-b px-2 pb-2">
              <ToggleGroup
                type="single"
                size="sm"
                variant="outline"
                value={serviceFilter}
                onValueChange={(v) => v && setServiceFilter(v)}
                className="flex-wrap justify-start"
              >
                <ToggleGroupItem value="all">All</ToggleGroupItem>
                {services.map((s) => (
                  <ToggleGroupItem key={s} value={s}>
                    {s}
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
            </div>
          )}
          {isPending ? (
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
                <ThreadRow
                  thread={t}
                  resolve={resolve}
                  showContactNames={showContactNames}
                  showAvatars={showAvatars}
                  active={selected?.id === t.id}
                  onClick={() => onSelect(t.id)}
                />
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
function handleLabel(handle: string, resolve: Resolver, showContactNames: boolean): string {
  if (!showContactNames) return handle;
  return resolve(handle)?.name ?? handle;
}

/**
 * How a thread is labelled. Group chats show their name if set, otherwise the
 * members' names joined; 1:1 chats show the contact name or the raw handle.
 */
function threadLabel(thread: ThreadSummary, resolve: Resolver, showContactNames: boolean): string {
  if (isGroup(thread)) {
    if (thread.displayName?.trim()) return thread.displayName;
    return thread.participants.map((h) => handleLabel(h, resolve, showContactNames)).join(", ");
  }
  const handle = threadHandle(thread);
  if (!showContactNames) return handle;
  return resolve(handle)?.name ?? handle;
}

/** Every message from every conversation, in one chronological stream. */
function Timeline({ onOpenThread }: { onOpenThread: (threadId: number) => void }) {
  const { data: total } = useQuery({
    queryKey: ["timelineCount"],
    queryFn: () => client.countTimelineMessages(),
  });

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Timeline" count={total} />
      <LazyVirtualList<TimelineMessage>
        count={total ?? 0}
        startAtBottom
        resetKey="timeline"
        estimateSize={72}
        windowKey={(page) => ["timelineWindow", page]}
        fetchWindow={(offset, limit) => client.getTimelineWindow(offset, limit)}
        renderItem={(item, _i, prev) => (
          <TimelineRow
            item={item}
            showDate={dayChanged(prev, item)}
            onOpen={() => onOpenThread(item.threadId)}
          />
        )}
      />
    </div>
  );
}

/** A recency bucket: a labelled half-open time window with a message count. */
type Period = { key: string; label: string; lo: number | null; hi: number | null };

/** Non-overlapping recency buckets, newest first, anchored at `now` (epoch s). */
function makePeriods(now: number): Period[] {
  const DAY = 86_400;
  return [
    { key: "24h", label: "Last 24 hours", lo: now - DAY, hi: null },
    { key: "7d", label: "Last 7 days", lo: now - 7 * DAY, hi: now - DAY },
    { key: "30d", label: "Last 30 days", lo: now - 30 * DAY, hi: now - 7 * DAY },
    { key: "1y", label: "Last year", lo: now - 365 * DAY, hi: now - 30 * DAY },
    { key: "older", label: "Older", lo: null, hi: now - 365 * DAY },
  ];
}

/**
 * The full all-conversations timeline, with recency buckets on the left acting
 * as jump shortcuts INTO that one continuous list — not filters. Selecting a
 * bucket scrolls to where that period begins; scrolling past it flows naturally
 * into the neighbouring periods, and the active bucket follows the scroll.
 */
function Periods({ onOpenThread }: { onOpenThread: (threadId: number) => void }) {
  // Anchor the buckets once so their bounds (and query keys) stay stable.
  const [now] = useState(() => Math.floor(Date.now() / 1000));
  const periods = useMemo(() => makePeriods(now), [now]);

  const { data: total } = useQuery({
    queryKey: ["timelineCount"],
    queryFn: () => client.countTimelineMessages(),
  });
  const { data: counts } = useQuery({
    queryKey: ["messageRanges", now],
    queryFn: () =>
      client.countMessageRanges(periods.map((p) => ({ lo: p.lo, hi: p.hi }))),
  });

  // The timeline is ascending (oldest first), so a period's starting row index is
  // the number of messages older than it — the sum of all older buckets' counts.
  // periods is newest-first, so "older" buckets are those later in the array.
  const startIndex = useMemo(() => {
    return periods.map((_, i) => {
      let s = 0;
      for (let j = i + 1; j < periods.length; j++) s += counts?.[j] ?? 0;
      return s;
    });
  }, [periods, counts]);

  const [topIndex, setTopIndex] = useState(0);
  const [jump, setJump] = useState<{ index: number; token: number } | undefined>();

  // Which bucket the current scroll position sits in (start ≤ topIndex < next).
  const activeIndex = counts
    ? periods.findIndex(
        (_, i) => topIndex >= startIndex[i] && topIndex < startIndex[i] + (counts[i] ?? 0),
      )
    : -1;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Periods" />
          <ScrollArea className="flex-1">
            {periods.map((p, i) => {
              const count = counts?.[i];
              return (
                <PeriodRow
                  key={p.key}
                  label={p.label}
                  count={count}
                  active={activeIndex === i}
                  disabled={!count}
                  onClick={() =>
                    setJump((prev) => ({
                      index: startIndex[i],
                      token: (prev?.token ?? 0) + 1,
                    }))
                  }
                />
              );
            })}
          </ScrollArea>
        </>
      }
      detail={
        <div className="flex h-full flex-col">
          <ViewHeader title="All messages" count={total} />
          <LazyVirtualList<TimelineMessage>
            count={total ?? 0}
            startAtBottom
            resetKey="periods-timeline"
            estimateSize={72}
            jumpTo={jump}
            onTopIndexChange={setTopIndex}
            windowKey={(page) => ["timelineWindow", page]}
            fetchWindow={(offset, limit) => client.getTimelineWindow(offset, limit)}
            renderItem={(item, _i, prev) => (
              <TimelineRow
                item={item}
                showDate={dayChanged(prev, item)}
                onOpen={() => onOpenThread(item.threadId)}
              />
            )}
          />
        </div>
      }
    />
  );
}

function PeriodRow({
  label,
  count,
  active,
  disabled,
  onClick,
}: {
  label: string;
  count: number | undefined;
  active: boolean;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button
        onClick={onClick}
        disabled={disabled}
        className="w-full text-left disabled:opacity-50"
      >
        <ItemContent>
          <ItemTitle>{label}</ItemTitle>
        </ItemContent>
        <span className="ml-auto shrink-0 text-xs tabular-nums text-muted-foreground">
          {count ?? "…"}
        </span>
      </button>
    </Item>
  );
}

function TimelineRow({
  item,
  showDate,
  onOpen,
}: {
  item: TimelineMessage;
  showDate: boolean;
  onOpen: () => void;
}) {
  const m = item.message;
  return (
    <div>
      {showDate && m.sentAt && (
        <div className="px-4 py-1.5 text-center text-xs font-medium text-muted-foreground">
          {formatDateHeader(m.sentAt)}
        </div>
      )}
      <button
        onClick={onOpen}
        className="flex w-full flex-col gap-0.5 px-4 py-2 text-left hover:bg-accent"
      >
        <div className="flex items-baseline gap-2">
          <span className="truncate text-sm font-medium">{item.threadTitle}</span>
          {!m.isFromMe && m.sender && m.sender !== item.threadTitle && (
            <span className="truncate text-xs text-muted-foreground">{m.sender}</span>
          )}
          <span className="ml-auto shrink-0 text-xs text-muted-foreground">
            {formatMessageTime(m.sentAt)}
          </span>
        </div>
        <div className="text-sm text-foreground/90">
          {m.isFromMe && <span className="text-muted-foreground">You: </span>}
          {m.body ? (
            <span className="whitespace-pre-wrap break-words">{m.body}</span>
          ) : m.attachments.length ? (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
              <Paperclip className="size-3.5" />
              {m.attachments[0].filename ?? "attachment"}
            </span>
          ) : (
            <span className="text-muted-foreground">—</span>
          )}
        </div>
      </button>
    </div>
  );
}

/** A day separator is shown when the calendar day changes between rows. */
function dayChanged(prev: TimelineMessage | undefined, cur: TimelineMessage): boolean {
  if (!prev) return true;
  const p = prev.message.sentAt;
  const c = cur.message.sentAt;
  if (!p || !c) return false;
  return new Date(p * 1000).toDateString() !== new Date(c * 1000).toDateString();
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
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button onClick={onClick} className="w-full text-left">
        {showAvatars && (
          <ItemMedia>
            {isGroup(thread) ? (
              <GroupAvatar thread={thread} resolve={resolve} />
            ) : (
              <Avatar>
                {resolved?.hasImage && (
                  <AvatarImage src={client.contactAvatarUrl(resolved.id)} alt="" />
                )}
                <AvatarFallback>{initials(name)}</AvatarFallback>
              </Avatar>
            )}
          </ItemMedia>
        )}
        <ItemContent className="gap-0.5">
          <div className="flex items-baseline justify-between gap-2">
            <ItemTitle className="truncate">{name}</ItemTitle>
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
function GroupAvatar({ thread, resolve }: { thread: ThreadSummary; resolve: Resolver }) {
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
          {m?.hasImage && <AvatarImage src={client.contactAvatarUrl(m.id)} alt="" />}
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
}: {
  thread: ThreadSummary;
  resolve: Resolver;
  showContactNames: boolean;
}) {
  const name = threadLabel(thread, resolve, showContactNames);
  const group = isGroup(thread);
  // For a group, list the members under the header.
  const members = group
    ? thread.participants.map((h) => handleLabel(h, resolve, showContactNames)).join(", ")
    : null;

  // A thread can hold tens of thousands of messages; the count sizes the virtual
  // scroller and LazyVirtualList fetches only the windows it renders.
  const { data: total } = useQuery({
    queryKey: ["messageCount", thread.id],
    queryFn: () => client.countThreadMessages(thread.id),
  });

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={name}>
        {group ? (
          <span className="max-w-[60%] truncate text-xs text-muted-foreground" title={members ?? ""}>
            {thread.participants.length} people · {members}
          </span>
        ) : (
          // App threads (e.g. TikTok) store the peer's @handle as the sole
          // participant — show it next to the service.
          (() => {
            const handle = thread.participants.find((p) => p.startsWith("@"));
            const bits = [handle, thread.service].filter(Boolean);
            return bits.length > 0 ? (
              <span className="text-xs text-muted-foreground">{bits.join(" · ")}</span>
            ) : null;
          })()
        )}
      </ViewHeader>
      <LazyVirtualList<Message>
        count={total ?? 0}
        startAtBottom
        resetKey={thread.id}
        windowKey={(page) => ["messageWindow", thread.id, page]}
        fetchWindow={(offset, limit) =>
          client.getThreadMessageWindow(thread.id, offset, limit)
        }
        renderItem={(message, _i, prev) => {
          // In a group, label an incoming message with its sender — but only
          // when the sender changes, so runs of messages aren't repetitive.
          const newSender =
            !prev || prev.isFromMe || prev.sender !== message.sender;
          const senderLabel =
            group && !message.isFromMe && message.sender && newSender
              ? (showContactNames ? (resolve(message.sender)?.name ?? message.sender) : message.sender)
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
              {message.body && (
                <p className="whitespace-pre-wrap break-words">{message.body}</p>
              )}
              {message.attachments.map((a) => (
                <div key={a.id} className={cn(message.body && "mt-1.5")}>
                  <AttachmentView att={a} />
                </div>
              ))}
            </BubbleContent>
          </Bubble>
        </MessageContent>
      </MessageRow>
    </div>
  );
}

/**
 * Renders one attachment by media type: images/audio/video play inline;
 * documents and everything else show an icon that opens the file on click.
 * Only materialized attachments (with extracted bytes) are interactive.
 */
function AttachmentView({ att }: { att: Attachment }) {
  const mime = att.mimeType ?? "";
  const available = !!att.localPath;

  if (available && mime.startsWith("image/")) {
    return (
      <button
        onClick={() => client.openAttachment(att.id)}
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
      <audio controls preload="none" src={client.attachmentUrl(att.id)} className="max-w-[240px]" />
    );
  }

  // Documents and unknowns: an icon + filename that opens the file on click.
  const Icon = mime.startsWith("image/") ? ImageIcon : mime ? FileText : Paperclip;
  return (
    <button
      onClick={() => available && client.openAttachment(att.id)}
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
