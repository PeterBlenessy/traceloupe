import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Building2, Mail, Phone, Users } from "lucide-react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyView, ListDetail, ListSearch, ViewHeader } from "@/components/view";
import { contactName, initials } from "@/lib/contact";
import { cn } from "@/lib/utils";
import { client, type Contact } from "@/lib/ipc";

export function ContactsView() {
  const navigate = useNavigate();
  const { data: active } = useQuery({
    queryKey: ["hasActiveBackup"],
    queryFn: () => client.hasActiveBackup(),
  });
  const { data: contacts, isPending } = useQuery({
    queryKey: ["contacts"],
    queryFn: () => client.listContacts(),
    enabled: active === true,
  });
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [q, setQ] = useState("");

  const filtered = useMemo(() => {
    if (!contacts) return [];
    const needle = q.trim().toLowerCase();
    if (!needle) return contacts;
    return contacts.filter((c) => {
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
  }, [contacts, q]);

  if (active === false) {
    return (
      <EmptyView icon={Users} title="No backup open" description="Import a backup to see contacts.">
        <Button onClick={() => navigate({ to: "/" })}>Choose a backup</Button>
      </EmptyView>
    );
  }

  const selected = filtered.find((c) => c.id === selectedId) ?? filtered[0] ?? null;

  return (
    <ListDetail
      master={
        <>
          <ViewHeader title="Contacts" count={contacts?.length} />
          <div className="border-b p-2">
            <ListSearch value={q} onChange={setQ} placeholder="Search contacts" />
          </div>
          <ScrollArea className="flex-1">
            {isPending &&
              Array.from({ length: 8 }).map((_, i) => (
                <div key={i} className="px-3 py-2">
                  <Skeleton className="h-9 w-full" />
                </div>
              ))}
            {contacts?.length === 0 && (
              <p className="px-4 py-6 text-sm text-muted-foreground">
                No contacts in this backup.
              </p>
            )}
            {filtered.map((c) => (
              <ContactRow
                key={c.id}
                contact={c}
                active={selected?.id === c.id}
                onClick={() => setSelectedId(c.id)}
              />
            ))}
          </ScrollArea>
        </>
      }
      detail={
        selected ? (
          <ContactDetail contact={selected} />
        ) : (
          !isPending && <EmptyView icon={Users} title="No contact selected" description="Pick someone on the left." />
        )
      }
    />
  );
}

function ContactRow({
  contact,
  active,
  onClick,
}: {
  contact: Contact;
  active: boolean;
  onClick: () => void;
}) {
  const name = contactName(contact);
  const isOrg = !contact.firstName && !contact.lastName && !!contact.organization;
  return (
    <Item asChild data-active={active} className="rounded-none data-[active=true]:bg-accent">
      <button onClick={onClick} className="w-full text-left">
        <ItemMedia>
          <Avatar>
            <AvatarFallback>{isOrg ? <Building2 className="size-4" /> : initials(name)}</AvatarFallback>
          </Avatar>
        </ItemMedia>
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

function ContactDetail({ contact }: { contact: Contact }) {
  const name = contactName(contact);
  const isOrg = !contact.firstName && !contact.lastName && !!contact.organization;
  return (
    <div className="flex h-full flex-col">
      <ViewHeader title="Contact" />
      <ScrollArea className="flex-1">
        <div className="mx-auto max-w-xl p-6">
          <div className="flex flex-col items-center gap-3 pb-6 text-center">
            <Avatar className="size-20 text-xl">
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
