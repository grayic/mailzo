import type { IconSvgElement } from "@hugeicons/react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Add01Icon,
  AiMagicIcon,
  ArrowLeft01Icon,
  Cancel01Icon,
  DatabaseIcon,
  DragDropVerticalIcon,
  FilterHorizontalIcon,
  Edit02Icon,
  Note01Icon,
  SentIcon,
  Notification01Icon,
  PaintBoardIcon,
  Search01Icon,
  Settings02Icon,
  SpamIcon,
  Shield01Icon,
  UserGroupIcon,
  KeyboardIcon,
  Link01Icon,
  UserCircleIcon,
  Wrench01Icon,
  Clock01Icon,
} from "../../icons";
import { api, type AgentAction } from "../../api";
import { useDragOrder } from "../../dragOrder";
import { ActivityPage } from "./ActivityPage";
import type { Account, Folder } from "../../types";
import { Avatar, Icon, IconButton } from "../ui";
import {
  AdvancedPage,
  AppearancePage,
  GeneralPage,
  NotificationsPage,
  PrivacyPage,
  ShortcutsPage,
  TemplatesPage,
  WritingPage,
} from "./AppPages";
import { AccountPage } from "./AccountPage";
import { AiPage } from "./AiPage";
import { FollowUpsPage } from "./FollowUpsPage";
import { ConnectionsPage } from "./ConnectionsPage";
import { HealthPage } from "./HealthPage";
import { InboxPage } from "./InboxPage";
import { RulesPage } from "./RulesPage";
import { SpamPage } from "./SpamPage";
import { TeamPage } from "./TeamPage";
import { tx } from "../../i18n";

/** Grouped by what someone is trying to do, with names short enough to scan. */
const PAGE_GROUPS: [string, [string, string, IconSvgElement][]][] = [
  [
    tx("App"),
    [
      ["general", tx("General"), Settings02Icon],
      ["appearance", tx("Appearance"), PaintBoardIcon],
      ["notifications", tx("Notifications"), Notification01Icon],
      ["shortcuts", tx("Shortcuts"), KeyboardIcon],
    ],
  ],
  [
    tx("Email"),
    [
      ["writing", tx("Writing"), Edit02Icon],
      ["templates", tx("Templates"), Note01Icon],
      ["follow_ups", tx("Follow-ups"), SentIcon],
      ["rules", tx("Filters"), FilterHorizontalIcon],
      ["spam", tx("Spam"), SpamIcon],
    ],
  ],
  [
    tx("Data"),
    [
      ["privacy", tx("Privacy"), Shield01Icon],
      ["ai", tx("AI"), AiMagicIcon],
      ["connections", tx("MCP"), Link01Icon],
      ["activity", tx("AI activity"), Clock01Icon],
      ["mail", tx("Storage"), DatabaseIcon],
      ["advanced", tx("Advanced"), Wrench01Icon],
    ],
  ],
  [
    tx("Account"),
    [
      ["account", tx("Profile"), UserCircleIcon],
      ["team", tx("Team"), UserGroupIcon],
    ],
  ],
];

