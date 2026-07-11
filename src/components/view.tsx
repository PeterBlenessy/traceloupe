/**
 * Shared view primitives — the design base every artifact view builds on, so
 * Messages / Contacts / Calls / Safari are consistent by construction rather
 * than by copy-paste. See docs/ui.md.
 *
 * These compose shadcn primitives (Empty, Item, ScrollArea); they add layout,
 * not new styling systems. If a view needs something bespoke, prefer adding a
 * primitive here over inlining it in the view.
 */
import { Search } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

/** The header strip at the top of a view: title, optional count, actions. */
export function ViewHeader({
  title,
  count,
  children,
}: {
  title: string;
  count?: number;
  children?: React.ReactNode;
}) {
  return (
    <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
      <h1 className="text-base font-semibold">{title}</h1>
      {count !== undefined && (
        <span className="text-sm text-muted-foreground">{count}</span>
      )}
      {children && <div className="ml-auto flex items-center gap-2">{children}</div>}
    </header>
  );
}

/** A full-height view that is a single scrolling column (Calls, Safari). */
export function ListView({
  title,
  count,
  header,
  children,
}: {
  title: string;
  count?: number;
  header?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title={title} count={count}>
        {header}
      </ViewHeader>
      <ScrollArea className="flex-1">
        <div className="mx-auto max-w-3xl p-2">{children}</div>
      </ScrollArea>
    </div>
  );
}

/** Master list on the left, detail pane on the right (Messages, Contacts). */
export function ListDetail({
  master,
  detail,
}: {
  master: React.ReactNode;
  detail: React.ReactNode;
}) {
  return (
    <div className="flex h-full">
      <div className="flex w-72 shrink-0 flex-col border-r">{master}</div>
      <div className="min-w-0 flex-1">{detail}</div>
    </div>
  );
}

/** A search box for filtering a list. Controlled. */
export function ListSearch({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  return (
    <div className="relative">
      <Search className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="h-9 select-text pl-8"
      />
    </div>
  );
}

/** Standard empty / no-selection state, built on shadcn Empty. */
export function EmptyView({
  icon: Icon,
  title,
  description,
  children,
  className,
}: {
  icon?: React.ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <Empty className={cn("h-full", className)}>
      <EmptyHeader>
        {Icon && (
          <EmptyMedia variant="icon">
            <Icon className="size-6" />
          </EmptyMedia>
        )}
        <EmptyTitle>{title}</EmptyTitle>
        {description && <EmptyDescription>{description}</EmptyDescription>}
      </EmptyHeader>
      {children}
    </Empty>
  );
}

export { Search };
