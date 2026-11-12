import { memo, ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { copyText } from "../api";
import { shortDate } from "../format";
import {
  Copy01Icon,
  Archive02Icon,
  ArrowDown01Icon,
  ArrowLeft01Icon,
  FilterHorizontalIcon,
  Mail01Icon,
  MailOpen01Icon,
  UserGroupIcon,
  Menu01Icon,
  PinIcon,
  Delete02Icon,
  MoreVerticalIcon,
  Attachment01Icon,
  CheckmarkSquare01Icon,
  CheckmarkCircle02Icon,
  FileEditIcon,
  InboxIcon,
  Refresh01Icon,
  StarIcon,
  Tick02Icon,
} from "../icons";
import type { Draft, ThreadSummary } from "../types";
import { DraftMenu } from "./ThreadMenu";
import { TagPills } from "../tags";
import { Avatar, DownloadBar, Icon, IconButton, Menu, MenuItem, Spinner } from "./ui";
import { language, tx } from "../i18n";

const HOLD_MS = 500;

export function dayLabel(unix: number): string {
  const d = new Date(unix * 1000);
  const start = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const now = new Date();
  const days = Math.round((start(now) - start(d)) / 86_400_000);
  if (days <= 0) return tx("Today");
  if (days === 1) return tx("Yesterday");
  if (days < 7) return d.toLocaleDateString(language, { weekday: "long" });
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString(language, { month: "long" });
  return d.toLocaleDateString(language, { month: "long", year: "numeric" });
}

const DAY_MS = 86400000;

/** How long someone has been waiting, once it's been a while. */
function waitingText(lastDate: number): string | null {
  const days = Math.floor((Date.now() - lastDate * 1000) / DAY_MS);
  return days < 1 ? null : days === 1 ? tx("Waiting 1 day") : tx("Waiting {n} days", { n: days });
}

function dueText(dueAt: number): string {
  const days = Math.ceil((dueAt * 1000 - Date.now()) / DAY_MS);
  if (days < 0) return "Overdue";
  if (days === 0) return tx("Due today");
  if (days === 1) return tx("Due tomorrow");
  return `Due ${new Date(dueAt * 1000).toLocaleDateString(language, { month: "short", day: "numeric" })}`;
}

export const lead = (t: ThreadSummary) =>
  t.participants.find((p) => p !== "me") ?? t.participants[0] ?? "?";

function DraftRow({
  draft,
  onOpen,
  onDiscard,
}: {
  draft: Draft;
  onOpen: (id: number) => void;
  onDiscard?: (id: number) => void;
}) {
  const people = [...draft.to, ...draft.cc, ...draft.bcc].map((a) => a.name || a.address);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  return (
    <>
      {menu && onDiscard && (
        <DraftMenu
          x={menu.x}
          y={menu.y}
          onOpen={() => onOpen(draft.id)}
          onDiscard={() => onDiscard(draft.id)}
          onClose={() => setMenu(null)}
        />
      )}
      <button
        className="thread"
        data-menu-open={menu !== null || undefined}
        onClick={() => onOpen(draft.id)}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY });
        }}
      >
        <span className="thread-avatar">
          <Avatar name={people[0] ?? "?"} email={draft.to[0]?.address} />
        </span>
        <span className="grow">
          <span className="thread-line">
            <span className="thread-from truncate-1">{people.join(", ") || tx("(no one yet)")}</span>
            <span className="thread-time">{shortDate(draft.updated_at)}</span>
          </span>
          <span className="thread-subject truncate-1">{draft.subject || tx("(no subject)")}</span>
          <span className="thread-snippet truncate-1">{draft.text.trim() || " "}</span>
        </span>
      </button>
    </>
  );
}

/** How often a new inbox's first download is counted while it's showing. */
const FIRST_LOAD_EVERY_MS = 3_000;

/** A new inbox's first download, counted as it comes in. */
function FirstLoad({ done, total, refresh }: { done: number; total: number | null; refresh: () => void }) {
  const again = useRef(refresh);
  again.current = refresh;
  useEffect(() => {
    const timer = setInterval(() => again.current(), FIRST_LOAD_EVERY_MS);
    return () => clearInterval(timer);
  }, []);
  const known = total !== null && total > 0;
  return (
    <div className="empty first-load" role="status" aria-live="polite">
      <div className="empty-icon">
        <Spinner />
      </div>
      <div className="empty-title">{tx("Getting your mail")}</div>
      <div className="empty-text tnum">
        {known
          ? tx("{done} of {total} emails", {
              done: Math.min(done, total).toLocaleString(language),
              total: total.toLocaleString(language),
            })
          : tx("Connecting to your inbox")}
      </div>
      <DownloadBar done={done} total={total} />
      <div className="empty-text">{tx("Newest emails show first, as they arrive.")}</div>
    </div>
  );
}

