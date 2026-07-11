import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Image as ImageIcon, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { EmptyView, ViewHeader } from "@/components/view";
import { formatDateTime } from "@/lib/format";
import { client, type MediaItem } from "@/lib/ipc";

export function GalleryView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: media, isPending } = useQuery({
    queryKey: ["media"],
    queryFn: () => client.listMedia(),
    enabled: active === true,
  });
  const { data: sources } = useQuery({
    queryKey: ["mediaSources"],
    queryFn: () => client.mediaSources(),
    enabled: active === true,
  });
  const [openId, setOpenId] = useState<number | null>(null);
  const [source, setSource] = useState<string>("all");

  const filtered = useMemo(() => {
    if (!media) return [];
    if (source === "all") return media;
    return media.filter((m) => (m.source ?? "Other") === source);
  }, [media, source]);

  if (active === false) {
    return (
      <EmptyView icon={ImageIcon} title="No backup open" description="Import a backup to see photos and videos.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const openItem = filtered.find((m) => m.id === openId) ?? null;
  const hasFilter = (sources?.length ?? 0) > 1;

  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Gallery" count={media?.length} />
      {hasFilter && (
        <div className="shrink-0 border-b px-2 py-2">
          <SourceFilter
            sources={sources ?? []}
            total={media?.length ?? 0}
            value={source}
            onChange={setSource}
          />
        </div>
      )}
      <ScrollArea className="flex-1">
        {isPending && (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-1 p-1">
            {Array.from({ length: 12 }).map((_, i) => (
              <Skeleton key={i} className="aspect-square" />
            ))}
          </div>
        )}
        {media?.length === 0 && (
          <p className="p-6 text-center text-sm text-muted-foreground">
            No photos or videos in this backup.
          </p>
        )}
        {filtered.length > 0 && (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-1 p-1">
            {filtered.map((m) => (
              <Thumb key={m.id} item={m} onOpen={() => setOpenId(m.id)} />
            ))}
          </div>
        )}
      </ScrollArea>

      <Lightbox item={openItem} onClose={() => setOpenId(null)} />
    </div>
  );
}

function SourceFilter({
  sources,
  total,
  value,
  onChange,
}: {
  sources: [string, number][];
  total: number;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <ToggleGroup
      type="single"
      value={value}
      onValueChange={(v) => onChange(v || "all")}
      variant="outline"
      size="sm"
      className="flex-wrap justify-start"
    >
      <ToggleGroupItem value="all">All {total}</ToggleGroupItem>
      {sources.map(([name, count]) => (
        <ToggleGroupItem key={name} value={name}>
          {name} {count}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

function Thumb({ item, onOpen }: { item: MediaItem; onOpen: () => void }) {
  const isVideo = item.kind === "video";
  return (
    <button
      onClick={onOpen}
      className="group relative aspect-square overflow-hidden rounded-sm bg-muted"
    >
      <img
        src={client.mediaUrl(item.id, { thumb: true })}
        alt={item.filename ?? ""}
        loading="lazy"
        className="size-full object-cover transition-transform group-hover:scale-105"
      />
      {item.source && (
        <span className="absolute bottom-1 left-1 rounded bg-black/55 px-1.5 py-0.5 text-[10px] font-medium text-white opacity-0 transition-opacity group-hover:opacity-100">
          {item.source}
        </span>
      )}
      {isVideo && (
        <span className="absolute inset-0 flex items-center justify-center bg-black/20">
          <Play className="size-8 fill-white text-white" />
        </span>
      )}
    </button>
  );
}

function Lightbox({ item, onClose }: { item: MediaItem | null; onClose: () => void }) {
  return (
    <Dialog open={!!item} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-3xl gap-2 p-2">
        <DialogTitle className="sr-only">{item?.filename ?? "Media"}</DialogTitle>
        {item && (
          <>
            <div className="flex items-center justify-center bg-muted/40">
              <img
                src={client.mediaUrl(item.id)}
                alt={item.filename ?? ""}
                className="max-h-[70vh] w-auto object-contain"
              />
            </div>
            <div className="flex items-center justify-between gap-2 px-2 pb-1 text-xs text-muted-foreground">
              <span className="select-text truncate">{item.filename ?? "—"}</span>
              <div className="flex shrink-0 items-center gap-2">
                {item.source && <span>{item.source}</span>}
                {item.takenAt && <span>{formatDateTime(item.takenAt)}</span>}
              </div>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
