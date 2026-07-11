import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ImageIcon, MessageSquare, Paperclip } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { formatListTime, formatMessageTime } from "@/lib/format";
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
      <EmptyState
        title="No backup open"
        body="Import a backup to read its messages."
        action={<Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>}
      />
    );
  }

  const selected =
    threads?.find((t) => t.id === selectedId) ?? threads?.[0] ?? null;

  return (
    <div className="flex h-full">
      <div className="flex w-72 shrink-0 flex-col border-r">
        <header className="px-4 py-3 text-sm font-semibold">Messages</header>
        <div className="flex-1 overflow-auto">
          {isPending &&
            Array.from({ length: 5 }).map((_, i) => (
              <div key={i} className="px-4 py-3">
                <Skeleton className="h-10 w-full" />
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
        </div>
      </div>
      <div className="flex-1">
        {selected ? (
          <Conversation thread={selected} />
        ) : (
          !isPending && (
            <EmptyState title="No conversation selected" body="Pick a thread on the left." />
          )
        )}
      </div>
    </div>
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
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex w-full flex-col gap-0.5 border-b px-4 py-3 text-left transition-colors",
        "hover:bg-accent/50",
        active && "bg-accent",
      )}
    >
      <div className="flex items-baseline justify-between gap-2">
        <span className="truncate text-sm font-medium">
          {thread.displayName ?? thread.identifier}
        </span>
        <span className="shrink-0 text-xs text-muted-foreground">
          {formatListTime(thread.lastMessageAt)}
        </span>
      </div>
      <span className="truncate text-xs text-muted-foreground">
        {thread.snippet ?? "No messages"}
      </span>
    </button>
  );
}

function Conversation({ thread }: { thread: ThreadSummary }) {
  const { data: messages, isPending } = useQuery({
    queryKey: ["messages", thread.id],
    queryFn: () => client.getThreadMessages(thread.id),
  });

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-2 border-b px-4 py-3">
        <div className="flex size-8 items-center justify-center rounded-full bg-muted">
          <MessageSquare className="size-4 text-muted-foreground" />
        </div>
        <div>
          <div className="text-sm font-medium">
            {thread.displayName ?? thread.identifier}
          </div>
          {thread.service && (
            <div className="text-xs text-muted-foreground">{thread.service}</div>
          )}
        </div>
      </header>
      <div className="flex-1 space-y-1 overflow-auto px-4 py-4">
        {isPending && <p className="text-sm text-muted-foreground">Loading…</p>}
        {messages?.map((m, i) => (
          <MessageBubble
            key={m.id}
            message={m}
            showTime={shouldShowTime(messages, i)}
          />
        ))}
      </div>
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
          {message.body && <p className="whitespace-pre-wrap break-words select-text">{message.body}</p>}
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
              <span className="truncate select-text">{a.filename ?? "attachment"}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function EmptyState({
  title,
  body,
  action,
}: {
  title: string;
  body: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
      <h1 className="text-lg font-medium">{title}</h1>
      <p className="max-w-sm text-sm text-muted-foreground">{body}</p>
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}