export const TABS: [string, string][] = [
  ["all", tx("All")],
  ["needs_you", tx("Needs you")],
  ["good_to_know", tx("Good to know")],
  ["rest", tx("The rest")],
];

const TEAM_TABS: [string, string][] = [
  ["team_open", tx("Open")],
  ["team_mine", tx("Mine")],
  ["team_unassigned", tx("Unassigned")],
  ["team_done", tx("Done")],
];

/** How a team looks through an inbox it shares. */
export function TeamTabs({ tab, onPick }: { tab: string; onPick: (tab: string) => void }) {
  return (
    <div role="tablist" aria-label={tx("Team view")} className="tabs">
      {TEAM_TABS.map(([id, name]) => (
        <button
          key={id}
          type="button"
          role="tab"
          aria-selected={tab === id}
          className="tab"
          onClick={() => onPick(id)}
        >
          {name}
        </button>
      ))}
    </div>
  );
}

/** Big counts kept short, as other mail apps do: 1,240 shows as 1.2K and 17,322 as 17K. The full number is on hover. */
const compactSmall = new Intl.NumberFormat(language, { notation: "compact", maximumFractionDigits: 1 });
const compactBig = new Intl.NumberFormat(language, { notation: "compact", maximumFractionDigits: 0 });
const shortCount = (n: number) => (n < 1000 ? String(n) : n < 10_000 ? compactSmall.format(n) : compactBig.format(n));

