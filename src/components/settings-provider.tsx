import { createContext, useContext, useEffect, useState } from "react";
import type { LightboxStyle } from "@/components/media-lightbox";
import { client, type LogLevel, type LogRecord } from "@/lib/ipc";
import { CLOCK_KEY, readClockFormat, setClockFormat, type ClockFormat } from "@/lib/format";

/** UI density: how tight rows, headers and controls pack together. Drives the
 *  global Tailwind `--spacing` scale via `data-density` on the document root. */
export type Density = "comfortable" | "cozy" | "compact";
/** Ordered least-dense → most-dense; drives the density stepper in the header. */
export const DENSITIES: Density[] = ["comfortable", "cozy", "compact"];

/** How message links preview. Escalating levels of contacting external sites:
 *  `off` (raw URLs, no network), `hover` (fetch on deliberate hover, default),
 *  `inline` (auto-unfurl every visible link in the bubble). */
export type LinkPreviewMode = "off" | "hover" | "inline";
export const LINK_PREVIEW_MODES: LinkPreviewMode[] = ["off", "hover", "inline"];

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
  /** How message links preview (off / on hover / inline). Contacts external
   *  sites for hover & inline. Defaults to hover. */
  linkPreviewMode: LinkPreviewMode;
  setLinkPreviewMode: (v: LinkPreviewMode) => void;
  /** How the image/video lightbox opens: a windowed modal, or fullscreen. */
  lightboxStyle: LightboxStyle;
  setLightboxStyle: (v: LightboxStyle) => void;
  /** Show file/EXIF/location metadata in the lightbox. */
  showMediaMetadata: boolean;
  setShowMediaMetadata: (v: boolean) => void;
  /** Import module ids the user chose, or null to use the catalog defaults. */
  importModules: string[] | null;
  setImportModules: (ids: string[]) => void;
  /** Dev-console log verbosity. */
  logLevel: LogLevel;
  setLogLevel: (level: LogLevel) => void;
  /** Clock format for all timestamps: locale default, or forced 12-/24-hour. */
  clockFormat: ClockFormat;
  setClockFormatPref: (pref: ClockFormat) => void;
  /** Require Touch ID before releasing an encrypted backup's keys (opt-in). */
  biometricUnlock: boolean;
  setBiometricUnlock: (v: boolean) => void;
  /** Whether Touch ID can work — the app is stably signed (see docs/signing.md). */
  biometricAvailable: boolean;
  /** How tightly the UI packs (rows, headers, controls). */
  density: Density;
  setDensity: (d: Density) => void;
};

const NAMES_KEY = "traceloupe-show-names";
const AVATARS_KEY = "traceloupe-show-avatars";
const LINK_PREVIEW_MODE_KEY = "traceloupe-link-preview-mode";
// Legacy per-behaviour toggles, migrated into the mode on first read.
const LEGACY_LINK_PREVIEWS_KEY = "traceloupe-link-previews";
const LEGACY_LINK_PREVIEWS_HOVER_KEY = "traceloupe-link-previews-hover";
const LIGHTBOX_STYLE_KEY = "traceloupe-lightbox-style";
const MEDIA_META_KEY = "traceloupe-media-metadata";
const IMPORT_MODULES_KEY = "traceloupe-import-modules";
const LOG_LEVEL_KEY = "traceloupe-log-level";
const BIOMETRIC_KEY = "traceloupe-biometric-unlock";
const DENSITY_KEY = "traceloupe-density";

/** Read the persisted density, defaulting to "comfortable". */
function readDensity(): Density {
  const raw = localStorage.getItem(DENSITY_KEY);
  return DENSITIES.includes(raw as Density) ? (raw as Density) : "comfortable";
}

/** Read the link-preview mode, defaulting to "hover". Migrates the two legacy
 *  boolean toggles: inline wins if it was on, else off if hover was disabled. */
