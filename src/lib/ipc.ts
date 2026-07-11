/**
 * Typed client for the Tauri command layer.
 *
 * Two implementations of the same interface: the real one over
 * `invoke()`, and a mock used when the app runs in a plain browser
 * (Vite dev server, Playwright). Views depend only on `SalvageClient`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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
}

const tauriClient: SalvageClient = {
  listBackups: (root) => invoke<DiscoveryResult>("list_backups", { root }),
  engineStatus: () => invoke<boolean>("engine_status"),
  importBackup: (args) => invoke<ImportResult>("import_backup", args),
  onImportProgress: (cb) => listen<ImportProgress>("import://progress", (e) => cb(e.payload)),
  hasActiveBackup: () => invoke<boolean>("has_active_backup"),
  openBackup: (backupId) => invoke<boolean>("open_backup", { backupId }),
  listThreads: () => invoke<ThreadSummary[]>("list_threads"),
  getThreadMessages: (threadId) => invoke<Message[]>("get_thread_messages", { threadId }),
  listCalls: () => invoke<Call[]>("list_calls"),
  listSafariHistory: () => invoke<HistoryVisit[]>("list_safari_history"),
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

let mockActive = false;

// A mock progress emitter so the import flow is exercisable in the browser.
type ProgressCb = (p: ImportProgress) => void;
const mockProgressSubs = new Set<ProgressCb>();

export const mockClient: SalvageClient = {
  listBackups: async () => ({ status: "ok", backups: mockBackups }),
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
    return { cachePath: "/mock/cache.db", threads: 2, messages: 8, mediaItems: 1, calls: 3, safariVisits: 3, warnings: [] };
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
};

const isTauri = "__TAURI_INTERNALS__" in window;

export const client: SalvageClient = isTauri ? tauriClient : mockClient;
