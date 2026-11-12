import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { ApiError, peopleApi, type Person, type PersonRow } from "../api";
import { language, tx } from "../i18n";
import { Menu01Icon, Search01Icon } from "../icons";
import { Avatar, Icon, IconButton, Problem, Spinner } from "./ui";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const lines = (text: string) =>
  text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);

/** The whole list from the last visit, so coming back shows it at once while it's read again. */
let lastPeople: PersonRow[] | null = null;
export const forgetContacts = () => {
  lastPeople = null;
};

/** Everyone the person deals with: saved contacts first, then who they email most. */
export function ContactsView({
  onPhoneNav,
  onWrite,
  onEmailsWith,
}: {
  onPhoneNav?: () => void;
  /** A new email to this address. */
  onWrite: (address: string) => void;
  /** Every email with this person, at any of their addresses. */
  onEmailsWith: (addresses: string[], name: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [people, setPeople] = useState<PersonRow[] | null>(lastPeople);
  const [picked, setPicked] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const filePicker = useRef<HTMLInputElement>(null);

  const load = useCallback(() => {
    peopleApi
      .list(query)
      .then((list) => {
        if (!query) lastPeople = list;
        setPeople(list);
      })
      .catch((e) => setProblem(say(e)));
  }, [query]);
  useEffect(() => {
    const timer = setTimeout(load, 150);
    return () => clearTimeout(timer);
  }, [load]);

  const importFile = async (file: File) => {
    setProblem(null);
    try {
      const r = await peopleApi.importVcard(await file.text());
      setNote(tx("Added {added}, updated {updated}.", { added: r.added, updated: r.updated }));
      load();
    } catch (e) {
      setProblem(say(e));
    }
  };

  const exportAll = async () => {
    setProblem(null);
    try {
      const saved = await peopleApi.exportFile();
      setNote(tx("Saved to Downloads as {name}.", { name: saved.name }));
    } catch (e) {
      setProblem(say(e));
    }
  };

  const shown = people?.find((p) => p.emails[0] === picked) ?? null;

  return (
    <div className="calendar contacts">
      <main className="calendar-month">
        <div className="list-head" data-tauri-drag-region="deep">
          {onPhoneNav && <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />}
          <h2 className="list-title">{tx("Contacts")}</h2>
          <span className="spacer" />
          <button className="btn btn-ghost btn-sm" onClick={() => filePicker.current?.click()}>
            {tx("Import")}
          </button>
          <button className="btn btn-ghost btn-sm" onClick={() => void exportAll()}>
            {tx("Export")}
          </button>
          <button
            className="btn btn-secondary btn-sm"
            onClick={() => {
              setAdding(true);
              setPicked(null);
            }}
          >
            {tx("New contact")}
          </button>
          <input
            ref={filePicker}
            type="file"
            accept=".vcf,text/vcard"
            className="hidden"
            aria-label={tx("Import contacts")}
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) void importFile(file);
            }}
          />
        </div>
        <div className="contacts-search">
          <Icon icon={Search01Icon} size="s" />
          <input
            type="search"
            aria-label={tx("Search contacts")}
            placeholder={tx("Search by name, email or company")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <Problem text={problem} />
        {note && (
          <div role="status" className="tip contacts-note">
            {note}
          </div>
        )}
        <div className="list-scroll">
          {!people && (
            <div className="empty">
              <Spinner />
            </div>
          )}
          {people?.length === 0 && <div className="empty-text contacts-empty">{tx("No one matches.")}</div>}
          {!!people?.length && (
            <div className="contacts-cols" aria-hidden="true">
              <span className="grow">{tx("Name")}</span>
              <span className="contacts-count">{tx("Emails")}</span>
            </div>
          )}
          {people?.map((p) => (
            <button
              key={p.emails[0] ?? p.id}
              className={`contacts-row ${picked === p.emails[0] ? "contacts-row-on" : ""}`}
              onClick={() => {
                setAdding(false);
                setPicked(p.emails[0] ?? null);
              }}
            >
              <Avatar name={p.name || p.emails[0] || "?"} email={p.emails[0]} size="s" />
              <span className="grow min-w-0">
                <span className="contacts-name truncate-1">{p.name || p.emails[0]}</span>
                <span className="contacts-meta truncate-1">
                  {[p.company, p.emails[0]].filter(Boolean).join(" · ")}
                </span>
              </span>
              {!p.saved && <span className="pill">{tx("From your mail")}</span>}
              <span className="contacts-count tnum" aria-label={tx("{n} emails", { n: p.count })}>
                {p.count > 0 ? p.count.toLocaleString(language) : ""}
              </span>
            </button>
          ))}
        </div>
      </main>
      <aside className="calendar-day" aria-label={tx("Contact")}>
        {adding ? (
          <PersonForm
            person={null}
            onCancel={() => setAdding(false)}
            onSaved={(p) => {
              setAdding(false);
              load();
              setPicked(p.emails[0] ?? null);
            }}
          />
        ) : shown ? (
          <PersonDetail key={shown.emails[0]} row={shown} onWrite={onWrite} onEmailsWith={onEmailsWith} onChanged={load} />
        ) : (
          <div className="empty-text">{tx("Pick someone to see their details.")}</div>
        )}
      </aside>
    </div>
  );
}

