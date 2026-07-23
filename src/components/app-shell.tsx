import { useEffect, useRef } from "react";
import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { cn } from "@/lib/utils";
import {
  Boxes,
  ShieldAlert,
  ShieldUser,
  X,
  CalendarDays,
  HeartPulse,
  ListTodo,
  Waypoints,
  Globe,
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
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { DeviceHero } from "@/components/device-hero";
import {
  SettingsDialogProvider,
  useSettingsDialog,
  type SettingsTab,
} from "@/components/settings-dialog-context";
import { useSystemAccent } from "@/lib/use-system-accent";
import { useResizableWidth } from "@/components/resize";
import { usePersistedState } from "@/lib/use-persisted-state";
import { ModeToggle } from "@/components/mode-toggle";
import { ToolbarGroup } from "@/components/toolbar-group";
import { AdaptiveToolbar } from "@/components/adaptive-toolbar";
import { ToolbarProvider, useToolbar } from "@/components/toolbar-context";
import { FilterControl } from "@/components/filter-control";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useQuery } from "@tanstack/react-query";
import {
  useSettings,
  DENSITIES,
  type Density,
  type LinkPreviewMode,
} from "@/components/settings-provider";
import { useTheme, type Theme } from "@/components/theme-provider";
import { ImportProvider, useImport } from "@/components/import-provider";
import {
  SafetyScanProvider,
  useSafetyScan,
} from "@/components/safety-scan-provider";
import { SafetyModelSettings } from "@/components/safety-model-settings";
import { SecuritySettings } from "@/components/security-settings";
import { ReimportProvider, useReimport } from "@/components/reimport-provider";
import { client, type LogLevel } from "@/lib/ipc";
import { formatCount, type ClockFormat } from "@/lib/format";

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
] as const;

