import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Account, AccountPrefs, Body, SearchFilters, SearchReply, Contact, Draft, Folder, Message, SyncStats, SharedStuff, Tag, ThreadSummary, ThreadTag } from "./types";
import { tx } from "./i18n";

/** Follow-ups that send themselves after an email until someone replies. */
export type FollowUpStep = { days: number; body: string; ai: boolean; at?: number };
export type FollowUps = { id: number; name: string; steps: FollowUpStep[] };
export type FollowUpRun = {
  id: number;
  sequence_id: number | null;
  sequence_name: string;
  account_id: number;
  thread_id: string;
  message_id: number;
  sent: number;
  steps: number;
  next_at: number | null;
  state: "running" | "replied" | "finished" | "stopped" | "failed";
  problem: string | null;
  subject: string | null;
};

/** How the person writes, which drafts follow. */
export type StyleProfile = { text: string; made_at: number; edited: boolean };

/** What an email means and what its sender wants, in plain words. */
export type Explained = { meaning: string; wants: string; tone: string; watch_out: string };

export class ApiError extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    // The engine writes in English; the person reads it in their own language.
    super(tx(message));
  }
}

const inMacApp = "__TAURI_INTERNALS__" in window;

// Links on the app's own screens that open outside it (setup steps, plan pages) go to the
// browser through the app, so a click never quietly does nothing. Email bodies are in their
// own frame and handled there.
if (inMacApp) {
  document.addEventListener(
    "click",
    (e) => {
      if (e.defaultPrevented || e.button !== 0) return;
      const link = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!link || link.target !== "_blank" || !/^(https?|mailto):/i.test(link.href)) return;
      e.preventDefault();
      void invoke("open_link", { url: link.href }).catch(() => {});
    },
    true,
  );
}
/** In a browser, where files are downloaded rather than saved by the app. */
export const onTheWeb = !inMacApp;

/** The app can be locked behind Touch ID or the Mac's password. Only in the Mac app. */
export const canLock = inMacApp;

/** Asks the Mac to check it's its owner. True when they pass; an error if it can't check. */
export async function proveItsMe(reason: string): Promise<boolean> {
  if (!inMacApp) throw new ApiError("unsupported", tx("Locking only works in the Mac app."));
  try {
    return await invoke<boolean>("prove_its_me", { reason });
  } catch (e) {
    throw new ApiError("unsupported", String(e));
  }
}

/** Whether the app is locked now. The lock is kept by the app, not this page. */
export async function appLocked(): Promise<boolean> {
  if (!canLock) return false;
  try {
    return (await call<{ locked: boolean }>("app_lock_state")).locked;
  } catch {
    return true;
  }
}

/** Unlocks after Touch ID or the Mac's password. A page can ask to lock, but only this unlocks. */
export async function unlockApp(reason: string): Promise<boolean> {
  if (!inMacApp) throw new ApiError("unsupported", tx("Locking only works in the Mac app."));
  try {
    return await invoke<boolean>("unlock_app", { reason });
  } catch (e) {
    throw new ApiError("unsupported", String(e));
  }
}

/** Things only the Mac app can do, such as its Dock icon. Does nothing elsewhere. */
export async function native(command: string, args: object): Promise<void> {
  if (inMacApp) await invoke(command, { ...args }).catch(() => {});
}

/**
 * When this window comes to the front or loses it. The page's own focus events won't do, as
 * they also fire for clicks inside an email and then miss the window losing focus.
 */
/** An email exactly as it came from the server, and what the person's provider found. */
export type MessageSource = {
  text: string;
  cut_short: boolean;
  message_id: string | null;
  spf: string | null;
  dkim: string | null;
  dmarc: string | null;
};

export function onWindowFocus(handler: (focused: boolean) => void): () => void {
  if (inMacApp) {
    const listening = getCurrentWindow().onFocusChanged(({ payload }) => handler(payload));
    return () => void listening.then((stop) => stop());
  }
  const gained = () => handler(true);
  const lost = () => handler(false);
  window.addEventListener("focus", gained);
  window.addEventListener("blur", lost);
  return () => {
    window.removeEventListener("focus", gained);
    window.removeEventListener("blur", lost);
  };
}

type PanelNews =
  | "mail-changed"
  /** From the menu bar panel: unread mail moved, waiting out its Undo, and that Undo. */
  | "mail-leaving"
  | "mail-staying"
  /** From the menu bar panel: Mark all as read was pressed, on these or on all of the inbox. */
  | "mail-read"
  /** From the app itself: new mail is saved, as its banner shows. */
  | "new-mail"
  | "open-thread"
  | "open-settings"
  | "sign-in-changed"
  | "settings-changed"
  | "before-restart"
  | "ready-to-restart"
  /** From the app itself: the lock was put on or taken off. */
  | "lock-changed"
  /** From the menu bar panel: show the unread email. */
  | "show-unread"
  | "banner-clicked"
  /** From a mouse's side buttons: "back" or "forward". */
  | "travel"
  | "unread-changed";

/** Copies text through macOS, which works from any window and any click; in a browser, the page's own way. */
export async function copyText(text: string): Promise<void> {
  if (!inMacApp) return navigator.clipboard.writeText(text);
  if (!(await invoke<boolean>("copy_text", { text }))) throw new Error("Couldn't copy that.");
}

/** The conversation a clicked banner was about, told only once. */
export async function clickedBanner(): Promise<ThreadSummary | null> {
  if (!inMacApp) return null;
  const clicked = await invoke<[number, string] | null>("clicked_banner");
  return clicked && api.threadSummary(clicked[0], clicked[1]);
}

/** Lets the main window know what happened in the menu bar panel. */
export function tellMainWindow(news: PanelNews, details?: unknown) {
  if (inMacApp) void emitTo("main", news, details).catch(() => {});
}

/** Lets the panel know at once, even while it's hidden, so it never opens on what it shouldn't. */
export function tellPanel(news: PanelNews, details?: unknown) {
  if (inMacApp) void emitTo("panel", news, details).catch(() => {});
}

