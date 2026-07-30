/**
 * Artifacts hosted inside another view, grouped by the row they belong to.
 *
 * The agreed rule is that artifact data folds into the view closest in meaning —
 * app permissions into Apps, not a generic "Artifacts" destination. A host view
 * asks for the artifacts assigned to it and gets them keyed by the value it
 * already displays (a bundle id, a handle), so it can attach each one to the
 * right row without knowing anything about the artifact itself.
 *
 * Which column to key on comes from the module's own `join_column`. The host does
 * not guess, and a module that omits it fails to load.
 */
import { useMemo } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";

import { client, type ArtifactRow, type ArtifactSummary } from "@/lib/ipc";

/** How many rows to pull per hosted artifact. These are per-device sets
 *  (permissions, alarms) — hundreds, not the hundreds of thousands that
 *  Messages deals in. A cap rather than paging, and it is stated rather than
 *  assumed: an artifact that outgrows it needs real paging, not a bigger guess. */
const PAGE = 5000;

export type HostedArtifact = {
  artifact: ArtifactSummary;
  /** join value (lowercased) → its rows. */
  byKey: Map<string, ArtifactRow[]>;
};

/**
 * Every artifact declaring `surface === host`, with rows grouped by join value.
 *
 * `enabled` lets a host skip the work entirely when it has no backup open.
 */
export function useHostedArtifacts(host: string, enabled: boolean): {
  hosted: HostedArtifact[];
  isPending: boolean;
} {
  const { data: artifacts, isPending: listPending } = useQuery({
    queryKey: ["artifacts"],
    queryFn: () => client.listArtifacts(),
    enabled,
  });

  const mine = useMemo(
    () => (artifacts ?? []).filter((a) => a.surface === host && a.joinColumn),
    [artifacts, host],
  );

  const rowQueries = useQueries({
    queries: mine.map((a) => ({
      queryKey: ["artifactRows", a.id],
      queryFn: () => client.getArtifactRows(a.id, 0, PAGE),
      enabled: enabled && a.rowCount > 0,
    })),
  });

  const hosted = useMemo(() => {
    return mine.map((artifact, i) => {
      const rows = rowQueries[i]?.data ?? [];
      const byKey = new Map<string, ArtifactRow[]>();
      const col = artifact.joinColumn!;
      for (const row of rows) {
        const raw = row[col];
        if (raw === null || raw === undefined) continue;
        // Bundle ids are case-stable in practice, but matching case-insensitively
        // costs nothing and a mismatch here silently drops a row from its app.
        const key = String(raw).toLowerCase();
        const list = byKey.get(key);
        if (list) list.push(row);
        else byKey.set(key, [row]);
      }
      return { artifact, byKey };
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mine, rowQueries.map((q) => q.data).join("|")]);

  return {
    hosted,
    isPending: listPending || rowQueries.some((q) => q.isPending),
  };
}
