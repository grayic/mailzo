import { useLayoutEffect, useRef, useState } from "react";
import {
  Archive02Icon,
  CheckmarkCircle02Icon,
  MailBlock01Icon,
  Delete02Icon,
  FolderExportIcon,
  Folder01Icon,
  InboxIcon,
  Mail01Icon,
  MailOpen01Icon,
  StarIcon,
} from "../icons";
import type { Folder } from "../types";
import { FOLDER_ICONS } from "./Sidebar";
import { Confirm, Icon, Menu, MenuItem } from "./ui";
import { tx } from "../i18n";

/** Where something can be moved. Folders differ between inboxes, so mail from
 * several gets only the ones they all have. */
export function MoveToItems({
  folders,
  onMove,
}: {
  folders: Folder[] | null;
  onMove: (to: string) => void;
}) {
  if (folders) {
    return (
      <>
        {folders.map((f) => (
          <MenuItem key={f.id} icon={FOLDER_ICONS[f.role] ?? Folder01Icon} onClick={() => onMove(f.path)}>
            {f.name}
          </MenuItem>
        ))}
      </>
    );
  }
  return (
    <>
      {[
        ["inbox", tx("Inbox")],
        ["archive", tx("Archive")],
        ["junk", tx("Spam")],
        ["trash", tx("Trash")],
      ].map(([role, name]) => (
        <MenuItem key={role} icon={FOLDER_ICONS[role]} onClick={() => onMove(role)}>
          {name}
        </MenuItem>
      ))}
    </>
  );
}

/** Shown instead of a conversation while several are selected. */
export function BulkPanel({
  count,
  sameInboxFolders,
  inBin,
  inJunk,
  canGoToInbox,
  onSpam,
  onAction,
  onMove,
  onClear,
  onDone,
}: {
  count: number;
  /** The folders to offer, when everything selected is in one inbox. */
  sameInboxFolders: Folder[] | null;
  /** Everything selected is in the bin or spam. */
  inBin: boolean;
  /** Everything selected is in Spam. */
  inJunk: boolean;
  canGoToInbox: boolean;
  onSpam: (spam: boolean) => void;
  onAction: (action: "seen" | "unseen" | "star" | "unstar" | "delete_forever") => void;
  onMove: (to: string) => void;
  onClear: () => void;
  /** Takes them out of Needs you, when some are in it. */
  onDone?: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  // In the full-width layout it lies over the list's header and tabs, and covers all of
  // them, however tall they are, so none of it peeks out underneath.
  useLayoutEffect(() => {
    const list = document.querySelector<HTMLElement>('.app[data-layout="full"] .list .list-scroll');
    const el = panel.current;
    if (!list || !el) return;
    const cover = () => {
      el.style.minHeight = `${Math.max(0, list.getBoundingClientRect().top - el.getBoundingClientRect().top)}px`;
    };
    cover();
    window.addEventListener("resize", cover);
    return () => window.removeEventListener("resize", cover);
  }, [count]);
  return (
    <div ref={panel} className="bulk">
      <div className="bulk-count tnum">{count}</div>
      <div className="empty-title">
        {count === 1 ? tx("conversation selected") : tx("conversations selected")}
      </div>
      <div className="bulk-actions" role="toolbar" aria-label={tx("Selected conversations actions")}>
        {onDone && (
          <button className="btn btn-secondary" onClick={onDone}>
            <Icon icon={CheckmarkCircle02Icon} size="s" />
            {tx("Done")}</button>
        )}
        {canGoToInbox && (
          <button className="btn btn-secondary" onClick={() => onMove("inbox")}>
            <Icon icon={InboxIcon} size="s" />
            {tx("Move to inbox")}</button>
        )}
        <button className="btn btn-secondary" onClick={() => onMove("archive")}>
          <Icon icon={Archive02Icon} size="s" />
          {tx("Archive")}</button>
        {inBin ? (
          <button className="btn btn-secondary" onClick={() => setConfirming(true)}>
            <Icon icon={Delete02Icon} size="s" />
            {tx("Delete for good")}</button>
        ) : (
          <button className="btn btn-secondary" onClick={() => onMove("trash")}>
            <Icon icon={Delete02Icon} size="s" />
            {tx("Delete")}</button>
        )}
        <button className="btn btn-secondary" onClick={() => onSpam(!inJunk)}>
          <Icon icon={inJunk ? CheckmarkCircle02Icon : MailBlock01Icon} size="s" />
          {inJunk ? tx("Not spam") : "Spam"}
        </button>
        <button className="btn btn-secondary" onClick={() => onAction("seen")}>
          <Icon icon={MailOpen01Icon} size="s" />
          {tx("Mark read")}</button>
        <button className="btn btn-secondary" onClick={() => onAction("unseen")}>
          <Icon icon={Mail01Icon} size="s" />
          {tx("Mark unread")}</button>
        <button className="btn btn-secondary" onClick={() => onAction("star")}>
          <Icon icon={StarIcon} size="s" />
          {tx("Star")}</button>
        <Menu
          label={tx("Move to folder")}
          trigger={(props) => (
            <button {...props} type="button" className="btn btn-secondary">
              <Icon icon={FolderExportIcon} size="s" />
              {tx("Move to")}</button>
          )}
        >
          {(close) => (
            <MoveToItems
              folders={sameInboxFolders}
              onMove={(to) => {
                close();
                onMove(to);
              }}
            />
          )}
        </Menu>
      </div>
      <button className="link-btn bulk-clear" onClick={onClear}>
        {tx("Clear selection")}</button>
      <div className="fine bulk-hint">
        {tx("⌘A selects all, and esc clears.")}
      </div>
      {confirming && (
        <Confirm
          title={count === 1 ? tx("Delete 1 conversation for good?") : tx("Delete {n} conversations for good?", { n: count })}
          confirm={tx("Yes, delete for good")}
          keep={tx("Keep")}
          danger
          onKeep={() => setConfirming(false)}
          onConfirm={() => {
            setConfirming(false);
            onAction("delete_forever");
          }}
        >
          {tx("They'll be removed from this folder for good. This can't be undone.")}</Confirm>
      )}
    </div>
  );
}
