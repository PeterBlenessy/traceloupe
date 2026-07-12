/**
 * Typed client for the Tauri command layer.
 *
 * Two implementations of the same interface: the real one over
 * `invoke()`, and a mock used when the app runs in a plain browser
 * (Vite dev server, Playwright). Views depend only on `SalvageClient`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

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
  | { phase: "parsing"; current: number; total: number; fraction: number; artifact: string }
  | { phase: "normalizing" };

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
}

export interface HistoryVisit {
  id: number;
  url: string;
  title: string | null;
  visitedAt: number | null;
  visitCount: number | null;
}

export interface Note {
  id: number;
  folder: string | null;
  title: string | null;
  snippet: string | null;
  body: string | null;
  createdAt: number | null;
  modifiedAt: number | null;
}

export interface LabeledValue {
  label: string | null;
  value: string;
}

export interface Contact {
  id: number;
  firstName: string | null;
  lastName: string | null;
  organization: string | null;
  phones: LabeledValue[];
  emails: LabeledValue[];
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

export interface Attachment {
  id: number;
  filename: string | null;
  mimeType: string | null;
  localPath: string | null;
}

export interface Message {
  id: number;
  isFromMe: boolean;
  sender: string | null;
  body: string | null;
  sentAt: number | null;
  attachments: Attachment[];
}

/** A message in the cross-conversation timeline, tagged with its thread. */
export interface TimelineMessage {
  threadId: number;
  threadTitle: string;
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

export interface SalvageClient {
  listBackups(root?: string): Promise<DiscoveryResult>;
  /** The default Finder/MobileSync backup folder, for seeding the picker. */
  defaultBackupRoot(): Promise<string | null>;
  /**
   * Open a native folder picker (defaulting to the MobileSync backup folder)
   * and return the chosen path, or null if cancelled. Selecting a folder grants
   * macOS access to it, sidestepping Full Disk Access.
   */
  pickBackupFolder(): Promise<string | null>;
  /** Open System Settings at the Full Disk Access pane. */
  openFullDiskAccessSettings(): Promise<void>;
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
  hasActiveBackup(): Promise<boolean>;
  openBackup(backupId: string): Promise<boolean>;
  /** Ids of backups already parsed (open instantly, no first-time read). */
  importedBackupIds(): Promise<string[]>;
  listThreads(): Promise<ThreadSummary[]>;
  /** Total messages in a thread; drives the lazily-loaded virtual scroller. */
  countThreadMessages(threadId: number): Promise<number>;
  /** A window of a thread's messages, oldest first, starting at `offset`. */
  getThreadMessageWindow(
    threadId: number,
    offset: number,
    limit: number,
  ): Promise<Message[]>;
  /** Total messages across all conversations; drives the timeline scroller. */
  countTimelineMessages(): Promise<number>;
  /** A window of the all-conversations timeline, oldest first, from `offset`. */
  getTimelineWindow(offset: number, limit: number): Promise<TimelineMessage[]>;
  /** Message counts for each half-open [lo, hi) epoch-second window. */
  countMessageRanges(ranges: TimeRange[]): Promise<number[]>;
  /** A window of messages whose time falls in [lo, hi), oldest first. */
  getRangeWindow(
    lo: number | null,
    hi: number | null,
    offset: number,
    limit: number,
  ): Promise<TimelineMessage[]>;
  listCalls(): Promise<Call[]>;
  listSafariHistory(): Promise<HistoryVisit[]>;
  listNotes(): Promise<Note[]>;
  listContacts(): Promise<Contact[]>;
  /** Bundle ids of apps that were installed on the device. */
  listInstalledApps(): Promise<string[]>;
  listMedia(): Promise<MediaItem[]>;
  mediaSources(): Promise<MediaSource[]>;
  // Windowed/filterable list queries (null filter = all), for lazy-loading
  // huge lists a slice at a time.
  countMedia(source: string | null): Promise<number>;
  getMediaWindow(source: string | null, offset: number, limit: number): Promise<MediaItem[]>;
  countCalls(search: string | null): Promise<number>;
  getCallsWindow(search: string | null, offset: number, limit: number): Promise<Call[]>;
  countSafari(search: string | null): Promise<number>;
  getSafariWindow(
    search: string | null,
    offset: number,
    limit: number,
  ): Promise<HistoryVisit[]>;
  /** URL the webview can load for a media item. `thumb` requests a thumbnail. */
  mediaUrl(id: number, opts?: { thumb?: boolean }): string;
  /** URL the webview can load for a contact's photo. */
  contactAvatarUrl(id: number): string;
  /** URL for a message attachment's bytes (`thumb` for an image thumbnail). */
  attachmentUrl(id: number, opts?: { thumb?: boolean }): string;
  /** Open an attachment's file with the OS default app (documents, etc.). */
  openAttachment(id: number): Promise<void>;
}

const tauriClient: SalvageClient = {
  listBackups: (root) => invoke<DiscoveryResult>("list_backups", { root }),
  defaultBackupRoot: () => invoke<string | null>("default_backup_root"),
  pickBackupFolder: async () => {
    const defaultPath = (await invoke<string | null>("default_backup_root")) ?? undefined;
    const chosen = await open({
      directory: true,
      multiple: false,
      title: "Choose an iPhone backup folder",
      defaultPath,
    });
    return typeof chosen === "string" ? chosen : null;
  },
  openFullDiskAccessSettings: () => invoke<void>("open_full_disk_access_settings"),
  engineStatus: () => invoke<boolean>("engine_status"),
  engineInfo: () => invoke<EngineInfo>("engine_info"),
  installEngine: () => invoke<void>("install_engine"),
  onEngineProgress: (cb) => listen<EngineProgress>("engine://progress", (e) => cb(e.payload)),
  listImportModules: () => invoke<ImportModule[]>("list_import_modules"),
  importBackup: (args) => invoke<ImportResult>("import_backup", args),
  onImportProgress: (cb) => listen<ImportProgress>("import://progress", (e) => cb(e.payload)),
  hasActiveBackup: () => invoke<boolean>("has_active_backup"),
  openBackup: (backupId) => invoke<boolean>("open_backup", { backupId }),
  importedBackupIds: () => invoke<string[]>("imported_backup_ids"),
  listThreads: () => invoke<ThreadSummary[]>("list_threads"),
  countThreadMessages: (threadId) =>
    invoke<number>("count_thread_messages", { threadId }),
  getThreadMessageWindow: (threadId, offset, limit) =>
    invoke<Message[]>("get_thread_message_window", { threadId, offset, limit }),
  countTimelineMessages: () => invoke<number>("count_timeline_messages"),
  getTimelineWindow: (offset, limit) =>
    invoke<TimelineMessage[]>("get_timeline_window", { offset, limit }),
  countMessageRanges: (ranges) =>
    invoke<number[]>("count_message_ranges", { ranges }),
  getRangeWindow: (lo, hi, offset, limit) =>
    invoke<TimelineMessage[]>("get_range_window", { lo, hi, offset, limit }),
  listCalls: () => invoke<Call[]>("list_calls"),
  listSafariHistory: () => invoke<HistoryVisit[]>("list_safari_history"),
  listNotes: () => invoke<Note[]>("list_notes"),
  countMedia: (source) => invoke<number>("count_media", { source }),
  getMediaWindow: (source, offset, limit) =>
    invoke<MediaItem[]>("get_media_window", { source, offset, limit }),
  countCalls: (search) => invoke<number>("count_calls", { search }),
  getCallsWindow: (search, offset, limit) =>
    invoke<Call[]>("get_calls_window", { search, offset, limit }),
  countSafari: (search) => invoke<number>("count_safari", { search }),
  getSafariWindow: (search, offset, limit) =>
    invoke<HistoryVisit[]>("get_safari_window", { search, offset, limit }),
  listContacts: () => invoke<Contact[]>("list_contacts"),
  listInstalledApps: () => invoke<string[]>("list_installed_apps"),
  listMedia: () => invoke<MediaItem[]>("list_media"),
  mediaSources: () => invoke<MediaSource[]>("media_sources"),
  // Served by the register_uri_scheme_protocol handler in the Rust shell.
  mediaUrl: (id, opts) =>
    `salvage-media://localhost/${id}${opts?.thumb ? "?thumb=1" : ""}`,
  contactAvatarUrl: (id) => `salvage-avatar://localhost/${id}`,
  attachmentUrl: (id, opts) =>
    `salvage-attachment://localhost/${id}${opts?.thumb ? "?thumb=1" : ""}`,
  openAttachment: (id) => invoke<void>("open_attachment", { attachmentId: id }),
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
    { id: 1, isFromMe: false, sender: "+15551234567", body: "Hey, are you around this weekend?", sentAt: 1717840800, attachments: [] },
    { id: 2, isFromMe: true, sender: null, body: "Yeah! What did you have in mind?", sentAt: 1717840980, attachments: [] },
    { id: 3, isFromMe: false, sender: "+15551234567", body: "Thinking of hiking Mission Peak", sentAt: 1717841100, attachments: [] },
    { id: 4, isFromMe: true, sender: null, body: "I'm in. Saturday morning?", sentAt: 1717841220, attachments: [] },
    { id: 5, isFromMe: false, sender: "+15551234567", body: "Here's the itinerary", sentAt: 1717841340, attachments: [{ id: 2, filename: "itinerary.pdf", mimeType: "application/pdf", localPath: "/mock/itinerary.pdf" }] },
    { id: 6, isFromMe: true, sender: null, body: "Here's the trailhead 📷", sentAt: 1717841460, attachments: [{ id: 1, filename: "salvage-test.png", mimeType: "image/png", localPath: "/mock/salvage-test.png" }] },
  ],
  2: [
    { id: 7, isFromMe: true, sender: null, body: "Landing at 6, boarding now", sentAt: 1717499000, attachments: [] },
    { id: 8, isFromMe: false, sender: "Mom", body: "Call me when you land ❤️", sentAt: 1717500000, attachments: [] },
  ],
  5: [
    { id: 9, isFromMe: false, sender: "★ hembokke", body: "have you seen this one 😂", sentAt: 1717599000, attachments: [] },
    { id: 10, isFromMe: true, sender: null, body: "sent you a video 🎵", sentAt: 1717600000, attachments: [] },
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
  attachments: [],
}));
mockMessages[4] = [
  { id: 2000, isFromMe: false, sender: "+15559876543", body: "Who's in for Saturday?", sentAt: 1717841600, attachments: [] },
  { id: 2001, isFromMe: true, sender: null, body: "I'm in!", sentAt: 1717841650, attachments: [] },
  { id: 2002, isFromMe: false, sender: "+15550001111", body: "See you at the trailhead!", sentAt: 1717841700, attachments: [] },
];

