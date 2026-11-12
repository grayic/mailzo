import { useState } from "react";
import {
  Tick02Icon,
  MoreHorizontalIcon,
  AlarmClockIcon,
  Clock01Icon,
  PinIcon,
  PinOffIcon,
  VolumeOffIcon,
  Archive02Icon,
  InboxIcon,
  ArrowLeft01Icon,
  ArrowTurnForwardIcon,
  Cancel01Icon,
  CheckmarkCircle02Icon,
  MailBlock01Icon,
  Tag01Icon,
  UserBlock01Icon,
  CheckmarkSquare01Icon,
  Delete02Icon,
  FolderExportIcon,
  Mail01Icon,
  MailOpen01Icon,
  MailReply01Icon,
  MailReplyAll01Icon,
  StarIcon,
} from "../icons";
import type { Draft, Folder, ThreadSummary } from "../types";
import { MoveToItems } from "./BulkPanel";
import { WhenItems } from "./Later";
import { ContextMenu, MenuItem } from "./ui";
import { tx } from "../i18n";

export type ThreadAction =
  | { kind: "select" }
  | { kind: "select_all" }
  | { kind: "clear_selection" }
  | { kind: "compose"; mode: Draft["mode"] }
  | { kind: "mark"; action: "seen" | "unseen" | "star" | "unstar" }
  | { kind: "move"; to: string }
  | { kind: "delete_forever" }
  | { kind: "spam"; block: boolean }
  | { kind: "not_spam" }
  | { kind: "sort"; category: string }
  | { kind: "pin"; on: boolean }
  | { kind: "mute"; on: boolean }
  | { kind: "snooze"; at: number | null }
  | { kind: "pick_snooze" }
  | { kind: "follow_up"; at: number | null }
  | { kind: "pick_follow_up" }
  | { kind: "needs_me"; on: boolean };

const TABS: [string, string][] = [
  ["general", tx("Good to know")],
  ["promotions", tx("The rest")],
];

/** What opens when an email is right-clicked. If it's part of a selection, the
 * actions are for everything selected. */