export function InboxTabs({
  tab,
  counts,
  onPick,
}: {
  tab: string;
  counts: Record<string, number>;
  onPick: (tab: string) => void;
}) {
  // With more tabs than fit, the row scrolls sideways; the open one is kept in sight.
  const row = useRef<HTMLDivElement>(null);
  useEffect(() => {
    row.current
      ?.querySelector<HTMLElement>('[aria-selected="true"]')
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [tab]);
  return (
    <div ref={row} role="tablist" aria-label={tx("Sort inbox")} className="tabs tabs-inbox">
      {TABS.map(([id, name]) => (
        <button
          key={id}
          type="button"
          role="tab"
          aria-selected={tab === id}
          className="tab"
          onClick={() => onPick(id)}
        >
          <span className="tab-name">{name}</span>
          {(counts[id] ?? 0) > 0 && (
            <span className="tab-count tnum" data-tip={counts[id] >= 1000 ? counts[id].toLocaleString(language) : undefined}>
              {shortCount(counts[id])}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}

/**
 * A sign-in code from the email, copied in one click; then `onCopied` does what Settings says
 * with the email. Inside a row it's a span, as a row is a button already.
 */
export function CodeChip({ code, onCopied, inRow = false }: { code: string; onCopied: () => void; inRow?: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = (e: React.MouseEvent) => {
    e.stopPropagation();
    void copyText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
      onCopied();
    });
  };
  const inside = (
    <>
      <Icon icon={Copy01Icon} size="xs" />
      <span className="tnum">{copied ? tx("Copied") : code}</span>
    </>
  );
  return inRow ? (
    <span role="button" className="code-chip" data-tip={tx("Copy OTP")} aria-label={`${tx("Copy OTP")} ${code}`} onClick={copy}>
      {inside}
    </span>
  ) : (
    <button type="button" className="code-chip" data-tip={tx("Copy OTP")} aria-label={`${tx("Copy OTP")} ${code}`} onClick={copy}>
      {inside}
    </button>
  );
}

/**
 * What's inside a row. The list, the menu bar panel and the preview in Settings all draw this one,
 * so they always match.
 */
export function ThreadRowBody({
  t,
  showPictures,
  unreadBar = true,
  check,
  quick,
  oneLine = false,
  onCopyCode,
}: {
  t: ThreadSummary;
  showPictures: boolean;
  /** Copies its sign-in code, if it has one. */
  onCopyCode?: () => void;
  /** Everything on one line, for the full-width layout. */
  oneLine?: boolean;
  /** Left out where everything shown is unread anyway. */
  unreadBar?: boolean;
  /** The tick for selecting it, sitting over the picture. */
  check?: ReactNode;
  /** Buttons that take the date's place while the pointer is on the row. */
  quick?: ReactNode;
}) {
  if (oneLine) {
    const waiting = t.needs_you ? waitingText(t.last_date) : null;
    return (
      <>
        {unreadBar && t.unread_count > 0 && <span aria-label={tx("Unread")} className="unread-bar" />}
        <span className="thread-avatar">
          <Avatar name={lead(t)} email={showPictures && !t.risk ? t.lead_address : undefined} size="s" />
          {check}
        </span>
        <span className="thread-one">
          <span className="thread-from truncate-1">
            {t.participants.join(", ") || tx("(unknown sender)")}
            {t.message_count > 1 && <span className="thread-count">{t.message_count}</span>}
          </span>
          <span className="thread-text truncate-1">
            <span className="thread-subject">{t.subject || tx("(no subject)")}</span>
            <span className="thread-snippet">{t.ai_line || t.snippet || ""}</span>
          </span>
          {t.risk > 0 ? (
            <span className="pill pill-risk">{t.risk === 2 ? tx("Looks like spam") : tx("Be careful")}</span>
          ) : waiting ? (
            <span className="pill" data-tip={t.why ?? undefined}>{waiting}</span>
          ) : t.needs_you && t.why ? (
            <span className="pill">{t.why}</span>
          ) : t.follow_due ? (
            <span className="pill pill-later">{tx("No reply yet")}</span>
          ) : null}
          {(t.has_attachments || t.flagged || t.pinned) && (
            <span className="thread-marks">
              {t.pinned && (
                <span aria-label={tx("Pinned")} className="hstack pin-mark">
                  <Icon icon={PinIcon} size="xs" />
                </span>
              )}
              {t.has_attachments && (
                <span aria-label={tx("Has attachments")} className="hstack">
                  <Icon icon={Attachment01Icon} size="xs" />
                </span>
              )}
              {t.flagged && (
                <span aria-label={tx("Starred")} className="hstack star-on">
                  <Icon icon={StarIcon} size="xs" />
                </span>
              )}
            </span>
          )}
          {t.code && onCopyCode && <CodeChip code={t.code} onCopied={onCopyCode} inRow />}
          <span className="thread-time">{shortDate(t.last_date)}</span>
          {quick && <span className="thread-quick">{quick}</span>}
        </span>
      </>
    );
  }
  return (
    <>
      {unreadBar && t.unread_count > 0 && <span aria-label={tx("Unread")} className="unread-bar" />}
      <span className="thread-avatar">
        <Avatar name={lead(t)} email={showPictures && !t.risk ? t.lead_address : undefined} />
        {check}
      </span>
      <span className="grow">
        <span className="thread-line">
          <span className="thread-from truncate-1">
            {t.participants.join(", ") || tx("(unknown sender)")}
          </span>
          <span className="thread-time">{shortDate(t.last_date)}</span>
          {quick && <span className="thread-quick">{quick}</span>}
        </span>
        <span className="thread-line">
          <span className="thread-subject truncate-1">
            {t.subject || tx("(no subject)")}
          </span>
          {t.code && onCopyCode && <CodeChip code={t.code} onCopied={onCopyCode} inRow />}
          {t.pinned && (
            <span aria-label={tx("Pinned")} className="hstack pin-mark" data-tip={tx("Pinned")}>
              <Icon icon={PinIcon} size="xs" />
            </span>
          )}
          {t.needs_you && t.due_at !== null && (
            <span className="pill pill-later">{dueText(t.due_at)}</span>
          )}
          {t.needs_you && waitingText(t.last_date) && (
            <span className="pill" data-tip={t.why ?? undefined}>{waitingText(t.last_date)}</span>
          )}
          {t.needs_you && !waitingText(t.last_date) && t.why && <span className="pill">{t.why}</span>}
          {t.follow_due && <span className="pill pill-later">{tx("No reply yet")}</span>}
          {t.returned_at !== null && !t.follow_due && <span className="pill pill-later">{tx("Back")}</span>}
          {t.snooze_until !== null && (
            <span className="pill">
              {new Date(t.snooze_until * 1000).toLocaleString(language, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}
            </span>
          )}
          {t.follow_at !== null && t.snooze_until === null && (
            <span className="pill" data-tip={tx("You'll be reminded if there's no reply")}>
              {new Date(t.follow_at * 1000).toLocaleDateString(language, { month: "short", day: "numeric" })}
            </span>
          )}
          {t.risk > 0 && (
            <span className="pill pill-risk">{t.risk === 2 ? tx("Looks like spam") : tx("Be careful")}</span>
          )}
          {t.label && !t.risk && (
            <span className="pill">{t.label === "receipt" ? "Receipt" : tx("Needs reply")}</span>
          )}
          <TagPills tags={t.tags} />
          {t.team?.done && <span className="pill">{tx("Done")}</span>}
          {t.team && t.team.comments > 0 && (
            <span aria-label={tx("{0} team notes", { "0": t.team.comments })} className="pill" data-tip={tx("Team notes")}>
              {t.team.comments === 1 ? tx("1 note") : tx("{n} notes", { n: t.team.comments })}
            </span>
          )}
          {t.team?.assignee_email && (
            <span
              className="team-holder"
              data-tip={t.team.mine ? tx("Assigned to you") : tx("Assigned to {0}", { "0": t.team.assignee_email })}
              aria-label={t.team.mine ? tx("Assigned to you") : tx("Assigned to {0}", { "0": t.team.assignee_email })}
            >
              <Avatar name={t.team.assignee_email} size="xs" />
            </span>
          )}
          {t.message_count > 1 && (
            <span className="thread-count">{t.message_count}</span>
          )}

        </span>
        {t.team && t.team.replying.length > 0 && (
          <span className="team-replying-row truncate-1">
            {t.team.replying.length === 1 ? tx("{who} is replying", { who: t.team.replying.join(", ") }) : tx("{who} are replying", { who: t.team.replying.join(", ") })}
          </span>
        )}
        <span className="thread-line">
          <span className="thread-snippet truncate-1 grow">{t.ai_line || t.snippet || " "}</span>
          {(t.has_attachments || t.flagged) && (
            <span className="thread-marks">
              {t.has_attachments && (
                <span aria-label={tx("Has attachments")} className="hstack">
                  <Icon icon={Attachment01Icon} size="xs" />
                </span>
              )}
              {t.flagged && (
                <span aria-label={tx("Starred")} className="hstack star-on">
                  <Icon icon={StarIcon} size="xs" />
                </span>
              )}
            </span>
          )}
        </span>
      </span>
    </>
  );
}

export const threadKey = (t: { account_id: number; thread_id: string }) =>
  `${t.account_id}:${t.thread_id}`;

/** What a row does, kept up to date without making every row draw again. */
type RowActions = {
  picking: boolean;
  onPick: (t: ThreadSummary) => void;
  onPeek?: (t: ThreadSummary) => void;
  onTogglePicked: (t: ThreadSummary, range: boolean) => void;
  onQuick?: (t: ThreadSummary, to: string) => void;
  onRowMenu?: (t: ThreadSummary, x: number, y: number) => void;
  onCopyCode?: (t: ThreadSummary) => void;
};

/** Drawn again only when something about this row changes, so opening an email stays instant in a long list. */
const ThreadRow = memo(function ThreadRow({
  t,
  isPicked,
  isOpen,
  menuOpen,
  showPictures,
  quickActions,
  oneLine,
  does,
}: {
  oneLine: boolean;
  t: ThreadSummary;
  isPicked: boolean;
  isOpen: boolean;
  menuOpen: boolean;
  showPictures: boolean;
  quickActions: boolean;
  does: React.RefObject<RowActions>;
}) {
  const isUnread = t.unread_count > 0;
  const held = useRef<ReturnType<typeof setTimeout> | null>(null);
  const heldOpened = useRef(false);
  return (
    <button
      className={`thread ${isUnread ? "thread-unread" : ""} ${isPicked ? "thread-picked" : ""} ${oneLine ? "thread-one-line" : ""}`}
      aria-current={isOpen}
      data-key={threadKey(t)}
      data-menu-open={menuOpen || undefined}
      onMouseEnter={() => does.current.onPeek?.(t)}
      onContextMenu={(e) => {
        e.preventDefault();
        does.current.onRowMenu?.(t, e.clientX, e.clientY);
      }}
      onTouchStart={(e) => {
        // Holding a row is the right-click of a phone.
        const { clientX, clientY } = e.touches[0];
        heldOpened.current = false;
        held.current = setTimeout(() => {
          held.current = null;
          heldOpened.current = true;
          does.current.onRowMenu?.(t, clientX, clientY);
        }, HOLD_MS);
      }}
      onTouchEnd={() => {
        if (held.current) clearTimeout(held.current);
        held.current = null;
      }}
      onTouchMove={() => {
        if (held.current) clearTimeout(held.current);
        held.current = null;
      }}
      onTouchCancel={() => {
        if (held.current) clearTimeout(held.current);
        held.current = null;
        heldOpened.current = false;
      }}
      onClick={(e) => {
        if (heldOpened.current) {
          heldOpened.current = false;
          return;
        }
        if (e.metaKey || e.ctrlKey || e.shiftKey || does.current.picking) {
          does.current.onTogglePicked(t, e.shiftKey);
        } else {
          does.current.onPick(t);
        }
      }}
    >
    <ThreadRowBody
        t={t}
        showPictures={showPictures}
        oneLine={oneLine}
        onCopyCode={does.current.onCopyCode ? () => does.current.onCopyCode?.(t) : undefined}
        check={
          <span
            role="checkbox"
            aria-checked={isPicked}
            aria-label={`Select ${t.subject || "conversation"}`}
            data-tip={tx("Select")}
            className="thread-check"
            onClick={(e) => {
              e.stopPropagation();
              does.current.onTogglePicked(t, e.shiftKey);
            }}
          >
            {isPicked && <Icon icon={Tick02Icon} size="xs" />}
          </span>
        }
        quick={
          quickActions && (
            <>
              {t.needs_you && (
                <span
                  role="button"
                  aria-label={tx("Done with {0}", { "0": t.subject || "conversation" })}
                  data-tip={tx("Done (D)")}
                  className="quick-action"
                  onClick={(e) => {
                    e.stopPropagation();
                    does.current.onQuick?.(t, "done");
                  }}
                >
                  <Icon icon={CheckmarkCircle02Icon} size="s" />
                </span>
              )}
              <span
                role="button"
                aria-label={`Archive ${t.subject || "conversation"}`}
                data-tip={tx("Archive")}
                className="quick-action"
                onClick={(e) => {
                  e.stopPropagation();
                  does.current.onQuick?.(t, "archive");
                }}
              >
                <Icon icon={Archive02Icon} size="s" />
              </span>
              <span
                role="button"
                aria-label={`Delete ${t.subject || "conversation"}`}
                data-tip={tx("Delete")}
                className="quick-action"
                onClick={(e) => {
                  e.stopPropagation();
                  does.current.onQuick?.(t, "trash");
                }}
              >
                <Icon icon={Delete02Icon} size="s" />
              </span>
              <span
                role="button"
                aria-label={tx("More actions for {0}", { "0": t.subject || "conversation" })}
                data-tip={tx("More")}
                className="quick-action"
                onClick={(e) => {
                  e.stopPropagation();
                  does.current.onRowMenu?.(t, e.clientX, e.clientY);
                }}
              >
                <Icon icon={MoreVerticalIcon} size="s" />
              </span>
            </>
          )
        }
      />
    </button>
  );
});

export function ThreadList({
  title,
  threads,
  selected,
  showPictures,
  refreshing,
  refreshTip,
  onSync,
  onPick,
  onPeek,
  picked,
  onTogglePicked,
  top,
  sub,
  emptyTitle = tx("Nothing here"),
  loading = false,
  firstLoad = null,
  foot,
  onToggleUnread,
  onMarkAllRead,
  unreadOnly = false,
  onToggleAll,
  onRowMenu,
  onQuick,
  onCopyCode,
  menuFor,
  drafts = [],
  onOpenDraft,
  onDiscardDraft,
  onNearEnd,
  onBySender,
  onBack,
  onPhoneNav,
  oneLine = false,
  search,
}: {
  /** One line a row, for the full-width layout. */
  oneLine?: boolean;
  /** Search controls in the header beside the title: the box, where the full-width layout has
   * room for it, and which inbox to look in. */
  search?: React.ReactNode;
  /** Shows the folders, on a phone where they don't fit beside the list. */
  onPhoneNav?: () => void;
  /** The bottom of what's loaded is coming into view. */
  onNearEnd?: () => void;
  /** Shows everyone with mail here instead of each email. */
  onBySender?: () => void;
  /** Back to where this list was opened from. */
  onBack?: () => void;
  title: string;
  threads: ThreadSummary[];
  selected: string | null;
  /** Off in spam, where a company's logo could make a fake email look genuine. */
  showPictures: boolean;
  refreshing: boolean;
  /** What refreshing is doing right now, or when it last finished. */
  refreshTip?: string;
  onSync: () => void;
  onPick: (t: ThreadSummary) => void;
  /** The pointer is on this row, so it's likely to be opened next. */
  onPeek?: (t: ThreadSummary) => void;
  /** Conversations ticked for acting on together. */
  picked: Set<string>;
  onTogglePicked: (t: ThreadSummary, range: boolean) => void;
  /** Sits above the list. */
  top?: React.ReactNode;
  /** Beside the title, such as how many are unread or were found. */
  sub?: string;
  /** What to say when the list is empty. */
  emptyTitle?: string;
  /** The list is on its way, so an empty one doesn't mean there's no mail. */
  loading?: boolean;
  /** A new inbox's mail is still arriving: how many of its emails are here so far. */
  firstLoad?: { done: number; total: number | null; refresh: () => void } | null;
  /** Under the last row. */
  foot?: React.ReactNode;
  /** Offers showing only what hasn't been read. */
  onToggleUnread?: () => void;
  unreadOnly?: boolean;
  /** Offered while showing only what hasn't been read. */
  onMarkAllRead?: () => void;
  /** Selects everything in the list, or clears it if all of it is selected. */
  onToggleAll?: () => void;
  /** Archiving or deleting from the row itself. Left out where that makes no sense. */
  onQuick?: (t: ThreadSummary, to: string) => void;
  /** Right-clicking a conversation. */
  onRowMenu?: (t: ThreadSummary, x: number, y: number) => void;
  /** Copying a sign-in code from its row. */
  onCopyCode?: (t: ThreadSummary) => void;
  /** The one whose right-click menu is showing. */
  menuFor?: string | null;
  /** Written here but not sent yet, shown above what's on the mail server. */
  drafts?: Draft[];
  onOpenDraft?: (id: number) => void;
  onDiscardDraft?: (id: number) => void;
}) {
  const picking = picked.size > 0;
  const does = useRef<RowActions>({ picking, onPick, onPeek, onTogglePicked, onQuick, onRowMenu, onCopyCode });
  does.current = { picking, onPick, onPeek, onTogglePicked, onQuick, onRowMenu, onCopyCode };
  const groups = useMemo(() => {
    const out: { label: string; items: ThreadSummary[] }[] = [];
    for (const t of threads) {
      const label = t.best_match
        ? tx("Best matches")
        : t.pinned
          ? "Pinned"
          : t.follow_due || t.returned_at !== null
            ? tx("Needs a look")
            : dayLabel(t.last_date);
      const last = out[out.length - 1];
      if (last?.label === label) last.items.push(t);
      else out.push({ label, items: [t] });
    }
    return out;
  }, [threads]);

  return (
    <main className={`list ${picking ? "list-picking" : ""}`}>
      <div className="list-head" data-tauri-drag-region="deep">
        {onPhoneNav && !onBack && (
          <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />
        )}
        {onBack && <IconButton icon={ArrowLeft01Icon} label={tx("Back")} small onClick={onBack} />}
        <h2 className="list-title">{title}</h2>
        {picking ? (
          <span className="list-sub">{picked.size} selected</span>
        ) : (
          sub && <span className="list-sub">{sub}</span>
        )}
        {search}
        {unreadOnly && onMarkAllRead && threads.length > 0 && (
          <IconButton
            icon={MailOpen01Icon}
            label={tx("Mark all as read")}
            small
            className="list-refresh"
            onClick={onMarkAllRead}
          />
        )}
        {(onToggleUnread || (onBySender && threads.length > 0)) && (
          <Menu
            label={tx("Filter")}
            align="end"
            trigger={(props) => (
              <button type="button" className="list-filter" aria-pressed={unreadOnly} {...props}>
                <Icon icon={FilterHorizontalIcon} size="xs" />
                {unreadOnly ? tx("Unread") : tx("Filter")}
                <Icon icon={ArrowDown01Icon} size="xs" />
              </button>
            )}
          >
            {(close) => (
              <>
                {onToggleUnread && (
                  <MenuItem
                    icon={Mail01Icon}
                    trailing={unreadOnly ? <Icon icon={Tick02Icon} size="s" /> : undefined}
                    onClick={() => {
                      close();
                      onToggleUnread();
                    }}
                  >
                    {tx("Unread only")}</MenuItem>
                )}
                {onBySender && threads.length > 0 && (
                  <MenuItem
                    icon={UserGroupIcon}
                    onClick={() => {
                      close();
                      onBySender();
                    }}
                  >
                    {tx("Group by sender")}</MenuItem>
                )}
              </>
            )}
          </Menu>
        )}
        {onToggleAll && threads.length > 0 && (
          <IconButton
            icon={CheckmarkSquare01Icon}
            label={picked.size === threads.length ? tx("Clear selection") : tx("Select all")}
            shortcut="⌘A"
            small
            className="list-refresh"
            onClick={onToggleAll}
          />
        )}
        <IconButton
          icon={Refresh01Icon}
          label={refreshing ? tx("Refreshing mail") : tx("Refresh mail")}
          tip={refreshTip}
          small
          className="list-refresh"
          disabled={refreshing}
          spinning={refreshing}
          onClick={onSync}
        />
      </div>
      {top}
      <div
        className="list-scroll"
        onScroll={(e) => {
          const el = e.currentTarget;
          // Three screens ahead, so the next page is usually in before anyone gets there.
          if (onNearEnd && el.scrollTop + el.clientHeight > el.scrollHeight - el.clientHeight * 3) onNearEnd();
        }}
      >
        {drafts.length > 0 && (
          <section aria-label={tx("Not sent yet")}>
            <div className="day-label">{tx("Not sent yet")}</div>
            {drafts.map((d) => (
              <DraftRow key={d.id} draft={d} onOpen={(id) => onOpenDraft?.(id)} onDiscard={onDiscardDraft} />
            ))}
          </section>
        )}
        {threads.length === 0 && drafts.length === 0 && firstLoad ? (
          <FirstLoad done={firstLoad.done} total={firstLoad.total} refresh={firstLoad.refresh} />
        ) : threads.length === 0 && drafts.length === 0 && loading ? (
          <div className="empty" role="status" aria-label={tx("Loading")}>
            <Spinner />
          </div>
        ) : threads.length === 0 && drafts.length === 0 ? (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={InboxIcon} size="l" />
            </div>
            <div className="empty-title">{emptyTitle}</div>
            <div className="empty-text">{unreadOnly ? tx("You've read everything here.") : tx("No mail in this folder.")}</div>
          </div>
        ) : (
          groups.map((g) => (
            <section key={`${g.label}:${threadKey(g.items[0])}`} aria-label={g.label}>
              <div className="day-label">{g.label}</div>
              {g.items.map((t) => {
                const key = threadKey(t);
                return (
                  <ThreadRow
                    key={key}
                    t={t}
                    isPicked={picked.has(key)}
                    isOpen={key === selected}
                    menuOpen={key === menuFor}
                    showPictures={showPictures}
                    oneLine={oneLine}
                  quickActions={!!onQuick}
                    does={does}
                  />
                );
              })}
            </section>
          ))
        )}
        {foot}
      </div>
    </main>
  );
}

export function DraftList({
  drafts,
  top,
  onOpen,
  onDiscard,
  onPhoneNav,
}: {
  drafts: Draft[];
  top?: React.ReactNode;
  onOpen: (id: number) => void;
  onDiscard: (id: number) => void;
  onPhoneNav?: () => void;
}) {
  return (
    <main className="list">
      <div className="list-head" data-tauri-drag-region="deep">
        {onPhoneNav && <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />}
        <h2 className="list-title">{tx("Drafts")}</h2>
      </div>
      {top}
      <div className="list-scroll">
        {drafts.length === 0 ? (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={FileEditIcon} size="l" />
            </div>
            <div className="empty-title">{tx("No drafts")}</div>
          </div>
        ) : (
          drafts.map((d) => <DraftRow key={d.id} draft={d} onOpen={onOpen} onDiscard={onDiscard} />)
        )}
      </div>
    </main>
  );
}
