import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import {
  Boxes,
  Globe,
  HardDrive,
  Image,
  Loader2,
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
import { Switch } from "@/components/ui/switch";
import { useQuery } from "@tanstack/react-query";
import { useSettings } from "@/components/settings-provider";
import { ImportProvider, useImport } from "@/components/import-provider";
import { client, type LogLevel } from "@/lib/ipc";
import { type ClockFormat } from "@/lib/format";

const nav = [
  { to: "/photos", label: "Photos", icon: Image },
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
    // ImportProvider lives above the routes so an import survives "run in
    // background" and navigation between views.
    <ImportProvider>
    {/* h-svh pins the app to a FIXED viewport height. shadcn's SidebarProvider
        only sets `min-h-svh`, which lets the layout grow with its content — so a
        virtualized list's tall spacer would inflate the whole document and its
        scroll container would never actually scroll (it just grows), defeating
        every `min-h-0`/`overflow-auto` below. A fixed height gives the flex chain
        something to constrain against so overflow scrolls instead of expanding. */}
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
            <ImportIndicator />
            <ModeToggle />
            <SettingsMenu />
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-hidden">
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
    </ImportProvider>
  );
}

/** A pill shown while an import runs in the background; click to reopen it. */
function ImportIndicator() {
  const { active, backgrounded, reopen } = useImport();
  if (!backgrounded || !active) return null;
  const p = active.progress;
  const detail =
    p?.phase === "normalizing"
      ? `Organizing ${p.step.toLowerCase()}…`
      : p?.phase === "parsing"
        ? `Reading ${p.artifact}…`
        : "starting…";
  return (
    <button
      onClick={reopen}
      title="Reopen import"
      className="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent"
    >
      <Loader2 className="size-3 animate-spin" />
      <span className="max-w-[16rem] truncate">
        Importing {active.backup.deviceName ?? active.backup.id} · {detail}
      </span>
    </button>
  );
}

/** Gear button + dialog exposing the app-wide display preferences. */
function SettingsMenu() {
  const {
    showContactNames,
    setShowContactNames,
    showAvatars,
    setShowAvatars,
    importModules,
    setImportModules,
    logLevel,
    setLogLevel,
    clockFormat,
    setClockFormatPref,
  } = useSettings();
  const { data: catalog } = useQuery({
    queryKey: ["importModules"],
    queryFn: () => client.listImportModules(),
  });
  // Effective selection: the user's saved choice, or every default.
  const selected = importModules ?? catalog?.filter((m) => m.default).map((m) => m.id) ?? [];
  const toggleModule = (id: string, on: boolean) => {
    const base = selected;
    setImportModules(on ? [...new Set([...base, id])] : base.filter((x) => x !== id));
  };

  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="Settings">
          <Settings className="size-4" />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-h-[85vh] gap-5 overflow-y-auto rounded-2xl sm:max-w-lg">
        <DialogHeader className="items-center">
          <DialogTitle className="text-center text-base">Settings</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-6">
          <SettingsGroup title="Display">
            <SettingsRow label="Show contact names" description="Display saved names instead of phone numbers.">
              <Switch checked={showContactNames} onCheckedChange={setShowContactNames} />
            </SettingsRow>
            <SettingsRow label="Show contact photos" description="Show contact avatars where available.">
              <Switch checked={showAvatars} onCheckedChange={setShowAvatars} />
            </SettingsRow>
            <SettingsRow label="Time format" description="How clock times are shown.">
              <select
                value={clockFormat}
                onChange={(e) => setClockFormatPref(e.target.value as ClockFormat)}
                aria-label="Time format"
                className="rounded-md border bg-transparent px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                <option value="system">System</option>
                <option value="24h">24-hour</option>
                <option value="12h">12-hour</option>
              </select>
            </SettingsRow>
          </SettingsGroup>

          {catalog && catalog.length > 0 && (
            <SettingsGroup
              title="Data to import"
              description="Choose which data types to parse. Applies to the next import or re-import."
            >
              {catalog.map((m) => (
                <SettingsRow key={m.id} label={m.label} description={m.category}>
                  <Switch
                    checked={selected.includes(m.id)}
                    onCheckedChange={(on) => toggleModule(m.id, on)}
                  />
                </SettingsRow>
              ))}
            </SettingsGroup>
          )}

          <SettingsGroup
            title="Developer"
            description="Backend logs print to the browser dev-tools console."
          >
            <SettingsRow label="Log level" description="Verbosity of import & backend logs.">
              <select
                value={logLevel}
                onChange={(e) => setLogLevel(e.target.value as LogLevel)}
                aria-label="Log level"
                className="rounded-md border bg-transparent px-2 py-1 text-sm capitalize outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {(["off", "error", "warn", "info", "debug", "trace"] as LogLevel[]).map((l) => (
                  <option key={l} value={l}>
                    {l}
                  </option>
                ))}
              </select>
            </SettingsRow>
          </SettingsGroup>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/**
 * A macOS System Settings-style group: a small header above a rounded card whose
 * rows are separated by hairline dividers.
 */
function SettingsGroup({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      <div className="px-1">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {title}
        </h3>
        {description && (
          <p className="mt-1 text-xs leading-snug text-muted-foreground">{description}</p>
        )}
      </div>
      <div className="divide-y divide-border overflow-hidden rounded-xl border bg-card">
        {children}
      </div>
    </section>
  );
}

/** One row inside a SettingsGroup: label + description on the left, control right. */
function SettingsRow({
  label,
  description,
  children,
}: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4 px-3.5 py-2.5">
      <div className="min-w-0">
        <div className="text-sm">{label}</div>
        {description && (
          <div className="truncate text-xs text-muted-foreground">{description}</div>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}