export function AppShell() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  // Follow the macOS accent color (System Settings → Appearance).
  useSystemAccent();
  // Scrollbar thumbs paint only while their element scrolls (index.css keys off
  // `data-scrolling`); the 12px gutter is always reserved, so nothing shifts.
  useEffect(() => {
    const timers = new WeakMap<Element, number>();
    const onScroll = (e: Event) => {
      const el =
        e.target === document ? document.documentElement : (e.target as Element);
      if (!(el instanceof Element)) return;
      el.setAttribute("data-scrolling", "");
      const prev = timers.get(el);
      if (prev !== undefined) window.clearTimeout(prev);
      timers.set(
        el,
        window.setTimeout(() => el.removeAttribute("data-scrolling"), 800),
      );
    };
    document.addEventListener("scroll", onScroll, { capture: true, passive: true });
    return () =>
      document.removeEventListener("scroll", onScroll, { capture: true });
  }, []);
  // Drag-resizable, persisted sidebar width (applies only when expanded; the
  // icon rail uses the fixed --sidebar-width-icon).
  const { width: sidebarWidth, startResize } = useResizableWidth(
    "traceloupe-sidebar-width",
    256,
    180,
    400,
  );
  // Persist whether the sidebar is expanded or collapsed to the icon rail.
  const [sidebarOpen, setSidebarOpen] = usePersistedState(
    "traceloupe-sidebar-open",
    true,
  );
  // The open backup's device, shown as the sidebar header (which opens /device).
  const { data: deviceInfo } = useQuery({
    queryKey: ["deviceInfo"],
    queryFn: () => client.deviceInfo(),
  });
  // With no backup open there is no Device view to show — the header must lead
  // back to the backup picker instead.
  const { data: hasBackup } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });

  return (
    // ImportProvider / ReimportProvider live above the routes so an import — and a
    // single-module re-import's spinner — survive "run in background" and
    // navigation between views.
    <ImportProvider>
      <ReimportProvider>
       <SafetyScanProvider>
       <SettingsDialogProvider>
       <ToolbarProvider>
        {/* h-svh pins the app to a FIXED viewport height. shadcn's SidebarProvider
        only sets `min-h-svh`, which lets the layout grow with its content — so a
        virtualized list's tall spacer would inflate the whole document and its
        scroll container would never actually scroll (it just grows), defeating
        every `min-h-0`/`overflow-auto` below. A fixed height gives the flex chain
        something to constrain against so overflow scrolls instead of expanding.
        `relative` anchors the sidebar resize handle. */}
        <SidebarProvider
          open={sidebarOpen}
          onOpenChange={setSidebarOpen}
          className="relative h-svh overflow-hidden"
          style={
            { "--sidebar-width": `${sidebarWidth}px` } as React.CSSProperties
          }
        >
          <AppTitleBar />
          {/* collapsible="icon": the trigger collapses the sidebar to an icon rail
          rather than sliding it off-canvas. */}
          <Sidebar collapsible="icon">
            {/* Clear the top chrome: when expanded the sidebar runs full height and
            its header just clears the macOS traffic lights (pt-10); when collapsed
            it sits UNDER the full-width title bar (h-13), so the icon clears the
            bar (pt-16). data-tauri-drag-region makes the band draggable. */}
            <SidebarHeader
              className="relative pt-10 group-data-[collapsible=icon]:pt-16"
              data-tauri-drag-region
            >
              {/* Native-macOS trigger placement: it lives IN the sidebar while
                  the sidebar shows (top-right, beside the traffic lights) and
                  moves out into the title bar when collapsed — so it never
                  reads as belonging to the content view's title. */}
              <div className="absolute right-2 top-2 group-data-[collapsible=icon]:hidden">
                <SidebarTrigger />
              </div>
              {/* The device identity as a hero: what backup you're looking at,
                  not the app's name. Doubles as the Device-info entry. */}
              <DeviceHero deviceInfo={deviceInfo ?? null} hasBackup={hasBackup} />
            </SidebarHeader>
            <SidebarContent>
              {/* Security and Safety get their own group: both operate on the
                  WHOLE backup (a spyware audit, a content scan), unlike the
                  content views below which are slices of its content. */}
              <SidebarGroup>
                <SidebarGroupLabel>Scans</SidebarGroupLabel>
                <SidebarGroupContent>
                  <SidebarMenu>
                    <SidebarMenuItem>
                      <SidebarMenuButton
                        asChild
                        isActive={pathname === "/security"}
                        tooltip="Security"
                      >
                        <Link to="/security">
                          <ShieldAlert />
                          <span>Security</span>
                        </Link>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                    <SidebarMenuItem>
                      <SidebarMenuButton
                        asChild
                        isActive={pathname === "/safety-scan"}
                        tooltip="Safety (experimental)"
                      >
                        <Link to="/safety-scan">
                          <ShieldUser />
                          <span>Safety</span>
                        </Link>
                      </SidebarMenuButton>
                      {/* Experimental: local-AI classification quality is not yet
                          validated on real hardware. */}
                      <SidebarMenuBadge className="text-[9px] font-medium uppercase tracking-wide text-muted-foreground">
                        Beta
                      </SidebarMenuBadge>
                    </SidebarMenuItem>
                  </SidebarMenu>
                </SidebarGroupContent>
              </SidebarGroup>
              <SidebarGroup>
                <SidebarGroupLabel>Content</SidebarGroupLabel>
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
            <SidebarFooter>
              <SidebarMenu>
                <SidebarMenuItem>
                  <SettingsMenu />
                </SidebarMenuItem>
              </SidebarMenu>
            </SidebarFooter>
          </Sidebar>
          <SidebarResizeEdge onPointerDown={(e) => startResize(e, "right")} />
          <SidebarInset>
            {/* The bar clearance lives as PADDING on this clipping wrapper (not
                on SidebarInset) so its padding-box reaches the window top:
                overflow clips at the padding box, which lets an opted-in list
                (data-underlap) rise under the translucent bar while every other
                view keeps starting below it. */}
            <div className="min-h-0 flex-1 overflow-hidden pt-13">
              <Outlet />
            </div>
          </SidebarInset>
        </SidebarProvider>
       </ToolbarProvider>
       </SettingsDialogProvider>
       </SafetyScanProvider>
      </ReimportProvider>
    </ImportProvider>
  );
}

