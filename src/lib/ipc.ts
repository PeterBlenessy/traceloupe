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
}

export interface Attachment {
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
  importBackup(args: {
    backupPath: string;
    backupId: string;
    password: string;
  }): Promise<ImportResult>;
  /** Subscribe to import progress events. Returns an unsubscribe fn. */
  onImportProgress(cb: (p: ImportProgress) => void): Promise<UnlistenFn>;
  hasActiveBackup(): Promise<boolean>;
  openBackup(backupId: string): Promise<boolean>;
  listThreads(): Promise<ThreadSummary[]>;
  getThreadMessages(threadId: number): Promise<Message[]>;
  listCalls(): Promise<Call[]>;
  listSafariHistory(): Promise<HistoryVisit[]>;
  listContacts(): Promise<Contact[]>;
  listMedia(): Promise<MediaItem[]>;
  mediaSources(): Promise<MediaSource[]>;
  /** URL the webview can load for a media item. `thumb` requests a thumbnail. */
  mediaUrl(id: number, opts?: { thumb?: boolean }): string;
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
  importBackup: (args) => invoke<ImportResult>("import_backup", args),
  onImportProgress: (cb) => listen<ImportProgress>("import://progress", (e) => cb(e.payload)),
  hasActiveBackup: () => invoke<boolean>("has_active_backup"),
  openBackup: (backupId) => invoke<boolean>("open_backup", { backupId }),
  listThreads: () => invoke<ThreadSummary[]>("list_threads"),
  getThreadMessages: (threadId) => invoke<Message[]>("get_thread_messages", { threadId }),
  listCalls: () => invoke<Call[]>("list_calls"),
  listSafariHistory: () => invoke<HistoryVisit[]>("list_safari_history"),
  listContacts: () => invoke<Contact[]>("list_contacts"),
  listMedia: () => invoke<MediaItem[]>("list_media"),
  mediaSources: () => invoke<MediaSource[]>("media_sources"),
  // Served by the register_uri_scheme_protocol handler in the Rust shell.
  mediaUrl: (id, opts) =>
    `salvage-media://localhost/${id}${opts?.thumb ? "?thumb=1" : ""}`,
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
    id: 1,
    identifier: "+15551234567",
    displayName: "+15551234567",
    service: "iMessage",
    lastMessageAt: 1717841460,
    messageCount: 6,
    snippet: "Here's the trailhead 📷",
  },
  {
    id: 2,
    identifier: "Mom",
    displayName: "Mom",
    service: "SMS",
    lastMessageAt: 1717500000,
    messageCount: 2,
    snippet: "Call me when you land ❤️",
  },
];

const mockMessages: Record<number, Message[]> = {
  1: [
    { id: 1, isFromMe: false, sender: "+15551234567", body: "Hey, are you around this weekend?", sentAt: 1717840800, attachments: [] },
    { id: 2, isFromMe: true, sender: null, body: "Yeah! What did you have in mind?", sentAt: 1717840980, attachments: [] },
    { id: 3, isFromMe: false, sender: "+15551234567", body: "Thinking of hiking Mission Peak", sentAt: 1717841100, attachments: [] },
    { id: 4, isFromMe: true, sender: null, body: "I'm in. Saturday morning?", sentAt: 1717841220, attachments: [] },
    { id: 5, isFromMe: false, sender: "+15551234567", body: "Perfect, I'll pick you up at 8", sentAt: 1717841340, attachments: [] },
    { id: 6, isFromMe: true, sender: null, body: "Here's the trailhead 📷", sentAt: 1717841460, attachments: [{ filename: "salvage-test.png", mimeType: "image/png", localPath: null }] },
  ],
  2: [
    { id: 7, isFromMe: true, sender: null, body: "Landing at 6, boarding now", sentAt: 1717499000, attachments: [] },
    { id: 8, isFromMe: false, sender: "Mom", body: "Call me when you land ❤️", sentAt: 1717500000, attachments: [] },
  ],
};

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

const mockContacts: Contact[] = [
  { id: 1, firstName: "Jordan", lastName: "Kim", organization: "Acme Corp", phones: [{ label: "Work", value: "+15559876543" }], emails: [{ label: "Work", value: "jordan@acme.example" }] },
  { id: 2, firstName: "Alex", lastName: "Rivera", organization: null, phones: [{ label: "Mobile", value: "+15551234567" }], emails: [{ label: "Home", value: "alex@example.com" }] },
  { id: 3, firstName: "Sam", lastName: "Taylor", organization: null, phones: [], emails: [{ label: "Home", value: "sam.taylor@example.com" }] },
  { id: 4, firstName: null, lastName: null, organization: "Bella Vista Pizza", phones: [{ label: "Mobile", value: "+15550001111" }], emails: [] },
];

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

let mockActive = false;

// A mock progress emitter so the import flow is exercisable in the browser.
type ProgressCb = (p: ImportProgress) => void;
const mockProgressSubs = new Set<ProgressCb>();

export const mockClient: SalvageClient = {
  listBackups: async () => ({ status: "ok", backups: mockBackups }),
  defaultBackupRoot: async () =>
    "/Users/dev/Library/Application Support/MobileSync/Backup",
  pickBackupFolder: async () =>
    "/Users/dev/Library/Application Support/MobileSync/Backup",
  openFullDiskAccessSettings: async () => {},
  engineStatus: async () => true,
  importBackup: async () => {
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
    return { cachePath: "/mock/cache.db", threads: 2, messages: 8, mediaItems: 4, calls: 3, safariVisits: 3, contacts: 4, warnings: [] };
  },
  onImportProgress: async (cb) => {
    mockProgressSubs.add(cb);
    return () => mockProgressSubs.delete(cb);
  },
  hasActiveBackup: async () => mockActive,
  openBackup: async () => {
    mockActive = true;
    return true;
  },
  listThreads: async () => (mockActive ? mockThreads : []),
  getThreadMessages: async (threadId) => (mockActive ? (mockMessages[threadId] ?? []) : []),
  listCalls: async () => (mockActive ? mockCalls : []),
  listSafariHistory: async () => (mockActive ? mockSafari : []),
  listContacts: async () => (mockActive ? mockContacts : []),
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
};

const isTauri = "__TAURI_INTERNALS__" in window;

export const client: SalvageClient = isTauri ? tauriClient : mockClient;
