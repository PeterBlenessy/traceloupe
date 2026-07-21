import { ShieldAlert } from "lucide-react";
import { useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { client } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/**
 * Inline Safety Scan flag (plan T9): a small severity-colored shield shown on
 * a Messages thread row or a Notes row that has at least one live Content
 * Finding. Clicking it jumps to the Safety Scan page. Fed by the cheap
 * per-source `safetyScanFindingMarks` lookup — no per-row query.
 */

const SEVERITY_CLASS: Record<1 | 2 | 3, string> = {
  3: "text-destructive",
  2: "text-amber-600 dark:text-amber-400",
  1: "text-muted-foreground",
};

const SEVERITY_LABEL: Record<1 | 2 | 3, string> = {
  3: "Serious finding",
  2: "Harmful finding",
  1: "Concerning finding",
};

/** Load the marks once; components read the map by id. Cached under the
 *  ['safetyScan','marks'] key so a scan/dismiss invalidation refreshes it. */
export function useFindingMarks() {
  return useQuery({
    queryKey: ["safetyScan", "marks"],
    queryFn: () => client.safetyScanFindingMarks(),
    // Marks are advisory chrome; don't block list render or spam refetches.
    staleTime: 30_000,
  });
}

export function SafetyFlagBadge({
  severity,
  className,
}: {
  severity: 1 | 2 | 3;
  className?: string;
}) {
  const navigate = useNavigate();
  // severity crosses the IPC seam as a u8; clamp to a valid key so an
  // out-of-range value can't crash the row it badges.
  const sev: 1 | 2 | 3 = severity === 3 ? 3 : severity === 2 ? 2 : 1;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role="button"
          tabIndex={0}
          aria-label={SEVERITY_LABEL[sev]}
          className={cn(
            "inline-flex shrink-0 cursor-pointer items-center",
            SEVERITY_CLASS[sev],
            className,
          )}
          onClick={(e) => {
            e.stopPropagation();
            e.preventDefault();
            navigate({ to: "/safety-scan" });
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.stopPropagation();
              e.preventDefault();
              navigate({ to: "/safety-scan" });
            }
          }}
        >
          <ShieldAlert className="size-3.5" />
        </span>
      </TooltipTrigger>
      <TooltipContent>{SEVERITY_LABEL[sev]} — open Safety Scan</TooltipContent>
    </Tooltip>
  );
}
