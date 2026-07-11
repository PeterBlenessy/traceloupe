import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ImageIcon, MessageSquare, Paperclip } from "lucide-react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyView, ListDetail, ViewHeader } from "@/components/view";
import { cn } from "@/lib/utils";
import { formatListTime, formatMessageTime } from "@/lib/format";
import { initials } from "@/lib/contact";
import { client, type Message, type ThreadSummary } from "@/lib/ipc";

export function MessagesView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: threads, isPending } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);

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

  const selected = threads?.find((t) => t.id === selectedId) ?? threads?.[0] ?? null;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Messages" count={threads?.length} />
          <ScrollArea className="flex-1">
            {isPending &&
              Array.from({ length: 6 }).map((_, i) => (
                <div key={i} className="px-3 py-2">
                  <Skeleton className="h-12 w-full" />
                </div>
              ))}
            {threads?.length === 0 && (
              <p className="px-4 py-6 text-sm text-muted-foreground">
                No messages in this backup.
              </p>
            )}
            {threads?.map((t) => (
              <ThreadRow
                key={t.id}
                thread={t}
                active={selected?.id === t.id}
                onClick={() => setSelectedId(t.id)}
              />
            ))}
          </ScrollArea>
        </>
      }
      detail={
        selected ? (
          <Conversation thread={selected} />
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

function ThreadRow({
  thread,
  active,
  onClick,
}: {
  thread: ThreadSummary;
  active: boolean;
  onClick: () => void;
}) {
  const name = thread.displayName ?? thread.identifier;
  return (
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button onClick={onClick} className="w-full text-left">
        <ItemMedia>
          <Avatar>
            <AvatarFallback>{initials(name)}</AvatarFallback>
          </Avatar>
        </ItemMedia>
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

function Conversation({ thread }: { thread: ThreadSummary }) {
  const { data: messages, isPending } = useQuery({
    queryKey: ["messages", thread.id],
    queryFn: () => client.getThreadMessages(thread.id),
  });
  const name = thread.displayName ?? thread.identifier;

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={name}>
        {thread.service && (
          <span className="text-xs text-muted-foreground">{thread.service}</span>
        )}
      </ViewHeader>
      <ScrollArea className="flex-1">
        <div className="space-y-1 px-4 py-4">
          {isPending && <p className="text-sm text-muted-foreground">Loading…</p>}
          {messages?.map((m, i) => (
            <MessageBubble key={m.id} message={m} showTime={shouldShowTime(messages, i)} />
          ))}
        </div>
      </ScrollArea>
    </div>
  );
}

/** Show a timestamp separator when >15 min elapsed since the previous message. */
function shouldShowTime(messages: Message[], i: number): boolean {
  if (i === 0) return true;
  const prev = messages[i - 1].sentAt ?? 0;
  const cur = messages[i].sentAt ?? 0;
  return cur - prev > 15 * 60;
}

function MessageBubble({ message, showTime }: { message: Message; showTime: boolean }) {
  const fromMe = message.isFromMe;
  return (
    <div>
      {showTime && message.sentAt && (
        <div className="py-2 text-center text-xs text-muted-foreground">
          {formatMessageTime(message.sentAt)}
        </div>
      )}
      <div className={cn("flex", fromMe ? "justify-end" : "justify-start")}>
        <div
          className={cn(
            "max-w-[70%] rounded-2xl px-3 py-2 text-sm",
            fromMe
              ? "rounded-br-sm bg-primary text-primary-foreground"
              : "rounded-bl-sm bg-muted text-foreground",
          )}
        >
          {message.body && (
            <p className="select-text whitespace-pre-wrap break-words">{message.body}</p>
          )}
          {message.attachments.map((a, i) => (
            <div
              key={i}
              className={cn(
                "mt-1 flex items-center gap-1.5 text-xs",
                fromMe ? "text-primary-foreground/80" : "text-muted-foreground",
              )}
            >
              {a.mimeType?.startsWith("image/") ? (
                <ImageIcon className="size-3.5" />
              ) : (
                <Paperclip className="size-3.5" />
              )}
              <span className="select-text truncate">{a.filename ?? "attachment"}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
