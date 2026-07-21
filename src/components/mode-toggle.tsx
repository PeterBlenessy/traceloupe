import { Moon, Sun, SunMoon } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useTheme, type Theme } from "@/components/theme-provider";

// Cycle order and per-theme presentation. Clicking advances to the next theme.
const ORDER: Theme[] = ["system", "light", "dark"];
const META: Record<Theme, { icon: typeof Sun; label: string }> = {
  system: { icon: SunMoon, label: "System" },
  light: { icon: Sun, label: "Light" },
  dark: { icon: Moon, label: "Dark" },
};

/** A single button that cycles System → Light → Dark on click (no menu). */
export function ModeToggle() {
  const { theme, setTheme } = useTheme();
  const Icon = META[theme].icon;
  const next = ORDER[(ORDER.indexOf(theme) + 1) % ORDER.length];

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          onClick={() => setTheme(next)}
        >
          <Icon className="size-4" />
          <span className="sr-only">
            Theme: {META[theme].label}. Switch to {META[next].label}.
          </span>
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        Theme: {META[theme].label} — click for {META[next].label}
      </TooltipContent>
    </Tooltip>
  );
}