/** The main window's side of that. Returns how to stop listening. */
export function hearFromPanel<T>(news: PanelNews, handler: (details: T) => void): () => void {
  if (!inMacApp) return () => {};
  const listening = listen<T>(news, (event) => handler(event.payload));
  return () => void listening.then((stop) => stop());
}

/** What must be finished before the app restarts for an update, in this window. */
const beforeRestart = new Set<() => Promise<unknown>>();

/** Messages on their way to the mail server from this window, however they were sent. */
const sendsGoing = new Set<Promise<unknown>>();

/** Keeps count of a send until it settles, so a restart never cuts one off half way. */
function trackSend<T>(send: Promise<T>): Promise<T> {
  sendsGoing.add(send);
  void send.catch(() => {}).finally(() => sendsGoing.delete(send));
  return send;
}

/**
 * Asks to be run before an update restarts the app, such as to send a message still waiting out
 * its undo time. Restarting doesn't run anything of this window's on its own way out, so without
 * this the message would quietly stay a draft. Returns how to stop.
 */
export function onBeforeRestart(finish: () => Promise<unknown>): () => void {
  beforeRestart.add(finish);
  return () => void beforeRestart.delete(finish);
}

/** How long saving and the waiting move get. */
const FINISH_MS = 20_000;
/** How long messages already sending get. A slow mail server can take a while. */
const SENDS_FINISH_MS = 60_000;
/** Whether everything finished, or what is still going. */
export type Finished = "done" | "sending" | "timeout";

const within = <T,>(work: Promise<T>, ms: number): Promise<T | "late"> =>
  Promise.race([work, new Promise<"late">((done) => setTimeout(() => done("late"), ms))]);

/** Runs everything waiting in this window, then waits for every message still sending. */
export async function finishBeforeRestart(): Promise<Finished> {
  const finished = await within(Promise.allSettled([...beforeRestart].map((finish) => finish())), FINISH_MS);
  // Sends already going before Restart was pressed are waited for too, not only those started here.
  await within(Promise.allSettled([...sendsGoing]), SENDS_FINISH_MS);
  if (sendsGoing.size > 0) return "sending";
  return finished === "late" ? "timeout" : "done";
}

/** Longer than the panel itself can take, so its answer is always heard. */
const PANEL_FINISH_MS = FINISH_MS + SENDS_FINISH_MS + 10_000;

/** The same in the menu bar panel. Resolves with its answer once it gives one. */
export async function finishPanelBeforeRestart(): Promise<Finished> {
  if (!inMacApp) return "done";
  const asked = `${Date.now()}-${Math.random()}`;
  let answered = (_: Finished) => {};
  const done = new Promise<Finished>((resolve) => {
    answered = resolve;
    // No answer at all means the panel isn't there, and so has nothing going either.
    setTimeout(() => resolve("done"), PANEL_FINISH_MS);
  });
  // Listening first, so a quick answer can't come before anyone hears it.
  const stop = await listen<{ asked: string; finished: Finished }>("ready-to-restart", (event) => {
    if (event.payload.asked === asked) answered(event.payload.finished);
  }).catch(() => null);
  if (!stop) return "done";
  tellPanel("before-restart", asked);
  const finished = await done;
  stop();
  return finished;
}

/** Opens the Mac's print window for what's on screen. */
export async function printPage(): Promise<void> {
  if (inMacApp) await invoke("print_page").catch(() => {});
  else window.print();
}

/** Providers name these differently, like Junk and Spam. Every inbox shows the same words. */
const STANDARD_NAMES: Record<string, string> = {
  inbox: "Inbox",
  sent: "Sent",
  drafts: "Drafts",
  archive: "Archive",
  junk: "Spam",
  trash: "Trash",
};

/** If an inbox has two folders of one kind, only the first takes the name, so they stay apart. */
function withStandardNames(folders: Folder[]): Folder[] {
  const named = new Set<string>();
  return folders.map((f) => {
    const name = STANDARD_NAMES[f.role];
    if (!name || named.has(f.role)) return f;
    named.add(f.role);
    return { ...f, name };
  });
}

async function call<T>(cmd: string, args: object = {}): Promise<T> {
  try {
    if (inMacApp) return await invoke<T>("call", { cmd, args });
    const res = await fetch("/api/call", {
      method: "POST",
      // Mailzo Cloud refuses requests without this, which other sites can't add.
      headers: { "content-type": "application/json", "x-dakly": "1" },
      body: JSON.stringify({ cmd, args }),
    });
    // Signed out of Mailzo Cloud, or the session ended: back to its sign-in page.
    if (res.status === 401) {
      window.location.assign("/sign-in");
      throw { code: "signed_out", message: tx("Sign in to keep going.") };
    }
    const data = await res.json();
    if (!res.ok) throw data;
    return data as T;
  } catch (e) {
    const err = e as { code?: string; message?: string };
    throw new ApiError(err.code ?? "unknown", err.message ?? String(e));
  }
}

export interface NewAccount {
  email: string;
  imap_host: string;
  imap_port: number;
  security: "tls" | "plain";
  username: string;
  password: string;
  smtp_host?: string;
  smtp_port?: number;
  smtp_security?: "tls" | "starttls" | "plain";
}

export type AuthState = "signed_out" | "needs_setup" | "linking" | "ready";

export interface AuthStatus {
  state: AuthState;
  email?: string;
  device_id?: number;
  device_name?: string;
}

export interface PendingDevice {
  id: number;
  name: string;
  created_at: number;
  /** The number the new device shows too. Both must match before allowing it. */
  number: string;
}

export interface TeamPerson {
  user_id: number;
  email: string;
  role: string;
}

export interface TeamStatus {
  team_id?: number;
  state: "none" | "waiting_for_key" | "ready";
  name?: string;
  role?: string;
  me?: number;
  members?: TeamPerson[];
}

export interface TeamInvite {
  id: number;
  role: string;
  expires_at: number;
}

