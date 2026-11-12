import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ApiError, api, type Newsletter } from "../api";
import { fullDate, shortDate } from "../format";
import {
  AlertCircleIcon,
  Archive02Icon,
  ArrowDown01Icon,
  MailRemove01Icon,
  Cancel01Icon,
  LinkSquare02Icon,
  Menu01Icon,
  News01Icon,
  Search01Icon,
  Tick02Icon,
} from "../icons";
import { useSettings } from "../settings";
import type { Account } from "../types";
import { AccountDot, Avatar, Dialog, Icon, IconButton, Problem, accountHue } from "./ui";
import { UNDO_MOVE_MS } from "./UndoMove";
import { flyAway } from "../fly";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
/** Rows are added as the list scrolls, as each one may fetch a sender's logo. */
const ROWS_AT_A_TIME = 60;
/** Senders may take days to stop, so mail after this long means it didn't work. */
const STILL_SENDING_AFTER = 7 * 86_400;
/** More senders than this at once are spelled out first, as one press then does a lot. */
const ASK_ABOVE = 10;

type Tab = "subscribed" | "unsubscribed";
type Sort = "latest" | "emails";
/** Where someone was, for coming back after looking at a sender's mail. */
export type NewsletterPlace = { tab: Tab; sort: Sort; query: string; rows: number; scroll: number };
/** Whose mail to show when a row is opened: every address of theirs in that inbox. */
export type NewsletterSender = { account_id: number; addresses: string[]; name: string; folder_id: number };

const keyOf = (r: Newsletter) => `${r.account_id}:${r.address}`;
const nameOf = (r: Newsletter) => r.name || r.address;
const plural = (n: number, one: string) => `${n.toLocaleString()} ${one}${n === 1 ? "" : "s"}`;

/** The part of an address that names the company: news.linkedin.com and linkedin.com are one. */
function companyDomain(address: string): string {
  const host = address.split("@")[1]?.toLowerCase() ?? address.toLowerCase();
  const parts = host.split(".");
  // Two-part endings such as co.uk keep one more part.
  const keep =
    parts.length > 2 &&
    parts[parts.length - 1].length === 2 &&
    ["co", "com", "org", "net", "ac", "gov"].includes(parts[parts.length - 2])
      ? 3
      : 2;
  return parts.slice(-keep).join(".");
}

/** One company or person in one inbox, however many addresses they send from (LinkedIn uses a dozen). */
type Sender = { key: string; rows: Newsletter[]; one: Newsletter };

/** All of a sender's rows as one: counts added up, their newest email, the best way out of
 * what's still coming, and left only once every address has been. */
function asOne(rows: Newsletter[]): Newsletter {
  const newest = rows.reduce((a, b) => (b.last_date > a.last_date ? b : a));
  const open = rows.filter((r) => !r.left);
  const from = open.length ? open : rows.filter((r) => r.message_id !== null);
  const way =
    (["one_click", "mail", "page", "none"] as const).map((how) => from.find((r) => r.how === how)).find(Boolean) ?? newest;
  return {
    ...newest,
    threads: rows.reduce((n, r) => n + r.threads, 0),
    message_id: way.message_id,
    how: way.how,
    left: open.length ? null : rows.reduce((a, b) => ((b.left?.at ?? 0) > (a.left?.at ?? 0) ? b : a)).left,
  };
}

function bySender(rows: Newsletter[]): Sender[] {
  const groups = new Map<string, Newsletter[]>();
  for (const r of rows) {
    const key = `${r.account_id}:${companyDomain(r.address)}:${nameOf(r).trim().toLowerCase()}`;
    groups.set(key, [...(groups.get(key) ?? []), r]);
  }
  return [...groups].map(([key, list]) => ({ key, rows: list, one: asOne(list) }));
}

/** What a click on Unsubscribe acts on: what's still coming, or, to try again, all of it. */
const waysOut = (s: Sender) => {
  const can = s.rows.filter((r) => r.message_id !== null && ["one_click", "mail", "page"].includes(r.how));
  const open = can.filter((r) => !r.left);
  return open.length ? open : can;
};

