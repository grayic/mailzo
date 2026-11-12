import { useCallback, useEffect, useRef, useState } from "react";
import {
  ApiError,
  api,
  appLocked,
  finishBeforeRestart,
  type Finished,
  hearFromPanel,
  native,
  onBeforeRestart,
  onWindowFocus,
  tellMainWindow,
} from "../api";
import { fullDate, shortDate } from "../format";
import {
  Archive02Icon,
  ArrowLeft01Icon,
  Cancel01Icon,
  Delete02Icon,
  Mail01Icon,
  Mailbox01Icon,
  MailOpen01Icon,
  MailReply01Icon,
  PencilEdit02Icon,
  Settings02Icon,
} from "../icons";
import { SettingsProvider, useSettings } from "../settings";
import type { Account, Draft, Message, ThreadSummary } from "../types";
import { Composer } from "./Composer";
import { MessageCard } from "./Reader";
import { CodeChip, InboxTabs, TABS, ThreadRowBody, lead, threadKey } from "./ThreadList";
import { MARK_ALL_MOST, MoveToast, UNDO_MOVE_MS, markAllInRounds, refTo, unmarkInRounds, useWaitingMove, type Movable } from "./UndoMove";
import { Avatar, Confirm, Icon, IconButton, LogoMark, Problem, SendingToast, Spinner, Tooltips } from "./ui";
import { tx } from "../i18n";

const LOOK_AGAIN_MS = 10_000;
/** How long the panel waits, while put away, before catching up with changes. */
const PUT_AWAY_LOOK_MS = 6_000;
const COUNT_DOWN_MS = 250;
const READ_AFTER_MS = 3_000;

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const closePanel = () => void native("hide_this_window", {});
const openMainWindow = () => void native("show_main_window", {});

/** A conversation being read, once its emails have arrived. */
type Reading = { thread: ThreadSummary; messages: Message[] | null; seen: boolean };
/** A message being written. `thread` is the conversation it replies to, if any. */
type Writing = { draftId: number; thread: ThreadSummary | null; problem?: { text: string; code?: string } };
/** A message waiting out its undo time. */
type Waiting = { draftId: number; thread: ThreadSummary | null; from: number; sendAt: number };

/** The small window under the menu bar icon: only what's unread, ready to deal with. */
export function MenuBarPanel() {
  return (
    <SettingsProvider>
      <Tooltips />
      <Panel />
    </SettingsProvider>
  );
}