export interface TeamTemplate {
  team_id: number;
  id: string;
  name: string;
  body: string;
  version: number;
}

export interface TeamThreadView {
  assignee: number | null;
  status: string;
  done: boolean;
  sent_count: number;
  last_sent_by: string | null;
  sending_by: string | null;
  comments: { id: string; author: string; mine: boolean; text: string; created_at: number }[];
  viewing: string[];
  replying: string[];
  me: number;
  can_manage: boolean;
  members: TeamPerson[];
}

export interface LinkedDevice {
  signed_out?: boolean;
  id: number;
  name: string;
  created_at: number;
  is_self: boolean;
}

export type ThreadRef = { account_id: number; thread_id: string; folder_id: number };
export type SpamLists = { blocked: string[]; trusted: string[]; sorted: Record<string, string> };
export type SpamReport = {
  mode: string;
  would_move: (ThreadRef & {
    from_name: string;
    from_address: string;
    subject: string;
    reasons: string[];
    score: number;
  })[];
  examples: { spam: number; good: number; needed: number };
  ai_problem: string | null;
};

export type FoundProvider = {
  provider: string;
  name: string;
  method: "password" | "app_password" | "bridge" | "browser" | "browser_soon" | "unsupported" | "manual";
  imap_host: string;
  imap_port: number;
  smtp_host: string;
  smtp_port: number;
  smtp_security: "tls" | "starttls" | "plain";
  usernames: string[];
  steps: string[];
  link: string | null;
  link_label: string | null;
  note: string | null;
  source: string;
  browser: string | null;
};

/** An email a question is about. */
/** What an email's own markup states: a flight, a stay, an order, a bill or a parcel. */
export type AskFact = {
  kind: "flight" | "lodging" | "order" | "invoice" | "parcel";
  place: string;
  flight?: string;
  from?: string;
  to?: string;
  /** As the email writes it: a date, or a date and the local time there. */
  departs?: string | number | null;
  name?: string;
  checkin?: string | number | null;
  booking?: string;
  seller?: string;
  number?: string;
  amount?: string;
  provider?: string;
  due?: string | number | null;
  carrier?: string;
  expected?: string | number | null;
};

export type AskSource = {
  id: number;
  account_id: number;
  thread_id: string;
  folder_id: number;
  subject: string;
  from: string;
  date: number;
  /** Found by meaning, not by the question's words: "Maybe related". */
  related?: boolean;
  /** A calendar event, for plans and briefs, rather than an email. */
  source_type?: "email" | "event";
};

export interface BriefCounts {
  needs_you: number;
  deadlines: number;
  no_reply: number;
  maybe_real: number;
  sorted: number;
}

export interface Newsletter {
  address: string;
  name: string;
  account_id: number;
  folder_id: number;
  threads: number;
  last_date: number;
  subject: string;
  /** Their newest email, whose link is used. Null once all their mail has gone. */
  message_id: number | null;
  /** What a click does: leaves at once, sends an email, or opens their page. None: they offer no way out. */
  how: "one_click" | "mail" | "page" | "none" | "";
  left: { at: number; how: "done" | "sent" | "opened"; name: string } | null;
}

export interface AiPlan {
  id: string;
  name: string;
  program: string;
  install_page: string;
  found: boolean;
  signed_in: boolean;
}

export type AiStatus = {
  via: string;
  has_key: boolean;
  /** AI comes with Mailzo Cloud, and no key or plan of the person's own is set. */
  included?: boolean;
  ending: string | null;
  service: string;
  services: { id: string; name: string; keys_page: string }[];
  /** What the chosen service offers. */
  models: string[];
  /** The one picked for writing. Empty leaves it to the app. */
  model: string;
  /** Apple Intelligence on this Mac: "ready", "not_this_mac", "turned_off" or "downloading". */
  on_device?: string | null;
  /** It's what answers now, as nothing else is connected. */
  using_on_device?: boolean;
  /** Ask AI answers with Apple Intelligence, picked in its header. */
  ask_on_device?: boolean;
};

export type Rule = {
  id: string;
  sentence: string;
  enabled: boolean;
  inbox: string | null;
  /** Which emails, in the person's words, for AI to judge. When set, `when` doesn't apply. */
  ai?: string | null;
  when: {
    from: string[];
    to: string[];
    subject: string[];
    words: string[];
    has_attachment: boolean;
    not_from: string[];
    not_words: string[];
  };
  then: {
    move_to: string | null;
    spam: boolean;
    archive: boolean;
    trash: boolean;
    mark_read: boolean;
    star: boolean;
    tab: string | null;
    mute: boolean;
  };
};

export type SenderRow = {
  address: string;
  name: string;
  threads: number;
  unread: number;
  last_date: number;
  subject: string;
  account_id: number;
  folder_id: number;
};

/** One thing a connected AI app did, or tried to. */
export type AgentAction = {
  id: number;
  at: number;
  app: string;
  tool: string;
  kind: "reading" | "organizing" | "sending";
  outcome: "done" | "refused" | "failed";
  problem: string | null;
  account_id: number | null;
  thread_id: string | null;
  folder_id: number | null;
  subject: string | null;
  who: string | null;
  detail: { query?: string; folder?: string; read?: boolean; to?: string; cc?: string; subject?: string; needs_ok?: boolean } | null;
};

export type MailHealth = {
  bytes: number;
  inboxes: {
    account_id: number;
    email: string;
    last_try: number | null;
    last_ok: number | null;
    problem: string | null;
    folders: {
      id: number;
      name: string;
      role: string;
      server_total: number | null;
      saved: number;
      last_synced_at: number | null;
      problem: string | null;
      bodies_to_come: number;
    }[];
  }[];
};

export type LaterCounts = { snoozed: number; reminders: number; muted: number; screener: number };

export type AutoReply = {
  on: boolean;
  subject: string;
  message: string;
  starts: number | null;
  ends: number | null;
  contacts_only: boolean;
};

export type MyTemplate = { id: string; name: string; body: string };

