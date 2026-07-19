import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import type { ToolbarIsland } from "@/components/adaptive-toolbar";
import type { FilterGroup } from "@/components/filter-groups";

/**
 * The current view's contribution to the app's single unified toolbar. A view
 * publishes its title + islands + search here (via {@link useViewToolbar}); the
 * app shell renders one {@link AdaptiveToolbar} combining these with the app-wide
 * controls. `null` = the view has no toolbar (the shell shows just app controls).
 */
export interface ViewToolbar {
  title?: string;
  count?: number;
  islands: ToolbarIsland[];
  search?: ReactNode;
  searchExpanded?: boolean;
  /** Faceted filter groups for the single **Filter** control. When present, the
   *  shell renders the new filter/sort/search cluster instead of `islands`. */
  filter?: FilterGroup[];
  /** Always-visible sort control, shown beside the Filter button. */
  sort?: ReactNode;
  /** Always-visible view-mode toggle (e.g. Notes' List/Folders), left of Filter. */
  modes?: ReactNode;
}

const ToolbarContext = createContext<{
  toolbar: ViewToolbar | null;
  setToolbar: (t: ViewToolbar | null) => void;
}>({ toolbar: null, setToolbar: () => {} });

export function ToolbarProvider({ children }: { children: ReactNode }) {
  const [toolbar, setToolbar] = useState<ViewToolbar | null>(null);
  return (
    <ToolbarContext.Provider value={{ toolbar, setToolbar }}>
      {children}
    </ToolbarContext.Provider>
  );
}

/** Read the published toolbar (used by the app shell). */
export function useToolbar() {
  return useContext(ToolbarContext).toolbar;
}

/**
 * Publish this view's toolbar to the app shell. Pass a **memoized** `toolbar`
 * (islands rebuilt only when their inputs change) so this doesn't loop — the
 * effect re-runs only when the content identity actually changes. Clears on
 * unmount so a view without a toolbar shows just the app controls.
 */
export function useViewToolbar(toolbar: ViewToolbar | null) {
  const { setToolbar } = useContext(ToolbarContext);
  useEffect(() => {
    setToolbar(toolbar);
    return () => setToolbar(null);
  }, [toolbar, setToolbar]);
}
