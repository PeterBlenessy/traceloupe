import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { CalendarDays, Clock, MapPin } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyView, ViewHeader } from "@/components/view";
import { VirtualList } from "@/components/virtual-list";
import { formatDate, formatDateTime } from "@/lib/format";
import { client, type CalendarEvent } from "@/lib/ipc";

const timeFmt = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
});

/** The event's when-line: "All day" or a start (→ end) date/time. */
function whenLabel(e: CalendarEvent): string {
  if (e.startAt == null) return "—";
  if (e.allDay) return `${formatDate(e.startAt)} · All day`;
  const start = formatDateTime(e.startAt);
  if (e.endAt != null && e.endAt > e.startAt) {
    // Same-day end → show just the end time, else the full end date/time.
    const sameDay =
      new Date(e.startAt * 1000).toDateString() ===
      new Date(e.endAt * 1000).toDateString();
    return `${start} – ${sameDay ? timeFmt.format(new Date(e.endAt * 1000)) : formatDateTime(e.endAt)}`;
  }
  return start;
}

function EventRow({ event }: { event: CalendarEvent }) {
  return (
    <div className="px-2 py-0.5">
      <div className="rounded-md border px-3 py-2.5">
        <div className="flex items-baseline justify-between gap-2">
          <span className="truncate font-medium">
            {event.title ?? "(untitled event)"}
          </span>
          {event.calendarName && (
            <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-xs text-muted-foreground">
              {event.calendarName}
            </span>
          )}
        </div>
        <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
          <Clock className="size-3 shrink-0" />
          <span className="select-text">{whenLabel(event)}</span>
        </div>
        {event.location && (
          <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
            <MapPin className="size-3 shrink-0" />
            <span className="select-text truncate">{event.location}</span>
          </div>
        )}
        {event.notes && (
          <p className="mt-1 select-text whitespace-pre-wrap break-words text-sm text-muted-foreground">
            {event.notes}
          </p>
        )}
      </div>
    </div>
  );
}

export function CalendarView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: events } = useQuery({
    queryKey: ["calendarEvents"],
    queryFn: () => client.listCalendarEvents(),
    enabled: active === true,
  });

  if (active === false) {
    return (
      <EmptyView
        icon={CalendarDays}
        title="No backup open"
        description="Import a backup to see calendar events."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Calendar" count={events?.length} />
      <div className="min-h-0 flex-1">
        {events && events.length === 0 ? (
          <p className="px-4 py-6 text-sm text-muted-foreground">
            No calendar events in this backup.
          </p>
        ) : (
          <div className="mx-auto h-full max-w-2xl">
            <VirtualList
              items={events ?? []}
              getKey={(e) => e.id}
              estimateSize={76}
              renderItem={(e) => <EventRow event={e} />}
            />
          </div>
        )}
      </div>
    </div>
  );
}
