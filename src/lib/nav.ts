/**
 * The app's navigation — the ONE list of content destinations.
 *
 * Shared rather than duplicated: the home dashboard orders and labels its tiles
 * from this, after six of fourteen tiles drifted to their own names and icons
 * (#163). A rename here renames the tile.
 */
import {
  Boxes,
  CalendarDays,
  Clock,
  Globe,
  HeartPulse,
  Image,
  ListTodo,
  MessageSquare,
  Mic,
  NotebookText,
  Phone,
  ShieldAlert,
  ShieldUser,
  Table2,
  Users,
  type LucideIcon,
} from "lucide-react";

export type NavItem = {
  to: string;
  label: string;
  icon: LucideIcon;
  module?: string;
};

/** The destination for artifacts that fit nowhere else.
 *
 *  NOT in `nav`, deliberately. Artifacts fold into the view closest in meaning —
 *  permissions into Apps — and a sidebar entry leading to a screen with nothing
 *  in it is worse than no entry. The shell appends this only when some module
 *  actually declares `surface = "standalone"`, which is a claim that has to be
 *  argued for rather than a default.
 */
export const standaloneArtifactsNav: NavItem = {
  to: "/artifacts",
  label: "Artifacts",
  icon: Table2,
};

export const nav: readonly NavItem[] = [
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
  { to: "/apps", label: "Apps", icon: Boxes },

] as const;

/** The scans, which the sidebar groups above the content views. */
export const scanNav: readonly NavItem[] = [
  { to: "/security", label: "Security", icon: ShieldAlert },
  { to: "/safety-scan", label: "Safety", icon: ShieldUser },
  // Sits with the scans rather than the content views: it is not "a kind of
  // data" but a way of reading all of them at once, which is the same shape as
  // a scan.
  { to: "/timeline", label: "Timeline", icon: Clock },
] as const;

/** Nav entry for a route, content or scan — how the dashboard borrows a
 *  destination's real name and icon instead of inventing its own. */
export function navFor(route: string): NavItem | undefined {
  return [...nav, ...scanNav].find((n) => n.to === route);
}

/** Where a route sits in the sidebar; unknown routes sort last so a module the
 *  nav has never heard of still appears rather than vanishing. */
export function navOrder(route: string): number {
  const i = nav.findIndex((n) => n.to === route);
  return i === -1 ? Number.MAX_SAFE_INTEGER : i;
}
