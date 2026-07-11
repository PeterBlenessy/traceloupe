import { createContext, useContext, useEffect, useState } from "react";

/**
 * Theme provider — applies the `light`/`dark` class to <html>, persists the
 * choice, and follows the OS when set to "system". Standard shadcn pattern,
 * adapted for Vite (no next-themes). The token layer in index.css does the
 * actual theming; this only toggles the class.
 */
export type Theme = "dark" | "light" | "system";

type ThemeProviderState = {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  /** The concrete theme in effect (system resolved to light/dark). */
  resolvedTheme: "dark" | "light";
};

const STORAGE_KEY = "salvage-theme";

const ThemeProviderContext = createContext<ThemeProviderState | null>(null);

function systemTheme(): "dark" | "light" {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(
    () => (localStorage.getItem(STORAGE_KEY) as Theme) || "system",
  );
  const [resolved, setResolved] = useState<"dark" | "light">(() =>
    theme === "system" ? systemTheme() : theme,
  );

  useEffect(() => {
    const root = window.document.documentElement;
    const apply = () => {
      const next = theme === "system" ? systemTheme() : theme;
      root.classList.toggle("dark", next === "dark");
      root.classList.toggle("light", next === "light");
      root.dataset.theme = next;
      setResolved(next);
    };
    apply();

    if (theme !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);

  const setTheme = (t: Theme) => {
    localStorage.setItem(STORAGE_KEY, t);
    setThemeState(t);
  };

  return (
    <ThemeProviderContext.Provider value={{ theme, setTheme, resolvedTheme: resolved }}>
      {children}
    </ThemeProviderContext.Provider>
  );
}

export function useTheme() {
  const ctx = useContext(ThemeProviderContext);
  if (!ctx) throw new Error("useTheme must be used within a ThemeProvider");
  return ctx;
}
