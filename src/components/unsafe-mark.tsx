/**
 * The person's own "unsafe" mark, for any kind of item.
 *
 * Photos had this first, and the mechanism was right while the scope was wrong:
 * a message, a contact or a visited page is every bit as likely to be the thing
 * someone wants to find again. One hook and one control serve every view, so the
 * mark cannot drift into three slightly different features.
 *
 * The mark is the PERSON's, not the device's — nothing in the backup says a
 * photo is unsafe. It survives a re-import because the backend keys it on the
 * item's own content rather than a cache row id.
 */
import { useCallback } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ShieldAlert } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { client } from "@/lib/ipc";

/** Which kind of thing is being marked. Matches the backend's `MarkKind`. */
export type MarkKind = "media" | "message" | "contact";

/**
 * The marked ids for one kind, plus a toggle.
 *
 * Ids rather than a flag on every row: the marked set is what a person picked by
 * hand, so it is small, and fetching it once beats carrying a boolean through
 * every list query.
 */
export function useUnsafeMarks(kind: MarkKind) {
  const qc = useQueryClient();
  const { data } = useQuery({
    queryKey: ["marks", kind],
    queryFn: () => client.markedIds(kind),
    // Advisory chrome: never block a list render on it.
    staleTime: 30_000,
  });
  const marked = new Set(data ?? []);

  const mutation = useMutation({
    mutationFn: ({ id, on }: { id: number; on: boolean }) =>
      client.setItemMark(kind, id, on),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["marks", kind] });
      void qc.invalidateQueries({ queryKey: ["markCounts"] });
    },
  });

  const toggle = useCallback(
    (id: number) => mutation.mutate({ id, on: !marked.has(id) }),
    // `marked` is rebuilt each render; keying on the query data keeps the
    // callback stable for the rows that did not change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [data, mutation],
  );

  return { marked, toggle, count: marked.size };
}

/** How many of each kind are marked — for a filter badge that must not lie. */
export function useMarkCounts() {
  const { data } = useQuery({
    queryKey: ["markCounts"],
    queryFn: () => client.markCounts(),
    staleTime: 30_000,
  });
  return data ?? { media: 0, message: 0, contact: 0 };
}

/**
 * The toggle itself. Amber shield, filled when marked — the same signal Photos
 * already used, so the mark reads identically wherever it appears.
 */
export function UnsafeMarkButton({
  marked,
  onToggle,
  label,
  className,
}: {
  marked: boolean;
  onToggle: () => void;
  /** What is being marked, for the tooltip and the accessible name. */
  label: string;
  className?: string;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={marked ? `Remove unsafe mark from ${label}` : `Mark ${label} unsafe`}
          aria-pressed={marked}
          onClick={(e) => {
            // Rows are buttons; marking one must not also open it.
            e.stopPropagation();
            onToggle();
          }}
          className={className}
        >
          <ShieldAlert
            className={cn(
              "size-4",
              marked ? "text-amber-400" : "text-muted-foreground/50",
            )}
          />
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {marked
          ? "Marked unsafe — click to remove"
          : "Mark unsafe, to find it again later"}
      </TooltipContent>
    </Tooltip>
  );
}