/** The last list, so coming back shows it straight away while it's read again. */
let lastRows: Newsletter[] | null = null;
export const forgetNewsletters = () => {
  lastRows = null;
};

/** Unsubscribes that didn't finish, and why, so they're never quietly put back as if
 * nothing happened. Kept until they go through. */
const trouble = new Map<string, { kind: "page" | "failed"; why: string; row: Newsletter }>();
/** How many unsubscribes run at once: all of them at once overwhelms slow senders. */
const AT_A_TIME = 4;

/** Senders clicked away and not yet back from the engine, so they stay gone while it works. */
const leaving = new Map<string, Newsletter>();
/** What can still be taken back. Nothing reaches a sender, and nothing is archived, until its time is up. */
let waiting: {
  rows: Newsletter[];
  archive: Sender[];
  what: string;
  /** Many at once, where a page that needs opening is left for its own row. */
  quietly: boolean;
  timer: ReturnType<typeof setTimeout>;
} | null = null;
let listChanged: (fromEngine: boolean) => void = () => {};
let tell: (notice: string) => void = () => {};
let inboxOf: (accountId: number) => number | undefined = () => undefined;

/** Who can only be left on their own page, which isn't opened for many at once. */
const onlyByPage = (names: string[]) =>
  `${names.length === 1 ? names[0] : plural(names.length, "sender")} can only be left on their own page: press Unsubscribe on ${names.length === 1 ? "their row" : "each row"}.`;

const send = (row: Newsletter, quietly = false) =>
  api
    .unsubscribe(row.message_id as number, quietly)
    .then((reply) => {
      if (reply.kind === "opened") tell("Their unsubscribe page opened in your browser.");
      if (reply.kind === "page") trouble.set(keyOf(row), { kind: "page", why: "Can only be left on their website", row });
      else trouble.delete(keyOf(row));
      return reply.kind;
    })
    .catch((e) => {
      trouble.set(keyOf(row), { kind: "failed", why: say(e), row });
      if (!quietly) tell(say(e));
      return "failed";
    })
    .finally(() => {
      leaving.delete(keyOf(row));
      listChanged(true);
    });

/** Their mail in the inbox, moved to Archive. */
const archive = async (s: Sender, quietly = false) => {
  const inbox = inboxOf(s.one.account_id);
  if (inbox === undefined) return;
  let moved = 0;
  try {
    for (const r of s.rows) {
      const list = await api.listFrom(inbox, r.address);
      if (list.length === 0) continue;
      await api.bulkThreads(
        list.map((t) => ({ account_id: t.account_id, thread_id: t.thread_id, folder_id: t.folder_id })),
        "move",
        "archive",
      );
      moved += list.length;
    }
    if (!quietly) {
      tell(
        moved
          ? `Moved ${plural(moved, "conversation")} from ${nameOf(s.one)} to Archive.`
          : `Nothing from ${nameOf(s.one)} is in the inbox.`,
      );
    }
  } catch (e) {
    tell(say(e));
  }
  listChanged(true);
};

const sendWaiting = () => {
  if (!waiting) return;
  clearTimeout(waiting.timer);
  const { rows, archive: archiving, quietly } = waiting;
  waiting = null;
  void (async () => {
    // A few at a time, so slow senders get the time they need.
    const kinds: string[] = [];
    let next = 0;
    const worker = async () => {
      while (next < rows.length) {
        const i = next++;
        kinds[i] = await send(rows[i], quietly);
      }
    };
    await Promise.all(Array.from({ length: Math.min(AT_A_TIME, rows.length) }, worker));
    const stuck = kinds.filter((k) => k === "page" || k === "failed").length;
    if (rows.length > 1) {
      const done = rows.length - stuck;
      tell(
        stuck === 0
          ? `Unsubscribed from ${plural(done, "list")}.`
          : `Unsubscribed from ${plural(done, "list")}. ${plural(stuck, "list")} need a step: they're at the top.`,
      );
    } else if (kinds[0] === "page") {
      tell(onlyByPage(rows.map(nameOf)));
    }
  })();
  for (const s of archiving) void archive(s, true);
  listChanged(false);
};

