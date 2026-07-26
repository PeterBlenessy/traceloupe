import { useEffect, useRef } from "react";

/**
 * Declares that a list is deliberately NOT virtualized because its length is
 * bounded — and checks that claim while developing (#67).
 *
 * The lesson from #61 is that "this list will always be short" is an assumption,
 * not a fact: ~8000 findings in a list written when findings were expected to be
 * few drove the render process to 99% CPU and 3.1 GB and froze the laptop. A
 * comment saying "one row per import module" ages silently; this does not.
 *
 * Call it at any list that renders every row, passing the bound you believe
 * holds and why. In dev, exceeding the bound logs once with the site's name; in
 * production it costs nothing but the hook call. It is a smoke alarm, not a
 * guard rail — it does not change what renders.
 */
export function useBoundedList(
  /** Where this list lives, e.g. "import-dialog modules". */
  name: string,
  /** How many rows it is about to render. */
  count: number,
  /** The most it should ever render, and the reason that holds. */
  bound: number,
) {
  // Warn once per site, not once per render: a list over its bound re-renders
  // constantly, and a flood of identical warnings is how the log became
  // unreadable in #60.
  const warned = useRef(false);
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    if (count <= bound || warned.current) return;
    warned.current = true;
    console.warn(
      `[bounded-list] "${name}" rendered ${count} rows but was declared bounded at ` +
        `${bound}. Either the bound is wrong or this list now needs the shared ` +
        `virtualization (VirtualList / VirtualListView / LazyListView) — see #67.`,
    );
  }, [name, count, bound]);
}
