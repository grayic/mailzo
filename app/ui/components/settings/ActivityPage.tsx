import { useEffect, useMemo, useState } from "react";
import { api, type AgentAction } from "../../api";
import {
  Archive02Icon,
  ArrowDown01Icon,
  ArrowRight01Icon,
  Attachment01Icon,
  Delete02Icon,
  FileEditIcon,
  Folder01Icon,
  FolderExportIcon,
  ListViewIcon,
  Mail01Icon,
  MailOpen01Icon,
  Search01Icon,
  SentIcon,
} from "../../icons";
import { Avatar, Confirm, Icon } from "../ui";
import { Choice, EmptyPage, Page } from "./controls";
import { language, tx } from "../../i18n";

const LOOK_AGAIN_MS = 5_000;
/** Actions this close together, by one app, were part of the same piece of work. */
const SAME_SESSION_SECONDS = 10 * 60;
const WEEK_SECONDS = 7 * 86_400;
/** This few can simply be shown, with no need to fold them away. */
const SHOWN_UNFOLDED = 2;

const KINDS = [
  { value: "all", label: tx("All activity") },
  { value: "reading", label: tx("Reading") },
  { value: "organizing", label: tx("Organizing") },
  { value: "sending", label: tx("Sending") },
  { value: "refused", label: tx("Refused") },
];

type Session = { key: number; app: string; actions: AgentAction[] };

const clockTime = (unix: number) =>
  new Date(unix * 1000).toLocaleTimeString(language, { hour: "numeric", minute: "2-digit" });
/** "a, b and c", the way the person's language says it. */
const together = (parts: string[]) => new Intl.ListFormat(language, { type: "conjunction" }).format(parts);

function dayName(unix: number) {
  const day = new Date(unix * 1000);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (day.toDateString() === today.toDateString()) return tx("Today");
  if (day.toDateString() === yesterday.toDateString()) return tx("Yesterday");
  return day.toLocaleDateString(language, { weekday: "long", month: "short", day: "numeric" });
}

function iconOf(a: AgentAction) {
  switch (a.tool) {
    case "search_mail":
      return Search01Icon;
    case "read_conversation":
      return MailOpen01Icon;
    case "read_attachment":
      return Attachment01Icon;
    case "list_folders":
    case "spam_or_trash":
      return Folder01Icon;
    case "list_drafts":
    case "save_draft":
      return FileEditIcon;
    case "mark_read":
      return a.detail?.read === false ? Mail01Icon : MailOpen01Icon;
    case "archive_conversation":
      return Archive02Icon;
    case "move_conversation":
      return FolderExportIcon;
    case "delete_conversation":
    case "delete_forever":
      return Delete02Icon;
    case "send_email":
      return SentIcon;
    default:
      return ListViewIcon;
  }
}

/** What happened in words, and what they tried to do when it didn't. */
function wordsFor(a: AgentAction): string {
  const done = a.outcome === "done";
  const say = (did: string, tried: string) => (done ? did : tried);
  const d = a.detail ?? {};
  const to = [d.to, d.cc && tx("cc {who}", { who: d.cc })].filter(Boolean).join(", ");
  const query = d.query ?? "";
  const folder = d.folder;
  switch (a.tool) {
    case "search_mail":
      return say(tx("Searched for “{query}”", { query }), tx("Tried to search for “{query}”", { query }));
    case "read_conversation":
      return say(tx("Read"), tx("Tried to read"));
    case "recent_mail":
      return say(tx("Looked at your newest mail"), tx("Tried to look at your newest mail"));
    case "list_inboxes":
      return say(tx("Listed your inboxes"), tx("Tried to list your inboxes"));
    case "list_folders":
      return say(tx("Listed your folders"), tx("Tried to list your folders"));
    case "list_drafts":
      return say(tx("Listed your drafts"), tx("Tried to list your drafts"));
    case "read_attachment":
      return say(tx("Opened an attachment"), tx("Tried to open an attachment"));
    case "spam_or_trash":
      return say(tx("Looked in spam or trash"), tx("Tried to look in spam or trash"));
    case "mark_read":
      return d.read === false
        ? say(tx("Marked unread"), tx("Tried to mark unread"))
        : say(tx("Marked read"), tx("Tried to mark read"));
    case "archive_conversation":
      return say(tx("Archived"), tx("Tried to archive"));
    case "move_conversation":
      return folder
        ? say(tx("Moved to {folder}", { folder }), tx("Tried to move to {folder}", { folder }))
        : say(tx("Moved to another folder"), tx("Tried to move to another folder"));
    case "delete_conversation":
      return say(tx("Moved to Trash"), tx("Tried to move to Trash"));
    case "delete_forever":
      return say(tx("Deleted for good"), tx("Tried to delete for good"));
    case "save_draft":
      return say(tx("Saved a draft to {to}", { to }), tx("Tried to save a draft to {to}", { to }));
    case "send_email":
      return d.needs_ok
        ? say(tx("Asked you to approve sending to {to}", { to }), tx("Tried to send to {to}", { to }))
        : say(tx("Sent to {to}", { to }), tx("Tried to send to {to}", { to }));
    default:
      return say(tx("Used your email"), tx("Tried to use your email"));
  }
}

