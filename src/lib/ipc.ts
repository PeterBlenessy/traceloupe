/**
 * Typed client for the Tauri command layer.
 *
 * Two implementations of the same interface: the real one over
 * `invoke()`, and a mock used when the app runs in a plain browser
 * (Vite dev server, Playwright). Views depend only on `TraceLoupeClient`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

export interface BackupInfo {
  id: string;
  path: string;
  deviceName: string | null;
  productType: string | null;
  productVersion: string | null;
  serialNumber: string | null;
  /** Unix epoch seconds. */
  lastBackupDate: number | null;
  isEncrypted: boolean | null;
}

export type DiscoveryResult =
  | { status: "ok"; backups: BackupInfo[] }
  | { status: "permissionDenied"; path: string }
  | { status: "notFound"; path: string };

export type ImportProgress =
  | {
      phase: "parsing";
      current: number;
      total: number;
      fraction: number;
      artifact: string;
    }
  | { phase: "indexing"; step: string; index: number; total: number };

/** Dev-console log verbosity, mirrored in the Rust `set_log_level` command. */
export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

/** A log record forwarded from the Rust backend to the dev-tools console. */
export interface LogRecord {
  level: Exclude<LogLevel, "off">;
  message: string;
}

/** The app's macOS code-signing status (gates Touch ID / stable Keychain). */
export interface SigningStatus {
  /** Stably signed with a real identity — Keychain persists, Touch ID can work. */
  signed: boolean;
  /** Ad-hoc signed (the dev default) — Keychain access is lost on rebuild. */
  adhoc: boolean;
  /** The signing authority, when signed. */
  identity: string | null;
}

/** A selectable data type for import (maps to iLEAPP modules behind the scenes). */
export interface ImportModule {
  id: string;
  label: string;
  category: string;
  default: boolean;
}

export interface ImportResult {
  cachePath: string;
  threads: number;
  messages: number;
  mediaItems: number;
  calls: number;
  safariVisits: number;
  contacts: number;
  warnings: string[];
}

export interface Call {
  id: number;
  address: string | null;
  direction: string | null;
  answered: boolean | null;
  durationS: number | null;
  occurredAt: number | null;
  service: string | null;
  /** FaceTime medium: "audio" | "video"; null for phone calls. */
  callType: string | null;
  /** Carrier/geo location shown beside the call, if any. */
  location: string | null;
  /** The number's ISO country code (lowercase alpha-2, e.g. "se"), or null. */
  countryCode: string | null;
}

export interface HistoryVisit {
  id: number;
  url: string;
  title: string | null;
  visitedAt: number | null;
  visitCount: number | null;
  /** URL recorded as deleted from history (a tombstone), not a live visit. */
  deleted: boolean;
}

/** A Safari bookmark, reading-list item, or open tab (`kind` selects which). */
export interface SafariBookmark {
  id: number;
  kind: "bookmark" | "reading_list" | "tab";
  title: string | null;
  url: string | null;
  folder: string | null;
  dateAdded: number | null;
  dateViewed: number | null;
  previewText: string | null;
  /** An open tab from a private-browsing window; false for bookmarks/reading-list. */
  private: boolean;
}

export interface Note {
  id: number;
  folder: string | null;
  title: string | null;
  snippet: string | null;
  /** Plain-text body. `null` for a locked note until unlocked with the password. */
  body: string | null;
  /** Rich HTML rendering of the body (headings/lists/checklists); null → use `body`. */
  bodyRich: string | null;
  createdAt: number | null;
  modifiedAt: number | null;
  /** Pinned to the top of the Notes app. */
  pinned: boolean;
  /** Password-protected: the body is withheld until unlocked. */
  locked: boolean;
  /** The user's password hint, if the note stored one. */
  passwordHint: string | null;
  /** Rich-content indicators: a checklist, and embedded image/attachment counts. */
  hasChecklist: boolean;
  /** Image attachments the note *references* (may be iCloud-only, not in the backup). */
  imageCount: number;
  /** Image attachments actually present in the backup (displayable). `<= imageCount`. */
  availableImageCount: number;
  attachmentCount: number;
  /** Hashtag tags on the note (iOS 15+); empty when none. */
  tags: string[];
  /** Whether the note has a first image (served as a list thumbnail). */
  hasImage: boolean;
}

/** An installed app with the App Store metadata the backup carries. */
export interface InstalledApp {
  bundleId: string;
  name: string | null;
  seller: string | null;
  version: string | null;
  genre: string | null;
  /** App Store release date (RFC-3339 string); format with `new Date(...)`. */
  released: string | null;
}

export interface Recording {
  id: number;
  title: string | null;
  folder: string | null;
  recordedAt: number | null;
  durationS: number | null;
  /** Trailing filename of the `.m4a`, for labeling an untitled memo. */
  fileName: string | null;
}

export interface CalendarEvent {
  id: number;
  title: string | null;
  notes: string | null;
  location: string | null;
  startAt: number | null;
  endAt: number | null;
  allDay: boolean;
  calendarName: string | null;
  url: string | null;
  /** "busy" | "free" | "tentative" | "unavailable" | null. */
  availability: string | null;
  recurring: boolean;
}

export interface Interaction {
  id: number;
  displayName: string | null;
  identifier: string | null;
  incoming: number;
  outgoing: number;
  incomingRecipient: number;
  firstAt: number | null;
  lastAt: number | null;
}

/** A per-app communication channel (which app CoreDuet interactions flowed through). */
export interface InteractionChannel {
  bundleId: string;
  incoming: number;
  outgoing: number;
}

export interface Workout {
  id: number;
  activity: string | null;
  startAt: number | null;
  endAt: number | null;
  durationS: number | null;
  distanceM: number | null;
  /** A GPS route was recorded for this workout. */
  hasRoute: boolean;
}

/** One point of a workout's (downsampled) GPS route. */
export interface RoutePoint {
  at: number | null;
  latitude: number;
  longitude: number;
  altitude: number | null;
}

export interface HealthSummary {
  sampleCount: number;
  firstAt: number | null;
  lastAt: number | null;
  workoutCount: number;
  /** Days with activity aggregates / sleep sessions / recorded timezones /
   *  earned achievements (section counts). */
  dayCount: number;
  sleepCount: number;
  timezoneCount: number;
  achievementCount: number;
  cycleCount: number;
}

/** One Cycle Tracking entry (a reproductive-health / symptom category sample). */
export interface CycleEntry {
  id: number;
  category: string;
  /** Decoded value (e.g. menstrual-flow "Medium"), or null. */
  detail: string | null;
  loggedAt: number | null;
}

/** One earned Apple Fitness achievement. */
export interface HealthAchievement {
  id: number;
  /** Template id, e.g. "MoveGoal200Percent" (humanized in the UI). */
  name: string;
  /** Midnight UTC of the earned day, unix seconds. */
  earnedAt: number | null;
  value: number | null;
  unit: string | null;
}

/** One timezone Health samples were recorded in — a travel-timeline entry. */
export interface HealthTimezone {
  /** IANA name, e.g. "Europe/Stockholm". */
  tzName: string;
  /** Device product types that recorded there (e.g. "iPhone12,8"). */
  devices: string[];
  samples: number;
  firstAt: number | null;
  lastAt: number | null;
}

/** One sleep-analysis session (a raw HealthKit category sample). */
export interface SleepSession {
  id: number;
  startAt: number | null;
  endAt: number | null;
  stage: string;
}

/** One day of Health activity (aggregated per UTC day at import). */
export interface HealthDay {
  dayAt: number;
  steps: number | null;
  distanceM: number | null;
  flights: number | null;
  activeKcal: number | null;
  restingKcal: number | null;
  hrMin: number | null;
  hrAvg: number | null;
  hrMax: number | null;
  /** Headphone audio exposure, loudest sample of the day (dB). */
  audioDbMax: number | null;
  /** Walking/mobility daily averages. */
  walkSpeedMs: number | null;
  stepLengthM: number | null;
  doubleSupportPct: number | null;
  walkAsymmetryPct: number | null;
  /** Activity rings (null when the device never tracked that ring). */
  moveKcal: number | null;
  moveGoalKcal: number | null;
  exerciseMin: number | null;
  exerciseGoalMin: number | null;
  standHours: number | null;
  standGoalHours: number | null;
}

export interface Reminder {
  id: number;
  title: string | null;
  notes: string | null;
  listName: string | null;
  dueAt: number | null;
  completed: boolean;
  completedAt: number | null;
  flagged: boolean;
  priority: number | null;
  createdAt: number | null;
}

/** Counts refreshed by a partial re-import (only the relevant field is set). */
export interface ReimportResult {
  module: string;
  recordings: number;
  mediaItems: number;
  messages: number;
  threads: number;
  notes: number;
  calls: number;
  safariVisits: number;
  warnings: string[];
}

export interface LabeledValue {
  label: string | null;
  value: string;
}

export interface Contact {
  id: number;
  firstName: string | null;
  lastName: string | null;
  middleName: string | null;
  nickname: string | null;
  organization: string | null;
  jobTitle: string | null;
  department: string | null;
  /** Birthday as a Unix timestamp, or null. */
  birthdayAt: number | null;
  note: string | null;
  phones: LabeledValue[];
  emails: LabeledValue[];
  /** Postal addresses, each formatted to one line with its label. */
  addresses: LabeledValue[];
  /** Related people: label = relationship (Mother / custom), value = name. */
  related: LabeledValue[];
  /** Names of the address-book groups this contact belongs to. */
  groups: string[];
  /** Social / IM profiles: label = service (Snapchat/…), value = username. */
  social: LabeledValue[];
  /** Whether a contact photo is stored (load it via `contactAvatarUrl`). */
  hasImage: boolean;
  /** 'Address Book' or a third-party app (e.g. 'TikTok'); drives the filter. */
  source: string;
}

export interface MediaItem {
  id: number;
  kind: string;
  source: string | null;
  mimeType: string | null;
  filename: string | null;
  takenAt: number | null;
  /** Comma-separated names of people detected in the photo, or null. */
  persons: string | null;
  latitude: number | null;
  longitude: number | null;
  favorite: boolean;
  /** Moment place/event name (e.g. "Florida"), or null. */
  location: string | null;
  /** User album names this photo is in, comma-separated, or null. */
  albums: string | null;
  /** Pixel dimensions and (video) duration. */
  width: number | null;
  height: number | null;
  durationS: number | null;
  /** Original file size in bytes. */
  fileSize: number | null;
  /** Camera "<make> <model>", lens model, and a formatted EXIF exposure summary. */
  camera: string | null;
  lens: string | null;
  exif: string | null;
  /** In the device's Hidden album. */
  hidden: boolean;
  /** In Recently Deleted, with the deletion time (Unix seconds) when known. */
  trashed: boolean;
  trashedAt: number | null;
  /** When the asset was added to the library (Unix seconds), which differs from
   *  capture for received/saved/imported media, or null. */
  addedAt: number | null;
  /** Media subtype ("screenshot" | "panorama" | "live" | "burst"), or null. */
  subtype: string | null;
}