function PersonDetail({
  row,
  onWrite,
  onEmailsWith,
  onChanged,
}: {
  row: PersonRow;
  onWrite: (address: string) => void;
  onEmailsWith: (addresses: string[], name: string) => void;
  onChanged: () => void;
}) {
  const address = row.emails[0] ?? "";
  const [info, setInfo] = useState<Awaited<ReturnType<typeof peopleApi.get>> | null>(null);
  const [editing, setEditing] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const load = useCallback(() => {
    peopleApi
      .get(address)
      .then(setInfo)
      .catch((e) => setProblem(say(e)));
  }, [address]);
  useEffect(load, [load]);

  const person = info?.person ?? null;
  const name = person?.name || info?.name_from_mail || row.name || address;
  if (editing) {
    return (
      <PersonForm
        person={person ?? { id: null, name, emails: row.emails, phones: [], company: row.company, notes: "" }}
        onCancel={() => setEditing(false)}
        onSaved={() => {
          setEditing(false);
          load();
          onChanged();
        }}
        onDeleted={() => {
          setEditing(false);
          onChanged();
        }}
      />
    );
  }
  const date = (unix: number | null | undefined) =>
    unix ? new Date(unix * 1000).toLocaleDateString(language, { day: "numeric", month: "long", year: "numeric" }) : null;
  return (
    <div className="stack">
      <div className="contacts-head">
        <Avatar name={name} email={address} size="m" />
        <div className="min-w-0">
          <h3 className="strong truncate-1">{name}</h3>
          {person?.company && <div className="muted truncate-1">{person.company}</div>}
        </div>
      </div>
      <div className="contacts-actions">
        <button className="btn btn-primary btn-sm" onClick={() => onWrite(address)}>
          {tx("Write")}
        </button>
        <button className="btn btn-secondary btn-sm" onClick={() => onEmailsWith(person?.emails.length ? person.emails : [address], name)}>
          {tx("Emails with them")}
        </button>
        <button className="btn btn-ghost btn-sm" onClick={() => setEditing(true)}>
          {person ? tx("Edit") : tx("Save as contact")}
        </button>
      </div>
      <div className="group">
        {(person?.emails ?? row.emails).map((e) => (
          <div key={e} className="group-row">
            <span className="grow truncate-1 selectable">{e}</span>
          </div>
        ))}
        {person?.phones.map((p) => (
          <div key={p} className="group-row">
            <span className="grow truncate-1 selectable tnum">{p}</span>
          </div>
        ))}
      </div>
      {person?.notes && <p className="body-text selectable contacts-notes">{person.notes}</p>}
      {info && info.count > 0 && (
        <p className="fine">
          {info.count === 1
            ? tx("1 email together, on {day}.", { day: date(info.last_at) ?? "" })
            : date(info.first_at) === date(info.last_at)
              ? tx("{n} emails together, on {day}.", { n: info.count, day: date(info.last_at) ?? "" })
              : tx("{n} emails together, from {first} to {last}.", {
                  n: info.count,
                  first: date(info.first_at) ?? "",
                  last: date(info.last_at) ?? "",
                })}
        </p>
      )}
      <Problem text={problem} />
    </div>
  );
}

function PersonForm({
  person,
  onCancel,
  onSaved,
  onDeleted,
}: {
  person: (Omit<Person, "id" | "uid" | "updated_at"> & { id: number | null }) | null;
  onCancel: () => void;
  onSaved: (p: Person) => void;
  onDeleted?: () => void;
}) {
  const [name, setName] = useState(person?.name ?? "");
  const [emails, setEmails] = useState((person?.emails ?? []).join("\n"));
  const [phones, setPhones] = useState((person?.phones ?? []).join("\n"));
  const [company, setCompany] = useState(person?.company ?? "");
  const [notes, setNotes] = useState(person?.notes ?? "");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setProblem(null);
    try {
      onSaved(
        await peopleApi.save({
          id: person?.id ?? undefined,
          name: name.trim(),
          emails: lines(emails),
          phones: lines(phones),
          company: company.trim(),
          notes,
        }),
      );
    } catch (err) {
      setProblem(say(err));
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!person?.id) return;
    try {
      await peopleApi.remove(person.id);
      onDeleted?.();
    } catch (err) {
      setProblem(say(err));
    }
  };

  return (
    <form className="stack" onSubmit={save}>
      <h3 className="strong">{person?.id ? tx("Edit contact") : tx("New contact")}</h3>
      <label className="field">
        <span className="field-label">{tx("Name")}</span>
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field">
        <span className="field-label">{tx("Email addresses, one per line")}</span>
        <textarea rows={2} value={emails} onChange={(e) => setEmails(e.target.value)} spellCheck={false} />
      </label>
      <label className="field">
        <span className="field-label">{tx("Phone numbers, one per line")}</span>
        <textarea rows={2} value={phones} onChange={(e) => setPhones(e.target.value)} spellCheck={false} />
      </label>
      <label className="field">
        <span className="field-label">{tx("Company")}</span>
        <input value={company} onChange={(e) => setCompany(e.target.value)} />
      </label>
      <label className="field">
        <span className="field-label">{tx("Notes")}</span>
        <textarea rows={4} value={notes} onChange={(e) => setNotes(e.target.value)} />
      </label>
      <Problem text={problem} />
      <div className="hstack">
        <button type="submit" className="btn btn-primary btn-sm" disabled={busy}>
          {busy && <Spinner size="xs" />}
          {tx("Save")}
        </button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={onCancel}>
          {tx("Cancel")}
        </button>
        <span className="spacer" />
        {person?.id && onDeleted && (
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => void remove()}>
            {tx("Delete")}
          </button>
        )}
      </div>
    </form>
  );
}
