import { useEffect, useState } from "react";
import type { IconSvgElement } from "@hugeicons/react";
import {
  Add01Icon,
  Archive02Icon,
  ArrowDown01Icon,
  ArrowLeft01Icon,
  ArrowRight01Icon,
  Delete02Icon,
  DragDropVerticalIcon,
  FileEditIcon,
  Folder01Icon,
  InboxIcon,
  Mailbox01Icon,
  MoreHorizontalIcon,
  PencilEdit02Icon,
  SentIcon,
  Settings02Icon,
  SidebarLeftIcon,
  SpamIcon,
  Tick02Icon,
  ViewIcon,
  ViewOffSlashIcon,
} from "../icons";
import { ApiError, api, onTheWeb } from "../api";
import type { Account, Folder } from "../types";
import { UpdateNote } from "./Updates";
import { Avatar, ContextMenu, Dialog, DownloadBar, Icon, IconButton, LogoMark, Menu, MenuItem, Problem, Spinner } from "./ui";
import { TagDot } from "../tags";
import { folderLabel, language, tx } from "../i18n";
import { useDragOrder } from "../dragOrder";
import { useSettings } from "../settings";

export const FOLDER_ICONS: Record<string, IconSvgElement> = {
  inbox: InboxIcon,
  sent: SentIcon,
  drafts: FileEditIcon,
  archive: Archive02Icon,
  junk: SpamIcon,
  trash: Delete02Icon,
  folder: Folder01Icon,
};

/** How the sidebar is arranged on this Mac, like whether it's collapsed: each group's order,
 * and what's been taken off it. */
type Arrangement = Record<string, { order: string[]; hidden: string[] }>;
const ARRANGEMENT_KEY = "sidebar-arrangement";
const MORE_KEY = "sidebar-more";

const readArrangement = (): Arrangement => {
  try {
    return JSON.parse(localStorage.getItem(ARRANGEMENT_KEY) ?? "{}") ?? {};
  } catch {
    return {};
  }
};

/** In the saved order, with anything new kept after it in the order it came in. */
function arranged<T>(items: T[], idOf: (item: T) => string, order: string[]): T[] {
  const place = (item: T) => {
    const at = order.indexOf(idOf(item));
    return at < 0 ? order.length : at;
  };
  return items
    .map((item, i) => ({ item, i }))
    .sort((a, b) => place(a.item) - place(b.item) || a.i - b.i)
    .map(({ item }) => item);
}

export const unreadIn = (folders: Folder[]) =>
  folders.filter((f) => f.role === "inbox").reduce((sum, f) => sum + f.unread, 0);

function Count({ n }: { n: number }) {
  return n > 0 ? <span className="nav-count nav-count-unread">{n}</span> : null;
}

/** Names shared inboxes by their team, since a person can be in several. */
function useTeamNames(accounts: Account[]) {
  const [names, setNames] = useState<Record<number, string>>({});
  const shared = accounts.filter((a) => a.prefs.team_shared).length;
  useEffect(() => {
    if (shared === 0) return;
    api
      .teamsStatus()
      .then(({ teams }) => setNames(Object.fromEntries(teams.map((t) => [t.team_id ?? 0, t.name ?? ""]))))
      .catch(() => {});
  }, [shared]);
  return names;
}

/** Chooses which account's folders the sidebar shows. One is always selected. */
function AccountSwitcher({
  accounts,
  selected,
  foldersBy,
  onPick,
  onAddAccount,
}: {
  accounts: Account[];
  selected: Account | null;
  foldersBy: Record<number, Folder[]>;
  onPick: (id: number) => void;
  onAddAccount: () => void;
}) {
  const teamNames = useTeamNames(accounts);
  return (
    <Menu
      label={tx("Switch account")}
      trigger={(props) => (
        <button
          {...props}
          type="button"
          className="switcher"
          aria-label={tx("Switch account")}
          data-tip={tx("Switch account")}
        >
          <Avatar name={selected?.email ?? "?"} size="xs" hue={selected?.prefs.color ?? undefined} />
          <span className="switcher-name truncate-1">{selected?.prefs.label || selected?.email}</span>
          <Icon icon={ArrowDown01Icon} size="xs" className="switcher-arrow" />
        </button>
      )}
    >
      {(close) => (
        <div className="switcher-menu">
          {accounts.map((a, i) => {
            const unread = unreadIn(foldersBy[a.id] ?? []);
            return (
              <MenuItem
                key={a.id}
                leading={<Avatar name={a.email} size="xs" hue={a.prefs.color ?? undefined} />}
                trailing={
                  <>
                    {unread > 0 && <span className="menu-count">{unread}</span>}
                    {selected?.id === a.id && <Icon icon={Tick02Icon} size="s" />}
                    {i < 8 && <span className="menu-key">⌘{i + 2}</span>}
                  </>
                }
                onClick={() => {
                  close();
                  onPick(a.id);
                }}
              >
                {a.prefs.label || a.email}
                {a.prefs.team_shared && <span className="pill">{teamNames[a.prefs.team_id ?? 0] || "team"}</span>}
              </MenuItem>
            );
          })}
          <div className="menu-sep" />
          <MenuItem
            icon={Add01Icon}
            onClick={() => {
              close();
              onAddAccount();
            }}
          >
            {tx("Connect an inbox")}</MenuItem>
        </div>
      )}
    </Menu>
  );
}