/** A media source and how many items came from it, for the gallery filter. */
export type MediaSource = [source: string, count: number];

export interface ThreadSummary {
  id: number;
  identifier: string;
  displayName: string | null;
  service: string | null;
  lastMessageAt: number | null;
  messageCount: number;
  snippet: string | null;
  /** Member handles for a group chat (empty or one for a 1:1). */
  participants: string[];
}

/** OpenGraph link preview (all fields best-effort). */
export interface LinkPreview {
  url: string;
  title: string | null;
  description: string | null;
  image: string | null;
  siteName: string | null;
}

export interface Attachment {
  id: number;
  filename: string | null;
  mimeType: string | null;
  localPath: string | null;
}

/** A camera-roll item matched (by filename) to a missing message attachment. */
export interface RecoveredMedia {
  id: number;
  kind: string;
}

export interface Message {
  id: number;
  isFromMe: boolean;
  sender: string | null;
  body: string | null;
  sentAt: number | null;
  /** iMessage receipts (Unix): when read / delivered, if known. */
  readAt: number | null;
  deliveredAt: number | null;
  /** Tapback summary folded onto this message, e.g. "❤️×2 👍", or null. */
  reactions: string | null;
  /** Preview of the message this one replies to, or null. */
  replyToSnippet: string | null;
  /** The message was edited (iOS 16+). */
  edited: boolean;
  /** Content class; "system" marks a group-action row (rename/add/remove/leave)
   *  rendered as a centered note rather than a chat bubble. */
  kind?: string | null;
  /** Expressive send effect (e.g. "Confetti", "Slam"), or null. */
  effect?: string | null;
  /** Recovered from the recoverable-message store: deleted but still on-device,
   *  with the deletion time (Unix) when known. */
  deleted?: boolean;
  deletedAt?: number | null;
  attachments: Attachment[];
}

/** A message in the cross-conversation timeline, tagged with its thread. */
export interface TimelineMessage {
  threadId: number;
  threadTitle: string;
  /** The thread's identifier — for a 1:1 chat, the other party's handle. Lets
   * the timeline show the conversation partner even on your outgoing messages. */
  threadHandle: string;
  service: string | null;
  message: Message;
}

/** A half-open time window [lo, hi) in epoch seconds; either bound may be null. */
export interface TimeRange {
  lo: number | null;
  hi: number | null;
}

export interface EngineInfo {
  /** An engine is resolvable right now (imports will work). */
  installed: boolean;
  /** Pinned engine version, e.g. "iLEAPP v2026.1.0". */
  version: string;
  /** A downloadable build has been published (the download flow is live). */
  canDownload: boolean;
}

export type EngineProgress =
  | { phase: "downloading"; received: number; total: number; fraction: number }
  | { phase: "verifying" }
  | { phase: "done" };

// --- Security Check (spyware/stalkerware indicator scan) -------------------

export type ScanKind = "explicit" | "passive";
export type Severity = "critical" | "warning" | "info";

/** Progress event during a scan or an indicator update. */
export interface ScanProgress {
  module: string;
  index: number;
  total: number;
}

export interface ScanSummary {
  runId: number;
  findings: number;
  cancelled: boolean;
}

export interface ScanRun {
  id: number;
  kind: ScanKind;
  startedAt: number;
  finishedAt: number | null;
  status: "running" | "done" | "cancelled" | "failed";
  modules: string[];
  indicatorCount: number | null;
  critical: number;
  warning: number;
  info: number;
}

export interface Finding {
  id: number;
  runId: number;
  severity: Severity;
  kind: string;
  module: string;
  malware: string;
  matchedValue: string;
  context: string | null;
  refKind: string | null;
  refId: number | null;
  eventTime: number | null;
  /** New since the previous completed scan of this backup (false on first scan). */
  isNew: boolean;
}

export interface FeedInfo {
  source: string;
  class: string;
  count: number;
  skipped: number;
}

export interface SnapshotInfo {
  generatedAt: string;
  feeds: FeedInfo[];
}

export type PassiveScope = "apps_only" | "full";
export type Consent = "unasked" | "granted" | "denied";

export interface DetectionSettings {
  passiveEnabled: boolean;
  passiveScope: PassiveScope;
  passiveConsent: Consent;
  autoUpdateIndicators: boolean;
  fetchConsent: Consent;
  expandShortUrls: boolean;
  /** Optional local folder of custom indicator files merged into scans. */
  customIndicatorDir: string | null;
}

export interface TraceLoupeClient {
  listBackups(root?: string): Promise<DiscoveryResult>;
  /** The default Finder/MobileSync backup folder, for seeding the picker. */
  defaultBackupRoot(): Promise<string | null>;
  /**
   * Open a native folder picker (defaulting to the MobileSync backup folder)
   * and return the chosen path, or null if cancelled. Selecting a folder grants
   * macOS access to it, sidestepping Full Disk Access.
   */
  pickBackupFolder(): Promise<string | null>;
  /** Open a native folder picker; returns the chosen path, or null if cancelled.
   *  Used for the custom indicator folder. */
  pickFolder(title?: string): Promise<string | null>;
  /** Open System Settings at the Full Disk Access pane. */
  openFullDiskAccessSettings(): Promise<void>;
  /** Open a URL in the user's default browser (e.g. an Apple Maps link). */
  openExternal(url: string): Promise<void>;
  /** Fetch a URL's OpenGraph metadata for a link preview. Opt-in — this makes an
   *  outbound request to the linked site; only call it when the setting is on. */
  fetchLinkPreview(url: string): Promise<LinkPreview>;
  engineStatus(): Promise<boolean>;
  engineInfo(): Promise<EngineInfo>;
  /** Download + verify + install the pinned engine. */
  installEngine(): Promise<void>;
  /** Subscribe to engine-install progress. Returns an unsubscribe fn. */
  onEngineProgress(cb: (p: EngineProgress) => void): Promise<UnlistenFn>;
  /** The catalog of importable data types the user can enable/disable. */
  listImportModules(): Promise<ImportModule[]>;
  importBackup(args: {
    backupPath: string;
    backupId: string;
    password: string;
    /** Module ids to import (empty = all defaults). */
    modules: string[];
  }): Promise<ImportResult>;
  /** Subscribe to import progress events. Returns an unsubscribe fn. */
  onImportProgress(cb: (p: ImportProgress) => void): Promise<UnlistenFn>;
  /** Stop the in-flight import (kills the iLEAPP subprocess). */
  cancelImport(): Promise<void>;
  /** Set the dev-console log verbosity at runtime. */
  setLogLevel(level: LogLevel): Promise<void>;
  /**
   * Enable/disable the Touch ID gate for releasing an encrypted backup's keys.
   * When on, reconstructing the decryptor prompts for Touch ID first.
   */
  setBiometricRequired(enabled: boolean): Promise<void>;
  /** The app's code-signing status — whether Touch ID / stable Keychain can work. */
  appSigningStatus(): Promise<SigningStatus>;
  /** Subscribe to backend log records (forwarded to the console). */
  onLog(cb: (r: LogRecord) => void): Promise<UnlistenFn>;
  hasActiveBackup(): Promise<boolean>;
  /** Close the open backup (clears session state; the on-disk cache remains). */
  closeBackup(): Promise<void>;
  openBackup(backupId: string): Promise<boolean>;
  /** Delete an imported backup's caches + stored password (not the original). */
  forgetBackup(backupId: string): Promise<void>;
  /** Ids of backups already parsed (open instantly, no first-time read). */
  importedBackupIds(): Promise<string[]>;
  listThreads(): Promise<ThreadSummary[]>;
  /** Device + backup metadata for the active backup, or null if unknown. */
  deviceInfo(): Promise<BackupInfo | null>;
  listCalendarEvents(): Promise<CalendarEvent[]>;
  listReminders(): Promise<Reminder[]>;
  listWorkouts(): Promise<Workout[]>;
  /** The GPS route of one workout, in recording order (empty if none). */
  workoutRoute(workoutId: number): Promise<RoutePoint[]>;
  /** Daily activity aggregates, most recent day first. */
  healthDaily(): Promise<HealthDay[]>;
  /** Sleep-analysis sessions, most recent first. */
  listSleep(): Promise<SleepSession[]>;
  /** Timezones Health data was recorded in, most samples first. */
  listHealthTimezones(): Promise<HealthTimezone[]>;
  /** Earned Apple Fitness achievements, most recent first. */
  listHealthAchievements(): Promise<HealthAchievement[]>;
  /** Cycle Tracking entries (flow + symptoms), most recent first. */
  listCycle(): Promise<CycleEntry[]>;
  healthSummary(): Promise<HealthSummary>;
  listInteractions(): Promise<Interaction[]>;
  /** Per-app interaction channels (which apps comms flowed through), busiest first. */
  interactionChannels(): Promise<InteractionChannel[]>;
  /** Distinct content kinds present (with counts), for the content-filter pills.
   * `threadId` scopes to one conversation; otherwise all messages in `service`. */
  messageKinds(
    threadId?: number | null,
    service?: string | null,
  ): Promise<[kind: string, count: number][]>;
  /** Total messages in a thread; drives the lazily-loaded virtual scroller.
   * `kind` filters by content class (null=all). */
  countThreadMessages(
    threadId: number,
    kind?: string | null,
  ): Promise<number>;
  /** A window of a thread's messages from `offset`; `desc` newest-first. */
  getThreadMessageWindow(
    threadId: number,
    offset: number,
    limit: number,
    desc?: boolean,
    kind?: string | null,
  ): Promise<Message[]>;
  /** The 0-based row index of a message within its thread under the given order
   *  and `kind` filter, or null if absent. Used to scroll to a message. */
  threadMessageIndex(
    threadId: number,
    messageId: number,
    kind?: string | null,
    desc?: boolean,
  ): Promise<number | null>;
  /** A same-named camera-roll item for a missing message attachment (best-effort;
   *  null if none). Lets an offloaded attachment show from Photos. */
  recoverAttachmentMedia(attachmentId: number): Promise<RecoveredMedia | null>;
  /** Total messages across all conversations (filtered by `service`, null=all);
   * drives the timeline scroller. `kind` filters by content class. */
  countTimelineMessages(
    service?: string | null,
    search?: string | null,
    kind?: string | null,
  ): Promise<number>;
  /** A window of the all-conversations timeline from `offset`; `desc` newest-first.
   * `search` matches message body / sender / conversation; `kind` filters class. */
  getTimelineWindow(
    offset: number,
    limit: number,
    service?: string | null,
    search?: string | null,
    desc?: boolean,
    kind?: string | null,
  ): Promise<TimelineMessage[]>;
  /** Message counts for each half-open [lo, hi) epoch-second window. */
  countMessageRanges(
    ranges: TimeRange[],
    service?: string | null,
    search?: string | null,
    kind?: string | null,
  ): Promise<number[]>;
  /** The earliest and latest dated message (Unix seconds), or null if none. */
  messageDateBounds(): Promise<[number, number] | null>;
  /** A window of messages whose time falls in [lo, hi); `desc` newest-first. */
  getRangeWindow(
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
    service?: string | null,
    search?: string | null,
    desc?: boolean,
    kind?: string | null,
  ): Promise<TimelineMessage[]>;
  listCalls(): Promise<Call[]>;
  listSafariHistory(): Promise<HistoryVisit[]>;
  listNotes(): Promise<Note[]>;
  /** Decrypt a locked note's body with the note password. Rejects on wrong password. */
  unlockNote(noteId: number, password: string): Promise<string>;
  listRecordings(): Promise<Recording[]>;
  listContacts(): Promise<Contact[]>;
  /** Apps installed on the device, with their App Store metadata. */
  listInstalledApps(): Promise<InstalledApp[]>;

