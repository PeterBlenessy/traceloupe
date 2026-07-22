import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  AtSign,
  Briefcase,
  Building2,
  Cake,
  Mail,
  MapPin,
  MessageSquare,
  Phone,
  StickyNote,
  Tag,
  User,
  Users,
} from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Item, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { ScrollArea } from "@/components/ui/scroll-area";
import { type BadgeFilterOption } from "@/components/badge-filter";
import { VirtualList } from "@/components/virtual-list";
import { useSettings } from "@/components/settings-provider";
import { SortControl, sortItems, type SortState } from "@/components/sort-control";
import { useViewToolbar } from "@/components/toolbar-context";
import { badgeGroup, type FilterGroup } from "@/components/filter-groups";
import { NoBackupState,
  EmptyView,
  ErrorState,
  ListDetail,
  ListSearch,
  ListSkeleton,
} from "@/components/view";
import { usePersistedState } from "@/lib/use-persisted-state";
import { contactName, initials } from "@/lib/contact";
import { phoneOrEmailKey } from "@/lib/use-contact-resolver";
import { cn } from "@/lib/utils";
import { client, type Contact } from "@/lib/ipc";
import { formatDate } from "@/lib/format";

export function ContactsView() {
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
  // Deep link from a message avatar: `?id=<contactId>` selects that contact and
  // switches the source filter to the one it belongs to, so it's visible.
  const search = useSearch({ strict: false }) as { id?: number };
  useEffect(() => {
    if (search.id == null || !contacts) return;
    const c = contacts.find((x) => x.id === search.id);
    if (c) {
      setSource(c.source);
      setSelectedId(c.id);
    }
  }, [search.id, contacts]);
  // Source filter: the device address book vs a third-party app's social graph
  // (e.g. TikTok). Default to the address book so app contacts don't bury it.
  const [source, setSource] = usePersistedState("contacts:source", "Address Book");
  const [sort, setSort] = usePersistedState<SortState>("contacts:sort", {
    by: "name",
    desc: false,
  });
  const { showAvatars } = useSettings();

  const sources = useMemo(() => {
    const set = new Set<string>();
    for (const c of contacts ?? []) set.add(c.source);
    return [...set].sort((a, b) => (a === "Address Book" ? -1 : b === "Address Book" ? 1 : a.localeCompare(b)));
  }, [contacts]);

  // If the saved source isn't present (e.g. a backup with no Address Book
  // contacts), fall back to the first available so the list never filters to
  // nothing.
  const activeSource = sources.includes(source) ? source : (sources[0] ?? source);

  const filtered = useMemo(() => {
    if (!contacts) return [];
    const needle = q.trim().toLowerCase();
    return contacts.filter((c) => {
      if (sources.length > 1 && c.source !== activeSource) return false;
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
  }, [contacts, q, activeSource, sources]);

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

  const filterGroups = useMemo<FilterGroup[]>(() => {
    if (sources.length <= 1) return [];
    const sourceOptions: BadgeFilterOption[] = sources.map((s) => ({
      value: s,
      label: s,
      count: (contacts ?? []).filter((c) => c.source === s).length,
    }));
    return [
      badgeGroup({ key: "source", label: "Source", description: "Address Book or a third-party app", options: sourceOptions, value: activeSource, onChange: setSource }),
    ];
  }, [sources, contacts, activeSource, setSource]);
  const sortNode = useMemo(
    () => (
      <SortControl
        fields={[
          { value: "name", label: "Name" },
          { value: "organization", label: "Organization" },
        ]}
        value={sort}
        onChange={setSort}
      />
    ),
    [sort, setSort],
  );
  const searchNode = useMemo(
    () => <ListSearch value={q} onChange={setQ} placeholder="Search contacts" />,
    [q],
  );
  const toolbar = useMemo(
    () =>
      active === true
        ? { title: "Contacts", count: filtered.length, filter: filterGroups, sort: sortNode, search: searchNode }
        : null,
    [active, filtered.length, filterGroups, sortNode, searchNode],
  );
  useViewToolbar(toolbar);

  if (active === false) {
    return (
      <NoBackupState
        icon={Users}
        title="See contacts"
        lead="The device's address book — phone numbers, emails, addresses, social handles, birthdays, and saved photos — laid out in a searchable directory."
        features={[
          { label: "Search", detail: "Search by name, organization, phone, or email." },
          { label: "Filter & sort", detail: "Filter by source and sort by name or organization." },
          { label: "Cross-linked", detail: "Jump straight to a person's message threads from their card." },
          { label: "Full card", detail: "See related names, groups, job title, nickname, and notes." },
        ]}
        note="Read straight from the backup on this Mac — nothing is uploaded."
      />
    );
  }

  const selected = sorted.find((c) => c.id === selectedId) ?? sorted[0] ?? null;

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1">
        <ListDetail
          master={
            error ? (
              <ErrorState error={error} />
            ) : isPending ? (
              <ListSkeleton />
            ) : filtered.length === 0 ? (
              <EmptyView
                icon={Users}
                title={
                  (contacts?.length ?? 0) === 0
                    ? "No contacts in this backup."
                    : "No matching contacts."
                }
              />
            ) : (
              <VirtualList
                items={sorted}
                getKey={(c) => c.id}
                estimateSize={56}
                renderItem={(c) => (
                  <div className="px-2 py-0.5">
                    <ContactRow
                      contact={c}
                      showAvatars={showAvatars}
                      active={selected?.id === c.id}
                      onClick={() => setSelectedId(c.id)}
                    />
                  </div>
                )}
              />
            )
          }
          detail={
            selected ? (
              <ContactDetail contact={selected} showAvatars={showAvatars} />
            ) : (
              !isPending && (
                <EmptyView
                  icon={Users}
                  title="No contact selected"
                  description="Pick someone on the left."
                />
              )
            )
          }
        />
      </div>
    </div>
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
    <Item
      asChild
      size="sm"
      data-active={active}
      className="rounded-md transition-colors hover:bg-accent/50 data-[active=true]:bg-accent data-[active=true]:hover:bg-accent"
    >
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
      {/* No header-bar name here — it's already the big name under the avatar
          below (and highlighted in the list), so a top-bar title just repeats it. */}
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
          {contact.addresses.length > 0 && (
            <FieldGroup title="Address">
              {contact.addresses.map((a, i) => (
                <Field key={i} icon={MapPin} label={a.label} value={a.value} wrap />
              ))}
            </FieldGroup>
          )}
          {contact.related.length > 0 && (
            <FieldGroup title="Related">
              {contact.related.map((rel, i) => (
                <Field key={i} icon={Users} label={rel.label} value={rel.value} />
              ))}
            </FieldGroup>
          )}
          {contact.social.length > 0 && (
            <FieldGroup title="Social">
              {contact.social.map((s, i) => (
                <Field key={i} icon={AtSign} label={s.label} value={s.value} />
              ))}
            </FieldGroup>
          )}
          {contact.groups.length > 0 && (
            <FieldGroup title="Groups">
              <div className="flex flex-wrap gap-1.5 px-3 py-2.5">
                {contact.groups.map((g) => (
                  <span
                    key={g}
                    className="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground"
                  >
                    <Tag className="size-3" />
                    {g}
                  </span>
                ))}
              </div>
            </FieldGroup>
          )}
          {(contact.jobTitle || contact.department) && (
            <FieldGroup title="Work">
              {contact.jobTitle && (
                <Field icon={Briefcase} label="Title" value={contact.jobTitle} />
              )}
              {contact.department && (
                <Field icon={Building2} label="Department" value={contact.department} />
              )}
            </FieldGroup>
          )}
          {contact.nickname && (
            <FieldGroup title="Nickname">
              <Field icon={User} label={null} value={contact.nickname} />
            </FieldGroup>
          )}
          {contact.birthdayAt != null && (
            <FieldGroup title="Birthday">
              <Field icon={Cake} label={null} value={formatDate(contact.birthdayAt)} />
            </FieldGroup>
          )}
          {contact.note && (
            <FieldGroup title="Note">
              <Field icon={StickyNote} label={null} value={contact.note} wrap />
            </FieldGroup>
          )}
          {conversations.length > 0 && (
            <FieldGroup title={`Conversations (${conversations.length})`}>
              {conversations.map((t) => (
                <button
                  key={t.id}
                  onClick={() => navigate({ to: "/messages", search: { thread: t.id } })}
                  data-slot="list-row"
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
  wrap,
}: {
  icon: typeof Phone;
  label: string | null;
  value: string;
  /** When set, the field is a clickable link (tel:/mailto:). Plain text otherwise. */
  href?: string;
  /** Let a long value (e.g. a note) wrap instead of truncating. */
  wrap?: boolean;
}) {
  const inner = (
    <>
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0">
        {label && <div className="text-xs text-muted-foreground">{label}</div>}
        <div className={cn("select-text text-sm", wrap ? "break-words" : "truncate")}>
          {value}
        </div>
      </div>
    </>
  );
  const className = cn(
    "flex items-center gap-3 border-b px-3 py-2.5 last:border-b-0",
    href && "transition-colors hover:bg-accent/50",
  );
  return href ? (
    <a href={href} className={className}>
      {inner}
    </a>
  ) : (
    <div className={className}>{inner}</div>
  );
}
