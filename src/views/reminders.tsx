import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Circle, CircleCheck, Flag, ListTodo } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyView, ViewHeader } from "@/components/view";
import { VirtualList } from "@/components/virtual-list";
import { formatDate } from "@/lib/format";
import { cn } from "@/lib/utils";
import { client, type Reminder } from "@/lib/ipc";

function ReminderRow({ reminder }: { reminder: Reminder }) {
  return (
    <div className="px-2 py-0.5">
      <div className="flex items-start gap-2.5 rounded-md border px-3 py-2.5">
        {reminder.completed ? (
          <CircleCheck className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        ) : (
          <Circle className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        )}
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-2">
            <span
              className={cn(
                "truncate",
                reminder.completed && "text-muted-foreground line-through",
              )}
            >
              {reminder.title ?? "(untitled)"}
              {reminder.flagged && (
                <Flag className="ml-1.5 inline size-3 fill-orange-500 text-orange-500" />
              )}
            </span>
            {reminder.listName && (
              <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-xs text-muted-foreground">
                {reminder.listName}
              </span>
            )}
          </div>
          {reminder.notes && (
            <p className="mt-0.5 select-text whitespace-pre-wrap break-words text-sm text-muted-foreground">
              {reminder.notes}
            </p>
          )}
          {(reminder.dueAt != null || reminder.completedAt != null) && (
            <div className="mt-0.5 text-xs text-muted-foreground">
              {reminder.completed && reminder.completedAt != null
                ? `Completed ${formatDate(reminder.completedAt)}`
                : reminder.dueAt != null
                  ? `Due ${formatDate(reminder.dueAt)}`
                  : null}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export function RemindersView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: reminders } = useQuery({
    queryKey: ["reminders"],
    queryFn: () => client.listReminders(),
    enabled: active === true,
  });

  if (active === false) {
    return (
      <EmptyView
        icon={ListTodo}
        title="No backup open"
        description="Import a backup to see reminders."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const open = reminders?.filter((r) => !r.completed).length ?? 0;

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Reminders" count={open} />
      <div className="min-h-0 flex-1">
        {reminders && reminders.length === 0 ? (
          <p className="px-4 py-6 text-sm text-muted-foreground">
            No reminders in this backup.
          </p>
        ) : (
          <div className="mx-auto h-full max-w-2xl">
            <VirtualList
              items={reminders ?? []}
              getKey={(r) => r.id}
              estimateSize={64}
              renderItem={(r) => <ReminderRow reminder={r} />}
            />
          </div>
        )}
      </div>
    </div>
  );
}