export interface McpSendAsk {
  id: string;
  from: string;
  to: string[];
  cc: string[];
  subject: string;
  text: string;
  /** What's sent along after its own words, such as the email it answers. */
  quoted: string;
}

export interface AiAppLink {
  id: string;
  name: string;
  link: "connected" | "not_connected" | "outdated" | "taken" | "unreadable";
  after: string;
}

export interface AiAppsStatus {
  apps: AiAppLink[];
  setup: string;
  last_used: string | null;
}

export const api = {
  /** Typed text looks in `account` alone when it's given. Filters carry their own. */
  /** `most`: conversations at most, for a list scrolled past the first 300. */
  searchMail: (search: { text: string; account?: number | null } | { filters: SearchFilters }, extra_ids: number[] = [], most?: number) =>
    call<SearchReply>("search_mail", { ...search, extra_ids, most }),
  searchServers: (search: { text: string; account?: number | null } | { filters: SearchFilters }) =>
    call<SearchReply>("search_servers", search),
  searchUnderstand: (text: string, today: string, utc_offset_minutes: number) =>
    call<{ used_ai: boolean; filters: SearchFilters; summary?: string }>("search_understand", {
      text,
      today,
      utc_offset_minutes,
    }),
  aiStatus: () => call<AiStatus>("ai_status"),
  aiSetKey: (key: string, service: string) => call<AiStatus>("ai_set_key", { key, service }),
  aiChooseModel: (model: string) => call<AiStatus>("ai_choose_model", { model }),
  aiRemoveKey: () => call<AiStatus>("ai_remove_key"),
  authStatus: () => call<AuthStatus>("auth_status"),
  authOptions: () => call<{ google: boolean }>("auth_options"),
  googleSignIn: () => call<{ state: AuthState }>("google_sign_in"),
  googleCancel: () => call("google_cancel"),
  findSigninCode: (email: string) => call<{ code: string | null }>("find_signin_code", { email }),
  authStart: (email: string) => call("auth_start", { email }),
  authWithMailSignIn: (email: string) => call<{ signed_in: boolean }>("auth_with_mail_sign_in", { email }),
  authVerify: (email: string, code: string) => call<{ state: AuthState }>("auth_verify", { email, code }),
  finishSetup: () => call<{ state: AuthState; recovery_key?: string }>("finish_setup"),
  linkStatus: () =>
    call<{ state: AuthState; number?: string | null; other_devices?: number; fresh_after?: number }>(
      "link_status",
    ),
  linkWithRecovery: (recovery_key: string) => call("link_with_recovery", { recovery_key }),
  pendingDevices: () => call<PendingDevice[]>("pending_devices"),
  approveDevice: (id: number, number: string) => call("approve_device", { id, number }),
  rejectDevice: (id: number) => call("reject_device", { id }),
  listDevices: () => call<LinkedDevice[]>("list_devices"),
  removeDevice: (id: number) => call("remove_device", { id }),
  newRecoveryKey: () => call<{ recovery_key: string }>("new_recovery_key"),
  signOut: () => call("sign_out"),
  teamStatus: (team_id?: number) => call<TeamStatus>("team_status", { team_id }),
  teamsStatus: () => call<{ teams: TeamStatus[] }>("teams_status"),
  teamsRefresh: () => call<{ teams: TeamStatus[] }>("teams_refresh"),
  teamCreate: (name: string) => call<TeamStatus>("team_create", { name }),
  teamJoin: (code: string) => call<TeamStatus>("team_join", { code }),
  teamInvite: (team_id: number, role: string) =>
    call<{ code: string; expires_at: number }>("team_invite", { team_id, role }),
  teamInvites: (team_id: number) => call<TeamInvite[]>("team_invites", { team_id }),
  teamCancelInvite: (team_id: number, id: number) => call("team_cancel_invite", { team_id, id }),
  teamLeave: (team_id: number) => call("team_leave", { team_id }),
  teamDelete: (team_id: number) => call("team_delete", { team_id }),
  teamRemoveMember: (team_id: number, id: number) => call("team_remove_member", { team_id, id }),
  teamSetRole: (team_id: number, id: number, role: string) => call("team_set_role", { team_id, id, role }),
  teamShareInbox: (team_id: number, account_id: number) => call("team_share_inbox", { team_id, account_id }),
  teamUnshareInbox: (account_id: number) => call("team_unshare_inbox", { account_id }),
  teamPoll: (wait: boolean) => call<{ active: boolean; changed: boolean }>("team_poll", { wait }),
  teamThread: (account_id: number, thread_id: string) =>
    call<TeamThreadView | null>("team_thread", { account_id, thread_id }),
  teamAssign: (account_id: number, thread_id: string, expected: number | null, assignee: number | null) =>
    call("team_assign", { account_id, thread_id, expected, assignee }),
  teamSetStatus: (account_id: number, thread_id: string, status: string) =>
    call("team_set_status", { account_id, thread_id, status }),
  teamComment: (account_id: number, thread_id: string, text: string, mentions: number[]) =>
    call("team_comment", { account_id, thread_id, text, mentions }),
  teamDeleteComment: (id: string) => call("team_delete_comment", { id }),
  teamPresence: (account_id: number, thread_id: string, kind: string) =>
    call("team_presence", { account_id, thread_id, kind }),
  teamTemplates: (team_id?: number) => call<TeamTemplate[]>("team_templates", { team_id }),
  teamSaveTemplate: (team_id: number, id: string | null, name: string, body: string, expected_version: number) =>
    call("team_save_template", { team_id, id, name, body, expected_version }),
  teamDeleteTemplate: (team_id: number, id: string, expected_version: number) =>
    call("team_delete_template", { team_id, id, expected_version }),
  eraseDevice: () => call("erase_device"),
  /** Starting fresh is booked two days ahead first; asking again early says it's waiting. */
  /** Emails a fresh code to the account's address, which deleting the account needs. */
  accountDeleteStart: () => call("account_delete_start"),
  /** Deletes the Mailzo account. This device keeps its inboxes and mail, with no account. */
  accountDelete: (code: string) => call<{ deleted: boolean }>("account_delete", { code }),
  startFresh: () =>
    call<{ state: "needs_setup" | "fresh_booked" | "fresh_waiting"; due_at?: number | null }>("start_fresh"),
  vaultSync: () => call<{ changed: boolean }>("vault_sync"),
  mailSignIn: (email: string, provider: string) =>
    call<Account>("mail_sign_in", { email, provider }),
  mailSignInCancel: () => call("mail_sign_in_cancel"),
  signInProviders: () => call<Record<string, boolean>>("sign_in_providers"),
  findProvider: (email: string) => call<FoundProvider>("find_provider", { email }),
  addAccount: (a: NewAccount) => call<Account>("add_account", a),
  listAccounts: () => call<Account[]>("list_accounts"),
  removeAccount: (account_id: number) => call("remove_account", { account_id }),
  reorderAccounts: (account_ids: number[]) => call("reorder_accounts", { account_ids }),
  sync: (account_id: number, quiet_now = false, inbox_only = false) =>
    call<SyncStats>("sync_account", { account_id, quiet_now, inbox_only }),
  alertTest: () => call("alert_test"),
  mailWait: (account_id: number) =>
    call<{ supported: boolean; changed: boolean }>("mail_wait", { account_id }),
  mailWaitStop: (account_id: number) => call("mail_wait_stop", { account_id }),
  previewAlertTone: (tone: string) =>
    inMacApp ? invoke("preview_alert_tone", { tone }).catch(() => {}) : Promise.resolve(),
  listFolders: (account_id: number) =>
    call<Folder[]>("list_folders", { account_id }).then(withStandardNames),
  listAll: (role: string, tab?: string, limit = 200, unread_only = false, offset = 0) =>
    call<ThreadSummary[]>("list_all", { role, limit, tab, unread_only, offset }),
  listThreads: (folder_id: number, tab?: string, limit = 200, unread_only = false, offset = 0) =>
    call<ThreadSummary[]>("list_threads", { folder_id, limit, tab, unread_only, offset }),
  createFolder: (account_id: number, name: string) => call<Folder>("create_folder", { account_id, name }),
  deleteFolder: (account_id: number, folder_id: number) =>
    call<{ moved: number }>("delete_folder", { account_id, folder_id }),
  listLater: (kind: string) => call<ThreadSummary[]>("list_later", { kind }),
  laterCounts: () => call<LaterCounts>("later_counts"),
  threadPin: (account_id: number, thread_id: string, on: boolean) =>
    call("thread_pin", { account_id, thread_id, on }),
  threadMute: (account_id: number, thread_id: string, on: boolean) =>
    call("thread_mute", { account_id, thread_id, on }),
  threadSnooze: (account_id: number, thread_id: string, at: number | null) =>
    call("thread_snooze", { account_id, thread_id, at }),
  threadFollowUp: (account_id: number, thread_id: string, at: number | null) =>
    call("thread_follow_up", { account_id, thread_id, at }),
  scheduleDraft: (id: number, send_at: number) => call("schedule_draft", { id, send_at }),
  unscheduleDraft: (id: number) => call<Draft>("unschedule_draft", { id }),
  draftRemind: (id: number, days: number | null) => call<Draft>("draft_remind", { id, days }),
  draftFromMessage: (message_id: number) => call<Draft>("draft_from_message", { message_id }),
  dueTick: (quiet_now: boolean) =>
    call<{ changed: boolean; sent: number; failed: { id: number; problem: string }[] }>("due_tick", { quiet_now }),
  mailHealth: () => call<MailHealth>("mail_health"),
  agentActivity: () => call<AgentAction[]>("agent_activity"),
  agentActivityClear: () => call("agent_activity_clear"),
  rulesList: () => call<Rule[]>("rules_list"),
  ruleUnderstand: (sentence: string) =>
    call<{ rule: Rule; used_ai: boolean; complete: boolean }>("rule_understand", { sentence }),
  rulePreview: (rule: Rule) =>
    call<{ count: number; samples: { from: string; subject: string }[] }>("rule_preview", { rule }),
  ruleTryAi: (rule: Rule) =>
    call<{ looked: number; count: number; samples: { from: string; subject: string }[] }>("rule_try_ai", { rule }),
  ruleSave: (rule: Rule) => call<Rule>("rule_save", { rule }),
  ruleDelete: (id: string) => call("rule_delete", { id }),
  ruleApplyNow: (id: string) => call<{ done: number }>("rule_apply_now", { id }),
  screenerList: () => call<SenderRow[]>("screener_list"),
  screenerDecide: (address: string, allow: boolean) => call("screener_decide", { address, allow }),
  listSenders: (folder_id: number) => call<SenderRow[]>("list_senders", { folder_id }),
  listFrom: (folder_id: number, address: string, screened = false) =>
    call<ThreadSummary[]>("list_from", { folder_id, address, screened }),
  threadShared: (account_id: number, thread_id: string) => call<SharedStuff>("thread_shared", { account_id, thread_id }),
  tagsList: () => call<Tag[]>("tags_list"),
  tagSave: (id: number | null, name: string, colour: string) => call<Tag>("tag_save", { id, name, colour }),
  tagDelete: (tag_id: number) => call("tag_delete", { tag_id }),
  threadTag: (account_id: number, thread_id: string, tag_id: number, on: boolean, amount: number | null = null) =>
    call<ThreadTag[]>("thread_tag", { account_id, thread_id, tag_id, on, amount }),
  taggedThreads: (tag_id: number) => call<ThreadSummary[]>("tagged_threads", { tag_id }),
  myTemplates: () => call<MyTemplate[]>("my_templates"),
  saveMyTemplate: (id: string | null, name: string, body: string) =>
    call("save_my_template", { id, name, body }),
  deleteMyTemplate: (id: string) => call("delete_my_template", { id }),
  noteTrackers: (message_id: number, companies: string[]) => call("note_trackers", { message_id, companies }),
  searchMeaning: (text: string) => call<{ available: boolean; ids: number[] }>("search_meaning", { text }),
  sequencesList: () => call<FollowUps[]>("sequences_list"),
  sequenceSave: (sequence: FollowUps) => call<FollowUps>("sequence_save", { sequence }),
  sequenceDelete: (id: number) => call("sequence_delete", { id }),
  sequenceStart: (message_id: number, sequence_id: number) =>
    call<FollowUpRun>("sequence_start", { message_id, sequence_id }),
  sequenceStartSteps: (message_id: number, steps: FollowUpStep[]) =>
    call<FollowUpRun>("sequence_start", { message_id, steps }),
  sequenceStop: (id: number) => call<FollowUpRun>("sequence_stop", { id }),
  sequenceRuns: (account_id?: number, thread_id?: string) =>
    call<FollowUpRun[]>("sequence_runs", { account_id, thread_id }),
  styleGet: (account_id: number) => call<StyleProfile | null>("style_get", { account_id }),
  styleSave: (account_id: number, text: string) => call<StyleProfile | null>("style_save", { account_id, text }),
  styleMake: (account_id: number) => call<StyleProfile>("style_make", { account_id }),
  aiExplain: (account_id: number, thread_id: string, language?: string, saved_only = false) =>
    call<Explained | null>("ai_explain", { account_id, thread_id, language, saved_only }),
  aiSummarize: (account_id: number, thread_id: string, language?: string, saved_only = false) =>
    call<{ summary: string; points: string[]; next?: string; needs_reply: boolean } | null>("ai_summarize", {
      account_id,
      thread_id,
      language,
      saved_only,
    }),
  aiWriteReply: (message_id: number, instruction: string) =>
    call<{ reply: string }>("ai_write_reply", { message_id, instruction }),
  /** Up to three short replies to this email, to pick with a tap. */
  aiShortReplies: (message_id: number) => call<{ replies: string[] }>("ai_short_replies", { message_id }),
  /** A few sentences on what needs them today. Saved only: just what's already made, or null. */
  aiBrief: (saved_only: boolean, language: string) =>
    call<{ text: string; count: number } | null>("ai_brief", { saved_only, language }),
  /** The person's own draft made better ("fix", "shorter", "formal", "friendly"), or "write" from notes. */
  aiPolish: (text: string, how: string, account_id?: number) =>
    call<{ text: string }>("ai_polish", { text, how, account_id }),
  aiTranslate: (message_id: number, language: string) =>
    call<{ text: string }>("ai_translate", { message_id, language }),
  /** The emails a question is about, found by code. A follow-up passes the question before it. */
  /** `about` is the conversation it was asked from, for everything with the people in it. */
  aiAskSources: (
    question: string,
    filters: SearchFilters | null,
    scope: number | null = null,
    quick = false,
    previous: string[] = [],
    about: { account_id: number; thread_id: string } | null = null,
    /** What was made of the last question, which a follow-up starts from. */
    previousPlan: unknown = null,
  ) =>
    call<{
      sources: AskSource[];
      kind?: "count" | "list" | "fact" | "catch_up" | "plan" | "about_itself";
      fact?: AskFact;
      /** How many emails matched exactly, more than a list shows when it's long. */
      total?: number;
      /** A name that also fits someone else, and who. */
      others?: { name: string; other: string }[];
      /** What the question is about, all the ways of saying it, to answer from. */
      words?: string[];
      /** What was made of this question, for the next one in the chat to start from. */
      plan?: unknown;
    }>("ai_ask_sources", {
      question,
      filters,
      scope,
      quick,
      previous,
      about,
      previous_plan: previousPlan,
    }),
  /** One turn of the chat about their mail, answered from the emails found for it. `related` are
   * those of `ids` found only as "Maybe related". */
  aiChat: (
    question: string,
    history: { role: "user" | "assistant"; text: string }[],
    ids: number[],
    language: string,
    related: number[] = [],
    plan: { words?: string[]; kind?: string; total?: number } = {},
  ) =>
    call<{
      answer: string;
      points: { text: string; ids: number[] }[];
      /** The few emails worth opening. */
      key: AskSource[];
      sources: AskSource[];
      suggestions: string[];
      /** The emails don't hold the answer. */
      not_found?: boolean;
      /** Details the AI wrote that the emails don't back up, taken out. */
      removed?: string[];
      /** More was found than this AI can read at once. */
      partial?: boolean;
      /** Asked how Ask AI works, which it doesn't say. */
      about_itself?: boolean;
    }>("ai_chat", { question, history, ids, related, language, plan }),
  aiAsk: (question: string, ids: number[], language: string) =>
    call<{ headline: string; items: (AskSource & { title: string; value: string })[] }>("ai_ask", {
      question,
      ids,
      language,
    }),
  /** Whether the AI that comes with Mailzo Cloud can be used: the brief, questions, AI sorting and the spam second opinion. */
  paidStatus: () => call<{ paid: boolean }>("paid_status"),
  threadPriority: (account_id: number, thread_id: string, group: string | null) =>
    call("thread_priority", { account_id, thread_id, group }),
  senderPriority: (address: string, group: string) => call("sender_priority", { address, group }),
  brief: () => call<BriefCounts>("brief"),
  aiPlans: () => call<AiPlan[]>("ai_plans"),
  aiPlanSignIn: (id: string) => call("ai_plan_sign_in", { id }),
  aiPlanUse: (id: string) => call<AiStatus>("ai_plan_use", { id }),
  aiSpend: () => call<{ calls: number; dollars: number | null }>("ai_spend"),
  mcpStatus: () => call<AiAppsStatus>("mcp_status"),
  mcpConnect: (id: string) => call<AiAppsStatus>("mcp_connect", { id }),
  mcpDisconnect: (id: string) => call<AiAppsStatus>("mcp_disconnect", { id }),
  mcpSendNext: () => call<McpSendAsk | null>("mcp_send_next"),
  mcpSendDecide: (id: string, send: boolean) => call("mcp_send_decide", { id, send }),
  autoReplyGet: (account_id: number) =>
    call<{ supported: boolean; reply?: AutoReply; provider?: string; link?: string | null }>("auto_reply_get", { account_id }),
  autoReplySet: (account_id: number, reply: AutoReply) => call("auto_reply_set", { account_id, reply }),
  tabCounts: (folder_id: number | null, unread = false) =>
    call<Record<string, number>>("tab_counts", { folder_id, unread }),
  spamMark: (threads: ThreadRef[], block = false) => call("spam_mark", { threads, block }),
  spamClear: (threads: ThreadRef[]) => call("spam_clear", { threads }),
  spamLists: () => call<SpamLists>("spam_lists"),
  spamUnlist: (address: string, kind: "blocked" | "trusted" | "sorted") =>
    call<SpamLists>("spam_unlist", { address, kind }),
  spamReport: () => call<SpamReport>("spam_report"),
  sortSender: (address: string, category: string) => call("sort_sender", { address, category }),
  /** Quietly hands back a page that needs visiting instead of opening it, for doing many at once. */
  unsubscribe: (message_id: number, quietly = false) =>
    call<{ kind: "done" | "sent" | "opened" | "open" | "page"; url?: string }>("unsubscribe", { message_id, quietly }),
  newsletters: () => call<Newsletter[]>("newsletters"),
  /** Lets a sender back in after unsubscribing: their new mail stays in the inbox again. */
  newsletterForget: (account_id: number, address: string) =>
    call("newsletter_forget", { account_id, address }),
  /** Mail with someone, in one inbox or all of them (null), wherever it's been filed. */
  mailWith: (account_id: number | null, addresses: string[]) =>
    call<ThreadSummary[]>("mail_with", { account_id, addresses }),
  getThread: (account_id: number, thread_id: string, folder_id: number | null) =>
    call<Message[]>("get_thread", { account_id, thread_id, folder_id }),
  threadSummary: (account_id: number, thread_id: string) =>
    call<ThreadSummary | null>("thread_summary", { account_id, thread_id }),
  setThreadSeen: (account_id: number, thread_id: string, seen: boolean) =>
    call("set_thread_seen", { account_id, thread_id, seen }),
  setThreadFlagged: (account_id: number, thread_id: string, flagged: boolean) =>
    call("set_thread_flagged", { account_id, thread_id, flagged }),
  moveThread: (account_id: number, thread_id: string, folder_id: number, to: string) =>
    call("move_thread", { account_id, thread_id, folder_id, to }),
  deleteForever: (account_id: number, thread_id: string, folder_id: number) =>
    call("delete_forever", { account_id, thread_id, folder_id }),
  bulkThreads: (
    threads: { account_id: number; thread_id: string; folder_id: number }[],
    action: string,
    to?: string,
    /** For read and unread: "here" changes this Mac only, "servers" then tells them. */
    step?: "here" | "servers",
  ) => call<{ done: number; problem?: string }>("bulk_threads", { threads, action, to, step }),
  /** Read or unread on this Mac first, then on the servers. `changedHere` runs in between, so
   * every window can show it before any server has heard. */
  markThreads: async (
    threads: { account_id: number; thread_id: string; folder_id: number }[],
    action: "seen" | "unseen",
    changedHere: () => void,
  ) => {
    await api.bulkThreads(threads, action, undefined, "here");
    changedHere();
    return api.bulkThreads(threads, action, undefined, "servers");
  },
  /** Where a browser downloads an attachment from. */
  /** Where a browser downloads a whole email as a file. */
  originalUrl: (message_id: number) => `/api/original?message_id=${encodeURIComponent(message_id)}`,
  attachmentUrl: (message_id: number, part: number) =>
    `/api/attachment?message_id=${encodeURIComponent(message_id)}&part=${encodeURIComponent(part)}`,
  /** A look without saving: a picture or PDF (as base64), plain text, or "none". */
  previewAttachment: (message_id: number, part: number) =>
    call<{ kind: "image" | "pdf" | "text" | "none"; name: string; mime?: string; data?: string; text?: string; cut_short?: boolean }>(
      "attachment_preview",
      { message_id, part },
    ),
  saveAttachment: (message_id: number, part: number) =>
    call<{ path: string; name: string; can_open: boolean }>("save_attachment", { message_id, part }),
  useSavedFile: (path: string, open: boolean) => call("use_saved_file", { path, open }),
  messageSource: (message_id: number) => call<MessageSource>("message_source", { message_id }),
  saveOriginal: (message_id: number) => call<{ path: string; name: string }>("save_original", { message_id }),
  draftNew: (
    account_id: number,
    mode: Draft["mode"],
    message_id: number | null,
    date_text: string | null,
  ) => call<Draft>("draft_new", { account_id, mode, message_id, date_text }),
  draftGet: (id: number) => call<Draft>("draft_get", { id }),
  draftList: () => call<Draft[]>("draft_list"),
  draftSave: (d: {
    id: number;
    account_id: number;
    from_address: string;
    to: Draft["to"];
    cc: Draft["cc"];
    bcc: Draft["bcc"];
    subject: string;
    html: string;
    text: string;
    encrypt?: boolean;
    sign?: boolean;
    /** The person confirmed sending it unencrypted though it quotes encrypted mail. */
    allow_plain?: boolean;
  }) => call<Draft>("draft_save", d),
  draftDiscard: (id: number) => call("draft_discard", { id }),
  draftAddAttachment: (id: number, name: string, mime: string, data: string) =>
    call<Draft>("draft_add_attachment", { id, name, mime, data }),
  draftRemoveAttachment: (id: number, attachment_id: string) =>
    call<Draft>("draft_remove_attachment", { id, attachment_id }),
  sendDraft: (id: number, force = false) =>
    trackSend(call<{ sent: boolean; warnings: string[] }>("send_draft", { force, id })),
  imageSenders: () => call<string[]>("image_senders"),
  setSenderImages: (address: string, allowed: boolean) => call("set_sender_images", { address, allowed }),
  avatar: (address: string) => call<{ picture: string | null; kind?: "person" | "logo" }>("avatar", { address }),
  appSettings: () => call<Record<string, string>>("app_settings"),
  setSetting: (key: string, value: string) => call("set_setting", { key, value }),
  updateAccountPrefs: (account_id: number, prefs: AccountPrefs) =>
    call("update_account_prefs", { account_id, prefs }),
  updateConnection: (c: {
    account_id: number;
    imap_host: string;
    imap_port: number;
    security: "tls" | "plain";
    username: string;
    password: string | null;
  }) => call("update_connection", c),
  noteImagesHeld: (message_id: number) => call("note_images_held", { message_id }),
  privacyStats: () =>
    call<{ images_held: number; trackers: { company: string; seen: number }[] }>("privacy_stats"),
  clearSavedMail: () => call("clear_saved_mail"),
  contacts: (account_id: number, query: string) => call<Contact[]>("contacts", { account_id, query }),
  updateAccountSettings: (a: {
    account_id: number;
    display_name: string;
    signature: string;
    smtp_host: string;
    smtp_port: number;
    smtp_security: Account["smtp_security"];
  }) => call("update_account_settings", a),
  checkSmtp: (account_id: number) =>
    call<{ host: string; port: number; security: Account["smtp_security"] }>("check_smtp", { account_id }),
  fetchBody: (message_id: number) => call<Body>("fetch_body", { message_id }),
};

