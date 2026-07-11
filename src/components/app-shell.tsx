import { Link, Outlet, useRouterState } from "@tanstack/react-router";
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
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { ModeToggle } from "@/components/mode-toggle";

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
  const pathname = useRouterState({ select: (s) => s.location.pathname });

  return (
    <SidebarProvider>
      <Sidebar>
        <SidebarHeader>
          <Link to="/" className="flex items-center gap-2 px-2 py-1.5 font-semibold">
            <HardDrive className="size-4" />
            Salvage
          </Link>
        </SidebarHeader>
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupContent>
              <SidebarMenu>
                {nav.map(({ to, label, icon: Icon }) => (
                  <SidebarMenuItem key={to}>
                    <SidebarMenuButton asChild isActive={pathname === to}>
                      <Link to={to}>
                        <Icon />
                        <span>{label}</span>
                      </Link>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
      </Sidebar>
      <SidebarInset>
        {/* A slim bar carrying the sidebar toggle and theme control; views
            render their own headers below it. */}
        <div className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
          <SidebarTrigger />
          <div className="ml-auto">
            <ModeToggle />
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-hidden">
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}