/** What's on each page, so search can find a setting by its name. */
const SETTINGS_ON: Record<string, string[]> = {
  general: [
    tx("Open at login"),
    tx("Show in menu bar"),
    tx("Show in Dock"),
    tx("Mark as read"),
    tx("After archiving or deleting"),
    tx("The Delete key"),
    tx("What opens first"),
    tx("Tab that opens first"),
    tx("Group emails into conversations"),
    tx("Screen new senders"),
    tx("Single-key shortcuts"),
  ],
  appearance: [tx("When you delete"), tx("Fly away with sound"), tx("Language"), tx("Theme"), tx("Dark mode"), tx("Light mode"), tx("Text size"), tx("Spacing"), tx("Lines of preview"), tx("Show sender pictures in the list"), tx("Show the unread count")],
  notifications: [tx("Notify me about new email"), tx("Play a sound"), tx("Quiet hours"), tx("Show the sender and subject"), tx("Test notification")],
  shortcuts: [tx("Keyboard shortcuts")],
  writing: [
    tx("New messages come from"),
    tx("Font"),
    tx("Undo send"),
    tx("Check spelling"),
    tx("Include the earlier message in replies"),
    tx("Keep drafts on your mail server too"),
    tx("The main Reply action"),
  ],
  spam: [tx("Sort the inbox into tabs"), tx("Blocked senders"), tx("Trusted senders"), tx("What it has learned"), tx("When an email looks like spam")],
  privacy: [
    tx("Pictures inside emails"),
    tx("Show sender pictures and logos"),
    tx("Senders whose pictures always show"),
    tx("Who tried to track you"),
    tx("What stays on your device"),
    tx("What our account server knows"),
  ],
  ai: [
    tx("AI key"),
    tx("AI service"),
    tx("Model for writing"),
    tx("Summaries"),
    tx("Writing replies"),
    tx("Search in your own words"),
    tx("Filters in plain words"),
    tx("Translating"),
    tx("Read new mail by itself"),
    tx("Spam second opinion"),
  ],
  connections: [tx("Allow connected apps"), tx("Copy setup")],
  activity: [tx("Agent activity"), tx("What AI apps did"), tx("Clear activity")],
  mail: [tx("Keep whole emails from"), tx("Room used on this device"), tx("Mail on this device")],
  advanced: [tx("Clear saved mail on this device")],
  account: [tx("Main account email"), tx("Recovery key"), tx("Your devices"), tx("Sign out"), tx("Erase this device")],
  team: [tx("Invite someone"), tx("Invite code"), tx("Shared inboxes"), tx("Assign emails"), tx("Leave private notes"), tx("Team name")],
};

const SETTINGS_ON_AN_INBOX = [
  tx("Signature"),
  tx("Your name, as people see it"),
  tx("Short label"),
  tx("Inbox colour"),
  tx("Include in All Inbox"),
  tx("Other address"),
  tx("Out-of-office replies"),
  tx("Notify me about"),
  tx("New password"),
  tx("Disconnect this inbox"),
];

/** Each inbox, in the order chosen by dragging them, or with Option and the arrow keys. */
function InboxNav({
  accounts,
  inboxId,
  onOpen,
  onMoved,
}: {
  accounts: Account[];
  inboxId: number | null | undefined;
  onOpen: (account: Account) => void;
  onMoved: () => void;
}) {
  /** The order once moved, until the saved one comes back. */
  const [order, setOrder] = useState<number[] | null>(null);
  useEffect(() => setOrder(null), [accounts]);

  const shown = order
    ? order.flatMap((id) => accounts.find((a) => a.id === id) ?? [])
    : accounts;

  const drag = useDragOrder(
    shown.map((a) => a.id),
    (ids) => {
      setOrder(ids);
      api
        .reorderAccounts(ids)
        .then(onMoved)
        .catch(() => setOrder(null));
    },
  );

  return (
    <div ref={drag.list} className={drag.listClass}>
      {shown.map((a, i) => {
        const { moving, ...row } = drag.row(a.id, i);
        return (
          <button
            key={a.id}
            className={`nav-item ${moving ? "nav-item-moving" : ""}`}
            aria-current={inboxId === a.id}
            {...row}
            onClick={() => {
              if (drag.clicked()) onOpen(a);
            }}
          >
            <Avatar name={a.email} size="xs" hue={a.prefs.color ?? undefined} />
            <span className="truncate-1 grow">{a.prefs.label || a.email}</span>
            {shown.length > 1 && <Icon icon={DragDropVerticalIcon} size="xs" className="nav-grip" />}
          </button>
        );
      })}
    </div>
  );
}

