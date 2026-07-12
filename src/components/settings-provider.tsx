import { createContext, useContext, useState } from "react";

/**
 * Settings provider — holds app-wide display preferences, reads their initial
 * values from localStorage, and persists on change. Mirrors the theme-provider
 * pattern (a React context + provider). Feature views read these via
 * `useSettings()` to decide whether to resolve contact names/avatars.
 */
type SettingsProviderState = {
  showContactNames: boolean;
  setShowContactNames: (v: boolean) => void;
  showAvatars: boolean;
  setShowAvatars: (v: boolean) => void;
};

const NAMES_KEY = "salvage-show-names";
const AVATARS_KEY = "salvage-show-avatars";

const SettingsProviderContext = createContext<SettingsProviderState | null>(null);

/** Read a boolean from localStorage, defaulting to true when absent. */
function readBool(key: string): boolean {
  return localStorage.getItem(key) !== "false";
}

export function SettingsProvider({ children }: { children: React.ReactNode }) {
  const [showContactNames, setShowContactNamesState] = useState<boolean>(() =>
    readBool(NAMES_KEY),
  );
  const [showAvatars, setShowAvatarsState] = useState<boolean>(() =>
    readBool(AVATARS_KEY),
  );

  const setShowContactNames = (v: boolean) => {
    localStorage.setItem(NAMES_KEY, String(v));
    setShowContactNamesState(v);
  };

  const setShowAvatars = (v: boolean) => {
    localStorage.setItem(AVATARS_KEY, String(v));
    setShowAvatarsState(v);
  };

  return (
    <SettingsProviderContext.Provider
      value={{ showContactNames, setShowContactNames, showAvatars, setShowAvatars }}
    >
      {children}
    </SettingsProviderContext.Provider>
  );
}

export function useSettings() {
  const ctx = useContext(SettingsProviderContext);
  if (!ctx) throw new Error("useSettings must be used within a SettingsProvider");
  return ctx;
}
