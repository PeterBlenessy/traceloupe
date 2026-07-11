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
  warnings: string[];
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
}

const tauriClient: SalvageClient = {
  listBackups: (root) => invoke<DiscoveryResult>("list_backups", { root }),
  engineStatus: () => invoke<boolean>("engine_status"),
  importBackup: (args) => invoke<ImportResult>("import_backup", args),
  onImportProgress: (cb) => listen<ImportProgress>("import://progress", (e) => cb(e.payload)),
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
    return { cachePath: "/mock/cache.db", threads: 1, messages: 6, mediaItems: 1, warnings: [] };
  },
  onImportProgress: async (cb) => {
    mockProgressSubs.add(cb);
    return () => mockProgressSubs.delete(cb);
  },
};

const isTauri = "__TAURI_INTERNALS__" in window;

export const client: SalvageClient = isTauri ? tauriClient : mockClient;