/** "searched 5 times and read 3 conversations", for a stretch of reading. */
function readingInWords(actions: AgentAction[]): string {
  const searches = actions.filter((a) => a.tool === "search_mail").length;
  const reads = new Set(
    actions.filter((a) => a.tool === "read_conversation").map((a) => `${a.account_id}:${a.thread_id}`),
  ).size;
  const attachments = actions.filter((a) => a.tool === "read_attachment").length;
  const used = (tool: string) => actions.some((a) => a.tool === tool);
  const parts = [
    searches > 0 ? (searches === 1 ? tx("searched once") : tx("searched {n} times", { n: searches })) : "",
    reads > 0 ? (reads === 1 ? tx("read 1 conversation") : tx("read {n} conversations", { n: reads })) : "",
    attachments > 0
      ? attachments === 1
        ? tx("opened 1 attachment")
        : tx("opened {n} attachments", { n: attachments })
      : "",
    used("recent_mail") ? tx("looked at your newest mail") : "",
    used("spam_or_trash") ? tx("looked in spam or trash") : "",
    used("list_inboxes") ? tx("listed your inboxes") : "",
    used("list_folders") ? tx("listed your folders") : "",
    used("list_drafts") ? tx("listed your drafts") : "",
  ].filter(Boolean);
  return parts.length ? together(parts) : "";
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

/** A change is anything that left the mail different, which asking to send doesn't. */
const changedMail = (a: AgentAction) => a.kind !== "reading" && a.outcome === "done" && !a.detail?.needs_ok;

/** The week in a line or two, led by the thing a person most wants to know. */
function weekInWords(actions: AgentAction[]) {
  const since = Date.now() / 1000 - WEEK_SECONDS;
  const week = actions.filter((a) => a.at >= since);
  if (week.length === 0) return { headline: tx("No AI app has used your email this week."), detail: "" };
  const changes = week.filter(changedMail).length;
  const refused = week.filter((a) => a.outcome === "refused").length;
  const apps = [...new Set(week.map((a) => a.app))];
  const reading = apps
    .map((app) => {
      const did = readingInWords(week.filter((a) => a.app === app && a.kind === "reading" && a.outcome === "done"));
      return did ? tx("{app} {did}.", { app, did }) : "";
    })
    .filter(Boolean);
  if (refused > 0) {
    reading.push(refused === 1 ? tx("1 request was refused.") : tx("{n} requests were refused.", { n: refused }));
  }
  if (changes > 0) {
    return {
      headline:
        changes === 1
          ? tx("1 change made to your email this week.")
          : tx("{n} changes made to your email this week.", { n: changes }),
      detail: reading.join(" "),
    };
  }
  // Said as what did happen, as "nothing" reads wrong right above a list of what happened.
  const one = apps.length === 1 ? apps[0] : null;
  const onlyRead = week.some((a) => a.kind === "reading" && a.outcome === "done");
  const headline = onlyRead
    ? one
      ? tx("{app} only read your email this week.", { app: one })
      : tx("AI apps only read your email this week.")
    : one
      ? tx("{app} didn't change your email this week.", { app: one })
      : tx("AI apps didn't change your email this week.");
  return { headline, detail: [...reading, tx("Nothing was moved, deleted or sent.")].join(" ") };
}

function sessionsOf(actions: AgentAction[]): Session[] {
  const out: Session[] = [];
  for (const a of actions) {
    const last = out[out.length - 1];
    const previous = last?.actions[last.actions.length - 1];
    if (
      last &&
      previous &&
      last.app === a.app &&
      previous.at - a.at <= SAME_SESSION_SECONDS &&
      dayName(previous.at) === dayName(a.at)
    ) {
      last.actions.push(a);
    } else {
      out.push({ key: a.id, app: a.app, actions: [a] });
    }
  }
  return out;
}

/** The same thing done again is one line with a count, not a screenful of repeats. */
function withoutRepeats(actions: AgentAction[]) {
  const lines = new Map<string, { action: AgentAction; times: number }>();
  for (const a of actions) {
    const key = [a.tool, a.outcome, a.account_id, a.thread_id, JSON.stringify(a.detail)].join("|");
    const line = lines.get(key);
    if (line) line.times += 1;
    else lines.set(key, { action: a, times: 1 });
  }
  return [...lines.values()];
}

function ActionLine({
  action: a,
  times,
  onOpenThread,
}: {
  action: AgentAction;
  times: number;
  onOpenThread: (a: AgentAction) => void;
}) {
  const body = (
    <>
      <Icon icon={iconOf(a)} size="s" />
      <span className="activity-words">
        <span className="activity-verb">{wordsFor(a)}</span>
        {(a.detail?.subject || a.subject) && (
          <span className="activity-about truncate-1">
            <strong>{a.detail?.subject || a.subject}</strong>
            {a.who && <span className="muted"> {a.who}</span>}
          </span>
        )}
      </span>
      {times > 1 && <span className="muted tnum">×{times}</span>}
      {a.outcome === "refused" && (
        <span className="pill pill-risk" data-tip={a.problem ?? undefined}>
          {tx("Refused")}
        </span>
      )}
      {a.outcome === "failed" && (
        <span className="pill" data-tip={a.problem ?? undefined}>
          {tx("Didn't work")}
        </span>
      )}
      <span className="muted tnum activity-time">{clockTime(a.at)}</span>
    </>
  );
  // Only what's still there can be opened, so a failed try at something unknown is just words.
  if (a.account_id !== null && a.thread_id && a.subject) {
    return (
      <button type="button" className="activity-line" onClick={() => onOpenThread(a)}>
        {body}
      </button>
    );
  }
  return <div className="activity-line">{body}</div>;
}

function SessionCard({ session, onOpenThread }: { session: Session; onOpenThread: (a: AgentAction) => void }) {
  const [open, setOpen] = useState(false);
  const { actions, app } = session;
  const quiet = actions.filter((a) => a.kind === "reading" && a.outcome === "done");
  const quietLines = withoutRepeats(quiet);
  // Changes and refusals never get folded away. They're what this page is for.
  const standOut = withoutRepeats(actions.filter((a) => !quiet.includes(a)));
  const foldable = quietLines.length > SHOWN_UNFOLDED;
  const newest = actions[0].at;
  const oldest = actions[actions.length - 1].at;
  const started = clockTime(oldest);
  const ended = clockTime(newest);
  return (
    <div className="activity-session">
      <div className="activity-session-head">
        <Avatar name={app} size="xs" />
        <span className="activity-app">{app}</span>
        <span className="muted tnum activity-when">{started === ended ? ended : `${started} – ${ended}`}</span>
      </div>
      <div className="activity-lines">
        {standOut.map(({ action, times }) => (
          <ActionLine key={action.id} action={action} times={times} onOpenThread={onOpenThread} />
        ))}
        {foldable && (
          <button type="button" className="activity-fold" aria-expanded={open} onClick={() => setOpen(!open)}>
            <Icon icon={open ? ArrowDown01Icon : ArrowRight01Icon} size="xs" />
            {sentence(readingInWords(quiet))}
          </button>
        )}
        {(!foldable || open) &&
          quietLines.map(({ action, times }) => (
            <ActionLine key={action.id} action={action} times={times} onOpenThread={onOpenThread} />
          ))}
      </div>
    </div>
  );
}

/** Everything connected AI apps have done, with what matters most kept in plain sight. */
export function ActivityPage({ onOpenThread }: { onOpenThread: (a: AgentAction) => void }) {
  const [actions, setActions] = useState<AgentAction[] | null>(null);
  const [kind, setKind] = useState("all");
  const [app, setApp] = useState("all");
  const [clearing, setClearing] = useState(false);

  useEffect(() => {
    const load = () =>
      api
        .agentActivity()
        .then(setActions)
        .catch(() => setActions((current) => current ?? []));
    void load();
    const timer = setInterval(() => void load(), LOOK_AGAIN_MS);
    return () => clearInterval(timer);
  }, []);

  const apps = useMemo(() => [...new Set((actions ?? []).map((a) => a.app))], [actions]);
  const week = useMemo(() => weekInWords(actions ?? []), [actions]);
  const days = useMemo(() => {
    const shown = (actions ?? []).filter(
      (a) =>
        (app === "all" || a.app === app) &&
        (kind === "all" || (kind === "refused" ? a.outcome === "refused" : a.kind === kind)),
    );
    const out: { name: string; sessions: Session[] }[] = [];
    for (const session of sessionsOf(shown)) {
      const name = dayName(session.actions[0].at);
      const last = out[out.length - 1];
      if (last?.name === name) last.sessions.push(session);
      else out.push({ name, sessions: [session] });
    }
    return out;
  }, [actions, kind, app]);

  const intro = tx("What connected AI apps did in your email. Kept here for 90 days.");
  if (actions === null) return <Page title={tx("AI activity")} intro={intro}>{null}</Page>;
  if (actions.length === 0) {
    return (
      <Page title={tx("AI activity")} intro={intro}>
        <EmptyPage
          title={tx("Nothing yet")}
          text={tx("When a connected AI app uses your email, everything it does shows here.")}
        />
      </Page>
    );
  }

  return (
    <Page
      title={tx("AI activity")}
      intro={intro}
      action={
        <div className="activity-filters">
          {apps.length > 1 && (
            <Choice
              label={tx("Show which app")}
              value={app}
              onChange={setApp}
              options={[{ value: "all", label: tx("All apps") }, ...apps.map((a) => ({ value: a, label: a }))]}
            />
          )}
          <Choice label={tx("Show which kind")} value={kind} onChange={setKind} options={KINDS} />
        </div>
      }
    >
      <div className="activity-week">
        <div className="activity-headline">{week.headline}</div>
        {week.detail && <div className="muted">{week.detail}</div>}
      </div>
      {days.length === 0 && <EmptyPage title={tx("None of this kind")} text={tx("Choose All activity to see everything.")} />}
      {days.map((day) => (
        <section key={day.name}>
          <div className="section-title">{day.name}</div>
          {day.sessions.map((session) => (
            <SessionCard key={session.key} session={session} onOpenThread={onOpenThread} />
          ))}
        </section>
      ))}
      <div>
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => setClearing(true)}>
          {tx("Clear activity")}
        </button>
      </div>
      {clearing && (
        <Confirm
          title={tx("Clear all AI activity?")}
          confirm={tx("Clear")}
          keep={tx("Keep")}
          danger
          onKeep={() => setClearing(false)}
          onConfirm={() => {
            setClearing(false);
            setActions([]);
            void api.agentActivityClear();
          }}
        >
          {tx("This only clears the record here. It doesn't undo anything the apps did.")}
        </Confirm>
      )}
    </Page>
  );
}