/** The single unified toolbar: the current view's title + islands (published via
 *  the toolbar context) on the left, the app-wide controls + search on the right. */
/**
 * The unified HTML title bar. When the sidebar is **collapsed** it spans the full
 * window width (`left-0`) above the icon rail, with the macOS traffic lights in
 * its left; when **expanded** the sidebar runs the full window height and the
 * title bar only covers the content area to its right (`left: --sidebar-width`),
 * so the sidebar's border/top isn't covered. The whole strip drags the window.
 */
function AppTitleBar() {
  const { state } = useSidebar();
  const { translucentToolbar } = useSettings();
  const collapsed = state === "collapsed";
  // Keyboard focus survives the trigger swap: the in-sidebar trigger is
  // display:none'd on collapse and the title-bar one unmounts on expand, so
  // the just-activated control vanishes and focus falls to <body>, restarting
  // tab order from the top. When that happens, hand focus to the visible
  // counterpart. Skipped on mount (focus starts on <body> without any swap).
  const mounted = useRef(false);
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    if (document.activeElement !== document.body) return;
    const id = requestAnimationFrame(() => {
      const triggers = document.querySelectorAll<HTMLElement>(
        '[data-sidebar="trigger"]',
      );
      for (const t of triggers) {
        if (t.offsetParent !== null) {
          t.focus();
          break;
        }
      }
    });
    return () => cancelAnimationFrame(id);
  }, [collapsed]);
  return (
    <header
      data-tauri-drag-region
      data-slot="app-titlebar"
      // Match the sidebar's own width transition so the two edges move together.
      style={{ left: collapsed ? 0 : "var(--sidebar-width)" }}
      // `absolute` (against the SidebarProvider root), NOT `fixed`: WKWebView
      // fails to sample async-scrolled content into a fixed element's
      // backdrop-filter, so the translucent bar read as opaque in the app
      // while working in Chrome. NoteSage's frosted title bar is absolute for
      // the same reason. The page never scrolls at the root, so the geometry
      // is identical. The frosted classes live HERE (not a CSS override of
      // bg-background) so there is exactly one element owning the bar's
      // background and no cascade fight for it.
      className={cn(
        "absolute right-0 top-0 z-20 flex h-13 items-center border-b px-3 transition-[left] duration-200 ease-linear",
        translucentToolbar
          ? "bg-background/65 backdrop-blur-2xl backdrop-saturate-150"
          : "bg-background",
      )}
    >
      <AppToolbar collapsed={collapsed} />
    </header>
  );
}

function AppToolbar({ collapsed }: { collapsed: boolean }) {
  const tb = useToolbar();
  return (
    <AdaptiveToolbar
      leading={
        // When collapsed the bar starts at the window's left edge, so pad past the
        // traffic lights; when expanded the lights sit over the sidebar (left of
        // this bar), so no extra padding is needed. The toggle is its own island.
        <div className={cn("flex items-center gap-2", collapsed && "pl-20")}>
          {/* The trigger only joins the title bar when the sidebar is hidden —
              while it's visible, the trigger sits inside the sidebar itself. */}
          {collapsed && (
            <div className="flex items-center rounded-lg border border-border/70 bg-muted/40 p-0.5">
              <SidebarTrigger />
            </div>
          )}
          {tb?.title && (
            <div className="flex items-baseline gap-2">
              <h1 className="text-base font-semibold">{tb.title}</h1>
              {tb.count !== undefined && (
                <span className="text-xs tabular-nums text-muted-foreground">
                  {formatCount(tb.count)}
                </span>
              )}
            </div>
          )}
        </div>
      }
      middle={
        // A view's right-aligned controls: view-mode toggle, the Filter panel
        // (when it has facets), sort, and search. Views with none (e.g. Device)
        // publish nothing and get just the title + app controls.
        <>
          {tb?.modes}
          {tb?.filter && tb.filter.length > 0 && <FilterControl groups={tb.filter} />}
          {tb?.sort}
          {tb?.search}
        </>
      }
      trailing={
        // App-wide controls, rightmost.
        <>
          <SafetyScanIndicator />
          <ModelDownloadIndicator />
          <ImportIndicator />
          <ToolbarGroup>
            <DensityToggle />
            <ModeToggle />
          </ToolbarGroup>
        </>
      }
    />
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
    <Tooltip>
      <TooltipTrigger asChild>
        <SidebarMenuAction
          showOnHover={!running}
          disabled={running}
          onClick={() => reimport(module)}
          aria-label={running ? `Re-importing ${label}` : `Re-import ${label}`}
        >
          {running ? <Loader2 className="animate-spin" /> : <RefreshCw />}
        </SidebarMenuAction>
      </TooltipTrigger>
      <TooltipContent side="right">
        {running ? `Re-importing ${label}…` : `Re-import ${label}`}
      </TooltipContent>
    </Tooltip>
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
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          onClick={reopen}
          className="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent"
        >
          <Loader2 className="size-3 animate-spin" />
          <span className="max-w-[16rem] truncate">
            Importing {active.backup.deviceName ?? active.backup.id} · {detail}
          </span>
        </button>
      </TooltipTrigger>
      <TooltipContent>Reopen import</TooltipContent>
    </Tooltip>
  );
}