export function ThreadMenu({
  x,
  y,
  thread,
  isPicked,
  selectedCount,
  allPicked,
  folders,
  inBin,
  inJunk,
  inArchive,
  canGoToInbox,
  onAction,
  onClose,
}: {
  x: number;
  y: number;
  thread: ThreadSummary;
  isPicked: boolean;
  /** How many this acts on, when it's more than the one clicked. */
  selectedCount: number;
  /** Everything in the list is already selected. */
  allPicked: boolean;
  /** The folders to offer, when everything is in one inbox. */
  folders: Folder[] | null;
  inBin: boolean;
  inJunk: boolean;
  inArchive: boolean;
  canGoToInbox: boolean;
  onAction: (action: ThreadAction) => void;
  onClose: () => void;
}) {
  const [page, setPage] = useState<"main" | "more" | "move" | "sort" | "snooze" | "follow">("main");
  const moving = page === "move";
  const many = selectedCount > 1;
  const act = (action: ThreadAction) => {
    onClose();
    onAction(action);
  };

  return (
    <ContextMenu x={x} y={y} label={tx("Email actions")} page={page} onClose={onClose}>
      {moving ? (
        <>
          <MenuItem icon={ArrowLeft01Icon} onClick={() => setPage("main")}>
            {tx("Move to")}</MenuItem>
          <div className="menu-sep" />
          <MoveToItems
            folders={folders?.filter((f) => many || f.id !== thread.folder_id) ?? null}
            onMove={(to) => act({ kind: "move", to })}
          />
        </>
      ) : page === "snooze" ? (
        <>
          <MenuItem icon={ArrowLeft01Icon} onClick={() => setPage("main")}>
            {tx("Snooze until")}</MenuItem>
          <div className="menu-sep" />
          <WhenItems close={onClose} onPick={(at) => onAction({ kind: "snooze", at })} onOther={() => onAction({ kind: "pick_snooze" })} />
        </>
      ) : page === "follow" ? (
        <>
          <MenuItem icon={ArrowLeft01Icon} onClick={() => setPage("main")}>
            {tx("Remind me if there's no reply by")}</MenuItem>
          <div className="menu-sep" />
          <WhenItems close={onClose} onPick={(at) => onAction({ kind: "follow_up", at })} onOther={() => onAction({ kind: "pick_follow_up" })} />
        </>
      ) : page === "sort" ? (
        <>
          <MenuItem icon={ArrowLeft01Icon} onClick={() => setPage("main")}>
            {tx("Always put this sender on")}</MenuItem>
          <div className="menu-sep" />
          {TABS.map(([category, name]) => (
            <MenuItem key={category} icon={Tag01Icon} onClick={() => act({ kind: "sort", category })}>
              {name}
            </MenuItem>
          ))}
        </>
      ) : page === "more" ? (
        <>
          <MenuItem icon={ArrowLeft01Icon} onClick={() => setPage("main")}>
            {tx("More")}</MenuItem>
          <div className="menu-sep" />
          {!isPicked && (
            <MenuItem icon={CheckmarkSquare01Icon} onClick={() => act({ kind: "select" })}>
              {tx("Select")}</MenuItem>
          )}
          {!allPicked && (
            <MenuItem icon={CheckmarkSquare01Icon} onClick={() => act({ kind: "select_all" })}>
              {tx("Select all")}</MenuItem>
          )}
          <MenuItem icon={MailReplyAll01Icon} onClick={() => act({ kind: "compose", mode: "reply_all" })}>
            {tx("Reply all")}</MenuItem>
          <div className="menu-sep" />
          {thread.follow_at || thread.follow_due ? (
            <MenuItem icon={AlarmClockIcon} onClick={() => act({ kind: "follow_up", at: null })}>
              {tx("Cancel reminder")}</MenuItem>
          ) : (
            <MenuItem icon={AlarmClockIcon} onClick={() => setPage("follow")}>
              {tx("Remind me…")}</MenuItem>
          )}
          <MenuItem icon={thread.pinned ? PinOffIcon : PinIcon} onClick={() => act({ kind: "pin", on: !thread.pinned })}>
            {thread.pinned ? "Unpin" : tx("Pin to the top")}
          </MenuItem>
          <MenuItem icon={VolumeOffIcon} onClick={() => act({ kind: "mute", on: !thread.muted })}>
            {thread.muted ? "Unmute" : tx("Mute conversation")}
          </MenuItem>
          {!inJunk && (
            <MenuItem icon={Tag01Icon} onClick={() => setPage("sort")}>
              {tx("Always put on tab…")}</MenuItem>
          )}
          {!inJunk && (
            <>
              <div className="menu-sep" />
              <MenuItem icon={MailBlock01Icon} onClick={() => act({ kind: "spam", block: false })}>
                {tx("Mark as spam")}</MenuItem>
              <MenuItem icon={UserBlock01Icon} onClick={() => act({ kind: "spam", block: true })}>
                {tx("Block sender")}</MenuItem>
            </>
          )}
        </>
      ) : (
        <>
          {many && <div className="menu-title tnum">{selectedCount} selected</div>}
          {(many || isPicked) && (
            <>
              <MenuItem
                icon={Cancel01Icon}
                onClick={() => act(many ? { kind: "clear_selection" } : { kind: "select" })}
              >
                {many ? tx("Clear selection") : "Deselect"}
              </MenuItem>
              <div className="menu-sep" />
            </>
          )}
          {!many && (
            <>
              <MenuItem icon={MailReply01Icon} onClick={() => act({ kind: "compose", mode: "reply" })}>
                {tx("Reply")}</MenuItem>
              <MenuItem icon={ArrowTurnForwardIcon} onClick={() => act({ kind: "compose", mode: "forward" })}>
                {tx("Forward")}</MenuItem>
              <div className="menu-sep" />
            </>
          )}
          {many || thread.unread_count === 0 ? (
            <MenuItem icon={Mail01Icon} onClick={() => act({ kind: "mark", action: "unseen" })}>
              {tx("Mark unread")}</MenuItem>
          ) : null}
          {many || thread.unread_count > 0 ? (
            <MenuItem icon={MailOpen01Icon} onClick={() => act({ kind: "mark", action: "seen" })}>
              {tx("Mark read")}</MenuItem>
          ) : null}
          {many || !thread.flagged ? (
            <MenuItem icon={StarIcon} onClick={() => act({ kind: "mark", action: "star" })}>
              {tx("Star")}</MenuItem>
          ) : null}
          {many || thread.flagged ? (
            <MenuItem icon={StarIcon} onClick={() => act({ kind: "mark", action: "unstar" })}>
              {tx("Remove star")}</MenuItem>
          ) : null}
          {!many &&
            (thread.snooze_until ? (
              <MenuItem icon={Clock01Icon} onClick={() => act({ kind: "snooze", at: null })}>
                {tx("Bring back now")}</MenuItem>
            ) : (
              <MenuItem icon={Clock01Icon} onClick={() => setPage("snooze")}>
                {tx("Snooze…")}</MenuItem>
            ))}
          {!many && (
            <MenuItem
              icon={thread.needs_you ? Cancel01Icon : Tick02Icon}
              onClick={() => act({ kind: "needs_me", on: !thread.needs_you })}
            >
              {thread.needs_you ? tx("Doesn't need me") : tx("Needs me")}
            </MenuItem>
          )}
          <div className="menu-sep" />
          {canGoToInbox && (
            <MenuItem icon={InboxIcon} onClick={() => act({ kind: "move", to: "inbox" })}>
              {tx("Move to inbox")}</MenuItem>
          )}
          {!inArchive && (
            <MenuItem icon={Archive02Icon} onClick={() => act({ kind: "move", to: "archive" })}>
              {tx("Archive")}</MenuItem>
          )}
          <MenuItem icon={FolderExportIcon} onClick={() => setPage("move")}>
            {tx("Move to…")}</MenuItem>
          {inJunk || thread.risk > 0 ? (
            <MenuItem icon={CheckmarkCircle02Icon} onClick={() => act({ kind: "not_spam" })}>
              {tx("Not spam")}</MenuItem>
          ) : many ? (
            <MenuItem icon={MailBlock01Icon} onClick={() => act({ kind: "spam", block: false })}>
              {tx("Mark as spam")}</MenuItem>
          ) : null}
          {inBin ? (
            <MenuItem icon={Delete02Icon} danger onClick={() => act({ kind: "delete_forever" })}>
              {tx("Delete for good")}</MenuItem>
          ) : (
            <MenuItem icon={Delete02Icon} danger onClick={() => act({ kind: "move", to: "trash" })}>
              {tx("Delete")}</MenuItem>
          )}
          {!many && (
            <>
              <div className="menu-sep" />
              <MenuItem icon={MoreHorizontalIcon} onClick={() => setPage("more")}>
                {tx("More…")}</MenuItem>
            </>
          )}
        </>
      )}
    </ContextMenu>
  );
}

/** The same idea for a draft that hasn't been sent. */
export function DraftMenu({
  x,
  y,
  onOpen,
  onDiscard,
  onClose,
}: {
  x: number;
  y: number;
  onOpen: () => void;
  onDiscard: () => void;
  onClose: () => void;
}) {
  return (
    <ContextMenu x={x} y={y} label={tx("Draft actions")} onClose={onClose}>
      <MenuItem
        icon={MailOpen01Icon}
        onClick={() => {
          onClose();
          onOpen();
        }}
      >
        {tx("Open")}</MenuItem>
      <div className="menu-sep" />
      <MenuItem
        icon={Delete02Icon}
        danger
        onClick={() => {
          onClose();
          onDiscard();
        }}
      >
        {tx("Discard draft")}</MenuItem>
    </ContextMenu>
  );
}
