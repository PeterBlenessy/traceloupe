import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import "./index.css";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { SettingsProvider } from "@/components/settings-provider";
import { AppShell } from "@/components/app-shell";
import { BackupPicker } from "@/views/backup-picker";
import { PhotosView } from "@/views/photos";
import { MessagesView } from "@/views/messages";
import { ContactsView } from "@/views/contacts";
import { CallsView } from "@/views/calls";
import { SafariView } from "@/views/safari";
import { NotesView } from "@/views/notes";
import { RecordingsView } from "@/views/recordings";
import { AppsView } from "@/views/apps";
import { DeviceView } from "@/views/device";
import { SecurityView } from "@/views/security";
import { SafetyScanView } from "@/views/safety-scan";
import { CalendarView } from "@/views/calendar";
import { RemindersView } from "@/views/reminders";
import { ArtifactsView } from "@/views/artifacts";
import { HealthView } from "@/views/health";
import { InteractionsView } from "@/views/interactions";

const rootRoute = createRootRoute({ component: AppShell });

const routes = [
  // `/` is the app's one home: the backup picker before a backup is open, and
  // the Device view (full device detail) once one is. `?choose` forces the
  // picker back so the user can switch backups.
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    validateSearch: (search: Record<string, unknown>): { choose?: true } =>
      search.choose ? { choose: true } : {},
    component: BackupPicker,
  }),
  createRoute({ getParentRoute: () => rootRoute, path: "/photos", component: PhotosView }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/messages",
    // `?thread=<id>` deep-links to a conversation (e.g. from a contact);
    // `?service=<label>` preselects the service filter (e.g. from the Apps view);
    // `?from=safety` adds a "Back to Safety Scan" return chip (round-trip from a
    // finding, mirroring the Notes deep-link).
    validateSearch: (
      search: Record<string, unknown>,
    ): {
      thread?: number;
      service?: string;
      from?: "safety";
      message?: number;
      /** The finding this was opened from, so the return chip can go back to
       *  it rather than to the top of the list (#224). */
      finding?: number;
    } => {
      const t = Number(search.thread);
      const m = Number(search.message);
      const fid = Number(search.finding);
      const service =
        typeof search.service === "string" ? search.service : undefined;
      return {
        ...(Number.isFinite(t) ? { thread: t } : {}),
        ...(service ? { service } : {}),
        ...(search.from === "safety" ? { from: "safety" as const } : {}),
        ...(Number.isFinite(m) ? { message: m } : {}),
        ...(Number.isFinite(fid) && fid > 0 ? { finding: fid } : {}),
      };
    },
    component: MessagesView,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/contacts",
    // `?id=<contactId>` deep-links to a contact (e.g. from a message avatar).
    validateSearch: (search: Record<string, unknown>): { id?: number } => {
      const id = Number(search.id);
      return Number.isFinite(id) ? { id } : {};
    },
    component: ContactsView,
  }),
  createRoute({ getParentRoute: () => rootRoute, path: "/calls", component: CallsView }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/artifacts",
    component: ArtifactsView,
  }),
  createRoute({ getParentRoute: () => rootRoute, path: "/safari", component: SafariView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/notes", component: NotesView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/recordings", component: RecordingsView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/apps", component: AppsView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/device", component: DeviceView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/security", component: SecurityView }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/safety-scan",
    // `?finding=<id>` returns to a specific finding — the round trip out to a
    // conversation and back. Without it the return could only ever land on the
    // top of the list, which is not "back" (#224).
    validateSearch: (search: Record<string, unknown>): { finding?: number } => {
      const f = Number(search.finding);
      return Number.isFinite(f) && f > 0 ? { finding: f } : {};
    },
    component: SafetyScanView,
  }),
  createRoute({ getParentRoute: () => rootRoute, path: "/calendar", component: CalendarView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/reminders", component: RemindersView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/health", component: HealthView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/interactions", component: InteractionsView }),
];

const router = createRouter({ routeTree: rootRoute.addChildren(routes) });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

// Backup data is immutable within a session, so treat every query as fresh and
// never auto-refetch. Without this, React Query's default refetch-on-focus
// re-runs heavy queries (e.g. a 68k-message thread) on every window focus,
// re-freezing the app. Explicit invalidateQueries() on import/open still forces
// a reload when the active backup actually changes.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: Infinity,
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
      retry: false,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <SettingsProvider>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
          {/* No close button: toasts auto-dismiss, and its "x" overlapped modal
              dialogs — a click there hit the dialog behind it, not the toast. */}
          <Toaster richColors />
        </QueryClientProvider>
      </SettingsProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