// All mock messages flattened into one chronological stream, for the timeline.
const mockTimeline: TimelineMessage[] = mockThreads
  .flatMap((t) =>
    (mockMessages[t.id] ?? []).map((message) => ({
      threadId: t.id,
      threadTitle: t.displayName ?? t.identifier,
      service: t.service,
      message,
    })),
  )
  .sort((a, b) => (a.message.sentAt ?? 0) - (b.message.sentAt ?? 0));

function inRange(sentAt: number | null, r: TimeRange): boolean {
  if (sentAt == null) return false;
  return (r.lo == null || sentAt >= r.lo) && (r.hi == null || sentAt < r.hi);
}

const mockCalls: Call[] = [
  { id: 1, address: "friend@icloud.com", direction: "incoming", answered: true, durationS: 128, occurredAt: 1717786800, service: "FaceTime Audio" },
  { id: 2, address: "+15559876543", direction: "incoming", answered: false, durationS: 0, occurredAt: 1717785000, service: "Phone Call" },
  { id: 3, address: "+15551234567", direction: "outgoing", answered: true, durationS: 312, occurredAt: 1717783200, service: "Phone Call" },
];

const mockSafari: HistoryVisit[] = [
  { id: 1, url: "https://en.wikipedia.org/wiki/Mission_Peak", title: "Mission Peak - Wikipedia", visitedAt: 1717801200, visitCount: 2 },
  { id: 2, url: "https://news.ycombinator.com/", title: "Hacker News", visitedAt: 1717797600, visitCount: 34 },
  { id: 3, url: "https://www.apple.com/", title: "Apple", visitedAt: 1717794000, visitCount: 12 },
];