// ponytail: quitting during the wait leaves them subscribed, the same as a move in UndoMove.tsx.
/** Leaves these after a moment to take it back. Opening a page changes nothing, and waiting
 * would only make it feel broken, so pages open at once. */
const leave = (rows: Newsletter[], what: string, archiving: Sender[] = [], quietly = false) => {
  sendWaiting();
  for (const r of rows) leaving.set(keyOf(r), r);
  for (const r of rows) if (r.how === "page") void send(r);
  const later = rows.filter((r) => r.how !== "page");
  if (later.length > 0 || archiving.length > 0) {
    waiting = { rows: later, archive: archiving, what, quietly, timer: setTimeout(sendWaiting, UNDO_MOVE_MS) };
  }
  listChanged(false);
};

const takeBack = () => {
  if (!waiting) return;
  clearTimeout(waiting.timer);
  for (const r of waiting.rows) leaving.delete(keyOf(r));
  waiting = null;
  listChanged(false);
};

/** As it will be once the engine has finished with it. */
const withLeaving = (r: Newsletter): Newsletter =>
  leaving.has(keyOf(r))
    ? {
        ...r,
        left: {
          at: Date.now() / 1000,
          how: r.how === "page" ? "opened" : r.how === "mail" ? "sent" : "done",
          name: r.name,
        },
      }
    : r;

function LeaveButton({ row, inbox, label, onLeave }: { row: Newsletter; inbox: string; label: string; onLeave: () => void }) {
  const tip =
    row.how === "page"
      ? "Opens their unsubscribe page"
      : row.how === "mail"
        ? `Sends an unsubscribe email from ${inbox}`
        : undefined;
  return (
    <button
      type="button"
      className="btn btn-secondary btn-sm"
      aria-label={`${label} from ${nameOf(row)}`}
      data-tip={tip}
      onClick={(e) => {
        e.stopPropagation();
        onLeave();
      }}
    >
      {label}
      {row.how === "page" && <Icon icon={LinkSquare02Icon} size="xs" />}
    </button>
  );
}

function Status({
  row,
  inbox,
  stuck,
  onLeave,
  onLetBack,
}: {
  row: Newsletter;
  inbox: string;
  stuck?: { kind: "page" | "failed"; why: string };
  onLeave: () => void;
  onLetBack: () => void;
}) {
  const left = row.left;
  if (!left && stuck) {
    // An inbox that can't sign in can't send the email some senders ask for: say that plainly.
    const signIn = stuck.kind === "failed" && /password|sign.?in|rejected|authenticat/i.test(stuck.why);
    return (
      <>
        <span className="news-status news-status-warn truncate-1" data-tip={stuck.why}>
          {stuck.kind === "page" ? "Needs their website" : signIn ? `Sign in to ${inbox} again` : stuck.why}
        </span>
        <LeaveButton row={row} inbox={inbox} label={stuck.kind === "page" ? "Open page" : "Try again"} onLeave={onLeave} />
      </>
    );
  }
  if (!left) {
    if (row.how === "none") return <span className="news-status">No unsubscribe link</span>;
    return row.message_id === null ? null : <LeaveButton row={row} inbox={inbox} label="Unsubscribe" onLeave={onLeave} />;
  }
  const when = fullDate(left.at);
  if (row.last_date > left.at + STILL_SENDING_AFTER && row.message_id !== null) {
    return (
      <>
        <span className="news-status" data-tip={`You unsubscribed ${when}, but they're still sending`}>
          Still sending
        </span>
        <LeaveButton row={row} inbox={inbox} label="Unsubscribe" onLeave={onLeave} />
      </>
    );
  }
  if (left.how === "opened" && row.message_id !== null) {
    return (
      <>
        <span className="news-status" data-tip={`Opened ${when}. Their page may have asked for one more step`}>
          Page opened
        </span>
        <LeaveButton row={row} inbox={inbox} label="Open again" onLeave={onLeave} />
      </>
    );
  }
  return (
    <>
      <span
        className="news-status news-status-done"
        data-tip={`${left.how === "sent" ? `Unsubscribe email sent ${when}` : `Unsubscribed ${when}`}. New emails from them go to Archive.`}
      >
        <Icon icon={Tick02Icon} size="s" />
        Unsubscribed
      </span>
      <button
        type="button"
        className="btn btn-ghost btn-sm"
        data-tip="Their new emails stay in your inbox again"
        onClick={(e) => {
          e.stopPropagation();
          onLetBack();
        }}
      >
        Let back in
      </button>
    </>
  );
}

