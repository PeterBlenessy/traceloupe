import { Link, Outlet } from "@tanstack/react-router";
import {
  Globe,
  HardDrive,
  Image,
  MessageSquare,
  NotebookText,
  Phone,
  Search,
  Users,
} from "lucide-react";
import { cn } from "@/lib/utils";

const nav = [
  { to: "/gallery", label: "Gallery", icon: Image },
  { to: "/messages", label: "Messages", icon: MessageSquare },
  { to: "/contacts", label: "Contacts", icon: Users },
  { to: "/calls", label: "Calls", icon: Phone },
  { to: "/safari", label: "Safari", icon: Globe },
  { to: "/notes", label: "Notes", icon: NotebookText },
  { to: "/browser", label: "App Data", icon: Search },
] as const;

export function AppShell() {
  return (
    <div className="flex h-screen">
      <aside className="flex w-52 shrink-0 flex-col border-r bg-muted/40">
        <Link
          to="/"
          className="flex items-center gap-2 px-4 py-4 text-sm font-semibold"
        >
          <HardDrive className="size-4" />
          Salvage
        </Link>
        <nav className="flex flex-col gap-0.5 px-2">
          {nav.map(({ to, label, icon: Icon }) => (
            <Link
              key={to}
              to={to}
              className={cn(
                "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm text-muted-foreground",
                "hover:bg-accent hover:text-accent-foreground",
                "[&.active]:bg-accent [&.active]:text-accent-foreground [&.active]:font-medium",
              )}
            >
              <Icon className="size-4" />
              {label}
            </Link>
          ))}
        </nav>
      </aside>
      <main className="flex-1 overflow-auto">
        <Outlet />
      </main>
    </div>
  );
}
