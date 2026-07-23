/**
 * DeviceHero — the sidebar's top item: a large phone-under-the-loupe
 * illustration that shows WHAT you're looking at (the backup's device), rather
 * than the app's name. Three states:
 *
 * - backup open:   the phone with data under the loupe, device name, model +
 *                  iOS version, and an Encrypted chip. Links to /device.
 * - no backup:     a dashed "ghost" phone (the loupe stays — it's the brand
 *                  mark) with a Choose-a-backup action. Links to /.
 * - collapsed:     a compact phone+loupe mark on the icon rail.
 *
 * The illustration is plain inline SVG driven by the theme's CSS variables
 * (`--card`, `--accent-color`, `--accent-soft`), so it re-tints with the
 * light/dark theme and the system accent for free.
 */
import { useId } from "react";
import { Link, useRouterState } from "@tanstack/react-router";
import { Lock } from "lucide-react";
import { useSidebar } from "@/components/ui/sidebar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { modelName } from "@/lib/device-names";
import type { BackupInfo } from "@/lib/ipc";
import { cn } from "@/lib/utils";

export function DeviceHero({
  deviceInfo,
  hasBackup,
}: {
  deviceInfo: BackupInfo | null;
  /** undefined while the hasActiveBackup query is still pending — the hero
   *  stays neutral instead of flashing the "Choose a backup" ask. */
  hasBackup: boolean | undefined;
}) {
  const { state } = useSidebar();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const collapsed = state === "collapsed";
  const pending = hasBackup === undefined;
  const to = hasBackup === true ? "/device" : "/";
  // The hero doubles as the Device-view (or backup-picker) nav entry, so it
  // carries the active treatment those routes would otherwise lack.
  const active =
    hasBackup === true ? pathname === "/device" : pathname === "/";
  const name = hasBackup === true ? (deviceInfo?.deviceName ?? "Device") : "TraceLoupe";
  const label = hasBackup === true ? name : "Your iPhone backups";
  const meta = [
    modelName(deviceInfo?.productType ?? null),
    deviceInfo?.productVersion ? `iOS ${deviceInfo.productVersion}` : null,
  ]
    .filter(Boolean)
    .join(" · ");

  if (collapsed) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <Link
            to={to}
            aria-label={label}
            aria-current={active ? "page" : undefined}
            className={cn(
              "mx-auto flex size-9 items-center justify-center rounded-md outline-hidden ring-sidebar-ring hover:bg-sidebar-accent focus-visible:ring-2",
              active && "bg-sidebar-accent",
            )}
          >
            <PhoneLoupeMark className="size-7" />
          </Link>
        </TooltipTrigger>
        <TooltipContent side="right">{label}</TooltipContent>
      </Tooltip>
    );
  }

  return (
    <Link
      to={to}
      aria-label={label}
      aria-current={active ? "page" : undefined}
      className={cn(
        "group/hero mx-1 flex flex-col items-center rounded-xl px-2 pb-4 pt-3 text-center outline-hidden ring-sidebar-ring transition-colors hover:bg-sidebar-accent/50 focus-visible:ring-2",
        active && "bg-sidebar-accent/50",
      )}
    >
      <PhoneLoupeArt ghost={hasBackup !== true} className="size-24" />
      <div className="mt-2 w-full truncate text-[13.5px] font-semibold">
        {name}
      </div>
      {hasBackup === true ? (
        <>
          {meta && (
            <div className="mt-0.5 w-full truncate text-[11px] text-sidebar-foreground/60">
              {meta}
            </div>
          )}
          {deviceInfo?.isEncrypted === true && (
            <span className="mt-2 inline-flex items-center gap-1 rounded-full bg-[var(--accent-soft)] px-2.5 py-0.5 text-[10.5px] font-medium text-[var(--accent-text)]">
              <Lock className="size-3" />
              Encrypted
            </span>
          )}
        </>
      ) : pending ? null : (
        <>
          <div className="mt-0.5 text-[11px] text-sidebar-foreground/60">
            No backup open
          </div>
          <span className="mt-2.5 inline-flex items-center rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground transition-colors group-hover/hero:bg-primary/90">
            Choose a backup
          </span>
        </>
      )}
    </Link>
  );
}

/** The full illustration: phone (solid or dashed ghost) under the accent loupe. */
function PhoneLoupeArt({
  ghost = false,
  className,
}: {
  ghost?: boolean;
  className?: string;
}) {
  // The lens clip needs a document-unique id — this component renders once per
  // sidebar, but useId keeps it collision-free wherever else it's reused.
  const clipId = useId();
  return (
    <svg viewBox="0 0 104 104" className={cn("shrink-0", className)} aria-hidden="true">
      {ghost ? (
        <rect
          x="26"
          y="4"
          width="46"
          height="82"
          rx="10"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeDasharray="7 7"
          opacity="0.4"
        />
      ) : (
        <>
          <rect
            x="26"
            y="4"
            width="46"
            height="82"
            rx="10"
            fill="var(--card)"
            stroke="currentColor"
            strokeWidth="2.5"
          />
          {/* screen, faintly accent-washed */}
          <rect x="32" y="10" width="34" height="70" rx="5" fill="var(--accent-soft)" />
          {/* dynamic island */}
          <rect x="42" y="13" width="14" height="4" rx="2" fill="currentColor" opacity="0.65" />
          {/* data lines on the screen */}
          <g
            stroke="var(--accent-color)"
            opacity="0.5"
            strokeWidth="3"
            strokeLinecap="round"
          >
            <path d="M38 27h13M38 35h20M38 43h10" />
          </g>
        </>
      )}
      <clipPath id={clipId}>
        <circle cx="66" cy="66" r="16" />
      </clipPath>
      <circle
        cx="66"
        cy="66"
        r="19"
        fill="var(--card)"
        stroke="var(--accent-color)"
        strokeWidth="3.5"
      />
      {ghost ? (
        // Empty lens: a plus — there's nothing to magnify yet.
        <g
          stroke="currentColor"
          strokeWidth="3.5"
          strokeLinecap="round"
          opacity="0.4"
        >
          <path d="M66 58v16M58 66h16" />
        </g>
      ) : (
        // The same data lines, magnified inside the lens.
        <g
          clipPath={`url(#${clipId})`}
          stroke="var(--accent-color)"
          strokeWidth="4"
          strokeLinecap="round"
        >
          <path d="M55 60h15M55 68h22M55 76h11" />
        </g>
      )}
      <line
        x1="80"
        y1="80"
        x2="91"
        y2="91"
        stroke="var(--accent-color)"
        strokeWidth="6"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** The compact rail mark: phone outline + loupe, heavier strokes for 28px. */
function PhoneLoupeMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 104 104" className={cn("shrink-0", className)} aria-hidden="true">
      <rect
        x="26"
        y="4"
        width="46"
        height="82"
        rx="10"
        fill="var(--card)"
        stroke="currentColor"
        strokeWidth="5"
      />
      <circle
        cx="66"
        cy="66"
        r="20"
        fill="var(--card)"
        stroke="var(--accent-color)"
        strokeWidth="7"
      />
      <line
        x1="81"
        y1="81"
        x2="94"
        y2="94"
        stroke="var(--accent-color)"
        strokeWidth="9"
        strokeLinecap="round"
      />
    </svg>
  );
}
