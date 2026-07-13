import { createContext, useContext, useEffect, useState } from "react";
import { client, type LogLevel, type LogRecord } from "@/lib/ipc";
import { CLOCK_KEY, readClockFormat, setClockFormat, type ClockFormat } from "@/lib/format";

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
  /** Import module ids the user chose, or null to use the catalog defaults. */
  importModules: string[] | null;
  setImportModules: (ids: string[]) => void;
  /** Dev-console log verbosity. */
  logLevel: LogLevel;
  setLogLevel: (level: LogLevel) => void;
  /** Clock format for all timestamps: locale default, or forced 12-/24-hour. */
  clockFormat: ClockFormat;
  setClockFormatPref: (pref: ClockFormat) => void;
};

const NAMES_KEY = "salvage-show-names";
const AVATARS_KEY = "salvage-show-avatars";
const IMPORT_MODULES_KEY = "salvage-import-modules";
const LOG_LEVEL_KEY = "salvage-log-level";

const LOG_LEVELS: LogLevel[] = ["off", "error", "warn", "info", "debug", "trace"];

/** Read the persisted log level, defaulting to "info". */
function readLogLevel(): LogLevel {
  const raw = localStorage.getItem(LOG_LEVEL_KEY);
  return LOG_LEVELS.includes(raw as LogLevel) ? (raw as LogLevel) : "info";
}

/** Print a backend log record to the dev-tools console. */
function printLog(r: LogRecord) {
  const fn =
    r.level === "error"
      ? console.error
      : r.level === "warn"
        ? console.warn
        : r.level === "debug" || r.level === "trace"
          ? console.debug
          : console.info;
  fn(`%c[salvage]%c ${r.message}`, "color:#a78bfa;font-weight:600", "color:inherit");
}

const SettingsProviderContext = createContext<SettingsProviderState | null>(null);

/** Read a boolean from localStorage, defaulting to true when absent. */
function readBool(key: string): boolean {
  return localStorage.getItem(key) !== "false";
}

/** Read a persisted string array, or null when absent/invalid. */
function readStringArray(key: string): string[] | null {
  const raw = localStorage.getItem(key);
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    return Array.isArray(v) ? (v as string[]) : null;
  } catch {
    return null;
  }
}

export function SettingsProvider({ children }: { children: React.ReactNode }) {
  const [showContactNames, setShowContactNamesState] = useState<boolean>(() =>
    readBool(NAMES_KEY),
  );
  const [showAvatars, setShowAvatarsState] = useState<boolean>(() =>
    readBool(AVATARS_KEY),
  );
  const [importModules, setImportModulesState] = useState<string[] | null>(() =>
    readStringArray(IMPORT_MODULES_KEY),
  );
  const [logLevel, setLogLevelState] = useState<LogLevel>(() => readLogLevel());
  const [clockFormat, setClockFormatState] = useState<ClockFormat>(() => readClockFormat());

  // Apply the log level to the backend (it gates emission), and forward backend
  // log records to the dev-tools console. The level is re-applied whenever it
  // changes; the console subscription lasts the app's lifetime.
  useEffect(() => {
    void client.setLogLevel(logLevel);
  }, [logLevel]);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void client.onLog(printLog).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const setLogLevel = (level: LogLevel) => {
    localStorage.setItem(LOG_LEVEL_KEY, level);
    setLogLevelState(level);
  };

  const setShowContactNames = (v: boolean) => {
    localStorage.setItem(NAMES_KEY, String(v));
    setShowContactNamesState(v);
  };

  const setShowAvatars = (v: boolean) => {
    localStorage.setItem(AVATARS_KEY, String(v));
    setShowAvatarsState(v);
  };

  const setImportModules = (ids: string[]) => {
    localStorage.setItem(IMPORT_MODULES_KEY, JSON.stringify(ids));
    setImportModulesState(ids);
  };

  const setClockFormatPref = (pref: ClockFormat) => {
    localStorage.setItem(CLOCK_KEY, pref);
    setClockFormat(pref); // rebuild the shared Intl formatters
    setClockFormatState(pref); // re-render consumers
  };

  return (
    <SettingsProviderContext.Provider
      value={{
        showContactNames,
        setShowContactNames,
        showAvatars,
        setShowAvatars,
        importModules,
        setImportModules,
        logLevel,
        setLogLevel,
        clockFormat,
        setClockFormatPref,
      }}
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