function SortHead({
  label,
  sort,
  current,
  className,
  onSort,
}: {
  label: string;
  sort: Sort;
  current: Sort;
  className: string;
  onSort: (sort: Sort) => void;
}) {
  const on = sort === current;
  return (
    <th className={className} aria-sort={on ? "descending" : "none"}>
      <button type="button" className="news-sort" onClick={() => onSort(sort)}>
        {label}
        {on && <Icon icon={ArrowDown01Icon} size="xs" />}
      </button>
    </th>
  );
}

/** Every sender with an unsubscribe link, across all inboxes, to leave or tidy away in one
 * click, or many at once. Senders without one aren't listed. */
export function Newsletters({
  accounts,
  place,
  searchRef,
  inboxFolder,
  onOpen,
  onNotice,
  onPhoneNav,
  signInNeeded = [],
}: {
  /** Inboxes whose sign-in has stopped working: they can't send unsubscribe emails. */
  signInNeeded?: number[];
  accounts: Account[];
  place?: NewsletterPlace;
  /** So the usual shortcut for searching lands here. */
  searchRef: React.RefObject<HTMLInputElement | null>;
  /** Where an inbox's mail is, for archiving a sender's. */
  inboxFolder: (accountId: number) => number | undefined;
  onOpen: (sender: NewsletterSender, from: NewsletterPlace) => void;
  onNotice: (notice: string) => void;
  onPhoneNav?: () => void;
}) {
  const { settings } = useSettings();
  const [rows, setRows] = useState(lastRows);
  const [problem, setProblem] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>(place?.tab ?? "subscribed");
  const [sort, setSort] = useState<Sort>(place?.sort ?? "latest");
  const [query, setQuery] = useState(place?.query ?? "");
  const [rowCount, setRowCount] = useState(place?.rows ?? ROWS_AT_A_TIME);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [alsoArchive, setAlsoArchive] = useState(true);
  /** What a big batch would do, waiting for a yes. */
  const [asking, setAsking] = useState(false);
  const [, redraw] = useState(0);
  const scroller = useRef<HTMLDivElement>(null);
  const scrollBack = useRef(place?.scroll ?? 0);
  inboxOf = inboxFolder;

  useEffect(() => {
    tell = onNotice;
    const load = () =>
      api
        .newsletters()
        .then((list) => {
          lastRows = list;
          setRows(list);
          setProblem(null);
        })
        .catch((e) => setProblem(say(e)));
    listChanged = (fromEngine) => {
      redraw((n) => n + 1);
      if (fromEngine) void load();
    };
    void load();
    return () => {
      listChanged = () => {};
    };
  }, [onNotice]);

  useLayoutEffect(() => {
    if (!rows || !scrollBack.current) return;
    if (scroller.current) scroller.current.scrollTop = scrollBack.current;
    scrollBack.current = 0;
  }, [rows]);

  const accountOf = useMemo(() => new Map(accounts.map((a) => [a.id, a])), [accounts]);
  const inboxName = (id: number) => {
    const a = accountOf.get(id);
    return a ? a.prefs.label || a.email : "";
  };

  const senders = bySender((rows ?? []).map(withLeaving));
  const subscribed = senders.filter((s) => !s.one.left);
  const unsubscribed = senders.filter((s) => s.one.left);
  const needle = query.trim().toLowerCase();
  const stuckOf = (x: Sender) => x.rows.map((r) => trouble.get(keyOf(r))).find(Boolean);
  const isStuck = (x: Sender) => !x.one.left && stuckOf(x) !== undefined;
  const pagesWaiting = subscribed.flatMap((x) =>
    x.rows.filter((r) => !r.left && trouble.get(keyOf(r))?.kind === "page"),
  );
  const openNext = () => {
    const row = pagesWaiting[0];
    if (!row) return;
    trouble.delete(keyOf(row));
    void send(row);
  };
  const letBack = (x: Sender) =>
    void Promise.all(x.rows.map((r) => api.newsletterForget(r.account_id, r.address)))
      .then(() => {
        onNotice(`${nameOf(x.one)} can reach your inbox again.`);
        listChanged(true);
      })
      .catch((e) => onNotice(say(e)));
  const matches = (tab === "subscribed" ? subscribed : unsubscribed)
    .filter(
      (s) =>
        !needle ||
        s.rows.some((r) =>
          [r.name, r.address, r.subject, inboxName(r.account_id)].some((text) => text.toLowerCase().includes(needle)),
        ),
    )
    .sort(
      (a, b) =>
        Number(isStuck(b)) - Number(isStuck(a)) ||
        (sort === "emails" ? b.one.threads - a.one.threads : 0) ||
        b.one.last_date - a.one.last_date,
    );
  const picking = tab === "subscribed";
  const chosen = matches.filter((s) => picked.has(s.key));

  const pick = (key: string, on: boolean) =>
    setPicked((now) => {
      const next = new Set(now);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });

  const flyRow = (x: Sender, count = 1) =>
    flyAway(document.querySelector(`.news-table tr[data-key="${CSS.escape(x.key)}"]`), settings.delete_effect ?? "sound", count);
  const leaveOne = (s: Sender) => {
    const ways = waysOut(s);
    if (ways.some((r) => r.how !== "page")) flyRow(s);
    leave(ways, `Unsubscribed from ${nameOf(s.one)}.`);
  };

  // Many at once only does what needs no page: those would fill the browser with tabs.
  const planFor = (archiveToo: boolean) => {
    const going = chosen.filter((s) => s.rows.some((r) => !r.left && (r.how === "one_click" || r.how === "mail")));
    const rowsGoing = going.flatMap((s) => s.rows.filter((r) => !r.left && (r.how === "one_click" || r.how === "mail")));
    const pages = chosen.filter((s) => !going.includes(s) && s.rows.some((r) => !r.left && r.how === "page"));
    const tidy = archiveToo ? chosen.filter((s) => going.includes(s) || s.one.how === "none") : [];
    const mails = rowsGoing.filter((r) => r.how === "mail").length;
    return { going, rowsGoing, pages, tidy, mails };
  };
  const leaveChosen = (sure = false) => {
    const { going, rowsGoing, pages, tidy } = planFor(alsoArchive);
    if (!sure && chosen.length > ASK_ABOVE && (rowsGoing.length > 0 || tidy.length > 0)) {
      setAsking(true);
      return;
    }
    const said = [
      going.length > 0 && `Unsubscribed from ${going.length === 1 ? nameOf(going[0].one) : plural(going.length, "sender")}.`,
      tidy.length > 0 && `Their emails are going to Archive.`,
    ]
      .filter(Boolean)
      .join(" ");
    if (going.length > 0) flyRow(going[0], going.length);
    if (rowsGoing.length > 0 || tidy.length > 0) leave(rowsGoing, said, tidy, true);
    if (pages.length > 0) onNotice(onlyByPage(pages.map((s) => nameOf(s.one))));
    setPicked(new Set(pages.map((s) => s.key)));
  };

  // All of these were left already, so archiving their mail is all there is to do.
  const archiveChosen = () => {
    for (const s of chosen) void archive(s, true);
    onNotice(`Moving emails from ${chosen.length === 1 ? nameOf(chosen[0].one) : plural(chosen.length, "sender")} to Archive.`);
    setPicked(new Set());
  };
  const canLeaveChosen = chosen.some((s) => s.rows.some((r) => !r.left && ["one_click", "mail", "page"].includes(r.how)));

  const startOver = () => {
    setRowCount(ROWS_AT_A_TIME);
    if (scroller.current) scroller.current.scrollTop = 0;
  };
  const showInbox = accounts.length > 1;
  const emptyTitle = needle
    ? "No senders found"
    : tab === "subscribed"
      ? "No newsletters"
      : "Nothing here yet";
  const emptyText =
    !needle && tab === "unsubscribed" ? "Senders you unsubscribe from show up here." : undefined;

  return (
    <>
    <main className="newsletters">
      <div className="list-head news-head" data-tauri-drag-region="deep">
        {onPhoneNav && <IconButton icon={Menu01Icon} label="Folders" small className="phone-only" onClick={onPhoneNav} />}
        <h2 className="list-title">Newsletters</h2>
        <div role="tablist" aria-label="Newsletters" className="tabs news-tabs">
          {(
            [
              ["subscribed", "Subscribed", subscribed.length],
              ["unsubscribed", "Unsubscribed", unsubscribed.length],
            ] as const
          ).map(([id, label, count]) => (
            <button
              key={id}
              type="button"
              role="tab"
              className="tab"
              aria-selected={tab === id}
              onClick={() => {
                setTab(id);
                setPicked(new Set());
                startOver();
              }}
            >
              {label}
              {count > 0 && <span className="tab-count tnum">{count.toLocaleString()}</span>}
            </button>
          ))}
        </div>
        <div className="search news-search">
          <Icon icon={Search01Icon} size="s" />
          <input
            ref={searchRef}
            type="search"
            aria-label="Search newsletters"
            placeholder="Search senders"
            value={query}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => {
              setQuery(e.target.value);
              startOver();
            }}
          />
        </div>
      </div>
      {picking && chosen.length > 0 && (
        <div className="news-bulk" role="toolbar" aria-label="Chosen senders">
          <span className="grow tnum">{plural(chosen.length, "sender")} chosen</span>
          {canLeaveChosen && (
            <label className="check-row">
              <input
                type="checkbox"
                className="check"
                checked={alsoArchive}
                onChange={(e) => setAlsoArchive(e.target.checked)}
              />
              <span>Also archive their emails</span>
            </label>
          )}
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => setPicked(new Set())}>
            Clear
          </button>
          {canLeaveChosen ? (
            <button type="button" className="btn btn-primary btn-sm" onClick={() => leaveChosen()}>
              Unsubscribe
            </button>
          ) : (
            <button type="button" className="btn btn-primary btn-sm" onClick={archiveChosen}>
              Archive their emails
            </button>
          )}
        </div>
      )}
      <Problem text={problem} />
      {tab === "subscribed" && pagesWaiting.length > 0 && (
        <div className="news-needs" role="status">
          <Icon icon={LinkSquare02Icon} size="s" />
          <span className="grow">
            {plural(pagesWaiting.length, "sender")} can only be left on their own website.
          </span>
          <button type="button" className="btn btn-secondary btn-sm" onClick={openNext}>
            Open next
          </button>
        </div>
      )}
      <div
        ref={scroller}
        className="list-scroll news-scroll"
        onScroll={(e) => {
          const el = e.currentTarget;
          if (el.scrollHeight - el.scrollTop - el.clientHeight < el.clientHeight && rowCount < matches.length) {
            setRowCount((n) => n + ROWS_AT_A_TIME);
          }
        }}
      >
        {rows && matches.length === 0 ? (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={News01Icon} size="l" />
            </div>
            <div className="empty-title">{emptyTitle}</div>
            {emptyText && <div className="empty-text">{emptyText}</div>}
          </div>
        ) : (
          rows && (
            <table className="news-table">
              <thead>
                <tr>
                  {picking && (
                    <th className="news-pick">
                      <input
                        type="checkbox"
                        className="check"
                        aria-label="Choose all"
                        checked={chosen.length > 0 && chosen.length === matches.length}
                        onChange={(e) => setPicked(e.target.checked ? new Set(matches.map((s) => s.key)) : new Set())}
                      />
                    </th>
                  )}
                  <th className="news-sender-col">Sender</th>
                  {showInbox && <th className="news-inbox">Inbox</th>}
                  <SortHead label="Emails" sort="emails" current={sort} className="news-count" onSort={setSort} />
                  <SortHead label="Latest email" sort="latest" current={sort} className="news-latest" onSort={setSort} />
                  <th className="news-action" aria-label="Unsubscribe" />
                </tr>
              </thead>
              <tbody>
                {matches.slice(0, rowCount).map((s) => {
                  const r = s.one;
                  const name = nameOf(r);
                  const account = accountOf.get(r.account_id);
                  const inbox = inboxName(r.account_id);
                  const open =
                    r.threads > 0
                      ? () =>
                          onOpen(
                            { account_id: r.account_id, addresses: s.rows.map((x) => x.address), name, folder_id: r.folder_id },
                            { tab, sort, query, rows: rowCount, scroll: scroller.current?.scrollTop ?? 0 },
                          )
                      : undefined;
                  const addresses =
                    s.rows.length > 1 ? `${s.rows.length} addresses at ${companyDomain(r.address)}` : r.name ? r.address : "";
                  return (
                    <tr key={s.key} className="news-row" data-key={s.key} data-opens={open ? "true" : undefined} onClick={open}>
                      {picking && (
                        <td className="news-pick" onClick={(e) => e.stopPropagation()}>
                          <input
                            type="checkbox"
                            className="check"
                            aria-label={`Choose ${name}`}
                            checked={picked.has(s.key)}
                            onChange={(e) => pick(s.key, e.target.checked)}
                          />
                        </td>
                      )}
                      <td>
                        {/* A click anywhere on the row opens it, and this one is for the keyboard. */}
                        <button type="button" className="news-sender" disabled={!open}>
                          <Avatar name={name} email={settings.list_pictures === "on" ? r.address : undefined} />
                          <span className="grow">
                            <span className="news-name truncate-1">{name}</span>
                            {addresses && <span className="news-sub truncate-1">{addresses}</span>}
                          </span>
                        </button>
                      </td>
                      {showInbox && (
                        <td className="news-inbox">
                          <span className="news-inbox-name">
                            {account && <AccountDot email={account.email} hue={accountHue(account)} />}
                            <span className="truncate-1">{inbox}</span>
                          </span>
                        </td>
                      )}
                      <td className="news-count tnum">{r.threads > 0 ? r.threads.toLocaleString() : ""}</td>
                      <td className="news-latest">
                        {r.last_date > 0 && (
                          <>
                            <span className="news-subject truncate-1">{r.subject || "(no subject)"}</span>
                            <span className="news-sub truncate-1">{shortDate(r.last_date)}</span>
                          </>
                        )}
                      </td>
                      <td className="news-action">
                        <span className="news-action-inner">
                          <Status row={r} inbox={inbox} stuck={r.left ? undefined : stuckOf(s)} onLeave={() => leaveOne(s)} onLetBack={() => letBack(s)} />
                          {r.threads > 0 && (
                            <span className="news-archive" onClick={(e) => e.stopPropagation()}>
                              <IconButton
                                icon={Archive02Icon}
                                label={`Archive emails from ${name}`}
                                tip="Moves their emails in the inbox to Archive"
                                small
                                onClick={() => void archive(s)}
                              />
                            </span>
                          )}
                        </span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )
        )}
      </div>
    </main>
    {asking && (() => {
      const { going, pages, mails, rowsGoing } = planFor(alsoArchive);
      const blocked = rowsGoing.filter((r) => r.how === "mail" && signInNeeded.includes(r.account_id));
      const blockedInboxes = [...new Set(blocked.map((r) => inboxName(r.account_id)))];
      const names = going.slice(0, 2).map((x) => nameOf(x.one));
      const more = going.length - names.length;
      return (
        <Dialog
          title={`Unsubscribe from ${plural(going.length, "sender")}?`}
          width="s"
          className="unsub-dialog"
          onClose={() => setAsking(false)}
          footer={
            <>
              <button type="button" className="btn btn-ghost" onClick={() => setAsking(false)}>
                Cancel
              </button>
              <button
                type="button"
                className="btn btn-primary"
                autoFocus
                onClick={() => {
                  setAsking(false);
                  leaveChosen(true);
                }}
              >
                Unsubscribe
              </button>
            </>
          }
        >
          <div className="unsub-who">
            <div className="unsub-faces" aria-hidden="true">
              {going.slice(0, 4).map((x) => (
                <Avatar
                  key={x.key}
                  size="s"
                  name={nameOf(x.one)}
                  email={settings.list_pictures === "on" ? x.one.address : undefined}
                />
              ))}
            </div>
            <span className="unsub-names truncate-1">
              {names.join(", ")}
              {more > 0 && ` and ${more.toLocaleString()} more`}
            </span>
          </div>
          <ul className="unsub-plan">
            <li>
              <span className="unsub-icon" data-tone="good">
                <Icon icon={MailRemove01Icon} size="s" />
              </span>
              <span className="unsub-line">
                <span className="unsub-line-title">They stop emailing you</span>
                <span className="unsub-line-help">
                  {mails > 0 ? "Some are told with a short email from your inbox." : "Done for you, right away."}
                </span>
              </span>
            </li>
            {blocked.length > 0 && (
              <li>
                <span className="unsub-icon" data-tone="warn">
                  <Icon icon={AlertCircleIcon} size="s" />
                </span>
                <span className="unsub-line">
                  <span className="unsub-line-title">Sign in to {blockedInboxes.join(" and ")} again</span>
                  <span className="unsub-line-help">
                    Until then, {plural(blocked.length, "unsubscribe email")} can't go. They'll show at the top to try again.
                  </span>
                </span>
              </li>
            )}
            {pages.length > 0 && (
              <li>
                <span className="unsub-icon" data-tone="warn">
                  <Icon icon={LinkSquare02Icon} size="s" />
                </span>
                <span className="unsub-line">
                  <span className="unsub-line-title">{plural(pages.length, "sender")} skipped</span>
                  <span className="unsub-line-help">They can only be left on their own website.</span>
                </span>
              </li>
            )}
          </ul>
          <label className="unsub-archive">
            <span className="unsub-icon">
              <Icon icon={Archive02Icon} size="s" />
            </span>
            <span className="grow">Also move their emails to Archive</span>
            <input
              type="checkbox"
              role="switch"
              className="toggle"
              aria-label="Also move their emails to Archive"
              checked={alsoArchive}
              onChange={(e) => setAlsoArchive(e.target.checked)}
            />
          </label>
        </Dialog>
      );
    })()}
    {/* Outside the panel's container, which would otherwise be what it centres on. */}
    {waiting && (
      <div role="status" className="toast toast-undo">
        <span className="grow">{waiting.what}</span>
        <button type="button" className="btn btn-secondary btn-sm" onClick={takeBack}>
          Undo
        </button>
        <IconButton icon={Cancel01Icon} label="Dismiss" small onClick={sendWaiting} />
      </div>
    )}
    </>
  );
}
