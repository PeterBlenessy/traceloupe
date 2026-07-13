import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Building2, Mail, MessageSquare, Phone, Users } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { EmptyView, ErrorState, ListDetail, ListSearch, ViewHeader } from "@/components/view";
import { contactName, initials } from "@/lib/contact";
import { phoneOrEmailKey } from "@/lib/use-contact-resolver";
import { cn } from "@/lib/utils";
import { client, type Contact } from "@/lib/ipc";

export function ContactsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: contacts, isPending, error } = useQuery({
    queryKey: ["contacts"],
    queryFn: () => client.listContacts(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [q, setQ] = useState("");
  // Source filter: the device address book vs a third-party app's social graph
  // (e.g. TikTok). Default to the address book so app contacts don't bury it.
  const [source, setSource] = useState("Address Book");
  const [sort, setSort] = useState<SortState>({ by: "name", desc: false });
  const { showAvatars } = useSettings();

  const sources = useMemo(() => {
    const set = new Set<string>();
    for (const c of contacts ?? []) set.add(c.source);
    return [...set].sort((a, b) => (a === "Address Book" ? -1 : b === "Address Book" ? 1 : a.localeCompare(b)));
  }, [contacts]);

  const filtered = useMemo(() => {
    if (!contacts) return [];
    const needle = q.trim().toLowerCase();
    return contacts.filter((c) => {
      if (sources.length > 1 && c.source !== source) return false;
      if (!needle) return true;
      const hay = [
        contactName(c),
        c.organization,
        ...c.phones.map((p) => p.value),
        ...c.emails.map((e) => e.value),
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      return hay.includes(needle);
    });
  }, [contacts, q, source, sources]);

  const sorted = useMemo(
    () =>
      sortItems(
        filtered,
        (c) =>
          sort.by === "organization"
            ? (c.organization ?? "").toLowerCase()
            : contactName(c).toLowerCase(),
        sort.desc,
      ),
    [filtered, sort],
  );

  if (active === false) {
    return (
      <EmptyView icon={Users} title="No backup open" description="Import a backup to see contacts.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const selected = sorted.find((c) => c.id === selectedId) ?? sorted[0] ?? null;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Contacts" count={filtered.length} />
          <div className="space-y-2 border-b p-2">
            <ListSearch value={q} onChange={setQ} placeholder="Search contacts" />
            {sources.length > 1 && (
              <ToggleGroup
                type="single"
                size="sm"
                variant="outline"
                value={source}
                onValueChange={(v) => v && setSource(v)}
                className="flex-wrap justify-start"
              >
                {sources.map((s) => (
                  <ToggleGroupItem key={s} value={s}>
                    {s}
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
            )}
            <div className="flex justify-end">
              <SortControl
                fields={[
                  { value: "name", label: "Name" },
                  { value: "organization", label: "Organization" },
                ]}
                value={sort}
                onChange={setSort}
              />
            </div>
          </div>
          {error ? (
            <ErrorState error={error} />
          ) : isPending ? (
            <div className="min-h-0 flex-1 overflow-auto">
              {Array.from({ length: 8 }).map((_, i) => (
                <div key={i} className="px-3 py-2">
                  <Skeleton className="h-9 w-full" />
                </div>
              ))}
            </div>
          ) : filtered.length === 0 ? (
            <p className="px-4 py-6 text-sm text-muted-foreground">
              No contacts in this backup.
            </p>
          ) : (
            <VirtualList
              items={sorted}
              getKey={(c) => c.id}
              estimateSize={56}
              renderItem={(c) => (
                <ContactRow
                  contact={c}
                  showAvatars={showAvatars}
                  active={selected?.id === c.id}
                  onClick={() => setSelectedId(c.id)}
                />
              )}
            />
          )}
        </>
      }
      detail={
        selected ? (
          <ContactDetail contact={selected} showAvatars={showAvatars} />
        ) : (
          !isPending && <EmptyView icon={Users} title="No contact selected" description="Pick someone on the left." />
        )
      }
    />
  );
}

function ContactRow({
  contact,
  showAvatars,
  active,
  onClick,
}: {
  contact: Contact;
  showAvatars: boolean;
  active: boolean;
  onClick: () => void;
}) {
  const name = contactName(contact);
  const isOrg = !contact.firstName && !contact.lastName && !!contact.organization;
  return (
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button onClick={onClick} className="w-full text-left">
        {showAvatars && (
          <ItemMedia>
            <Avatar>
              {contact.hasImage && (
                <AvatarImage src={client.contactAvatarUrl(contact.id)} alt="" />
              )}
              <AvatarFallback>{isOrg ? <Building2 className="size-4" /> : initials(name)}</AvatarFallback>
            </Avatar>
          </ItemMedia>
        )}
        <ItemContent>
          <ItemTitle className="truncate">{name}</ItemTitle>
          {contact.organization && !isOrg && (
            <ItemDescription className="truncate">{contact.organization}</ItemDescription>
          )}
        </ItemContent>
      </button>
    </Item>
  );
}

function ContactDetail({ contact, showAvatars }: { contact: Contact; showAvatars: boolean }) {
  const navigate = useNavigate();
  const name = contactName(contact);
  const isOrg = !contact.firstName && !contact.lastName && !!contact.organization;

  // Message threads whose handle matches one of this contact's numbers/emails.
  const { data: threads } = useQuery({
    queryKey: ["threads"],
    queryFn: () => client.listThreads(),
  });
  const keys = new Set(
    [...contact.phones, ...contact.emails]
      .map((v) => phoneOrEmailKey(v.value))
      .filter(Boolean),
  );
  const conversations = (threads ?? []).filter(
    (t) =>
      keys.has(phoneOrEmailKey(t.displayName ?? t.identifier)) ||
      t.participants.some((h) => keys.has(phoneOrEmailKey(h))),
  );
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Contact" />
      <ScrollArea className="flex-1">
        <div className="mx-auto max-w-xl p-6">
          <div className="flex flex-col items-center gap-3 pb-6 text-center">
            <Avatar className="size-20 text-xl">
              {contact.hasImage && showAvatars && (
                <AvatarImage src={client.contactAvatarUrl(contact.id)} alt="" />
              )}
              <AvatarFallback>
                {isOrg ? <Building2 className="size-8" /> : initials(name)}
              </AvatarFallback>
            </Avatar>
            <div>
              <h2 className="text-xl font-semibold">{name}</h2>
              {contact.organization && !isOrg && (
                <p className="text-sm text-muted-foreground">{contact.organization}</p>
              )}
            </div>
          </div>

          {contact.phones.length > 0 && (
            <FieldGroup title="Phone">
              {contact.phones.map((p, i) => (
                <Field key={i} icon={Phone} label={p.label} value={p.value} href={`tel:${p.value}`} />
              ))}
            </FieldGroup>
          )}
          {contact.emails.length > 0 && (
            <FieldGroup title="Email">
              {contact.emails.map((e, i) => (
                <Field key={i} icon={Mail} label={e.label} value={e.value} href={`mailto:${e.value}`} />
              ))}
            </FieldGroup>
          )}
          {conversations.length > 0 && (
            <FieldGroup title={`Conversations (${conversations.length})`}>
              {conversations.map((t) => (
                <button
                  key={t.id}
                  onClick={() => navigate({ to: "/messages", search: { thread: t.id } })}
                  className={cn(
                    "flex w-full items-center gap-3 border-b px-3 py-2.5 text-left last:border-b-0",
                    "transition-colors hover:bg-accent/50",
                  )}
                >
                  <MessageSquare className="size-4 shrink-0 text-muted-foreground" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm">
                      {t.participants.length > 1
                        ? (t.displayName ?? "Group chat")
                        : "Direct messages"}
                    </div>
                    <div className="truncate text-xs text-muted-foreground">
                      {t.messageCount} message{t.messageCount === 1 ? "" : "s"}
                      {t.snippet ? ` · ${t.snippet}` : ""}
                    </div>
                  </div>
                </button>
              ))}
            </FieldGroup>
          )}
          {contact.phones.length === 0 && contact.emails.length === 0 && (
            <p className="text-center text-sm text-muted-foreground">
              No phone or email saved for this contact.
            </p>
          )}
        </div>
      </ScrollArea>
    </div>
  );
}

function FieldGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mb-4">
      <h3 className="mb-1 px-1 text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      <div className="overflow-hidden rounded-lg border">{children}</div>
    </div>
  );
}

function Field({
  icon: Icon,
  label,
  value,
  href,
}: {
  icon: typeof Phone;
  label: string | null;
  value: string;
  href: string;
}) {
  return (
    <a
      href={href}
      className={cn(
        "flex items-center gap-3 border-b px-3 py-2.5 last:border-b-0",
        "transition-colors hover:bg-accent/50",
      )}
    >
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0">
        {label && <div className="text-xs text-muted-foreground">{label}</div>}
        <div className="select-text truncate text-sm">{value}</div>
      </div>
    </a>
  );
}
