import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import {
  Boxes,
  CalendarDays,
  HeartPulse,
  ListTodo,
  Smartphone,
  Waypoints,
  Globe,
  HardDrive,
  Image,
  Loader2,
  MessageSquare,
  Mic,
  NotebookText,
  Phone,
  RefreshCw,
  Rows2,
  Rows3,
  Rows4,
  Settings,
  SlidersHorizontal,
  Terminal,
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
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { useResizableWidth } from "@/components/resize";
import { ModeToggle } from "@/components/mode-toggle";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useQuery } from "@tanstack/react-query";
import {
  useSettings,
  DENSITIES,
  type Density,
} from "@/components/settings-provider";
import { useTheme, type Theme } from "@/components/theme-provider";
import { ImportProvider, useImport } from "@/components/import-provider";
import { ReimportProvider, useReimport } from "@/components/reimport-provider";
import { client, type LogLevel } from "@/lib/ipc";
import { type ClockFormat } from "@/lib/format";

const nav = [
  { to: "/photos", label: "Photos", icon: Image, module: "camera_roll" },
  {
    to: "/messages",
    label: "Messages",
    icon: MessageSquare,
    module: "messages",
  },
  { to: "/contacts", label: "Contacts", icon: Users },
  { to: "/calls", label: "Calls", icon: Phone, module: "calls" },
  { to: "/safari", label: "Safari", icon: Globe, module: "safari" },
  { to: "/notes", label: "Notes", icon: NotebookText, module: "notes" },
  { to: "/recordings", label: "Recordings", icon: Mic, module: "recordings" },
  { to: "/calendar", label: "Calendar", icon: CalendarDays },
  { to: "/reminders", label: "Reminders", icon: ListTodo },
  { to: "/health", label: "Health", icon: HeartPulse },
  { to: "/interactions", label: "Interactions", icon: Waypoints },
  { to: "/apps", label: "Apps", icon: Boxes },
  { to: "/device", label: "Device", icon: Smartphone },
] as const;

export function AppShell() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  // Drag-resizable, persisted sidebar width (applies only when expanded; the
  // icon rail uses the fixed --sidebar-width-icon).
  const { width: sidebarWidth, startResize } = useResizableWidth(
    "traceloupe-sidebar-width",
    256,
    180,
    400,
  );

  return (
    // ImportProvider / ReimportProvider live above the routes so an import — and a
    // single-module re-import's spinner — survive "run in background" and
    // navigation between views.
    <ImportProvider>
      <ReimportProvider>
        {/* h-svh pins the app to a FIXED viewport height. shadcn's SidebarProvider
        only sets `min-h-svh`, which lets the layout grow with its content — so a
        virtualized list's tall spacer would inflate the whole document and its
        scroll container would never actually scroll (it just grows), defeating
        every `min-h-0`/`overflow-auto` below. A fixed height gives the flex chain
        something to constrain against so overflow scrolls instead of expanding.
        `relative` anchors the sidebar resize handle. */}
        <SidebarProvider
          className="relative h-svh overflow-hidden"
          style={
            { "--sidebar-width": `${sidebarWidth}px` } as React.CSSProperties
          }
        >
          {/* collapsible="icon": the trigger collapses the sidebar to an icon rail
          rather than sliding it off-canvas. */}
          <Sidebar collapsible="icon">
            <SidebarHeader>
              <Link
                to="/"
                className="flex items-center gap-2 px-2 py-1.5 font-semibold"
              >
                <HardDrive className="size-4" />
                <span className="group-data-[collapsible=icon]:hidden">
                  TraceLoupe
                </span>
              </Link>
            </SidebarHeader>
            <SidebarContent>
              <SidebarGroup>
                <SidebarGroupContent>
                  <SidebarMenu>
                    {nav.map((item) => (
                      <SidebarMenuItem key={item.to}>
                        <SidebarMenuButton
                          asChild
                          isActive={pathname === item.to}
                          tooltip={item.label}
                        >
                          <Link to={item.to}>
                            <item.icon />
                            <span>{item.label}</span>
                          </Link>
                        </SidebarMenuButton>
                        {"module" in item && (
                          <ReimportAction
                            module={item.module}
                            label={item.label}
                          />
                        )}
                      </SidebarMenuItem>
                    ))}
                  </SidebarMenu>
                </SidebarGroupContent>
              </SidebarGroup>
            </SidebarContent>
          </Sidebar>
          <SidebarResizeEdge onPointerDown={(e) => startResize(e, "right")} />
          <SidebarInset>
            {/* A slim bar carrying the sidebar toggle and theme control; views
            render their own headers below it. */}
            <div className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
              <SidebarTrigger />
              <div className="ml-auto flex items-center gap-1">
                <ImportIndicator />
                <DensityToggle />
                <ModeToggle />
                <SettingsMenu />
              </div>
            </div>
            <div className="min-h-0 flex-1 overflow-hidden">
              <Outlet />
            </div>
          </SidebarInset>
        </SidebarProvider>
      </ReimportProvider>
    </ImportProvider>
  );
}