const mockNotes: Note[] = [
  { id: 1, folder: "Notes", title: "Hike checklist", snippet: "Water, snacks, sunscreen…", body: "Water\nSnacks\nSunscreen\nHat\nExtra socks", createdAt: 1717000000, modifiedAt: 1717838000 },
  { id: 2, folder: "Work", title: "Q3 ideas", snippet: "Ship the importer, then…", body: "Ship the importer, then work on lazy decode and the encrypted path.", createdAt: 1716500000, modifiedAt: 1717500000 },
  { id: 3, folder: "Notes", title: null, snippet: "Grocery list", body: "Milk\nEggs\nBröd\nKaffe", createdAt: 1716000000, modifiedAt: 1716600000 },
];

const mockContacts: Contact[] = [
  { id: 1, firstName: "Jordan", lastName: "Kim", organization: "Acme Corp", phones: [{ label: "Work", value: "+15559876543" }], emails: [{ label: "Work", value: "jordan@acme.example" }], hasImage: true, source: "Address Book" },
  { id: 2, firstName: "Alex", lastName: "Rivera", organization: null, phones: [{ label: "Mobile", value: "+15551234567" }], emails: [{ label: "Home", value: "alex@example.com" }], hasImage: true, source: "Address Book" },
  { id: 3, firstName: "Sam", lastName: "Taylor", organization: null, phones: [], emails: [{ label: "Home", value: "sam.taylor@example.com" }], hasImage: false, source: "Address Book" },
  { id: 4, firstName: null, lastName: null, organization: "Bella Vista Pizza", phones: [{ label: "Mobile", value: "+15550001111" }], emails: [], hasImage: false, source: "Address Book" },
  // A third-party app's social graph: name + @handle only (behind the filter).
  { id: 5, firstName: "★ Alice ✿", lastName: null, organization: "@ccidkk", phones: [], emails: [], hasImage: false, source: "TikTok" },
  { id: 6, firstName: "jhopesop", lastName: null, organization: "@jhopesop", phones: [], emails: [], hasImage: false, source: "TikTok" },
];