/** A pill shown while the Safety Scan model downloads in the background — so
 *  the ~5 GB download is visible and cancelable from anywhere, not only inside
 *  the Settings dialog (which is modal). The download itself already runs in
 *  the SafetyScanProvider, above the routes, so it keeps going as you navigate
 *  or close Settings; this just surfaces it. */
function ModelDownloadIndicator() {
  const { download, cancelDownload } = useSafetyScan();
  if (!download) return null;
  const pct =
    download.phase === "downloading" && download.total > 0
      ? Math.round((download.received / download.total) * 100)
      : null;
  const label =
    download.phase === "verifying"
      ? "Verifying model…"
      : pct !== null
        ? `Downloading model · ${pct}%`
        : "Downloading model…";
  return (
    <span className="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground">
      <Loader2 className="size-3 animate-spin" />
      <span className="max-w-[14rem] truncate">{label}</span>
      {download.phase === "downloading" && (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              onClick={cancelDownload}
              aria-label="Cancel model download"
              className="ml-0.5 rounded-full p-0.5 hover:bg-accent hover:text-foreground"
            >
              <X className="size-3" />
            </button>
          </TooltipTrigger>
          <TooltipContent>Cancel model download</TooltipContent>
        </Tooltip>
      )}
    </span>
  );
}

/** A Safety Scan runs in the SafetyScanProvider (above the routes), so it keeps
 *  going after you leave the Safety view. This pill surfaces it in the title bar
 *  — spinner + progress — and jumps back to the view on click. Hidden while the
 *  Safety view is open (it's redundant there). */
function SafetyScanIndicator() {
  const { scan } = useSafetyScan();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  if (!scan || pathname === "/safety-scan") return null;
  const label =
    scan.phase === "loading"
      ? "Loading model…"
      : scan.phase === "summarizing"
        ? "Writing report…"
        : scan.phase === "classifying" && scan.total > 0
          ? `Scanning · ${scan.done}/${scan.total}`
          : "Scanning…";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Link
          to="/safety-scan"
          className="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <Loader2 className="size-3 animate-spin" />
          <span className="max-w-[14rem] truncate">{label}</span>
        </Link>
      </TooltipTrigger>
      <TooltipContent>Go to Safety Scan</TooltipContent>
    </Tooltip>
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
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="size-8"
          onClick={() => setDensity(next)}
        >
          <Icon className="size-5" />
          <span className="sr-only">
            Density: {label}. Switch to {DENSITY_META[next].label}.
          </span>
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        Density: {label} — click for {DENSITY_META[next].label}
      </TooltipContent>
    </Tooltip>
  );
}

