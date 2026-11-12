import { useEffect, useRef, useState } from "react";
import type { AskSource, Rule, SenderRow } from "../api";
import type { SearchFilters } from "../types";
import { ApiError, api } from "../api";
import { myLanguage, shortDate } from "../format";
import {
  ArrowLeft01Icon,
  Calendar03Icon,
  Clock01Icon,
  InboxIcon,
  Tick02Icon,
  UserBlock01Icon,
  Menu01Icon,
} from "../icons";
import { Avatar, Dialog, Icon, IconButton, MenuItem, Problem, Spinner } from "./ui";
import type { Draft } from "../types";
import { language, tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

const at = (d: Date, hour: number) => {
  const copy = new Date(d);
  copy.setHours(hour, 0, 0, 0);
  return Math.floor(copy.getTime() / 1000);
};

const inDays = (from: Date, days: number) => {
  const d = new Date(from);
  d.setDate(d.getDate() + days);
  return d;
};

/** The small-hours "tomorrow" is still today to most people, so both are offered. */
const SMALL_HOURS_END = 6;

/** The usual moments to come back to something. Anything already past is left out. */
export function whenChoices(now = new Date()): { label: string; at: number; hint: string }[] {
  const out: { label: string; at: number; hint: string }[] = [];
  const nowUnix = Math.floor(now.getTime() / 1000);
  const hint = (unix: number) =>
    new Date(unix * 1000).toLocaleString(language, { weekday: "short", hour: "numeric", minute: "2-digit" });
  const add = (label: string, unix: number) => {
    if (unix > nowUnix + 60) out.push({ label, at: unix, hint: hint(unix) });
  };
  if (now.getHours() < SMALL_HOURS_END) add(tx("This morning"), at(now, 9));
  if (now.getHours() >= SMALL_HOURS_END && now.getHours() < 17) add(tx("Later today"), nowUnix + 3 * 3600);
  add(tx("This evening"), at(now, 18));
  add(tx("Tomorrow morning"), at(inDays(now, 1), 9));
  // Not offered when the weekend is tomorrow, or right now.
  const untilSaturday = (6 - now.getDay() + 7) % 7;
  if (untilSaturday > 1) add(now.getDay() === 0 ? tx("Next weekend") : tx("This weekend"), at(inDays(now, untilSaturday), 9));
  // On a Sunday, next week is tomorrow morning, which is already there.
  const untilMonday = ((8 - now.getDay()) % 7) || 7;
  if (untilMonday > 1) add(tx("Next week"), at(inDays(now, untilMonday), 9));
  return out;
}

/** Fills a menu with those moments, and a way to choose another. */
export function WhenItems({
  close,
  onPick,
  onOther,
}: {
  close: () => void;
  onPick: (unix: number) => void;
  onOther: () => void;
}) {
  return (
    <>
      {whenChoices().map((c) => (
        <MenuItem
          key={c.label}
          icon={Clock01Icon}
          trailing={<span className="menu-key">{c.hint}</span>}
          onClick={() => {
            close();
            onPick(c.at);
          }}
        >
          {c.label}
        </MenuItem>
      ))}
      <div className="menu-sep" />
      <MenuItem
        icon={Calendar03Icon}
        onClick={() => {
          close();
          onOther();
        }}
      >
        {tx("Pick a date and time")}</MenuItem>
    </>
  );
}

const localValue = (unix: number) => {
  const d = new Date(unix * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

export function PickTime({
  title,
  confirm,
  note,
  onPick,
  onClose,
}: {
  title: string;
  confirm: string;
  note?: string;
  onPick: (unix: number) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState(() => localValue(Math.floor(Date.now() / 1000) + 3600));
  const unix = Math.floor(new Date(value).getTime() / 1000);
  const past = !(unix > Date.now() / 1000);
  return (
    <Dialog
      title={title}
      width="s"
      onClose={onClose}
      onSubmit={(e) => {
        e.preventDefault();
        if (!past) onPick(unix);
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tx("Cancel")}</button>
          <button type="submit" className="btn btn-primary" disabled={past}>
            {confirm}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{tx("Date and time")}</span>
        <input
          type="datetime-local"
          aria-label={tx("Date and time")}
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
      </label>
      {past && <div className="fine note">{tx("Pick a time that's still to come.")}</div>}
      {note && !past && <div className="fine note">{note}</div>}
    </Dialog>
  );
}

const fullWhen = (unix: number | null) =>
  unix ? new Date(unix * 1000).toLocaleString(language, { dateStyle: "medium", timeStyle: "short" }) : "";

export function ScheduledList({
  drafts,
  top,
  onOpen,
  onPhoneNav,
}: {
  drafts: Draft[];
  top?: React.ReactNode;
  onOpen: (id: number) => void;
  onPhoneNav?: () => void;
}) {
  return (
    <main className="list">
      <div className="list-head" data-tauri-drag-region="deep">
        {onPhoneNav && <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />}
        <h2 className="list-title">{tx("Scheduled")}</h2>
      </div>
      {top}
      <div className="list-scroll">
        <div className="later-note">{tx("These go when the time comes, as long as this app is open then.")}</div>
        {drafts.length === 0 ? (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={Clock01Icon} size="l" />
            </div>
            <div className="empty-title">{tx("Nothing scheduled")}</div>
          </div>
        ) : (
          drafts.map((d) => {
            const people = [...d.to, ...d.cc, ...d.bcc].map((a) => a.name || a.address);
            return (
              <button key={d.id} className="thread" onClick={() => onOpen(d.id)}>
                <span className="thread-avatar">
                  <Avatar name={people[0] ?? "?"} email={d.to[0]?.address} />
                </span>
                <span className="grow">
                  <span className="thread-line">
                    <span className="thread-from truncate-1">{people.join(", ")}</span>
                    <span className="thread-time">{fullWhen(d.send_at)}</span>
                  </span>
                  <span className="thread-subject truncate-1">{d.subject || tx("(no subject)")}</span>
                  <span className="thread-snippet truncate-1">{d.text.trim() || " "}</span>
                </span>
              </button>
            );
          })
        )}
      </div>
    </main>
  );
}

/** New senders waiting to be let in. Their mail stays where it is on the server meanwhile. */
export function ScreenerList({
  top,
  onChanged,
  onOpen,
  onPhoneNav,
}: {
  onPhoneNav?: () => void;
  top?: React.ReactNode;
  onChanged: () => void;
  onOpen: (row: SenderRow) => void;
}) {
  const [rows, setRows] = useState<SenderRow[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const load = () =>
    api
      .screenerList()
      .then(setRows)
      .catch((e) => setProblem(say(e)));
  useEffect(() => {
    void load();
  }, []);
  const decide = async (row: SenderRow, allow: boolean) => {
    setRows((list) => list?.filter((r) => r.address !== row.address) ?? null);
    try {
      await api.screenerDecide(row.address, allow);
    } catch (e) {
      setProblem(say(e));
    }
    await load();
    onChanged();
  };
  return (
    <main className="list">
      <div className="list-head" data-tauri-drag-region="deep">
        {onPhoneNav && <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />}
        <h2 className="list-title">{tx("New senders")}</h2>
      </div>
      {top}
      <div className="list-scroll">
        <div className="later-note">
          {tx("People emailing you for the first time wait here. Nothing is deleted. Let them in once and they stay in.")}</div>
        <Problem text={problem} />
        {rows?.length === 0 && (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={InboxIcon} size="l" />
            </div>
            <div className="empty-title">{tx("Nobody's waiting")}</div>
          </div>
        )}
        {rows?.map((r) => (
          <div key={`${r.account_id}:${r.address}`} className="sender-row">
            <button className="sender-open" onClick={() => onOpen(r)}>
              <Avatar name={r.name || r.address} />
              <span className="grow">
                <span className="thread-from truncate-1">{r.name || r.address}</span>
                <span className="thread-snippet truncate-1">
                  {r.threads > 1 ? `${r.threads} emails · ` : ""}
                  {r.subject}
                </span>
              </span>
            </button>
            <button className="btn btn-secondary btn-sm" onClick={() => void decide(r, true)}>
              <Icon icon={Tick02Icon} size="s" />
              {tx("Let in")}</button>
            <IconButton
              icon={UserBlock01Icon}
              label={`Block ${r.address}`}
              small
              onClick={() => void decide(r, false)}
            />
          </div>
        ))}
      </div>
    </main>
  );
}

/** Everyone with mail here, instead of each email. */
export function SenderList({
  top,
  title,
  senders,
  onOpen,
  onBack,
}: {
  top?: React.ReactNode;
  title: string;
  senders: SenderRow[];
  onOpen: (row: SenderRow) => void;
  onBack: () => void;
}) {
  return (
    <main className="list">
      <div className="list-head" data-tauri-drag-region="deep">
        <IconButton icon={ArrowLeft01Icon} label={tx("Back to emails")} small onClick={onBack} />
        <h2 className="list-title">{title}</h2>
        <span className="list-sub">{tx("by sender")}</span>
      </div>
      {top}
      <div className="list-scroll">
        {senders.map((r) => (
          <div key={`${r.account_id}:${r.address}`} className="sender-row">
            <button className="sender-open" onClick={() => onOpen(r)}>
              <Avatar name={r.name || r.address} email={r.address} />
              <span className="grow">
                <span className="thread-line">
                  <span className="thread-from truncate-1">{r.name || r.address}</span>
                  <span className="thread-time">{shortDate(r.last_date)}</span>
                </span>
                <span className="thread-snippet truncate-1">{r.subject}</span>
              </span>
              <span className="thread-count">{r.threads}</span>
              {r.unread > 0 && <span className="nav-count nav-count-unread">{r.unread}</span>}
            </button>
          </div>
        ))}
      </div>
    </main>
  );
}

/** Ask about your mail: the closest emails are found here, and AI answers from them. */
export function AskAnswer({
  question,
  filters,
  onClose,
  onOpen,
}: {
  question: string;
  /** What the search box already worked out, so AI needn't be asked twice. */
  filters: SearchFilters | null;
  onClose: () => void;
  onOpen: (t: { account_id: number; thread_id: string; folder_id: number }) => void;
}) {
  const [sources, setSources] = useState<AskSource[] | null>(null);
  const [answer, setAnswer] = useState<Awaited<ReturnType<typeof api.aiAsk>> | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const asked = useRef(false);
  useEffect(() => {
    if (asked.current) return;
    asked.current = true;
    void (async () => {
      try {
        const found = (await api.aiAskSources(question, filters)).sources;
        setSources(found);
        setAnswer(await api.aiAsk(question, found.map((s) => s.id), myLanguage()));
      } catch (e) {
        setProblem(say(e));
      }
    })();
  }, [question, filters]);
  const rows: (AskSource & { title?: string; value?: string })[] = answer?.items ?? sources ?? [];
  return (
    <Dialog title={question} width="m" label={tx("Answer")} onClose={onClose}>
      <div className="stack">
        {problem ? (
          <Problem text={problem} />
        ) : answer ? (
          <div className="selectable ask-headline">{answer.headline}</div>
        ) : (
          <div className="hstack-m muted">
            <Spinner size="xs" />
            {sources ? (sources.length === 1 ? tx("Reading 1 email…") : tx("Reading {n} emails…", { n: sources.length })) : tx("Looking through your email…")}
          </div>
        )}
        {rows.length > 0 && (
          <div className={`ask-list ${answer ? "" : "ask-list-waiting"}`}>
            {rows.map((s) => (
              <button
                key={s.id}
                className="ask-row"
                onClick={() => {
                  onClose();
                  onOpen(s);
                }}
              >
                <span className="ask-row-main">
                  <span className="truncate-1 strong">{s.title || s.subject || tx("(no subject)")}</span>
                  <span className="truncate-1 muted">
                    {s.from} · {shortDate(s.date)}
                  </span>
                </span>
                {s.value && <span className="ask-value tnum">{s.value}</span>}
              </button>
            ))}
          </div>
        )}
      </div>
    </Dialog>
  );
}

export type { Rule };
