import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  ApiError,
  api,
  native,
  type AuthStatus,
  type LaterCounts,
  type MailHealth,
  type PendingDevice,
  type SenderRow,
} from "./api";
import { fullDate, myLanguage, timeAgo } from "./format";
import { AddAccount } from "./components/AddAccount";
import { ApproveDevice } from "./components/ApproveDevice";
import { BulkPanel } from "./components/BulkPanel";
import { CalendarView } from "./components/CalendarView";
import { ContactsView, forgetContacts } from "./components/ContactsView";
import { forgetNewsletters, Newsletters, type NewsletterPlace } from "./components/Newsletters";
import { CommandBar, type Command } from "./components/CommandBar";
import { Composer } from "./components/Composer";
import { type AskAbout, AskChat } from "./components/AskChat";
import { Choice } from "./components/settings/controls";
import { AskAnswer, PickTime, ScheduledList, ScreenerList, SenderList, whenChoices } from "./components/Later";
import { MailBody } from "./components/MailBody";
import { McpSendApproval } from "./components/McpSendApproval";
import { Brief, RestBar } from "./components/Priority";
import { SearchBox, SearchFoot, SearchNote } from "./components/SearchBox";
import { SettingsWindow } from "./components/settings/SettingsWindow";
import { ThreadMenu, type ThreadAction } from "./components/ThreadMenu";
import { SignIn } from "./components/SignIn";
import { LockGate } from "./components/AppLock";
import { TodayStrip } from "./components/TodayStrip";
import { forgetTrustedSenders, Reader } from "./components/Reader";
import { Sidebar, unreadIn } from "./components/Sidebar";
import { clickedBanner, hearFromPanel, onBeforeRestart, printPage, tellPanel } from "./api";
import { DraftList, InboxTabs, TABS, TeamTabs, ThreadList, lead, threadKey } from "./components/ThreadList";
import { flyAway } from "./fly";
import { Confirm, Icon, IconButton, openSettings, SendingToast, Spinner, Tooltips } from "./components/ui";
import {
  AllReadToast,
  MARK_ALL_MOST,
  MoveToast,
  UNDO_MOVE_MS,
  canGoToInbox,
  comingTo,
  markAllInRounds,
  unmarkInRounds,
  useWaitingMove,
  type Movable,
} from "./components/UndoMove";
import { SettingsProvider, useSettings } from "./settings";
import { UpdateBar, useAutomaticUpdates } from "./components/Updates";
import { TagPicker, TagsContext } from "./tags";
import { AiChat02Icon, AlarmClockIcon, AlertCircleIcon, Calendar03Icon, News01Icon, ShoppingBag01Icon, Cancel01Icon, Clock01Icon, Tag01Icon, UserAdd01Icon, VolumeOffIcon, ContactBookIcon } from "./icons";
import { createPortal } from "react-dom";
import type { Account, Draft, Folder, Message, ReplyStart, SearchFilters, Tag, ThreadSummary } from "./types";
import { folderLabel, language, tx } from "./i18n";
import { reloadPaid, usePaid } from "./paid";

const REFRESH_EVERY_MS = 60_000;
/** Most looks are only at the inbox, as going through every folder can take most of a minute. */
const WHOLE_LOOK_EVERY = 5;
/** Says which inbox didn't refresh and why, in the server's own words. `raw` may start with
 * the folder it was on ("INBOX: Could not reach the mail server: the connection was lost"). */
const refreshProblem = (inbox: string, raw: string): string => {
  const reach = raw.match(/Could not reach the mail server: (.+)$/);
  if (reach) return tx("Couldn't reach the mail server for {0}: {1}", { "0": inbox, "1": reach[1] });
  return tx("Couldn't refresh {0}: {1}", { "0": inbox, "1": raw });
};
const MAIL_WAIT_GAP_MS = 3_000;
const MAIL_WAIT_RETRY_MS = 15_000;
const MAIL_WAIT_MOST_RETRY_MS = 5 * 60_000;
const TEAM_LOOK_AGAIN_MS = 15_000;
const TEAM_RETRY_MS = 5_000;
const TEAM_NEXT_LOOK_MS = 200;
const CHECK_DEVICES_EVERY_MS = 5_000;
const READ_AFTER_MS = 3_000;
const DUE_EVERY_MS = 30_000;
const MORE_MAIL_AFTER_MS = 1_500;
const SHOW_FIRST_LOOK_MS = 1_000;
const RAIL_KEY = "sidebar-rail";
/** How many places back and forward can go through. */
const TRAIL_MOST = 50;
/** Somewhere back and forward can return to: what was showing, and in which inbox. */
type Place = { view: View; account: number | null };
const HEALTH_EVERY_MS = 30_000;
/** Failed refreshes in a row before an inbox's problem is shown. */
const SAY_AFTER_FAILS = 3;
const LIST_PAGE = 200;
/** How long after a list shows before the other tabs are loaded ahead. */
const PREFETCH_AFTER_MS = 800;
/** How many conversations a search finds at first, as the engine gives. Scrolling asks for more. */
const FIRST_RESULTS = 300;
/** "Archive all" in The rest reads the whole tab this much at a time, the most the engine gives at once. */
const REST_PAGE = 5000;
/** And stops after this many, so a list that never ends can't keep it going. */
const REST_PAGES_MAX = 40;
/** An inbox this long without a good refresh, and failing, might be missing mail. */
// A day: a server busy for a while, or a Mac asleep or waiting on a prompt, isn't worth a
// warning, as other mail apps never show one. A whole day of failing is.
const STALE_AFTER_SECONDS = 24 * 60 * 60;

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/**
 * Shows the mail app as soon as an inbox is connected. A Mailzo account is optional: it's
 * only for using the same inboxes on other devices, and is offered from inside the app.
 */
export default function App() {
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  /** With no account, whether an inbox is connected yet, so the mail can show. */
  const [hasInbox, setHasInbox] = useState(false);
  /** Setting up an account from inside the app, with a way back to the mail. */
  const [settingUp, setSettingUp] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api
      .authStatus()
      .then(async (status) => {
        if (status.state === "signed_out") setHasInbox((await api.listAccounts()).length > 0);
        if (status.state === "ready") setSettingUp(false);
        setAuth(status);
      })
      .catch((e) => setStartupError(say(e)));
  }, []);

  useEffect(refresh, [refresh]);

  const signInState = auth?.state;
  useEffect(() => tellPanel("sign-in-changed"), [signInState]);

  if (startupError) {
    return (
      <div className="empty" role="alert">
        <div className="empty-icon">
          <Icon icon={AlertCircleIcon} size="l" />
        </div>
        <div className="empty-title">{tx("Mailzo couldn't start")}</div>
        <div className="empty-text">{startupError}</div>
      </div>
    );
  }
  if (!auth) return null;
  const thisDeviceOnly = auth.state === "signed_out" && hasInbox;
  return (
    <SettingsProvider>
      <Tooltips />
      {auth.state === "ready" || (thisDeviceOnly && !settingUp) ? (
        <LockGate>
          <Mail
            key={auth.device_id ?? "this-device"}
            auth={auth}
            onSignedOut={refresh}
            onSetUpAccount={() => setSettingUp(true)}
          />
        </LockGate>
      ) : (
        <SignIn
          status={auth}
          onChanged={refresh}
          onNotNow={thisDeviceOnly ? () => setSettingUp(false) : undefined}
        />
      )}
    </SettingsProvider>
  );
}

/** Which list is showing: every account's inbox, one folder, or unsent drafts. */
type View =
  | { kind: "all" }
  | { kind: "folder"; folderId: number }
  | { kind: "drafts" }
  | { kind: "search" }
  | { kind: "later"; which: "snoozed" | "reminders" | "muted" | "no_reply" | "deadlines" | "purchases" }
  | { kind: "scheduled" }
  | { kind: "screener" }
  | { kind: "tag"; tagId: number; name: string }
  | { kind: "calendar" }
  /** Chat with the inbox. */
  | { kind: "ask"; about?: AskAbout }
  | { kind: "newsletters"; place?: NewsletterPlace }
  | { kind: "contacts" }
  /** Everyone with mail in these folders, as one row each. */
  | { kind: "senders"; folderIds: number[]; title: string; back: View }
  /** One sender's mail, from one folder, or wherever it's filed in one inbox. */
  | {
      kind: "from";
      folderId: number;
      address: string;
      title: string;
      screened: boolean;
      /** All mail with them, as Newsletters and Contacts show it, rather than one folder's. */
      with?: { account: number | null; addresses: string[] };
      back: View;
    };

const LATER_TITLES = {
  snoozed: "Snoozed",
  reminders: "Reminders",
  muted: "Muted",
  no_reply: tx("No reply yet"),
  deadlines: "Deadlines",
  purchases: tx("Purchases"),
};
type ThreadRef = { account_id: number; thread_id: string };
/** How a row looks after a change. `null` takes it out of the list. */
type RowChange = (t: ThreadSummary) => ThreadSummary | null;
const withChange = (t: ThreadSummary, changes: Map<string, RowChange>) => {
  const change = changes.get(threadKey(t));
  return change ? (change(t) ?? []) : t;
};
/** What these look like straight away, without waiting to hear back. */
const MARKED: Record<string, RowChange> = {
  seen: (t) => ({ ...t, unread_count: 0 }),
  unseen: (t) => ({ ...t, unread_count: Math.max(1, t.message_count) }),
  star: (t) => ({ ...t, flagged: true }),
  unstar: (t) => ({ ...t, flagged: false }),
  delete_forever: () => null,
};
/** A new email or two coming in isn't a download worth mentioning. */
const DOWNLOAD_SHOWN_FROM = 100;
/** The views showing a list of conversations, which is what mail keys act on. */
const THREAD_VIEWS: View["kind"][] = ["all", "folder", "later", "from", "search", "tag"];
/** How long the last list may stay up while the next one comes. A quick answer never flickers. */
const LIST_WAIT_MS = 150;
/** When to start reading the other lists ahead of time, once the first screen is up. */
const READ_AHEAD_AFTER_MS = 2_000;
/** The room left between those reads, as everything else needs the database too. */
const READ_AHEAD_GAP_MS = 200;
/** While older mail downloads, rounds follow closely with rarely anything new to show. */
const SHOW_DOWNLOADS_EVERY_MS = 15_000;
/** Conversations kept in memory, opened or read ahead. About 20 KB each on a real inbox. */
const THREADS_KEPT = 300;
/** How many to read ahead from the top of a list, and on each side of the open one. */
const THREADS_AHEAD = 50;
const THREADS_BESIDE = 3;
/** Waits for the list to settle, so flicking through folders doesn't read each of them. */
const THREADS_AHEAD_AFTER_MS = 300;
const THREADS_AHEAD_GAP_MS = 20;
const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
const threadMemoryKey = (t: OpenThread) => `${threadKey(t)}:${t.folder_id}`;

/** Where a list is remembered, for the views that are worth going back to. */
function listMemoryKey(shown: View, tab: string | undefined, unread: boolean): string | null {
  const place =
    shown.kind === "all"
      ? "all"
      : shown.kind === "folder"
        ? `folder:${shown.folderId}`
        : shown.kind === "later"
          ? `later:${shown.which}`
          : shown.kind === "from"
            ? `from:${shown.folderId}:${shown.address}:${shown.screened}`
            : null;
  return place && `${place}|${tab ?? ""}|${unread}`;
}

const NO_SEARCH_NOTE = {
  summary: null as string | null,
  aiThinking: false,
  checking: false,
  problem: null as string | null,
  unreached: [] as string[],
  inBin: 0,
  byMeaning: 0,
};

const looksLikeASentence = (text: string) => text.trim().split(/\s+/).length >= 3 && !text.includes(":");