/** A drag handle at the expanded sidebar's right edge for resizing its width.
 *  Hidden on mobile and when collapsed to the icon rail. */
function SidebarResizeEdge({
  onPointerDown,
}: {
  onPointerDown: (e: React.PointerEvent) => void;
}) {
  const { state, isMobile } = useSidebar();
  if (isMobile || state === "collapsed") return null;
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      onPointerDown={onPointerDown}
      title="Drag to resize the sidebar"
      className="absolute inset-y-0 z-20 w-1 cursor-col-resize bg-transparent transition-colors hover:bg-primary/40 active:bg-primary/60"
      style={{ left: "var(--sidebar-width)", transform: "translateX(-2px)" }}
    />
  );
}

/**
 * The per-view re-import control, living on its sidebar nav item: a spinner while
 * that module re-imports (always visible so it's legible from any view), or a
 * hover-revealed refresh button when idle. Hidden until a backup is open — there's
 * nothing to re-import into otherwise. State comes from ReimportProvider (above
 * the routes), so switching views never leaves the spinner stale.
 */
function ReimportAction({ module, label }: { module: string; label: string }) {
  const { isRunning, reimport } = useReimport();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  if (active !== true) return null;
  const running = isRunning(module);
  return (
    <SidebarMenuAction
      showOnHover={!running}
      disabled={running}
      onClick={() => reimport(module)}
      title={running ? `Re-importing ${label}…` : `Re-import ${label}`}
      aria-label={running ? `Re-importing ${label}` : `Re-import ${label}`}
    >
      {running ? <Loader2 className="animate-spin" /> : <RefreshCw />}
    </SidebarMenuAction>
  );
}

