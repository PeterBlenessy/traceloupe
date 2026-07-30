/**
 * A contact's photo, with the initials fallback and a hover card that jumps to
 * Contacts.
 *
 * Shared rather than per-view: Calls resolved contacts and showed their *names*
 * but never their photos, so the same person was recognisable in Messages and
 * anonymous in Calls (#223). Copying the component would have been a second copy
 * of a rule — and a second thing to forget when one changes.
 */
import type React from "react";
import { useNavigate } from "@tanstack/react-router";

import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { initials } from "@/lib/contact";
import { cn } from "@/lib/utils";
import { client } from "@/lib/ipc";
import type { ResolvedContact } from "@/lib/use-contact-resolver";

export function ContactAvatar({
  resolved,
  name,
  handle,
  className,
}: {
  resolved: ResolvedContact | null;
  name: string;
  handle?: string | null;
  className?: string;
}) {
  const navigate = useNavigate();
  const contactId = resolved?.id ?? null;
  const open = (e: React.SyntheticEvent) => {
    if (contactId == null) return;
    e.stopPropagation();
    e.preventDefault();
    void navigate({ to: "/contacts", search: { id: contactId } });
  };
  return (
    <HoverCard openDelay={250} closeDelay={100}>
      <HoverCardTrigger asChild>
        <span
          role={contactId != null ? "button" : undefined}
          tabIndex={contactId != null ? 0 : undefined}
          // aria-label, not title: this span already has a HoverCard, and a
          // native tooltip fighting it meant two hover affordances on one
          // element — one of them in the browser's styling. The label still
          // names the control for screen readers.
          aria-label={contactId != null ? `Open ${name} in Contacts` : undefined}
          onClick={open}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") open(e);
          }}
          className={cn(
            "shrink-0 rounded-full outline-none",
            contactId != null &&
              "cursor-pointer focus-visible:ring-2 focus-visible:ring-ring",
          )}
        >
          <Avatar className={className}>
            {resolved?.hasImage && (
              <AvatarImage src={client.contactAvatarUrl(resolved.id)} alt="" />
            )}
            <AvatarFallback>{initials(name)}</AvatarFallback>
          </Avatar>
        </span>
      </HoverCardTrigger>
      <HoverCardContent
        side="right"
        align="start"
        className="w-60"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-3">
          <Avatar className="size-10 shrink-0">
            {resolved?.hasImage && (
              <AvatarImage src={client.contactAvatarUrl(resolved.id)} alt="" />
            )}
            <AvatarFallback>{initials(name)}</AvatarFallback>
          </Avatar>
          <div className="min-w-0">
            <div className="truncate text-sm font-medium">{name}</div>
            {handle && handle !== name && (
              <div className="truncate text-xs text-muted-foreground">
                {handle}
              </div>
            )}
            <div
              className={cn(
                "mt-0.5 text-2xs",
                contactId != null ? "text-primary" : "text-muted-foreground",
              )}
            >
              {contactId != null ? "Open in Contacts →" : "Not in contacts"}
            </div>
          </div>
        </div>
      </HoverCardContent>
    </HoverCard>
  );
}
