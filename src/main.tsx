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
import { AppShell } from "@/components/app-shell";
import { BackupPicker } from "@/views/backup-picker";
import { MessagesView } from "@/views/messages";
import { CallsView } from "@/views/calls";
import { SafariView } from "@/views/safari";
import { Placeholder } from "@/views/placeholder";

const rootRoute = createRootRoute({ component: AppShell });

const routes = [
  createRoute({ getParentRoute: () => rootRoute, path: "/", component: BackupPicker }),
  createRoute({ getParentRoute: () => rootRoute, path: "/gallery", component: () => <Placeholder title="Gallery" /> }),
  createRoute({ getParentRoute: () => rootRoute, path: "/messages", component: MessagesView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/contacts", component: () => <Placeholder title="Contacts" /> }),
  createRoute({ getParentRoute: () => rootRoute, path: "/calls", component: CallsView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/safari", component: SafariView }),
  createRoute({ getParentRoute: () => rootRoute, path: "/notes", component: () => <Placeholder title="Notes" /> }),
  createRoute({ getParentRoute: () => rootRoute, path: "/browser", component: () => <Placeholder title="App Data" /> }),
];

const router = createRouter({ routeTree: rootRoute.addChildren(routes) });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

const queryClient = new QueryClient();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