/** A pill shown while an import runs in the background; click to reopen it. */
function ImportIndicator() {
  const { active, backgrounded, reopen } = useImport();
  if (!backgrounded || !active) return null;
  const p = active.progress;
  const detail =
    p?.phase === "indexing"
      ? `${p.step}… (${p.index}/${p.total})`
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

// A "rows" glyph per level (more rows = denser), à la Airtable/Notion's row-height
// control — the recognizable idiom for density (unlike "A", which reads as text size).
const DENSITY_META: Record<
  Density,
  { icon: typeof Rows2; label: string }
> = {
  comfortable: { icon: Rows2, label: "Comfortable" },
  cozy: { icon: Rows3, label: "Cozy" },
  compact: { icon: Rows4, label: "Compact" },
};

/** A single header button that cycles list density; the icon reflects the level. */
function DensityToggle() {
  const { density, setDensity } = useSettings();
  const next = DENSITIES[(DENSITIES.indexOf(density) + 1) % DENSITIES.length];
  const { icon: Icon, label } = DENSITY_META[density];
  return (
    <Button
      variant="ghost"
      size="icon"
      className="size-7"
      onClick={() => setDensity(next)}
      title={`Density: ${label} — click for ${DENSITY_META[next].label}`}
    >
      <Icon className="size-4" />
      <span className="sr-only">
        Density: {label}. Switch to {DENSITY_META[next].label}.
      </span>
    </Button>
  );
}

/** Gear button + dialog exposing the app-wide display preferences. */
function SettingsMenu() {
  const {
    showContactNames,
    setShowContactNames,
    showAvatars,
    setShowAvatars,
    linkPreviews,
    setLinkPreviews,
    importModules,
    setImportModules,
    logLevel,
    setLogLevel,
    clockFormat,
    setClockFormatPref,
    biometricUnlock,
    setBiometricUnlock,
    biometricAvailable,
    density,
    setDensity,
  } = useSettings();
  const { theme, setTheme } = useTheme();
  const { data: catalog } = useQuery({
    queryKey: ["importModules"],
    queryFn: () => client.listImportModules(),
  });
  // Effective selection: the user's saved choice, or every default.
  const selected =
    importModules ?? catalog?.filter((m) => m.default).map((m) => m.id) ?? [];
  const toggleModule = (id: string, on: boolean) => {
    const base = selected;
    setImportModules(
      on ? [...new Set([...base, id])] : base.filter((x) => x !== id),
    );
  };

  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="Settings">
          <Settings className="size-4" />
        </Button>
      </DialogTrigger>
      <DialogContent className="flex h-[75vh] gap-0 overflow-hidden rounded-2xl p-0 sm:max-w-2xl">
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <DialogDescription className="sr-only">
          Display, apps to import, and developer preferences.
        </DialogDescription>
        {/* macOS System Settings-style two-pane layout: a full-height sidebar
            (its own background, bleeding to the dialog's rounded edges) beside a
            scrolling content pane. `contents` dissolves the Tabs wrapper so its
            children become the dialog's flex items directly. */}
        <Tabs defaultValue="general" orientation="vertical" className="contents">
          <TabsList
            variant="line"
            className="!h-full w-48 shrink-0 flex-col items-stretch justify-start gap-0.5 border-r bg-muted/30 !rounded-none !p-3"
          >
            <div className="mb-1.5 px-2 text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
              TraceLoupe
            </div>
            {(
              [
                ["general", "General", SlidersHorizontal],
                ["apps", "Apps", Boxes],
                ["developer", "Developer", Terminal],
              ] as const
            ).map(([value, label, Icon]) => (
              <TabsTrigger
                key={value}
                value={value}
                // Sidebar row: icon + label, filled accent pill when active.
                // `flex-none h-8` stops the trigger's base `flex-1` from stretching
                // rows to fill the tall sidebar; `[&::after]:hidden` drops the line
                // variant's edge bar.
                className="h-8 flex-none justify-start gap-2.5 rounded-md px-2 text-[13px] hover:bg-muted [&::after]:hidden data-[state=active]:!bg-accent data-[state=active]:!text-accent-foreground data-[state=active]:font-medium data-[state=active]:shadow-sm"
              >
                <Icon className="size-4 shrink-0" />
                <span className="flex-1 truncate text-left">{label}</span>
              </TabsTrigger>
            ))}
          </TabsList>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto px-8 pt-8 pb-6">
          <TabsContent
            value="general"
            className="mt-0 flex flex-col gap-6"
          >
            <SettingsGroup title="Display">
              <SettingsRow
                label="Appearance"
                description="Light, dark, or follow the system."
              >
                <select
                  value={theme}
                  onChange={(e) => setTheme(e.target.value as Theme)}
                  aria-label="Appearance"
                  className="inline-flex h-8 items-center rounded-md border bg-transparent px-2.5 text-sm capitalize outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <option value="system">System</option>
                  <option value="light">Light</option>
                  <option value="dark">Dark</option>
                </select>
              </SettingsRow>
              <SettingsRow
                label="Show contact names"
                description="Display saved names instead of phone numbers."
              >
                <Switch
                  aria-label="Show contact names"
                  checked={showContactNames}
                  onCheckedChange={setShowContactNames}
                />
              </SettingsRow>
              <SettingsRow
                label="Show contact photos"
                description="Show contact avatars where available."
              >
                <Switch
                  aria-label="Show contact photos"
                  checked={showAvatars}
                  onCheckedChange={setShowAvatars}
                />
              </SettingsRow>
              <SettingsRow
                label="Load link previews"
                description="Fetch a title & image for links in messages. Off by default — this contacts the linked websites."
              >
                <Switch
                  aria-label="Load link previews"
                  checked={linkPreviews}
                  onCheckedChange={setLinkPreviews}
                />
              </SettingsRow>
              <SettingsRow
                label="Time format"
                description="How clock times are shown."
              >
                <select
                  value={clockFormat}
                  onChange={(e) =>
                    setClockFormatPref(e.target.value as ClockFormat)
                  }
                  aria-label="Time format"
                  className="inline-flex h-8 items-center rounded-md border bg-transparent px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <option value="system">System</option>
                  <option value="24h">24-hour</option>
                  <option value="12h">12-hour</option>
                </select>
              </SettingsRow>
              <SettingsRow
                label="Density"
                description="How tightly lists and controls pack together."
              >
                <select
                  value={density}
                  onChange={(e) => setDensity(e.target.value as Density)}
                  aria-label="Density"
                  className="inline-flex h-8 items-center rounded-md border bg-transparent px-2.5 text-sm capitalize outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <option value="comfortable">Comfortable</option>
                  <option value="cozy">Cozy</option>
                  <option value="compact">Compact</option>
                </select>
              </SettingsRow>
            </SettingsGroup>

            <SettingsGroup
              title="Security"
              description="Encrypted backups store their password in the macOS Keychain."
            >
              <SettingsRow
                label="Require Touch ID"
                description={
                  biometricAvailable
                    ? "Ask for Touch ID before unlocking an encrypted backup's keys."
                    : "Unavailable on an unsigned build — sign the app (docs/signing.md) to use Touch ID."
                }
              >
                <Switch
                  aria-label="Require Touch ID"
                  checked={biometricUnlock}
                  disabled={!biometricAvailable}
                  onCheckedChange={setBiometricUnlock}
                />
              </SettingsRow>
            </SettingsGroup>
          </TabsContent>

          <TabsContent
            value="apps"
            className="mt-0 flex flex-col gap-6"
          >
            {catalog && catalog.length > 0 ? (
              <SettingsGroup
                title="Data to import"
                description="Choose which data types to parse. Applies to the next import or re-import."
              >
                {catalog.map((m) => (
                  <SettingsRow
                    key={m.id}
                    label={m.label}
                    description={m.category}
                  >
                    <Switch
                      aria-label={m.label}
                      checked={selected.includes(m.id)}
                      onCheckedChange={(on) => toggleModule(m.id, on)}
                    />
                  </SettingsRow>
                ))}
              </SettingsGroup>
            ) : (
              <p className="px-1 py-6 text-sm text-muted-foreground">
                No import catalog available.
              </p>
            )}
          </TabsContent>

          <TabsContent
            value="developer"
            className="mt-0 flex flex-col gap-6"
          >
            <SettingsGroup
              title="Developer"
              description="Backend logs print to the browser dev-tools console."
            >
              <SettingsRow
                label="Log level"
                description="Verbosity of import & backend logs."
              >
                <select
                  value={logLevel}
                  onChange={(e) => setLogLevel(e.target.value as LogLevel)}
                  aria-label="Log level"
                  className="inline-flex h-8 items-center rounded-md border bg-transparent px-2.5 text-sm capitalize outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  {(
                    [
                      "off",
                      "error",
                      "warn",
                      "info",
                      "debug",
                      "trace",
                    ] as LogLevel[]
                  ).map((l) => (
                    <option key={l} value={l}>
                      {l}
                    </option>
                  ))}
                </select>
              </SettingsRow>
            </SettingsGroup>
          </TabsContent>
          </div>
        </Tabs>
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
          <p className="mt-1 text-xs leading-snug text-muted-foreground">
            {description}
          </p>
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
          <div className="truncate text-xs text-muted-foreground">
            {description}
          </div>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}
