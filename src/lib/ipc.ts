/**
 * Typed client for the Tauri command layer.
 *
 * Two implementations of the same interface: the real one over
 * `invoke()`, and a mock used when the app runs in a plain browser
 * (Vite dev server, Playwright). Views depend only on `TraceLoupeClient`.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
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

/**
 * What became of one module's source store during the import (#288).
 *
 * `failed` is the one that matters: the store was in the backup and we could
 * not read it, so the view is empty because of US, not because the device held
 * nothing. Everything else the app says about emptiness is wording; this is a
 * fact the import knows and used to throw away.
 */
export interface ModuleStatus {
  module: string;
  status: "parsed" | "source-absent" | "failed";
  /** The underlying error, when `status` is `"failed"`. */
  detail: string | null;
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
  /** Unix epoch ms — lets the console show real times and keep order. */
  atMs: number;
}

/** One flush of the log stream. Records arrive batched (~10/s) rather than one
 *  IPC message per line, and `dropped` reports records the backend discarded to
 *  stay bounded under a flood — surfaced rather than hidden (#60). */
export interface LogBatch {
  records: LogRecord[];
  dropped: number;
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

/** A selectable data type for import (maps to a native Rust parser). */
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
  /** Safari profile the visit belongs to (iOS 17+): "Default" for the main
   *  history, otherwise the profile's name. Null on imports predating profiles. */
  profile: string | null;
  /** The visit happened on another iCloud-synced device, not this one. */
  synced: boolean;
  /** URL that redirected *to* this visit, when Safari recorded a chain. */
  redirectSource: string | null;
  /** URL this visit redirected *to*. */
  redirectDestination: string | null;
}

/** One Safari web search: a term recovered from a search-engine URL in history
 *  ("visited"), or typed into the search field ("typed"). */
export interface WebSearch {
  id: number;
  term: string;
  searchedAt: number | null;
  /** "visited" (recovered from a history URL) or "typed" (RecentWebSearches). */
  source: string;
  /** Search-engine host, when the term came from a URL. */
  engine: string | null;
  /** The result-page URL, when the term came from a URL. */
  url: string | null;
  /** Safari profile the search belongs to, when it came from history. */
  profile: string | null;
}

/** What a backup can say about messages that are GONE — content no longer in
 *  sms.db, as opposed to recently-deleted ones which keep their row and are
 *  flagged by `Message.deletedAt`. */
export interface DeletionEvidence {
  /** Deletions iOS recorded itself, in `sync_deleted_messages`. */
  recorded: number;
  /** ROWIDs allocated with no row. `message` is AUTOINCREMENT, so a gap means
   *  a row existed. NOT to be added to `recorded` — they usually describe the
   *  same deletions, so summing double-counts. */
  missingRowids: number;
  /** How many separate runs those missing ROWIDs fall into. */
  gaps: number;
  firstGapAt: number | null;
  lastGapAt: number | null;
}

/** An Apple device that contributed Health data to this phone. Health survives
 *  migration between phones, so this reaches back past devices no longer owned. */
export interface DeviceUse {
  /** `ProductType` as stored (`iPhone12,1`) — an identifier, not a marketing
   *  name. Render through `modelName()` in device-names.ts. */
  model: string;
  /** OS build (`21D50`). Null on the per-device rollup, which spans builds. */
  osBuild: string | null;
  firstAt: number | null;
  lastAt: number | null;
  samples: number;
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
  /** When this copy was downloaded on the account (RFC-3339). */
  downloaded: string | null;
  /** The Apple ID (account email) that downloaded the app. */
  appleId: string | null;
  /** App Store age rating label, e.g. "17+". */
  contentRating: string | null;
  /** Finer App Store category, e.g. "Social". */
  subgenre: string | null;
}

