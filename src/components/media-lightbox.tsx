/**
 * Shared image/video lightbox shell for Photos and Messages. Two styles (chosen
 * in Settings ▸ Media ▸ Viewer style): a windowed modal card, or a fullscreen view.
 * In both, clicking anywhere outside the media closes it, and the metadata sits
 * on a solid (opaque) bar so it's always readable — never over transparency.
 */
import { useEffect } from "react";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

export type LightboxStyle = "windowed" | "fullscreen";

export function MediaLightbox({
  open,
  onClose,
  style,
  title,
  media,
  meta,
  hasPrev = false,
  hasNext = false,
  onPrev,
  onNext,
}: {
  open: boolean;
  onClose: () => void;
  style: LightboxStyle;
  /** sr-only dialog title (accessibility). */
  title: string;
  /** The media element (<img>/<video>). */
  media: React.ReactNode;
  /** Metadata rendered on the opaque footer bar; omit for none. */
  meta?: React.ReactNode;
  hasPrev?: boolean;
  hasNext?: boolean;
  onPrev?: () => void;
  onNext?: () => void;
}) {
  // Arrow keys page prev/next (the Dialog already handles Escape + overlay click).
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") onPrev?.();
      else if (e.key === "ArrowRight") onNext?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onPrev, onNext]);

  const fullscreen = style === "fullscreen";

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        showCloseButton={false}
        className={cn(
          "flex flex-col gap-0 border-none p-0 text-neutral-100 shadow-none",
          fullscreen
            ? "h-screen w-screen max-w-none rounded-none bg-neutral-950 sm:max-w-none"
            : "h-[85vh] max-w-3xl overflow-hidden rounded-xl border border-white/10 bg-neutral-900 sm:max-w-3xl",
        )}
      >
        <DialogTitle className="sr-only">{title}</DialogTitle>
        <button
          onClick={onClose}
          aria-label="Close"
          className="absolute right-2 top-2 z-10 rounded-full bg-black/60 p-2 text-white hover:bg-black/80"
        >
          <X className="size-5" />
        </button>
        {/* The media is a DIRECT child so its `max-h-full` resolves against this
            flex area (an intermediate wrapper breaks the chain and lets a tall/
            portrait image overflow onto the metadata bar). Clicking the surround
            (this element itself, not the media or a control) closes; overflow is
            hidden as a belt-and-suspenders against any overflow. */}
        <div
          className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden"
          onClick={(e) => {
            if (e.target === e.currentTarget) onClose();
          }}
        >
          {hasPrev && (
            <button
              onClick={onPrev}
              aria-label="Previous"
              className="absolute left-2 z-10 rounded-full bg-black/60 p-2 text-white hover:bg-black/80"
            >
              <ChevronLeft className="size-6" />
            </button>
          )}
          {media}
          {hasNext && (
            <button
              onClick={onNext}
              aria-label="Next"
              className="absolute right-2 z-10 rounded-full bg-black/60 p-2 text-white hover:bg-black/80"
            >
              <ChevronRight className="size-6" />
            </button>
          )}
        </div>
        {meta && (
          // Opaque bar so metadata is always legible (never over transparency).
          <div className="shrink-0 border-t border-white/10 bg-neutral-950 px-3 py-2 text-xs text-neutral-200">
            {meta}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