  // --- Security Check ---
  /** Run a scan over the active backup. "explicit" runs all modules (and may
   *  fetch fresh feeds); "passive" is apps-only by default. */
  runSecurityScan(kind: ScanKind): Promise<ScanSummary>;
  /** Cancel a scan in flight (no-op if none running). */
  cancelScan(): Promise<void>;
  /** Subscribe to scan / indicator-update progress. Returns an unsubscribe fn. */
  onScanProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn>;
  /** Past scan runs for the active backup, newest first. */
  listScanRuns(): Promise<ScanRun[]>;
  /** The most recent completed run's id, or null. */
  latestScanRun(): Promise<number | null>;
  /** Findings for a run, most severe first. */
  listFindings(
    runId: number,
    minSeverity?: Severity | null,
    module?: string | null,
  ): Promise<Finding[]>;
  /** Info about the active indicator snapshot (feed counts + freshness). */
  getIndicatorInfo(): Promise<SnapshotInfo>;
  /** Fetch fresh indicator feeds now. Makes an outbound request to the public
   *  feed repos; sends nothing about the user or their backup (ADR 0001). */
  updateIndicators(): Promise<SnapshotInfo>;
  getDetectionSettings(): Promise<DetectionSettings>;
  setDetectionSettings(settings: DetectionSettings): Promise<void>;
  /** Find known URL-shortener links in text (local, no network). */
  findShortenerUrls(text: string): Promise<string[]>;
  /** Resolve a shortened link to its destination. Contacts the link's shortener
   *  (the sole sanctioned backup-data exit, ADR 0001) — call only after explicit
   *  user approval. Reveals the target without visiting it. */
  expandShortUrl(url: string): Promise<string>;
  /** Whether the per-use de-shorten approval prompt is suppressed for THIS
   *  backup (never global). */
  deshortenAutoApproveGet(): Promise<boolean>;
  deshortenAutoApproveSet(enabled: boolean): Promise<void>;
  /** Run the Passive Check now against the already-imported backup (the
   *  first-launch consent flow). Returns null if consent isn't granted or no
   *  backup is open. */
  runPassiveCheckNow(): Promise<ScanSummary | null>;
  /** Open a save dialog and write a CSV report of the run. Returns the path
   *  written, or null if the user cancelled. */
  exportScanReport(runId: number): Promise<string | null>;
  listMedia(): Promise<MediaItem[]>;
  mediaSources(): Promise<MediaSource[]>;
  // Windowed/filterable list queries (null filter = all), for lazy-loading
  // huge lists a slice at a time.
  countMedia(
    source: string | null,
    lo?: number | null,
    hi?: number | null,
    search?: string | null,
  ): Promise<number>;
  /** Media counts for each [lo, hi) window in `source` — Photos time-filter chips. */
  countMediaRanges(
    source: string | null,
    ranges: TimeRange[],
    search?: string | null,
  ): Promise<number[]>;
  getMediaWindow(
    source: string | null,
    lo: number | null,
    hi: number | null,
    search: string | null,
    offset: number,
    limit: number,
    sortBy: string,
    desc: boolean,
  ): Promise<MediaItem[]>;
  countCalls(
    search: string | null,
    lo?: number | null,
    hi?: number | null,
  ): Promise<number>;
  /** Call counts for each [lo, hi) window (respecting `search`). */
  countCallRanges(
    ranges: TimeRange[],
    search?: string | null,
  ): Promise<number[]>;
  getCallsWindow(
    search: string | null,
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
    sortBy: string,
    desc: boolean,
  ): Promise<Call[]>;
  countSafari(
    search: string | null,
    lo?: number | null,
    hi?: number | null,
  ): Promise<number>;
  /** Safari-visit counts for each [lo, hi) window (respecting `search`). */
  countSafariRanges(
    search: string | null,
    ranges: TimeRange[],
  ): Promise<number[]>;
  getSafariWindow(
    search: string | null,
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
    sortBy: string,
    desc: boolean,
  ): Promise<HistoryVisit[]>;
  /** Count of one Safari `kind` (bookmark/reading_list/tab) matching search+range. */
  countSafariBookmarks(
    kind: string,
    search: string | null,
    lo?: number | null,
    hi?: number | null,
  ): Promise<number>;
  countSafariBookmarkRanges(
    kind: string,
    search: string | null,
    ranges: TimeRange[],
  ): Promise<number[]>;
  getSafariBookmarksWindow(
    kind: string,
    search: string | null,
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
    sortBy: string,
    desc: boolean,
  ): Promise<SafariBookmark[]>;
  /** URL the webview can load for a media item. `thumb` requests a thumbnail;
   *  `cacheKey` (see `useMediaCacheKey`) makes each mount request a fresh URL to
   *  dodge WebKit's cached-failed-task quirk on remount. */
  mediaUrl(id: number, opts?: { thumb?: boolean; cacheKey?: number }): string;
  /** URL the webview can load for a contact's photo. */
  contactAvatarUrl(id: number): string;
  /** URL for a message attachment's bytes (`thumb` for an image thumbnail;
   *  `cacheKey` as in `mediaUrl`). */
  attachmentUrl(id: number, opts?: { thumb?: boolean; cacheKey?: number }): string;
  /** URL the webview can load for a voice recording's audio bytes. */
  audioUrl(id: number): string;
  /** URL for a note's first-image thumbnail (see `Note.hasImage`). */
  noteImageUrl(id: number, index?: number): string;
  /** Open an attachment's file with the OS default app (documents, etc.). */
  openAttachment(id: number): Promise<void>;
  /**
   * Re-import one natively-parsed data type into the open backup, replacing just
   * that type's rows (no iLEAPP). `moduleId` is one of "recordings",
   * "camera_roll", "messages", "notes", "calls", "safari".
   */
  reimportModule(moduleId: string): Promise<ReimportResult>;
}

/** Build the `?thumb=1&k=…` query suffix shared by media/attachment URLs. */
function mediaQuery(opts?: { thumb?: boolean; cacheKey?: number }): string {
  const parts: string[] = [];
  if (opts?.thumb) parts.push("thumb=1");
  if (opts?.cacheKey != null) parts.push(`k=${opts.cacheKey}`);
  return parts.length ? `?${parts.join("&")}` : "";
}

