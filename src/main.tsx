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
import { SettingsProvider } from "@/components/settings-provider";
import { AppShell } from "@/components/app-shell";
import { BackupPicker } from "@/views/backup-picker";
import { PhotosView } from "@/views/photos";
import { MessagesView } from "@/views/messages";
import { ContactsView } from "@/views/contacts";
import { CallsView } from "@/views/calls";
import { SafariView } from "@/views/safari";
import { NotesView } from "@/views/notes";
import { AppsView } from "@/views/apps";

const rootRoute = createRootRoute({ component: AppShell });

const routes = [
  createRoute({ getParentRoute: () => rootRoute, path: "/", component: BackupPicker }),
  createRoute({ getParentRoute: () => rootRoute, path: "/photos", component: PhotosView }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/messages",
    // `?thread=<id>` deep-links to a conversation (e.g. from a contact).
    validateSearch: (search: Record<string, unknown>): { thread?: number } => {
      const t = Number(search.thread);
      return Number.isFinite(t) ? { thread: t } : {};
    },
    component: MessagesView,
  }),
  createRoute({ getParentRoute: () => rootRoute, path: "/contacts", component: ContactsView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/calls", component: CallsView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/safari", component: SafariView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/notes", component: NotesView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/apps", component: AppsView }),
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
        </QueryClientProvider>
      </SettingsProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