export function Sidebar({
  accounts,
  selected,
  foldersBy,
  currentFolderId,
  showingAll,
  onShowAll,
  onPickAccount,
  onPickFolder,
  onAddAccount,
  onCompose,
  onOpenSettings,
  draftsFor,
  showingDrafts,
  onShowDrafts,
  extras = [],
  download,
  onChosen,
  onFolderAdded,
  onFolderDeleted,
  rail,
  onToggleRail,
  onBack,
  onForward,
}: {
  /** Showing only icons, with their names as tooltips. */
  rail: boolean;
  onToggleRail: () => void;
  /** Back to where they were, and forward again, when there's somewhere to go. */
  onBack?: () => void;
  onForward?: () => void;
  /** Something was picked, so a phone can go back to showing the list. */
  onChosen?: () => void;
  /** Things set aside, shown only when there's something in them. */
  extras?: {
    id: string;
    label: string;
    icon: IconSvgElement;
    colour?: string;
    count: number;
    current: boolean;
    onClick: () => void;
    /** A small word beside the name, such as "Beta". */
    tag?: string;
    /** Kept under More…, out of the way until it's opened. */
    more?: boolean;
  }[];
  /** How the download is getting on, while there's one going. */
  download?: { done: number; total: number | null } | null;
  accounts: Account[];
  selected: Account | null;
  foldersBy: Record<number, Folder[]>;
  currentFolderId: number | null;
  showingAll: boolean;
  onShowAll: () => void;
  onPickAccount: (id: number) => void;
  onPickFolder: (id: number) => void;
  onAddAccount: () => void;
  onFolderAdded: (folder: Folder) => void;
  onFolderDeleted: (folder: Folder, moved: number) => void;
  onCompose: () => void;
  onOpenSettings: () => void;
  /** How many drafts are waiting to be sent from this inbox. */
  draftsFor: (accountId: number) => number;
  showingDrafts: boolean;
  onShowDrafts: () => void;
}) {
  const { settings, change } = useSettings();
  const paused = settings.download_paused === "on";
  const [addingFolder, setAddingFolder] = useState(false);
  /** A right-click on an item: hiding or showing it, and deleting a folder of their own. */
  const [menu, setMenu] = useState<{ x: number; y: number; group: string; id: string; hidden: boolean; folder?: Folder } | null>(
    null,
  );
  const [deleting, setDeleting] = useState<Folder | null>(null);
  const [arrangement, setArrangement] = useState(readArrangement);
  /** Whether what's been hidden is showing, so it can be put back. */
  const [showingHidden, setShowingHidden] = useState(false);
  const [moreOpen, setMoreOpen] = useState(() => {
    try {
      return localStorage.getItem(MORE_KEY) === "open";
    } catch {
      return false;
    }
  });
  const toggleMore = () =>
    setMoreOpen((now) => {
      try {
        localStorage.setItem(MORE_KEY, now ? "closed" : "open");
      } catch {
        // Kept for now, though not for next time.
      }
      return !now;
    });
  /** With only icons showing, a name moves to a tooltip. */
  const named = (name: string) => (rail ? { "aria-label": name, "data-tip": name } : {});

  const keep = (group: string, change: { order?: string[]; hidden?: string[] }) =>
    setArrangement((now) => {
      const next = { ...now, [group]: { order: now[group]?.order ?? [], hidden: now[group]?.hidden ?? [], ...change } };
      try {
        localStorage.setItem(ARRANGEMENT_KEY, JSON.stringify(next));
      } catch {
        // Kept for now, though not for next time.
      }
      return next;
    });
  const hiddenIn = (group: string) => arrangement[group]?.hidden ?? [];
  const setHidden = (group: string, id: string, hide: boolean) =>
    keep(group, { hidden: hide ? [...hiddenIn(group), id] : hiddenIn(group).filter((other) => other !== id) });
  /** Ones dragged into place first, then the rest, hidden ones included, as they were. */
  const reorder = (group: string, ids: string[]) =>
    keep(group, { order: [...ids, ...(arrangement[group]?.order ?? []).filter((id) => !ids.includes(id))] });
  const openMenu = (e: React.MouseEvent, item: { group: string; id: string; hidden: boolean; folder?: Folder }) => {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY, ...item });
  };

  const views = arranged(extras, (x) => x.id, arrangement.views?.order ?? []);
  const shownViews = views.filter((x) => !hiddenIn("views").includes(x.id) && !x.more);
  const moreViews = views.filter((x) => !hiddenIn("views").includes(x.id) && x.more);
  // Whatever is open stays in sight, even with More… closed.
  const moreShowing = moreOpen || moreViews.some((x) => x.current);
  const hiddenViews = views.filter((x) => hiddenIn("views").includes(x.id));
  const folderGroup = selected ? `folders:${selected.email}` : "";
  const folders = arranged(selected ? (foldersBy[selected.id] ?? []) : [], (f) => f.path, arrangement[folderGroup]?.order ?? []);
  const shownFolders = folders.filter((f) => !hiddenIn(folderGroup).includes(f.path));
  const hiddenFolders = folders.filter((f) => hiddenIn(folderGroup).includes(f.path));
  const viewDrag = useDragOrder(
    shownViews.map((x) => x.id),
    (ids) => reorder("views", ids),
  );
  const folderDrag = useDragOrder(
    shownFolders.map((f) => f.path),
    (paths) => reorder(folderGroup, paths),
  );

  const viewRow = (x: (typeof extras)[number], hidden: boolean, i = 0, still = hidden) => {
    const { moving, ...row } = viewDrag.row(x.id, i);
    return (
      <button
        key={x.id}
        className={`nav-item ${moving ? "nav-item-moving" : ""} ${hidden ? "nav-item-hidden" : ""}`}
        aria-current={x.current}
        {...named(x.label)}
        {...(still ? {} : row)}
        onClick={() => {
          if (!still && !viewDrag.clicked()) return;
          onChosen?.();
          x.onClick();
        }}
        onContextMenu={(e) => openMenu(e, { group: "views", id: x.id, hidden })}
      >
        {x.colour ? <span className="nav-tag-dot"><TagDot colour={x.colour} /></span> : <Icon icon={x.icon} />}
        <span className="truncate-1 grow">{x.label}</span>
        {x.tag && <span className="pill pill-new nav-tag">{x.tag}</span>}
        {!still && <Icon icon={DragDropVerticalIcon} size="xs" className="nav-grip" />}
      </button>
    );
  };

  const folderRow = (f: Folder, hidden: boolean, i = 0) => {
    const { moving, ...row } = folderDrag.row(f.path, i);
    return (
      <button
        key={f.id}
        className={`nav-item ${moving ? "nav-item-moving" : ""} ${hidden ? "nav-item-hidden" : ""}`}
        aria-current={f.id === currentFolderId}
        {...named(f.role === "inbox" && f.unread > 0 ? tx("{name}, {n} unread", { name: folderLabel(f), n: f.unread }) : folderLabel(f))}
        {...(hidden ? {} : row)}
        onClick={() => {
          if (folderDrag.clicked()) onPickFolder(f.id);
        }}
        onContextMenu={(e) => openMenu(e, { group: folderGroup, id: f.path, hidden, folder: f })}
      >
        <Icon icon={FOLDER_ICONS[f.role] ?? Folder01Icon} />
        <span className="truncate-1 grow">{folderLabel(f)}</span>
        {f.role === "inbox" && <Count n={f.unread} />}
        {f.role === "drafts" && draftsFor(f.account_id) > 0 && (
          <span className="nav-count">{draftsFor(f.account_id)}</span>
        )}
        {!hidden && <Icon icon={DragDropVerticalIcon} size="xs" className="nav-grip" />}
      </button>
    );
  };

  /** The way back to everything hidden, views and folders alike, at the foot of the sidebar. */
  const hiddenCount = hiddenViews.length + hiddenFolders.length;
  const hiddenToggle = () =>
    hiddenCount > 0 && (
      <button
        type="button"
        className="nav-item nav-hidden-toggle"
        aria-expanded={showingHidden}
        {...named(tx("{n} hidden", { n: hiddenCount }))}
        onClick={() => setShowingHidden((now) => !now)}
      >
        <Icon icon={ViewOffSlashIcon} />
        <span className="truncate-1 grow">{tx("{n} hidden", { n: hiddenCount })}</span>
        <Icon icon={ArrowDown01Icon} size="xs" className="nav-hidden-arrow" />
      </button>
    );
  return (
    <nav aria-label={tx("Mailboxes")} className="sidebar">
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} label={tx("Sidebar item actions")} onClose={() => setMenu(null)}>
          <MenuItem
            icon={menu.hidden ? ViewIcon : ViewOffSlashIcon}
            onClick={() => {
              setHidden(menu.group, menu.id, !menu.hidden);
              setMenu(null);
            }}
          >
            {menu.hidden ? tx("Show in sidebar") : tx("Hide from sidebar")}</MenuItem>
          {menu.folder?.role === "folder" && (
            <MenuItem
              icon={Delete02Icon}
              onClick={() => {
                setDeleting(menu.folder ?? null);
                setMenu(null);
              }}
            >
              {tx("Delete folder…")}</MenuItem>
          )}
        </ContextMenu>
      )}
      {deleting && (
        <DeleteFolder
          folder={deleting}
          onClose={() => setDeleting(null)}
          onDeleted={(moved) => {
            onFolderDeleted(deleting, moved);
            setDeleting(null);
          }}
        />
      )}
      {addingFolder && selected && (
        <NewFolder
          account={selected}
          onClose={() => setAddingFolder(false)}
          onAdded={(folder) => {
            setAddingFolder(false);
            onFolderAdded(folder);
          }}
        />
      )}
      <div className={`sidebar-top sidebar-head ${onTheWeb ? "sidebar-head-web" : ""}`} data-tauri-drag-region>
        {onTheWeb && (
          <span className="sidebar-brand">
            <LogoMark />
            <span className="sidebar-brand-name">{tx("Mailzo")}</span>
          </span>
        )}
        <span className="nav-arrows">
          <IconButton icon={ArrowLeft01Icon} label={tx("Back")} shortcut="⌘[" small disabled={!onBack} onClick={onBack} />
          <IconButton icon={ArrowRight01Icon} label={tx("Forward")} shortcut="⌘]" small disabled={!onForward} onClick={onForward} />
        </span>
        <IconButton
          icon={SidebarLeftIcon}
          label={rail ? tx("Expand sidebar") : tx("Collapse sidebar")}
          shortcut="⌘\"
          small
          className="sidebar-toggle"
          onClick={onToggleRail}
        />
      </div>

      <div className="switcher-row">
        <AccountSwitcher
          accounts={accounts}
          selected={selected}
          foldersBy={foldersBy}
          onPick={onPickAccount}
          onAddAccount={onAddAccount}
        />
      </div>

      <button className="btn btn-primary compose-btn" {...named(tx("New message"))} data-tip-key={rail ? "⌘N" : undefined} onClick={onCompose}>
        <Icon icon={PencilEdit02Icon} size="s" />
        <span>{tx("New message")}</span>
        <span className="kbd">⌘N</span>
      </button>

      <div className="nav-scroll">
        <button className="nav-item" aria-current={showingAll} data-tip={tx("Every account's inbox")} data-tip-key="⌘1" {...named(tx("All Inbox"))} onClick={onShowAll}>
          <Icon icon={Mailbox01Icon} />
          <span className="truncate-1">{tx("All Inbox")}</span>
          <Count
            n={unreadIn(
              accounts.filter((a) => a.prefs.in_all_inbox).flatMap((a) => foldersBy[a.id] ?? []),
            )}
          />
        </button>
        <div ref={viewDrag.list} className={viewDrag.listClass}>
          {shownViews.map((x, i) => viewRow(x, false, i))}
        </div>
        {moreViews.length > 0 && (
          <>
            <button
              type="button"
              className="nav-item nav-more-toggle"
              aria-expanded={moreShowing}
              {...named(tx("More"))}
              onClick={toggleMore}
            >
              <Icon icon={MoreHorizontalIcon} />
              <span className="truncate-1 grow">{tx("More")}</span>
              <Icon icon={ArrowDown01Icon} size="xs" className="nav-hidden-arrow" />
            </button>
            {moreShowing && (
              <div className="nav-more-list">{moreViews.map((x) => viewRow(x, false, 0, true))}</div>
            )}
          </>
        )}
        <div className="nav-label-row">
          <div className="nav-label">{tx("Folders")}</div>
          {selected && (
            <IconButton icon={Add01Icon} label={tx("New folder")} small onClick={() => setAddingFolder(true)} />
          )}
        </div>
        <div ref={folderDrag.list} className={folderDrag.listClass}>
          {shownFolders.map((f, i) => folderRow(f, false, i))}
        </div>
        {/* A few providers have no Drafts folder, and drafts must still be findable. */}
        {selected &&
          draftsFor(selected.id) > 0 &&
          !(foldersBy[selected.id] ?? []).some((f) => f.role === "drafts") && (
          <>
            <div className="nav-gap" />
            <button
              className="nav-item"
              aria-current={showingDrafts}
              {...named("Drafts")}
              onClick={() => {
                onChosen?.();
                onShowDrafts();
              }}
            >
              <Icon icon={FileEditIcon} />
              <span className="truncate-1">{tx("Drafts")}</span>
            </button>
          </>
        )}
        {hiddenToggle()}
        {showingHidden && (
          <>
            {hiddenViews.map((x) => viewRow(x, true))}
            {hiddenFolders.map((f) => folderRow(f, true))}
          </>
        )}
      </div>

      <div className="sidebar-foot">
        <UpdateNote rail={rail} />
        {download && (
          <div className="sidebar-download" data-paused={paused || undefined}>
            <div className="sidebar-download-row">
              <div className="sidebar-download-text truncate-1 tnum">
                {download.total === null
                  ? tx("Connecting to your inbox")
                  : paused
                    ? tx("Paused {done}/{total}", { done: download.done.toLocaleString(language), total: download.total.toLocaleString(language) })
                    : tx("Downloading {done}/{total}", { done: download.done.toLocaleString(language), total: download.total.toLocaleString(language) })}
              </div>
              {download.total !== null && (
                <button
                  type="button"
                  className="link-btn sidebar-download-toggle"
                  data-tip={paused ? undefined : tx("New mail still comes in.")}
                  onClick={() => change("download_paused", paused ? "off" : "on")}
                >
                  {paused ? tx("Resume") : tx("Pause")}</button>
              )}
            </div>
            <DownloadBar done={download.done} total={download.total} />
          </div>
        )}
        {/* Always in view, so nobody has to hunt for them in a menu. */}
        <div className="sidebar-actions">
          <button
            type="button"
            className="nav-item"
            {...named(tx("Settings"))}
            onClick={() => {
              onChosen?.();
              onOpenSettings();
            }}
          >
            <Icon icon={Settings02Icon} />
            <span className="truncate-1">{tx("Settings")}</span>
            {!rail && <span className="nav-key">⌘,</span>}
          </button>
          <button
            type="button"
            className="nav-item"
            {...named(tx("Connect an inbox"))}
            onClick={() => {
              onChosen?.();
              onAddAccount();
            }}
          >
            <Icon icon={Add01Icon} />
            <span className="truncate-1">{tx("Connect an inbox")}</span>
          </button>
        </div>
      </div>
    </nav>
  );
}