// Colored initials SVGs standing in for real contact photos in the browser mock.
const mockAvatarColors: Record<number, string> = { 1: "#7c3aed", 2: "#0891b2" };
function mockAvatarDataUrl(id: number): string {
  const color = mockAvatarColors[id] ?? "#888";
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='96' height='96'><rect width='96' height='96' fill='${color}'/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

const mockMedia: MediaItem[] = [
  { id: 1, kind: "photo", source: "Messages", mimeType: "image/png", filename: "salvage-test.png", takenAt: 1717841460 },
  { id: 2, kind: "photo", source: "Messages", mimeType: "image/png", filename: "sunset.png", takenAt: 1717841520 },
  { id: 3, kind: "photo", source: "Photos", mimeType: "image/png", filename: "forest.png", takenAt: 1717841580 },
  { id: 4, kind: "photo", source: "WhatsApp", mimeType: "image/heic", filename: "IMG_0421.heic", takenAt: 1717841640 },
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

// A realistic mix: some Salvage-supported apps, some not, plus system apps.
const mockInstalledApps = [
  "net.whatsapp.WhatsApp",
  "com.burbn.instagram",
  "com.toyopagroup.picaboo", // Snapchat
  "com.zhiliaoapp.musically", // TikTok
  "org.telegram.messenger",
  "com.spotify.client",
  "com.apple.mobilesafari",
  "com.google.Gmail",
  "com.tinyspeck.chatlyio", // Slack
  "com.ubercab.UberClient",
];

let mockActive = false;
const mockImported = new Set<string>();

// Mock-side filters mirroring the backend's windowed SQL, so the browser mock
// behaves like the real windowed/filterable queries.
function mockFilterMedia(source: string | null): MediaItem[] {
  return source ? mockMedia.filter((m) => (m.source ?? "Other") === source) : mockMedia;
}
function mockFilterCalls(search: string | null): Call[] {
  if (!search) return mockCalls;
  const q = search.toLowerCase();
  return mockCalls.filter((c) => c.address?.toLowerCase().includes(q));
}
function mockFilterSafari(search: string | null): HistoryVisit[] {
  if (!search) return mockSafari;
  const q = search.toLowerCase();
  return mockSafari.filter(
    (h) => h.url.toLowerCase().includes(q) || (h.title?.toLowerCase().includes(q) ?? false),
  );
}

// A mock progress emitter so the import flow is exercisable in the browser.
type ProgressCb = (p: ImportProgress) => void;
const mockProgressSubs = new Set<ProgressCb>();

const mockEngineSubs = new Set<(p: EngineProgress) => void>();

export const mockClient: SalvageClient = {
  listBackups: async () => ({ status: "ok", backups: mockBackups }),
  defaultBackupRoot: async () =>
    "/Users/dev/Library/Application Support/MobileSync/Backup",
  pickBackupFolder: async () =>
    "/Users/dev/Library/Application Support/MobileSync/Backup",
  openFullDiskAccessSettings: async () => {},
  engineStatus: async () => true,
  engineInfo: async () => ({ installed: true, version: "iLEAPP v2026.1.0", canDownload: true }),
  installEngine: async () => {
    for (let i = 1; i <= 5; i++) {
      await new Promise((r) => setTimeout(r, 200));
      mockEngineSubs.forEach((cb) =>
        cb({ phase: "downloading", received: i * 15_000_000, total: 78_000_000, fraction: i / 5 }),
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
    { id: "messages", label: "Messages", category: "Communication", default: true },
    { id: "calls", label: "Call history", category: "Communication", default: true },
    { id: "contacts", label: "Contacts", category: "Communication", default: true },
    { id: "safari", label: "Safari history", category: "Web", default: true },
    { id: "notes", label: "Notes", category: "Productivity", default: true },
    { id: "camera_roll", label: "Camera roll photos", category: "Media", default: true },
  ],
  importBackup: async ({ backupId }) => {
    const artifacts = ["contacts", "callHistory", "safariHistory", "notes", "sms"];
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
    await new Promise((r) => setTimeout(r, 200));
    mockProgressSubs.forEach((cb) => cb({ phase: "normalizing" }));
    await new Promise((r) => setTimeout(r, 300));
    mockActive = true;
    mockImported.add(backupId);
    return { cachePath: "/mock/cache.db", threads: 2, messages: 8, mediaItems: 4, calls: 3, safariVisits: 3, contacts: 4, warnings: [] };
  },
  onImportProgress: async (cb) => {
    mockProgressSubs.add(cb);
    return () => mockProgressSubs.delete(cb);
  },
  hasActiveBackup: async () => mockActive,
  openBackup: async (backupId) => {
    if (!mockImported.has(backupId)) return false;
    mockActive = true;
    return true;
  },
  importedBackupIds: async () => [...mockImported],
  listThreads: async () => (mockActive ? mockThreads : []),
  countThreadMessages: async (threadId) =>
    mockActive ? (mockMessages[threadId]?.length ?? 0) : 0,
  getThreadMessageWindow: async (threadId, offset, limit) =>
    mockActive ? (mockMessages[threadId] ?? []).slice(offset, offset + limit) : [],
  countTimelineMessages: async () => (mockActive ? mockTimeline.length : 0),
  getTimelineWindow: async (offset, limit) =>
    mockActive ? mockTimeline.slice(offset, offset + limit) : [],
  countMessageRanges: async (ranges) =>
    ranges.map((r) =>
      mockActive
        ? mockTimeline.filter((t) => inRange(t.message.sentAt, r)).length
        : 0,
    ),
  getRangeWindow: async (lo, hi, offset, limit) =>
    mockActive
      ? mockTimeline
          .filter((t) => inRange(t.message.sentAt, { lo, hi }))
          .slice(offset, offset + limit)
      : [],
  listCalls: async () => (mockActive ? mockCalls : []),
  listSafariHistory: async () => (mockActive ? mockSafari : []),
  listNotes: async () => (mockActive ? mockNotes : []),
  countMedia: async (source) => (mockActive ? mockFilterMedia(source).length : 0),
  getMediaWindow: async (source, offset, limit) =>
    mockActive ? mockFilterMedia(source).slice(offset, offset + limit) : [],
  countCalls: async (search) => (mockActive ? mockFilterCalls(search).length : 0),
  getCallsWindow: async (search, offset, limit) =>
    mockActive ? mockFilterCalls(search).slice(offset, offset + limit) : [],
  countSafari: async (search) => (mockActive ? mockFilterSafari(search).length : 0),
  getSafariWindow: async (search, offset, limit) =>
    mockActive ? mockFilterSafari(search).slice(offset, offset + limit) : [],
  listContacts: async () => (mockActive ? mockContacts : []),
  listInstalledApps: async () => (mockActive ? mockInstalledApps : []),
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
  openAttachment: async () => {},
};

const isTauri = "__TAURI_INTERNALS__" in window;

export const client: SalvageClient = isTauri ? tauriClient : mockClient;
