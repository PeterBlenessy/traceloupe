import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Mic } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Item, ItemContent, ItemDescription, ItemTitle } from "@/components/ui/item";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { EmptyView, ErrorState, ListDetail, ViewHeader } from "@/components/view";
import { ReimportButton } from "@/components/reimport-button";
import { formatDateTime, formatDuration, formatListTime } from "@/lib/format";
import { client, type Recording } from "@/lib/ipc";

export function RecordingsView() {
  const navigate = useNavigate();
  // Subscribe to the clock preference so times re-render on change.
  const { clockFormat } = useSettings();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: recordings, isPending, error } = useQuery({
    queryKey: ["recordings"],
    queryFn: () => client.listRecordings(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [sort, setSort] = useState<SortState>({ by: "recorded", desc: true });
  const sortedRecordings = useMemo(
    () =>
      recordings
        ? sortItems(
            recordings,
            (r) =>
              sort.by === "title"
                ? recordingTitle(r).toLowerCase()
                : sort.by === "duration"
                  ? (r.durationS ?? 0)
                  : r.recordedAt,
            sort.desc,
          )
        : recordings,
    [recordings, sort],
  );

  if (active === false) {
    return (
      <EmptyView icon={Mic} title="No backup open" description="Import a backup to see recordings.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const selected =
    sortedRecordings?.find((r) => r.id === selectedId) ?? sortedRecordings?.[0] ?? null;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Recordings" count={recordings?.length}>
            <ReimportButton module="recordings" />
          </ViewHeader>
          {(recordings?.length ?? 0) > 0 && (
            <div className="flex shrink-0 justify-end border-b px-2 py-1.5">
              <SortControl
                fields={[
                  { value: "recorded", label: "Date" },
                  { value: "title", label: "Title" },
                  { value: "duration", label: "Duration" },
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
          ) : (recordings?.length ?? 0) === 0 ? (
            <p className="px-4 py-6 text-sm text-muted-foreground">
              No voice recordings in this backup.
            </p>
          ) : (
            <VirtualList
              key={clockFormat}
              items={sortedRecordings!}
              getKey={(r) => r.id}
              estimateSize={64}
              renderItem={(r) => (
                <RecordingRow
                  recording={r}
                  active={selected?.id === r.id}
                  onClick={() => setSelectedId(r.id)}
                />
              )}
            />
          )}
        </>
      }
      detail={
        selected ? (
          <RecordingDetail recording={selected} />
        ) : (
          !isPending && (
            <EmptyView
              icon={Mic}
              title="No recording selected"
              description="Pick a recording on the left."
            />
          )
        )
      }
    />
  );
}

/** A recording's display title: its label, else the filename (sans extension). */
function recordingTitle(r: Recording): string {
  const label = r.title?.trim();
  if (label) return label;
  const name = r.fileName?.replace(/\.m4a$/i, "").trim();
  return name || "Untitled recording";
}

function RecordingRow({
  recording,
  active,
  onClick,
}: {
  recording: Recording;
  active: boolean;
  onClick: () => void;
}) {
  const duration = formatDuration(Math.round(recording.durationS ?? 0));
  return (
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button onClick={onClick} className="w-full text-left">
        <ItemContent className="gap-0.5">
          <div className="flex items-baseline justify-between gap-2">
            <ItemTitle className="truncate">{recordingTitle(recording)}</ItemTitle>
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatListTime(recording.recordedAt)}
            </span>
          </div>
          <ItemDescription className="truncate">{duration || "—"}</ItemDescription>
        </ItemContent>
      </button>
    </Item>
  );
}

function RecordingDetail({ recording }: { recording: Recording }) {
  const duration = formatDuration(Math.round(recording.durationS ?? 0));
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={recordingTitle(recording)}>
        {duration && <span className="text-xs text-muted-foreground">{duration}</span>}
      </ViewHeader>
      <div className="flex flex-1 flex-col items-center justify-center gap-6 p-6">
        <div className="flex flex-col items-center gap-2 text-center">
          <div className="flex h-16 w-16 items-center justify-center rounded-full bg-accent">
            <Mic className="h-7 w-7 text-muted-foreground" />
          </div>
          {recording.recordedAt && (
            <p className="text-sm text-muted-foreground">{formatDateTime(recording.recordedAt)}</p>
          )}
        </div>
        {/* `key` forces the element to reload its source when switching recordings. */}
        <audio
          key={recording.id}
          controls
          preload="metadata"
          src={client.audioUrl(recording.id)}
          className="w-full max-w-md"
        />
        {recording.fileName && (
          <p className="max-w-md truncate text-xs text-muted-foreground">{recording.fileName}</p>
        )}
      </div>
    </div>
  );
}