function DeleteFolder({
  folder,
  onClose,
  onDeleted,
}: {
  folder: Folder;
  onClose: () => void;
  onDeleted: (moved: number) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const remove = async () => {
    setBusy(true);
    setProblem(null);
    try {
      onDeleted((await api.deleteFolder(folder.account_id, folder.id)).moved);
    } catch (e) {
      setProblem(e instanceof ApiError ? e.message : String(e));
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={`Delete ${folder.name}?`}
      width="s"
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tx("Keep")}</button>
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void remove()}>
            {busy && <Spinner size="xs" />}
            {tx("Delete folder")}</button>
        </>
      }
    >
      {tx("Any emails in it move to Archive.")}<Problem text={problem} />
    </Dialog>
  );
}

function NewFolder({
  account,
  onClose,
  onAdded,
}: {
  account: Account;
  onClose: () => void;
  onAdded: (folder: Folder) => void;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const create = async () => {
    if (!name.trim() || busy) return;
    setBusy(true);
    setProblem(null);
    try {
      onAdded(await api.createFolder(account.id, name));
    } catch (e) {
      setProblem(e instanceof ApiError ? e.message : String(e));
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={tx("New folder")}
      subtitle={account.prefs.label || account.email}
      width="s"
      onClose={onClose}
      onSubmit={(e) => {
        e.preventDefault();
        void create();
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tx("Cancel")}</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !name.trim()}>
            {busy && <Spinner size="xs" />}
            {tx("Create folder")}</button>
        </>
      }
    >
      <div className="stack">
        <input aria-label={tx("Folder name")} placeholder={tx("Folder name")} autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        <Problem text={problem} />
      </div>
    </Dialog>
  );
}