export type Calendar = {
  id: number;
  account_id: number | null;
  kind: string;
  name: string;
  hue: number;
  problem: string | null;
};

export type Occurrence = {
  event_id: number;
  calendar_id: number;
  title: string;
  location: string;
  notes: string;
  starts: number;
  ends: number;
  all_day: boolean;
  repeats: boolean;
  my_status: string;
  organizer: string | null;
  attendees: { address: string; name: string; status: string }[];
  hue: number;
  is_override: boolean;
};

export type EventInput = {
  id?: number;
  calendar_id: number;
  title: string;
  location: string;
  notes: string;
  starts: number;
  ends: number;
  all_day: boolean;
  repeat?: string | null;
  occurrence?: number;
};

export type Invite = {
  method: string;
  uid: string;
  title: string;
  location: string;
  starts: number;
  ends: number;
  all_day: boolean;
  organizer: string | null;
  organizer_address: string;
  other_sender: boolean;
  can_answer: boolean;
  cancelled: boolean;
  my_status: string | null;
};

export const calendarApi = {
  list: () =>
    call<{ calendars: Calendar[]; can_connect: { account_id: number; email: string }[] }>("calendar_list"),
  connect: (account_id: number) =>
    call<{ calendars: Calendar[]; can_connect: { account_id: number; email: string }[] }>("calendar_connect", { account_id }),
  sync: () => call<{ calendars: Calendar[]; can_connect: { account_id: number; email: string }[] }>("calendar_sync"),
  events: (from: number, to: number) => call<Occurrence[]>("calendar_events", { from, to }),
  save: (event: EventInput) => call<{ id: number }>("calendar_save", { event }),
  remove: (id: number) => call("calendar_delete", { id }),
  invite: (message_id: number) => call<Invite | null>("invite_for_message", { message_id }),
  respond: (message_id: number, status: "accept" | "maybe" | "decline") =>
    call<{ status: string; warnings: string[] }>("invite_respond", { message_id, status }),
  cancelled: (message_id: number) => call("invite_cancelled", { message_id }),
};