export function SettingsWindow({
  page,
  email,
  onSetUpAccount,
  accounts,
  foldersBy,
  onPage,
  onClose,
  onAccountsChanged,
  onOpenThread,
  onTeamChanged,
  onSaveDefaultInbox,
  onDisconnect,
  onConnect,
  onSignedOut,
  onMailCleared,
  onAiChanged,
  onFiltersChanged,
}: {
  /** A page's name, or `inbox:<id>`. */
  page: string;
  /** The Mailzo account's email, or empty when there is no account on this device. */
  email: string;
  onSetUpAccount: () => void;
  accounts: Account[];
  foldersBy: Record<number, Folder[]>;
  onPage: (page: string) => void;
  onClose: () => void;
  onAccountsChanged: () => void;
  /** Goes to a conversation mentioned on one of these pages, leaving Settings. */
  onOpenThread: (a: AgentAction) => void;
  onTeamChanged: () => void;
  onSaveDefaultInbox: (accountId: number | null) => void;
  onDisconnect: (accountId: number) => void;
  onConnect: () => void;
  onSignedOut: () => void;
  onMailCleared: () => void;
  onAiChanged: () => void;
  onFiltersChanged: () => void;
}) {
  // Esc closes this unless a pop-up or a menu on top of it is open. Set up only once,
  // so it hears Esc before a pop-up closes itself.
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector(".scrim, [role='menu']")) closeRef.current();
    };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, []);

  // A phone shows the list of pages first, then the one chosen.
  const [phonePage, setPhonePage] = useState(page !== "general");
  const go = (to: string) => {
    setPhonePage(true);
    onPage(to);
  };

  const [search, setSearch] = useState("");
  const wanted = search.trim().toLowerCase();
  const hits = [
    ...PAGE_GROUPS.flatMap(([, pages]) =>
      pages.map(([id, label, icon]) => ({
        to: id,
        label,
        picture: <Icon icon={icon} />,
        settings: SETTINGS_ON[id] ?? [],
      })),
    ),
    ...accounts.map((a) => ({
      to: `inbox:${a.id}`,
      label: a.prefs.label || a.email,
      picture: <Avatar name={a.email} size="xs" hue={a.prefs.color ?? undefined} />,
      settings: SETTINGS_ON_AN_INBOX,
    })),
  ].flatMap((place) =>
    [place.label, ...place.settings]
      .filter((name) => name.toLowerCase().includes(wanted))
      .map((name) => ({ ...place, name })),
  );
  const connectLabel = tx("Connect an inbox");
  const connectButton = (
    <button className="nav-item" onClick={onConnect}>
      <Icon icon={Add01Icon} />
      <span className="truncate-1">{connectLabel}</span>
    </button>
  );

  const inboxId = page.startsWith("inbox:") ? Number(page.slice(6)) : null;
  const inbox = inboxId !== null ? accounts.find((a) => a.id === inboxId) : undefined;
  // An inbox can leave while its page is open, when it stops being shared.
  const inboxGone = inboxId !== null && !inbox;
  useEffect(() => {
    if (inboxGone) onPage("general");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inboxGone]);

  return createPortal(
    <div role="dialog" aria-label={tx("Settings")} className="settings" data-phone-page={phonePage || undefined}>
      <nav aria-label={tx("Settings pages")} className="settings-nav">
        <div className="sidebar-top settings-nav-top" data-tauri-drag-region>
          <IconButton icon={Cancel01Icon} label={tx("Close settings")} className="phone-only" onClick={onClose} />
        </div>
        <div className="settings-nav-head">
          <button className="nav-item settings-back" onClick={onClose}>
            <Icon icon={ArrowLeft01Icon} />
            <span className="truncate-1">{tx("Back to inbox")}</span>
          </button>
          <form
            role="search"
            className="search"
            onSubmit={(e) => {
              e.preventDefault();
              if (wanted && hits[0]) go(hits[0].to);
            }}
          >
            <Icon icon={Search01Icon} size="s" />
            <input
              type="search"
              aria-label={tx("Search settings")}
              placeholder={tx("Search settings")}
              value={search}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => setSearch(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape" && search) {
                  e.stopPropagation();
                  setSearch("");
                }
              }}
            />
            {search && <IconButton icon={Cancel01Icon} label={tx("Clear search")} small onClick={() => setSearch("")} />}
          </form>
        </div>
        <div className="settings-nav-scroll">
          {wanted ? (
            <>
              {hits.map((hit) => (
                <button
                  key={`${hit.to}:${hit.name}`}
                  className="nav-item settings-hit"
                  aria-current={page === hit.to}
                  onClick={() => go(hit.to)}
                >
                  {hit.picture}
                  <span className="settings-hit-words">
                    <span className="truncate-1">{hit.name}</span>
                    {hit.name !== hit.label && <span className="settings-hit-page truncate-1">{hit.label}</span>}
                  </span>
                </button>
              ))}
              {connectLabel.toLowerCase().includes(wanted) && connectButton}
              {hits.length === 0 && !connectLabel.toLowerCase().includes(wanted) && (
                <div className="settings-nav-title">{tx("No settings found")}</div>
              )}
            </>
          ) : (
            <>
              {PAGE_GROUPS.map(([group, pages], i) => (
                <div key={group}>
                  <div className={`settings-nav-title ${i > 0 ? "settings-nav-gap" : ""}`}>{group}</div>
                  {pages.map(([id, label, icon]) => (
                    <button key={id} className="nav-item" aria-current={page === id} onClick={() => go(id)}>
                      <Icon icon={icon} />
                      <span className="truncate-1">{label}</span>
                    </button>
                  ))}
                </div>
              ))}
              <div className="settings-nav-title settings-nav-gap">{tx("Inboxes")}</div>
              <InboxNav
                accounts={accounts}
                inboxId={inboxId}
                onOpen={(a) => go(`inbox:${a.id}`)}
                onMoved={onAccountsChanged}
              />
              {connectButton}
            </>
          )}
        </div>
      </nav>
      <main className="settings-page">
        <div className="settings-bar" data-tauri-drag-region="deep">
          <IconButton icon={ArrowLeft01Icon} label={tx("Back to settings")} className="phone-only" onClick={() => setPhonePage(false)} />
          <IconButton icon={Cancel01Icon} label={tx("Close")} shortcut="esc" onClick={onClose} />
        </div>
        {page === "general" && <GeneralPage />}
        {page === "appearance" && <AppearancePage />}
        {page === "writing" && (
          <WritingPage accounts={accounts} onSaveDefaultInbox={onSaveDefaultInbox} />
        )}
        {page === "templates" && <TemplatesPage />}
        {page === "follow_ups" && <FollowUpsPage />}
        {page === "notifications" && <NotificationsPage />}
        {page === "privacy" && <PrivacyPage />}
        {page === "spam" && <SpamPage onChanged={onFiltersChanged} />}
        {page === "rules" && <RulesPage accounts={accounts} foldersBy={foldersBy} />}
        {page === "mail" && <HealthPage />}
        {page === "connections" && <ConnectionsPage onSeeActivity={() => go("activity")} />}
        {page === "activity" && <ActivityPage onOpenThread={onOpenThread} />}
        {page === "ai" && <AiPage onChanged={onAiChanged} accounts={accounts} onAccountsChanged={onAccountsChanged} />}
        {page === "shortcuts" && <ShortcutsPage />}
        {page === "team" && <TeamPage
            accounts={accounts}
            signedIn={email !== ""}
            onSetUpAccount={onSetUpAccount}
            onChanged={() => {
              onTeamChanged();
              onAccountsChanged();
            }}
          />}
        {page === "account" && <AccountPage email={email} onSignedOut={onSignedOut} onSetUpAccount={onSetUpAccount} />}
        {page === "advanced" && <AdvancedPage onCleared={onMailCleared} />}
        {inbox && (
          <InboxPage
            key={inbox.id}
            account={inbox}
            folders={foldersBy[inbox.id] ?? []}
            onChanged={onAccountsChanged}
            onDisconnect={() => onDisconnect(inbox.id)}
          />
        )}
      </main>
    </div>,
    document.body,
  );
}