/** A fetched App Store icon as a self-contained data: URI. */
export interface AppIcon {
  bundleId: string;
  dataUri: string;
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

/** One home-dashboard tile (#157).
 *
 *  Carries its own `label`, `route` and `icon`, so this view renders whatever
 *  the backend sends without knowing which modules exist. A kind of data added
 *  later appears here with no frontend change at all — an unrecognised `icon`
 *  falls back to a generic glyph rather than dropping the tile. */
export interface ModuleMetric {
  id: string;
  label: string;
  /** Where the tile navigates, e.g. "/messages". */
  route: string;
  /** Icon name; unknown values fall back to a generic one. */
  icon: string;
  count: number;
  /** The period this data covers; null when the source has no timestamps. */
  firstAt: number | null;
  lastAt: number | null;
  /** Bucket counts across the span, sized to the data. Empty when there is too
   *  little of it to be a shape. */
  series: number[];
  /** What is inside — services, channels, Health categories — biggest first.
   *  The view draws these as brand icons instead of one generic glyph. */
  facets: { label: string; count: number }[];
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
  /** The feeds this run actually ran against (per-run receipt, stamped at
   *  scan start — independent of later feed updates). */
  feeds: FeedInfo[];
  /** Snapshot generated-at (unix seconds) at scan time; null on legacy runs. */
  feedsGeneratedAt: number | null;
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
  /** Optional local folder of custom indicator files merged into scans. */
  customIndicatorDir: string | null;
}

// --- Safety Scan types (ADR 0002) ---

/** The Forensic 9 category slugs (docs/CONTEXT.md). */
export type ContentCategory =
  | "threat-violence"
  | "harassment-bullying"
  | "sexual-content"
  | "grooming-exploitation"
  | "self-harm"
  | "hate-identity"
  | "coercive-control"
  | "scam-fraud"
  | "drugs-illegal";

export interface SafetyModelInfo {
  id: string;
  displayName: string;
  /** One-line role blurb (why you'd pick this model). */
  note: string;
  sizeBytes: number;
  installed: boolean;
  recommended: boolean;
}

export interface SafetyModelStatus {
  totalRamBytes: number;
  models: SafetyModelInfo[];
  readyModelId: string | null;
}

/** Result of a one-shot llama-server health check. */
export interface SafetyHealthReport {
  ok: boolean;
  modelId: string;
  displayName: string;
  startupMs: number;
  message: string;
}

export type SafetyModelProgressEvent =
  | { phase: "downloading"; received: number; total: number }
  | { phase: "verifying" }
  | { phase: "done" }
  | { phase: "error"; message: string };

/** In-flight model download snapshot, for rehydrating the UI after a refresh. */
export interface SafetyModelDownloadStatus {
  modelId: string;
  received: number;
  total: number;
  phase: "downloading" | "verifying";
}

/** How the Findings panel asks for a page. */
export interface ContentFindingPage {
  /** Only this severity, or every severity when undefined. */
  severity?: 1 | 2 | 3;
  includeDismissed: boolean;
  sortBy: "severity" | "date";
  desc: boolean;
  /** Order by conversation, so grouped mode can build headings from a window. */
  groupByThread: boolean;
  /** Drop findings whose source is gone — the report does, the panel doesn't. */
  excludeStale?: boolean;
  offset: number;
  limit: number;
}

/** A standing "this is fine" rule. Scope is conversation or category — NOT
 *  sender: a finding carries thread_identifier and category, but the sender
 *  lives on the message in the cache, a different database. */
export interface Suppression {
  scope: "thread" | "category";
  value: string;
  reason: string | null;
}

export interface ContentFindingCounts {
  /** Rows the requested filter matches — the virtualizer's count. */
  matching: number;
  live: number;
  /** Not dismissed and not stale — what the printable report includes. */
  liveFresh: number;
  dismissed: number;
  /** Live, not-stale findings nobody has read yet — the app's unread count. */
  unread: number;
  serious: number;
  harmful: number;
  concerning: number;
}

/** One bar of one report chart (#66). Index `i` is severity `i + 1`, so 0 is
 *  concerning and 2 serious. `confirmed` is what the cascade's strong tier
 *  agreed with; `unconfirmed` is what only the fast sweep ever saw — drawn
 *  hatched, so a number can't borrow authority the model never gave it. */
export interface ChartBucket {
  /** A date key, a category slug, or a thread identifier. Empty = notes. */
  key: string;
  confirmed: [number, number, number];
  unconfirmed: [number, number, number];
}

/** The report's charts, aggregated in SQL over EVERY finding the filter matches
 *  — never over the page the list renders, which is capped (#61/#65). */
export interface FindingAnalytics {
  /** What one bar of `overTime` spans; chosen from the span the findings cover
   *  so the axis stays readable from a two-week scan to a ten-year one. */
  unit: "day" | "week" | "month" | "quarter" | "year";
  /** Keyed by the LOCAL calendar: `YYYY-MM-DD` (day, and a week's Monday),
   *  `YYYY-MM`, `YYYY-Qn`, `YYYY`. Only non-empty buckets; the view fills gaps. */
  overTime: ChartBucket[];
  byCategory: ChartBucket[];
  /** The busiest conversations, most findings first, capped by the backend. */
  byConversation: ChartBucket[];
  /** Conversations past that cap, and their findings — stated, never dropped. */
  otherConversations: number;
  otherConversationFindings: number;
  /** How many findings the charts describe under the requested filter. */
  charted: number;
  /** In scope but undated, so absent from `overTime` alone. */
  undated: number;
  /** Dismissed as false positives: out of every chart, reported so the reader
   *  can see how much the model got wrong. */
  dismissed: number;
}

/** Which macOS setting changed. The payload is only an identifier — the value
 *  is re-read through the same command that reads it at startup, so there is one
 *  path rather than two that can disagree. */
export interface AccessibilityPrefs {
  reduceMotion: boolean;
  reduceTransparency: boolean;
  increaseContrast: boolean;
  differentiateWithoutColor: boolean;
  /** 1 small · 2 medium · 3 large (System Settings → Appearance). */
  sidebarIconSize: number;
  /** "automatic" | "whenScrolling" | "always". */
  showScrollBars: string;
}

export type SystemChange = {
  kind:
    | "accent"
    | "appearance"
    | "textSize"
    | "keyboardAccess"
    | "accessibility"
    | "locale";
};

export type SafetyScanEvent =
  | { phase: "loading" }
  | {
      phase: "classifying";
      done: number;
      total: number;
      /** Findings in this scan's scope right now — earlier runs included. */
      findings: number;
      /** How many of those were already there when this run started. */
      preexisting: number;
    }
  | { phase: "summarizing" }
  | {
      phase: "done";
      scanId: number;
      status: string;
      findings: number;
      classified: number;
      reused: number;
      skipped: number;
    }
  | { phase: "error"; message: string };

/** A Content Finding: one probabilistic model verdict on a message or note. */
export interface ContentFinding {
  id: number;
  sourceKind: "message" | "note";
  sourceId: number | null;
  /** Cache thread id for message findings — the Messages deep-link. */
  threadId: number | null;
  threadIdentifier: string | null;
  /** Messaging service for the app icon ("iMessage"/"TikTok"/…), "Notes" for
   * note findings, null when unresolved. */
  service: string | null;
  /** Unix epoch seconds. */
  occurredAt: number | null;
  fingerprint: string;
  category: ContentCategory;
  /** 1 = concerning, 2 = clearly harmful, 3 = serious/imminent. */
  severity: 1 | 2 | 3;
  rationale: string;
  stale: boolean;
  dismissed: boolean;
  /** True when the cascade's strong tier (E4B) re-checked and kept this finding
   *  — "confirmed" (two models agree) vs a sweep-only (E2B) unconfirmed flag. */
  rechecked: boolean;
  /** Has anyone revealed this finding's flagged text? Unread findings are shown
   *  differently in the list, the way unread mail is. */
  seen: boolean;
  /** Why it was dismissed, when it was. */
  dismissReason: string | null;
}

/** The flagged source behind a finding, loaded on demand for the peek popover. */
export interface FindingSnippet {
  /** The flagged text (message body, or note title + stripped body). */
  text: string;
  /** "Me" for the device owner, else the handle/name; null for notes. */
  sender: string | null;
  /** The conversation's name/handle — shown as "Me → recipient" when the
   *  device owner's own message is flagged; null for notes. */
  recipient: string | null;
  /** Unix epoch seconds; null for notes. */
  occurredAt: number | null;
  /** Service for the app icon ("iMessage"/"TikTok"/…), "Notes" for notes. */
  service: string | null;
}

export interface SafetyScanStatus {
  id: number;
  model: string;
  rangeStart: number | null;
  rangeEnd: number | null;
  /** 'interrupted' = a stranded 'running' row repaired at backup open. */
  status: "running" | "completed" | "cancelled" | "failed" | "interrupted";
  startedAt: number;
  finishedAt: number | null;
  chunksTotal: number;
  chunksDone: number;
}

export interface SafetyScanReport {
  scan: SafetyScanStatus | null;
  report: string | null;
  /** [threadIdentifier, summary] per flagged thread. */
  threadSummaries: [string, string][];
}

/** One thread's on-demand summary and how it was produced (#18). "model" is the
 *  classifier's prose; "deterministic" is built from the finding data when no
 *  model server is live — shown with an honest label rather than passed off as
 *  model output; "cached" was already stored for these exact findings. */
export interface ThreadFindingSummary {
  threadRef: string;
  content: string;
  source: "cached" | "model" | "deterministic" | string;
}

/** One past scan for the history list (no internal "chunks" — just what a user
 *  cares about: period, when, status, model, and what it found). */
export interface SafetyScanHistoryItem {
  id: number;
  model: string;
  rangeStart: number | null;
  rangeEnd: number | null;
  /** Which content the scan covered. */
  sources: "all" | "messages" | "notes" | string;
  /** 'interrupted' = a stranded 'running' row repaired at backup open. */
  status: "running" | "completed" | "cancelled" | "failed" | "interrupted";
  startedAt: number;
  finishedAt: number | null;
  findings: number;
  /** Live finding counts by severity for the row badge. */
  serious: number;
  harmful: number;
  concerning: number;
  /** Why a failed run failed — shown on hovering the history row's warning
   *  badge. Null for every other status: cancelled and interrupted explain
   *  themselves. */
  error: string | null;
}

/** Top live-finding severity per flagged thread/note, for inline badges. */
export interface FindingMarks {
  /** cache threads.id → highest severity (1|2|3). */
  threads: Record<number, 1 | 2 | 3>;
  /** cache notes.id → highest severity (1|2|3). */
  notes: Record<number, 1 | 2 | 3>;
}

/** One artifact and its shape.
 *
 *  The UI knows no artifact by name — it renders whatever the backend
 *  describes, which is what lets a new artifact appear with no frontend change
 *  at all. Same principle as the dashboard's METRIC_SOURCES. */
export type ArtifactSummary = {
  id: string;
  name: string;
  category: string | null;
  /** One sentence describing what this is, in plain language. */
  description: string;
  /** Which view hosts it. `standalone` is for data that fits nowhere else — the
   *  agreed rule is to fold into the view closest in meaning. */
  surface: "apps" | "contacts" | "device" | "standalone";
  /** How the host should present it. `facts` means ONE record whose columns are
   *  label/value pairs folded into the host's own summary — sixteen one-row tables
   *  is an absurd way to show sixteen device facts. */
  shape: "table" | "facts";
  /** The column whose value identifies the host row (a bundle id for Apps), so
   *  a host can attach rows without knowing what the artifact is. */
  joinColumn: string | null;
  /** What a host may show on the row itself, before anything is expanded —
   *  declared by the module, so a host never needs to know which artifact it is
   *  looking at. `null` means the row shows only a record count. */
  highlight: {
    column: string;
    whenColumn: string | null;
    whenAnyOf: string[];
    noneLabel: string | null;
  } | null;
  /** Column headers, in declared order. A row is an unordered map, so this is
   *  what keeps column order stable between artifacts and between runs. */
  columns: string[];
  /** Which of `columns` are timestamps, declared by the module's own `kind`.
   *
   *  The renderer used to infer this from values (does every number fall inside a
   *  plausible date range?) over a fact the module already states. The Bluetooth
   *  pairings module is why that mattered: its two counters are integers that
   *  must not be rendered as dates. */
  timestampColumns: string[];
  /** Which of `columns` are byte counts, declared by the module's `kind`. Rendered
   *  as human sizes — raw bytes are unreadable at the scale data usage reaches. */
  byteColumns: string[];
  /** Columns holding a number of SECONDS, formatted as a duration. */
  durationColumns: string[];
  rowCount: number;
  requiresEncryptedBackup: boolean;
};

/** One artifact row: column name → value. Already typed by the module's column
 *  spec; timestamps arrive as Unix seconds. */
export type ArtifactRow = Record<string, string | number | boolean | null>;

/** Why the Artifacts view might have nothing to show.
 *
 *  "The backup contained none" and "nobody has looked yet" are different facts.
 *  Saying the first when the second is true is a claim the user cannot check —
 *  and it is what a cache imported before the modules existed looks like. */
export type ExtractionState = "up-to-date" | "never-run" | "stale";

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
  /** Notified when a macOS setting changes (accent, appearance, text size), so
   *  the app adopts it immediately instead of at the next window focus. */
  onSystemChange(cb: (c: SystemChange) => void): Promise<UnlistenFn>;
  /** The accessibility text-size category as a multiplier for the type ramp. */
  systemTextScale(): Promise<number>;
  /** The colour macOS paints a selected row — kept separate from the accent. */
  systemSelectionColor(): Promise<string | null>;
  /** Display preferences to respect: reduce motion/transparency, increase
   *  contrast, differentiate without colour, and the sidebar icon size. */
  accessibilityPrefs(): Promise<AccessibilityPrefs>;
  /** Whether macOS Full Keyboard Access is on (System Settings → Keyboard →
   *  "Keyboard navigation"). With it off, native Tab visits only text fields
   *  and lists. */
  fullKeyboardAccess(): Promise<boolean>;
  /** Subscribe to the backend log stream over a Tauri Channel — the transport
   *  Tauri recommends for high-throughput ordered data (their event system
   *  explicitly is not). Batched and bounded backend-side. */
  subscribeLogs(cb: (b: LogBatch) => void): Promise<void>;
  /** Also write logs to a file on disk (off by default). */
  setFileLogging(enabled: boolean): Promise<void>;
  /** Where the file sink writes, for display in Settings. */
  logFilePath(): Promise<string | null>;
  /** Reveal the log file in Finder. */
  revealLogFile(): Promise<void>;
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
  /** Why each module ended up empty or not, from the import that built the
   *  open cache. See `use-parse-failed.ts` (#288). */
  moduleStatus(): Promise<ModuleStatus[]>;
  /** The macOS accent color as an oklch CSS string, or null when the host has
   *  no readable accent (non-macOS) — the UI then keeps its baked-in default. */
  systemAccentColor(): Promise<string | null>;
  listCalendarEvents(): Promise<CalendarEvent[]>;
  listReminders(): Promise<Reminder[]>;
  /** Artifacts this backup yielded, from the declarative modules. */
  listArtifacts(): Promise<ArtifactSummary[]>;
  /** Row index of a finding under these filters, or null when filtered out —
   *  for returning to the finding a conversation was opened from (#224). */
  contentFindingRank(
    scanId: number | null,
    /** The FILTERS only — a rank is a property of the order, not of a window. */
    page: Omit<ContentFindingPage, "offset" | "limit">,
    findingId: number,
  ): Promise<number | null>;
  /** Whether the stored rows came from the module set installed now. */
  artifactsExtractionState(): Promise<ExtractionState>;
  /** Run the modules against the already-open backup; returns any warnings. */
  extractArtifacts(): Promise<string[]>;
  getArtifactRows(artifactId: string, offset: number, limit: number): Promise<ArtifactRow[]>;
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
  /** Distinct content kinds present (with counts), for the content-filter pills.
   * `threadId` scopes to one conversation; otherwise all messages in `service`. */
  messageKinds(
    threadId?: number | null,
    service?: string | null,
  ): Promise<[kind: string, count: number][]>;
  /** Total messages in a thread; drives the lazily-loaded virtual scroller.
   * `kind` filters by content class (null=all); `search` matches body/sender. */
  countThreadMessages(
    threadId: number,
    kind?: string | null,
    search?: string | null,
  ): Promise<number>;
  /** A window of a thread's messages from `offset`; `desc` newest-first.
   *  `search` matches body/sender (in-conversation search). */
  getThreadMessageWindow(
    threadId: number,
    offset: number,
    limit: number,
    desc?: boolean,
    kind?: string | null,
    search?: string | null,
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
  /** Notes per time range, dated like the Notes view — so a Safety-Scan filter
   *  count and the Notes view agree for the same period. */
  countNoteRanges(ranges: TimeRange[]): Promise<number[]>;
  /** The earliest and latest dated message (Unix seconds), or null if none. */
  /** The home dashboard's tiles: every kind of data this backup yielded, with
   *  its count, span and sparkline. Loaded AFTER the home view paints — these
   *  aggregates must not land on the open-backup timing (#40). */
  /** The locale to FORMAT in: language from the user's language, region from
   *  their Region setting. The webview's own default drops the region override
   *  and answers e.g. `en-US` on a Mac set to Sweden (#161). */
  getSystemLocale(): Promise<string>;
  moduleMetrics(): Promise<ModuleMetric[]>;
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
  /** Fetch real App Store icons (data: URIs) for the given bundle ids from
   *  Apple's iTunes API. Opt-in — contacts a remote server. Returns only the
   *  apps it could resolve; cached on disk after the first fetch. */
  getAppIcons(bundleIds: string[]): Promise<AppIcon[]>;

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

  // --- Safety Scan (local-LLM content analysis; ADR 0002) ---
  /** Local model catalog + install state + the RAM-gated recommendation. */
  getSafetyScanModelStatus(): Promise<SafetyModelStatus>;
  /** Spin the sandboxed server up for a model, confirm it loads, tear it down.
   *  On-demand proof the local model actually runs on this Mac. */
  safetyScanHealthCheck(modelId?: string | null): Promise<SafetyHealthReport>;
  /** Download a catalog model (progress on `safetyscan://model-progress`). */
  downloadSafetyScanModel(modelId: string): Promise<void>;
  cancelSafetyScanModelDownload(): Promise<void>;
  /** The in-flight download, if any — lets the UI rehydrate after a refresh. */
  getSafetyScanDownloadStatus(): Promise<SafetyModelDownloadStatus | null>;
  /** The in-flight scan's last progress event, or null when none is running.
   *  Lets the UI re-attach after losing its state (a reload, a crash, or the
   *  webview respawning) instead of showing an idle view over a running scan. */
  getSafetyScanStatus(): Promise<SafetyScanEvent | null>;
  /** The in-flight import's last progress event and which backup it belongs to,
   *  or null when none is running. Lets the UI re-attach after a reload (#72). */
  getImportStatus(): Promise<{
    backupId: string;
    event: ImportProgress;
  } | null>;
  /** The in-flight security scan's last progress, or null when none runs (#72). */
  getSecurityScanStatus(): Promise<ScanProgress | null>;
  /** Module ids currently re-importing (#72). */
  getReimportStatus(): Promise<string[]>;
  /** Start a Safety Scan over the active backup. Progress arrives on
   *  `safetyscan://progress`; rejects if one is already running. */
  runSafetyScan(opts: {
    modelId?: string | null;
    rangeStart?: number | null;
    rangeEnd?: number | null;
    /** Which content to scan: "all" (default), "messages", or "notes". */
    sources?: string | null;
    /** Resume THIS scan (same history row, findings accumulate) instead of
     *  starting a new one; its stored scope is authoritative. */
    resumeScanId?: number | null;
  }): Promise<void>;
  cancelSafetyScan(): Promise<void>;
  onSafetyScanProgress(cb: (p: SafetyScanEvent) => void): Promise<UnlistenFn>;
  onSafetyModelProgress(
    cb: (p: SafetyModelProgressEvent) => void,
  ): Promise<UnlistenFn>;
  /** All Content Findings for the active backup (dismissed included). */
  /** Findings, most severe first; `scanId` restricts to one scan's. */
  /** One page of a scan's findings, filtered and ordered by SQLite (#65). */
  listContentFindings(
    scanId: number | undefined,
    page: ContentFindingPage,
  ): Promise<ContentFinding[]>;
  /** The pills' numbers, plus how many rows the given filter matches — counted
   *  with the same predicate as the page, so the two can never disagree. */
  countContentFindings(
    scanId: number | undefined,
    filter?: {
      severity?: 1 | 2 | 3;
      includeDismissed?: boolean;
      excludeStale?: boolean;
    },
  ): Promise<ContentFindingCounts>;
  /** The report's charts, aggregated in SQL over every finding the filter
   *  matches. Pass the SAME filter the list is showing — the aggregates share
   *  the list's scope predicate, so a chart and the rows beneath it always
   *  describe one population. */
  contentFindingAnalytics(
    scanId: number | undefined,
    filter?: {
      severity?: 1 | 2 | 3;
      includeDismissed?: boolean;
      excludeStale?: boolean;
    },
  ): Promise<FindingAnalytics>;
  /** The flagged source (text, sender, time, service) for a finding, fetched
   *  from the backup on demand. Null when the source row is gone or its id is
   *  stale after a re-import. */
  contentFindingSnippet(
    sourceKind: "message" | "note",
    sourceId: number | null,
  ): Promise<FindingSnippet | null>;
  /** Compact per-thread / per-note top severity for inline badges. */
  safetyScanFindingMarks(): Promise<FindingMarks>;
  /** Mark/unmark a finding as a false positive (keyed to survive re-scans). */
  dismissContentFinding(
    fingerprint: string,
    category: string,
    dismissed: boolean,
    /** Why. Recorded only when dismissing; the report shows it. */
    reason?: string,
  ): Promise<void>;
  /** Record that a finding's flagged text has been revealed. Called on expand —
   *  the one deliberate act that means it was read. One-way: collapsing does
   *  not un-read it. */
  markContentFindingSeen(fingerprint: string, category: string): Promise<void>;
  /** Dismiss a whole conversation or category, now and in future.
   *
   *  The rule DISMISSES what it covers rather than hiding it — a dismissed
   *  finding is counted, reachable and says why; a hidden one is gone. Returns
   *  how many existing findings it dismissed. */
  addSafetySuppression(
    scope: "thread" | "category",
    value: string,
    reason?: string,
  ): Promise<number>;
  listSafetySuppressions(): Promise<Suppression[]>;
  removeSafetySuppression(scope: string, value: string): Promise<void>;
  /** A scan's report + per-thread summaries. Latest scan when `scanId` is
   *  omitted, or a specific past scan from the history list. */
  getSafetyScanReport(scanId?: number): Promise<SafetyScanReport>;
  /** Generate (or fetch) ONE thread's summary on demand (#18). Scan end only
   *  writes prose for the top few threads by severity; this fills in the rest
   *  when the user opens one. Cached results are free. With no model server live
   *  it returns a deterministic summary built from the findings — `source` says
   *  which, so the UI can label it honestly. Null when the thread has no live
   *  findings in the scan's scope. */
  generateThreadSummary(
    scanId: number,
    threadRef: string,
  ): Promise<ThreadFindingSummary | null>;
  /** Past scans (newest first) for the history list. */
  listSafetyScans(): Promise<SafetyScanHistoryItem[]>;
  /** Remove a past scan and everything scoped to it. */
  deleteSafetyScan(scanId: number): Promise<void>;
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
  /**
   * `addresses` are the call peers whose CONTACT NAME matched `search`,
   * resolved client-side and matched by the backend with a plain `IN` (#279).
   *
   * The rows show names; the log stores addresses. Matching a typed name back
   * to an address needs phone normalisation, and that lives in exactly one
   * place — `use-contact-resolver.ts`, the same code that produced the name on
   * screen. A second normalisation in SQL could disagree with the first, and a
   * disagreement shows the WRONG person's calls.
   */
  countCalls(
    search: string | null,
    lo?: number | null,
    hi?: number | null,
    addresses?: string[] | null,
  ): Promise<number>;
  /** Call counts for each [lo, hi) window (respecting `search`). */
  countCallRanges(
    ranges: TimeRange[],
    search?: string | null,
    addresses?: string[] | null,
  ): Promise<number[]>;
  getCallsWindow(
    search: string | null,
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
    sortBy: string,
    desc: boolean,
    addresses?: string[] | null,
  ): Promise<Call[]>;
  /** Every distinct peer address in the call log, for name resolution (#279). */
  callAddresses(): Promise<string[]>;
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
  /** Evidence about messages that are gone from this backup. */
  messageDeletionEvidence(): Promise<DeletionEvidence>;
  /** Every Apple device that ever wrote Health data here, oldest first. */
  listDevicesUsed(): Promise<DeviceUse[]>;
  /** One row per (device, OS build) — an upgrade timeline, oldest first. */
  listDeviceOsHistory(): Promise<DeviceUse[]>;
  /** Count of Safari web searches matching search+range. */
  countSafariSearches(
    search: string | null,
    lo?: number | null,
    hi?: number | null,
  ): Promise<number>;
  countSafariSearchRanges(
    search: string | null,
    ranges: TimeRange[],
  ): Promise<number[]>;
  getSafariSearchesWindow(
    search: string | null,
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
    sortBy: string,
    desc: boolean,
  ): Promise<WebSearch[]>;
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

/**
 * Subscribe to a backend progress stream over a Tauri **Channel** rather than an
 * event (#65). Tauri's guidance is that events "are not designed for low latency
 * or high throughput" and that rapid events delivered to an async listener may be
 * processed OUT OF ORDER — for a progress stream that is a correctness problem,
 * not a cosmetic one.
 *
 * Keeps the `listen()` contract callers already use: returns an unlisten
 * function. Unlisten detaches the callback here rather than telling the backend
 * to forget the channel — each of these streams holds a single slot that the next
 * subscribe replaces, and a send into a detached channel is dropped. Subscribing
 * twice is therefore safe; the newest subscriber wins, which is exactly what a
 * webview reload needs.
 *
 * Every one of these streams pairs with a status-snapshot command that the UI
 * reads at mount. The snapshot answers "what is happening right now"; the stream
 * only carries what happens next. Anything emitted before subscribing is dropped
 * — as it was with events, where an emit reached only a live listener.
 */
async function subscribeStream<T>(
  command: string,
  cb: (payload: T) => void,
): Promise<UnlistenFn> {
  let live = true;
  const channel = new Channel<T>();
  channel.onmessage = (p) => {
    if (live) cb(p);
  };
  await invoke(command, { channel });
  return () => {
    live = false;
  };
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
    subscribeStream<ImportProgress>("subscribe_import_progress", cb),
  cancelImport: () => invoke("cancel_import"),
  setLogLevel: (level) => invoke("set_log_level", { level }),
  setBiometricRequired: (enabled) =>
    invoke("set_biometric_required", { enabled }),
  appSigningStatus: () => invoke<SigningStatus>("app_signing_status"),
  onSystemChange: (cb) => subscribeStream<SystemChange>("subscribe_system_changes", cb),
  systemTextScale: () => invoke<number>("get_system_text_scale"),
  fullKeyboardAccess: () => invoke<boolean>("get_full_keyboard_access"),
  accessibilityPrefs: () =>
    invoke<AccessibilityPrefs>("get_accessibility_prefs"),
  systemSelectionColor: () =>
    invoke<string | null>("get_system_selection_color"),
  subscribeLogs: async (cb) => {
    const channel = new Channel<LogBatch>();
    channel.onmessage = cb;
    await invoke("subscribe_logs", { channel });
  },
  setFileLogging: (enabled) => invoke("set_file_logging", { enabled }),
  logFilePath: () => invoke<string | null>("log_file_path"),
  revealLogFile: () => invoke("reveal_log_file"),
  hasActiveBackup: () => invoke<boolean>("has_active_backup"),
  closeBackup: () => invoke<void>("close_backup"),
  openBackup: (backupId) => invoke<boolean>("open_backup", { backupId }),
  forgetBackup: (backupId) => invoke<void>("forget_backup", { backupId }),
  importedBackupIds: () => invoke<string[]>("imported_backup_ids"),
  listThreads: () => invoke<ThreadSummary[]>("list_threads"),
  deviceInfo: () => invoke<BackupInfo | null>("device_info"),
  moduleStatus: () => invoke<ModuleStatus[]>("module_status"),
  systemAccentColor: () => invoke<string | null>("get_system_accent_color"),
  listCalendarEvents: () => invoke<CalendarEvent[]>("list_calendar_events"),
  listReminders: () => invoke<Reminder[]>("list_reminders"),
  listArtifacts: () => invoke<ArtifactSummary[]>("list_artifacts"),
  contentFindingRank: (scanId, page, findingId) =>
    invoke<number | null>("content_finding_rank", {
      scanId,
      severity: page.severity,
      includeDismissed: page.includeDismissed,
      sortBy: page.sortBy,
      desc: page.desc,
      groupByThread: page.groupByThread,
      excludeStale: page.excludeStale ?? false,
      findingId,
    }),
  artifactsExtractionState: () => invoke<ExtractionState>("artifacts_extraction_state"),
  extractArtifacts: () => invoke<string[]>("extract_artifacts"),
  getArtifactRows: (artifactId, offset, limit) =>
    invoke<ArtifactRow[]>("get_artifact_rows", { artifactId, offset, limit }),
  listWorkouts: () => invoke<Workout[]>("list_workouts"),
  workoutRoute: (workoutId) => invoke<RoutePoint[]>("workout_route", { workoutId }),
  healthDaily: () => invoke<HealthDay[]>("health_daily"),
  listSleep: () => invoke<SleepSession[]>("list_sleep"),
  listHealthTimezones: () => invoke<HealthTimezone[]>("list_health_timezones"),
  listHealthAchievements: () =>
    invoke<HealthAchievement[]>("list_health_achievements"),
  listCycle: () => invoke<CycleEntry[]>("list_cycle"),
  healthSummary: () => invoke<HealthSummary>("health_summary"),
  messageKinds: (threadId = null, service = null) =>
    invoke<[string, number][]>("message_kinds", {
      threadId: threadId ?? null,
      service: service ?? null,
    }),
  countThreadMessages: (threadId, kind = null, search = null) =>
    invoke<number>("count_thread_messages", {
      threadId,
      kind: kind ?? null,
      search: search ?? null,
    }),
  getThreadMessageWindow: (
    threadId,
    offset,
    limit,
    desc = false,
    kind = null,
    search = null,
  ) =>
    invoke<Message[]>("get_thread_message_window", {
      threadId,
      offset,
      limit,
      desc,
      kind: kind ?? null,
      search: search ?? null,
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
  countNoteRanges: (ranges) =>
    invoke<number[]>("count_note_ranges", { ranges }),
  getSystemLocale: () => invoke<string>("get_system_locale"),
  moduleMetrics: () => invoke<ModuleMetric[]>("module_metrics"),
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
  countCalls: (search, lo = null, hi = null, addresses = null) =>
    invoke<number>("count_calls", { search, lo, hi, addresses }),
  countCallRanges: (ranges, search = null, addresses = null) =>
    invoke<number[]>("count_call_ranges", {
      ranges,
      search: search ?? null,
      addresses,
    }),
  getCallsWindow: (search, lo, hi, offset, limit, sortBy, desc, addresses = null) =>
    invoke<Call[]>("get_calls_window", {
      search,
      lo,
      hi,
      offset,
      limit,
      sortBy,
      desc,
      addresses,
    }),
  callAddresses: () => invoke<string[]>("call_addresses"),
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
  messageDeletionEvidence: () =>
    invoke<DeletionEvidence>("message_deletion_evidence"),
  listDevicesUsed: () => invoke<DeviceUse[]>("list_devices_used"),
  listDeviceOsHistory: () => invoke<DeviceUse[]>("list_device_os_history"),
  countSafariSearches: (search, lo = null, hi = null) =>
    invoke<number>("count_safari_searches", { search, lo, hi }),
  countSafariSearchRanges: (search, ranges) =>
    invoke<number[]>("count_safari_search_ranges", { search, ranges }),
  getSafariSearchesWindow: (search, lo, hi, offset, limit, sortBy, desc) =>
    invoke<WebSearch[]>("get_safari_searches_window", {
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
  getAppIcons: (bundleIds) =>
    invoke<AppIcon[]>("get_app_icons", { bundleIds }),

  runSecurityScan: (kind) =>
    invoke<ScanSummary>("run_security_scan", { kind }),
  cancelScan: () => invoke("cancel_scan"),
  onScanProgress: (cb) =>
    subscribeStream<ScanProgress>("subscribe_security_progress", cb),
  listScanRuns: () => invoke<ScanRun[]>("list_scan_runs"),
  latestScanRun: () => invoke<number | null>("latest_scan_run"),
  listFindings: (runId, minSeverity, module) =>
    invoke<Finding[]>("list_findings", {
      runId,
      minSeverity: minSeverity ?? null,
      module: module ?? null,
    }),
  getSafetyScanModelStatus: () =>
    invoke<SafetyModelStatus>("get_safety_scan_model_status"),
  safetyScanHealthCheck: (modelId) =>
    invoke<SafetyHealthReport>("safety_scan_health_check", {
      modelId: modelId ?? null,
    }),
  downloadSafetyScanModel: (modelId) =>
    invoke("download_safety_scan_model", { modelId }),
  getSafetyScanDownloadStatus: () =>
    invoke<SafetyModelDownloadStatus | null>("get_safety_scan_download_status"),
  cancelSafetyScanModelDownload: () =>
    invoke("cancel_safety_scan_model_download"),
  getSafetyScanStatus: () =>
    invoke<SafetyScanEvent | null>("get_safety_scan_status"),
  getImportStatus: () =>
    invoke<{ backupId: string; event: ImportProgress } | null>(
      "get_import_status",
    ),
  getSecurityScanStatus: () =>
    invoke<ScanProgress | null>("get_security_scan_status"),
  getReimportStatus: () => invoke<string[]>("get_reimport_status"),
  runSafetyScan: (opts) =>
    invoke("run_safety_scan", {
      modelId: opts.modelId ?? null,
      rangeStart: opts.rangeStart ?? null,
      rangeEnd: opts.rangeEnd ?? null,
      sources: opts.sources ?? null,
      resumeScanId: opts.resumeScanId ?? null,
    }),
  cancelSafetyScan: () => invoke("cancel_safety_scan"),
  onSafetyScanProgress: (cb) =>
    subscribeStream<SafetyScanEvent>("subscribe_safety_scan_progress", cb),
  onSafetyModelProgress: (cb) =>
    subscribeStream<SafetyModelProgressEvent>(
      "subscribe_safety_model_progress",
      cb,
    ),
  listContentFindings: (scanId, page) =>
    invoke<ContentFinding[]>("list_content_findings", {
      scanId: scanId ?? null,
      severity: page.severity ?? null,
      includeDismissed: page.includeDismissed,
      sortBy: page.sortBy,
      desc: page.desc,
      groupByThread: page.groupByThread,
      excludeStale: page.excludeStale ?? false,
      offset: page.offset,
      limit: page.limit,
    }),
  countContentFindings: (scanId, filter) =>
    invoke<ContentFindingCounts>("count_content_findings", {
      scanId: scanId ?? null,
      severity: filter?.severity ?? null,
      includeDismissed: filter?.includeDismissed ?? false,
      excludeStale: filter?.excludeStale ?? false,
    }),
  contentFindingAnalytics: (scanId, filter) =>
    invoke<FindingAnalytics>("content_finding_analytics", {
      scanId: scanId ?? null,
      severity: filter?.severity ?? null,
      includeDismissed: filter?.includeDismissed ?? false,
      excludeStale: filter?.excludeStale ?? false,
    }),
  contentFindingSnippet: (sourceKind, sourceId) =>
    invoke<FindingSnippet | null>("content_finding_snippet", {
      sourceKind,
      sourceId: sourceId ?? null,
    }),
  safetyScanFindingMarks: () =>
    invoke<FindingMarks>("safety_scan_finding_marks"),
  dismissContentFinding: (fingerprint, category, dismissed, reason) =>
    invoke("dismiss_content_finding", {
      fingerprint,
      category,
      dismissed,
      reason: reason ?? null,
    }),
  markContentFindingSeen: (fingerprint, category) =>
    invoke("mark_content_finding_seen", { fingerprint, category }),
  addSafetySuppression: (scope, value, reason) =>
    invoke<number>("add_safety_suppression", { scope, value, reason: reason ?? null }),
  listSafetySuppressions: () =>
    invoke<Suppression[]>("list_safety_suppressions"),
  removeSafetySuppression: (scope, value) =>
    invoke("remove_safety_suppression", { scope, value }),
  getSafetyScanReport: (scanId) =>
    invoke<SafetyScanReport>("get_safety_scan_report", {
      scanId: scanId ?? null,
    }),
  generateThreadSummary: (scanId, threadRef) =>
    invoke<ThreadFindingSummary | null>("generate_thread_summary", {
      scanId,
      threadRef,
    }),
  listSafetyScans: () =>
    invoke<SafetyScanHistoryItem[]>("list_safety_scans"),
  deleteSafetyScan: (scanId) =>
    invoke("delete_safety_scan", { scanId }),

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

// A thread's mock messages, optionally filtered by an in-conversation search
// (body/sender), mirroring the backend's LIKE filter.
function mockThreadMessages(threadId: number, search?: string | null): Message[] {
  const all = mockMessages[threadId] ?? [];
  const q = search?.trim().toLowerCase();
  if (!q) return all;
  return all.filter(
    (m) =>
      (m.body ?? "").toLowerCase().includes(q) ||
      (m.sender ?? "").toLowerCase().includes(q),
  );
}

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
    profile: "Default",
    synced: false,
    redirectSource: null,
    redirectDestination: null,
  },
  {
    id: 2,
    url: "https://news.ycombinator.com/",
    title: "Hacker News",
    visitedAt: 1717797600,
    visitCount: 34,
    deleted: false,
    profile: "Default",
    // Browsed on another device signed into the same iCloud account.
    synced: true,
    redirectSource: null,
    redirectDestination: null,
  },
  {
    id: 3,
    url: "https://www.apple.com/",
    title: "Apple",
    visitedAt: 1717794000,
    visitCount: 12,
    deleted: false,
    // A second Safari profile, so the profile badge has something to render.
    profile: "Work",
    synced: false,
    redirectSource: "https://t.co/9dK2p",
    redirectDestination: null,
  },
  {
    id: 4,
    url: "https://secret.example/cleared",
    title: null,
    visitedAt: 1717790000,
    visitCount: null,
    deleted: true,
    profile: "Default",
    synced: false,
    redirectSource: null,
    redirectDestination: null,
  },
];

const mockDeviceUse: DeviceUse[] = [
  // Per (device, build). The rollup and the OS history are both derived from
  // this in the mock exactly as they are in SQL, so the two cannot disagree here
  // in a way they would not in the real client.
  { model: "iPhone10,4", osBuild: "17G80", firstAt: 1598373708, lastAt: 1610000000, samples: 7498 },
  { model: "iPhone12,1", osBuild: "20B110", firstAt: 1688243583, lastAt: 1706000000, samples: 5100 },
  { model: "iPhone12,1", osBuild: "21D50", firstAt: 1706104059, lastAt: 1722629759, samples: 7119 },
  // Single sample: a device owned, but its window dates no upgrade.
  { model: "Watch4,3", osBuild: "20U502", firstAt: 1620929821, lastAt: 1620929821, samples: 2 },
];

const mockSafariSearches: WebSearch[] = [
  {
    id: 1,
    term: "mission peak trail conditions",
    searchedAt: 1717801100,
    source: "visited",
    engine: "google.com",
    url: "https://www.google.com/search?q=mission+peak+trail+conditions",
    profile: "Default",
  },
  {
    id: 2,
    term: "tor browser",
    searchedAt: 1717799000,
    source: "visited",
    engine: "duckduckgo.com",
    url: "https://duckduckgo.com/?q=tor+browser",
    profile: "Default",
  },
  // Typed but never opened: no URL, so the row is not clickable.
  {
    id: 3,
    term: "digitalcorpora",
    searchedAt: 1717790500,
    source: "typed",
    engine: null,
    url: null,
    profile: null,
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
  { bundleId: "net.whatsapp.WhatsApp", name: "WhatsApp Messenger", seller: "WhatsApp Inc.", version: "23.24.0", genre: "Social Networking", released: "2009-05-03T00:00:00Z", downloaded: "2023-11-02T09:14:00Z", appleId: "jane.doe@icloud.com", contentRating: "17+", subgenre: null },
  { bundleId: "com.burbn.instagram", name: "Instagram", seller: "Instagram, Inc.", version: "436.0.0", genre: "Photo & Video", released: "2010-10-06T08:12:41Z", downloaded: "2024-03-12T18:41:00Z", appleId: "jane.doe@icloud.com", contentRating: "12+", subgenre: "Social Networking" },
  { bundleId: "com.toyopagroup.picaboo", name: "Snapchat", seller: "Snap, Inc.", version: "12.80.0", genre: "Photo & Video", released: "2011-07-13T00:00:00Z", downloaded: "2024-01-20T12:00:00Z", appleId: "jane.doe@icloud.com", contentRating: "12+", subgenre: null },
  { bundleId: "com.zhiliaoapp.musically", name: "TikTok", seller: "TikTok Ltd.", version: "34.1.0", genre: "Entertainment", released: "2014-04-01T00:00:00Z", downloaded: "2024-05-30T21:05:00Z", appleId: "jane.doe@icloud.com", contentRating: "17+", subgenre: "Social Networking" },
  { bundleId: "org.telegram.messenger", name: "Telegram Messenger", seller: "Telegram FZ-LLC", version: "10.5.1", genre: "Social Networking", released: "2013-08-14T00:00:00Z", downloaded: "2022-06-15T07:30:00Z", appleId: "jane.doe@icloud.com", contentRating: "17+", subgenre: null },
  { bundleId: "com.spotify.client", name: "Spotify - Music and Podcasts", seller: "Spotify Ltd.", version: "8.9.10", genre: "Music", released: "2011-07-14T00:00:00Z", downloaded: "2021-02-01T00:00:00Z", appleId: "jane.doe@icloud.com", contentRating: "12+", subgenre: null },
  { bundleId: "com.google.Gmail", name: "Gmail - Email by Google", seller: "Google LLC", version: "6.0.240107", genre: "Productivity", released: "2011-11-02T00:00:00Z", downloaded: "2020-09-10T00:00:00Z", appleId: "jane.doe@icloud.com", contentRating: "4+", subgenre: null },
  { bundleId: "com.tinyspeck.chatlyio", name: "Slack", seller: "Slack Technologies Inc.", version: "23.11.90", genre: "Business", released: "2013-08-21T00:00:00Z", downloaded: "2023-04-18T00:00:00Z", appleId: "jane.doe@icloud.com", contentRating: "4+", subgenre: null },
  { bundleId: "com.ubercab.UberClient", name: "Uber - Request a ride", seller: "Uber Technologies, Inc.", version: "3.577.10", genre: "Travel", released: "2010-08-05T00:00:00Z", downloaded: "2024-02-02T00:00:00Z", appleId: "jane.doe@icloud.com", contentRating: "4+", subgenre: null },
  { bundleId: "com.acme.nannycam", name: "Nanny Cam Viewer", seller: "Acme Security", version: "2.1.0", genre: "Utilities", released: "2020-01-01T00:00:00Z", downloaded: "2024-06-01T14:22:00Z", appleId: "unknown-account@outlook.com", contentRating: "4+", subgenre: null },
  { bundleId: "com.apple.mobilesafari", name: null, seller: null, version: null, genre: null, released: null, downloaded: null, appleId: null, contentRating: null, subgenre: null },
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
  customIndicatorDir: null,
};

let mockDeshortenAutoApprove = false;

/**
 * Dev-only list inflater for the mock client: repeats a fixture list until it has
 * N entries. Virtualization is a claim about behaviour at scale, and a 5-row
 * fixture cannot test it — this makes "does this rail actually virtualize?" a
 * repeatable check rather than a promise (#67).
 *
 * Set it with `localStorage.setItem("traceloupe-mock-bulk", "4000")`. NOT a URL
 * parameter: the router validates search params and strips unknown ones on the
 * first navigation, so a `?bulk=` knob silently stops applying the moment you
 * leave the landing route — which reads exactly like a passing test.
 */
function mockBulk<T>(rows: T[], renumber: (row: T, i: number) => T): T[] {
  if (typeof localStorage === "undefined" || !rows.length) return rows;
  const n = Number(localStorage.getItem("traceloupe-mock-bulk") ?? 0);
  if (!Number.isFinite(n) || n <= rows.length) return rows;
  return Array.from({ length: n }, (_, i) => renumber(rows[i % rows.length], i));
}

let mockScanRuns: ScanRun[] = [];

let mockSafetyModelInstalled = false;
/** Findings spread over ~10 months across four conversations and a note.
 *
 *  The spread is the point: two findings a fortnight apart can't tell you
 *  whether the report's charts bucket, rank, fill their gaps or split confirmed
 *  from unconfirmed correctly (#66). This fixture covers a month-bucketed span
 *  with quiet months in it, several categories and severities, both cascade
 *  tiers, and the three findings that are supposed to be treated specially — a
 *  dismissed one, a stale one, and one with no date at all.
 *
 *  Thread identifiers are the real mock threads', so the labels resolve to names
 *  the way they do against a backup. */
const mockContentFindings: ContentFinding[] = (
  [
    // [daysAgo, thread, category, severity, rechecked]
    [3, "tiktok", "scam-fraud", 2, false, "Unsolicited crypto investment pitch pushing urgent transfer."],
    [12, "alex", "coercive-control", 2, true, "Demands constant location sharing and account passwords."],
    [18, "sam", "sexual-content", 1, false, "Sexually explicit exchange; both parties appear adult."],
    [20, "alex", "coercive-control", 3, true, "Threatens to cut off money and contact if she leaves."],
    [26, "alex", "harassment-bullying", 2, false, "Repeated insults after being asked to stop."],
    [33, "hiking", "threat-violence", 2, true, "Talk of 'sorting him out' after the argument."],
    [40, "alex", "coercive-control", 2, true, "Insists on reading her messages every evening."],
    [55, "sam", "sexual-content", 1, false, "Explicit image request; recipient declines."],
    [70, "hiking", "threat-violence", 3, true, "Explicit threat to turn up at someone's home."],
    [95, "tiktok", "scam-fraud", 1, false, "Link to a lookalike wallet site."],
    [120, "alex", "coercive-control", 3, true, "Tracks her whereabouts and confronts her about them."],
    [150, "note", "self-harm", 2, true, "Journal entry describing self-harm ideation."],
    [185, "sam", "harassment-bullying", 1, false, "Name-calling in a group thread."],
    [210, "hiking", "harassment-bullying", 2, false, "Pile-on aimed at one member of the group."],
    [250, "alex", "coercive-control", 2, true, "Controls who she is allowed to see at weekends."],
    [300, "tiktok", "scam-fraud", 2, false, "Account-recovery phish impersonating support."],
  ] as const
).map(([daysAgo, who, category, severity, rechecked, rationale], i) => {
  const thread = {
    alex: { threadId: 1, identifier: "12", service: "iMessage" },
    sam: { threadId: 2, identifier: "8", service: "SMS" },
    hiking: { threadId: 4, identifier: "20", service: "iMessage" },
    tiktok: {
      threadId: 5,
      identifier: "0:1:179546233697390592:7145206438070666245",
      service: "TikTok",
    },
    note: { threadId: null, identifier: null, service: "Notes" },
  }[who];
  return {
    id: i + 1,
    sourceKind: who === "note" ? ("note" as const) : ("message" as const),
    sourceId: who === "note" ? 1 : 2 + i,
    threadId: thread.threadId,
    threadIdentifier: thread.identifier,
    service: thread.service,
    occurredAt: Math.floor(Date.now() / 1000) - 86_400 * daysAgo,
    fingerprint: `mockfp-${category}-${i}`,
    category,
    severity,
    rationale,
    // One stale (source content gone: the report drops it, the panel keeps it)
    // and one dismissed, so the two disclosures the charts owe the reader are
    // never zero in the mock.
    stale: daysAgo === 33,
    dismissed: daysAgo === 18,
    // A few already read, so the mock shows both states — a fixture where
    // everything is unread would never exercise the read styling.
    seen: daysAgo === 18 || daysAgo === 12 || daysAgo === 40,
    dismissReason: daysAgo === 18 ? "Both adults, consensual" : null,
    rechecked,
  } satisfies ContentFinding;
});

// A finding with no date. It cannot sit on a timeline, so the charts count it
// everywhere else and say so — the case that would otherwise leave the chart's
// total quietly disagreeing with the list's.
// A timestamp that decoded wrong: Apple stores seconds since 2001, and a zeroed
// or mis-read column lands in 1970. It is not a date, so it belongs with the
// undatable ones — before the window existed, this one finding stretched the
// axis across half a century and squashed the other sixteen into one bar.
mockContentFindings.push({
  id: mockContentFindings.length + 1,
  sourceKind: "message",
  sourceId: 99,
  threadId: 2,
  threadIdentifier: "8",
  service: "SMS",
  occurredAt: 0,
  fingerprint: "mockfp-harassment-epoch",
  category: "harassment-bullying",
  severity: 1,
  rationale: "Message whose timestamp did not decode.",
  stale: false,
  dismissed: false,
  seen: false,
  dismissReason: null,
  rechecked: false,
});

mockContentFindings.push({
  id: mockContentFindings.length + 1,
  sourceKind: "note",
  sourceId: 2,
  threadId: null,
  threadIdentifier: null,
  service: "Notes",
  occurredAt: null,
  fingerprint: "mockfp-self-harm-undated",
  category: "self-harm",
  severity: 1,
  rationale: "Undated note referring to wanting to disappear.",
  stale: false,
  dismissed: false,
  seen: false,
  dismissReason: null,
  rechecked: false,
});

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

let mockSuppressions: Suppression[] = [];
let mockActive = false;
const mockImported = new Set<string>();
// Scan-history rows the browser mock has "deleted", so delete + re-list behaves.
const mockDeletedScanIds = new Set<number>();

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
/** `?mock=no-data` renders a backup that imported cleanly and simply holds
 *  nothing — the one case where "No calls in this backup." is the TRUE thing to
 *  say. It exists so check-filtered-empty can assert the honest message still
 *  appears; without that half, "never say it" would pass the guard (#278). */
const mockNoData =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("mock") === "no-data";

/** `?mock=parse-failed` renders the app as it looks when a store WAS in the
 *  backup and could not be read (#288) — the one empty-state reason that had
 *  no way to be seen, and so no way to be guarded. */
const mockParseFailed =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("mock") === "parse-failed";

function mockFilterCalls(
  search: string | null,
  range?: TimeRange,
  addresses?: string[] | null,
): Call[] {
  // A store that failed to parse produced no rows -- that is the whole point of
  // the state. Returning calls here would make the mock incoherent and the
  // guard unable to reach the empty view it exists to measure.
  if (mockParseFailed || mockNoData) return [];
  let out = mockCalls;
  if (search) {
    const q = search.toLowerCase();
    const byName = new Set(addresses ?? []);
    // Mirrors the SQL: substring on the address OR an exact address the client
    // matched by contact name (#279).
    out = out.filter(
      (c) =>
        c.address?.toLowerCase().includes(q) ||
        (c.address != null && byName.has(c.address)),
    );
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
function mockFilterSafariSearches(
  search: string | null,
  range?: TimeRange,
): WebSearch[] {
  let out = mockSafariSearches;
  if (search) {
    const q = search.toLowerCase();
    out = out.filter(
      (w) =>
        w.term.toLowerCase().includes(q) ||
        (w.engine?.toLowerCase().includes(q) ?? false),
    );
  }
  if (range && (range.lo != null || range.hi != null)) {
    out = out.filter((w) => inRange(w.searchedAt ?? null, range));
  }
  return out;
}
function mockFilterSafari(
  search: string | null,
  range?: TimeRange,
): HistoryVisit[] {
  if (mockNoData) return [];
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
const safariSearchKey = (by: string) => (w: WebSearch) =>
  by === "term" ? w.term : by === "engine" ? w.engine : w.searchedAt;
const safariKey = (by: string) => (h: HistoryVisit) =>
  by === "title" ? h.title : by === "visits" ? h.visitCount : h.visitedAt;

// A mock progress emitter so the import flow is exercisable in the browser.
type ProgressCb = (p: ImportProgress) => void;
const mockProgressSubs = new Set<ProgressCb>();

/** Which findings a mock scan holds. One definition for the list and the count,
 *  because two would drift exactly the way #59 did. */
/** The live severity split of the whole fixture — what a scan that saw all of it
 *  reports on its history card. */
function mockScanTotals() {
  const live = mockContentFindings.filter((f) => !f.dismissed);
  return {
    findings: live.length,
    serious: live.filter((f) => f.severity === 3).length,
    harmful: live.filter((f) => f.severity === 2).length,
    concerning: live.filter((f) => f.severity === 1).length,
  };
}

function mockFindingsForScan(scanId?: number) {
  if (!mockActive) return [];
  // Mock scan 1 found only the first finding; scans 2 and 4 found none;
  // scan 3 (latest) found everything.
  if (scanId === 1) return mockContentFindings.slice(0, 1);
  if (scanId === 2 || scanId === 4) return [];
  return mockBulk(mockContentFindings, (f, i) => ({ ...f, id: 900000 + i }));
}

const mockEngineSubs = new Set<(p: EngineProgress) => void>();

export /** Mock-only: `?mock=unencrypted` flips the fake backup to an unencrypted one,
 *  so the encrypted-only empty states can be exercised by the design checks. */
/** Mock-only: flips once extractArtifacts() is called, so the never-run →
 *  extracted transition can be driven in the design checks. */
let mockArtifactsExtracted = false;

const mockUnencrypted =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("mock") === "unencrypted";


const mockClient: TraceLoupeClient = {
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
  onSystemChange: async () => () => {},
  systemTextScale: async () => 1,
  fullKeyboardAccess: async () => false,
  accessibilityPrefs: async () => ({
    reduceMotion: false,
    reduceTransparency: false,
    increaseContrast: false,
    differentiateWithoutColor: false,
    sidebarIconSize: 2,
    showScrollBars: "automatic",
  }),
  systemSelectionColor: async () => null,
  subscribeLogs: async () => {},
  setFileLogging: async () => {},
  logFilePath: async () => "/tmp/traceloupe.log",
  revealLogFile: async () => {},
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
          // `?mock=unencrypted` renders the app as it looks against a backup
          // made without encryption, where Health and iCloud
          // tabs cannot exist. Mock-only: this client is never used inside
          // Tauri, so it cannot affect the shipped app.
          isEncrypted: !mockUnencrypted,
        }
      : null,
  moduleStatus: async () =>
    !mockActive
      ? []
      : mockParseFailed
        ? [
            {
              module: "calls",
              status: "failed" as const,
              detail:
                "Native Calls: couldn't read CallHistory.storedata (file is not a database); using iLEAPP.",
            },
            { module: "messages", status: "parsed" as const, detail: null },
          ]
        : [
            { module: "messages", status: "parsed" as const, detail: null },
            { module: "calls", status: "parsed" as const, detail: null },
            { module: "contacts", status: "parsed" as const, detail: null },
            { module: "safari", status: "parsed" as const, detail: null },
            { module: "notes", status: "parsed" as const, detail: null },
          ],
  systemAccentColor: async () => null,
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
    mockActive && !mockUnencrypted
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
    mockActive && !mockUnencrypted
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
    mockActive && !mockUnencrypted
      ? [
          { id: 1, startAt: 1717822800, endAt: 1717851600, stage: "In Bed" },
          { id: 2, startAt: 1717824600, endAt: 1717837200, stage: "Deep" },
          { id: 3, startAt: 1717737600, endAt: 1717763400, stage: "In Bed" },
        ]
      : [],
  listHealthAchievements: async () =>
    mockActive && !mockUnencrypted
      ? [
          { id: 1, name: "NewMoveRecord", earnedAt: 1717804800, value: 1284, unit: "kcal" },
          { id: 2, name: "MoveGoal200Percent", earnedAt: 1717718400, value: 400, unit: "kcal" },
          { id: 3, name: "PerfectWeekMove", earnedAt: 1716854400, value: 7, unit: "count" },
        ]
      : [],
  listCycle: async () =>
    mockActive && !mockUnencrypted
      ? [
          { id: 1, category: "Menstrual flow", detail: "Medium", loggedAt: 1717718400 },
          { id: 2, category: "Abdominal cramps", detail: null, loggedAt: 1717718400 },
          { id: 3, category: "Mood changes", detail: null, loggedAt: 1717632000 },
          { id: 4, category: "Menstrual flow", detail: "Light", loggedAt: 1717632000 },
        ]
      : [],
  listHealthTimezones: async () =>
    mockActive && !mockUnencrypted
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
    mockActive && !mockUnencrypted
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
  artifactsExtractionState: async () => (mockArtifactsExtracted ? "up-to-date" : "never-run"),
  extractArtifacts: async () => {
    mockArtifactsExtracted = true;
    return [];
  },
  contentFindingRank: async (_scanId, _page, findingId) =>
    mockActive && findingId > 0 ? 1 : null,
  listArtifacts: async () =>
    mockActive && mockArtifactsExtracted
      ? [
          {
            id: "tcc",
            name: "Permissions",
            category: "Security",
            description:
              "Which apps were allowed to use the camera, microphone, photos, contacts and location — and when that was decided.",
            surface: "apps" as const,
            shape: "table" as const,
            joinColumn: "App",
            highlight: {
              column: "Permission",
              whenColumn: "Decision",
              whenAnyOf: ["Allowed", "Limited"],
              noneLabel: "none granted",
            },
            columns: ["App", "Permission", "Decision", "Decided"],
            timestampColumns: ["Decided"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 5,
            requiresEncryptedBackup: false,
          },
          {
            id: "location_clients",
            name: "Location access",
            category: "Security",
            description:
              "Apps and services that asked this iPhone for its location, and when each last stopped receiving it.",
            surface: "apps" as const,
            shape: "table" as const,
            joinColumn: "App",
            highlight: null,
            columns: [
              "App",
              "Client",
              "Bundle path",
              "Registered",
              "Stopped receiving",
              "Location stopped",
            ],
            timestampColumns: ["Stopped receiving", "Location stopped"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 3,
            requiresEncryptedBackup: false,
          },
          {
            id: "data_usage",
            name: "Data usage",
            category: "Network",
            description:
              "How much data each app sent and received, over Wi-Fi and cellular, and which system process carried it.",
            surface: "apps" as const,
            shape: "table" as const,
            joinColumn: "App",
            highlight: null,
            columns: [
              "App",
              "Carried by",
              "Cellular down",
              "Cellular up",
              "Wi-Fi down",
              "Wi-Fi up",
              "Records",
              "First",
              "Last",
            ],
            timestampColumns: ["First", "Last"],
            byteColumns: ["Cellular down", "Cellular up", "Wi-Fi down", "Wi-Fi up"],
            durationColumns: [],
            rowCount: 4,
            requiresEncryptedBackup: false,
          },
          // The two device-surface modules, with the shapes the real ones
          // declare. Values mirror what `explore_real_backup` printed for Josh
          // Hickman's public iOS 17 image, with the names changed — a mock that
          // does not match the real shape is how the hosted path came to render
          // nothing in the app while looking correct here (#232).
          {
            id: "accounts",
            name: "Accounts",
            category: "Device",
            description:
              "Which services are signed in on this device — mail, calendars, iCloud, Game Center and more — with when each was added.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Service",
              "Account",
              "Label",
              "Part of",
              "Added",
              "Status",
              "Signed in",
              "Registered by",
            ],
            timestampColumns: ["Added"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 4,
            requiresEncryptedBackup: false,
          },
          {
            id: "home_screen",
            name: "Home screen",
            category: "Device",
            description:
              "Which apps, widgets and folders are on the home screen, and which page each is on.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Page", "Identifier", "Kind", "Size"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 3,
            requiresEncryptedBackup: false,
          },
          {
            id: "dock",
            name: "Dock",
            category: "Device",
            description: "The apps kept in the dock, one tap away, in the order they appear.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Position", "App"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "alltrails",
            name: "AllTrails recordings",
            category: "Locations",
            description:
              "Outdoor activities recorded in AllTrails — what was walked or hiked, when, how far, and roughly where.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Activity",
              "Started",
              "Ended",
              "Distance (m)",
              "Moving time (s)",
              "Total time (s)",
              "Climb (m)",
              "Calories",
              "Roughly where (lat)",
              "Roughly where (lon)",
              "Private",
              "Segments",
            ],
            timestampColumns: ["Started", "Ended"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "podcasts",
            name: "Podcasts",
            category: "Media",
            description:
              "Podcast shows subscribed to on this device, who publishes each, and when one was last listened to.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Show",
              "Published by",
              "Category",
              "Subscribed",
              "Added",
              "Last played",
              "Feed",
            ],
            timestampColumns: ["Added", "Last played"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "backup_sizing",
            name: "Backup size by domain",
            category: "Device",
            description:
              "How much of this backup each part of the device accounts for, as iOS measured it before writing the backup.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Domain", "Size"],
            timestampColumns: [],
            byteColumns: ["Size"],
            durationColumns: [],
            rowCount: 3,
            requiresEncryptedBackup: false,
          },
          {
            id: "watch_apps",
            name: "Apple Watch apps",
            category: "Device",
            description:
              "Apps installed on the Apple Watch paired with this iPhone, and which iPhone app each belongs to.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "App",
              "Companion app",
              "Version",
              "Build",
              "On the watch",
              "Minimum watchOS",
              "Paired device",
            ],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "bluetooth_nearby",
            name: "Nearby Bluetooth",
            category: "Device",
            description:
              "Low-energy Bluetooth devices this iPhone saw in range but never paired with — other people's devices, trackers and appliances.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Device", "Address", "Seen counter", "Identifier"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 4,
            requiresEncryptedBackup: false,
          },
          {
            id: "siri_settings",
            name: "Siri",
            category: "Device",
            description:
              "How Siri is set up on this device — the voice it speaks with, the language it uses, and whether its data syncs to iCloud.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Voice language",
              "Voice name",
              "Custom voice",
              "Syncs to iCloud",
              "Recognises voices",
            ],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "alarms",
            name: "Alarms",
            category: "Device",
            description:
              "Clock alarms set on this iPhone — the time each is for, whether it is switched on, and when it was last changed.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Hour",
              "Minute",
              "On",
              "Snooze allowed",
              "Last changed",
              "Last dismissed",
              "Identifier",
            ],
            timestampColumns: ["Last changed", "Last dismissed"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "timers",
            name: "Timers",
            category: "Device",
            description:
              "Timers set in the Clock app — what each was called, how long it ran for, and when it was last changed.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Title",
              "Duration",
              "State",
              "Due",
              "Fire time",
              "Sound",
              "Identifier",
            ],
            timestampColumns: ["Due"],
            byteColumns: [],
            durationColumns: ["Duration"],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "imei_imsi",
            name: "Cellular identity",
            category: "Device",
            description:
              "The handset's IMEI, the subscriber's IMSI, and the phone number each SIM carried — the identifiers a carrier record is keyed on.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "SIM",
              "IMEI",
              "IMSI",
              "Phone number",
              "Number copied from SIM",
              "Last registered network",
            ],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "find_my",
            name: "Find My",
            category: "Device",
            description:
              "The Apple account this device is registered to for Find My, when that was enabled, and whether it sends its last location before dying.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Apple account (DSID)",
              "Find My enabled",
              "Send last location",
              "OS version recorded",
              "Enable context",
            ],
            timestampColumns: ["Find My enabled"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "message_retention",
            name: "Message retention",
            category: "Device",
            description:
              "How long this iPhone was set to keep messages before deleting them — context for any conversation that stops early.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Keep messages (iOS 17+)",
              "Keep messages (iOS 16 and earlier)",
            ],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "backup_settings",
            name: "Backup history",
            category: "Device",
            description:
              "When this device last backed up to a computer and to iCloud, and whether iCloud backup was switched on.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Last computer backup",
              "Computer backup time zone",
              "Last iCloud backup",
              "iCloud backup time zone",
              "iCloud backup on",
            ],
            timestampColumns: ["Last computer backup", "Last iCloud backup"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "location_services",
            name: "Location Services",
            category: "Device",
            description:
              "Whether Location Services was switched on at all — the context that decides what the per-app location list means.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Location Services on", "Last system version"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "stopwatch",
            name: "Stopwatch",
            category: "Device",
            description:
              "The Clock app's stopwatch — its state and how far the current run has got.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["State", "Current run"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: ["Current run"],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "airdrop",
            name: "AirDrop",
            category: "Device",
            description:
              "This device's AirDrop identifier, and who it was set to be discoverable by.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: ["AirDrop ID", "Discoverable by"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "world_clock",
            name: "World Clock",
            category: "Device",
            description:
              "Cities added to the Clock app's World Clock, with the time zone and coordinates of each.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "City",
              "Country",
              "Time zone",
              "Latitude",
              "Longitude",
              "Locale",
              "Identifier",
            ],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "sleep_schedule",
            name: "Sleep schedule",
            category: "Device",
            description:
              "The bedtime and wake time set on this iPhone, and whether sleep tracking was switched on.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Wake hour",
              "Wake minute",
              "Bedtime hour",
              "Bedtime minute",
              "On",
              "Sleep tracking",
              "Off until",
              "Last changed",
            ],
            timestampColumns: ["Off until", "Last changed"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "device_locale",
            name: "Language and region",
            category: "Device",
            description:
              "The language this iPhone is set to, its region format, and whether it shows a 24-hour clock.",
            surface: "device" as const,
            shape: "facts" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Language",
              "Region format",
              "Last known locale",
              "24-hour clock",
              "Passcode keyboard",
            ],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 1,
            requiresEncryptedBackup: false,
          },
          {
            id: "bluetooth_devices",
            name: "Bluetooth devices",
            category: "Device",
            description:
              "Classic Bluetooth accessories this iPhone has paired with — headphones, speakers, car kits — with the name the owner gave each one.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Address", "Named by owner", "Device name", "Kind"],
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 3,
            requiresEncryptedBackup: false,
          },
          {
            id: "wifi_private_mac",
            name: "Private Wi-Fi addresses",
            category: "Network",
            description:
              "The randomised hardware address this iPhone presented to each Wi-Fi network, and when it last changed.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Network",
              "Private address",
              "Address valid",
              "Access point",
              "Open network",
              "Still known",
              "Last joined",
              "Address generated",
            ],
            timestampColumns: ["Last joined", "Address generated"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "wifi_networks",
            name: "Wi-Fi networks",
            category: "Network",
            description:
              "Every Wi-Fi network this iPhone has joined, when it was added, when it was last joined, and the access point it saw.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Network",
              "Security",
              "Access point",
              "Channel",
              "Hidden",
              "Joined by user",
              "Joined automatically",
              "Added",
              "Last seen",
            ],
            timestampColumns: [
              "Joined by user",
              "Joined automatically",
              "Added",
              "Last seen",
            ],
            byteColumns: [],
            durationColumns: [],
            rowCount: 3,
            requiresEncryptedBackup: false,
          },
          {
            id: "sim_cards",
            name: "SIM cards",
            category: "Device",
            description:
              "Which SIMs have been in this device, the phone number each carried, and the slot it used.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: ["Slot", "Phone number", "SIM serial (ICCID)", "Last updated"],
            timestampColumns: ["Last updated"],
            byteColumns: [],
            durationColumns: [],
            rowCount: 2,
            requiresEncryptedBackup: false,
          },
          {
            id: "bluetooth_paired",
            name: "Bluetooth pairings",
            category: "Device",
            description:
              "Low-energy Bluetooth accessories this iPhone is paired with — watches, trackers, headphones and tags — with the address each advertises.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: null,
            highlight: null,
            columns: [
              "Device",
              "Address",
              "Resolves to",
              "Connection counter",
              "Seen counter",
              "Identifier",
            ],
            // Deliberately EMPTY: the two counters are device-relative, not
            // dates. If they were ever declared as timestamps the renderer would
            // print a confident 1970s date, which is the bug the module's own
            // comment explains at length.
            timestampColumns: [],
            byteColumns: [],
            durationColumns: [],
            rowCount: 3,
            requiresEncryptedBackup: false,
          },
          // Mock-only: an artifact that CANNOT exist in an unencrypted backup,
          // so the explanation path has something to render. No shipped module
          // is gated yet — the mechanism is ready for the first one that is
          // (Apple's list has 28 candidates; see the coverage audit).
          {
            id: "mock_gated",
            name: "Focus modes",
            category: "Device",
            description: "Which Focus modes exist on the device and when each was last changed.",
            surface: "device" as const,
            shape: "table" as const,
            joinColumn: "Mode",
            highlight: null,
            columns: ["Mode", "Enabled", "Changed"],
            timestampColumns: ["Changed"],
            byteColumns: [],
            durationColumns: [],
            rowCount: mockUnencrypted ? 0 : 2,
            requiresEncryptedBackup: true,
          },
        ]
      : [],
  getArtifactRows: async (artifactId) =>
    // Timers is the mock's one `duration` column, so the browser checks
    // actually render that kind rather than trusting it was wired up.
    mockActive && artifactId === "imei_imsi"
      ? [
          {
            SIM: "8901260971148676693",
            IMEI: "353985100845978",
            IMSI: "310260974867669",
            "Phone number": "+19195794674",
            "Number copied from SIM": "+19195794674",
            "Last registered network": "310260",
          },
        ]
      : mockActive && artifactId === "find_my"
        ? [
            {
              "Apple account (DSID)": "17193901029",
              "Find My enabled": 1688242982,
              "Send last location": true,
              "OS version recorded": "17.3",
              "Enable context": 3,
            },
          ]
      : mockActive && artifactId === "message_retention"
      // Strings, because a mapped column always is — and "90" is the unmapped
      // code travelling as itself.
      ? [
          {
            "Keep messages (iOS 17+)": "30 days",
            "Keep messages (iOS 16 and earlier)": "90",
          },
        ]
      : mockActive && artifactId === "backup_settings"
        ? [
            {
              "Last computer backup": 1722107200,
              "Computer backup time zone": "Europe/Stockholm",
              "Last iCloud backup": 1722207200,
              "iCloud backup time zone": "Europe/Stockholm",
              "iCloud backup on": true,
            },
          ]
      : mockActive && artifactId === "location_services"
        ? [{ "Location Services on": true, "Last system version": "21D50" }]
      : mockActive && artifactId === "stopwatch"
      ? [{ State: 2, "Current run": 93.5 }]
      : mockActive && artifactId === "airdrop"
        ? [{ "AirDrop ID": "6f8a2b1c9d4e", "Discoverable by": "Contacts Only" }]
      : mockActive && artifactId === "timers"
      ? [
          {
            Title: "Pasta",
            Duration: 600,
            State: 1,
            Due: 1722180000,
            "Fire time": "MTTimerDate",
            Sound: "system:Radial",
            Identifier: "1D8B30D8-DF6F-4644-B7E3-534F4E26CB86",
          },
          {
            // The shape the real device actually has: stored, not scheduled,
            // so Due is empty and "Fire time" says why.
            Title: "CURRENT_TIMER",
            Duration: 900,
            State: 1,
            Due: null,
            "Fire time": "MTTimerTimeInterval",
            Sound: "system:Radial",
            Identifier: "2E9C41E9-EF70-5755-C8F4-645F5F37DC97",
          },
        ]
      : mockActive && artifactId === "world_clock"
        ? [
            {
              City: "Stockholm",
              Country: "Sweden",
              "Time zone": "Europe/Stockholm",
              Latitude: 59.3293,
              Longitude: 18.0686,
              Locale: "sv_SE",
              Identifier: "Stockholm",
            },
            {
              City: "Cupertino",
              Country: "United States",
              "Time zone": "America/Los_Angeles",
              Latitude: 37.323,
              Longitude: -122.0322,
              Locale: "en_US",
              Identifier: "Cupertino",
            },
          ]
      // Bundle ids MUST match mockInstalledApps, or Apps has nothing to attach
      // these to and the hosted path silently renders nothing.
      : mockActive && artifactId === "location_clients"
      ? [
          {
            App: "net.whatsapp.WhatsApp",
            Client: "inet.whatsapp.WhatsApp:",
            "Bundle path": "/private/var/containers/Bundle/Application/WhatsApp.app",
            Registered: "Yes",
            "Stopped receiving": 1722629788,
            "Location stopped": 1722629700,
          },
          {
            // Same app, a second session — only the key tells them apart.
            App: "net.whatsapp.WhatsApp",
            Client: "lnet.whatsapp.WhatsApp:p/System/Library/LocationBundles/Nav.bundle",
            "Bundle path": null,
            Registered: null,
            "Stopped receiving": null,
            "Location stopped": 1722598764,
          },
          {
            App: "com.burbn.instagram",
            Client: "icom.burbn.instagram:",
            "Bundle path": "/private/var/containers/Bundle/Application/Instagram.app",
            Registered: "Yes",
            "Stopped receiving": 1722092872,
            "Location stopped": 1722092862,
          },
        ]
      : mockActive && artifactId === "data_usage"
      ? [
          {
            App: "net.whatsapp.WhatsApp",
            "Carried by": "WhatsApp",
            "Cellular down": 481233920,
            "Cellular up": 92341760,
            "Wi-Fi down": 2914512896,
            "Wi-Fi up": 402653184,
            Records: 6,
            First: 1700851484,
            Last: 1722190914,
          },
          {
            App: "com.burbn.instagram",
            "Carried by": "nsurlsessiond",
            "Cellular down": 1073741824,
            "Cellular up": 51628130,
            "Wi-Fi down": 8589934592,
            "Wi-Fi up": 214748364,
            Records: 9,
            First: 1701078640,
            Last: 1722179466,
          },
          {
            App: "com.zhiliaoapp.musically",
            "Carried by": "TikTok",
            "Cellular down": 249049925,
            "Cellular up": 51377128,
            // Never recorded on this lineage, as distinct from zero traffic.
            "Wi-Fi down": null,
            "Wi-Fi up": null,
            Records: 3,
            First: 1707165210,
            Last: 1722101455,
          },
          {
            App: "com.apple.AppStore",
            "Carried by": "appstored",
            "Cellular down": 2715051824,
            "Cellular up": 51628130,
            "Wi-Fi down": 0,
            "Wi-Fi up": 0,
            Records: 7,
            First: 1700851484,
            Last: 1722190914,
          },
        ]
      : mockActive && artifactId === "accounts"
      ? [
          {
            Service: "Gmail",
            Account: "person@example.com",
            Label: "Gmail",
            "Part of": null,
            Added: 1704326400,
            Status: "Active",
            "Signed in": "Yes",
            "Registered by": "com.apple.mobilemail",
          },
          {
            Service: "CardDAV",
            Account: null,
            Label: null,
            // A sub-account: on real data these are what make one sign-in look
            // like several duplicate rows until the parent is shown.
            "Part of": "Gmail",
            Added: 1704326300,
            Status: "Active",
            "Signed in": "Yes",
            "Registered by": "com.apple.accounts.accountsd",
          },
          {
            Service: "Holiday Calendar",
            Account: null,
            Label: "US Holidays",
            "Part of": null,
            Added: 1703326400,
            Status: "Active",
            "Signed in": "Yes",
            "Registered by": "dataaccessd",
          },
          {
            Service: "iCloud",
            Account: "person@example.com",
            Label: "iCloud",
            "Part of": null,
            Added: 1702326400,
            Status: "Active",
            "Signed in": "Yes",
            "Registered by": "com.apple.purplebuddy",
          },
          {
            Service: "Game Center",
            Account: "person@example.com",
            Label: null,
            "Part of": null,
            Added: 1701326400,
            Status: "Inactive",
            "Signed in": "No",
            "Registered by": "appstored",
          },
        ]
      : mockActive && artifactId === "home_screen"
        ? [
            { Page: "0", Identifier: "net.whatsapp.WhatsApp", Kind: "app", Size: "small" },
            {
              Page: "0",
              // A widget: a UUID, not a bundle id — `Kind` is what says so.
              Identifier: "A5E1414E-FD2B-486D-BAC2-B0DEED262F03",
              Kind: "custom",
              Size: "medium",
            },
            { Page: "1", Identifier: "com.burbn.instagram", Kind: "app", Size: "small" },
          ]
      : mockActive && artifactId === "dock"
        ? [
            { Position: "0", App: "com.apple.mobilephone" },
            { Position: "1", App: "com.apple.mobilesafari" },
          ]
      : mockActive && artifactId === "alltrails"
        ? [
            {
              Activity: "Morning hike",
              Started: 1704307200,
              Ended: 1704311698,
              "Distance (m)": 6796,
              "Moving time (s)": 4498,
              "Total time (s)": 4498,
              "Climb (m)": 84,
              Calories: 650,
              "Roughly where (lat)": 38.8,
              "Roughly where (lon)": -77.3,
              Private: "Yes",
              Segments: 1,
            },
            {
              Activity: "Bass Lake Trail",
              Started: 1638307200,
              Ended: 1638310225,
              "Distance (m)": 3049,
              "Moving time (s)": 2846,
              "Total time (s)": 3025,
              "Climb (m)": 29,
              Calories: 411,
              "Roughly where (lat)": 35.65,
              "Roughly where (lon)": -78.85,
              Private: "No",
              // Paused and resumed — one activity, two segments.
              Segments: 2,
            },
          ]
      : mockActive && artifactId === "podcasts"
        ? [
            {
              Show: "Listened Show",
              "Published by": "A tech journalist",
              Category: "Tech News",
              Subscribed: "Yes",
              Added: 1585164062,
              "Last played": 1611156927,
              Feed: "https://example.com/feed",
            },
            {
              Show: "Never Played Show",
              "Published by": "Example Radio",
              Category: "Daily News",
              Subscribed: "Yes",
              Added: 1585163878,
              // Followed, never actually played — the distinction the artifact is for.
              "Last played": null,
              Feed: "https://example.org/rss",
            },
          ]
      : mockActive && artifactId === "backup_sizing"
        ? [
            { Domain: "CameraRollDomain", Size: 3221225472 },
            { Domain: "KeyboardDomain", Size: 2535424 },
            { Domain: "AppDomainGroup-group.com.example.chat", Size: 175961 },
          ]
      : mockActive && artifactId === "watch_apps"
        ? [
            {
              App: "com.example.chatapp.watchkitapp",
              "Companion app": "com.example.chatapp",
              Version: "2.4",
              Build: "2401",
              "On the watch": true,
              "Minimum watchOS": "9.6",
              // What the `*` matched — the paired device, not the whole path.
              "Paired device": "48BEB26F-3064-4BEF-A616-AB96D8C5BD15",
            },
            {
              App: "com.example.todo.watchkitapp",
              "Companion app": "com.example.todo",
              Version: "1.0",
              Build: null,
              // Listed for the watch, but not on it.
              "On the watch": false,
              "Minimum watchOS": null,
              "Paired device": "48BEB26F-3064-4BEF-A616-AB96D8C5BD15",
            },
          ]
      : mockActive && artifactId === "bluetooth_nearby"
        ? [
            // Named first, then the anonymous rotating addresses — nothing dropped.
            {
              Device: "Garage Opener",
              Address: "Public CC:6A:10:54:65:FF",
              "Seen counter": 4352299,
              Identifier: "11111111-0000-0000-0000-000000000002",
            },
            {
              Device: "Fitness Band",
              Address: "Random ED:FD:03:AC:36:76",
              "Seen counter": 4337974,
              Identifier: "11111111-0000-0000-0000-000000000004",
            },
            {
              Device: null,
              Address: "Random AA:BB:CC:DD:EE:01",
              "Seen counter": 4000000,
              Identifier: "11111111-0000-0000-0000-000000000001",
            },
            {
              Device: "",
              Address: "Random AA:BB:CC:DD:EE:03",
              "Seen counter": 4100000,
              Identifier: "11111111-0000-0000-0000-000000000003",
            },
          ]
      : mockActive && artifactId === "siri_settings"
        ? [
            {
              "Voice language": "en-US",
              "Voice name": "nora",
              "Custom voice": true,
              "Syncs to iCloud": true,
              "Recognises voices": false,
            },
          ]
      : mockActive && artifactId === "alarms"
        ? [
            {
              Hour: 10,
              Minute: 41,
              On: false,
              "Snooze allowed": true,
              "Last changed": 1722177663,
              "Last dismissed": 1722177663,
              Identifier: "4ABC24C8-A16E-440D-A56D-0F7C2D46825E",
            },
          ]
      : mockActive && artifactId === "sleep_schedule"
        ? [
            {
              "Wake hour": 6,
              "Wake minute": 0,
              "Bedtime hour": 22,
              "Bedtime minute": 45,
              On: false,
              "Sleep tracking": true,
              "Off until": 1689849000,
              "Last changed": 1722076501,
            },
          ]
      : mockActive && artifactId === "device_locale"
        ? [
            {
              Language: "en-US",
              "Region format": "en_US",
              "Last known locale": "en_US",
              "24-hour clock": true,
              "Passcode keyboard": "en_US@sw=QWERTY;hw=Automatic",
            },
          ]
      : mockActive && artifactId === "bluetooth_devices"
        ? [
            {
              Address: "08:65:18:75:5E:75",
              "Named by owner": "Alex's AirPods",
              "Device name": "AirPods 3",
              Kind: "Headphones",
            },
            {
              Address: "7C:04:D0:89:89:A0",
              "Named by owner": "Sam's AirPods",
              "Device name": "AirPods",
              Kind: "Headphones",
            },
            {
              // Never renamed: null, not a fallback to the model.
              Address: "F8:6F:C1:4E:FF:6A",
              "Named by owner": null,
              "Device name": "Apple Watch",
              Kind: "Watch",
            },
          ]
      : mockActive && artifactId === "wifi_private_mac"
        ? [
            {
              Network: "HomeNet",
              "Private address": "8a:1b:2c:3d:4e:5f",
              "Address valid": true,
              "Access point": "6a:22:32:98:f4:df",
              "Open network": false,
              "Still known": true,
              "Last joined": 1689450273,
              "Address generated": 1700312363,
            },
            {
              Network: "Cafe Wifi",
              "Private address": "00:11:22:33:44:55",
              // Present but not in use.
              "Address valid": false,
              "Access point": null,
              "Open network": true,
              "Still known": false,
              "Last joined": 1700000000,
              "Address generated": 1699000000,
            },
          ]
      : mockActive && artifactId === "wifi_networks"
        ? [
            {
              Network: "HomeNet",
              Security: "WPA2 Personal",
              "Access point": "6a:22:32:98:f4:df",
              Channel: 153,
              Hidden: false,
              "Joined by user": 1688243921,
              "Joined automatically": 1689450000,
              Added: 1688243920,
              "Last seen": 1689450218,
            },
            {
              Network: "Hilton Garden Inn Guest",
              Security: "OWE Transition",
              "Access point": "70:a7:41:67:ac:9d",
              Channel: 6,
              Hidden: false,
              "Joined by user": 1715116642,
              "Joined automatically": 1715168000,
              Added: 1715116642,
              "Last seen": 1715168820,
            },
            {
              // No __OSSpecific__ subtree on this one: absent, not zero.
              Network: "Cafe Wifi",
              Security: "None",
              "Access point": null,
              Channel: null,
              Hidden: true,
              "Joined by user": null,
              // Never joined at all, deliberately or otherwise.
              "Joined automatically": null,
              Added: 1700000000,
              "Last seen": null,
            },
          ]
      : mockActive && artifactId === "sim_cards"
        ? [
            {
              Slot: 1,
              "Phone number": "+15550100",
              "SIM serial (ICCID)": "8901260971148676693",
              "Last updated": 1704307200,
            },
            {
              Slot: 2,
              "Phone number": "+15550199",
              "SIM serial (ICCID)": "8944500000000000001",
              "Last updated": 1703307200,
            },
          ]
      : mockActive && artifactId === "bluetooth_paired"
        ? [
            {
              Device: "Example Watch",
              Address: "Random 50:32:66:45:35:EF",
              "Resolves to": "Public F8:6F:C1:4E:FF:6A",
              "Connection counter": 9639,
              "Seen counter": 4315986,
              Identifier: "6C0C35A0-84CE-3572-2E72-4CF3D03BD1AF",
            },
            {
              Device: "Fitness Band",
              Address: "Public B4:C2:6A:7F:D3:7A",
              "Resolves to": "Public B4:C2:6A:7F:D3:7A",
              "Connection counter": 2143,
              "Seen counter": 395626,
              Identifier: "E3B37CA8-1AA5-AD44-B0FE-A617BB09B64A",
            },
            {
              Device: "Nameless Tag",
              Address: "Random E8:F0:58:00:C0:FB",
              "Resolves to": null,
              "Connection counter": 3662,
              "Seen counter": 748458,
              Identifier: "C4E4E254-6060-26CA-7C80-EE01F3C5C346",
            },
          ]
      : mockActive && artifactId === "mock_gated"
      ? mockUnencrypted
        ? []
        : [
            { Mode: "Sleep", Enabled: true, Changed: 1700000000 },
            { Mode: "Work", Enabled: false, Changed: 1700000400 },
          ]
      : mockActive && artifactId === "tcc"
        ? // Bundle ids MUST match mockInstalledApps, or the Apps view has nothing
          // to attach these to and the hosted-artifact path silently renders
          // nothing — which is exactly what happened the first time.
          [
            { App: "net.whatsapp.WhatsApp", Permission: "Camera", Decision: "Allowed", Decided: 1700000000 },
            { App: "net.whatsapp.WhatsApp", Permission: "Microphone", Decision: "Allowed", Decided: 1700000100 },
            { App: "net.whatsapp.WhatsApp", Permission: "Contacts", Decision: "Allowed", Decided: 1700000150 },
            { App: "net.whatsapp.WhatsApp", Permission: "Photos", Decision: "Limited", Decided: 1700000200 },
            { App: "net.whatsapp.WhatsApp", Permission: "Tracking", Decision: "Denied", Decided: 1700000250 },
            { App: "com.burbn.instagram", Permission: "Camera", Decision: "Allowed", Decided: 1700000300 },
            { App: "com.burbn.instagram", Permission: "kTCCServiceLocation", Decision: "Denied", Decided: 1700000350 },
            { App: "com.zhiliaoapp.musically", Permission: "Microphone", Decision: "Not decided", Decided: null },
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
  countThreadMessages: async (threadId, _kind = null, search = null) =>
    mockActive ? mockThreadMessages(threadId, search).length : 0,
  getThreadMessageWindow: async (
    threadId,
    offset,
    limit,
    desc = false,
    _kind = null,
    search = null,
  ) => {
    if (!mockActive) return [];
    const all = mockThreadMessages(threadId, search);
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
  countNoteRanges: async (ranges) =>
    ranges.map((r) => {
      if (!mockActive) return 0;
      return mockNotes.filter((n) => {
        const t = n.modifiedAt ?? n.createdAt;
        if (t == null) return false;
        return (r.lo == null || t >= r.lo) && (r.hi == null || t < r.hi);
      }).length;
    }),
  // Deliberately NOT the browser default: the mock has to exercise a locale
  // that DIFFERS from the webview's, because formatting in the webview's own
  // locale is the bug (#161). en-SE is the case that found it — English
  // language, Region Sweden.
  getSystemLocale: async () => "en-SE",
  moduleMetrics: async () => {
    if (!mockActive) return [];
    // Mirrors the backend: every module it knows about, sources with no rows
    // dropped, bucket count following the data. Deliberately covers ALL of them
    // — five were missing before, so the design lint and every screenshot check
    // had never once seen those tiles (#163).
    const now = Math.floor(Date.now() / 1000);
    const TIMELINE_START = 1_167_609_600; // 2007-01-01
    const shape = (stamps: number[]) => {
      const ok = stamps.filter((t) => t >= TIMELINE_START && t <= now + 86_400);
      if (ok.length < 4) {
        return ok.length
          ? { firstAt: Math.min(...ok), lastAt: Math.max(...ok), series: [] }
          : { firstAt: null, lastAt: null, series: [] };
      }
      const lo = Math.min(...ok);
      const hi = Math.max(...ok);
      const n = Math.min(16, Math.max(4, ok.length));
      const series = new Array(n).fill(0);
      for (const t of ok) {
        series[Math.min(n - 1, Math.floor(((t - lo) * n) / (hi - lo + 1)))] += 1;
      }
      return { firstAt: lo, lastAt: hi, series };
    };
    const tally = (values: (string | null | undefined)[]) => {
      const by = new Map<string, number>();
      for (const v of values) if (v) by.set(v, (by.get(v) ?? 0) + 1);
      return [...by.entries()]
        .map(([label, count]) => ({ label, count }))
        .sort((a, b) => b.count - a.count);
    };

    // Read the OTHER mocks through the client rather than copying their
    // fixtures: a third copy of "what calendar data exists" would drift from
    // the view's, which is the whole failure this dashboard already had once.
    const [events, reminders, workouts, sleep] =
      await Promise.all([
        mockClient.listCalendarEvents(),
        mockClient.listReminders(),
        mockClient.listWorkouts(),
        mockClient.listSleep(),
      ]);

    const msgs = Object.values(mockMessages).flat();
    const serviceOf = (threadId: number) =>
      mockThreads.find((t) => t.id === threadId)?.service ?? null;

    const sources: {
      id: string;
      label: string;
      route: string;
      icon: string;
      count: number;
      stamps: number[];
      facets?: { label: string; count: number }[];
    }[] = [
      {
        id: "messages", label: "Messages", route: "/messages", icon: "messages",
        count: msgs.length,
        stamps: msgs.map((m) => m.sentAt ?? 0),
        facets: tally(
          Object.entries(mockMessages).flatMap(([tid, ms]) =>
            ms.map(() => serviceOf(Number(tid))),
          ),
        ),
      },
      {
        id: "photos", label: "Photos", route: "/photos", icon: "photos",
        count: mockMedia.length,
        stamps: mockMedia.map((m) => m.takenAt ?? 0),
        facets: tally(mockMedia.map((m) => m.source)),
      },
      {
        id: "contacts", label: "Contacts", route: "/contacts", icon: "contacts",
        count: mockContacts.length, stamps: [],
      },
      {
        id: "calls", label: "Calls", route: "/calls", icon: "calls",
        count: mockCalls.length,
        stamps: mockCalls.map((c) => c.occurredAt ?? 0),
        facets: tally(mockCalls.map((c) => c.service)),
      },
      {
        id: "safari", label: "Safari", route: "/safari", icon: "safari",
        count: mockSafari.length,
        stamps: mockSafari.map((v) => v.visitedAt ?? 0),
      },
      {
        id: "notes", label: "Notes", route: "/notes", icon: "notes",
        count: mockNotes.length,
        stamps: mockNotes.map((n) => n.modifiedAt ?? 0),
      },
      {
        id: "recordings", label: "Recordings", route: "/recordings", icon: "recordings",
        count: mockRecordings.length,
        stamps: mockRecordings.map((r) => r.recordedAt ?? 0),
      },
      {
        id: "calendar", label: "Calendar", route: "/calendar", icon: "calendar",
        count: events.length,
        stamps: events.map((e) => e.startAt ?? 0),
      },
      {
        id: "reminders", label: "Reminders", route: "/reminders", icon: "reminders",
        count: reminders.length,
        stamps: reminders.map((r) => r.dueAt ?? 0),
      },
      {
        id: "health", label: "Health", route: "/health", icon: "health",
        count: workouts.length + sleep.length,
        stamps: [
          ...workouts.map((w) => w.startAt ?? 0),
          ...sleep.map((x) => x.startAt ?? 0),
        ],
        facets: [
          { label: "Workouts", count: workouts.length },
          { label: "Sleep", count: sleep.length },
        ].filter((f) => f.count > 0).sort((a, b) => b.count - a.count),
      },
      {
        id: "apps", label: "Apps", route: "/apps", icon: "apps",
        count: mockInstalledApps.length, stamps: [],
        facets: mockInstalledApps.map((a) => ({ label: a.bundleId, count: 0 })),
      },
    ];

    return sources
      .filter((s) => s.count > 0)
      .map(({ stamps, facets, ...rest }) => ({
        ...rest,
        facets: facets ?? [],
        ...shape(stamps.filter(Boolean)),
      }));
  },
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
  listCalls: async () => (mockActive && !mockParseFailed ? mockCalls : []),
  listSafariHistory: async () => (mockActive ? mockSafari : []),
  listNotes: async () => (mockActive && !mockNoData ? mockNotes : []),
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
  countCalls: async (search, lo = null, hi = null, addresses = null) =>
    mockActive ? mockFilterCalls(search, { lo, hi }, addresses).length : 0,
  countCallRanges: async (ranges, search = null, addresses = null) =>
    ranges.map((r) =>
      mockActive ? mockFilterCalls(search, r, addresses).length : 0,
    ),
  getCallsWindow: async (
    search,
    lo,
    hi,
    offset,
    limit,
    sortBy,
    desc,
    addresses = null,
  ) =>
    mockActive
      ? mockSortBy(
          mockFilterCalls(search, { lo, hi }, addresses),
          callKey(sortBy),
          desc,
        ).slice(offset, offset + limit)
      : [],
  callAddresses: async () =>
    mockActive && !mockParseFailed
      ? [...new Set(mockCalls.map((c) => c.address).filter((a): a is string => !!a))]
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
  messageDeletionEvidence: async () =>
    mockActive
      ? {
          // The mock mirrors the validation device: iOS recorded two deletions
          // and two ROWIDs are missing — the SAME two. Shown separately so the
          // UI is exercised against the double-counting trap, not around it.
          recorded: 2,
          missingRowids: 2,
          gaps: 2,
          firstGapAt: 1717790000,
          lastGapAt: 1717801200,
        }
      : { recorded: 0, missingRowids: 0, gaps: 0, firstGapAt: null, lastGapAt: null },
  listDevicesUsed: async () => {
    if (!mockActive) return [];
    const byModel = new Map<string, DeviceUse>();
    for (const d of mockDeviceUse) {
      const prev = byModel.get(d.model);
      byModel.set(d.model, {
        model: d.model,
        osBuild: null,
        firstAt: Math.min(prev?.firstAt ?? d.firstAt!, d.firstAt!),
        lastAt: Math.max(prev?.lastAt ?? d.lastAt!, d.lastAt!),
        samples: (prev?.samples ?? 0) + d.samples,
      });
    }
    return [...byModel.values()].sort((a, b) => (a.firstAt ?? 0) - (b.firstAt ?? 0));
  },
  listDeviceOsHistory: async () =>
    mockActive
      ? mockDeviceUse
          .filter((d) => d.firstAt !== d.lastAt)
          .sort((a, b) => (a.firstAt ?? 0) - (b.firstAt ?? 0))
      : [],
  countSafariSearches: async (search, lo = null, hi = null) =>
    mockActive ? mockFilterSafariSearches(search, { lo, hi }).length : 0,
  countSafariSearchRanges: async (search, ranges) =>
    ranges.map((r) => (mockActive ? mockFilterSafariSearches(search, r).length : 0)),
  getSafariSearchesWindow: async (search, lo, hi, offset, limit, sortBy, desc) =>
    mockActive
      ? mockSortBy(
          mockFilterSafariSearches(search, { lo, hi }),
          safariSearchKey(sortBy),
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
  // A tiny inline SVG per bundle id so the icon-swap path is exercised in dev
  // (the real command fetches from Apple).
  getAppIcons: async (bundleIds) =>
    bundleIds.map((bundleId) => {
      const hue = [...bundleId].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 0);
      const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='60' height='60'><rect width='60' height='60' rx='13' fill='hsl(${hue} 65% 45%)'/><circle cx='30' cy='30' r='12' fill='white' opacity='0.9'/></svg>`;
      return { bundleId, dataUri: `data:image/svg+xml;utf8,${encodeURIComponent(svg)}` };
    }),

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
        feeds: mockSnapshotInfo.feeds,
        feedsGeneratedAt: Math.floor(
          Date.parse(mockSnapshotInfo.generatedAt) / 1000,
        ),
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
  listScanRuns: async () =>
    mockActive
      ? mockBulk(mockScanRuns, (r, i) => ({ ...r, id: 100000 + i }))
      : [],
  latestScanRun: async () =>
    mockActive && mockScanRuns.length ? mockScanRuns[0].id : null,
  listFindings: async (_runId, minSeverity) => {
    if (!mockActive) return [];
    const rank = (s: Severity) =>
      s === "critical" ? 3 : s === "warning" ? 2 : 1;
    const min = minSeverity ? rank(minSeverity) : 1;
    return mockBulk(
      mockFindings.filter((f) => rank(f.severity) >= min),
      (f, i) => ({ ...f, id: 100000 + i }),
    );
  },
  getSafetyScanModelStatus: async () => ({
    totalRamBytes: 16 * 1024 ** 3,
    models: [
      {
        id: "gemma-4-E4B-it-Q4_K_M",
        displayName: "Gemma 4 E4B",
        note: "Best accuracy — the default classifier.",
        sizeBytes: 4_977_171_584,
        installed: mockSafetyModelInstalled,
        recommended: true,
      },
      {
        id: "gemma-4-E2B-it-Q4_K_M",
        displayName: "Gemma 4 E2B",
        note: "Lighter fallback — smaller and faster; use it if the larger model is slow or won't load on this Mac.",
        sizeBytes: 3_106_738_272,
        installed: false,
        recommended: false,
      },
    ],
    readyModelId: mockSafetyModelInstalled ? "gemma-4-E4B-it-Q4_K_M" : null,
  }),
  safetyScanHealthCheck: async (modelId) => ({
    ok: true,
    modelId: modelId ?? "gemma-4-E4B-it-Q4_K_M",
    displayName: "Gemma 4 E4B",
    startupMs: 4200,
    message: "Server started and Gemma 4 E4B loaded in 4.2s.",
  }),
  getSafetyScanDownloadStatus: async () => null,
  getSafetyScanStatus: async () => null,
  getImportStatus: async () => null,
  getSecurityScanStatus: async () => null,
  getReimportStatus: async () => [],
  downloadSafetyScanModel: async () => {
    mockSafetyModelInstalled = true;
  },
  cancelSafetyScanModelDownload: async () => {},
  runSafetyScan: async () => {
    if (!mockActive) throw new Error("no backup is open");
  },
  cancelSafetyScan: async () => {},
  onSafetyScanProgress: async () => () => {},
  onSafetyModelProgress: async () => () => {},
  listContentFindings: async (scanId, page) => {
    // The mock applies the same filters and order as SQLite, so the browser
    // harness exercises the real paging path rather than a shortcut.
    const all = mockFindingsForScan(scanId);
    let rows = all.filter((f) => page.includeDismissed || !f.dismissed);
    if (page.excludeStale) rows = rows.filter((f) => !f.stale);
    if (page.severity) rows = rows.filter((f) => f.severity === page.severity);
    const dir = page.desc ? -1 : 1;
    rows = [...rows].sort((a, b) => {
      if (page.groupByThread) {
        const at = a.threadIdentifier ?? "\uffff";
        const bt = b.threadIdentifier ?? "\uffff";
        if (at !== bt) return at < bt ? -1 : 1;
      }
      if (page.sortBy === "severity" && a.severity !== b.severity)
        return (a.severity - b.severity) * dir;
      const ao = a.occurredAt ?? 0;
      const bo = b.occurredAt ?? 0;
      if (ao !== bo) return (ao - bo) * dir;
      return (a.id - b.id) * dir;
    });
    return rows.slice(page.offset, page.offset + page.limit);
  },
  countContentFindings: async (scanId, filter) => {
    const all = mockFindingsForScan(scanId);
    const live = all.filter((f) => !f.dismissed);
    let matched = filter?.includeDismissed ? all : live;
    if (filter?.excludeStale) matched = matched.filter((f) => !f.stale);
    if (filter?.severity)
      matched = matched.filter((f) => f.severity === filter.severity);
    return {
      matching: matched.length,
      live: live.length,
      liveFresh: live.filter((f) => !f.stale).length,
      dismissed: all.length - live.length,
      unread: live.filter((f) => !f.stale && !f.seen).length,
      serious: live.filter((f) => f.severity === 3).length,
      harmful: live.filter((f) => f.severity === 2).length,
      concerning: live.filter((f) => f.severity === 1).length,
    };
  },
  contentFindingAnalytics: async (scanId, filter) => {
    const all = mockFindingsForScan(scanId);
    const live = filter?.includeDismissed ? all : all.filter((f) => !f.dismissed);
    let matched = filter?.excludeStale ? live.filter((f) => !f.stale) : live;
    if (filter?.severity)
      matched = matched.filter((f) => f.severity === filter.severity);

    // Mirrors the backend's grouping shape so the mock exercises the same view
    // paths — including the splits, which are what the charts are about.
    const bucketOf = (rows: typeof matched, key: (f: (typeof matched)[0]) => string) => {
      const by = new Map<string, ChartBucket>();
      for (const f of rows) {
        const k = key(f);
        const b =
          by.get(k) ??
          ({ key: k, confirmed: [0, 0, 0], unconfirmed: [0, 0, 0] } as ChartBucket);
        const i = (f.severity - 1) as 0 | 1 | 2;
        if (f.rechecked) b.confirmed[i]++;
        else b.unconfirmed[i]++;
        by.set(k, b);
      }
      return [...by.values()];
    };
    const total = (b: ChartBucket) =>
      b.confirmed.reduce((a, n) => a + n, 0) + b.unconfirmed.reduce((a, n) => a + n, 0);

    // Same window as the backend (TIMELINE_START): a timestamp before the iPhone
    // existed, or in the future, is a decode failure rather than a date, and one
    // of them would stretch the axis across half a century. Mirrored here on
    // purpose — the divergent copy of the dismissed count in this very file is
    // what made that lesson concrete.
    const TIMELINE_START = 1_167_609_600; // 2007-01-01
    const horizon = Math.floor(Date.now() / 1000) + 86_400;
    const datable = (f: (typeof matched)[0]) =>
      f.occurredAt != null && f.occurredAt >= TIMELINE_START && f.occurredAt <= horizon;
    const dated = matched.filter(datable);
    const stamps = dated.map((f) => f.occurredAt as number);
    const span = stamps.length ? Math.max(...stamps) - Math.min(...stamps) : 0;
    const DAY = 86400;
    const unit =
      span <= 31 * DAY
        ? "day"
        : span <= 210 * DAY
          ? "week"
          : span <= 1095 * DAY
            ? "month"
            : span <= 3650 * DAY
              ? "quarter"
              : "year";
    const pad = (n: number) => String(n).padStart(2, "0");
    const timeKey = (at: number) => {
      const d = new Date(at * 1000);
      const y = d.getFullYear();
      switch (unit) {
        case "day":
          return `${y}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
        case "week": {
          const m = new Date(d);
          // Back to this week's Monday, matching the SQL bucket.
          m.setDate(m.getDate() - ((m.getDay() + 6) % 7));
          return `${m.getFullYear()}-${pad(m.getMonth() + 1)}-${pad(m.getDate())}`;
        }
        case "month":
          return `${y}-${pad(d.getMonth() + 1)}`;
        case "quarter":
          return `${y}-Q${Math.floor(d.getMonth() / 3) + 1}`;
        default:
          return `${y}`;
      }
    };

    const byConversation = bucketOf(matched, (f) => f.threadIdentifier ?? "").sort(
      (a, b) => total(b) - total(a) || a.key.localeCompare(b.key),
    );
    const CAP = 12;
    const overflow = byConversation.slice(CAP);
    return {
      unit,
      overTime: bucketOf(dated, (f) => timeKey(f.occurredAt as number)).sort((a, b) =>
        a.key.localeCompare(b.key),
      ),
      byCategory: bucketOf(matched, (f) => f.category).sort(
        (a, b) => total(b) - total(a) || a.key.localeCompare(b.key),
      ),
      byConversation: byConversation.slice(0, CAP),
      otherConversations: overflow.length,
      otherConversationFindings: overflow.reduce((n, b) => n + total(b), 0),
      charted: matched.length,
      undated: matched.length - dated.length,
      // What these charts LEFT OUT, mirroring the backend: zero when the caller
      // asked for dismissed findings (nothing was left out), and narrowed by the
      // severity filter like everything else. The disclosure beside the charts
      // says "left out of every chart" — in the mock too, that has to be true.
      dismissed: filter?.includeDismissed
        ? 0
        : all.filter(
            (f) =>
              f.dismissed &&
              (!filter?.excludeStale || !f.stale) &&
              (!filter?.severity || f.severity === filter.severity),
          ).length,
    };
  },
  contentFindingSnippet: async (sourceKind, sourceId) => {
    if (!mockActive || sourceId == null) return null;
    const finding = mockContentFindings.find((f) => f.sourceId === sourceId);
    return sourceKind === "note"
      ? {
          text: "Journal — Jun 3\nToday was rough. Kept thinking about what they said…",
          sender: null,
          recipient: null,
          occurredAt: null,
          service: "Notes",
        }
      : {
          text: "you need to send me your location right now, and show me who you were with",
          sender: "Alex",
          recipient: "Me",
          occurredAt: finding?.occurredAt ?? 1717300000,
          service: finding?.service ?? "iMessage",
        };
  },
  safetyScanFindingMarks: async () => {
    const marks: FindingMarks = { threads: {}, notes: {} };
    if (!mockActive) return marks;
    for (const f of mockContentFindings) {
      if (f.dismissed || f.stale) continue;
      if (f.sourceKind === "message" && f.threadId != null) {
        marks.threads[f.threadId] = Math.max(
          marks.threads[f.threadId] ?? 0,
          f.severity,
        ) as 1 | 2 | 3;
      } else if (f.sourceKind === "note" && f.sourceId != null) {
        marks.notes[f.sourceId] = Math.max(
          marks.notes[f.sourceId] ?? 0,
          f.severity,
        ) as 1 | 2 | 3;
      }
    }
    return marks;
  },
  dismissContentFinding: async (fingerprint, category, dismissed, reason) => {
    for (const f of mockContentFindings) {
      if (f.fingerprint === fingerprint && f.category === category) {
        f.dismissed = dismissed;
        // Dismissing implies reading — the control lives inside the expansion,
        // so it cannot be reached without revealing the text. Mirrored here so
        // the mock's unread count behaves like the backend's.
        if (dismissed) f.seen = true;
        f.dismissReason = dismissed ? (reason ?? null) : null;
      }
    }
  },
  addSafetySuppression: async (scope, value, reason) => {
    let n = 0;
    for (const f of mockContentFindings) {
      const hit =
        scope === "thread" ? f.threadIdentifier === value : f.category === value;
      // Never overwrite a decision made by hand — same rule as the backend.
      if (hit && !f.dismissed && f.dismissReason == null) {
        f.dismissed = true;
        f.dismissReason = reason ?? "Matched a rule you set";
        n += 1;
      }
    }
    mockSuppressions.push({ scope, value, reason: reason ?? null });
    return n;
  },
  listSafetySuppressions: async () => (mockActive ? mockSuppressions : []),
  removeSafetySuppression: async (scope, value) => {
    const i = mockSuppressions.findIndex((s) => s.scope === scope && s.value === value);
    if (i >= 0) mockSuppressions.splice(i, 1);
  },
  markContentFindingSeen: async (fingerprint, category) => {
    for (const f of mockContentFindings) {
      if (f.fingerprint === fingerprint && f.category === category) f.seen = true;
    }
  },
  generateThreadSummary: async (_scanId, threadRef) => ({
    threadRef,
    content: `${threadRef}: 2 findings flagged. Peak severity 2. Open the conversation to review them in context.`,
    source: "deterministic" as const,
  }),
  getSafetyScanReport: async (scanId) =>
    mockActive && mockContentFindings.length
      ? {
          scan: {
            id: scanId ?? 1,
            model: "gemma-4-E4B-it-Q4_K_M",
            rangeStart: null,
            rangeEnd: null,
            status: "completed",
            startedAt: Math.floor(Date.now() / 1000) - 3600,
            finishedAt: Math.floor(Date.now() / 1000),
            chunksTotal: 42,
            chunksDone: 42,
          },
          report:
            "Two conversations produced findings. The most serious is an escalating pattern of monitoring demands in “Alex” (coercive-control, severity 2), alongside one severity-2 scam attempt in “+1 555 0100”. Review “Alex” first.",
          threadSummaries: [
            [
              "mock-thread-alex",
              "Repeated demands for location sharing and account passwords across three weeks; looks like a pattern, peaking at severity 2.",
            ],
          ],
        }
      : { scan: null, report: null, threadSummaries: [] },
  listSafetyScans: async () =>
    mockActive && mockContentFindings.length
      ? mockBulk([
          {
            id: 3,
            model: "gemma-4-E4B-it-Q4_K_M",
            sources: "messages",
            rangeStart: Math.floor(new Date(2024, 0, 1).getTime() / 1000),
            rangeEnd: Math.floor(new Date(2025, 0, 1).getTime() / 1000) - 1,
            status: "completed" as const,
            startedAt: Math.floor(Date.now() / 1000) - 3600,
            finishedAt: Math.floor(Date.now() / 1000) - 3000,
            // Derived, not typed in: the rail badge and the detail pane read the
            // same fixture, and two hand-kept copies are how they would drift.
            ...mockScanTotals(),
            error: null,
          },
          {
            id: 2,
            model: "gemma-4-E4B-it-Q4_K_M",
            sources: "all",
            rangeStart: null,
            rangeEnd: null,
            status: "failed" as const,
            startedAt: Math.floor(Date.now() / 1000) - 90000,
            finishedAt: Math.floor(Date.now() / 1000) - 89700,
            findings: 0,
            serious: 0,
            harmful: 0,
            concerning: 0,
            error: "model server exited before the first chunk",
          },
          {
            id: 1,
            model: "gemma-3n-E2B-it-Q4_K_M",
            sources: "notes",
            rangeStart: null,
            rangeEnd: null,
            status: "completed" as const,
            startedAt: Math.floor(Date.now() / 1000) - 200000,
            finishedAt: Math.floor(Date.now() / 1000) - 199000,
            // Must match what listContentFindings(1) returns (its first mock
            // finding, severity 2) — the rail badge and the detail pane must
            // never disagree.
            findings: 1,
            serious: 0,
            harmful: 1,
            concerning: 0,
            error: null,
          },
          // A clean completed scan and a scan with a 4-digit count: the two row
          // states that stress the card's layout hardest (an empty-looking right
          // side, and the widest possible pill next to a long title). Without
          // them the layout could only ever be checked against the easy cases
          // (#92).
          {
            id: 4,
            model: "gemma-4-E4B-it-Q4_K_M",
            sources: "iMessage,TikTok",
            rangeStart: null,
            rangeEnd: null,
            status: "completed" as const,
            startedAt: Math.floor(Date.now() / 1000) - 300000,
            finishedAt: Math.floor(Date.now() / 1000) - 299400,
            findings: 0,
            serious: 0,
            harmful: 0,
            concerning: 0,
            error: null,
          },
          {
            id: 5,
            model: "gemma-4-E4B-it-Q4_K_M",
            sources: "messages,notes",
            rangeStart: null,
            rangeEnd: null,
            status: "completed" as const,
            startedAt: Math.floor(Date.now() / 1000) - 400000,
            finishedAt: Math.floor(Date.now() / 1000) - 380000,
            findings: 1284,
            serious: 12,
            harmful: 307,
            concerning: 965,
            error: null,
          },
          ].filter((s) => !mockDeletedScanIds.has(s.id)),
          (s, i) => ({ ...s, id: 100000 + i }),
        )
      : [],
  deleteSafetyScan: async (scanId) => {
    mockDeletedScanIds.add(scanId);
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
