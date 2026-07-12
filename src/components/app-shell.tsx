import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import {
  Boxes,
  Globe,
  HardDrive,
  Image,
  MessageSquare,
  NotebookText,
  Phone,
  Settings,
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
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { useSettings } from "@/components/settings-provider";

const nav = [
  { to: "/gallery", label: "Gallery", icon: Image },
  { to: "/messages", label: "Messages", icon: MessageSquare },
  { to: "/contacts", label: "Contacts", icon: Users },
  { to: "/calls", label: "Calls", icon: Phone },
  { to: "/safari", label: "Safari", icon: Globe },
  { to: "/notes", label: "Notes", icon: NotebookText },
  { to: "/apps", label: "Apps", icon: Boxes },
] as const;

export function AppShell() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });

  return (
    // h-svh pins the app to a FIXED viewport height. shadcn's SidebarProvider
    // only sets `min-h-svh`, which lets the layout grow with its content — so a
    // virtualized list's tall spacer would inflate the whole document and its
    // scroll container would never actually scroll (it just grows), defeating
    // every `min-h-0`/`overflow-auto` below. A fixed height gives the flex chain
    // something to constrain against so overflow scrolls instead of expanding.
    <SidebarProvider className="h-svh overflow-hidden">
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
          <div className="ml-auto flex items-center gap-1">
            <ModeToggle />
            <SettingsMenu />
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-hidden">
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

/** Gear button + dialog exposing the app-wide display preferences. */
function SettingsMenu() {
  const { showContactNames, setShowContactNames, showAvatars, setShowAvatars } =
    useSettings();

  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="Settings">
          <Settings className="size-4" />
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-6 py-2">
          <div className="flex items-center justify-between gap-4">
            <div className="flex flex-col gap-1">
              <Label htmlFor="show-contact-names">Show contact names</Label>
              <p className="text-muted-foreground text-sm">
                Display saved names instead of phone numbers.
              </p>
            </div>
            <Switch
              id="show-contact-names"
              checked={showContactNames}
              onCheckedChange={setShowContactNames}
            />
          </div>
          <div className="flex items-center justify-between gap-4">
            <div className="flex flex-col gap-1">
              <Label htmlFor="show-avatars">Show contact photos</Label>
              <p className="text-muted-foreground text-sm">
                Show contact avatars where available.
              </p>
            </div>
            <Switch
              id="show-avatars"
              checked={showAvatars}
              onCheckedChange={setShowAvatars}
            />
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
