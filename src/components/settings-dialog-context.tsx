/**
 * Deep-linking for the Settings dialog: any view can open it on a specific tab
 * ("Download the model from Settings → Safety" jumps straight there) instead of
 * telling the user where to click. The dialog itself stays in app-shell's
 * SettingsMenu; this context just lifts its open/tab state so other components
 * can drive it.
 */
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/** Tab values of the Settings dialog's nav (see SettingsMenu in app-shell). */
export type SettingsTab =
  | "general"
  | "media"
  | "apps"
  | "security"
  | "safety"
  | "developer";

interface SettingsDialogState {
  open: boolean;
  setOpen: (open: boolean) => void;
  tab: SettingsTab;
  setTab: (tab: SettingsTab) => void;
  /** Open the dialog, optionally jumping to a tab. */
  openSettings: (tab?: SettingsTab) => void;
}

const SettingsDialogContext = createContext<SettingsDialogState | null>(null);

export function SettingsDialogProvider({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<SettingsTab>("general");

  const openSettings = useCallback((t?: SettingsTab) => {
    if (t) setTab(t);
    setOpen(true);
  }, []);

  const value = useMemo(
    () => ({ open, setOpen, tab, setTab, openSettings }),
    [open, tab, openSettings],
  );

  return (
    <SettingsDialogContext.Provider value={value}>
      {children}
    </SettingsDialogContext.Provider>
  );
}

export function useSettingsDialog(): SettingsDialogState {
  const ctx = useContext(SettingsDialogContext);
  if (!ctx)
    throw new Error("useSettingsDialog must be used within SettingsDialogProvider");
  return ctx;
}

/**
 * An inline "Settings → X" reference that actually goes there. A button (it
 * performs an in-app action, not navigation) styled as running text, so a
 * sentence like "Download it once from Settings → Safety" stays a sentence.
 */
export function SettingsLink({
  tab,
  children,
}: {
  tab: SettingsTab;
  children: ReactNode;
}) {
  const { openSettings } = useSettingsDialog();
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={() => openSettings(tab)}
          className="rounded-sm font-medium text-[var(--accent-text)] underline underline-offset-2 decoration-1 outline-hidden hover:opacity-80 focus-visible:ring-2 focus-visible:ring-ring"
        >
          {children}
        </button>
      </TooltipTrigger>
      <TooltipContent>Open Settings</TooltipContent>
    </Tooltip>
  );
}