function Panel() {
  const { settings, reload: reloadSettings } = useSettings();
  const [signedIn, setSignedIn] = useState<boolean | null>(null);
  /** The app is locked: the engine shows no mail, so the panel says so. */
  const [locked, setLocked] = useState(false);
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  // The same tabs as the main window, starting on All.
  const [tab, setTab] = useState("all");
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const [counts, setCounts] = useState<Record<string, number>>({});
  const tabsShown = settings.smart_tabs === "on";
  const tabsShownRef = useRef(tabsShown);
  tabsShownRef.current = tabsShown;
  const [problem, setProblem] = useState<string | null>(null);
  /** What Mark all as read just cleared, for taking it back. */
  /** What Mark all as read did: the first round, to show again on Undo, and every one marked. */
  const [allRead, setAllRead] = useState<{ first: ThreadSummary[]; marked: Movable[] } | null>(null);
  const [askingAllRead, setAskingAllRead] = useState(false);
  const markingAll = useRef(false);
  const [reading, setReading] = useState<Reading | null>(null);
  const [writing, setWriting] = useState<Writing | null>(null);
  const [waiting, setWaiting] = useState<Waiting[]>([]);
  const [clock, setClock] = useState(Date.now());
  const [trusted, setTrusted] = useState<string[]>([]);
  // Messages opened and never touched leave nothing behind in Drafts.
  const blankDrafts = useRef(new Set<number>());
  const sendStarted = useRef(new Set<number>());
  const readTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const writingRef = useRef(writing);
  writingRef.current = writing;
  const readingRef = useRef(reading);
  readingRef.current = reading;
  const waitingRef = useRef(waiting);
  waitingRef.current = waiting;

  const moves = useWaitingMove((moved, why) => {
    if (why) setProblem(why);
    tellMainWindow("mail-changed", moved);
    void load();
  });
  const movesGone = moves.gone;
  /** The unread mail of every tab, from the last look. */
  const byTab = useRef<Record<string, ThreadSummary[]>>({});
  const tabList = useCallback(
    (id: string) => (byTab.current[id] ?? []).filter((t) => !movesGone.current.has(threadKey(t))),
    [movesGone],
  );
  // The store still counts mail waiting out its Undo, so each tab leaves those out as its list does.
  const shownCounts = Object.fromEntries(
    Object.entries(counts).map(([id, n]) => [
      id,
      Math.max(0, n - (byTab.current[id] ?? []).filter((t) => movesGone.current.has(threadKey(t))).length),
    ]),
  );

  const load = useCallback(async () => {
    // A look at the list part way through would bring back rows that are about to go.
    if (markingAll.current) return;
    try {
      // Mail shows once an inbox is connected, with or without a Mailzo account, but never
      // while signing in to one is half done.
      const state = (await api.authStatus()).state;
      const lockedNow = await appLocked();
      setLocked(lockedNow);
      const ready =
        !lockedNow && (state === "ready" || (state === "signed_out" && (await api.listAccounts()).length > 0));
      setSignedIn(ready);
      if (!ready) {
        setAccounts(null);
        setThreads([]);
        setReading(null);
        setWriting(null);
        return;
      }
      // Every tab loads at once, so switching to one shows it straight away.
      const ids = tabsShownRef.current ? TABS.map(([id]) => id) : ["all"];
      const [list, lists, tabCounts] = await Promise.all([
        api.listAccounts(),
        Promise.all(ids.map((id) => api.listAll("inbox", id === "all" ? undefined : id, 50, true))),
        tabsShownRef.current ? api.tabCounts(null, true) : Promise.resolve({}),
      ]);
      byTab.current = Object.fromEntries(ids.map((id, i) => [id, lists[i]]));
      setAccounts(list);
      setCounts(tabCounts);
      setThreads(tabList(tabsShownRef.current ? tabRef.current : "all"));
    } catch (e) {
      setProblem(say(e));
      setAccounts((now) => now ?? []);
    }
  }, [tabList]);

  const stopReading = useCallback(() => {
    if (readTimer.current) clearTimeout(readTimer.current);
    setReading(null);
    void load();
  }, [load]);

  // Fresh each time it opens. Clicking away puts it away, unless a message is being
  // written, as attaching a file opens a window of the Mac's own.
  useEffect(() => {
    const pressed = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || writingRef.current) return;
      if (readingRef.current) stopReading();
      else closePanel();
    };
    const stopWatching = onWindowFocus((focused) => {
      if (focused) {
        reloadSettings();
        void load();
      } else if (!writingRef.current) {
        stopReading();
        closePanel();
      }
    });
    window.addEventListener("keydown", pressed);
    const timer = setInterval(() => {
      if (document.hasFocus()) void load();
    }, LOOK_AGAIN_MS);
    // Put away, it catches up a little later and once for many changes, so it never slows
    // down the main window, above all as the app opens and every inbox comes in at once.
    let later: ReturnType<typeof setTimeout> | null = null;
    const loadSoon = () => {
      if (document.hasFocus()) {
        void load();
        return;
      }
      later ??= setTimeout(() => {
        later = null;
        void load();
      }, PUT_AWAY_LOOK_MS);
    };
    loadSoon();
    const stopHearing = hearFromPanel("sign-in-changed", () => void load());
    const stopHearingLock = hearFromPanel("lock-changed", () => void load());
    // So it's already up to date when it opens, rather than catching up as it does.
    const stopHearingMail = hearFromPanel("unread-changed", loadSoon);
    // New mail shows here as its banner does, not once the main window has caught up.
    const stopHearingNew = hearFromPanel("new-mail", loadSoon);
    return () => {
      if (later) clearTimeout(later);
      stopWatching();
      stopHearing();
      stopHearingLock();
      stopHearingMail();
      stopHearingNew();
      window.removeEventListener("keydown", pressed);
      clearInterval(timer);
    };
  }, [load, reloadSettings, stopReading]);

  useEffect(() => {
    void api
      .imageSenders()
      .then(setTrusted)
      .catch(() => {});
  }, []);

  // Changed on this Mac first, so the main window and the counts show it at once; the server after.
  const setSeen = (t: ThreadSummary, seen: boolean) =>
    api
      .markThreads([refTo(t)], seen ? "seen" : "unseen", () => tellMainWindow("mail-changed"))
      .then(({ problem: why }) => why && setProblem(why))
      .catch((e) => setProblem(say(e)));

  /** Takes these out of every tab's saved list too, so switching tabs can't bring them back
   * while the mail server is still catching up. */
  const forget = (gone: ThreadSummary[]) => {
    const keys = new Set(gone.map(threadKey));
    for (const id of Object.keys(byTab.current)) {
      byTab.current[id] = byTab.current[id].filter((t) => !keys.has(threadKey(t)));
    }
  };

  const markRead = (t: ThreadSummary) => {
    setProblem(null);
    forget([t]);
    setThreads((list) => list.filter((other) => threadKey(other) !== threadKey(t)));
    void setSeen(t, true).then(load);
  };



  const moveTo = (t: ThreadSummary, to: string, note?: string) => {
    setProblem(null);
    // Read while open, it's already left the counts.
    const unread = t.unread_count > 0 && !(readingRef.current?.thread === t && readingRef.current.seen);
    if (readTimer.current) clearTimeout(readTimer.current);
    setReading(null);
    setThreads((list) => list.filter((other) => threadKey(other) !== threadKey(t)));
    moves.move([refTo(t)], to, note);
    // The main window's counts, and the menu bar's, drop now rather than once Undo has had its time.
    if (unread) tellMainWindow("mail-leaving", [refTo(t)]);
  };

  /** Once its sign-in code is copied, an email goes where Settings says: the bin unless chosen otherwise. */
  const afterCodeCopied = (t: ThreadSummary) => {
    const after = settings.after_code_copy ?? "trash";
    if (after === "trash" || after === "archive") moveTo(t, after, tx("OTP copied."));
    else if (after === "read") markRead(t);
  };

  const read = async (t: ThreadSummary) => {
    setProblem(null);
    setReading({ thread: t, messages: null, seen: false });
    try {
      const messages = await api.getThread(t.account_id, t.thread_id, t.folder_id);
      setReading((now) => (now?.thread === t ? { ...now, messages } : now));
    } catch (e) {
      setProblem(say(e));
      setReading(null);
      return;
    }
    const markIt = () => {
      setReading((now) => (now?.thread === t ? { ...now, seen: true } : now));
      void setSeen(t, true);
    };
    if (settings.mark_read === "now") markIt();
    else if (settings.mark_read === "delay") readTimer.current = setTimeout(markIt, READ_AFTER_MS);
  };

  // Making a draft takes a moment, and a second click meanwhile would make a second one.
  const startingRef = useRef(false);
  const [starting, setStarting] = useState(false);

  const reply = async (t: ThreadSummary, mode?: Draft["mode"], message?: Message) => {
    // One being written already stays as it is, rather than being swapped out unseen.
    if (startingRef.current || writingRef.current) return;
    startingRef.current = true;
    setStarting(true);
    setProblem(null);
    try {
      const messages = message ? [message] : await api.getThread(t.account_id, t.thread_id, t.folder_id);
      const last = messages[messages.length - 1];
      const chosen = mode ?? (settings.reply_default === "reply_all" ? "reply_all" : "reply");
      const draft = await api.draftNew(t.account_id, chosen, last.id, fullDate(last.date));
      blankDrafts.current.add(draft.id);
      setWriting({ draftId: draft.id, thread: t });
    } catch (e) {
      setProblem(say(e));
    } finally {
      startingRef.current = false;
      setStarting(false);
    }
  };

  useEffect(() => {
    if (!allRead) return;
    const timer = setTimeout(() => setAllRead(null), UNDO_MOVE_MS);
    return () => clearTimeout(timer);
  }, [allRead]);

  const markAllRead = async () => {
    setProblem(null);
    setAllRead(null);
    const shown = threads;
    setThreads([]);
    markingAll.current = true;
    // More can be unread than this list shows, and all means all of them in this tab.
    const asked = tabsShown && tab !== "all" ? tab : undefined;
    // Every tab empties at once when it's all of them; otherwise this tab's mail leaves the others.
    // Their counts go with them, so nothing says there's mail left to read.
    if (asked === undefined) {
      for (const id of Object.keys(byTab.current)) byTab.current[id] = [];
      setCounts({});
    } else {
      forget(shown);
      setCounts((now) => ({ ...now, [asked]: 0 }));
    }
    // The main window, and the menu bar's number, show it now rather than once this Mac has it all.
    tellMainWindow("mail-read", { rows: shown.map(refTo), all: asked === undefined });
    const marked: Movable[] = [];
    let first: ThreadSummary[] = [];
    try {
      const done = await markAllInRounds(
        async () => {
          const round = await api.listAll("inbox", asked, MARK_ALL_MOST, true);
          forget(round);
          if (!first.length) first = round;
          return round;
        },
        () => tellMainWindow("mail-changed"),
        undefined,
        marked,
      );
      if (done.problem) setProblem(done.problem);
    } catch (e) {
      if (marked.length === 0) setThreads(shown);
      setProblem(say(e));
    }
    // Even after a problem partway, what was marked can be taken back.
    if (marked.length > 0) setAllRead({ first, marked });
    markingAll.current = false;
    tellMainWindow("mail-changed");
    void load();
  };

  const undoAllRead = async () => {
    const all = allRead;
    if (!all) return;
    setAllRead(null);
    setThreads(all.first);
    // Back on this tab straight away; the others catch up once the server has it.
    byTab.current[tabsShown ? tab : "all"] = all.first;
    try {
      await unmarkInRounds(all.marked, () => tellMainWindow("mail-changed"));
    } catch (e) {
      setProblem(say(e));
    }
    tellMainWindow("mail-changed");
    void load();
  };

  const newMessage = async () => {
    const from = accounts?.find((a) => a.prefs.default_for_new) ?? accounts?.[0];
    if (!from || startingRef.current || writingRef.current) return;
    startingRef.current = true;
    setStarting(true);
    setProblem(null);
    try {
      const draft = await api.draftNew(from.id, "new", null, null);
      blankDrafts.current.add(draft.id);
      setWriting({ draftId: draft.id, thread: null });
    } catch (e) {
      setProblem(say(e));
    } finally {
      startingRef.current = false;
      setStarting(false);
    }
  };

  /** What's left once a message has gone. A reply means the conversation has been dealt with. */
  const sent = (thread: ThreadSummary | null, warnings: string[]) => {
    setProblem(warnings[0] ?? null);
    if (thread) markRead(thread);
    else tellMainWindow("mail-changed");
  };

  /** Back to being written. If something else is being written, it waits in Drafts. */
  const bringBack = (w: Waiting, why?: Writing["problem"]) => {
    if (writingRef.current) {
      setProblem(why?.text ?? tx("Your message went back to Drafts."));
      tellMainWindow("mail-changed");
      return;
    }
    setWriting({ draftId: w.draftId, thread: w.thread, problem: why });
  };

  const sendNow = async (w: Waiting) => {
    if (sendStarted.current.has(w.draftId)) return;
    sendStarted.current.add(w.draftId);
    setWaiting((list) => list.filter((other) => other.draftId !== w.draftId));
    try {
      const { warnings } = await api.sendDraft(w.draftId);
      sent(w.thread, warnings);
    } catch (e) {
      // Nothing is lost. It goes back to being a draft, with the reason showing.
      sendStarted.current.delete(w.draftId);
      bringBack(w, { text: say(e), code: e instanceof ApiError ? e.code : undefined });
    }
  };
  const sendNowRef = useRef(sendNow);
  sendNowRef.current = sendNow;

  // Before an update restarts the app, messages still in their undo time go now and the
  // waiting move is made, as neither would survive the restart. The main window waits for the answer.
  const movesNowRef = useRef(moves.moveNow);
  movesNowRef.current = moves.moveNow;
  useEffect(() => {
    const stopFinishing = onBeforeRestart(() =>
      Promise.all([...waitingRef.current.map((w) => sendNowRef.current(w)), movesNowRef.current()]),
    );
    const stopHearing = hearFromPanel<string>("before-restart", (asked) => {
      void finishBeforeRestart()
        .catch((): Finished => "timeout")
        .then((finished) => tellMainWindow("ready-to-restart", { asked, finished }));
    });
    return () => {
      stopFinishing();
      stopHearing();
    };
  }, []);

  const hasWaiting = waiting.length > 0;
  useEffect(() => {
    if (!hasWaiting) return;
    const timer = setInterval(() => {
      const now = Date.now();
      setClock(now);
      waitingRef.current.filter((w) => w.sendAt <= now).forEach((w) => void sendNowRef.current(w));
    }, COUNT_DOWN_MS);
    return () => clearInterval(timer);
  }, [hasWaiting]);

  const queue = (draftId: number) => {
    const thread = writingRef.current?.thread ?? null;
    setWriting(null);
    setReading(null);
    setClock(Date.now());
    setWaiting((list) => [...list, { draftId, thread, from: Date.now(), sendAt: Date.now() + (Number(settings.undo_send) || 0) * 1000 }]);
  };

  const undo = (w: Waiting) => {
    setWaiting((list) => list.filter((other) => other.draftId !== w.draftId));
    bringBack(w);
  };

  const openInApp = (t: ThreadSummary) => {
    tellMainWindow("open-thread", t);
    openMainWindow();
  };

  const trustSender = (address: string, allowed: boolean) => {
    const key = address.toLowerCase();
    setTrusted((list) => (allowed ? [...list, key] : list.filter((a) => a !== key)));
    void api.setSenderImages(key, allowed).catch(() => {});
  };

  const stopWriting = () => {
    setWriting(null);
    void load();
  };

  if (signedIn === false && locked) {
    return (
      <div className="panel">
        <div className="list-scroll">
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={Mailbox01Icon} size="l" />
            </div>
            <div className="empty-title">{tx("Mailzo is locked")}</div>
            <div className="empty-text">{tx("Open Mailzo and unlock it to see your unread email here.")}</div>
          </div>
        </div>
        <footer className="panel-foot">
          <button className="btn btn-ghost btn-sm" onClick={openMainWindow}>
            <LogoMark />
            {tx("Open Mailzo")}</button>
        </footer>
      </div>
    );
  }

  if (signedIn === false) {
    return (
      <div className="panel">
        <div className="list-scroll">
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={Mailbox01Icon} size="l" />
            </div>
            <div className="empty-title">{tx("Connect your email")}</div>
            <div className="empty-text">{tx("Open Mailzo to connect an inbox, or to finish signing in, and your unread email shows up here.")}</div>
          </div>
        </div>
        <footer className="panel-foot">
          <button className="btn btn-ghost btn-sm" onClick={openMainWindow}>
            <LogoMark />
            {tx("Open Mailzo")}</button>
        </footer>
      </div>
    );
  }

  if (accounts === null) return <div className="panel" />;

  const showPictures = settings.list_pictures === "on";

  const composer = writing && (
    <Composer
      key={writing.draftId}
      inline={writing.thread !== null}
      draftId={writing.draftId}
      accounts={accounts}
      initialProblem={writing.problem}
      blankDrafts={blankDrafts.current}
      onClose={stopWriting}
      onInboxGone={stopWriting}
      onQueue={queue}
      onSent={(warnings) => {
        const thread = writing.thread;
        setWriting(null);
        setReading(null);
        sent(thread, warnings);
      }}
      onOpenSettings={openMainWindow}
      onScheduled={stopWriting}
      onLeft={() => tellMainWindow("mail-changed")}
    />
  );

  const toasts = (
    <>
      <Problem text={problem} />
      {allRead && (
        <div role="status" className="toast toast-undo">
          <span className="grow tnum">
            {allRead.marked.length === 1
              ? tx("1 conversation marked as read.")
              : tx("{n} conversations marked as read.", { n: allRead.marked.length })}
          </span>
          <button className="btn btn-secondary btn-sm" onClick={() => void undoAllRead()}>
            {tx("Undo")}
          </button>
          <IconButton icon={Cancel01Icon} label={tx("Dismiss")} small onClick={() => setAllRead(null)} />
        </div>
      )}
      {moves.waiting && (
        <MoveToast
          move={moves.waiting}
          onUndo={() => {
            tellMainWindow("mail-staying", moves.waiting?.threads ?? []);
            moves.undo();
            void load();
          }}
          onDismiss={() => void moves.moveNow()}
        />
      )}
      {waiting.map((w) => (
        <SendingToast
          key={w.draftId}
          from={w.from}
          sendAt={w.sendAt}
          clock={clock}
          onUndo={() => undo(w)}
          onSendNow={() => void sendNow(w)}
        />
      ))}
    </>
  );

  if (writing?.thread) {
    const t = writing.thread;
    return (
      <div className="panel">
        <div className="panel-bar">
          <IconButton icon={ArrowLeft01Icon} label={tx("Back")} small onClick={stopWriting} />
        </div>
        <div className="list-scroll panel-reply">
          <div className="panel-quote">
            <div className="panel-quote-from">
              <Avatar name={lead(t)} email={showPictures && !t.risk ? t.lead_address : undefined} size="s" />
              <span className="thread-from truncate-1 grow">{t.participants.join(", ") || tx("(unknown sender)")}</span>
              <span className="thread-time">{shortDate(t.last_date)}</span>
            </div>
            <div className="panel-quote-subject">{t.subject || tx("(no subject)")}</div>
            <div className="thread-snippet panel-quote-text">{t.ai_line || t.snippet}</div>
          </div>
          {composer}
        </div>
        {toasts}
      </div>
    );
  }

  if (reading) {
    const t = reading.thread;
    const messages = reading.messages;
    const toggleSeen = () => {
      const seen = !reading.seen;
      void setSeen(t, seen);
      stopReading();
    };
    return (
      <div className="panel">
        <div className="panel-bar">
          <IconButton icon={ArrowLeft01Icon} label={tx("Back")} small onClick={stopReading} />
          <span className="grow" />
          <IconButton
            icon={reading.seen ? Mail01Icon : MailOpen01Icon}
            label={reading.seen ? tx("Mark unread") : tx("Mark read")}
            small
            onClick={toggleSeen}
          />
          <IconButton icon={Archive02Icon} label={tx("Archive")} small onClick={() => moveTo(t, "archive")} />
          <IconButton icon={Delete02Icon} label={tx("Delete")} small onClick={() => moveTo(t, "trash")} />
        </div>
        <div className="list-scroll panel-reading">
          <h1 className="reader-subject selectable">{t.subject || tx("(no subject)")}</h1>
          {t.code && (
            <div className="hstack">
              <CodeChip code={t.code} onCopied={() => afterCodeCopied(t)} />
            </div>
          )}
          {messages === null ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : (
            messages.map((m, i) => (
              <MessageCard
                key={m.id}
                message={m}
                startOpen={i === messages.length - 1 || !m.seen}
                showPicture
                keyFacts={t.needs_you && i === messages.length - 1}
                trustedSender={trusted.includes(m.from.address.toLowerCase())}
                onTrustSender={m.from.address ? (allowed) => trustSender(m.from.address, allowed) : undefined}
                onCompose={(mode, message) => void reply(t, mode, message)}
                inJunk={false}
                onSpam={() => {
                  void api
                    .spamMark([{ account_id: t.account_id, thread_id: t.thread_id, folder_id: t.folder_id }], false)
                    .then(() => tellMainWindow("mail-changed"))
                    .catch((e) => setProblem(say(e)));
                  stopReading();
                }}
                onNotSpam={() => {
                  void api
                    .spamClear([{ account_id: t.account_id, thread_id: t.thread_id, folder_id: t.folder_id }])
                    .then(() => tellMainWindow("mail-changed"))
                    .catch((e) => setProblem(say(e)));
                  stopReading();
                }}
              />
            ))
          )}
        </div>
        <footer className="panel-foot">
          <button className="btn btn-ghost btn-sm" onClick={() => openInApp(t)}>
            <LogoMark />
            {tx("Open in Mailzo")}</button>
          <button className="btn btn-primary btn-sm" disabled={starting} onClick={() => void reply(t)}>
            {starting ? <Spinner size="xs" /> : <Icon icon={MailReply01Icon} size="s" />}
            {tx("Reply")}</button>
        </footer>
        {toasts}
      </div>
    );
  }

  return (
    <div className="panel">
      <div className="list-head">
        <h2 className="list-title grow">
          <button
            type="button"
            className="panel-title-link"
            aria-label={tx("Open unread email in Mailzo")}
            onClick={() => {
              tellMainWindow("show-unread");
              openMainWindow();
            }}
          >
            {tx("Unread")}
          </button>
        </h2>
        {threads.length > 0 && (
          <IconButton
            icon={MailOpen01Icon}
            label={tx("Mark all as read")}
            // Just the one needs no asking: it's what's on screen, and Undo is there.
            onClick={() => (threads.length === 1 ? void markAllRead() : setAskingAllRead(true))}
          />
        )}
        {accounts.length > 0 && (
          <button className="btn btn-primary btn-sm" disabled={starting} onClick={() => void newMessage()}>
            {starting ? <Spinner size="xs" /> : <Icon icon={PencilEdit02Icon} size="s" />}
            {tx("New message")}</button>
        )}
      </div>
      {tabsShown && accounts.length > 0 && (
        <InboxTabs
          tab={tab}
          counts={shownCounts}
          onPick={(next) => {
            tabRef.current = next;
            setTab(next);
            setThreads(tabList(next));
            void load();
          }}
        />
      )}
      <div className="list-scroll">
        {threads.length === 0 ? (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={Mailbox01Icon} size="l" />
            </div>
            <div className="empty-title">{accounts.length > 0 ? tx("You're all caught up") : tx("No inboxes yet")}</div>
            <div className="empty-text">
              {accounts.length > 0 ? tx("New email will show up here.") : tx("Open Mailzo to get started.")}
            </div>
          </div>
        ) : (
          threads.map((t) => {
            const what = t.subject || "conversation";
            const quick = (label: string, icon: typeof Archive02Icon, run: () => void) => (
              <span
                role="button"
                aria-label={`${label} ${what}`}
                data-tip={label}
                className="quick-action"
                onClick={(e) => {
                  e.stopPropagation();
                  run();
                }}
              >
                <Icon icon={icon} size="s" />
              </span>
            );
            return (
              <button key={threadKey(t)} className="thread thread-unread" onClick={() => void read(t)}>
                <ThreadRowBody
                  t={t}
                  showPictures={showPictures}
                  unreadBar={false}
                  onCopyCode={() => afterCodeCopied(t)}
                  quick={
                    <>
                      {quick("Reply", MailReply01Icon, () => void reply(t))}
                      {quick(tx("Mark as read"), MailOpen01Icon, () => markRead(t))}
                      {quick("Archive", Archive02Icon, () => moveTo(t, "archive"))}
                      {quick("Delete", Delete02Icon, () => moveTo(t, "trash"))}
                    </>
                  }
                />
              </button>
            );
          })
        )}
      </div>
      <footer className="panel-foot">
        <button className="btn btn-ghost btn-sm" onClick={openMainWindow}>
          <LogoMark />
          {tx("Open Mailzo")}</button>
        <IconButton
          icon={Settings02Icon}
          label={tx("Settings")}
          small
          onClick={() => {
            tellMainWindow("open-settings");
            openMainWindow();
          }}
        />
      </footer>
      {composer}
      {toasts}
      {askingAllRead && (
        <Confirm
          title={tx("Mark all as read?")}
          confirm={tx("Mark all as read")}
          keep={tx("Cancel")}
          onKeep={() => setAskingAllRead(false)}
          onConfirm={() => {
            setAskingAllRead(false);
            void markAllRead();
          }}
        >
          {tx("Every unread email here will be marked as read.")}</Confirm>
      )}
    </div>
  );
}
