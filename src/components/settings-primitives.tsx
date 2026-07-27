import { cloneElement, isValidElement, useId } from "react";
/**
 * The Settings dialog's shared building blocks.
 *
 * Extracted from app-shell so panels living in their own files can use them
 * (#93). While these were private to app-shell, Safety and Security physically
 * could not reach them and hand-rolled their own rows — which is why those two
 * tabs drifted to different gaps, and why a spacing bug in `SettingsRow` showed
 * in four tabs but not those two.
 *
 * Standard components where they fit; bespoke markup only where a panel genuinely
 * is not a label-and-control row (a model download card, a feed provenance line).
 */
import type React from "react";

/**
 * A macOS System Settings-style group: a small header above a rounded card whose
 * rows are separated by hairline dividers.
 */
export function SettingsGroup({
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

export function SettingsRow({
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
  // Name the control from the row's own label. A settings row is a label and a
  // control side by side, but nothing connected them — so every Switch in
  // Settings announced itself as an unnamed button to a screen reader. Doing it
  // here means the next row added is named without anyone remembering to.
  const labelId = useId();
  const child = isValidElement(children)
    ? (children as React.ReactElement<Record<string, unknown>>)
    : null;
  const named =
    child && !child.props["aria-label"] && !child.props["aria-labelledby"]
      ? cloneElement(child, { "aria-labelledby": labelId })
      : children;

  return (
    <div className="px-3 py-1.5">
      <div className="flex min-h-[calc(1.75rem*var(--text-scale))] items-center gap-4">
        <div id={labelId} className="min-w-0 flex-1 text-sm">
          {label}
        </div>
        <div className="shrink-0">{named}</div>
      </div>
      {description && (
        <div className="mt-0.5 text-xs leading-snug text-muted-foreground">
          {description}
        </div>
      )}
    </div>
  );
}
