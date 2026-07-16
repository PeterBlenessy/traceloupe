import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ArrowDownLeft, ArrowUpRight, Waypoints } from "lucide-react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { EmptyView, VirtualListView } from "@/components/view";
import { initials } from "@/lib/contact";
import { formatCount, formatDate } from "@/lib/format";
import { client, type Interaction } from "@/lib/ipc";

function label(i: Interaction): string {
  return i.displayName ?? i.identifier ?? "Unknown";
}

function InteractionRow({ interaction }: { interaction: Interaction }) {
  const name = label(interaction);
  const total = interaction.incoming + interaction.outgoing;
  return (
    <div className="flex items-center gap-3 rounded-md border px-3 py-2.5">
      <Avatar className="size-9 shrink-0">
        <AvatarFallback>{initials(name)}</AvatarFallback>
      </Avatar>
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium">{name}</div>
        {interaction.displayName && interaction.identifier && (
          <div className="truncate text-xs text-muted-foreground">
            {interaction.identifier}
          </div>
        )}
        {interaction.firstAt != null && interaction.lastAt != null && (
          <div className="text-xs text-muted-foreground">
            {formatDate(interaction.firstAt)} – {formatDate(interaction.lastAt)}
          </div>
        )}
      </div>
      <div className="flex shrink-0 flex-col items-end gap-0.5 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">{formatCount(total)}</span>
        <span className="inline-flex items-center gap-1.5 tabular-nums">
          <ArrowDownLeft className="size-3" />
          {formatCount(interaction.incoming)}
          <ArrowUpRight className="ml-1 size-3" />
          {formatCount(interaction.outgoing)}
        </span>
      </div>
    </div>
  );
}

export function InteractionsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const {
    data: interactions,
    isPending,
    error,
  } = useQuery({
    queryKey: ["interactions"],
    queryFn: () => client.listInteractions(),
    enabled: active === true,
  });

  if (active === false) {
    return (
      <EmptyView
        icon={Waypoints}
        title="No backup open"
        description="Import a backup to see the interaction graph."
      >
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  return (
    <VirtualListView<Interaction>
      title="Interactions"
      count={interactions?.length}
      items={interactions ?? []}
      getKey={(i) => i.id}
      estimateSize={68}
      isPending={isPending}
      error={error}
      emptyMessage="No interaction data in this backup."
      renderItem={(i) => <InteractionRow interaction={i} />}
    />
  );
}