/** An inbox's own PGP key, for encrypted email. */
export type PgpStatus = { has_key: boolean; fingerprint: string | null; created_at: number | null; public_key: string | null };
/** Someone else's public key, learned from their email or imported. */
export type PgpKey = {
  address: string;
  fingerprint: string;
  source: "autocrypt" | "imported";
  /** A later email offered a different key. The one in "fingerprint" is still used. */
  changed?: boolean;
  old_fingerprint?: string;
  new_fingerprint?: string;
};

export const pgpApi = {
  status: (account_id: number) => call<PgpStatus>("pgp_status", { account_id }),
  generate: (account_id: number) => call<PgpStatus>("pgp_generate", { account_id }),
  importSecret: (account_id: number, armored: string, passphrase?: string) =>
    call<PgpStatus>("pgp_import_secret", { account_id, armored, passphrase }),
  remove: (account_id: number) => call("pgp_remove", { account_id }),
  keys: () => call<PgpKey[]>("pgp_keys_list"),
  importKey: (armored: string) => call<{ address: string; fingerprint: string }[]>("pgp_key_import", { armored }),
  deleteKey: (address: string) => call("pgp_key_delete", { address }),
  acceptKeyChange: (address: string) => call("pgp_key_accept_change", { address }),
  canEncrypt: (account_id: number, to: string[]) =>
    call<{ own_key: boolean; missing: string[] }>("pgp_can_encrypt", { account_id, to }),
};

/** Someone in the address book, or someone the person emails who isn't saved yet. */
export type PersonRow = { id: number | null; name: string; emails: string[]; company: string; saved: boolean; count: number; last_at: number | null };
/** A saved contact. */
export type Person = { id: number; uid: string; name: string; emails: string[]; phones: string[]; company: string; notes: string; updated_at: number };

export const peopleApi = {
  list: (query: string) => call<PersonRow[]>("people_list", { query }),
  get: (address: string) =>
    call<{ person: Person | null; name_from_mail: string; count: number; last_at: number | null; first_at: number | null }>("person_get", { address }),
  save: (p: { id?: number; name: string; emails: string[]; phones: string[]; company: string; notes: string }) => call<Person>("person_save", p),
  remove: (id: number) => call("person_delete", { id }),
  importVcard: (text: string) => call<{ added: number; updated: number; skipped: number }>("people_import_vcard", { text }),
  /** All saved contacts as one .vcf file in Downloads. */
  exportFile: () => call<{ path: string; name: string }>("people_export_file"),
};