/** Sits behind everything on screen, and is all that shows on paper. */
function PrintSheet({ messages, subject, onDone }: { messages: Message[]; subject: string; onDone: () => void }) {
  useEffect(() => {
    document.documentElement.classList.add("printing");
    // The emails need a moment to lay out in their frames.
    const started = setTimeout(() => {
      void printPage().finally(() => setTimeout(onDone, 500));
    }, 400);
    return () => {
      clearTimeout(started);
      document.documentElement.classList.remove("printing");
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return createPortal(
    <div className="print-sheet">
      <h1>{subject || tx("(no subject)")}</h1>
      {messages.map((m) => (
        <article key={m.id}>
          <div className="print-head">
            <strong>{m.from.name ? `${m.from.name} <${m.from.address}>` : m.from.address}</strong>
            <span>{fullDate(m.date)}</span>
          </div>
          <div className="print-head">{tx("To")}{" "}{m.to.map((a) => a.name || a.address).join(", ")}</div>
          <MailBody html={m.body?.html ?? null} text={m.body?.text ?? null} />
        </article>
      ))}
    </div>,
    document.body,
  );
}
/** Typing that's stopped this long gets the slower search. */
const SEARCH_PAUSE_MS = 700;
const SEARCH_TYPING_MS = 120;

/** The conversation that's open. The same email sent to two inboxes has the same
 * thread name, so a conversation is only identified together with its inbox. */
type OpenThread = { account_id: number; thread_id: string; folder_id: number };

function Mail({
  auth,
  onSignedOut,
  onSetUpAccount,
}: {
  auth: AuthStatus;
  onSignedOut: () => void;
  /** Starts the optional Mailzo account, for using these inboxes on other devices too. */
  onSetUpAccount: () => void;
}) {
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  /** The account chosen in the dropdown. The first one until another is picked. */
  const [chosenAccount, setChosenAccount] = useState<number | null>(null);
  /** Places gone through, as a browser keeps them, each with the inbox the sidebar was showing.
   * A search isn't kept, as its results go with it. */
  const [trail, setTrail] = useState<{ back: Place[]; ahead: Place[] }>({ back: [], ahead: [] });
  const [foldersBy, setFoldersBy] = useState<Record<number, Folder[]>>({});
  const [view, setView] = useState<View>({ kind: "all" });
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [listLoading, setListLoading] = useState(false);
  const [unreadOnly, setUnreadOnly] = useState(false);
  /** What Mark all as read just cleared, for taking it back. */
  const [allRead, setAllRead] = useState<Movable[] | null>(null);
  /** How many Mark all as read has done so far, while it's going. */
  const [markingCount, setMarkingCount] = useState<number | null>(null);
  const markingAll = useRef(false);
  const unreadOnlyRef = useRef(false);
  const [open, setOpen] = useState<OpenThread | null>(null);
  const [messages, setMessages] = useState<Message[] | null>(null);
  const [adding, setAdding] = useState(false);
  /** The Settings page to go back to if connecting an inbox is cancelled. */
  const addingFromSettings = useRef<string | null>(null);
  const [rail, setRail] = useState(() => {
    try {
      return localStorage.getItem(RAIL_KEY) === "on";
    } catch {
      return false;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(RAIL_KEY, rail ? "on" : "off");
    } catch {
      // It just opens wide next time.
    }
  }, [rail]);
  /** Someone asked for mail to be checked. The regular checks go on without showing. */
  const [refreshing, setRefreshing] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingDevice[]>([]);
  const { settings, reload: reloadSettings, whenSaved } = useSettings();
  useAutomaticUpdates();
  /** The Settings page showing, or null when Settings is closed. */
  const [settingsPage, setSettingsPage] = useState<string | null>(null);
  openSettings.at = setSettingsPage;
  /** Inboxes whose browser sign-in has run out. */
  const [needsSignIn, setNeedsSignIn] = useState<number[]>([]);
  /** An email opened from an answer about search results, so it can go back to the answer. */
  const [fromAnswer, setFromAnswer] = useState(false);
  /** Inboxes waiting on a browser sign-in. */
  const [signingIn, setSigningIn] = useState<number[]>([]);
  /** The conversation a reply is being written under, if any. */
  const [inlineThread, setInlineThread] = useState<string | null>(null);
  const composingInline = useRef(inlineThread);
  composingInline.current = inlineThread;
  // Starts on the tab chosen in Settings, so the first list asked for is already the right one.
  const [tab, setTab] = useState(() =>
    settings.smart_tabs === "on" && TABS.some(([id]) => id === settings.open_tab) ? settings.open_tab : "all",
  );
  const tabRef = useRef(tab);
  const [teamTab, setTeamTab] = useState("team_open");
  const teamTabRef = useRef(teamTab);
  const [teamTick, setTeamTick] = useState(0);
  // Starts the wait for news again once someone has just joined a team.
  const [teamEpoch, setTeamEpoch] = useState(0);
  const [tabCounts, setTabCounts] = useState<Record<string, number>>({});
  const [searchText, setSearchText] = useState("");
  const [searchNote, setSearchNote] = useState(NO_SEARCH_NOTE);
  const [hasAiKey, setHasAiKey] = useState(false);
  // The brief, questions, AI sorting and the spam second opinion come with Mailzo Cloud.
  const paid = usePaid();
  /** What a reply opens with, from a one-tap answer or a reply already written. */
  const replyStart = useRef<ReplyStart | null>(null);
  /** Goes up when a one-tap answer is chosen for the reply already open. */
  const [startAgain, setStartAgain] = useState(0);
  const searchInput = useRef<HTMLInputElement | null>(null);
  /** What the list is showing results for, kept so a refresh can ask again. */
  const searchFilters = useRef<SearchFilters | null>(null);
  /** The one inbox a search looks in, or null for all of them. */
  const [searchIn, setSearchIn] = useState<number | null>(null);
  const searchExtra = useRef<number[]>([]);
  /** Goes up with each new search, so answers to old ones are ignored. */
  const searchRun = useRef(0);
  const searchTimers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const viewBeforeSearch = useRef<View>({ kind: "all" });
  /** The email that was right-clicked, and where. */
  const [rowMenu, setRowMenu] = useState<{ thread: ThreadSummary; x: number; y: number } | null>(null);
  /** Waiting for a yes before these are deleted for good. */
  const [deleting, setDeleting] = useState<ThreadSummary[] | null>(null);
  /** Sent messages waiting out the undo time. */
  const [waitingSends, setWaitingSends] = useState<{ draftId: number; from: number; sendAt: number }[]>([]);
  const [clock, setClock] = useState(() => Date.now());
  /** Each message is sent once, however the timers and buttons land. */
  const sendStarted = useRef(new Set<number>());
  const [reopenProblem, setReopenProblem] = useState<{ text: string; code?: string } | undefined>();
  const [composing, setComposing] = useState<number | null>(null);
  // Replies just opened and not touched yet, which leave nothing behind if abandoned.
  const blankDrafts = useRef(new Set<number>());
  const composingRef = useRef(composing);
  composingRef.current = composing;
  const [drafts, setDrafts] = useState<Draft[]>([]);
  /** Conversations ticked for acting on together. */
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const pickAnchor = useRef<string | null>(null);
  const readTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** How much of the list to load. Grows as the person scrolls. */
  const listSize = useRef(LIST_PAGE);
  const growingList = useRef(false);
  const listWait = useRef<ReturnType<typeof setTimeout> | null>(null);
  const listRun = useRef(0);
  /** What each list last showed, so going back is instant. A fresh copy follows unnoticed. */
  const listMemory = useRef(new Map<string, ThreadSummary[]>());
  /** Goes up whenever remembered lists change under a read, so a list asked for before then isn't kept. */
  const listGeneration = useRef(0);
  const countsMemory = useRef(new Map<string, Record<string, number>>());
  /** Each with the date its list row showed, as copies of one email in two folders can differ. */
  const threadMemory = useRef(new Map<string, { messages: Message[]; rowDate: number }>());
  const openRowDate = useRef(0);
  const threadsToRead = useRef<ThreadSummary[]>([]);
  const readingThreads = useRef(false);
  const [laterCounts, setLaterCounts] = useState<LaterCounts>({ snoozed: 0, reminders: 0, muted: 0, screener: 0 });
  const [health, setHealth] = useState<MailHealth | null>(null);
  const [senders, setSenders] = useState<SenderRow[]>([]);
  const [commandOpen, setCommandOpen] = useState(false);
  const [asking, setAsking] = useState<string | null>(null);
  const [printing, setPrinting] = useState<Message[] | null>(null);
  /** Choosing a time to come back to this conversation. */
  const [pickFor, setPickFor] = useState<{ thread: { account_id: number; thread_id: string }; kind: "snooze" | "follow" } | null>(null);
  const [findRequest, setFindRequest] = useState(0);
  /** On a phone there's room for one thing at a time, and the folders are one of them. */
  const [phoneNav, setPhoneNav] = useState(false);
  const moreMailTimers = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const retryTimers = useRef(new Set<ReturnType<typeof setTimeout>>());
  const noticeRef = useRef<string | null>(null);
  const syncNotice = useRef<string | null>(null);
  /** Changes on screen that aren't saved yet. A list read before they are mustn't undo them. */
  const rowChanges = useRef(new Map<string, RowChange>());
  const afterMove = useRef<(threads: Movable[], problem: string | null, to: string) => void>(() => {});
  const moves = useWaitingMove((threads, problem, to) => afterMove.current(threads, problem, to));
  const openingDraft = useRef(false);
  /** The row the open conversation was opened from, for its marks once it's no longer in the list. */
  const openRow = useRef<ThreadSummary | null>(null);
  const composeStarting = useRef(false);
  // Shown on the Reply button while the reply is being made, which can take a moment when
  // the app is busy, so a click never looks ignored.
  const [replyStarting, setReplyStarting] = useState(false);
  const addingRef = useRef(false);

  // Lets a finished refresh update whatever is showing by then.
  const viewRef = useRef(view);
  viewRef.current = view;
  const openRef = useRef(open);
  openRef.current = open;
  noticeRef.current = notice;
  addingRef.current = adding;
  const foldersRef = useRef(foldersBy);
  foldersRef.current = foldersBy;
  const accountsRef = useRef(accounts);
  accountsRef.current = accounts;

  // Sorted, as dragging inboxes into another order doesn't make them different inboxes.
  const accountIds = accounts?.map((a) => a.id).sort((x, y) => x - y).join(",") ?? "";
  const selected = accounts?.find((a) => a.id === chosenAccount) ?? accounts?.[0] ?? null;

  /** The tab a list is on. Tabs apply to inboxes only. */
  const tabOf = useCallback((shown: View) => {
    const folder =
      shown.kind === "folder"
        ? Object.values(foldersRef.current)
            .flat()
            .find((f) => f.id === shown.folderId)
        : undefined;
    const inboxFolder = folder?.role === "inbox";
    // A shared inbox is sorted by who has what, not by what kind of email it is.
    const teamView =
      inboxFolder && accountsRef.current?.find((a) => a.id === folder?.account_id)?.prefs.team_shared;
    const tabbed =
      !teamView && settingsRef.current.smart_tabs === "on" && (shown.kind === "all" || inboxFolder);
    return { tabbed, onTab: teamView ? teamTabRef.current : tabbed ? tabRef.current : undefined };
  }, []);

  /** A list as it should look now, whatever is on its way out of it or into it. */
  const withMovesShown = useCallback(
    (from: ThreadSummary[], shown: View) => {
      const list = from
        .filter((t) => !moves.gone.current.has(threadKey(t)))
        .flatMap((t) => withChange(t, rowChanges.current));
      const here =
        shown.kind === "folder"
          ? Object.values(foldersRef.current)
              .flat()
              .find((f) => f.id === shown.folderId)
          : undefined;
      const coming = here ? (comingTo(moves.gone.current, here) as ThreadSummary[]).filter((t) => t.subject !== undefined) : [];
      return coming.length > 0 ? [...coming, ...list].sort((a, b) => b.last_date - a.last_date) : list;
    },
    [moves.gone],
  );

  /** Counts on their way, so every inbox's folders coming in at once asks only once. True if
   *  they were asked for again meanwhile, and so are asked once more when these arrive. */
  const countsComing = useRef(new Map<string, boolean>());
  /** Goes up when a number is changed on screen ahead of the mail, so counts asked for before
   *  that can't put the old number back. The count asked for after it shows instead. */
  const countsChanged = useRef(0);
  const reloadCounts = useCallback((shown: View) => {
    const place = shown.kind === "folder" ? shown.folderId : null;
    const unread = unreadOnlyRef.current;
    const key = `${place}|${unread}`;
    if (countsComing.current.has(key)) {
      countsComing.current.set(key, true);
      return;
    }
    countsComing.current.set(key, false);
    const asked = countsChanged.current;
    api
      .tabCounts(place, unread)
      .then((counts) => {
        if (asked !== countsChanged.current) return;
        countsMemory.current.set(key, counts);
        if (viewRef.current === shown && unreadOnlyRef.current === unread) setTabCounts(counts);
      })
      .catch(() => {})
      .finally(() => {
        const again = countsComing.current.get(key);
        countsComing.current.delete(key);
        if (again && viewRef.current === shown) reloadCountsRef.current(shown);
      });
  }, []);
  const reloadCountsRef = useRef(reloadCounts);
  reloadCountsRef.current = reloadCounts;

  /** Tabs being loaded ahead, so the same one isn't asked for twice at once. */
  const prefetching = useRef(new Set<string>());
  const loadListNow = useCallback(async () => {
    const shown = viewRef.current;
    const unread = unreadOnlyRef.current;
    const generation = listGeneration.current;
    if (shown.kind === "drafts" || shown.kind === "scheduled" || shown.kind === "screener" || shown.kind === "calendar" || shown.kind === "ask" || shown.kind === "contacts" || shown.kind === "newsletters") return;
    if (shown.kind === "senders") {
      const lists = await Promise.all(shown.folderIds.map((id) => api.listSenders(id)));
      if (viewRef.current === shown) {
        setSenders(lists.flat().sort((a, b) => b.last_date - a.last_date));
      }
      return;
    }
    let list: ThreadSummary[];
    let askedTab: string | undefined;
    if (shown.kind === "later") {
      list = await api.listLater(shown.which);
    } else if (shown.kind === "tag") {
      list = await api.taggedThreads(shown.tagId);
    } else if (shown.kind === "from") {
      list = shown.with
        ? await api.mailWith(shown.with.account, shown.with.addresses)
        : await api.listFrom(shown.folderId, shown.address, shown.screened);
    } else if (shown.kind === "search") {
      if (!searchFilters.current) return;
      const run = searchRun.current;
      const reply = await api.searchMail({ filters: searchFilters.current }, searchExtra.current, listSize.current);
      if (run !== searchRun.current) return;
      list = reply.threads;
      setSearchNote((note) => ({ ...note, inBin: reply.in_bin }));
    } else {
      const { tabbed, onTab } = tabOf(shown);
      askedTab = onTab;
      const listedTab = onTab === "all" ? undefined : onTab;
      list =
        shown.kind === "all"
          ? await api.listAll("inbox", listedTab, listSize.current, unread)
          : await api.listThreads(shown.folderId, listedTab, listSize.current, unread);
      if (tabbed) reloadCounts(shown);
      // The other tabs load too, so switching to one shows it at once. One after another and
      // a moment later, so they don't slow down the list on screen, above all just after opening.
      if (tabbed && TABS.some(([id]) => id === onTab)) {
        const ahead = TABS.map(([id]) => id).filter((id) => {
          const key = listMemoryKey(shown, id, unread);
          return key && !listMemory.current.has(key) && !prefetching.current.has(key);
        });
        ahead.forEach((id) => prefetching.current.add(listMemoryKey(shown, id, unread)!));
        void (async () => {
          await new Promise((wait) => setTimeout(wait, PREFETCH_AFTER_MS));
          for (const id of ahead) {
            const key = listMemoryKey(shown, id, unread)!;
            try {
              if (generation !== listGeneration.current || listMemory.current.has(key)) continue;
              const other = id === "all" ? undefined : id;
              const found = await (shown.kind === "all"
                ? api.listAll("inbox", other, LIST_PAGE, unread)
                : api.listThreads(shown.folderId, other, LIST_PAGE, unread));
              // Thrown away or changed meanwhile, so this may show mail as it no longer is.
              if (generation === listGeneration.current && !listMemory.current.has(key)) {
                listMemory.current.set(key, found);
              }
            } catch {
              // Loaded when the tab is opened instead.
            } finally {
              prefetching.current.delete(key);
            }
          }
        })();
      }
    }
    // Kept even if they've moved on by now, for when they come back.
    const memoryKey = listMemoryKey(shown, askedTab, unread);
    if (memoryKey && generation === listGeneration.current) listMemory.current.set(memoryKey, list);
    // A look part way through marking everything read would bring those rows back for a moment.
    if (viewRef.current !== shown || markingAll.current) return;
    // "Unread only" was switched while this was on its way. The view stays the same, so only this tells.
    if (unread !== unreadOnlyRef.current) return;
    // Someone clicked another tab while this was on its way.
    if (askedTab && askedTab !== tabRef.current && askedTab !== teamTabRef.current) return;
    list = withMovesShown(list, shown);
    setThreads(list);
    // Anything no longer in the list can't stay selected.
    setPicked((current) => {
      if (current.size === 0) return current;
      const present = new Set(list.map(threadKey));
      const kept = [...current].filter((k) => present.has(k));
      return kept.length === current.size ? current : new Set(kept);
    });
    // New replies appear in the conversation that's open.
    // That includes one that has just left this tab, such as by being marked done.
    const current = openRef.current;
    if (current) {
      const fresh = await api.getThread(current.account_id, current.thread_id, current.folder_id);
      if (openRef.current === current && fresh.length > 0) setMessages(fresh);
    }
  }, [tabOf, withMovesShown, reloadCounts]);

  /** The list being loaded, and what it was for. */
  const listComing = useRef<{ what: string; done: Promise<void>; again: Promise<void> | null } | null>(null);
  /** Loads the list on screen. Asked again while the same list is on its way, as when every
   *  inbox finishes its refresh at once, it loads once more after that, not once for each. */
  const reloadList = useCallback((): Promise<void> => {
    const shown = viewRef.current;
    const what = JSON.stringify([
      shown,
      tabRef.current,
      teamTabRef.current,
      unreadOnlyRef.current,
      listSize.current,
      searchRun.current,
      listGeneration.current,
    ]);
    const coming = listComing.current;
    if (coming && coming.what === what) {
      coming.again ??= coming.done
        .catch(() => {})
        .then(() => {
          if (listComing.current === coming) listComing.current = null;
          return reloadListRef.current();
        });
      return coming.again;
    }
    const entry: { what: string; done: Promise<void>; again: Promise<void> | null } = {
      what,
      done: Promise.resolve(),
      again: null,
    };
    entry.done = loadListNow().finally(() => {
      if (listComing.current === entry && !entry.again) listComing.current = null;
    });
    listComing.current = entry;
    return entry.done;
  }, [loadListNow]);

  const reloadDrafts = useCallback(() => {
    api
      .draftList()
      .then(setDrafts)
      .catch(() => {});
  }, []);

  useEffect(reloadDrafts, [reloadDrafts]);

  /** Unread mail the menu bar panel moved and is still waiting out its Undo, with its folder.
   * The store counts it until then, so the counts here, and the menu bar's, leave it out. */
  const leavingFromPanel = useRef(new Map<string, number>());
  /** Moves folders' unread counts by these amounts, at once. */
  const shiftUnread = useCallback((by: Map<number, number>) => {
    if (by.size === 0) return;
    setFoldersBy((all) =>
      Object.fromEntries(
        Object.entries(all).map(([id, folders]) => [
          id,
          folders.map((f) => (by.has(f.id) ? { ...f, unread: Math.max(0, f.unread + (by.get(f.id) ?? 0)) } : f)),
        ]),
      ),
    );
  }, []);
  /** The panel moved these, or took the move back with Undo. */
  const panelMoved = useCallback(
    (rows: Movable[], leaving: boolean) => {
      const by = new Map<number, number>();
      for (const t of rows) {
        const key = threadKey(t);
        if (leavingFromPanel.current.has(key) === leaving) continue;
        if (leaving) leavingFromPanel.current.set(key, t.folder_id);
        else leavingFromPanel.current.delete(key);
        by.set(t.folder_id, (by.get(t.folder_id) ?? 0) + (leaving ? -1 : 1));
      }
      shiftUnread(by);
    },
    [shiftUnread],
  );

  /** Each inbox's folders on their way, and a second look asked for meanwhile. */
  const foldersComing = useRef(new Map<number, { done: Promise<void>; again: Promise<void> | null }>());
  const reloadFolders = useCallback((id: number): Promise<void> => {
    const coming = foldersComing.current.get(id);
    if (coming) {
      coming.again ??= coming.done
        .catch(() => {})
        .then(() => {
          if (foldersComing.current.get(id) === coming) foldersComing.current.delete(id);
          return reloadFoldersRef.current(id);
        });
      return coming.again;
    }
    const entry: { done: Promise<void>; again: Promise<void> | null } = { done: Promise.resolve(), again: null };
    entry.done = (async () => {
      const list = await api.listFolders(id);
      const leaving = [...leavingFromPanel.current.values()];
      setFoldersBy((all) => ({
        ...all,
        [id]: list.map((f) => ({ ...f, unread: Math.max(0, f.unread - leaving.filter((at) => at === f.id).length) })),
      }));
      const shown = viewRef.current;
      if (shown.kind === "all" || shown.kind === "folder") reloadCounts(shown);
    })().finally(() => {
      if (foldersComing.current.get(id) === entry && !entry.again) foldersComing.current.delete(id);
    });
    foldersComing.current.set(id, entry);
    return entry.done;
  }, [reloadCounts]);
  const reloadFoldersRef = useRef(reloadFolders);
  reloadFoldersRef.current = reloadFolders;

  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  // Mail from another inbox or grouped another way isn't what those lists would show now.
  useEffect(() => {
    listGeneration.current++;
    listMemory.current.clear();
    countsMemory.current.clear();
    threadMemory.current.clear();
    threadsToRead.current = [];
  }, [accountIds, settings.conversations]);

  /** A conversation as last read, unless its row shows it has new mail or was marked unread since. */
  const knownThread = useCallback((t: ThreadSummary) => {
    const known = threadMemory.current.get(threadMemoryKey(t));
    if (!known?.messages.length || known.rowDate < t.last_date) return undefined;
    const unread = known.messages.filter((m) => !m.seen).length;
    if (unread < t.unread_count) return undefined;
    // It may have been read just before being marked as read, and the row knows better.
    return t.unread_count === 0 && unread > 0 ? known.messages.map((m) => ({ ...m, seen: true })) : known.messages;
  }, []);

  const rememberThread = useCallback((t: OpenThread, messages: Message[], rowDate: number) => {
    const memory = threadMemory.current;
    const key = threadMemoryKey(t);
    // The oldest goes first, so one that's used again moves to the back.
    memory.delete(key);
    memory.set(key, { messages, rowDate });
    if (memory.size > THREADS_KEPT) memory.delete(memory.keys().next().value as string);
  }, []);

  /**
   * Remembered lists and conversations catch up with a change to these, so going back to a tab
   * never shows them as they were. "gone" takes them out of every list, "stale" forgets the lists
   * they were in so those are read again, and anything else changes their rows to match.
   */
  const rememberChange = useCallback(
    (chosen: { account_id: number; thread_id: string }[], change: "gone" | "stale" | Partial<ThreadSummary>) => {
      const keys = new Set(chosen.map(threadKey));
      listGeneration.current++;
      listMemory.current.forEach((list, key) => {
        // Snoozed, muted and the other later lists may gain them, which only a fresh read finds.
        if (key.startsWith("later:")) {
          listMemory.current.delete(key);
          return;
        }
        if (!list.some((t) => keys.has(threadKey(t)))) return;
        if (change === "gone") {
          listMemory.current.set(key, list.filter((t) => !keys.has(threadKey(t))));
        } else if (change === "stale" || ("unread_count" in change && key.endsWith("|true"))) {
          listMemory.current.delete(key);
        } else {
          listMemory.current.set(key, list.map((t) => (keys.has(threadKey(t)) ? { ...t, ...change } : t)));
        }
      });
      threadMemory.current.forEach((_, key) => {
        if (keys.has(key.slice(0, key.lastIndexOf(":")))) threadMemory.current.delete(key);
      });
    },
    [],
  );

  /** Every remembered list goes, for a change that reaches more mail than can be told apart here. */
  const forgetLists = useCallback(() => {
    listGeneration.current++;
    listMemory.current.clear();
  }, []);

  /** Reads these before anything still waiting, one at a time. */
  const readThreadsAhead = useCallback(
    (rows: ThreadSummary[]) => {
      threadsToRead.current = [...rows, ...threadsToRead.current].slice(0, THREADS_KEPT);
      if (readingThreads.current) return;
      readingThreads.current = true;
      void (async () => {
        for (let t = threadsToRead.current.shift(); t; t = threadsToRead.current.shift()) {
          if (knownThread(t)) continue;
          const list = await api.getThread(t.account_id, t.thread_id, t.folder_id).catch(() => null);
          if (list) rememberThread(t, list, t.last_date);
          await pause(THREADS_AHEAD_GAP_MS);
        }
        readingThreads.current = false;
      })();
    },
    [knownThread, rememberThread],
  );

  useEffect(() => {
    const timer = setTimeout(() => readThreadsAhead(threads.slice(0, THREADS_AHEAD)), THREADS_AHEAD_AFTER_MS);
    return () => clearTimeout(timer);
  }, [threads, readThreadsAhead]);

  useEffect(() => {
    if (open && messages) rememberThread(open, messages, openRowDate.current);
  }, [open, messages, rememberThread]);

  // Reads ahead, so even a first visit to a folder or tab is instant.
  const sidebarFolders = selected ? foldersBy[selected.id] : undefined;
  const sidebarFolderIds = sidebarFolders?.map((f) => f.id).join(",") ?? "";
  useEffect(() => {
    if (!sidebarFolderIds) return;
    let live = true;
    void (async () => {
      await pause(READ_AHEAD_AFTER_MS);
      const everyTab: (string | undefined)[] =
        settingsRef.current.smart_tabs === "on" ? TABS.map(([id]) => id) : [undefined];
      const folders = sidebarFolderIds.split(",").map(Number);
      const reads: [View, string | undefined][] = [
        ...everyTab.map((tab): [View, string | undefined] => [{ kind: "all" }, tab]),
        ...folders.flatMap((folderId): [View, string | undefined][] => {
          const shown: View = { kind: "folder", folderId };
          const { tabbed, onTab } = tabOf(shown);
          // A shared inbox's tabs are about who has what, and change as the team works.
          if (onTab !== undefined && !tabbed) return [];
          return tabbed ? everyTab.map((tab) => [shown, tab]) : [[shown, undefined]];
        }),
      ];
      for (const [shown, tab] of reads) {
        if (!live) return;
        const key = listMemoryKey(shown, tab, false);
        if (!key || listMemory.current.has(key)) continue;
        const listedTab = tab === "all" ? undefined : tab;
        const generation = listGeneration.current;
        try {
          const list =
            shown.kind === "folder"
              ? await api.listThreads(shown.folderId, listedTab, LIST_PAGE)
              : await api.listAll("inbox", listedTab, LIST_PAGE);
          // A refresh that got here first is newer than this.
          if (live && generation === listGeneration.current && !listMemory.current.has(key)) {
            listMemory.current.set(key, list);
          }
          if (tab === everyTab[0] && tab !== undefined) {
            const place = shown.kind === "folder" ? shown.folderId : null;
            const counts = await api.tabCounts(place);
            if (live) countsMemory.current.set(`${place}|false`, counts);
          }
        } catch {
          return;
        }
        await pause(READ_AHEAD_GAP_MS);
      }
    })();
    return () => {
      live = false;
    };
  }, [sidebarFolderIds, settings.conversations, tabOf]);

  // This looks through all the mail, which takes seconds on a big mailbox, so it's asked rarely.
  const healthAsked = useRef(0);
  /** `now` skips the wait between looks, for when something just changed, like a new inbox. */
  const reloadHealth = useCallback(async (now = false) => {
    if (!now && Date.now() - healthAsked.current < HEALTH_EVERY_MS / 2) return;
    healthAsked.current = Date.now();
    try {
      setHealth(await api.mailHealth());
    } catch {
      // It only adds a note. The mail itself isn't affected.
    }
  }, []);

  const reloadLater = useCallback(() => {
    api
      .laterCounts()
      .then(setLaterCounts)
      .catch(() => {});
  }, []);

  const [tags, setTags] = useState<Tag[]>([]);
  const [taggingOpen, setTaggingOpen] = useState(false);
  const reloadTags = useCallback(() => {
    api
      .tagsList()
      .then(setTags)
      .catch(() => {});
  }, []);
  useEffect(reloadTags, [reloadTags]);

  useEffect(() => {
    reloadLater();
    void reloadHealth();
    const timer = setInterval(() => void reloadHealth(), HEALTH_EVERY_MS);
    return () => clearInterval(timer);
  }, [reloadLater, reloadHealth]);

  const quietNow = () => {
    const s = settingsRef.current;
    const hour = new Date().getHours();
    const [from, until] = [Number(s.quiet_start), Number(s.quiet_end)];
    return (
      s.quiet_hours === "on" && (from <= until ? hour >= from && hour < until : hour >= from || hour < until)
    );
  };

  const lastShownAt = useRef(new Map<number, number>());
  /** Refreshes in a row that went wrong, by inbox. */
  const failedInARow = useRef(new Map<number, number>());
  const inboxName = (id: number) => {
    const a = accountsRef.current?.find((x) => x.id === id);
    return a ? a.prefs.label || a.email : tx("an inbox");
  };
  const syncOne = useCallback(
    async (id: number, downloading: boolean, inboxOnly = false): Promise<string | null> => {
      let problem: string | null = null;
      let nothingToShow = false;
      let showing: ReturnType<typeof setInterval> | null = null;
      try {
        // A new inbox's first download can take minutes, so its mail shows as it arrives.
        if ((await api.listFolders(id)).length === 0) {
          showing = setInterval(() => void reloadFolders(id).then(reloadList).catch(() => {}), SHOW_FIRST_LOOK_MS);
        }
        // The engine has no idea what time it is where the person is.
        const s = settingsRef.current;
        const hour = new Date().getHours();
        const [from, until] = [Number(s.quiet_start), Number(s.quiet_end)];
        const quiet =
          s.quiet_hours === "on" &&
          (from <= until ? hour >= from && hour < until : hour >= from || hour < until);
        const stats = await api.sync(id, quiet, inboxOnly);
        // Older mail is still coming down, so this carries on straight away.
        if (stats.more_to_download && !moreMailTimers.current.has(id)) {
          moreMailTimers.current.set(
            id,
            setTimeout(() => {
              moreMailTimers.current.delete(id);
              if (accountsRef.current?.some((a) => a.id === id)) void syncRef.current(id, true);
            }, MORE_MAIL_AFTER_MS),
          );
        }
        // One was already running and may have passed a folder that just changed.
        if (stats.already_running) {
          await new Promise<void>((go) => {
            const retry = setTimeout(() => {
              retryTimers.current.delete(retry);
              go();
            }, 2000);
            retryTimers.current.add(retry);
          });
          return accountsRef.current?.some((a) => a.id === id) ? await syncOne(id, downloading) : null;
        }
        if (stats.errors.length) {
          problem = refreshProblem(inboxName(id), stats.errors[0]);
        }
        void reloadHealth();
        setNeedsSignIn((list) => list.filter((x) => x !== id));
        // A refresh that changed nothing leaves the lists and counts as they are.
        const changedNothing =
          stats.new_messages === 0 &&
          stats.removed_messages === 0 &&
          stats.flags_changed === 0 &&
          stats.filtered === 0 &&
          stats.older_saved === 0 &&
          stats.bodies_fetched === 0;
        nothingToShow =
          changedNothing ||
          (downloading &&
            stats.new_messages === 0 &&
            stats.removed_messages === 0 &&
            Date.now() - (lastShownAt.current.get(id) ?? 0) < SHOW_DOWNLOADS_EVERY_MS);
      } catch (e) {
        // A failed look brought nothing new.
        nothingToShow = true;
        if (e instanceof ApiError && e.code === "needs_signin") {
          // This gets its own note with a button, not a passing message.
          setNeedsSignIn((list) => (list.includes(id) ? list : [...list, id]));
        } else {
          // Saved mail is still readable, so this is a notice and not a stop.
          problem = refreshProblem(inboxName(id), say(e));
        }
      } finally {
        if (showing) clearInterval(showing);
      }
      if (nothingToShow) return problem;
      lastShownAt.current.set(id, Date.now());
      try {
        await reloadFolders(id);
        await reloadList();
        // New mail may have come from someone new, or brought something back.
        reloadLater();
      } catch (e) {
        problem = say(e);
      }
      return problem;
    },
    [reloadFolders, reloadList, reloadLater, reloadHealth],
  );

  /** Refreshes one inbox, or all of them, and reports any problem once. */
  const sync = useCallback(
    /** `asked`: the person pressed refresh, so a problem is said straight away. */
    async (id?: number, downloading = false, inboxOnly = false, asked = false) => {
      const ids =
        id !== undefined ? [id] : (accountsRef.current ?? []).map((a) => a.id);
      const results = await Promise.all(ids.map((one) => syncOne(one, downloading, inboxOnly)));
      // A one-off hiccup, like a server slow to answer once, isn't worth a word: an inbox says
      // so after failing a few refreshes in a row. One not refreshing for long has its own note.
      const said = results.map((problem, i) => {
        const one = ids[i];
        if (!problem) {
          failedInARow.current.delete(one);
          return null;
        }
        const times = (failedInARow.current.get(one) ?? 0) + 1;
        failedInARow.current.set(one, times);
        return asked || times >= SAY_AFTER_FAILS ? problem : null;
      });
      const found = said.find((r) => r) ?? null;
      // Refreshing only clears what refreshing said, not a message about something else.
      if (found || noticeRef.current === syncNotice.current) setNotice(found);
      syncNotice.current = found;
    },
    [syncOne],
  );

  useEffect(
    () => () => {
      moreMailTimers.current.forEach((t) => clearTimeout(t));
      moreMailTimers.current.clear();
      retryTimers.current.forEach((t) => clearTimeout(t));
      retryTimers.current.clear();
    },
    [],
  );

  const syncRef = useRef(sync);
  syncRef.current = sync;

  /** The inbox is what someone's looking for, so the button only waits for that. The other
   * folders follow quietly, as going through them all takes a while. */
  const refreshNow = () => {
    setRefreshing(true);
    void sync(undefined, false, true, true).finally(() => {
      setRefreshing(false);
      void sync();
    });
  };

  useEffect(() => {
    api
      .listAccounts()
      .then(setAccounts)
      .catch((e) => {
        setAccounts([]);
        setNotice(say(e));
      });
  }, []);

  /** Picks up inboxes connected or disconnected on other devices. */
  const syncInboxList = useCallback(async () => {
    try {
      const { changed } = await api.vaultSync();
      if (!changed) return;
      setAccounts(await api.listAccounts());
      reloadSettings();
    } catch (e) {
      if (e instanceof ApiError && e.code === "session_revoked") onSignedOut();
      // Otherwise it is only offline, and saved mail still works.
    }
  }, [onSignedOut, reloadSettings]);

  useEffect(() => {
    void syncInboxList();
    const timer = setInterval(() => void syncInboxList(), REFRESH_EVERY_MS);
    return () => clearInterval(timer);
  }, [syncInboxList]);

  // Watches for a new device asking to join, or this one having been removed.
  useEffect(() => {
    const check = () =>
      api
        .pendingDevices()
        .then(setPending)
        .catch((e) => {
          if (e instanceof ApiError && e.code === "session_revoked") onSignedOut();
        });
    void check();
    const timer = setInterval(check, CHECK_DEVICES_EVERY_MS);
    return () => clearInterval(timer);
  }, [onSignedOut]);

  const clearOpen = () => {
    openRef.current = null;
    setOpen(null);
    setMessages(null);
  };

  // Shows saved mail at once, then keeps every inbox up to date, not only the
  // one on screen. Runs again when an inbox is connected or disconnected.
  useEffect(() => {
    if (!accountIds) {
      setFoldersBy({});
      setThreads([]);
      return;
    }
    const ids = accountIds.split(",").map(Number);
    // Anything that belonged to a disconnected inbox goes away.
    setFoldersBy((all) => Object.fromEntries(ids.filter((id) => all[id]).map((id) => [id, all[id]])));
    if (openRef.current && !ids.includes(openRef.current.account_id)) clearOpen();
    // Until it's in, an empty list means still loading, not "No mail in this folder".
    const run = listRun.current;
    setListLoading(true);
    // All Inbox needs nothing from the folders, so its mail is asked for at the same time.
    const listFirst = viewRef.current.kind === "all";
    Promise.all([...ids.map(reloadFolders), ...(listFirst ? [reloadList()] : [])])
      .then(() => (listFirst ? undefined : reloadList()))
      .catch((e) => setNotice(say(e)))
      .finally(() => {
        if (run === listRun.current) setListLoading(false);
      });
    void sync();
    let looks = 0;
    const timer = setInterval(() => {
      looks += 1;
      void sync(undefined, false, looks % WHOLE_LOOK_EVERY !== 0);
    }, REFRESH_EVERY_MS);
    return () => clearInterval(timer);
  }, [accountIds, reloadFolders, reloadList, sync]);

  // New mail shows the moment the server says so. The one-minute refresh above stays as the
  // backstop, for servers that can't say and connections that quietly die.
  useEffect(() => {
    if (!accountIds) return;
    const ids = accountIds.split(",").map(Number);
    let stopped = false;
    const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
    const watch = async (id: number) => {
      let backoff = 0;
      while (!stopped) {
        try {
          const { supported, changed } = await api.mailWait(id);
          if (stopped || !supported) return;
          backoff = 0;
          // Finished before asking again, as the server would only say the same thing meanwhile.
          if (changed) await sync(id, false, true);
          // A server that reports constantly mustn't be logged in to constantly.
          await pause(MAIL_WAIT_GAP_MS);
        } catch (e) {
          // Repeating a refused password can get an account locked, so stop asking.
          if (e instanceof ApiError && (e.code === "auth" || e.code === "needs_signin")) return;
          backoff = Math.min(backoff * 2 || MAIL_WAIT_RETRY_MS, MAIL_WAIT_MOST_RETRY_MS);
          await pause(backoff);
        }
      }
    };
    ids.forEach((id) => void watch(id));
    return () => {
      stopped = true;
      ids.forEach((id) => void api.mailWaitStop(id).catch(() => {}));
    };
  }, [accountIds, sync]);

  const allFolders = Object.values(foldersBy).flat();

  // What's showing went away with a disconnected inbox. Go back to everything.
  useEffect(() => {
    if (!accounts || accounts.length === 0) return;
    const folderGone =
      view.kind === "folder" &&
      Object.keys(foldersBy).length > 0 &&
      !allFolders.some((f) => f.id === view.folderId);
    if (folderGone) show({ kind: "all" });
  });

  const stopSearching = () => {
    searchRun.current += 1;
    searchTimers.current.forEach(clearTimeout);
    searchTimers.current = [];
    searchFilters.current = null;
    searchExtra.current = [];
    setSearchNote(NO_SEARCH_NOTE);
    setSearchText("");
  };

  /** Loads a different list. If it's slow the old one makes way, so its mail can't pass for this one's. */
  const loadNewList = () => {
    const run = ++listRun.current;
    if (listWait.current) clearTimeout(listWait.current);
    setListLoading(false);
    const shown = viewRef.current;
    const remembered = listMemory.current.get(listMemoryKey(shown, tabOf(shown).onTab, unreadOnlyRef.current) ?? "");
    if (remembered) {
      setThreads(withMovesShown(remembered, shown));
      const counts = countsMemory.current.get(
        `${shown.kind === "folder" ? shown.folderId : null}|${unreadOnlyRef.current}`,
      );
      if (counts) setTabCounts(counts);
    } else {
      listWait.current = setTimeout(() => {
        setThreads([]);
        setTabCounts({});
        setListLoading(true);
      }, LIST_WAIT_MS);
    }
    void reloadList()
      .catch((e) => setNotice(say(e)))
      .finally(() => {
        if (run !== listRun.current) return;
        if (listWait.current) clearTimeout(listWait.current);
        setListLoading(false);
      });
  };

  const show = (next: View, record = true) => {
    const was = viewRef.current;
    if (record && was.kind !== "search" && JSON.stringify(was) !== JSON.stringify(next)) {
      const left = { view: was, account: selected?.id ?? null };
      setTrail((t) => ({ back: [...t.back, left].slice(-TRAIL_MOST), ahead: [] }));
    }
    // It may be what they're going to look at, so it can't wait any longer.
    void moves.moveNow();
    listRun.current++;
    if (listWait.current) clearTimeout(listWait.current);
    setListLoading(false);
    if (next.kind !== "search") stopSearching();
    viewRef.current = next;
    listSize.current = LIST_PAGE;
    setView(next);
    setPicked(new Set());
    pickAnchor.current = null;
    clearOpen();
    // Nothing in the old list is on screen, so no key should reach it.
    if (!THREAD_VIEWS.includes(next.kind)) setThreads([]);
    if (next.kind === "all" || next.kind === "folder" || next.kind === "later" || next.kind === "from") loadNewList();
    else if (next.kind !== "drafts") void reloadList().catch((e) => setNotice(say(e)));
  };

  /** The bottom of what's loaded is near, so more comes. */
  const growList = () => {
    const shown = viewRef.current;
    if (shown.kind !== "all" && shown.kind !== "folder" && shown.kind !== "search") return;
    if (growingList.current || threads.length < listSize.current) return;
    growingList.current = true;
    listSize.current += LIST_PAGE;
    void reloadList()
      .catch(() => {})
      .finally(() => {
        growingList.current = false;
      });
  };

  const applySearchResults = (list: ThreadSummary[]) => {
    setThreads(
      list.filter((t) => !moves.gone.current.has(threadKey(t))).flatMap((t) => withChange(t, rowChanges.current)),
    );
    setPicked((current) => {
      if (current.size === 0) return current;
      const present = new Set(list.map(threadKey));
      const kept = [...current].filter((k) => present.has(k));
      return kept.length === current.size ? current : new Set(kept);
    });
  };

  /** The slower part: AI, if it's on, then the mail servers for older email. */
  const searchDeeper = async (text: string, run: number, useAi: boolean, inbox: number | null) => {
    const stale = () => run !== searchRun.current;
    let filters: { filters: SearchFilters } | { text: string; account: number | null } = { text, account: inbox };
    if (useAi && hasAiKey && settings.ai_search === "on" && looksLikeASentence(text)) {
      setSearchNote((note) => ({ ...note, aiThinking: true }));
      try {
        const now = new Date();
        const pad = (n: number) => String(n).padStart(2, "0");
        const today = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
        const understood = await api.searchUnderstand(text, today, -now.getTimezoneOffset());
        if (stale()) return;
        if (understood.used_ai) {
          filters = { filters: { ...understood.filters, account: inbox } };
          searchFilters.current = filters.filters;
          const reply = await api.searchMail(filters);
          if (stale()) return;
          applySearchResults(reply.threads);
          setSearchNote((note) => ({
            ...note,
            aiThinking: false,
            summary: understood.summary || tx("Understood by AI"),
            inBin: reply.in_bin,
          }));
        } else {
          setSearchNote((note) => ({ ...note, aiThinking: false }));
        }
      } catch (e) {
        if (stale()) return;
        setSearchNote((note) => ({ ...note, aiThinking: false, problem: say(e) }));
      }
    }
    setSearchNote((note) => ({ ...note, checking: true }));
    try {
      const reply = await api.searchServers(filters);
      if (stale()) return;
      searchExtra.current = reply.extra_ids ?? [];
      applySearchResults(reply.threads);
      setSearchNote((note) => ({
        ...note,
        checking: false,
        unreached: reply.unreached ?? [],
        inBin: reply.in_bin,
      }));
      // Words can miss what's meant, so a phrase also looks for mail about the same thing.
      if (useAi && text.trim().includes(" ") && !text.includes(":")) {
        const meaning = await api.searchMeaning(text);
        if (stale() || meaning.ids.length === 0) return;
        const extra = [...new Set([...searchExtra.current, ...meaning.ids])];
        searchExtra.current = extra;
        const merged = await api.searchMail(filters, extra);
        if (stale()) return;
        applySearchResults(merged.threads);
        const added = Math.max(0, merged.threads.length - reply.threads.length);
        setSearchNote((note) => ({ ...note, byMeaning: added, inBin: merged.in_bin }));
      }
    } catch {
      if (!stale()) setSearchNote((note) => ({ ...note, checking: false }));
    }
  };

  /** Saved email straight away, and the rest once typing pauses. */
  const startSearch = (text: string, right_now: boolean, useAi = true, inbox = searchIn) => {
    searchRun.current += 1;
    const run = searchRun.current;
    searchTimers.current.forEach(clearTimeout);
    searchExtra.current = [];
    listSize.current = FIRST_RESULTS;
    if (viewRef.current.kind !== "search") {
      const was = viewRef.current;
      viewBeforeSearch.current = was;
      // From one inbox's folder a search looks in that inbox, and from All Inbox in all of them.
      inbox = was.kind === "folder" ? (allFolders.find((f) => f.id === was.folderId)?.account_id ?? null) : null;
      const next: View = { kind: "search" };
      viewRef.current = next;
      setView(next);
      setPicked(new Set());
      pickAnchor.current = null;
      clearOpen();
      // What was showing isn't a result, so it goes straight away.
      listRun.current += 1;
      if (listWait.current) clearTimeout(listWait.current);
      setThreads([]);
      setListLoading(true);
    }
    setSearchIn(inbox);
    setSearchNote(NO_SEARCH_NOTE);
    const quick = async () => {
      try {
        const reply = await api.searchMail({ text, account: inbox });
        if (run !== searchRun.current) return;
        searchFilters.current = reply.filters;
        applySearchResults(reply.threads);
        setListLoading(false);
        setSearchNote((note) => ({ ...note, inBin: reply.in_bin }));
      } catch (e) {
        if (run !== searchRun.current) return;
        setListLoading(false);
        setNotice(say(e));
      }
    };
    if (right_now) {
      searchTimers.current = [];
      void quick().then(() => searchDeeper(text, run, useAi, inbox));
    } else {
      searchTimers.current = [
        setTimeout(() => void quick(), SEARCH_TYPING_MS),
        setTimeout(() => void searchDeeper(text, run, useAi, inbox), SEARCH_PAUSE_MS),
      ];
    }
  };

  const changeSearch = (text: string) => {
    if (text.trim()) {
      setSearchText(text);
      startSearch(text, false);
    } else {
      show(viewBeforeSearch.current);
    }
  };

  const clearSearch = () => {
    show(viewBeforeSearch.current);
    searchInput.current?.blur();
  };

  const searchWithFilters = async (filters: SearchFilters) => {
    searchRun.current += 1;
    const run = searchRun.current;
    searchFilters.current = filters;
    searchExtra.current = [];
    setSearchNote({ ...NO_SEARCH_NOTE, checking: true });
    const local = await api.searchMail({ filters });
    if (run !== searchRun.current) return;
    applySearchResults(local.threads);
    const reply = await api.searchServers({ filters }).catch(() => null);
    if (run !== searchRun.current) return;
    if (reply) {
      searchExtra.current = reply.extra_ids ?? [];
      applySearchResults(reply.threads);
    }
    setSearchNote({ ...NO_SEARCH_NOTE, inBin: reply?.in_bin ?? local.in_bin, unreached: reply?.unreached ?? [] });
  };

  const reloadAiStatus = useCallback(() => {
    api
      .aiStatus()
      .then((s) => setHasAiKey(s.has_key))
      .catch(() => {});
    void reloadPaid();
  }, []);
  useEffect(reloadAiStatus, [reloadAiStatus]);

  const pickAccount = (id: number) => {
    setChosenAccount(id);
    const inbox = foldersBy[id]?.find((f) => f.role === "inbox");
    show(inbox ? { kind: "folder", folderId: inbox.id } : { kind: "all" });
  };

  /** A draft that's only on the mail server opens to be finished here. */
  const openServerDraft = async (t: ThreadSummary) => {
    if (openingDraft.current) return;
    openingDraft.current = true;
    try {
      const list = await api.getThread(t.account_id, t.thread_id, t.folder_id);
      const newest = list[list.length - 1];
      if (!newest) return;
      const draft = await api.draftFromMessage(newest.id);
      reloadDrafts();
      await reloadList();
      openDraft(draft.id);
    } catch (e) {
      setNotice(say(e));
    } finally {
      openingDraft.current = false;
    }
  };

  const pickThread = (t: ThreadSummary) => {
    const inDrafts = Object.values(foldersRef.current)
      .flat()
      .find((f) => f.id === t.folder_id)?.role === "drafts";
    if (inDrafts && viewRef.current.kind === "folder") {
      void openServerDraft(t);
      return;
    }
    const next: OpenThread = {
      account_id: t.account_id,
      thread_id: t.thread_id,
      folder_id: t.folder_id,
    };
    // Clicking the one already open must not disturb a reply or note being written.
    const same = openRef.current !== null && threadKey(openRef.current) === threadKey(next);
    if (typeof t.pinned === "boolean") openRow.current = t;
    else if (!same) openRow.current = null;
    openRef.current = next;
    setOpen(next);
    openRowDate.current = t.last_date;
    if (!same) setMessages(knownThread(t) ?? null);
    api
      .getThread(next.account_id, next.thread_id, next.folder_id)
      .then((list) => {
        if (openRef.current === next) setMessages(list);
      })
      .catch((e) => setNotice(say(e)));
    const at = threads.findIndex((x) => threadKey(x) === threadKey(t));
    if (at >= 0) {
      readThreadsAhead([
        ...threads.slice(at + 1, at + 1 + THREADS_BESIDE),
        ...threads.slice(Math.max(0, at - THREADS_BESIDE), at),
      ]);
    }
    if (readTimer.current) clearTimeout(readTimer.current);
    if (t.unread_count && settings.mark_read === "now") {
      markThreadRead(t);
    } else if (t.unread_count && settings.mark_read === "delay") {
      readTimer.current = setTimeout(() => {
        if (openRef.current === next) markThreadRead(t);
      }, READ_AFTER_MS);
    }
  };

  /** Unread counts catch up with these rows at once. A reload puts them right afterwards. */
  const countChange = (rows: ThreadSummary[], change: RowChange) => {
    const by = new Map<number, number>();
    rows.forEach((t) => {
      const was = t.unread_count > 0;
      const is = (change(t)?.unread_count ?? 0) > 0;
      if (was !== is) by.set(t.folder_id, (by.get(t.folder_id) ?? 0) + (is ? 1 : -1));
    });
    shiftUnread(by);
  };

  /** Shows a change to these rows at once. Returns how to stop holding it, once it's saved. */
  const showChange = (chosen: ThreadRef[], change: RowChange) => {
    const keys = new Set(chosen.map(threadKey));
    const apply = (list: ThreadSummary[]) => list.flatMap((t) => (keys.has(threadKey(t)) ? (change(t) ?? []) : t));
    countChange(threads.filter((t) => keys.has(threadKey(t))), change);
    keys.forEach((k) => rowChanges.current.set(k, change));
    setThreads(apply);
    listGeneration.current++;
    listMemory.current.forEach((list, key) => listMemory.current.set(key, apply(list)));
    // It may have left the list while open, and then this row is what knows if it's pinned or muted.
    const row = openRow.current;
    if (row && keys.has(threadKey(row))) openRow.current = change(row) ?? row;
    return () =>
      keys.forEach((k) => {
        if (rowChanges.current.get(k) === change) rowChanges.current.delete(k);
      });
  };

  /** If the server can't be reached the next refresh brings this back in line, so
   * there's nothing to report. */
  const markThreadRead = (t: ThreadRef) => {
    const saved = showChange([t], MARKED.seen);
    const ref = { account_id: t.account_id, thread_id: t.thread_id, folder_id: (t as Partial<ThreadSummary>).folder_id ?? 0 };
    // Read on this Mac first, so the menu bar and every count show it at once; the server after.
    api
      .markThreads([ref], "seen", () => {
        tellPanel("unread-changed");
        void reloadFolders(t.account_id).catch(() => {});
      })
      .then(() => rememberChange([t], { unread_count: 0 }))
      .catch(() => {})
      .finally(saved);
  };

  /** What opens once this conversation has left the list, as chosen in Settings. */
  const nextAfter = (from: OpenThread) => {
    const index = threads.findIndex((t) => threadKey(t) === threadKey(from));
    if (settings.after_action === "list" || index < 0) return null;
    return settings.after_action === "previous"
      ? (threads[index - 1] ?? threads[index + 1] ?? null)
      : (threads[index + 1] ?? threads[index - 1] ?? null);
  };

  /** The rows go straight away. The move itself waits a moment, so it can be taken back. */
  const moveAway = (chosen: OpenThread[], to: string, note?: string) => {
    if (chosen.length === 0) return;
    const keys = new Set(chosen.map(threadKey));
    if (to === "trash") {
      // The first one's row flies off, before it leaves the list.
      flyAway(
        document.querySelector(`.thread[data-key="${CSS.escape(threadKey(chosen[0]))}"]`),
        settingsRef.current.delete_effect ?? "sound",
        chosen.length,
      );
    }
    const from = openRef.current;
    const wasOpen = from !== null && keys.has(threadKey(from));
    const next = wasOpen && chosen.length === 1 ? nextAfter(from) : null;
    setNotice(null);
    countChange(threads.filter((t) => keys.has(threadKey(t))), () => null);
    // The whole row where there is one, so it can show in its new folder straight away.
    // Looked up by key, as "Archive all" can move thousands at once.
    const rows = new Map(threads.map((t) => [threadKey(t), t]));
    moves.move(
      chosen.map((c) => rows.get(threadKey(c)) ?? c),
      to,
      note,
    );
    setThreads((list) => list.filter((t) => !keys.has(threadKey(t))));
    setPicked((current) => {
      const left = new Set([...current].filter((k) => !keys.has(k)));
      if (left.size === 0) pickAnchor.current = null;
      return left;
    });
    if (wasOpen) clearOpen();
    if (next) pickThread(next);
  };

  afterMove.current = (moved, problem, to) => {
    if (problem) setNotice(problem);
    else {
      // Remembered lists catch up at once, so going back never shows mail in the old place.
      const movedKeys = new Set(moved.map(threadKey));
      listGeneration.current++;
      const arriving = (moved as ThreadSummary[]).filter((t) => t.subject !== undefined);
      const destinations = new Map(
        Object.values(foldersRef.current)
          .flat()
          .filter((f) => f.role === to || f.path === to)
          .map((f) => [f.id, f.account_id]),
      );
      listMemory.current.forEach((list, key) => {
        const kept = list.filter((t) => !movedKeys.has(threadKey(t)));
        const [place, tab] = key.split("|");
        const account = destinations.get(Number(place.slice("folder:".length)));
        const here = account !== undefined && (tab === "" || tab === "all");
        const added = here ? arriving.filter((t) => t.account_id === account) : [];
        listMemory.current.set(key, added.length > 0 ? [...added, ...kept].sort((a, b) => b.last_date - a.last_date) : kept);
      });
    }
    void reloadList().catch(() => {});
    const inboxes = [...new Set(moved.map((t) => t.account_id))];
    inboxes.forEach((id) => {
      void reloadFolders(id).catch(() => {});
      // Brings moved mail into the folder it went to.
      void sync(id);
    });
  };

  /** On screen now and saved in the background. If saving fails the list goes back to what's
   * true and says why. An open conversation that leaves the list makes way for the next one. */
  const changeInBackground = async (
    chosen: ThreadRef[],
    change: RowChange,
    save: () => Promise<unknown>,
    /** What remembered lists should hold once it's saved, if not simply read again. */
    remember?: () => "gone" | "stale" | Partial<ThreadSummary>,
  ) => {
    const from = openRef.current;
    const opened = from && chosen.find((t) => threadKey(t) === threadKey(from));
    const leaves = !!opened && change((threads.find((t) => threadKey(t) === threadKey(opened)) ?? opened) as ThreadSummary) === null;
    const next = from && leaves && chosen.length === 1 ? nextAfter(from) : null;
    setNotice(null);
    const saved = showChange(chosen, change);
    if (leaves) clearOpen();
    if (next) pickThread(next);
    const rows = chosen.map((c) => threads.find((t) => threadKey(t) === threadKey(c)) ?? c) as ThreadSummary[];
    let remembered = remember ?? (() => (rows.every((t) => change(t) === null) ? ("gone" as const) : ("stale" as const)));
    try {
      await save();
    } catch (e) {
      setNotice(say(e));
      remembered = () => "stale";
    }
    saved();
    rememberChange(chosen, remembered());
    tellPanel("unread-changed");
    await reloadList().catch(() => {});
    await Promise.all([...new Set(chosen.map((t) => t.account_id))].map((id) => reloadFolders(id))).catch(() => {});
    reloadLater();
  };

  const togglePicked = (t: ThreadSummary, range: boolean) => {
    const key = threadKey(t);
    const anchor = pickAnchor.current;
    setPicked((current) => {
      const next = new Set(current);
      if (range && anchor) {
        const keys = threads.map(threadKey);
        const [a, b] = [keys.indexOf(anchor), keys.indexOf(key)].sort((x, y) => x - y);
        if (a >= 0) keys.slice(a, b + 1).forEach((k) => next.add(k));
        else next.add(key);
      } else if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
    pickAnchor.current = key;
    // The conversation that was open makes way for the selection.
    clearOpen();
  };

  const selectAll = () => {
    setPicked(new Set(threads.map(threadKey)));
    clearOpen();
  };

  const pickedThreads = threads.filter((t) => picked.has(threadKey(t)));

  const actOnPicked = (action: string, to?: string) => actOnThreads(pickedThreads, action, to);

  /** Does one thing to these conversations, and takes them out of the selection. */
  const actOnThreads = async (chosen: ThreadSummary[], action: string, to?: string) => {
    if (chosen.length === 0) return;
    if (action === "move" && to) {
      moveAway(chosen, to);
      return;
    }
    setPicked((current) => {
      const next = new Set(current);
      chosen.forEach((t) => next.delete(threadKey(t)));
      if (next.size === 0) pickAnchor.current = null;
      return next;
    });
    // Some may not go through, and then those lists are read again rather than guessed at.
    let problem = false;
    await changeInBackground(chosen, MARKED[action] ?? ((t) => t), async () => {
      // These may be on their way here, and deleting for good needs them to have arrived.
      await moves.moveNow();
      const refs = chosen.map((t) => ({
        account_id: t.account_id,
        thread_id: t.thread_id,
        folder_id: t.folder_id,
      }));
      // Read and unread change on this Mac first and show in the menu bar at once.
      const reply =
        action === "seen" || action === "unseen"
          ? await api.markThreads(refs, action, () => tellPanel("unread-changed"))
          : await api.bulkThreads(refs, action, to);
      setNotice(reply.problem ?? null);
      problem = !!reply.problem;
    }, () =>
      problem
        ? "stale"
        : action === "delete_forever"
          ? "gone"
          : action === "seen"
            ? { unread_count: 0 }
            : action === "star"
              ? { flagged: true }
              : "stale",
    );
    // Brings moved mail into the folder it went to.
    if (action === "move") [...new Set(chosen.map((t) => t.account_id))].forEach((id) => void sync(id));
  };

  useEffect(() => {
    if (!allRead) return;
    const timer = setTimeout(() => setAllRead(null), UNDO_MOVE_MS);
    return () => clearTimeout(timer);
  }, [allRead]);

  const afterMarkingAll = async (changed: { account_id: number }[]) => {
    markingAll.current = false;
    // Every remembered list can hold these rows.
    forgetLists();
    await reloadList().catch(() => {});
    await Promise.all([...new Set(changed.map((t) => t.account_id))].map((id) => reloadFolders(id))).catch(() => {});
    tellPanel("mail-changed");
  };

  /** Mark all as read in the panel shows here at once, as this Mac only has it once every round is done. */
  const readByPanel = useRef<(() => void) | null>(null);
  const readFromPanelRef = useRef((_rows: Movable[], _all: boolean) => {});
  readFromPanelRef.current = (rows, all) => {
    const inboxes = new Set(
      (accounts ?? [])
        .filter((a) => a.prefs.in_all_inbox)
        .flatMap((a) => (foldersBy[a.id] ?? []).filter((f) => f.role === "inbox").map((f) => f.id)),
    );
    const listed = new Set(threads.map(threadKey));
    readByPanel.current?.();
    // Rows listed here take their counts with them; the rest go by their folder, or all of them for All.
    readByPanel.current = showChange(all ? threads.filter((t) => inboxes.has(t.folder_id)) : rows, MARKED.seen);
    if (all) {
      setFoldersBy((by) =>
        Object.fromEntries(
          Object.entries(by).map(([id, folders]) => [id, folders.map((f) => (inboxes.has(f.id) ? { ...f, unread: 0 } : f))]),
        ),
      );
    } else {
      const by = new Map<number, number>();
      for (const t of rows) {
        if (!listed.has(threadKey(t))) by.set(t.folder_id, (by.get(t.folder_id) ?? 0) - 1);
      }
      shiftUnread(by);
    }
  };

  /** Everything this list could show, not only what's on screen so far, in rounds until none are left. */
  const markAllRead = async () => {
    const shown = viewRef.current;
    if (shown.kind !== "all" && shown.kind !== "folder") return;
    const { onTab } = tabOf(shown);
    const listedTab = onTab === "all" ? undefined : onTab;
    const before = threads;
    setAllRead(null);
    setThreads([]);
    markingAll.current = true;
    setMarkingCount(0);
    const marked: Movable[] = [];
    try {
      const done = await markAllInRounds(
        () =>
          shown.kind === "all"
            ? api.listAll("inbox", listedTab, MARK_ALL_MOST, true)
            : api.listThreads(shown.folderId, listedTab, MARK_ALL_MOST, true),
        () => void afterMarkingAll(marked),
        setMarkingCount,
        marked,
      );
      if (done.problem) setNotice(done.problem);
    } catch (e) {
      if (marked.length === 0) setThreads(before);
      setNotice(say(e));
    }
    // Even after a problem partway, what was marked can be taken back.
    if (marked.length > 0) setAllRead(marked);
    setMarkingCount(null);
    await afterMarkingAll(marked);
  };

  const undoAllRead = async () => {
    const all = allRead;
    if (!all) return;
    setAllRead(null);
    await unmarkInRounds(all, () => void afterMarkingAll(all)).catch((e) => setNotice(say(e)));
    await afterMarkingAll(all);
  };

  const openDraft = (id: number) => {
    setReopenProblem(undefined);
    setInlineThread(null);
    setComposing(id);
  };

  const discardDraft = (id: number) => {
    api
      .draftDiscard(id)
      .catch((e) => setNotice(say(e)))
      .finally(reloadDrafts);
  };

  /** "Spam" or "Not spam", on whatever's chosen. The open conversation moves on if it goes. */
  const markSpam = async (chosen: ThreadSummary[], spam: boolean, block = false) => {
    if (chosen.length === 0) return;
    const refs = chosen.map((t) => ({
      account_id: t.account_id,
      thread_id: t.thread_id,
      folder_id: t.folder_id,
    }));
    const inJunk = (t: ThreadSummary) =>
      Object.values(foldersBy)
        .flat()
        .find((f) => f.id === t.folder_id)?.role === "junk";
    setPicked((current) => {
      const next = new Set(current);
      chosen.forEach((t) => next.delete(threadKey(t)));
      return next;
    });
    await changeInBackground(
      chosen,
      (t) => (spam || inJunk(t) ? null : { ...t, risk: 0 }),
      async () => {
        await (spam ? api.spamMark(refs, block) : api.spamClear(refs));
        // What the open email says about itself has changed.
        const now = openRef.current;
        if (now && chosen.some((t) => threadKey(t) === threadKey(now))) {
          const fresh = await api.getThread(now.account_id, now.thread_id, now.folder_id);
          if (openRef.current && threadKey(openRef.current) === threadKey(now)) setMessages(fresh);
        }
      },
    );
    // Brings it into the folder it went to.
    [...new Set(chosen.map((t) => t.account_id))].forEach((id) => void sync(id));
  };

  const openThreadSummary = (): ThreadSummary[] => {
    if (!open) return [];
    const found = threads.find((t) => threadKey(t) === threadKey(open));
    return found ? [found] : [{ ...open } as ThreadSummary];
  };

  const unsubscribing = useRef(false);
  const unsubscribe = async (m: Message) => {
    // It can send an email, so a second click mustn't send another.
    if (unsubscribing.current) return;
    unsubscribing.current = true;
    try {
      const reply = await api.unsubscribe(m.id);
      void reloadListRef.current();
      if (reply.kind === "open" && reply.url) {
        // On the web, their page opens in a new tab of this browser.
        const tab = window.open(reply.url, "_blank", "noopener,noreferrer");
        setNotice(
          tab === null
            ? tx("Your browser blocked their unsubscribe page. Allow pop-ups for Mailzo and try again.")
            : tx("Their unsubscribe page opened in a new tab."),
        );
      } else {
        setNotice(
          reply.kind === "opened"
            ? tx("Their unsubscribe page opened in your browser.")
            : tx("You're unsubscribed. It can take a few days to stop."),
        );
      }
    } catch (e) {
      setNotice(say(e));
    } finally {
      unsubscribing.current = false;
    }
  };

  const pickTeamTab = (next: string) => {
    teamTabRef.current = next;
    setTeamTab(next);
    setPicked(new Set());
    pickAnchor.current = null;
    clearOpen();
    void reloadList().catch((e) => setNotice(say(e)));
  };

  const toggleUnread = () => {
    unreadOnlyRef.current = !unreadOnlyRef.current;
    setUnreadOnly(unreadOnlyRef.current);
    listSize.current = LIST_PAGE;
    setPicked(new Set());
    pickAnchor.current = null;
    loadNewList();
  };

  /** Every email in The rest, found when "Archive all" is pressed, as only a page of it is loaded. */
  const restToArchive = useRef<{ shown: View; threads: ThreadSummary[] } | null>(null);
  const countRest = async (): Promise<number> => {
    const shown = viewRef.current;
    restToArchive.current = null;
    if (shown.kind !== "all" && shown.kind !== "folder") return 0;
    const all: ThreadSummary[] = [];
    try {
      for (let page = 0; page < REST_PAGES_MAX; page++) {
        const offset = page * REST_PAGE;
        const found =
          shown.kind === "all"
            ? await api.listAll("inbox", "rest", REST_PAGE, false, offset)
            : await api.listThreads(shown.folderId, "rest", REST_PAGE, false, offset);
        all.push(...found);
        if (found.length < REST_PAGE) break;
      }
    } catch (e) {
      setNotice(say(e));
      return 0;
    }
    const threads = all.filter((t) => !moves.gone.current.has(threadKey(t)));
    restToArchive.current = { shown, threads };
    return threads.length;
  };
  const archiveAllShown = () => {
    const rest = restToArchive.current;
    restToArchive.current = null;
    // They went somewhere else while the question was up, and that's not what was counted.
    if (!rest || rest.shown !== viewRef.current || tabRef.current !== "rest") return;
    moveAway(rest.threads, "archive");
  };

  const pickTab = (next: string) => {
    tabRef.current = next;
    setTab(next);
    setPicked(new Set());
    pickAnchor.current = null;
    clearOpen();
    loadNewList();
  };

  const showingLater = (which: string) => viewRef.current.kind === "later" && viewRef.current.which === which;

  /** Whether sorting a row somewhere takes it out of the tab on screen. */
  const leavesTab = (goesTo: string) => {
    const tab = tabOf(viewRef.current).onTab;
    return !!tab && tab !== "all" && !tab.startsWith("team_") && tab !== goesTo;
  };

  const pinThread = (t: ThreadRef, on: boolean) => {
    const done = changeInBackground([t], (r) => ({ ...r, pinned: on }), () => api.threadPin(t.account_id, t.thread_id, on));
    // Only inboxes keep pinned mail at the top. The next list puts anything else right.
    const shown = viewRef.current;
    const inInbox =
      shown.kind === "all" || (shown.kind === "folder" && foldersRef.current && Object.values(foldersRef.current).flat().find((f) => f.id === shown.folderId)?.role === "inbox");
    if (inInbox) setThreads((list) => [...list].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.last_date - a.last_date));
    return done;
  };

  const muteThread = (t: ThreadRef, on: boolean) => {
    const stays = on === showingLater("muted");
    return changeInBackground([t], (r) => (stays ? { ...r, muted: on } : null), () => api.threadMute(t.account_id, t.thread_id, on));
  };

  const snoozeThread = (t: ThreadRef, at: number | null) => {
    const stays = (at !== null) === showingLater("snoozed");
    return changeInBackground([t], (r) => (stays ? { ...r, snooze_until: at } : null), () => api.threadSnooze(t.account_id, t.thread_id, at));
  };

  const followUpThread = (t: ThreadRef, at: number | null) => {
    const stays = at !== null || !showingLater("reminders");
    return changeInBackground(
      [t],
      (r) => (stays ? { ...r, follow_at: at, follow_due: false } : null),
      () => api.threadFollowUp(t.account_id, t.thread_id, at),
    );
  };

  const starThread = (t: ThreadRef, on: boolean) =>
    changeInBackground(
      [t],
      on ? MARKED.star : MARKED.unstar,
      () => api.setThreadFlagged(t.account_id, t.thread_id, on),
      () => ({ flagged: on }),
    );

  const markThreadUnread = (t: ThreadRef) => {
    clearOpen();
    return changeInBackground([t], MARKED.unseen, () => api.setThreadSeen(t.account_id, t.thread_id, false));
  };

  /** In or out of Needs you, for one or many. "Done" is the way out. */
  const setNeedsMe = async (threads: ThreadSummary[], on: boolean) => {
    const group = on ? "needs_you" : "good_to_know";
    const stays = !leavesTab(group);
    // Counting again takes a moment on a big mailbox, so the number moves with the email.
    const moved = threads.filter((t) => !!t.needs_you !== on).length;
    if (moved > 0) {
      countsChanged.current++;
      setTabCounts((c) => ({ ...c, needs_you: Math.max(0, (c.needs_you ?? 0) + (on ? moved : -moved)) }));
    }
    await changeInBackground(
      threads,
      (r) => (stays ? { ...r, needs_you: on } : null),
      async () => {
        for (const t of threads) await api.threadPriority(t.account_id, t.thread_id, group);
      },
    );
  };

  /** Something chosen from the menu on a right-clicked email. */
  const onRowAction = async (thread: ThreadSummary, action: ThreadAction) => {
    const many = picked.has(threadKey(thread)) && picked.size > 1;
    const chosen = many ? pickedThreads : [thread];
    const isOpen = open !== null && threadKey(open) === threadKey(thread);
    switch (action.kind) {
      case "needs_me":
        await setNeedsMe(chosen, action.on);
        break;
      case "select":
        togglePicked(thread, false);
        break;
      case "select_all":
        selectAll();
        break;
      case "clear_selection":
        setPicked(new Set());
        pickAnchor.current = null;
        break;
      case "compose":
        try {
          // Opens the conversation, so the reply has somewhere to sit. A selection would
          // take the place of the conversation, so it goes.
          setPicked(new Set());
          pickAnchor.current = null;
          if (!isOpen) pickThread(thread);
          const list = await api.getThread(thread.account_id, thread.thread_id, thread.folder_id);
          await compose(action.mode, list[list.length - 1] ?? null, thread);
        } catch (e) {
          setNotice(say(e));
        }
        break;
      case "mark":
        await actOnThreads(chosen, action.action);
        break;
      case "move":
        // The open conversation moves on to the next one, as chosen in Settings.
        moveAway(chosen, action.to);
        break;
      case "delete_forever":
        setDeleting(chosen);
        break;
      case "spam":
        await markSpam(chosen, true, action.block);
        break;
      case "not_spam":
        await markSpam(chosen, false);
        break;
      case "pin":
        await pinThread(thread, action.on);
        break;
      case "mute":
        await muteThread(thread, action.on);
        break;
      case "snooze":
        await snoozeThread(thread, action.at);
        break;
      case "follow_up":
        await followUpThread(thread, action.at);
        break;
      case "pick_snooze":
        setPickFor({ thread, kind: "snooze" });
        break;
      case "pick_follow_up":
        setPickFor({ thread, kind: "follow" });
        break;
      case "sort": {
        const stays = !leavesTab(action.category);
        await changeInBackground(
          threads.filter((t) => t.lead_address === thread.lead_address),
          (r) => (stays ? { ...r, needs_you: false } : null),
          // Chosen outright, so mail that counted as needing them goes too.
          async () => {
            await api.sortSender(thread.lead_address, action.category);
            await api.senderPriority(thread.lead_address, action.category === "promotions" ? "rest" : "good_to_know");
          },
        );
        forgetLists();
        break;
      }
    }
  };

  const compose = async (
    mode: Draft["mode"],
    message: Message | null,
    /** The conversation it's replying within, if it isn't the one already open. */
    within: OpenThread | null = open,
  ) => {
    const target =
      message?.account_id ??
      accounts?.find((a) => a.prefs.default_for_new)?.id ??
      open?.account_id ??
      selected?.id;
    // A one-tap answer is for the reply this opens. If none opens, it mustn't wait for the next one.
    const start = replyStart.current;
    replyStart.current = null;
    if (target === undefined) return;
    // Read fresh, as opening another conversation may have just closed a reply.
    if (composingRef.current !== null) {
      const sameReply = message && within && composingInline.current === threadKey(within);
      if (!sameReply) setNotice(tx("Finish or close the message you're writing first."));
      else if (start) {
        // The reply to this conversation is already open, so the answer goes into it.
        replyStart.current = start;
        setStartAgain((n) => n + 1);
      }
      return;
    }
    // A second click while the first is still opening would make a second, empty draft.
    if (composeStarting.current) {
      if (start) replyStart.current = start;
      return;
    }
    replyStart.current = start;
    composeStarting.current = true;
    setReplyStarting(true);
    try {
      const d = await api.draftNew(
        target,
        mode,
        message?.id ?? null,
        message ? fullDate(message.date) : null,
      );
      // Replies open under the conversation. New messages float.
      const replying = !!(message && within);
      blankDrafts.current.add(d.id);
      setInlineThread(replying && within ? threadKey(within) : null);
      setReopenProblem(undefined);
      setComposing(d.id);
      reloadDrafts();
    } catch (e) {
      replyStart.current = null;
      setNotice(say(e));
    } finally {
      composeStarting.current = false;
      setReplyStarting(false);
    }
  };

  // Waits for the team's news, so changes show up within moments. When there's no
  // team, or no connection, it looks again later rather than keep asking.
  const reloadListRef = useRef(reloadList);
  reloadListRef.current = reloadList;
  useEffect(() => {
    let live = true;
    const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
    void (async () => {
      while (live) {
        try {
          const news = await api.teamPoll(true);
          // Even if this loop is ending: the engine has already taken this news in.
          if (news.changed) {
            setTeamTick((n) => n + 1);
            setAccounts(await api.listAccounts());
            await reloadListRef.current();
          }
          await pause(news.active ? TEAM_NEXT_LOOK_MS : TEAM_LOOK_AGAIN_MS);
        } catch {
          await pause(TEAM_RETRY_MS);
        }
      }
    })();
    return () => {
      live = false;
    };
  }, [teamEpoch]);

  // Moving to another conversation keeps the reply as a draft, and doesn't carry it along.
  useLayoutEffect(() => {
    if (inlineThread === null) return;
    // A message brought back by Undo reopens as a floating one.
    if (composing === null) {
      setInlineThread(null);
    } else if (!open || threadKey(open) !== inlineThread) {
      setComposing(null);
      setInlineThread(null);
      reloadDrafts();
    }
  }, [open, composing, inlineThread, reloadDrafts]);

  /** A new email to this address, open and ready to write. */
  const writeTo = async (to: string, subject: string, accountId?: number) => {
    const target = accountId ?? accounts?.find((a) => a.prefs.default_for_new)?.id ?? selected?.id;
    if (target === undefined) return;
    // Like any new message: not while one is being written or one is still opening.
    if (composingRef.current !== null) {
      setNotice(tx("Finish or close the message you're writing first."));
      return;
    }
    if (composeStarting.current) return;
    composeStarting.current = true;
    try {
      const draft = await api.draftNew(target, "new", null, null);
      await api.draftSave({ ...draft, to: [{ name: "", address: to }], subject });
      setComposing(draft.id);
      reloadDrafts();
    } catch (e) {
      setNotice(say(e));
    } finally {
      composeStarting.current = false;
    }
  };

  const composeRef = useRef(compose);
  composeRef.current = compose;
  const moveAwayRef = useRef(moveAway);
  moveAwayRef.current = moveAway;
  /** Moves the conversation that's open. */
  const leaveTo = (to: string) => {
    if (openRef.current) moveAway([openRef.current], to);
  };
  const leaveRef = useRef(leaveTo);
  leaveRef.current = leaveTo;
  const threadsRef = useRef(threads);
  threadsRef.current = threads;
  const pickedSizeRef = useRef(picked.size);
  pickedSizeRef.current = picked.size;
  const pickAccountRef = useRef(pickAccount);
  pickAccountRef.current = pickAccount;
  const showRef = useRef(show);
  showRef.current = show;
  /** Back and forward, for the keys, which are set up once. */
  const travelRef = useRef({ back: () => {}, forward: () => {} });
  const lastMessage = useRef<Message | null>(null);
  lastMessage.current = messages?.[messages.length - 1] ?? null;

  // Something changed in the menu bar panel, or it wants a conversation opened here.
  const pickThreadRef = useRef(pickThread);
  pickThreadRef.current = pickThread;
  useEffect(() => {
    const openHere = (t: ThreadSummary) => {
      setSettingsPage(null);
      // The calendar and the like have nowhere to show it.
      if (!THREAD_VIEWS.includes(viewRef.current.kind)) showRef.current({ kind: "all" });
      pickThreadRef.current(t);
    };
    const openClickedBanner = () => {
      void clickedBanner()
        .then((t) => t && openHere(t))
        .catch(() => {});
    };
    // The click can be what started the app, before anything here was listening.
    openClickedBanner();
    const stops = [
      hearFromPanel("banner-clicked", openClickedBanner),
      hearFromPanel<Movable[] | null>("mail-changed", (moved) => {
        // These have left the store now, so its counts are right without leaving them out.
        moved?.forEach((t) => leavingFromPanel.current.delete(threadKey(t)));
        // Whatever it marked read is this Mac's now, so the reload below shows it as it is.
        readByPanel.current?.();
        readByPanel.current = null;
        reloadDrafts();
        void reloadListRef.current();
        accountsRef.current?.forEach((a) => void reloadFolders(a.id));
        // Brings moved mail into the folder it went to.
        void syncRef.current();
      }),
      // Shown as its banner appears; what changed elsewhere follows when the refresh is done.
      hearFromPanel("new-mail", () => {
        void reloadListRef.current();
        accountsRef.current?.forEach((a) => void reloadFolders(a.id));
      }),
      hearFromPanel<Movable[]>("mail-leaving", (rows) => panelMoved(rows, true)),
      hearFromPanel<Movable[]>("mail-staying", (rows) => panelMoved(rows, false)),
      hearFromPanel<{ rows: Movable[]; all: boolean }>("mail-read", ({ rows, all }) => readFromPanelRef.current(rows, all)),
      hearFromPanel<ThreadSummary>("open-thread", openHere),
      hearFromPanel("open-settings", () => setSettingsPage((page) => page ?? "general")),
      // A mouse's side buttons, like the arrows at the top of the sidebar.
      hearFromPanel<string>("travel", (way) => (way === "back" ? travelRef.current.back() : travelRef.current.forward())),
      hearFromPanel("show-unread", () => {
        setSettingsPage(null);
        unreadOnlyRef.current = true;
        setUnreadOnly(true);
        tabRef.current = "all";
        setTab("all");
        showRef.current({ kind: "all" });
      }),
    ];
    return () => stops.forEach((stop) => stop());
  }, [reloadDrafts, reloadFolders, panelMoved]);

  /** Keys that act straight away, for people who like to keep their hands there. */
  const singleKeyRef = useRef((_e: KeyboardEvent) => {});
  singleKeyRef.current = (e: KeyboardEvent) => {
    if (settings.keyboard_keys !== "on" || addingRef.current || composeStarting.current) return;
    // Holding a key down moves through the list, but acts on one conversation only once.
    if (e.repeat && !["j", "k", "ArrowDown", "ArrowUp"].includes(e.key)) return;
    if (!THREAD_VIEWS.includes(viewRef.current.kind) && e.key !== "c" && e.key !== "/") return;
    const inside = document.activeElement as HTMLElement | null;
    if (inside?.closest("input, textarea, select, .ProseMirror, iframe, [contenteditable]")) return;
    if (document.querySelector('.settings, .scrim, [role="dialog"], [role="menu"], .composer-inline')) return;
    const shown = threadsRef.current;
    const at = open ? shown.findIndex((t) => threadKey(t) === threadKey(open)) : -1;
    // It may have left this list while open, and then the row it was opened from knows its marks.
    const kept = open && openRow.current && threadKey(openRow.current) === threadKey(open) ? openRow.current : undefined;
    const here = at >= 0 ? shown[at] : kept;
    const take = () => {
      e.preventDefault();
      return true;
    };
    switch (e.key) {
      case "j":
      case "ArrowDown": {
        const next = shown[at + 1];
        if (next && take()) pickThread(next);
        break;
      }
      case "k":
      case "ArrowUp": {
        const previous = shown[Math.max(at - 1, 0)];
        if (previous && take()) pickThread(previous);
        break;
      }
      case "c":
        take();
        void composeRef.current("new", null);
        break;
      case "/":
        take();
        searchInput.current?.focus();
        break;
      case "e":
        if (open && take()) void leaveRef.current("archive");
        break;
      case "#":
        if (open && take()) void leaveRef.current("trash");
        break;
      case "d": {
        // Done: out of Needs you, for what's selected or else the open one.
        const chosen = pickedThreads.length ? pickedThreads : here ? [here] : [];
        const done = chosen.filter((t) => t.needs_you);
        if (done.length && take()) {
          if (pickedThreads.length) setPicked(new Set());
          void setNeedsMe(done, false);
        }
        break;
      }
      case "s":
        if (open && take()) {
          // Out of the list, its own messages know best, the same as the star in the reader.
          void starThread(open, !(at >= 0 ? shown[at].flagged : (messages?.some((m) => m.flagged) ?? false)));
        }
        break;
      case "u":
        if (open && take()) void markThreadUnread(open);
        break;
      case "h":
        if (open && take()) {
          const when = whenChoices().find((c) => c.label === tx("Tomorrow morning"));
          if (when) void snoozeThread(open, when.at);
        }
        break;
      case "p":
        if (open && take()) void pinThread(open, !(here?.pinned ?? false));
        break;
      case "m":
        if (open && take()) void muteThread(open, !(here?.muted ?? false));
        break;
      case "r":
      case "a":
      case "f":
        if (lastMessage.current && take()) {
          const mode = e.key === "r" ? "reply" : e.key === "a" ? "reply_all" : "forward";
          void composeRef.current(mode, lastMessage.current);
        }
        break;
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && pickedSizeRef.current > 0 && !document.querySelector('[role="dialog"], [role="menu"]')) {
        setPicked(new Set());
        pickAnchor.current = null;
        return;
      }
      // In the full-width layout, Escape goes back from an email to the list.
      // A key sent to the window itself has no element to look inside.
      const typing = e.target instanceof Element && e.target.closest("input, textarea, [contenteditable='true']");
      if (
        e.key === "Escape" &&
        settingsRef.current.layout === "full" &&
        openRef.current &&
        !typing &&
        !document.querySelector('[role="dialog"], [role="menu"], .composer')
      ) {
        clearOpen();
        return;
      }
      if (
        (e.key === "Backspace" || e.key === "Delete") &&
        !e.metaKey &&
        !e.repeat &&
        !addingRef.current &&
        !composeStarting.current &&
        THREAD_VIEWS.includes(viewRef.current.kind) &&
        openRef.current &&
        !document.querySelector('[role="dialog"], [role="menu"], .composer-inline') &&
        !(document.activeElement as HTMLElement | null)?.closest("input, textarea, .ProseMirror, iframe")
      ) {
        e.preventDefault();
        void leaveRef.current(settingsRef.current.delete_key === "archive" ? "archive" : "trash");
        return;
      }
      if (!e.metaKey && !e.ctrlKey && !e.altKey) {
        singleKeyRef.current(e);
        return;
      }
      if (!e.metaKey || e.shiftKey || e.altKey) return;
      if (e.key === ",") {
        e.preventDefault();
        setSettingsPage((page) => (page ? null : "general"));
        return;
      }
      // Settings and pop-ups cover the mail, so its shortcuts wait until they close.
      if (document.querySelector(".settings, .scrim")) return;
      if (e.key === "\\") {
        e.preventDefault();
        setRail((on) => !on);
        return;
      }
      if (e.key === "k") {
        e.preventDefault();
        setCommandOpen(true);
        return;
      }
      // ⌘← and ⌘→ too, as some mice send them for their side buttons, unless they're
      // moving the cursor in something being typed.
      if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && !typing) {
        e.preventDefault();
        if (e.key === "ArrowLeft") travelRef.current.back();
        else travelRef.current.forward();
        return;
      }
      if (e.key === "[" || e.key === "]") {
        e.preventDefault();
        if (e.key === "[") travelRef.current.back();
        else travelRef.current.forward();
        return;
      }
      if (e.key === "a") {
        // Selecting text in a box or an email still works as usual.
        const inside = document.activeElement as HTMLElement | null;
        if (inside?.closest("input, textarea, .ProseMirror, [role='dialog'], iframe")) return;
        // The selection would take the place of a reply being written.
        if (document.querySelector(".composer-inline")) return;
        if (addingRef.current || composeStarting.current || !THREAD_VIEWS.includes(viewRef.current.kind)) return;
        e.preventDefault();
        setPicked(new Set(threadsRef.current.map(threadKey)));
        return;
      }
      if (e.key === "f") {
        e.preventDefault();
        searchInput.current?.focus();
        searchInput.current?.select();
        return;
      }
      // ⌘1 is All Inbox, and ⌘2 onwards are each account in turn.
      if (e.key === "1") {
        e.preventDefault();
        showRef.current({ kind: "all" });
      } else if (/^[2-9]$/.test(e.key)) {
        const target = accountsRef.current?.[Number(e.key) - 2]?.id;
        if (target !== undefined) {
          e.preventDefault();
          pickAccountRef.current(target);
        }
      } else if (e.key === "n") {
        e.preventDefault();
        void composeRef.current("new", null);
      } else if (e.key === "r" && lastMessage.current) {
        e.preventDefault();
        void composeRef.current(
          settingsRef.current.reply_default === "reply_all" ? "reply_all" : "reply",
          lastMessage.current,
        );
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const removeAccount = async (id: number) => {
    try {
      await api.removeAccount(id);
      setAccounts(await api.listAccounts());
      reloadDrafts();
    } catch (e) {
      setNotice(say(e));
    }
  };

  const saveDefaultInbox = async (id: number | null) => {
    try {
      for (const a of accounts ?? []) {
        const wanted = a.id === id;
        if (a.prefs.default_for_new !== wanted) {
          await api.updateAccountPrefs(a.id, { ...a.prefs, default_for_new: wanted });
        }
      }
      setAccounts(await api.listAccounts());
    } catch (e) {
      setNotice(say(e));
    }
  };

  /** Holds a finished message for the undo time, then sends it. */
  const queueSend = (draftId: number) => {
    setComposing(null);
    setReopenProblem(undefined);
    reloadDrafts();
    const seconds = Number(settings.undo_send) || 0;
    sendStarted.current.delete(draftId);
    setClock(Date.now());
    setWaitingSends((list) => [...list, { draftId, from: Date.now(), sendAt: Date.now() + seconds * 1000 }]);
  };

  const undoSend = (draftId: number) => {
    setWaitingSends((list) => list.filter((w) => w.draftId !== draftId));
    // Something else is being written, and that mustn't be pushed aside.
    if (composingRef.current !== null) {
      setNotice(tx("Your message went back to Drafts."));
      reloadDrafts();
      return;
    }
    setReopenProblem(undefined);
    setInlineThread(null);
    setComposing(draftId);
  };

  const sendWaiting = useCallback(
    async (draftId: number) => {
      if (sendStarted.current.has(draftId)) return;
      sendStarted.current.add(draftId);
      setWaitingSends((list) => list.filter((w) => w.draftId !== draftId));
      try {
        const reply = await api.sendDraft(draftId);
        setNotice(reply.warnings[0] ?? null);
        reloadDrafts();
        // The Sent copy is already in, so the reply shows in its conversation now.
        void reloadListRef.current();
        void reloadAccountsRef.current();
        void sync();
      } catch (e) {
        // Nothing is lost. It goes back to being a draft, with the reason showing.
        if (composingRef.current !== null) {
          setNotice(tx("Your message went back to Drafts: {0}", { "0": say(e) }));
          reloadDrafts();
          return;
        }
        setReopenProblem({ text: say(e), code: e instanceof ApiError ? e.code : undefined });
        setInlineThread(null);
        setComposing(draftId);
      }
    },
    [reloadDrafts, sync],
  );

  // Counts down each waiting message, and sends it when its time is up. If the app
  // quits first, it simply stays an unsent draft.
  const waitingRef = useRef(waitingSends);
  waitingRef.current = waitingSends;
  useEffect(() => {
    if (waitingSends.length === 0) return;
    const timer = setInterval(() => {
      const now = Date.now();
      setClock(now);
      waitingRef.current.filter((w) => w.sendAt <= now).forEach((w) => void sendWaiting(w.draftId));
    }, 250);
    return () => clearInterval(timer);
  }, [waitingSends.length, sendWaiting]);

  // Before an update restarts the app, messages still in their undo time go now and the
  // waiting move is made, as neither would survive the restart.
  const sendWaitingRef = useRef(sendWaiting);
  sendWaitingRef.current = sendWaiting;
  const moveNow = moves.moveNow;
  useEffect(
    () =>
      onBeforeRestart(() =>
        Promise.all([...waitingRef.current.map((w) => sendWaitingRef.current(w.draftId)), moveNow()]),
      ),
    [moveNow],
  );

  const reloadAccountsRef = useRef(async () => {});

  // Whatever's due: snoozes ending, reminders, scheduled mail. The first look happens
  // once the inboxes are known, as this may have been off for a while.
  useEffect(() => {
    let live = true;
    const tick = async () => {
      try {
        const reply = await api.dueTick(quietNow());
        if (!live) return;
        if (reply.changed) {
          reloadDrafts();
          reloadLater();
          void reloadList().catch(() => {});
          accountsRef.current?.forEach((a) => void reloadFolders(a.id).catch(() => {}));
        }
        const failed = reply.failed[0];
        if (failed) setNotice(tx("A scheduled message went back to Drafts: {0}", { "0": failed.problem }));
      } catch {
        // Tried again next time.
      }
    };
    void tick();
    const timer = setInterval(() => void tick(), DUE_EVERY_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountIds]);

  // A web page's menu (Reload and so on) makes no sense in an app. Text boxes and
  // selected text keep theirs, for copy and paste.
  useEffect(() => {
    const menu = (e: MouseEvent) => {
      const inText = (e.target as HTMLElement).closest?.("input, textarea, .ProseMirror, .selectable");
      if (!inText && !window.getSelection()?.toString()) e.preventDefault();
    };
    document.addEventListener("contextmenu", menu);
    return () => document.removeEventListener("contextmenu", menu);
  }, []);

  const reloadAccounts = async () => {
    try {
      setAccounts(await api.listAccounts());
    } catch (e) {
      setNotice(say(e));
    }
  };

  reloadAccountsRef.current = reloadAccounts;

  const openInboxSettings = (id: number) => {
    void reloadAccounts().then(() => setSettingsPage(`inbox:${id}`));
  };

  // A different choice of rows means what's open may no longer be in the list.
  const firstConversations = useRef(true);
  useEffect(() => {
    if (firstConversations.current) {
      firstConversations.current = false;
      return;
    }
    clearOpen();
    void whenSaved().then(() => {
      void reloadList().catch(() => {});
      accountsRef.current?.forEach((a) => void reloadFolders(a.id).catch(() => {}));
    });
  }, [settings.conversations, settings.smart_tabs, reloadList, reloadFolders, whenSaved]);

  // Opens on the first inbox instead, if that's the choice.
  const openedFirst = useRef(false);
  useEffect(() => {
    if (openedFirst.current || !selected) return;
    const inbox = foldersBy[selected.id]?.find((f) => f.role === "inbox");
    if (!inbox) return;
    openedFirst.current = true;
    if (settings.open_first === "selected") show({ kind: "folder", folderId: inbox.id });
  });

  const unreadInInboxes = Object.values(foldersBy)
    .flat()
    .filter((f) => f.role === "inbox")
    .reduce((sum, f) => sum + f.unread, 0);
  const dockCount = settings.dock_badge === "on" ? unreadInInboxes : 0;
  useEffect(() => {
    void native("set_dock_badge", { count: dockCount });
  }, [dockCount]);
  useEffect(() => tellPanel("unread-changed"), [unreadInInboxes]);
  // Signing out leaves no count behind on the Dock or menu bar icons.
  useEffect(() => () => void native("set_dock_badge", { count: 0 }), []);
  // Nor anything remembered about whose mail this was.
  useEffect(
    () => () => {
      forgetNewsletters();
      forgetContacts();
      forgetTrustedSenders();
    },
    [],
  );

  if (accounts === null) return null;

  const openFolders = open ? (foldersBy[open.account_id] ?? []) : [];
  const openFolder = open ? (openFolders.find((f) => f.id === open.folder_id) ?? null) : null;
  const openSummary = open ? threads.find((t) => threadKey(t) === threadKey(open)) : undefined;
  /** Once its sign-in code is copied, an email goes where Settings › Reading says: the bin unless chosen otherwise. */
  const afterCodeCopied = (thread: ThreadSummary) => {
    const after = settings.after_code_copy ?? "trash";
    if (after === "trash" || after === "archive") moveAway([thread], after, tx("OTP copied."));
    else if (after === "read") void actOnThreads([thread], "seen");
  };
  // While it's in the list, the list's row is the freshest, so it's what's kept for later.
  // Not an effect: this runs after the early return above, where no hook may be.
  if (openSummary && openRow.current && threadKey(openRow.current) === threadKey(openSummary)) {
    openRow.current = openSummary;
  }
  // It may have left this list while open, and then the row it was opened from knows if it's pinned or muted.
  const openMarked =
    openSummary ?? (open && openRow.current && threadKey(openRow.current) === threadKey(open) ? openRow.current : undefined);
  // It may have left this tab while still open, so its own messages have the last word.
  const openStarred = openSummary?.flagged ?? messages?.some((m) => m.flagged) ?? false;
  const viewFolder = view.kind === "folder" ? allFolders.find((f) => f.id === view.folderId) : undefined;
  const teamInboxShown =
    viewFolder?.role === "inbox" &&
    !!accounts.find((a) => a.id === viewFolder.account_id)?.prefs.team_shared;
  const tabsShown =
    !teamInboxShown && settings.smart_tabs === "on" && (view.kind === "all" || viewFolder?.role === "inbox");
  const openAccount = open ? accounts.find((a) => a.id === open.account_id) : undefined;
  // The same number the sidebar shows, whichever part of the list has loaded.
  const unreadHere =
    view.kind === "all"
      ? unreadIn(accounts.filter((a) => a.prefs.in_all_inbox).flatMap((a) => foldersBy[a.id] ?? []))
      : (viewFolder?.unread ?? 0);
  const listSub =
    view.kind === "search"
      ? threads.length > 0
        ? `${threads.length} found`
        : undefined
      : !tabsShown && unreadHere > 0
        ? `${unreadHere} unread`
        : undefined;

  // Inboxes that have been failing long enough that mail could be being missed.
  const staleInboxes = (health?.inboxes ?? []).filter(
    (i) =>
      i.problem &&
      !needsSignIn.includes(i.account_id) &&
      (i.last_ok === null || Date.now() / 1000 - i.last_ok > STALE_AFTER_SECONDS),
  );
  // An inbox that can't be reached isn't downloading anything.
  const working = (health?.inboxes ?? []).filter((i) => !i.problem);
  const downloadingFolders = working.flatMap((i) => i.folders);
  const mailTotal = downloadingFolders.reduce((sum, f) => sum + Math.max(f.server_total ?? 0, f.saved), 0);
  const mailToCome = downloadingFolders.reduce(
    (sum, f) => sum + Math.max(0, (f.server_total ?? f.saved) - f.saved) + f.bodies_to_come,
    0,
  );
  // An inbox in this view whose first download hasn't brought its inbox down yet.
  const viewAccountIds =
    view.kind === "all" ? accounts.map((a) => a.id) : viewFolder?.role === "inbox" ? [viewFolder.account_id] : [];
  // An inbox added since the last look at health hasn't brought anything down yet either.
  const notLookedAtYet = !!health && viewAccountIds.some((id) => !health.inboxes.some((i) => i.account_id === id));
  const newHere = notLookedAtYet || working.some((i) => {
    if (!viewAccountIds.includes(i.account_id)) return false;
    const inbox = i.folders.find((f) => f.role === "inbox");
    if (!inbox) return i.last_ok === null;
    const unfinished = inbox.last_synced_at === null || (inbox.server_total ?? 0) > inbox.saved;
    return unfinished && (i.last_ok === null || inbox.saved === 0);
  });
  // One count of the download: in the list while a new inbox's list is still blank, in the sidebar after.
  const download =
    newHere || mailToCome >= DOWNLOAD_SHOWN_FROM
      ? { done: mailTotal - mailToCome, total: mailTotal > 0 ? mailTotal : null }
      : null;
  const downloadInList = newHere && threads.length === 0;
  const lastUpdates = (health?.inboxes ?? []).flatMap((i) => i.last_ok ?? []);
  const refreshTip =
    refreshing
      ? tx("Checking for new mail")
      : lastUpdates.length > 0
        ? tx("Refresh mail · Updated {0}", { "0": timeAgo(Math.min(...lastUpdates)) })
        : tx("Refresh mail");

  const signInAgain = async (account: Account) => {
    setSigningIn((list) => [...list, account.id]);
    try {
      await api.mailSignIn(account.email, account.auth);
      setNeedsSignIn((list) => list.filter((x) => x !== account.id));
      void sync(account.id);
    } catch (e) {
      if (!(e instanceof ApiError && e.code === "cancelled")) setNotice(say(e));
    } finally {
      setSigningIn((list) => list.filter((x) => x !== account.id));
    }
  };

  const openMarks = open
    ? {
        pinned: openMarked?.pinned ?? false,
        muted: openMarked?.muted ?? false,
        snoozed: !!openSummary?.snooze_until,
        reminding: !!openSummary?.follow_at || !!openSummary?.follow_due,
        onPin: () => void pinThread(open, !(openMarked?.pinned ?? false)),
        onMute: () => void muteThread(open, !(openMarked?.muted ?? false)),
        onSnooze: (at: number | null) => void snoozeThread(open, at),
        onFollowUp: (at: number | null) => void followUpThread(open, at),
        needsYou: !!openMarked?.needs_you,
        onNeedsMe: openMarked && tabsShown ? () => void setNeedsMe([openMarked], !openMarked.needs_you) : undefined,
      }
    : undefined;

  const inboxFolderIds = () =>
    accounts
      .filter((a) => a.prefs.in_all_inbox && !a.prefs.team_shared)
      .flatMap((a) => foldersBy[a.id] ?? [])
      .filter((f) => f.role === "inbox")
      .map((f) => f.id);

  const replyInline = composing !== null && open !== null && inlineThread === threadKey(open);
  // Send and archive archives the conversation the reply belongs to, kept as it was when sending
  // started, even if another one has been opened while it went.
  const repliedTo = replyInline ? open : null;
  const composer =
    composing === null ? null : (
          <Composer
            key={composing}
            inline={replyInline}
            draftId={composing}
            accounts={accounts}
            initialProblem={reopenProblem}
            blankDrafts={blankDrafts.current}
            onClose={() => {
              setComposing(null);
              setReopenProblem(undefined);
              reloadDrafts();
            }}
            onInboxGone={() => {
              setComposing(null);
              setReopenProblem(undefined);
              reloadDrafts();
              setNotice(tx("That inbox is no longer shared with you, so the message you were writing went with it."));
            }}
            onQueue={queueSend}
            start={replyStart.current}
            startAgain={startAgain}
            onStarted={() => {
              replyStart.current = null;
            }}
            onSendDone={repliedTo ? () => moveAwayRef.current([repliedTo], "archive") : undefined}
            onSent={(warnings) => {
              setComposing(null);
              setReopenProblem(undefined);
              reloadDrafts();
              setNotice(warnings[0] ?? null);
              // The outgoing server may have just been found.
              void reloadAccounts();
              // The Sent copy is already in, so the reply shows in its conversation now.
              void reloadListRef.current();
              void sync();
            }}
            onOpenSettings={openInboxSettings}
            onLeft={reloadDrafts}
            onScheduled={(at) => {
              setComposing(null);
              setReopenProblem(undefined);
              reloadDrafts();
              setNotice(tx("Scheduled for {0}. It goes as long as this app is open then.", { "0": new Date(at * 1000).toLocaleString(language, { dateStyle: "medium", timeStyle: "short" }) }));
            }}
        />
    );

  const fullLayout = settings.layout === "full";

  /** A place from the trail, with the inbox it was in. A folder deleted since leaves All Inbox. */
  const showPlace = ({ view: to, account }: Place) => {
    if (account !== null && accounts?.some((a) => a.id === account)) setChosenAccount(account);
    if (to.kind === "folder") {
      const folder = allFolders.find((f) => f.id === to.folderId);
      if (!folder) return show({ kind: "all" }, false);
      setChosenAccount(folder.account_id);
    }
    show(to, false);
  };
  const here = (): Place => ({ view: viewRef.current, account: selected?.id ?? null });
  // In the full-width layout an open email covers the list, so back closes it first.
  const canGoBack = trail.back.length > 0 || (fullLayout && open !== null);
  const goBack = () => {
    if (fullLayout && openRef.current) return clearOpen();
    const to = trail.back[trail.back.length - 1];
    if (!to) return;
    const was = here();
    setTrail((t) => ({ back: t.back.slice(0, -1), ahead: was.view.kind === "search" ? t.ahead : [...t.ahead, was] }));
    showPlace(to);
  };
  const goForward = () => {
    const to = trail.ahead[trail.ahead.length - 1];
    if (!to) return;
    const was = here();
    setTrail((t) => ({ back: was.view.kind === "search" ? t.back : [...t.back, was], ahead: t.ahead.slice(0, -1) }));
    showPlace(to);
  };
  travelRef.current = { back: goBack, forward: goForward };
  const searchBox = (
    <div className="list-search">
      <SearchBox
        value={searchText}
        busy={searchNote.aiThinking || searchNote.checking}
        aiOn={hasAiKey && settings.ai_search === "on"}
        focusOnShow={searchText !== ""}
        inputRef={searchInput}
        onChange={changeSearch}
        onSubmit={() => {
          if (searchText.trim()) startSearch(searchText, true);
        }}
        onClear={clearSearch}
      />
    </div>
  );
  // The full-width layout has room for the search in the list's header.
  const aboveList = (withSearch: boolean) => (
    <>
      <UpdateBar />
      {accounts
        .filter((a) => needsSignIn.includes(a.id))
        .map((a) => (
          <div key={a.id} role="alert" className="signin-note">
            <span className="grow">{a.email} {" "}{tx("needs you to sign in again.")}</span>
            <button className="btn btn-secondary btn-sm" disabled={signingIn.includes(a.id)} onClick={() => void signInAgain(a)}>
              {signingIn.includes(a.id) ? tx("Finish in your browser…") : tx("Sign in again")}
            </button>
          </div>
        ))}
      {staleInboxes.map((i) => (
        <div key={i.account_id} role="alert" className="signin-note">
          <span className="grow">
            {i.email} {" "}{tx("isn't refreshing, so you may be missing new mail.")}</span>
          <button className="btn btn-secondary btn-sm" onClick={() => setSettingsPage("mail")}>
            {tx("See why")}</button>
        </div>
      ))}
      {fullLayout && view.kind === "all" && <TodayStrip />}
      {withSearch && searchBox}
      {tabsShown && <InboxTabs tab={tab} counts={tabCounts} onPick={pickTab} />}
      {tabsShown && tab === "needs_you" && (
        <Brief
          refreshKey={tabCounts.needs_you ?? 0}
          onDeadlines={() => show({ kind: "later", which: "deadlines" })}
          onNoReply={() => show({ kind: "later", which: "no_reply" })}
          onSpam={() => {
            const junk = allFolders.find((f) => f.role === "junk");
            if (junk) show({ kind: "folder", folderId: junk.id });
          }}
        />
      )}
      {tabsShown && tab === "rest" && (
        <RestBar
          count={threads.length}
          onCleanUp={() => show({ kind: "newsletters" })}
          onCountAll={countRest}
          onArchiveAll={archiveAllShown}
        />
      )}
      {teamInboxShown && <TeamTabs tab={teamTab} onPick={pickTeamTab} />}
      {view.kind === "search" && (
        <SearchNote
          summary={searchNote.summary}
          aiThinking={searchNote.aiThinking}
          problem={searchNote.problem}
          onExactWords={() => startSearch(searchText, true, false)}
        />
      )}
    </>
  );
  const searchTop = aboveList(true);

  // A message waiting out its undo time isn't a draft to the person.
  const unsentDrafts = drafts.filter((d) => !d.send_at && !waitingSends.some((w) => w.draftId === d.id));
  const scheduledDrafts = drafts
    .filter((d) => d.send_at)
    .sort((a, b) => (a.send_at ?? 0) - (b.send_at ?? 0));
  const pickedInboxes = [...new Set(pickedThreads.map((t) => t.account_id))];
  const pickedFolders = pickedThreads.map((t) => allFolders.find((f) => f.id === t.folder_id));

  /** Everything the command bar can do, right now. */
  const commands = (): Command[] => {
    const list: Command[] = [
      { id: "new", label: tx("New message"), keys: "⌘N", section: "Mail", run: () => void compose("new", null) },
      { id: "all", label: tx("Go to All Inbox"), keys: "⌘1", section: tx("Go to"), run: () => show({ kind: "all" }) },
      { id: "ask", label: tx("Ask your email"), section: tx("Go to"), run: () => show({ kind: "ask" }) },
      { id: "calendar", label: tx("Go to Calendar"), section: tx("Go to"), run: () => show({ kind: "calendar" }) },
      { id: "contacts", label: tx("Go to Contacts"), section: tx("Go to"), run: () => show({ kind: "contacts" }) },
      { id: "newsletters", label: tx("Go to Newsletters"), section: tx("Go to"), run: () => show({ kind: "newsletters" }) },
      { id: "snoozed", label: tx("Go to Snoozed"), section: tx("Go to"), run: () => show({ kind: "later", which: "snoozed" }) },
      { id: "reminders", label: tx("Go to Reminders"), section: tx("Go to"), run: () => show({ kind: "later", which: "reminders" }) },
      { id: "muted", label: tx("Go to Muted"), section: tx("Go to"), run: () => show({ kind: "later", which: "muted" }) },
      { id: "scheduled", label: tx("Go to Scheduled"), section: tx("Go to"), run: () => show({ kind: "scheduled" }) },
      { id: "screener", label: tx("Go to New senders"), section: tx("Go to"), run: () => show({ kind: "screener" }) },
      { id: "purchases", label: tx("Go to Purchases"), section: tx("Go to"), run: () => show({ kind: "later", which: "purchases" }) },
      { id: "deadlines", label: tx("Go to Deadlines"), section: tx("Go to"), run: () => show({ kind: "later", which: "deadlines" }) },
      { id: "no_reply", label: tx("Go to No reply yet"), section: tx("Go to"), run: () => show({ kind: "later", which: "no_reply" }) },
      { id: "refresh", label: tx("Refresh mail"), section: "Mail", run: refreshNow },
      { id: "sidebar", label: tx("Collapse or expand the sidebar"), keys: "⌘\\", section: tx("Go to"), run: () => setRail((on) => !on) },
      { id: "settings", label: tx("Open Settings"), keys: "⌘,", section: "Settings", run: () => setSettingsPage("general") },
      { id: "rules", label: tx("Open Filters"), section: "Settings", run: () => setSettingsPage("rules") },
      { id: "health", label: tx("Open Storage"), section: "Settings", run: () => setSettingsPage("mail") },
      { id: "keys", label: tx("Show keyboard shortcuts"), section: "Settings", run: () => setSettingsPage("shortcuts") },
    ];
    for (const a of accounts) {
      list.push({
        id: `inbox-${a.id}`,
        label: tx("Go to {0}", { "0": a.prefs.label || a.email }),
        section: tx("Go to"),
        run: () => pickAccount(a.id),
      });
    }
    for (const f of selected ? (foldersBy[selected.id] ?? []) : []) {
      list.push({
        id: `folder-${f.id}`,
        label: tx("Go to {0}", { "0": f.name }),
        section: tx("Go to"),
        run: () => show({ kind: "folder", folderId: f.id }),
      });
    }
    if (open && messages && messages.length > 0 && openMarks) {
      const last = messages[messages.length - 1];
      list.unshift(
        { id: "reply", label: tx("Reply"), keys: "R", section: tx("This email"), run: () => void compose("reply", last) },
        { id: "reply-all", label: tx("Reply all"), keys: "A", section: tx("This email"), run: () => void compose("reply_all", last) },
        { id: "forward", label: tx("Forward"), keys: "F", section: tx("This email"), run: () => void compose("forward", last) },
        { id: "archive", label: tx("Archive"), keys: "E", section: tx("This email"), run: () => void leaveRef.current("archive") },
        { id: "delete", label: tx("Delete"), keys: "#", section: tx("This email"), run: () => void leaveRef.current("trash") },
        { id: "snooze", label: tx("Snooze until tomorrow morning"), keys: "H", section: tx("This email"), run: () => {
          const when = whenChoices().find((c) => c.label === tx("Tomorrow morning"));
          if (when) openMarks.onSnooze(when.at);
        } },
        { id: "pin", label: openMarks.pinned ? "Unpin" : tx("Pin to the top"), keys: "P", section: tx("This email"), run: openMarks.onPin },
        { id: "mute", label: openMarks.muted ? "Unmute" : tx("Mute conversation"), keys: "M", section: tx("This email"), run: openMarks.onMute },
        { id: "find", label: tx("Find in this conversation"), section: tx("This email"), run: () => setFindRequest((n) => n + 1) },
        { id: "print", label: tx("Print"), section: tx("This email"), run: () => setPrinting(messages) },
      );
    }
    return list;
  };

  const overlays = (
    <>
      {pending[0] && (
        <ApproveDevice
          key={pending[0].id}
          device={pending[0]}
          onDone={() => setPending((p) => p.slice(1))}
        />
      )}
      {settingsPage && (
        <SettingsWindow
          page={settingsPage}
          email={auth.email ?? ""}
          onSetUpAccount={onSetUpAccount}
          accounts={accounts}
          foldersBy={foldersBy}
          onPage={setSettingsPage}
          onClose={() => {
            setSettingsPage(null);
            void reloadList().catch(() => {});
          }}
          onTeamChanged={() => setTeamEpoch((n) => n + 1)}
          onAccountsChanged={() => {
            void reloadAccounts();
            void reloadList().catch(() => {});
          }}
          onOpenThread={({ account_id, thread_id, folder_id }) => {
            if (account_id === null || thread_id === null || folder_id === null) return;
            setSettingsPage(null);
            pickThread({ account_id, thread_id, folder_id, unread_count: 0 } as ThreadSummary);
          }}
          onSaveDefaultInbox={(id) => void saveDefaultInbox(id)}
          onDisconnect={(id) => {
            setSettingsPage(null);
            void removeAccount(id);
          }}
          onConnect={() => {
            addingFromSettings.current = settingsPage;
            setSettingsPage(null);
            setAdding(true);
          }}
          onSignedOut={() => {
            setSettingsPage(null);
            onSignedOut();
          }}
          onAiChanged={reloadAiStatus}
          onFiltersChanged={() => {
            void reloadList().catch(() => {});
            void sync();
          }}
          onMailCleared={() => {
            clearOpen();
            void reloadList().catch(() => {});
            void sync();
          }}
        />
      )}
      {composing !== null && !replyInline && composer}
      {commandOpen && (
        <CommandBar
          commands={commands()}
          onClose={() => setCommandOpen(false)}
          onSearch={(text) => {
            setSearchText(text);
            startSearch(text, true);
          }}
          onAsk={paid && settings.ai_chat === "on" ? setAsking : undefined}
          askLocked={!paid && settings.ai_chat === "on"}
        />
      )}
      {asking && (
        <AskAnswer
          question={asking}
          filters={searchText.trim() === asking.trim() ? searchFilters.current : null}
          onClose={() => setAsking(null)}
          onOpen={(s) => pickThread({ ...s, unread_count: 0 } as ThreadSummary)}
        />
      )}
      {printing && (
        <PrintSheet messages={printing} subject={printing[0]?.subject ?? ""} onDone={() => setPrinting(null)} />
      )}
      {pickFor && (
        <PickTime
          title={pickFor.kind === "snooze" ? tx("Snooze until") : tx("Remind me if there's no reply by")}
          confirm={pickFor.kind === "snooze" ? tx("Snooze") : tx("Remind me")}
          onClose={() => setPickFor(null)}
          onPick={(at) => {
            const { thread, kind } = pickFor;
            setPickFor(null);
            void (kind === "snooze" ? snoozeThread(thread, at) : followUpThread(thread, at));
          }}
        />
      )}
      {rowMenu && (() => {
        const many = picked.has(threadKey(rowMenu.thread)) && picked.size > 1;
        const chosen = many ? pickedThreads : [rowMenu.thread];
        const inboxes = new Set(chosen.map((t) => t.account_id));
        const roles = chosen.map((t) => allFolders.find((f) => f.id === t.folder_id)?.role);
        return (
          <ThreadMenu
            key={threadKey(rowMenu.thread)}
            x={rowMenu.x}
            y={rowMenu.y}
            thread={rowMenu.thread}
            isPicked={picked.has(threadKey(rowMenu.thread))}
            allPicked={picked.size === threads.length}
            selectedCount={chosen.length}
            folders={inboxes.size === 1 ? (foldersBy[chosen[0].account_id] ?? null) : null}
            inBin={roles.every((r) => r === "trash" || r === "junk")}
            inJunk={roles.every((r) => r === "junk")}
            inArchive={roles.every((r) => r === "archive")}
            canGoToInbox={roles.every(canGoToInbox)}
            onAction={(action) => void onRowAction(rowMenu.thread, action)}
            onClose={() => setRowMenu(null)}
          />
        );
      })()}
      {deleting && (
        <Confirm
          title={
            deleting.length === 1
              ? tx("Delete this conversation for good?")
              : `Delete ${deleting.length} conversations for good?`
          }
          confirm={tx("Yes, delete for good")}
          keep={tx("Keep")}
          danger
          onKeep={() => setDeleting(null)}
          onConfirm={() => {
            const chosen = deleting;
            setDeleting(null);
            void actOnThreads(chosen, "delete_forever");
          }}
        >
          {tx("This can't be undone.")}</Confirm>
      )}
      {markingCount !== null && (
        <div role="status" className="toast">
          <Spinner size="xs" />
          <span className="grow tnum">
            {tx("Marking as read… {n}", { n: markingCount.toLocaleString(language) })}
          </span>
        </div>
      )}
      {allRead && (
        <AllReadToast count={allRead.length} onUndo={() => void undoAllRead()} onDismiss={() => setAllRead(null)} />
      )}
      {moves.waiting && (
        <MoveToast
          move={moves.waiting}
          onUndo={() => {
            moves.undo();
            void reloadList().catch(() => {});
            accountsRef.current?.forEach((a) => void reloadFolders(a.id).catch(() => {}));
          }}
          onDismiss={() => void moves.moveNow()}
        />
      )}
      {waitingSends.map((w) => (
        <SendingToast
          key={w.draftId}
          from={w.from}
          sendAt={w.sendAt}
          clock={clock}
          onUndo={() => undoSend(w.draftId)}
          onSendNow={() => void sendWaiting(w.draftId)}
        />
      ))}
    </>
  );

  if (accounts.length === 0 || adding) {
    // The same outer element as the main screen, with the overlays first in both, so Settings
    // and the rest stay as they are when the first inbox arrives (such as on joining a team)
    // rather than starting again from the top.
    return (
      <TagsContext.Provider value={tags}>
        {overlays}
        <AddAccount
          onCancel={
            accounts.length > 0
              ? () => {
                  setAdding(false);
                  setSettingsPage(addingFromSettings.current);
                  addingFromSettings.current = null;
                }
              : undefined
          }
          startEmail={accounts.length === 0 ? auth.email : undefined}
          intro={
            accounts.length === 0 && auth.email
              ? tx("You're signed in as {0}. Connect it to see your mail here, or use a different address.", { "0": auth.email })
              : undefined
          }
          onAdded={(a) => {
            // Signing in again on an inbox already here replaces it.
            setAccounts([...accounts.filter((x) => x.id !== a.id), a]);
            setNeedsSignIn((list) => list.filter((x) => x !== a.id));
            // So its first download shows as "Getting your mail", not as an empty inbox.
            void reloadHealth(true);
            setAdding(false);
            addingFromSettings.current = null;
          }}
        >
          {accounts.length === 0 && (
            <>
              <button className="link-btn" onClick={() => setSettingsPage("team")}>
                {tx("Join a team")}</button>
              <button className="link-btn" onClick={() => setSettingsPage("account")}>
                {tx("Your account")}</button>
            </>
          )}
        </AddAccount>
      </TagsContext.Provider>
    );
  }

  return (
    <TagsContext.Provider value={tags}>
      {overlays}
      {notice && (
        <div role="status" className="toast">
          <Icon icon={AlertCircleIcon} size="s" />
          <span className="grow">{notice}</span>
          <IconButton icon={Cancel01Icon} label={tx("Dismiss")} small onClick={() => setNotice(null)} />
        </div>
      )}
      <div
        className="app"
        data-tauri-drag-region
        data-pane={phoneNav ? "nav" : picked.size === 0 && open ? "reader" : "list"}
        data-picking={picked.size > 0 || undefined}
        data-rail={rail || undefined}
        data-layout={settings.layout === "full" ? "full" : undefined}
      >
        <Sidebar
          rail={rail}
          onToggleRail={() => setRail((on) => !on)}
          onBack={canGoBack ? goBack : undefined}
          onForward={trail.ahead.length > 0 ? goForward : undefined}
          accounts={accounts}
          selected={selected}
          foldersBy={foldersBy}
          currentFolderId={view.kind === "folder" ? view.folderId : null}
          showingAll={view.kind === "all"}
          onShowAll={() => {
            setPhoneNav(false);
            show({ kind: "all" });
          }}
          onPickAccount={(id) => {
            setPhoneNav(false);
            pickAccount(id);
          }}
          onPickFolder={(id) => {
            setPhoneNav(false);
            show({ kind: "folder", folderId: id });
          }}
          onChosen={() => setPhoneNav(false)}
          onAddAccount={() => setAdding(true)}
          onFolderDeleted={(folder, moved) => {
            if (view.kind === "folder" && view.folderId === folder.id) show({ kind: "all" });
            void reloadFolders(folder.account_id);
            void sync(folder.account_id);
            setNotice(moved > 0 ? (moved === 1 ? tx("1 email moved to Archive.") : tx("{moved} emails moved to Archive.", { moved })) : null);
          }}
          onFolderAdded={(folder) => {
            void reloadFolders(folder.account_id).then(() => show({ kind: "folder", folderId: folder.id }));
          }}
          onCompose={() => void compose("new", null)}
          onOpenSettings={() => setSettingsPage("general")}
          draftsFor={(id) => unsentDrafts.filter((d) => d.account_id === id).length}
          showingDrafts={view.kind === "drafts"}
          onShowDrafts={() => show({ kind: "drafts" })}
          download={downloadInList ? null : download}
          extras={[
            { id: "ask", label: tx("Ask AI"), tag: tx("Beta"), icon: AiChat02Icon, count: 0, current: view.kind === "ask", onClick: () => show({ kind: "ask" }) },
            { id: "calendar", label: tx("Calendar"), icon: Calendar03Icon, more: true, count: 0, current: view.kind === "calendar", onClick: () => show({ kind: "calendar" }) },
            { id: "contacts", label: tx("Contacts"), icon: ContactBookIcon, more: true, count: 0, current: view.kind === "contacts", onClick: () => show({ kind: "contacts" }) },
            { id: "newsletters", label: tx("Newsletters"), icon: News01Icon, count: 0, current: view.kind === "newsletters", onClick: () => show({ kind: "newsletters" }) },
            { id: "purchases", label: tx("Purchases"), icon: ShoppingBag01Icon, count: 0, current: view.kind === "later" && view.which === "purchases", onClick: () => show({ kind: "later", which: "purchases" }) },
            ...(laterCounts.screener > 0
              ? [{ id: "screener", label: tx("New senders"), icon: UserAdd01Icon, count: laterCounts.screener, current: view.kind === "screener", onClick: () => show({ kind: "screener" }) }]
              : []),
            ...(laterCounts.snoozed > 0
              ? [{ id: "snoozed", label: tx("Snoozed"), icon: Clock01Icon, count: laterCounts.snoozed, current: view.kind === "later" && view.which === "snoozed", onClick: () => show({ kind: "later", which: "snoozed" }) }]
              : []),
            ...(laterCounts.reminders > 0
              ? [{ id: "reminders", label: tx("Reminders"), icon: AlarmClockIcon, count: laterCounts.reminders, current: view.kind === "later" && view.which === "reminders", onClick: () => show({ kind: "later", which: "reminders" }) }]
              : []),
            ...(scheduledDrafts.length > 0
              ? [{ id: "scheduled", label: tx("Scheduled"), icon: Clock01Icon, count: scheduledDrafts.length, current: view.kind === "scheduled", onClick: () => show({ kind: "scheduled" }) }]
              : []),
            ...(laterCounts.muted > 0
              ? [{ id: "muted", label: tx("Muted"), icon: VolumeOffIcon, count: laterCounts.muted, current: view.kind === "later" && view.which === "muted", onClick: () => show({ kind: "later", which: "muted" }) }]
              : []),
            ...tags
              .filter((t) => t.count > 0)
              .map((t) => ({
                id: `tag:${t.id}`,
                label: t.name,
                icon: Tag01Icon,
                colour: t.colour,
                count: t.count,
                current: view.kind === "tag" && view.tagId === t.id,
                onClick: () => show({ kind: "tag", tagId: t.id, name: t.name }),
              })),
          ]}
        />
        <div className="main-panel">
        {view.kind === "newsletters" ? (
          <Newsletters
            accounts={accounts}
            signInNeeded={needsSignIn}
            place={view.place}
            searchRef={searchInput}
            onNotice={setNotice}
            onPhoneNav={() => setPhoneNav(true)}
            inboxFolder={(accountId) => allFolders.find((f) => f.role === "inbox" && f.account_id === accountId)?.id}
            onOpen={(sender, place) => {
              // Kept with where they were, so going back returns to the same tab and spot.
              viewRef.current = { kind: "newsletters", place };
              show({ kind: "from", folderId: sender.folder_id, address: sender.addresses[0], title: sender.name, screened: false, with: { account: sender.account_id, addresses: sender.addresses }, back: { kind: "newsletters", place } })
            }}
          />
        ) : view.kind === "calendar" ? (
          <CalendarView onPhoneNav={() => setPhoneNav(true)} />
        ) : view.kind === "ask" ? (
          <AskChat
            key={view.about ? `${view.about.account_id}:${view.about.thread_id}` : "everything"}
            about={view.about ?? null}
            onClearAbout={() => show({ kind: "ask" })}
            accounts={accounts}
            onPhoneNav={() => setPhoneNav(true)}
            onOpenAiSettings={() => setSettingsPage("ai")}
            onOpen={(s) => {
              // An event opens the calendar; an email opens where it's filed. Back returns to the chat.
              if (s.source_type === "event") {
                show({ kind: "calendar" });
                return;
              }
              show({ kind: "folder", folderId: s.folder_id });
              pickThread({ ...s, unread_count: 0 } as unknown as ThreadSummary);
            }}
          />
        ) : view.kind === "contacts" ? (
          <ContactsView
            onPhoneNav={() => setPhoneNav(true)}
            onWrite={(address) => void writeTo(address, "")}
            onEmailsWith={(addresses, name) => {
              const inbox = allFolders.find((f) => f.role === "inbox" && f.account_id === selected?.id) ?? allFolders.find((f) => f.role === "inbox");
              if (inbox) show({ kind: "from", folderId: inbox.id, address: addresses[0], title: name, screened: false, with: { account: null, addresses }, back: { kind: "contacts" } });
            }}
          />
        ) : (
        <>
        {view.kind === "drafts" ? (
          <DraftList
            top={searchTop}
            onPhoneNav={() => setPhoneNav(true)}
            drafts={unsentDrafts.filter((d) => d.account_id === selected?.id)}
            onOpen={openDraft}
            onDiscard={discardDraft}
          />
        ) : view.kind === "scheduled" ? (
          <ScheduledList
            top={searchTop}
            onPhoneNav={() => setPhoneNav(true)}
            drafts={scheduledDrafts}
            onOpen={(id) => {
              // Taken off the schedule while it's being changed, so it can't go half-edited.
              void api
                .unscheduleDraft(id)
                .then(() => {
                  reloadDrafts();
                  setNotice(tx("Taken off the schedule while you change it. Send or schedule it again when you're done."));
                  openDraft(id);
                })
                .catch((e) => setNotice(say(e)));
            }}
          />
        ) : view.kind === "screener" ? (
          <ScreenerList
            top={searchTop}
            onPhoneNav={() => setPhoneNav(true)}
            onChanged={() => {
              reloadLater();
              accountsRef.current?.forEach((a) => void reloadFolders(a.id).catch(() => {}));
            }}
            onOpen={(row) =>
              show({ kind: "from", folderId: row.folder_id, address: row.address, title: row.name || row.address, screened: true, back: { kind: "screener" } })
            }
          />
        ) : view.kind === "senders" ? (
          <SenderList
            top={searchTop}
            title={view.title}
            senders={senders}
            onBack={() => show(view.back)}
            onOpen={(row) =>
              show({ kind: "from", folderId: row.folder_id, address: row.address, title: row.name || row.address, screened: false, back: view })
            }
          />
        ) : (
          <ThreadList
            top={fullLayout ? aboveList(false) : searchTop}
            search={
              <>
                {fullLayout && searchBox}
                {view.kind === "search" && accounts.length > 1 && (
                  <Choice
                    label={tx("Look in")}
                    className="list-filter list-filter-shrinks"
                    value={String(searchIn ?? "all")}
                    options={[
                      { value: "all", label: tx("All inboxes") },
                      ...accounts.map((a) => ({ value: String(a.id), label: a.prefs.label || a.email })),
                    ]}
                    onChange={(v) => startSearch(searchText, true, true, v === "all" ? null : Number(v))}
                  />
                )}
              </>
            }
            oneLine={fullLayout}
            title={
              view.kind === "search"
                ? tx("Results")
                : view.kind === "all"
                  ? tx("All Inbox")
                  : view.kind === "later"
                    ? LATER_TITLES[view.which]
                    : view.kind === "tag"
                      ? view.name
                    : view.kind === "from"
                      ? view.title
                      : (viewFolder ? folderLabel(viewFolder) : tx("Inbox"))
            }
            loading={listLoading}
            unreadOnly={unreadOnly}
            onToggleUnread={view.kind === "all" || view.kind === "folder" ? toggleUnread : undefined}
            onMarkAllRead={view.kind === "all" || view.kind === "folder" ? () => void markAllRead() : undefined}
            foot={
              view.kind === "search" ? (
                <SearchFoot
                  checking={searchNote.checking}
                  unreached={searchNote.unreached}
                  inBin={searchNote.inBin}
                  byMeaning={searchNote.byMeaning}
                  onShowBin={() => {
                    const filters = searchFilters.current;
                    if (filters) void searchWithFilters({ ...filters, include_bin: true });
                  }}
                />
              ) : undefined
            }
            onBack={view.kind === "from" ? () => show(view.back) : undefined}
            onPhoneNav={() => setPhoneNav(true)}
            onNearEnd={growList}
            onBySender={
              view.kind === "all" || viewFolder?.role === "inbox"
                ? () =>
                    show({
                      kind: "senders",
                      folderIds: view.kind === "folder" ? [view.folderId] : inboxFolderIds(),
                      title: view.kind === "all" ? tx("All Inbox") : (viewFolder ? folderLabel(viewFolder) : tx("Inbox")),
                      back: view,
                    })
                : undefined
            }
            sub={listSub}
            firstLoad={
              downloadInList && download
                ? {
                    ...download,
                    // Counted again often while it's showing, so the number moves.
                    refresh: () => {
                      healthAsked.current = 0;
                      void reloadHealth();
                    },
                  }
                : null
            }
            emptyTitle={
              view.kind === "search"
                ? tx("No emails found")
                : tabsShown && tab === "needs_you"
                  ? tx("Nothing needs you right now")
                  : tabsShown
                    ? tx("Nothing on this tab")
                    : undefined
            }
            threads={threads}
            selected={open ? threadKey(open) : null}
            showPictures={
              viewFolder?.role !== "junk" &&
              !searchFilters.current?.include_bin &&
              settings.list_pictures === "on"
            }
            refreshing={refreshing}
            refreshTip={refreshTip}
            onSync={refreshNow}
            onPick={pickThread}
            onPeek={(t) => readThreadsAhead([t])}
            picked={picked}
            onTogglePicked={togglePicked}
            onToggleAll={() => {
              if (picked.size === threads.length) {
                setPicked(new Set());
                pickAnchor.current = null;
              } else {
                selectAll();
              }
            }}
            onRowMenu={(thread, x, y) => setRowMenu({ thread, x, y })}
            onCopyCode={afterCodeCopied}
            onQuick={
              viewFolder?.role === "trash" || viewFolder?.role === "junk"
                ? undefined
                : (thread, to) =>
                    void (to === "done"
                      ? setNeedsMe([thread], false)
                      : onRowAction(thread, { kind: "move", to }))
            }
            menuFor={rowMenu ? threadKey(rowMenu.thread) : null}
            drafts={
              viewFolder?.role === "drafts"
                ? unsentDrafts.filter((d) => d.account_id === viewFolder.account_id)
                : []
            }
            onOpenDraft={openDraft}
            onDiscardDraft={discardDraft}
          />
        )}
        <section aria-label={tx("Conversation")} className="pane-reader">
          {picked.size > 0 ? (
            <BulkPanel
              count={picked.size}
              sameInboxFolders={
                pickedInboxes.length === 1 ? (foldersBy[pickedInboxes[0]] ?? null) : null
              }
              inBin={pickedFolders.every((f) => f?.role === "trash" || f?.role === "junk")}
              inJunk={pickedFolders.every((f) => f?.role === "junk")}
              canGoToInbox={pickedFolders.every((f) => canGoToInbox(f?.role))}
              onSpam={(spam) => void markSpam(pickedThreads, spam)}
              onAction={(action) => void actOnPicked(action)}
              onMove={(to) => void actOnPicked("move", to)}
              onClear={() => setPicked(new Set())}
              onDone={
                pickedThreads.some((t) => t.needs_you)
                  ? () => {
                      const done = pickedThreads.filter((t) => t.needs_you);
                      setPicked(new Set());
                      void setNeedsMe(done, false);
                    }
                  : undefined
              }
            />
          ) : view.kind === "search" && !open && settings.ai_chat === "on" ? (
            <AskChat
              key={`search:${searchIn}`}
              search={{ account: searchIn }}
              accounts={accounts}
              onOpenAiSettings={() => setSettingsPage("ai")}
              onOpen={(s) => {
                setFromAnswer(true);
                pickThread({ ...s, unread_count: 0 } as unknown as ThreadSummary);
              }}
            />
          ) : (
          <Reader
            key={open ? threadKey(open) : "none"}
            onBackToAnswer={
              fromAnswer && view.kind === "search"
                ? () => {
                    setFromAnswer(false);
                    clearOpen();
                  }
                : undefined
            }
            code={openMarked?.code}
            onAskAbout={
              openMarked
                ? () =>
                    show({
                      kind: "ask",
                      about: { account_id: openMarked.account_id, thread_id: openMarked.thread_id, label: lead(openMarked) },
                    })
                : undefined
            }
            onCopyCode={() => openMarked && afterCodeCopied(openMarked)}
            messages={messages}
            loading={open !== null}
            folder={openFolder}
            folders={openFolders}
            accountEmail={
              accounts.length > 1 && open && view.kind === "all"
                ? accounts.find((a) => a.id === open.account_id)?.email
                : undefined
            }
            onGoToInbox={() => open && pickAccount(open.account_id)}
            starred={openStarred}
            onCompose={(mode, m) => void compose(mode, m)}
            replyStarting={replyStarting}
            onReplyWith={(m, start) => {
              replyStart.current = start;
              void compose(settings.reply_default === "reply_all" ? "reply_all" : "reply", m);
            }}
            needsYou={openSummary?.needs_you ?? false}
            onArchive={() => leaveTo("archive")}
            onDelete={() => leaveTo("trash")}
            onDeleteForever={() => open && void changeInBackground([open], () => null, () => api.deleteForever(open.account_id, open.thread_id, open.folder_id))}
            onMove={leaveTo}
            settingsOpen={settingsPage !== null}
            replyBox={replyInline ? composer : null}
            marks={openMarks}
            onPhoneBack={clearOpen}
            ai={
              hasAiKey
                ? {
                    summaries: settings.ai_summaries === "on",
                    translate: settings.ai_translate === "on",
                    language: myLanguage(),
                  }
                : undefined
            }
            onPrint={() => messages && setPrinting(messages)}
            onTags={() => setTaggingOpen(true)}
            canFollowUp={(() => {
              const newest = messages?.[messages.length - 1];
              const mine = [openAccount?.email, ...(openAccount?.prefs.aliases ?? []).map((x) => x.address)];
              return !!newest && mine.some((m) => m?.toLowerCase() === newest.from.address.toLowerCase());
            })()}
            tags={openSummary?.tags}
            findRequest={findRequest}
            team={
              open && openAccount?.prefs.team_shared
                ? {
                    thread: { account_id: open.account_id, thread_id: open.thread_id },
                    tick: teamTick,
                    replying: replyInline,
                    onChanged: () => void reloadList().catch(() => {}),
                  }
                : null
            }
            onSpam={() => void markSpam(openThreadSummary(), true)}
            onNotSpam={() => void markSpam(openThreadSummary(), false)}
            onUnsubscribe={(m) => void unsubscribe(m)}
            unread={(openSummary?.unread_count ?? 0) > 0}
            replyMode={settings.reply_default === "reply_all" ? "reply_all" : "reply"}
            onMarkRead={() => open && markThreadRead(open)}
            onMarkUnread={() => open && void markThreadUnread(open)}
            onStar={() => open && void starThread(open, !openStarred)}
          />
          )}
        </section>
        </>
        )}
        </div>
      </div>
      <McpSendApproval onSent={() => void reloadListRef.current()} />
      {taggingOpen && open && (
        <TagPicker
          thread={{ account_id: open.account_id, thread_id: open.thread_id, tags: openSummary?.tags }}
          onClose={() => setTaggingOpen(false)}
          onChanged={() => {
            reloadTags();
            void reloadList().catch(() => {});
          }}
        />
      )}
    </TagsContext.Provider>
  );
}