const tauriClient: TraceLoupeClient = {
  listBackups: (root) => invoke<DiscoveryResult>("list_backups", { root }),
  defaultBackupRoot: () => invoke<string | null>("default_backup_root"),
  pickBackupFolder: async () => {
    const defaultPath =
      (await invoke<string | null>("default_backup_root")) ?? undefined;
    const chosen = await open({
      directory: true,
      multiple: false,
      title: "Choose an iPhone backup folder",
      defaultPath,
    });
    return typeof chosen === "string" ? chosen : null;
  },
  pickFolder: async (title) => {
    const chosen = await open({
      directory: true,
      multiple: false,
      title: title ?? "Choose a folder",
    });
    return typeof chosen === "string" ? chosen : null;
  },
  openFullDiskAccessSettings: () =>
    invoke<void>("open_full_disk_access_settings"),
  openExternal: (url) => openUrl(url),
  fetchLinkPreview: (url) => invoke<LinkPreview>("fetch_link_preview", { url }),
  engineStatus: () => invoke<boolean>("engine_status"),
  engineInfo: () => invoke<EngineInfo>("engine_info"),
  installEngine: () => invoke<void>("install_engine"),
  onEngineProgress: (cb) =>
    listen<EngineProgress>("engine://progress", (e) => cb(e.payload)),
  listImportModules: () => invoke<ImportModule[]>("list_import_modules"),
  importBackup: (args) => invoke<ImportResult>("import_backup", args),
  onImportProgress: (cb) =>
    listen<ImportProgress>("import://progress", (e) => cb(e.payload)),
  cancelImport: () => invoke("cancel_import"),
  setLogLevel: (level) => invoke("set_log_level", { level }),
  setBiometricRequired: (enabled) =>
    invoke("set_biometric_required", { enabled }),
  appSigningStatus: () => invoke<SigningStatus>("app_signing_status"),
  onLog: (cb) => listen<LogRecord>("app://log", (e) => cb(e.payload)),
  hasActiveBackup: () => invoke<boolean>("has_active_backup"),
  closeBackup: () => invoke<void>("close_backup"),
  openBackup: (backupId) => invoke<boolean>("open_backup", { backupId }),
  forgetBackup: (backupId) => invoke<void>("forget_backup", { backupId }),
  importedBackupIds: () => invoke<string[]>("imported_backup_ids"),
  listThreads: () => invoke<ThreadSummary[]>("list_threads"),
  deviceInfo: () => invoke<BackupInfo | null>("device_info"),
  listCalendarEvents: () => invoke<CalendarEvent[]>("list_calendar_events"),
  listReminders: () => invoke<Reminder[]>("list_reminders"),
  listWorkouts: () => invoke<Workout[]>("list_workouts"),
  workoutRoute: (workoutId) => invoke<RoutePoint[]>("workout_route", { workoutId }),
  healthDaily: () => invoke<HealthDay[]>("health_daily"),
  listSleep: () => invoke<SleepSession[]>("list_sleep"),
  listHealthTimezones: () => invoke<HealthTimezone[]>("list_health_timezones"),
  listHealthAchievements: () =>
    invoke<HealthAchievement[]>("list_health_achievements"),
  listCycle: () => invoke<CycleEntry[]>("list_cycle"),
  healthSummary: () => invoke<HealthSummary>("health_summary"),
  listInteractions: () => invoke<Interaction[]>("list_interactions"),
  interactionChannels: () =>
    invoke<InteractionChannel[]>("interaction_channels"),
  messageKinds: (threadId = null, service = null) =>
    invoke<[string, number][]>("message_kinds", {
      threadId: threadId ?? null,
      service: service ?? null,
    }),
  countThreadMessages: (threadId, kind = null) =>
    invoke<number>("count_thread_messages", { threadId, kind: kind ?? null }),
  getThreadMessageWindow: (threadId, offset, limit, desc = false, kind = null) =>
    invoke<Message[]>("get_thread_message_window", {
      threadId,
      offset,
      limit,
      desc,
      kind: kind ?? null,
    }),
  threadMessageIndex: (threadId, messageId, kind = null, desc = false) =>
    invoke<number | null>("thread_message_index", {
      threadId,
      messageId,
      kind: kind ?? null,
      desc,
    }),
  recoverAttachmentMedia: (attachmentId) =>
    invoke<RecoveredMedia | null>("recover_attachment_media", { attachmentId }),
  countTimelineMessages: (service, search = null, kind = null) =>
    invoke<number>("count_timeline_messages", {
      service: service ?? null,
      search: search ?? null,
      kind: kind ?? null,
    }),
  getTimelineWindow: (
    offset,
    limit,
    service,
    search = null,
    desc = false,
    kind = null,
  ) =>
    invoke<TimelineMessage[]>("get_timeline_window", {
      offset,
      limit,
      service: service ?? null,
      search: search ?? null,
      desc,
      kind: kind ?? null,
    }),
  countMessageRanges: (ranges, service, search = null, kind = null) =>
    invoke<number[]>("count_message_ranges", {
      ranges,
      service: service ?? null,
      search: search ?? null,
      kind: kind ?? null,
    }),
  messageDateBounds: () =>
    invoke<[number, number] | null>("message_date_bounds"),
  getRangeWindow: (
    lo,
    hi,
    offset,
    limit,
    service,
    search = null,
    desc = false,
    kind = null,
  ) =>
    invoke<TimelineMessage[]>("get_range_window", {
      lo,
      hi,
      offset,
      limit,
      service: service ?? null,
      search: search ?? null,
      desc,
      kind: kind ?? null,
    }),
  listCalls: () => invoke<Call[]>("list_calls"),
  listSafariHistory: () => invoke<HistoryVisit[]>("list_safari_history"),
  listNotes: () => invoke<Note[]>("list_notes"),
  unlockNote: (noteId, password) =>
    invoke<string>("unlock_note", { noteId, password }),
  listRecordings: () => invoke<Recording[]>("list_recordings"),
  countMedia: (source, lo = null, hi = null, search = null) =>
    invoke<number>("count_media", { source, lo, hi, search }),
  countMediaRanges: (source, ranges, search = null) =>
    invoke<number[]>("count_media_ranges", { source, ranges, search }),
  getMediaWindow: (source, lo, hi, search, offset, limit, sortBy, desc) =>
    invoke<MediaItem[]>("get_media_window", {
      source,
      lo,
      hi,
      search,
      offset,
      limit,
      sortBy,
      desc,
    }),
  countCalls: (search, lo = null, hi = null) =>
    invoke<number>("count_calls", { search, lo, hi }),
  countCallRanges: (ranges, search = null) =>
    invoke<number[]>("count_call_ranges", { ranges, search: search ?? null }),
  getCallsWindow: (search, lo, hi, offset, limit, sortBy, desc) =>
    invoke<Call[]>("get_calls_window", {
      search,
      lo,
      hi,
      offset,
      limit,
      sortBy,
      desc,
    }),
  countSafari: (search, lo = null, hi = null) =>
    invoke<number>("count_safari", { search, lo, hi }),
  countSafariRanges: (search, ranges) =>
    invoke<number[]>("count_safari_ranges", { search, ranges }),
  getSafariWindow: (search, lo, hi, offset, limit, sortBy, desc) =>
    invoke<HistoryVisit[]>("get_safari_window", {
      search,
      lo,
      hi,
      offset,
      limit,
      sortBy,
      desc,
    }),
  countSafariBookmarks: (kind, search, lo = null, hi = null) =>
    invoke<number>("count_safari_bookmarks", { kind, search, lo, hi }),
  countSafariBookmarkRanges: (kind, search, ranges) =>
    invoke<number[]>("count_safari_bookmark_ranges", { kind, search, ranges }),
  getSafariBookmarksWindow: (kind, search, lo, hi, offset, limit, sortBy, desc) =>
    invoke<SafariBookmark[]>("get_safari_bookmarks_window", {
      kind,
      search,
      lo,
      hi,
      offset,
      limit,
      sortBy,
      desc,
    }),
  listContacts: () => invoke<Contact[]>("list_contacts"),
  listInstalledApps: () => invoke<InstalledApp[]>("list_installed_apps"),

  runSecurityScan: (kind) =>
    invoke<ScanSummary>("run_security_scan", { kind }),
  cancelScan: () => invoke("cancel_scan"),
  onScanProgress: (cb) =>
    listen<ScanProgress>("scan://progress", (e) => cb(e.payload)),
  listScanRuns: () => invoke<ScanRun[]>("list_scan_runs"),
  latestScanRun: () => invoke<number | null>("latest_scan_run"),
  listFindings: (runId, minSeverity, module) =>
    invoke<Finding[]>("list_findings", {
      runId,
      minSeverity: minSeverity ?? null,
      module: module ?? null,
    }),
  getIndicatorInfo: () => invoke<SnapshotInfo>("get_indicator_info"),
  updateIndicators: () => invoke<SnapshotInfo>("update_indicators"),
  getDetectionSettings: () =>
    invoke<DetectionSettings>("get_detection_settings"),
  setDetectionSettings: (settings) =>
    invoke("set_detection_settings", { settings }),
  findShortenerUrls: (text) =>
    invoke<string[]>("find_shortener_urls", { text }),
  expandShortUrl: (url) => invoke<string>("expand_short_url", { url }),
  deshortenAutoApproveGet: () =>
    invoke<boolean>("deshorten_auto_approve_get"),
  deshortenAutoApproveSet: (enabled) =>
    invoke("deshorten_auto_approve_set", { enabled }),
  runPassiveCheckNow: () =>
    invoke<ScanSummary | null>("run_passive_check_now"),
  exportScanReport: async (runId) => {
    const path = await save({
      title: "Save Security Check report",
      defaultPath: "security-check-report.csv",
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (typeof path !== "string") return null;
    await invoke("export_scan_report", { runId, path });
    return path;
  },
  listMedia: () => invoke<MediaItem[]>("list_media"),
  mediaSources: () => invoke<MediaSource[]>("media_sources"),
  // Served by the register_uri_scheme_protocol handler in the Rust shell.
  // (mediaQuery below builds the `?thumb=1&k=…` suffix.)
  mediaUrl: (id, opts) =>
    `traceloupe-media://localhost/${id}${mediaQuery(opts)}`,
  contactAvatarUrl: (id) => `traceloupe-avatar://localhost/${id}`,
  attachmentUrl: (id, opts) =>
    `traceloupe-attachment://localhost/${id}${mediaQuery(opts)}`,
  audioUrl: (id) => `traceloupe-audio://localhost/${id}`,
  noteImageUrl: (id, index) => `traceloupe-note-image://localhost/${id}${index != null ? `/${index}` : ""}`,
  openAttachment: (id) => invoke<void>("open_attachment", { attachmentId: id }),
  reimportModule: (moduleId) =>
    invoke<ReimportResult>("reimport_module", { moduleId }),
};

const mockBackups: BackupInfo[] = [
  {
    id: "00008030-000A1B2C3D4E5F",
    path: "/Users/dev/Library/Application Support/MobileSync/Backup/00008030-000A1B2C3D4E5F",
    deviceName: "Peter's iPhone",
    productType: "iPhone12,3",
    productVersion: "17.5.1",
    serialNumber: "F2LXXXXXXXXX",
    lastBackupDate: 1749400000,
    isEncrypted: true,
  },
  {
    id: "11119040-000B2C3D4E5F6A",
    path: "/Users/dev/Library/Application Support/MobileSync/Backup/11119040-000B2C3D4E5F6A",
    deviceName: "Old iPhone SE",
    productType: "iPhone12,8",
    productVersion: "15.8",
    serialNumber: null,
    lastBackupDate: 1680000000,
    isEncrypted: false,
  },
];

// Mock message data mirroring the test fixture, so the Messages view is
// exercisable in the browser. Becomes "active" after a mock import.
const mockThreads: ThreadSummary[] = [
  {
    // identifier is the chat ROWID (as iLEAPP stores it); displayName is the handle.
    id: 1,
    identifier: "12",
    displayName: "+15551234567",
    service: "iMessage",
    lastMessageAt: 1717841460,
    messageCount: 6,
    snippet: "Here's the trailhead 📷",
    participants: ["+15551234567"],
  },
  {
    id: 2,
    identifier: "8",
    displayName: "+15559876543",
    service: "SMS",
    lastMessageAt: 1717500000,
    messageCount: 2,
    snippet: "Call me when you land ❤️",
    participants: ["+15559876543"],
  },
  {
    // A group chat: displayName holds the group's name; members via participants.
    id: 4,
    identifier: "20",
    displayName: "Hiking Crew",
    service: "iMessage",
    lastMessageAt: 1717841700,
    messageCount: 3,
    snippet: "See you at the trailhead!",
    participants: ["+15551234567", "+15559876543", "+15550001111"],
  },
  {
    // A third-party app DM (TikTok), tagged by its service for the app filter.
    id: 5,
    identifier: "0:1:179546233697390592:7145206438070666245",
    displayName: "★ hembokke",
    service: "TikTok",
    lastMessageAt: 1717600000,
    messageCount: 2,
    snippet: "sent you a video 🎵",
    participants: ["@hembokke"],
  },
];

const mockMessages: Record<number, Message[]> = {
  1: [
    {
      id: 1,
      isFromMe: false,
      sender: "+15551234567",
      body: "Hey, are you around this weekend?",
      sentAt: 1717840800,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
    {
      id: 2,
      isFromMe: true,
      sender: null,
      body: "Yeah! What did you have in mind?",
      sentAt: 1717840980,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
    {
      id: 3,
      isFromMe: false,
      sender: "+15551234567",
      body: "Thinking of hiking Mission Peak",
      sentAt: 1717841100,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
    {
      id: 4,
      isFromMe: true,
      sender: null,
      body: "I'm in. Saturday morning?",
      sentAt: 1717841220,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      deleted: true,
      deletedAt: 1717927620,
      attachments: [],
    },
    {
      id: 5,
      isFromMe: false,
      sender: "+15551234567",
      body: "Here's the itinerary",
      sentAt: 1717841340,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [
        {
          id: 2,
          filename: "itinerary.pdf",
          mimeType: "application/pdf",
          localPath: "/mock/itinerary.pdf",
        },
      ],
    },
    {
      id: 6,
      isFromMe: true,
      sender: null,
      body: "Here's the trailhead 📷",
      sentAt: 1717841460,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [
        {
          id: 1,
          filename: "traceloupe-test.png",
          mimeType: "image/png",
          localPath: "/mock/traceloupe-test.png",
        },
      ],
    },
  ],
  2: [
    {
      id: 7,
      isFromMe: true,
      sender: null,
      body: "Landing at 6, boarding now",
      sentAt: 1717499000,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
    {
      id: 8,
      isFromMe: false,
      sender: "Mom",
      body: "Call me when you land ❤️",
      sentAt: 1717500000,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
  ],
  5: [
    {
      id: 9,
      isFromMe: false,
      sender: "★ hembokke",
      body: "have you seen this one 😂",
      sentAt: 1717599000,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
    {
      id: 10,
      isFromMe: true,
      sender: null,
      body: "sent you a video 🎵",
      sentAt: 1717600000,
      readAt: null,
      deliveredAt: null,
      reactions: null,
      replyToSnippet: null,
      edited: false,
      attachments: [],
    },
  ],
};

// A large synthetic thread, so virtualization can be stress-tested in a browser
// (the small fixtures above never exceed the viewport, hiding scroll bugs).
mockThreads.push({
  id: 3,
  identifier: "Big Test Group",
  displayName: "Big Test Group",
  service: "iMessage",
  lastMessageAt: 1717000000 + 2999 * 600,
  messageCount: 3000,
  snippet: "Message number 3000",
  participants: ["Big Test Group"],
});
mockMessages[3] = Array.from({ length: 3000 }, (_, i) => ({
  id: 1000 + i,
  isFromMe: i % 3 === 0,
  sender: i % 3 === 0 ? null : "Big Test Group",
  body: `Message number ${i + 1} in the big test thread`,
  sentAt: 1717000000 + i * 600,
  readAt: null,
  deliveredAt: null,
  reactions: null,
  replyToSnippet: null,
  edited: false,
  attachments: [],
}));
mockMessages[4] = [
  {
    id: 2000,
    isFromMe: false,
    sender: "+15559876543",
    body: "Who's in for Saturday?",
    sentAt: 1717841600,
    readAt: null,
    deliveredAt: null,
    reactions: null,
    replyToSnippet: null,
    edited: false,
    attachments: [],
  },
  {
    id: 2001,
    isFromMe: true,
    sender: null,
    body: "I'm in!",
    sentAt: 1717841650,
    readAt: null,
    deliveredAt: null,
    reactions: null,
    replyToSnippet: null,
    edited: false,
    effect: "Confetti",
    attachments: [],
  },
  {
    id: 2002,
    isFromMe: false,
    sender: "+15550001111",
    body: "See you at the trailhead!",
    sentAt: 1717841700,
    readAt: null,
    deliveredAt: null,
    reactions: null,
    replyToSnippet: null,
    edited: false,
    attachments: [],
  },
  // App-bubble messages (no text of their own) now surface as typed placeholders.
  {
    id: 2003,
    isFromMe: true,
    sender: null,
    body: "Digital Touch",
    sentAt: 1717841760,
    readAt: null,
    deliveredAt: null,
    reactions: null,
    replyToSnippet: null,
    edited: false,
    attachments: [],
  },
  {
    id: 2004,
    isFromMe: false,
    sender: "+15559876543",
    body: "GamePigeon",
    sentAt: 1717841820,
    readAt: null,
    deliveredAt: null,
    reactions: null,
    replyToSnippet: null,
    edited: false,
    attachments: [],
  },
];

// All mock messages flattened into one chronological stream, for the timeline.
const mockTimeline: TimelineMessage[] = mockThreads
  .flatMap((t) =>
    (mockMessages[t.id] ?? []).map((message) => ({
      threadId: t.id,
      threadTitle: t.displayName ?? t.identifier,
      threadHandle: t.identifier,
      service: t.service,
      message,
    })),
  )
  .sort((a, b) => (a.message.sentAt ?? 0) - (b.message.sentAt ?? 0));

function inRange(sentAt: number | null, r: TimeRange): boolean {
  if (sentAt == null) return false;
  return (r.lo == null || sentAt >= r.lo) && (r.hi == null || sentAt < r.hi);
}
function mockFilterTimeline(
  service: string | null | undefined,
  range: TimeRange | undefined,
  search: string | null | undefined,
): TimelineMessage[] {
  const q = search?.toLowerCase() ?? null;
  return mockTimeline.filter((t) => {
    if (service && t.service !== service) return false;
    if (range && !inRange(t.message.sentAt, range)) return false;
    if (q) {
      const hay = [
        t.message.body,
        t.message.sender,
        t.threadTitle,
        t.threadHandle,
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

const mockCalls: Call[] = [
  {
    id: 1,
    address: "friend@icloud.com",
    direction: "incoming",
    answered: true,
    durationS: 128,
    occurredAt: 1717786800,
    service: "facetime",
    callType: "audio",
    location: null,
    countryCode: null,
  },
  {
    id: 2,
    address: "+15559876543",
    direction: "incoming",
    answered: false,
    durationS: 0,
    occurredAt: 1717785000,
    service: "phone",
    callType: null,
    location: "California",
    countryCode: "us",
  },
  {
    id: 3,
    address: "+15551234567",
    direction: "outgoing",
    answered: true,
    durationS: 312,
    occurredAt: 1717783200,
    service: "phone",
    callType: null,
    location: null,
    countryCode: "gb",
  },
];

const mockSafari: HistoryVisit[] = [
  {
    id: 1,
    url: "https://en.wikipedia.org/wiki/Mission_Peak",
    title: "Mission Peak - Wikipedia",
    visitedAt: 1717801200,
    visitCount: 2,
    deleted: false,
  },
  {
    id: 2,
    url: "https://news.ycombinator.com/",
    title: "Hacker News",
    visitedAt: 1717797600,
    visitCount: 34,
    deleted: false,
  },
  {
    id: 3,
    url: "https://www.apple.com/",
    title: "Apple",
    visitedAt: 1717794000,
    visitCount: 12,
    deleted: false,
  },
  {
    id: 4,
    url: "https://secret.example/cleared",
    title: null,
    visitedAt: 1717790000,
    visitCount: null,
    deleted: true,
  },
];

const mockSafariBookmarks: SafariBookmark[] = [
  {
    id: 1,
    kind: "bookmark",
    title: "Apple",
    url: "https://www.apple.com/",
    folder: null,
    dateAdded: 1700000000,
    dateViewed: null,
    previewText: null,
    private: false,
  },
  {
    id: 2,
    kind: "bookmark",
    title: "Hacker News",
    url: "https://news.ycombinator.com/",
    folder: "Tech",
    dateAdded: 1699000000,
    dateViewed: null,
    previewText: null,
    private: false,
  },
  {
    id: 3,
    kind: "reading_list",
    title: "A long read",
    url: "https://example.com/article",
    folder: null,
    dateAdded: 1712000000,
    dateViewed: 1712500000,
    previewText: "An interesting article saved for later.",
    private: false,
  },
  {
    id: 4,
    kind: "tab",
    title: "Wikipedia",
    url: "https://en.wikipedia.org/",
    folder: null,
    dateAdded: null,
    dateViewed: 1717200000,
    previewText: null,
    private: false,
  },
  {
    id: 5,
    kind: "tab",
    title: "Shopping cart",
    url: "https://shop.example.com/cart",
    folder: null,
    dateAdded: null,
    dateViewed: 1717450000,
    previewText: null,
    private: true,
  },
];

// Mock note timestamps are relative to "now" so the recency groupings (Last 7
// Days, Last 30 Days, …) are demonstrable in the browser preview.
const DAY = 86_400;
const nowS = Math.floor(Date.now() / 1000);
const mockNotes: Note[] = [
  {
    id: 2,
    folder: "Work",
    title: "Q3 ideas",
    snippet: "Ship the importer, then…",
    body: "Ship the importer, then work on lazy decode and the encrypted path.",
    createdAt: nowS - 40 * DAY,
    modifiedAt: nowS - 2 * DAY,
    pinned: true,
    locked: false,
    passwordHint: null,
    hasChecklist: false,
    imageCount: 2,
    availableImageCount: 0,
    attachmentCount: 2,
    tags: [],
    hasImage: true,
    bodyRich: null,
  },
  {
    id: 1,
    folder: "Notes",
    title: "Hike checklist",
    snippet: "Water, snacks, sunscreen…",
    body: "Water\nSnacks\nSunscreen\nHat\nExtra socks",
    createdAt: nowS - 6 * DAY,
    modifiedAt: nowS - 3 * DAY,
    pinned: false,
    locked: false,
    passwordHint: null,
    hasChecklist: false,
    imageCount: 0,
    availableImageCount: 0,
    attachmentCount: 0,
    tags: [],
    hasImage: false,
    bodyRich: null,
  },
  {
    id: 3,
    folder: "Notes",
    title: null,
    snippet: "Grocery list",
    body: "Milk\nEggs\nBröd\nKaffe",
    createdAt: nowS - 25 * DAY,
    modifiedAt: nowS - 20 * DAY,
    pinned: false,
    locked: false,
    passwordHint: null,
    hasChecklist: false,
    imageCount: 0,
    availableImageCount: 0,
    attachmentCount: 0,
    tags: [],
    hasImage: false,
    bodyRich: null,
  },
  {
    id: 4,
    folder: "Personal",
    title: "Passwords",
    snippet: null,
    body: null,
    createdAt: nowS - 400 * DAY,
    modifiedAt: nowS - 300 * DAY,
    pinned: false,
    locked: true,
    passwordHint: "the usual",
    hasChecklist: false,
    imageCount: 0,
    availableImageCount: 0,
    attachmentCount: 0,
    tags: [],
    hasImage: false,
    bodyRich: null,
  },
];

const mockRecordings: Recording[] = [
  {
    id: 1,
    title: "Morning idea",
    folder: null,
    recordedAt: 1717838000,
    durationS: 42.5,
    fileName: "20240608 083320.m4a",
  },
  {
    id: 2,
    title: "Meeting notes",
    folder: null,
    recordedAt: 1717500000,
    durationS: 195,
    fileName: "20240604 100000.m4a",
  },
  {
    id: 3,
    title: null,
    folder: null,
    recordedAt: 1716600000,
    durationS: 9.2,
    fileName: "New Recording 3.m4a",
  },
];

const contactExtras = {
  middleName: null,
  nickname: null,
  jobTitle: null,
  department: null,
  birthdayAt: null,
  note: null,
  addresses: [] as LabeledValue[],
  related: [] as LabeledValue[],
  groups: [] as string[],
  social: [] as LabeledValue[],
};
const mockContacts: Contact[] = [
  {
    id: 1,
    firstName: "Jordan",
    lastName: "Kim",
    organization: "Acme Corp",
    phones: [{ label: "Work", value: "+15559876543" }],
    emails: [{ label: "Work", value: "jordan@acme.example" }],
    hasImage: true,
    source: "Address Book",
    ...contactExtras,
  },
  {
    id: 2,
    firstName: "Alex",
    lastName: "Rivera",
    organization: null,
    phones: [{ label: "Mobile", value: "+15551234567" }],
    emails: [{ label: "Home", value: "alex@example.com" }],
    hasImage: true,
    source: "Address Book",
    ...contactExtras,
    jobTitle: "Engineer",
    birthdayAt: 1678307200,
    note: "met at the conference",
    addresses: [{ label: "Home", value: "1 Market St, Springfield, CA 90001, USA" }],
    related: [
      { label: "Mother", value: "Maria Rivera" },
      { label: "Bestie", value: "Sam Taylor" },
    ],
    groups: ["Climbing", "Family"],
    social: [{ label: "Snapchat", value: "alex_r" }],
  },
  {
    id: 3,
    firstName: "Sam",
    lastName: "Taylor",
    organization: null,
    phones: [],
    emails: [{ label: "Home", value: "sam.taylor@example.com" }],
    hasImage: false,
    source: "Address Book",
    ...contactExtras,
  },
  {
    id: 4,
    firstName: null,
    lastName: null,
    organization: "Bella Vista Pizza",
    phones: [{ label: "Mobile", value: "+15550001111" }],
    emails: [],
    hasImage: false,
    source: "Address Book",
    ...contactExtras,
  },
  // A third-party app's social graph: name + @handle only (behind the filter).
  {
    id: 5,
    firstName: "★ Alice ✿",
    lastName: null,
    organization: "@ccidkk",
    phones: [],
    emails: [],
    hasImage: false,
    source: "TikTok",
    ...contactExtras,
  },
  {
    id: 6,
    firstName: "jhopesop",
    lastName: null,
    organization: "@jhopesop",
    phones: [],
    emails: [],
    hasImage: false,
    source: "TikTok",
    ...contactExtras,
  },
];

// Colored initials SVGs standing in for real contact photos in the browser mock.
const mockAvatarColors: Record<number, string> = { 1: "#7c3aed", 2: "#0891b2" };
function mockAvatarDataUrl(id: number): string {
  const color = mockAvatarColors[id] ?? "#888";
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='96' height='96'><rect width='96' height='96' fill='${color}'/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

const mockMedia: MediaItem[] = [
  {
    id: 1,
    kind: "photo",
    source: "Messages",
    mimeType: "image/png",
    filename: "traceloupe-test.png",
    takenAt: 1717841460,
    persons: null,
    latitude: null,
    longitude: null,
    favorite: false,
    location: null,
    albums: null,
    width: null,
    height: null,
    durationS: null,
    fileSize: null,
    camera: null,
    lens: null,
    exif: null,
    hidden: false,
    trashed: false,
    trashedAt: null,
    addedAt: null,
    subtype: "live",
  },
  {
    id: 2,
    kind: "photo",
    source: "Messages",
    mimeType: "image/png",
    filename: "sunset.png",
    takenAt: 1717841520,
    persons: null,
    latitude: null,
    longitude: null,
    favorite: false,
    location: null,
    albums: null,
    width: null,
    height: null,
    durationS: null,
    fileSize: null,
    camera: null,
    lens: null,
    exif: null,
    hidden: false,
    trashed: false,
    trashedAt: null,
    addedAt: null,
    subtype: "panorama",
  },
  {
    id: 3,
    kind: "photo",
    source: "Photos",
    mimeType: "image/png",
    filename: "forest.png",
    takenAt: 1717841580,
    persons: "Alice, Bob",
    latitude: null,
    longitude: null,
    favorite: false,
    location: "Florida",
    albums: "Vacation",
    width: 4032,
    height: 3024,
    durationS: null,
    fileSize: 2097152,
    camera: "Apple iPhone 14 Pro",
    lens: "iPhone 14 Pro back camera",
    exif: "ISO 100 · ƒ/1.8 · 1/125s · 26 mm",
    hidden: false,
    trashed: false,
    trashedAt: null,
    addedAt: 1720000000,
    subtype: "burst",
  },
  {
    id: 4,
    kind: "photo",
    source: "WhatsApp",
    mimeType: "image/heic",
    filename: "IMG_0421.heic",
    takenAt: 1717841640,
    persons: null,
    latitude: null,
    longitude: null,
    favorite: false,
    location: null,
    albums: null,
    width: null,
    height: null,
    durationS: null,
    fileSize: null,
    camera: null,
    lens: null,
    exif: null,
    hidden: false,
    trashed: true,
    trashedAt: 1718000000,
    addedAt: null,
    subtype: "screenshot",
  },
];

// Solid-color SVG data URIs mirroring the fixture's seeded photos.
const mockMediaColors: Record<number, string> = {
  1: "#4a90e2",
  2: "#f0823c",
  3: "#3ca05a",
  4: "#c8507a",
};
function mockMediaDataUrl(id: number): string {
  const color = mockMediaColors[id] ?? "#888";
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='240' height='240'><rect width='240' height='240' fill='${color}'/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

// A realistic mix: some TraceLoupe-supported apps, some not, plus system apps.
// Metadata mirrors what Info.plist's iTunesMetadata carries (name/seller/
// version/genre/release date); system apps carry none.
const mockInstalledApps: InstalledApp[] = [
  { bundleId: "net.whatsapp.WhatsApp", name: "WhatsApp Messenger", seller: "WhatsApp Inc.", version: "23.24.0", genre: "Social Networking", released: "2009-05-03T00:00:00Z" },
  { bundleId: "com.burbn.instagram", name: "Instagram", seller: "Instagram, Inc.", version: "436.0.0", genre: "Photo & Video", released: "2010-10-06T08:12:41Z" },
  { bundleId: "com.toyopagroup.picaboo", name: "Snapchat", seller: "Snap, Inc.", version: "12.80.0", genre: "Photo & Video", released: "2011-07-13T00:00:00Z" },
  { bundleId: "com.zhiliaoapp.musically", name: "TikTok", seller: "TikTok Ltd.", version: "34.1.0", genre: "Entertainment", released: "2014-04-01T00:00:00Z" },
  { bundleId: "org.telegram.messenger", name: "Telegram Messenger", seller: "Telegram FZ-LLC", version: "10.5.1", genre: "Social Networking", released: "2013-08-14T00:00:00Z" },
  { bundleId: "com.spotify.client", name: "Spotify - Music and Podcasts", seller: "Spotify Ltd.", version: "8.9.10", genre: "Music", released: "2011-07-14T00:00:00Z" },
  { bundleId: "com.google.Gmail", name: "Gmail - Email by Google", seller: "Google LLC", version: "6.0.240107", genre: "Productivity", released: "2011-11-02T00:00:00Z" },
  { bundleId: "com.tinyspeck.chatlyio", name: "Slack", seller: "Slack Technologies Inc.", version: "23.11.90", genre: "Business", released: "2013-08-21T00:00:00Z" },
  { bundleId: "com.ubercab.UberClient", name: "Uber - Request a ride", seller: "Uber Technologies, Inc.", version: "3.577.10", genre: "Travel", released: "2010-08-05T00:00:00Z" },
  { bundleId: "com.apple.mobilesafari", name: null, seller: null, version: null, genre: null, released: null },
];

const mockSnapshotInfo: SnapshotInfo = {
  generatedAt: "2026-07-20T16:08:47Z",
  feeds: [
    { source: "AmnestyTech/pegasus", class: "mercenary", count: 1549, skipped: 0 },
    { source: "mvt-project/predator", class: "mercenary", count: 812, skipped: 0 },
    { source: "echap/ioc", class: "stalkerware", count: 2746, skipped: 0 },
    { source: "echap/watchware", class: "watchware", count: 159, skipped: 0 },
  ],
};

let mockDetectionSettings: DetectionSettings = {
  passiveEnabled: true,
  passiveScope: "apps_only",
  passiveConsent: "unasked",
  autoUpdateIndicators: true,
  fetchConsent: "unasked",
  expandShortUrls: false,
  customIndicatorDir: null,
};

let mockDeshortenAutoApprove = false;

let mockScanRuns: ScanRun[] = [];

const mockFindings: Finding[] = [
  {
    id: 1,
    runId: 1,
    severity: "info",
    kind: "bundle_id",
    module: "apps",
    malware: "KasperskySafeKids",
    matchedValue: "com.kaspersky.safekids",
    context: "com.kaspersky.safekids",
    refKind: "app",
    refId: null,
    eventTime: null,
    isNew: false,
  },
  {
    id: 2,
    runId: 1,
    severity: "warning",
    kind: "domain",
    module: "safari",
    malware: "TheTruthSpy",
    matchedValue: "thetruthspy.com",
    context: "tap here https://bit.ly/3xShort to install — thetruthspy.com",
    refKind: "safari_history",
    refId: 42,
    eventTime: 1700001000,
    isNew: true,
  },
];

let mockActive = false;
const mockImported = new Set<string>();

// Mock-side filters mirroring the backend's windowed SQL, so the browser mock
// behaves like the real windowed/filterable queries.
function mockFilterMedia(
  source: string | null,
  range?: TimeRange,
  search?: string | null,
): MediaItem[] {
  let out = source
    ? mockMedia.filter((m) => (m.source ?? "Other") === source)
    : mockMedia;
  if (range && (range.lo != null || range.hi != null)) {
    out = out.filter((m) => inRange(m.takenAt ?? null, range));
  }
  if (search) {
    const q = search.toLowerCase();
    out = out.filter((m) =>
      [m.filename, m.persons, m.location, m.albums].some(
        (f) => f?.toLowerCase().includes(q) ?? false,
      ),
    );
  }
  return out;
}
function mockFilterCalls(search: string | null, range?: TimeRange): Call[] {
  let out = mockCalls;
  if (search) {
    const q = search.toLowerCase();
    out = out.filter((c) => c.address?.toLowerCase().includes(q));
  }
  if (range && (range.lo != null || range.hi != null)) {
    out = out.filter(
      (c) =>
        c.occurredAt != null &&
        (range.lo == null || c.occurredAt >= range.lo) &&
        (range.hi == null || c.occurredAt < range.hi),
    );
  }
  return out;
}
function mockFilterSafari(
  search: string | null,
  range?: TimeRange,
): HistoryVisit[] {
  let out = mockSafari;
  if (search) {
    const q = search.toLowerCase();
    out = out.filter(
      (h) =>
        h.url.toLowerCase().includes(q) ||
        (h.title?.toLowerCase().includes(q) ?? false),
    );
  }
  if (range && (range.lo != null || range.hi != null)) {
    out = out.filter((h) => inRange(h.visitedAt ?? null, range));
  }
  return out;
}
function mockFilterBookmarks(
  kind: string,
  search: string | null,
  range?: TimeRange,
): SafariBookmark[] {
  let out = mockSafariBookmarks.filter((b) => b.kind === kind);
  if (search) {
    const q = search.toLowerCase();
    out = out.filter(
      (b) =>
        (b.url?.toLowerCase().includes(q) ?? false) ||
        (b.title?.toLowerCase().includes(q) ?? false),
    );
  }
  if (range && (range.lo != null || range.hi != null)) {
    out = out.filter((b) => inRange(b.dateAdded ?? null, range));
  }
  return out;
}

/** Mirror the backend's sort for the in-browser mock: nulls last regardless of
 *  direction, so sorted mock lists match the real app. */
function mockSortBy<T>(
  items: T[],
  key: (t: T) => number | string | null | undefined,
  desc: boolean,
): T[] {
  const sign = desc ? -1 : 1;
  return [...items].sort((a, b) => {
    const ka = key(a) ?? null;
    const kb = key(b) ?? null;
    if (ka === null && kb === null) return 0;
    if (ka === null) return 1;
    if (kb === null) return -1;
    return ka < kb ? -sign : ka > kb ? sign : 0;
  });
}
const mediaKey = (by: string) => (m: MediaItem) =>
  by === "source" ? m.source : m.takenAt;
const callKey = (by: string) => (c: Call) =>
  by === "name" ? c.address : by === "duration" ? c.durationS : c.occurredAt;
const safariKey = (by: string) => (h: HistoryVisit) =>
  by === "title" ? h.title : by === "visits" ? h.visitCount : h.visitedAt;

// A mock progress emitter so the import flow is exercisable in the browser.
type ProgressCb = (p: ImportProgress) => void;
const mockProgressSubs = new Set<ProgressCb>();

const mockEngineSubs = new Set<(p: EngineProgress) => void>();

export const mockClient: TraceLoupeClient = {
  listBackups: async () => ({ status: "ok", backups: mockBackups }),
  defaultBackupRoot: async () =>
    "/Users/dev/Library/Application Support/MobileSync/Backup",
  pickBackupFolder: async () =>
    "/Users/dev/Library/Application Support/MobileSync/Backup",
  pickFolder: async () => "/Users/dev/custom-indicators",
  openFullDiskAccessSettings: async () => {},
  openExternal: async (url) => {
    window.open(url, "_blank");
  },
  fetchLinkPreview: async (url) => ({
    url,
    title: "Example page title",
    description: "A mock OpenGraph description for the link preview.",
    image: null,
    siteName: new URL(url).hostname,
  }),
  engineStatus: async () => true,
  engineInfo: async () => ({
    installed: true,
    version: "iLEAPP v2026.1.0",
    canDownload: true,
  }),
  installEngine: async () => {
    for (let i = 1; i <= 5; i++) {
      await new Promise((r) => setTimeout(r, 200));
      mockEngineSubs.forEach((cb) =>
        cb({
          phase: "downloading",
          received: i * 15_000_000,
          total: 78_000_000,
          fraction: i / 5,
        }),
      );
    }
    mockEngineSubs.forEach((cb) => cb({ phase: "verifying" }));
    await new Promise((r) => setTimeout(r, 300));
    mockEngineSubs.forEach((cb) => cb({ phase: "done" }));
  },
  onEngineProgress: async (cb) => {
    mockEngineSubs.add(cb);
    return () => mockEngineSubs.delete(cb);
  },
  listImportModules: async () => [
    {
      id: "messages",
      label: "Messages",
      category: "Communication",
      default: true,
    },
    {
      id: "calls",
      label: "Call history",
      category: "Communication",
      default: true,
    },
    {
      id: "contacts",
      label: "Contacts",
      category: "Communication",
      default: true,
    },
    { id: "safari", label: "Safari history", category: "Web", default: true },
    { id: "notes", label: "Notes", category: "Productivity", default: true },
    {
      id: "camera_roll",
      label: "Camera roll photos",
      category: "Media",
      default: true,
    },
  ],
  importBackup: async ({ backupId }) => {
    const artifacts = [
      "contacts",
      "callHistory",
      "safariHistory",
      "notes",
      "sms",
    ];
    for (let i = 0; i < artifacts.length; i++) {
      await new Promise((r) => setTimeout(r, 250));
      mockProgressSubs.forEach((cb) =>
        cb({
          phase: "parsing",
          current: i + 1,
          total: artifacts.length,
          fraction: (i + 1) / artifacts.length,
          artifact: artifacts[i],
        }),
      );
    }
    const steps = [
      "Preparing",
      "Indexing Messages",
      "Indexing Contacts",
      "Indexing App Chats",
      "Indexing Photos",
    ];
    for (let i = 0; i < steps.length; i++) {
      await new Promise((r) => setTimeout(r, 250));
      mockProgressSubs.forEach((cb) =>
        cb({ phase: "indexing", step: steps[i], index: i + 1, total: steps.length }),
      );
    }
    await new Promise((r) => setTimeout(r, 200));
    mockActive = true;
    mockImported.add(backupId);
    return {
      cachePath: "/mock/cache.db",
      threads: 2,
      messages: 8,
      mediaItems: 4,
      calls: 3,
      safariVisits: 3,
      contacts: 4,
      warnings: [],
    };
  },
  onImportProgress: async (cb) => {
    mockProgressSubs.add(cb);
    return () => mockProgressSubs.delete(cb);
  },
  cancelImport: async () => {},
  setLogLevel: async () => {},
  setBiometricRequired: async () => {},
  // Pretend the mock/browser preview is signed so the enabled toggle UI shows.
  appSigningStatus: async () => ({
    signed: true,
    adhoc: false,
    identity: "Mock Identity",
  }),
  onLog: async () => () => {},
  hasActiveBackup: async () => mockActive,
  closeBackup: async () => {
    mockActive = false;
  },
  openBackup: async (backupId) => {
    if (!mockImported.has(backupId)) return false;
    mockActive = true;
    return true;
  },
  forgetBackup: async (backupId) => {
    mockImported.delete(backupId);
  },
  importedBackupIds: async () => [...mockImported],
  listThreads: async () => (mockActive ? mockThreads : []),
  deviceInfo: async () =>
    mockActive
      ? {
          id: "mock-device",
          path: "/mock/backup",
          deviceName: "Peter's iPhone",
          productType: "iPhone15,2",
          productVersion: "17.5.1",
          serialNumber: "F2LW00XYZ123",
          lastBackupDate: 1717800000,
          isEncrypted: true,
        }
      : null,
  listCalendarEvents: async () =>
    mockActive
      ? [
          {
            id: 1,
            title: "Team standup",
            notes: "daily sync",
            location: "HQ · Room 4",
            startAt: 1717840800,
            endAt: 1717842600,
            allDay: false,
            calendarName: "Work",
            url: null,
            availability: "busy",
            recurring: true,
          },
          {
            id: 2,
            title: "Anna's birthday",
            notes: null,
            location: null,
            startAt: 1717804800,
            endAt: null,
            allDay: true,
            calendarName: "Family",
            url: null,
            availability: "free",
            recurring: false,
          },
        ]
      : [],
  listWorkouts: async () =>
    mockActive
      ? [
          {
            id: 1,
            activity: "Running",
            startAt: 1717840800,
            endAt: 1717842600,
            durationS: 1800,
            distanceM: 5200,
            hasRoute: true,
          },
          {
            id: 2,
            activity: "Walking",
            startAt: 1717754400,
            endAt: 1717756200,
            durationS: 1800,
            distanceM: 2100,
            hasRoute: false,
          },
        ]
      : [],
  workoutRoute: async (workoutId) =>
    mockActive && workoutId === 1
      ? // A wobbly out-and-back loop, enough shape to exercise the preview.
        Array.from({ length: 120 }, (_, i) => {
          const t = (i / 119) * 2 * Math.PI;
          return {
            at: 1717840800 + i * 15,
            latitude: 56.05 + 0.004 * Math.sin(t) + 0.001 * Math.sin(3 * t),
            longitude: 13.0 + 0.007 * (1 - Math.cos(t)) + 0.001 * Math.cos(5 * t),
            altitude: 20 + 5 * Math.sin(2 * t),
          };
        })
      : [],
  healthDaily: async () =>
    mockActive
      ? [
          {
            dayAt: 1717804800,
            steps: 8412,
            distanceM: 6120,
            flights: 9,
            activeKcal: 412,
            restingKcal: 1688,
            hrMin: 52,
            hrAvg: 71,
            hrMax: 142,
            audioDbMax: 74,
            walkSpeedMs: 1.31,
            stepLengthM: 0.68,
            doubleSupportPct: 0.28,
            walkAsymmetryPct: 0.03,
            moveKcal: 412,
            moveGoalKcal: 500,
            exerciseMin: 22,
            exerciseGoalMin: 30,
            standHours: 9,
            standGoalHours: 12,
          },
          {
            dayAt: 1717718400,
            steps: 3120,
            distanceM: 2210,
            flights: 2,
            activeKcal: 180,
            restingKcal: 1671,
            hrMin: null,
            hrAvg: null,
            hrMax: null,
            audioDbMax: null,
            walkSpeedMs: null,
            stepLengthM: null,
            doubleSupportPct: null,
            walkAsymmetryPct: null,
            moveKcal: 180,
            moveGoalKcal: 500,
            exerciseMin: null,
            exerciseGoalMin: null,
            standHours: null,
            standGoalHours: null,
          },
        ]
      : [],
  listSleep: async () =>
    mockActive
      ? [
          { id: 1, startAt: 1717822800, endAt: 1717851600, stage: "In Bed" },
          { id: 2, startAt: 1717824600, endAt: 1717837200, stage: "Deep" },
          { id: 3, startAt: 1717737600, endAt: 1717763400, stage: "In Bed" },
        ]
      : [],
  listHealthAchievements: async () =>
    mockActive
      ? [
          { id: 1, name: "NewMoveRecord", earnedAt: 1717804800, value: 1284, unit: "kcal" },
          { id: 2, name: "MoveGoal200Percent", earnedAt: 1717718400, value: 400, unit: "kcal" },
          { id: 3, name: "PerfectWeekMove", earnedAt: 1716854400, value: 7, unit: "count" },
        ]
      : [],
  listCycle: async () =>
    mockActive
      ? [
          { id: 1, category: "Menstrual flow", detail: "Medium", loggedAt: 1717718400 },
          { id: 2, category: "Abdominal cramps", detail: null, loggedAt: 1717718400 },
          { id: 3, category: "Mood changes", detail: null, loggedAt: 1717632000 },
          { id: 4, category: "Menstrual flow", detail: "Light", loggedAt: 1717632000 },
        ]
      : [],
  listHealthTimezones: async () =>
    mockActive
      ? [
          {
            tzName: "Europe/Stockholm",
            devices: ["iPhone12,8", "iPhone8,1"],
            samples: 310211,
            firstAt: 1500000000,
            lastAt: 1717900000,
          },
          {
            tzName: "America/New_York",
            devices: ["iPhone12,8"],
            samples: 3120,
            firstAt: 1651000000,
            lastAt: 1652200000,
          },
          {
            tzName: "Europe/Copenhagen",
            devices: ["iPhone12,8"],
            samples: 1890,
            firstAt: 1620000000,
            lastAt: 1688000000,
          },
        ]
      : [],
  healthSummary: async () =>
    mockActive
      ? {
          sampleCount: 344063,
          firstAt: 1500000000,
          lastAt: 1717900000,
          workoutCount: 2,
          dayCount: 2,
          sleepCount: 3,
          timezoneCount: 3,
          achievementCount: 3,
          cycleCount: 4,
        }
      : {
          sampleCount: 0,
          firstAt: null,
          lastAt: null,
          workoutCount: 0,
          dayCount: 0,
          sleepCount: 0,
          timezoneCount: 0,
          achievementCount: 0,
          cycleCount: 0,
        },
  listInteractions: async () =>
    mockActive
      ? [
          {
            id: 1,
            displayName: "Robin Chen",
            identifier: "+15551234567",
            incoming: 842,
            outgoing: 1203,
            incomingRecipient: 96,
            firstAt: 1500000000,
            lastAt: 1717900000,
          },
          {
            id: 2,
            displayName: null,
            identifier: "team@work.example",
            incoming: 210,
            outgoing: 55,
            incomingRecipient: 12,
            firstAt: 1600000000,
            lastAt: 1717800000,
          },
        ]
      : [],
  interactionChannels: async () =>
    mockActive
      ? [
          { bundleId: "com.apple.MobileSMS", incoming: 5873, outgoing: 6482 },
          { bundleId: "com.apple.InCallService", incoming: 140, outgoing: 157 },
          { bundleId: "com.toyopagroup.picaboo", incoming: 0, outgoing: 166 },
          { bundleId: "com.apple.facetime", incoming: 60, outgoing: 62 },
          { bundleId: "com.google.Gmail", incoming: 8, outgoing: 4 },
        ]
      : [],
  listReminders: async () =>
    mockActive
      ? [
          {
            id: 1,
            title: "Buy milk",
            notes: "2% please",
            listName: "Groceries",
            dueAt: 1717840800,
            completed: false,
            completedAt: null,
            flagged: true,
            priority: 1,
            createdAt: 1717000000,
          },
          {
            id: 2,
            title: "Call the bank",
            notes: null,
            listName: "Reminders",
            dueAt: null,
            completed: true,
            completedAt: 1717700000,
            flagged: false,
            priority: null,
            createdAt: 1716000000,
          },
        ]
      : [],
  // The mock messages carry no `kind`, so no content-kinds are advertised and the
  // filter is a no-op here.
  messageKinds: async () => [],
  countThreadMessages: async (threadId, _kind = null) =>
    mockActive ? (mockMessages[threadId]?.length ?? 0) : 0,
  getThreadMessageWindow: async (
    threadId,
    offset,
    limit,
    desc = false,
    _kind = null,
  ) => {
    if (!mockActive) return [];
    const all = mockMessages[threadId] ?? [];
    const ordered = desc ? [...all].reverse() : all;
    return ordered.slice(offset, offset + limit);
  },
  threadMessageIndex: async (threadId, messageId, _kind = null, desc = false) => {
    if (!mockActive) return null;
    const all = mockMessages[threadId] ?? [];
    const ordered = desc ? [...all].reverse() : all;
    const i = ordered.findIndex((m) => m.id === messageId);
    return i < 0 ? null : i;
  },
  recoverAttachmentMedia: async () => null,
  countTimelineMessages: async (service, search = null, _kind = null) =>
    mockActive ? mockFilterTimeline(service, undefined, search).length : 0,
  getTimelineWindow: async (
    offset,
    limit,
    service,
    search = null,
    desc = false,
    _kind = null,
  ) => {
    if (!mockActive) return [];
    const filtered = mockFilterTimeline(service, undefined, search);
    const ordered = desc ? [...filtered].reverse() : filtered;
    return ordered.slice(offset, offset + limit);
  },
  countMessageRanges: async (ranges, service, search = null, _kind = null) =>
    ranges.map((r) =>
      mockActive ? mockFilterTimeline(service, r, search).length : 0,
    ),
  messageDateBounds: async () => {
    if (!mockActive) return null;
    const ts = Object.values(mockMessages)
      .flat()
      .map((m) => m.sentAt)
      .filter((t): t is number => t != null);
    return ts.length ? [Math.min(...ts), Math.max(...ts)] : null;
  },
  getRangeWindow: async (
    lo,
    hi,
    offset,
    limit,
    service,
    search = null,
    desc = false,
    _kind = null,
  ) => {
    if (!mockActive) return [];
    const filtered = mockFilterTimeline(service, { lo, hi }, search);
    const ordered = desc ? [...filtered].reverse() : filtered;
    return ordered.slice(offset, offset + limit);
  },
  listCalls: async () => (mockActive ? mockCalls : []),
  listSafariHistory: async () => (mockActive ? mockSafari : []),
  listNotes: async () => (mockActive ? mockNotes : []),
  unlockNote: async (_noteId, password) =>
    password === "test"
      ? "Bank PIN: 1234\nWiFi: hunter2"
      : Promise.reject(new Error("Wrong password.")),
  listRecordings: async () => (mockActive ? mockRecordings : []),
  countMedia: async (source, lo = null, hi = null, search = null) =>
    mockActive ? mockFilterMedia(source, { lo, hi }, search).length : 0,
  countMediaRanges: async (source, ranges, search = null) =>
    ranges.map((r) =>
      mockActive ? mockFilterMedia(source, r, search).length : 0,
    ),
  getMediaWindow: async (source, lo, hi, search, offset, limit, sortBy, desc) =>
    mockActive
      ? mockSortBy(
          mockFilterMedia(source, { lo, hi }, search),
          mediaKey(sortBy),
          desc,
        ).slice(offset, offset + limit)
      : [],
  countCalls: async (search, lo = null, hi = null) =>
    mockActive ? mockFilterCalls(search, { lo, hi }).length : 0,
  countCallRanges: async (ranges, search = null) =>
    ranges.map((r) => (mockActive ? mockFilterCalls(search, r).length : 0)),
  getCallsWindow: async (search, lo, hi, offset, limit, sortBy, desc) =>
    mockActive
      ? mockSortBy(
          mockFilterCalls(search, { lo, hi }),
          callKey(sortBy),
          desc,
        ).slice(offset, offset + limit)
      : [],
  countSafari: async (search, lo = null, hi = null) =>
    mockActive ? mockFilterSafari(search, { lo, hi }).length : 0,
  countSafariRanges: async (search, ranges) =>
    ranges.map((r) => (mockActive ? mockFilterSafari(search, r).length : 0)),
  getSafariWindow: async (search, lo, hi, offset, limit, sortBy, desc) =>
    mockActive
      ? mockSortBy(
          mockFilterSafari(search, { lo, hi }),
          safariKey(sortBy),
          desc,
        ).slice(offset, offset + limit)
      : [],
  countSafariBookmarks: async (kind, search, lo = null, hi = null) =>
    mockActive ? mockFilterBookmarks(kind, search, { lo, hi }).length : 0,
  countSafariBookmarkRanges: async (kind, search, ranges) =>
    ranges.map((r) =>
      mockActive ? mockFilterBookmarks(kind, search, r).length : 0,
    ),
  getSafariBookmarksWindow: async (
    kind,
    search,
    lo,
    hi,
    offset,
    limit,
    sortBy,
    desc,
  ) =>
    mockActive
      ? mockSortBy(
          mockFilterBookmarks(kind, search, { lo, hi }),
          (b) => (sortBy === "title" ? (b.title ?? "") : b.dateAdded),
          desc,
        ).slice(offset, offset + limit)
      : [],
  listContacts: async () => (mockActive ? mockContacts : []),
  listInstalledApps: async () => (mockActive ? mockInstalledApps : []),

  runSecurityScan: async (kind) => {
    if (!mockActive) throw new Error("no backup is open");
    mockScanRuns = [
      {
        id: 1,
        kind,
        startedAt: Math.floor(Date.now() / 1000) - 2,
        finishedAt: Math.floor(Date.now() / 1000),
        status: "done",
        modules: kind === "passive" ? ["apps"] : ["apps", "messages", "safari"],
        indicatorCount: 5833,
        critical: kind === "passive" ? 0 : 0,
        warning: kind === "passive" ? 0 : 1,
        info: 1,
      },
      ...mockScanRuns,
    ];
    return {
      runId: 1,
      findings: kind === "passive" ? 1 : 2,
      cancelled: false,
    };
  },
  cancelScan: async () => {},
  onScanProgress: async () => () => {},
  listScanRuns: async () => (mockActive ? mockScanRuns : []),
  latestScanRun: async () =>
    mockActive && mockScanRuns.length ? mockScanRuns[0].id : null,
  listFindings: async (_runId, minSeverity) => {
    if (!mockActive) return [];
    const rank = (s: Severity) =>
      s === "critical" ? 3 : s === "warning" ? 2 : 1;
    const min = minSeverity ? rank(minSeverity) : 1;
    return mockFindings.filter((f) => rank(f.severity) >= min);
  },
  getIndicatorInfo: async () => mockSnapshotInfo,
  updateIndicators: async () => mockSnapshotInfo,
  getDetectionSettings: async () => mockDetectionSettings,
  setDetectionSettings: async (s) => {
    mockDetectionSettings = s;
  },
  findShortenerUrls: async (text) => {
    const hosts = ["bit.ly", "tinyurl.com", "t.co", "youtu.be"];
    return (text.match(/https?:\/\/[^\s"'<>()]+/g) ?? []).filter((u) =>
      hosts.some((h) => u.toLowerCase().includes(`//${h}/`)),
    );
  },
  expandShortUrl: async (url) =>
    `https://revealed.example/from/${encodeURIComponent(url)}`,
  deshortenAutoApproveGet: async () => mockDeshortenAutoApprove,
  deshortenAutoApproveSet: async (enabled) => {
    mockDeshortenAutoApprove = enabled;
  },
  runPassiveCheckNow: async () => {
    if (!mockActive || mockDetectionSettings.passiveConsent !== "granted")
      return null;
    return { runId: 1, findings: 1, cancelled: false };
  },
  exportScanReport: async () => "/tmp/security-check-report.csv",

  listMedia: async () => (mockActive ? mockMedia : []),
  mediaSources: async () => {
    if (!mockActive) return [];
    const counts = new Map<string, number>();
    for (const m of mockMedia) {
      const s = m.source ?? "Other";
      counts.set(s, (counts.get(s) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  },
  mediaUrl: (id) => mockMediaDataUrl(id),
  contactAvatarUrl: (id) => mockAvatarDataUrl(id),
  attachmentUrl: (id) => mockMediaDataUrl(id),
  // A short silent WAV so the browser mock renders a working <audio> control
  // (the real bytes come from the traceloupe-audio scheme under Tauri).
  audioUrl: () => SILENT_WAV_DATA_URL,
  noteImageUrl: (_id?: number, _index?: number) => "",
  openAttachment: async () => {},
  reimportModule: async (moduleId) => ({
    module: moduleId,
    recordings: mockActive ? mockRecordings.length : 0,
    mediaItems: 0,
    messages: 0,
    threads: 0,
    notes: 0,
    calls: 0,
    safariVisits: 0,
    warnings: [],
  }),
};

/** ~0.1s of silence — lets the mock player render/seek without a backend. */
const SILENT_WAV_DATA_URL =
  "data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAESsAACJWAAACABAAZGF0YQAAAAA=";

const isTauri = "__TAURI_INTERNALS__" in window;

export const client: TraceLoupeClient = isTauri ? tauriClient : mockClient;
