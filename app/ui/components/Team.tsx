import { FormEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ApiError, api, type TeamThreadView } from "../api";
import {
  Add01Icon,
  CheckmarkCircle02Icon,
  Delete02Icon,
  SentIcon,
  Tick02Icon,
  UserGroupIcon,
} from "../icons";
import { Avatar, Icon, IconButton, Menu, MenuItem, Problem, Spinner } from "./ui";
import { language, tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** Often enough that the server, which forgets after 25 seconds, keeps remembering. */
const SAY_IM_HERE_EVERY_MS = 10_000;

export interface TeamThreadRef {
  account_id: number;
  thread_id: string;
}

/** Everything about one shared conversation, kept fresh, and tells the team who's here. */
export function useTeamThread(thread: TeamThreadRef | null, tick: number, replying: boolean) {
  const [view, setView] = useState<TeamThreadView | null>(null);
  const account = thread?.account_id;
  const id = thread?.thread_id;

  const reload = useCallback(() => {
    if (account === undefined || id === undefined) return;
    api
      .teamThread(account, id)
      .then(setView)
      .catch(() => {});
  }, [account, id]);

  useEffect(reload, [reload, tick]);

  useEffect(() => {
    if (account === undefined || id === undefined) return;
    const kind = replying ? "replying" : "viewing";
    const beat = () => {
      // Someone writing a reply is still replying, even with the window out of sight.
      if (replying || !document.hidden) void api.teamPresence(account, id, kind).catch(() => {});
    };
    beat();
    const timer = setInterval(beat, SAY_IM_HERE_EVERY_MS);
    return () => clearInterval(timer);
  }, [account, id, replying]);

  useEffect(() => {
    if (account === undefined || id === undefined) return;
    return () => void api.teamPresence(account, id, "none").catch(() => {});
  }, [account, id]);

  return { view, reload };
}

const listOf = (people: string[]) =>
  people.length <= 1
    ? (people[0] ?? "")
    : new Intl.ListFormat(language, { type: "conjunction" }).format(people);

/** "Sam is replying", for putting right where it matters. */
export function whoIsReplying(view: TeamThreadView | null) {
  if (!view) return null;
  const people = [...view.replying];
  if (view.sending_by && !people.includes(view.sending_by)) people.push(view.sending_by);
  if (people.length === 0) return null;
  return people.length === 1 ? tx("{who} is replying", { who: listOf(people) }) : tx("{who} are replying", { who: listOf(people) });
}

export function TeamBar({
  thread,
  view,
  onChanged,
}: {
  thread: TeamThreadRef;
  view: TeamThreadView;
  onChanged: () => void;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setProblem(null);
    try {
      await action();
    } catch (e) {
      setProblem(say(e));
    }
    setBusy(false);
    onChanged();
  };

  const assign = (assignee: number | null) =>
    run(() => api.teamAssign(thread.account_id, thread.thread_id, view.assignee, assignee));
  const holder = view.members.find((m) => m.user_id === view.assignee);
  const holderName = !holder ? null : holder.user_id === view.me ? "You" : holder.email;
  const viewing = view.viewing.filter((p) => !view.replying.includes(p));

  // One chip says who has it; everything else you can do with it is in its menu.
  return (
    <div className="team-bar">
      <Menu
        label={tx("Assign")}
        trigger={(props) => (
          <button
            {...props}
            type="button"
            className="team-assign"
            data-done={view.done || undefined}
            disabled={busy}
            data-tip={tx("Who has this")}
          >
            {/* A round spot for whoever has it: an empty circle with a plus until someone does. */}
            <span className="team-assign-face" data-empty={(!holder && !view.done) || undefined}>
              {busy ? (
                <Spinner size="xs" />
              ) : view.done ? (
                <Icon icon={CheckmarkCircle02Icon} size="s" />
              ) : holder ? (
                <Avatar name={holder.email} size="s" />
              ) : (
                <Icon icon={Add01Icon} size="xs" />
              )}
            </span>
            <span className="truncate-1">{view.done ? tx("Done") : (holderName ?? tx("Assign"))}</span>
          </button>
        )}
      >
        {(close) => (
          <>
            {!holder && (
              <MenuItem
                leading={<Avatar name={view.members.find((m) => m.user_id === view.me)?.email ?? ""} size="xs" />}
                onClick={() => {
                  close();
                  void assign(view.me);
                }}
              >
                {tx("Take this")}</MenuItem>
            )}
            <MenuItem
              icon={CheckmarkCircle02Icon}
              onClick={() => {
                close();
                void run(() => api.teamSetStatus(thread.account_id, thread.thread_id, view.done ? "open" : "done"));
              }}
            >
              {view.done ? tx("Reopen") : tx("Mark done")}</MenuItem>
            <div className="menu-sep" />
            {view.members.map((m) => (
              <MenuItem
                key={m.user_id}
                leading={<Avatar name={m.email} size="xs" />}
                trailing={m.user_id === view.assignee ? <Icon icon={Tick02Icon} size="s" /> : undefined}
                onClick={() => {
                  close();
                  void assign(m.user_id);
                }}
              >
                {m.user_id === view.me ? `${m.email} (you)` : m.email}
              </MenuItem>
            ))}
            {holder && (
              <MenuItem
                icon={UserGroupIcon}
                onClick={() => {
                  close();
                  void assign(null);
                }}
              >
                {tx("Nobody yet")}</MenuItem>
            )}
          </>
        )}
      </Menu>
      {viewing.length > 0 && (
        <span className="team-presence truncate-1">
          <span className="team-dot" />
          {viewing.length === 1 ? tx("{who} is here too", { who: listOf(viewing) }) : tx("{who} are here too", { who: listOf(viewing) })}</span>
      )}
      <Problem text={problem} />
    </div>
  );
}

export function TeamNotes({
  thread,
  view,
  onChanged,
}: {
  thread: TeamThreadRef;
  view: TeamThreadView;
  onChanged: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // The box only opens when someone wants to write, so it doesn't take up every email.
  const [writing, setWriting] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    setWriting(false);
    setText("");
  }, [thread.account_id, thread.thread_id]);

  // As tall as what's written, up to the limit in the styles, and small again once posted.
  useLayoutEffect(() => {
    const area = box.current;
    if (!area) return;
    area.style.height = "auto";
    area.style.height = `${area.scrollHeight}px`;
  }, [text]);

  const typing = /(?:^|\s)@([^\s@]*)$/.exec(text);
  const suggestions = typing
    ? view.members.filter(
        (m) => m.user_id !== view.me && m.email.toLowerCase().startsWith(typing[1].toLowerCase()),
      )
    : [];

  const mention = (email: string) => {
    if (!typing) return;
    setText(`${text.slice(0, text.length - typing[1].length)}${email} `);
    box.current?.focus();
  };

  const post = async (e?: FormEvent) => {
    e?.preventDefault();
    const words = text.trim();
    if (!words || busy) return;
    setBusy(true);
    setProblem(null);
    const lower = words.toLowerCase();
    // The whole address, so @sam@x.co doesn't also mention sam@x.com.
    const mentions = view.members
      .filter((m) => {
        const mark = `@${m.email.toLowerCase()}`;
        return lower.split(/[\s,;:!?()]+/).some((w) => w.replace(/[.]+$/, "") === mark);
      })
      .map((m) => m.user_id);
    try {
      await api.teamComment(thread.account_id, thread.thread_id, words, mentions);
      // Anything typed since it was sent stays, and keeps the box open.
      setText((now) => {
        if (now.trim() !== words) return now;
        setWriting(false);
        return "";
      });
      onChanged();
    } catch (err) {
      setProblem(say(err));
    }
    setBusy(false);
  };

  const startWriting = () => {
    setWriting(true);
    requestAnimationFrame(() => box.current?.focus());
  };

  if (view.comments.length === 0 && !writing) {
    return (
      <button type="button" className="team-note-open" onClick={startWriting}>
        <Icon icon={UserGroupIcon} size="s" />
        {tx("Add a team note")}<span className="team-notes-private">{tx("Only your team sees it")}</span>
      </button>
    );
  }

  return (
    <section aria-label={tx("Team notes")} className="team-notes">
      <div className="team-notes-title">
        <Icon icon={UserGroupIcon} size="s" />
        <span className="grow">{tx("Team notes")}</span>
        <span className="team-notes-private">{tx("Only your team sees these")}</span>
      </div>
      {view.comments.map((c) => (
        <div key={c.id} className="team-note" data-note={c.id}>
          <Avatar name={c.author} size="xs" />
          <div className="grow">
            <div className="team-note-head">
              <span className="strong truncate-1">{c.mine ? "You" : c.author}</span>
              <span className="muted">{new Date(c.created_at * 1000).toLocaleString(language)}</span>
              {(c.mine || view.can_manage) && (
                <IconButton
                  icon={Delete02Icon}
                  label={tx("Delete note")}
                  onClick={() => {
                    api
                      .teamDeleteComment(c.id)
                      .then(onChanged)
                      .catch((e) => setProblem(say(e)));
                  }}
                />
              )}
            </div>
            <div className="team-note-text selectable">{c.text}</div>
          </div>
        </div>
      ))}
      {!writing && (
        <button type="button" className="btn btn-ghost btn-sm team-note-add" onClick={startWriting}>
          <Icon icon={Add01Icon} size="xs" />
          {tx("Add a note")}</button>
      )}
      {writing && (
      <form className="team-note-form" onSubmit={post}>
        <div className="team-note-field">
        <textarea
          ref={box}
          aria-label={tx("Write a note for your team")}
          placeholder={tx("Write a note for your team")}
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Shift+Enter starts a new line, and Enter while choosing Bangla or similar letters isn't sending.
            if (e.key === "Escape" && !text.trim()) {
              e.stopPropagation();
              setWriting(false);
              return;
            }
            if (e.key !== "Enter" || e.shiftKey || e.nativeEvent.isComposing) return;
            e.preventDefault();
            if (suggestions.length > 0) mention(suggestions[0].email);
            else void post();
          }}
        />
        <button
          type="submit"
          className="btn btn-primary btn-sm team-note-send"
          aria-label={tx("Add note")}
          disabled={busy || !text.trim()}
        >
          {busy ? <Spinner size="xs" /> : <Icon icon={SentIcon} size="s" />}
          {tx("Add note")}</button>
        </div>
        <div className="team-note-hint">
          <span className="grow">{tx("Enter adds the note · Shift+Enter starts a new line · @ mentions someone")}</span>
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => {
              setWriting(false);
              setText("");
            }}
          >
            {tx("Cancel")}</button>
        </div>
        {suggestions.length > 0 && (
          <div role="listbox" aria-label={tx("Mention someone")} className="team-suggest">
            {suggestions.map((m) => (
              <button key={m.user_id} type="button" role="option" aria-selected={false} onClick={() => mention(m.email)}>
                <Avatar name={m.email} size="xs" />
                <span className="truncate-1">{m.email}</span>
              </button>
            ))}
          </div>
        )}
      </form>
      )}
      <Problem text={problem} />
    </section>
  );
}