function readLinkPreviewMode(): LinkPreviewMode {
  const raw = localStorage.getItem(LINK_PREVIEW_MODE_KEY);
  if (LINK_PREVIEW_MODES.includes(raw as LinkPreviewMode)) {
    return raw as LinkPreviewMode;
  }
  if (localStorage.getItem(LEGACY_LINK_PREVIEWS_KEY) === "true") return "inline";
  if (localStorage.getItem(LEGACY_LINK_PREVIEWS_HOVER_KEY) === "false") return "off";
  return "hover";
}

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
  fn(`%c[traceloupe]%c ${r.message}`, "color:#a78bfa;font-weight:600", "color:inherit");
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
  const [linkPreviewMode, setLinkPreviewModeState] = useState<LinkPreviewMode>(
    () => readLinkPreviewMode(),
  );
  const [lightboxStyle, setLightboxStyleState] = useState<LightboxStyle>(() =>
    localStorage.getItem(LIGHTBOX_STYLE_KEY) === "windowed"
      ? "windowed"
      : "fullscreen",
  );
  const [showMediaMetadata, setShowMediaMetadataState] = useState<boolean>(() =>
    readBool(MEDIA_META_KEY),
  );
  const [importModules, setImportModulesState] = useState<string[] | null>(() =>
    readStringArray(IMPORT_MODULES_KEY),
  );
  const [logLevel, setLogLevelState] = useState<LogLevel>(() => readLogLevel());
  const [clockFormat, setClockFormatState] = useState<ClockFormat>(() => readClockFormat());
  // Off by default (opt-in): only true when explicitly stored.
  const [biometricUnlock, setBiometricUnlockState] = useState<boolean>(
    () => localStorage.getItem(BIOMETRIC_KEY) === "true",
  );
  const [biometricAvailable, setBiometricAvailable] = useState<boolean>(false);
  const [signingChecked, setSigningChecked] = useState<boolean>(false);
  const [density, setDensityState] = useState<Density>(() => readDensity());

  // Reflect density onto the document root; a CSS rule keyed off `data-density`
  // scales the global Tailwind `--spacing`, so every spacing utility tightens at
  // once. "comfortable" is the default (no attribute needed) but we always set it
  // so a change back clears any previous value.
  useEffect(() => {
    document.documentElement.dataset.density = density;
  }, [density]);

  // Touch ID only works on a stably-signed build (an adhoc dev binary loses
  // Keychain access on rebuild). Detect it, and: disable the gate when unsigned,
  // or default it ON when signed and the user hasn't chosen — so a signed build
  // gets fingerprint-unlock automatically without a manual toggle. A stored user
  // choice always wins (setBiometricUnlock persists; these paths don't). Fails
  // CLOSED (treated as unsigned/off) so a failed check never leaves the toggle
  // disabled-but-checked while the backend is actually gating.
  useEffect(() => {
    let cancelled = false;
    const apply = (signed: boolean) => {
      if (cancelled) return;
      setBiometricAvailable(signed);
      if (!signed) setBiometricUnlockState(false);
      else if (localStorage.getItem(BIOMETRIC_KEY) === null) setBiometricUnlockState(true);
      setSigningChecked(true);
    };
    client
      .appSigningStatus()
      .then((s) => apply(s.signed))
      .catch(() => apply(false));
    return () => {
      cancelled = true;
    };
  }, []);

  // Push the gate preference to the backend — but only once the signing check has
  // settled the effective value, so we never push a transient/wrong flag at
  // startup. Re-pushed whenever the preference changes.
  useEffect(() => {
    if (!signingChecked) return;
    void client.setBiometricRequired(biometricUnlock).catch(() => {});
  }, [biometricUnlock, signingChecked]);

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
  const setLinkPreviewMode = (v: LinkPreviewMode) => {
    localStorage.setItem(LINK_PREVIEW_MODE_KEY, v);
    setLinkPreviewModeState(v);
  };
  const setLightboxStyle = (v: LightboxStyle) => {
    localStorage.setItem(LIGHTBOX_STYLE_KEY, v);
    setLightboxStyleState(v);
  };
  const setShowMediaMetadata = (v: boolean) => {
    localStorage.setItem(MEDIA_META_KEY, String(v));
    setShowMediaMetadataState(v);
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

  const setBiometricUnlock = (v: boolean) => {
    localStorage.setItem(BIOMETRIC_KEY, String(v));
    setBiometricUnlockState(v); // the effect above pushes it to the backend
  };

  const setDensity = (d: Density) => {
    localStorage.setItem(DENSITY_KEY, d);
    setDensityState(d); // the effect above applies it to the document root
  };

  return (
    <SettingsProviderContext.Provider
      value={{
        showContactNames,
        setShowContactNames,
        showAvatars,
        setShowAvatars,
        linkPreviewMode,
        setLinkPreviewMode,
        lightboxStyle,
        setLightboxStyle,
        showMediaMetadata,
        setShowMediaMetadata,
        importModules,
        setImportModules,
        logLevel,
        setLogLevel,
        clockFormat,
        setClockFormatPref,
        biometricUnlock,
        setBiometricUnlock,
        biometricAvailable,
        density,
        setDensity,
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