/** Gear button + dialog exposing the app-wide display preferences. */
function SettingsMenu() {
  const {
    showContactNames,
    setShowContactNames,
    showAvatars,
    setShowAvatars,
    linkPreviewMode,
    setLinkPreviewMode,
    lightboxStyle,
    setLightboxStyle,
    showMediaMetadata,
    setShowMediaMetadata,
    recoverFromPhotos,
    setRecoverFromPhotos,
    fetchAppIcons,
    setFetchAppIcons,
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
    translucentToolbar,
    setTranslucentToolbar,
  } = useSettings();
  const { theme, setTheme } = useTheme();
  // Lifted open/tab state so views can deep-link (e.g. "Settings → Safety").
  const { open, setOpen, tab, setTab } = useSettingsDialog();
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
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <SidebarMenuButton tooltip="Settings">
          <Settings />
          <span>Settings</span>
        </SidebarMenuButton>
      </DialogTrigger>
      <DialogContent className="flex h-[75vh] gap-0 overflow-hidden rounded-2xl p-0 sm:max-w-3xl">
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <DialogDescription className="sr-only">
          Display, apps to import, and developer preferences.
        </DialogDescription>
        {/* macOS System Settings-style two-pane layout: a full-height sidebar
            (its own background, bleeding to the dialog's rounded edges) beside a
            scrolling content pane. `contents` dissolves the Tabs wrapper so its
            children become the dialog's flex items directly. */}
        <Tabs
          value={tab}
          onValueChange={(v) => setTab(v as SettingsTab)}
          orientation="vertical"
          className="contents"
        >
          {/* The dialog's nav pane mirrors the app sidebar: same surface token,
              same row metrics (h-9, 20px icons), same accent-soft active pill. */}
          <TabsList
            variant="line"
            className="!h-full w-48 shrink-0 flex-col items-stretch justify-start gap-0.5 border-r bg-sidebar !rounded-none !p-3"
          >
            <div className="mb-1.5 px-2 text-[10.5px] font-medium uppercase tracking-wider text-sidebar-foreground/60">
              TraceLoupe
            </div>
            {(
              [
                ["general", "General", SlidersHorizontal],
                ["media", "Media", Image],
                ["apps", "Apps", Boxes],
                ["security", "Security", ShieldAlert],
                ["safety", "Safety", ShieldUser],
                ["developer", "Developer", Terminal],
              ] as const
            ).map(([value, label, Icon]) => (
              <TabsTrigger
                key={value}
                value={value}
                // Sidebar row: icon + label, accent-soft pill when active — the
                // same treatment as the app sidebar's active nav item.
                // `flex-none h-9` stops the trigger's base `flex-1` from stretching
                // rows to fill the tall sidebar; `[&::after]:hidden` drops the line
                // variant's edge bar.
                className="h-9 flex-none justify-start gap-2.5 rounded-md px-2 text-sm hover:bg-sidebar-accent [&::after]:hidden data-[state=active]:!bg-[var(--accent-soft)] data-[state=active]:!text-[var(--accent-text)] data-[state=active]:font-medium"
              >
                <Icon className="size-5 shrink-0" />
                <span className="flex-1 truncate text-left">{label}</span>
              </TabsTrigger>
            ))}
          </TabsList>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto px-8 pt-8 pb-6">
          <TabsContent
            value="general"
            className="mt-0 flex flex-col gap-6"
          >
            <SettingsGroup
              title="Display"
              description="Appearance, contact display, and how lists and links are shown."
            >
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
                description="Use each contact's saved photo as their avatar in lists and chats."
              >
                <Switch
                  aria-label="Show contact photos"
                  checked={showAvatars}
                  onCheckedChange={setShowAvatars}
                />
              </SettingsRow>
              <SettingsRow
                label="Link previews"
                description="Off keeps raw URLs and never touches the network. On hover fetches a preview only when you point at a link; Inline unfurls every link in the bubble — both reach out to the linked sites."
              >
                <select
                  value={linkPreviewMode}
                  onChange={(e) =>
                    setLinkPreviewMode(e.target.value as LinkPreviewMode)
                  }
                  aria-label="Link previews"
                  className="inline-flex h-8 items-center rounded-md border bg-transparent px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <option value="off">Off</option>
                  <option value="hover">On hover</option>
                  <option value="inline">Inline</option>
                </select>
              </SettingsRow>
              <SettingsRow
                label="Time format"
                description="12-hour, 24-hour, or match your system."
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
                description="Comfortable, cozy, or compact row and control spacing."
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
              <SettingsRow
                label="Translucent toolbar"
                description="Make the toolbar slightly see-through, with long lists scrolling visibly beneath it."
              >
                <Switch
                  aria-label="Translucent toolbar"
                  checked={translucentToolbar}
                  onCheckedChange={setTranslucentToolbar}
                />
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
                    : "Unavailable on an unsigned build — sign the app (docs/reference/signing.md) to use Touch ID."
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
            value="media"
            className="mt-0 flex flex-col gap-6"
          >
            <SettingsGroup
              title="Photo & video viewer"
              description="How images and videos open from Photos and Messages."
            >
              <SettingsRow
                label="Viewer style"
                description="Open media in a windowed panel, or fill the screen."
              >
                <select
                  value={lightboxStyle}
                  onChange={(e) =>
                    setLightboxStyle(e.target.value as "windowed" | "fullscreen")
                  }
                  aria-label="Viewer style"
                  className="inline-flex h-8 items-center rounded-md border bg-transparent px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <option value="fullscreen">Fullscreen</option>
                  <option value="windowed">Windowed</option>
                </select>
              </SettingsRow>
              <SettingsRow
                label="Show media details"
                description="Show file, date, EXIF and location metadata in the viewer."
              >
                <Switch
                  aria-label="Show media details"
                  checked={showMediaMetadata}
                  onCheckedChange={setShowMediaMetadata}
                />
              </SettingsRow>
              <SettingsRow
                label="Recover attachments from Photos"
                description="When a message photo or video is missing, show the same-named camera-roll item instead. Best-effort name matching — it can occasionally show the wrong photo, so recovered media is labelled."
              >
                <Switch
                  aria-label="Recover attachments from Photos"
                  checked={recoverFromPhotos}
                  onCheckedChange={setRecoverFromPhotos}
                />
              </SettingsRow>
            </SettingsGroup>
          </TabsContent>

          <TabsContent
            value="apps"
            className="mt-0 flex flex-col gap-6"
          >
            <SettingsGroup
              title="App details"
              description="How the Apps view shows each installed app."
            >
              <SettingsRow
                label="Fetch real app icons"
                description="Look up each app's real icon from Apple's App Store. The only feature that leaves your Mac — it tells Apple which apps the backup contains — so it's off by default; otherwise apps show a colored initial tile."
              >
                <Switch
                  aria-label="Fetch real app icons"
                  checked={fetchAppIcons}
                  onCheckedChange={setFetchAppIcons}
                />
              </SettingsRow>
            </SettingsGroup>
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

          <TabsContent value="security" className="mt-0 flex flex-col gap-6">
            <SettingsGroup
              title="Security Check"
              description="How TraceLoupe checks your backups against public spyware and stalkerware lists."
            >
              <div className="p-3">
                <SecuritySettings />
              </div>
            </SettingsGroup>
          </TabsContent>

          <TabsContent value="safety" className="mt-0 flex flex-col gap-6">
            <SettingsGroup
              title="Safety Scan model"
              description="The local AI model that powers Safety Scan's on-device content analysis."
            >
              <div className="p-3">
                <SafetyModelSettings />
              </div>
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
  // Stacked layout (macOS System Settings pattern): the label and control sit
  // together on the first row; the description flows full-width beneath them. A
  // side-by-side layout squeezes the description into whatever width the control
  // leaves, wrapping long help text one word per line.
  return (
    <div className="px-3.5 py-2.5">
      <div className="flex min-h-7 items-center gap-4">
        <div className="min-w-0 flex-1 text-sm">{label}</div>
        <div className="shrink-0">{children}</div>
      </div>
      {description && (
        <div className="mt-1 text-xs leading-relaxed text-muted-foreground">
          {description}
        </div>
      )}
    </div>
  );
}
