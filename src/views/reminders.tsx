import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Circle, CircleCheck, Flag, ListTodo } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyView, VirtualListView } from "@/components/view";
import { formatDate } from "@/lib/format";
import { cn } from "@/lib/utils";
import { client, type Reminder } from "@/lib/ipc";

function ReminderRow({ reminder }: { reminder: Reminder }) {
  return (
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
            <Badge variant="secondary" className="shrink-0">
              {reminder.listName}
            </Badge>
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
  );
}

export function RemindersView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: reminders,
    isPending,
    error,
  } = useQuery({
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

  return (
    <VirtualListView<Reminder>
      title="Reminders"
      count={reminders?.length}
      items={reminders ?? []}
      getKey={(r) => r.id}
      estimateSize={64}
      isPending={isPending}
      error={error}
      emptyMessage="No reminders in this backup."
      renderItem={(r) => <ReminderRow reminder={r} />}
    />
  );
}
