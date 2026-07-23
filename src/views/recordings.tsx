import { useMemo, useState } from "react";
import { usePersistedState } from "@/lib/use-persisted-state";
import { useQuery } from "@tanstack/react-query";
import { Mic } from "lucide-react";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemTitle,
} from "@/components/ui/item";
import { VirtualList } from "@/components/virtual-list";
import { useTimePresets } from "@/components/time-filter";
import { useSettings } from "@/components/settings-provider";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useViewToolbar } from "@/components/toolbar-context";
import { timeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState,
  EmptyView,
  ErrorState,
  ListDetail,
  ListSearch,
  ListSkeleton,
  ViewHeader,
} from "@/components/view";
import { formatDateTime, formatDuration, formatListTime } from "@/lib/format";
import { client, type Recording, type TimeRange } from "@/lib/ipc";

export function RecordingsView() {
  // Subscribe to the clock preference so times re-render on change.
  const { clockFormat } = useSettings();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: recordings,
    isPending,
    error,
  } = useQuery({
    queryKey: ["recordings"],
    queryFn: () => client.listRecordings(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [q, setQ] = useState("");
  const [sort, setSort] = usePersistedState<SortState>("recordings:sort", { by: "recorded", desc: true });
  // Time filter — same preset chips + custom range as Photos/Notes, over the
  // recording's recorded date.
  const { presets } = useTimePresets();
  const [range, setRange] = useState<TimeRange>({ lo: null, hi: null });

  // Whether a recording's date falls in a [lo, hi) window (undated ones only pass
  // the fully-open "All" window).
  const inWindow = (at: number | null, lo: number | null, hi: number | null) => {
    if (lo == null && hi == null) return true;
    if (at == null) return false;
    return (lo == null || at >= lo) && (hi == null || at < hi);
  };

  // Search + title match, before the time filter — the base for the chip counts.
  const baseFiltered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return (recordings ?? []).filter(
      (r) => !needle || recordingTitle(r).toLowerCase().includes(needle),
    );
  }, [recordings, q]);

  const presetCounts = useMemo(
    () =>
      presets.map(
        (p) => baseFiltered.filter((r) => inWindow(r.recordedAt, p.lo, p.hi)).length,
      ),
    [presets, baseFiltered],
  );

  const sortedRecordings = useMemo(() => {
    if (!recordings) return recordings;
    const matched = baseFiltered.filter((r) =>
      inWindow(r.recordedAt, range.lo, range.hi),
    );
    return sortItems(
      matched,
      (r) =>
        sort.by === "title"
          ? recordingTitle(r).toLowerCase()
          : sort.by === "duration"
            ? (r.durationS ?? 0)
            : r.recordedAt,
      sort.desc,
    );
  }, [recordings, sort, baseFiltered, range]);

  const hasRecordings = (recordings?.length ?? 0) > 0;
  const filterGroups = useMemo<FilterGroup[]>(
    () =>
      hasRecordings
        ? [timeGroup({ description: "When the recording was made", presets, counts: presetCounts, value: range, onChange: setRange })]
        : [],
    [hasRecordings, presets, presetCounts, range],
  );
  const sortNode = useMemo(
    () =>
      hasRecordings ? (
        <SortControl
          fields={[
            { value: "recorded", label: "Date" },
            { value: "title", label: "Title" },
            { value: "duration", label: "Duration" },
          ]}
          value={sort}
          onChange={setSort}
        />
      ) : undefined,
    [hasRecordings, sort, setSort],
  );
  const searchNode = useMemo(
    () => (hasRecordings ? <ListSearch value={q} onChange={setQ} placeholder="Search recordings" /> : undefined),
    [hasRecordings, q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Recordings", count: sortedRecordings?.length, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, sortedRecordings?.length, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <NoBackupState
        icon={Mic}
        title="Hear voice memos"
        lead="The device's Voice Memos, restored from the backup and playable right here with an inline audio player."
        features={[
          { label: "Search", detail: "Search recordings by title." },
          { label: "Time range", detail: "Narrow to any date range." },
          { label: "Sort", detail: "Order by date, title, or duration." },
          { label: "Playback", detail: "Play any memo inline and see its folder, date, and length." },
        ]}
        note="Audio is read directly from the backup on this Mac."
      />
    );
  }

  const selected =
    sortedRecordings?.find((r) => r.id === selectedId) ??
    sortedRecordings?.[0] ??
    null;

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1">
        <ListDetail
          master={
            error ? (
              <ErrorState error={error} />
            ) : isPending ? (
              <ListSkeleton rows={6} />
            ) : (recordings?.length ?? 0) === 0 ? (
              <EmptyView icon={Mic} title="No voice recordings in this backup." />
            ) : (sortedRecordings?.length ?? 0) === 0 ? (
              <EmptyView icon={Mic} title="No recordings match the current search or time range." />
            ) : (
              <VirtualList
                key={clockFormat}
                items={sortedRecordings!}
                underlap
                getKey={(r) => r.id}
                estimateSize={64}
                renderItem={(r) => (
                  <div className="px-2 py-0.5">
                    <RecordingRow
                      recording={r}
                      active={selected?.id === r.id}
                      onClick={() => setSelectedId(r.id)}
                    />
                  </div>
                )}
              />
            )
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
      </div>
    </div>
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
    <Item
      asChild
      data-active={active}
      className="rounded-md transition-colors hover:bg-accent/50 data-[active=true]:bg-accent data-[active=true]:hover:bg-accent"
    >
      <button onClick={onClick} className="w-full text-left">
        <ItemContent className="gap-0.5">
          <div className="flex items-baseline justify-between gap-2">
            <ItemTitle className="truncate">
              {recordingTitle(recording)}
            </ItemTitle>
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatListTime(recording.recordedAt)}
            </span>
          </div>
          <ItemDescription className="truncate">
            {[duration || "—", recording.folder].filter(Boolean).join(" · ")}
          </ItemDescription>
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
        {duration && (
          <span className="text-xs text-muted-foreground">{duration}</span>
        )}
      </ViewHeader>
      <div className="flex flex-1 flex-col items-center justify-center gap-6 p-6">
        <div className="flex flex-col items-center gap-2 text-center">
          <div className="flex h-16 w-16 items-center justify-center rounded-full bg-accent">
            <Mic className="h-7 w-7 text-muted-foreground" />
          </div>
          {recording.recordedAt && (
            <p className="text-sm text-muted-foreground">
              {formatDateTime(recording.recordedAt)}
            </p>
          )}
          {recording.folder && (
            <p className="text-xs text-muted-foreground">
              {recording.folder}
            </p>
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
          <p className="max-w-md truncate text-xs text-muted-foreground">
            {recording.fileName}
          </p>
        )}
      </div>
    </div>
  );
}
