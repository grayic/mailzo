import { useCallback, useRef, useState } from "react";
import { ApiError, api } from "../api";
import { Cancel01Icon } from "../icons";
import type { ThreadSummary } from "../types";
import { threadKey } from "./ThreadList";
import { IconButton } from "./ui";
import { tx } from "../i18n";

/** How long a move can be taken back. Nothing has happened on the mail server until then. */
export const UNDO_MOVE_MS = 6_000;
/** How many conversations go to the mail server in one go. */
const MOVE_CHUNK = 500;

/** How many are marked read in one go. Mark all as read goes round again until none are left. */
export const MARK_ALL_MOST = 5_000;
/** A stop, should a list keep coming back however often it's marked: 300,000 conversations. */
const MARK_ALL_ROUNDS = 60;
export const refTo = ({ account_id, thread_id, folder_id }: ThreadSummary) => ({ account_id, thread_id, folder_id });

export type Movable = { account_id: number; thread_id: string; folder_id: number };

/**
 * Marks every unread conversation `unread` lists as read, a round of MARK_ALL_MOST at a time,
 * until none are left. Only one round is held at once, so a mailbox of any size is fine. Every
 * round changes this Mac first, so `changedHere` lets every window show all of it at once, and
 * the servers hear after. What was marked is kept as small references, for Undo.
 */
export async function markAllInRounds(
  unread: () => Promise<ThreadSummary[]>,
  changedHere: () => void,
  onProgress?: (done: number) => void,
  /** Filled in as it goes, so a failure partway still knows what was marked, for Undo. */
  marked: Movable[] = [],
): Promise<{ first: ThreadSummary[]; marked: Movable[]; problem: string | null }> {
  let first: ThreadSummary[] = [];
  let lastStart = "";
  const from = marked.length;
  for (let round = 0; round < MARK_ALL_ROUNDS; round++) {
    const batch = await unread();
    // Nothing left, or the same ones again, which marking didn't change: either way, done.
    if (batch.length === 0 || threadKey(batch[0]) === lastStart) break;
    lastStart = threadKey(batch[0]);
    if (round === 0) first = batch;
    await api.bulkThreads(batch.map(refTo), "seen", undefined, "here");
    for (const t of batch) marked.push(refTo(t));
    onProgress?.(marked.length);
    if (batch.length < MARK_ALL_MOST) break;
  }
  changedHere();
  return { first, marked, problem: await tellServers(marked.slice(from), "seen") };
}

/** Undo for the above, in the same size rounds, and also here first. */
export async function unmarkInRounds(marked: Movable[], changedHere: () => void): Promise<void> {
  for (let i = 0; i < marked.length; i += MARK_ALL_MOST) {
    await api.bulkThreads(marked.slice(i, i + MARK_ALL_MOST), "unseen", undefined, "here");
  }
  changedHere();
  await tellServers(marked, "unseen");
}

/** The servers hear last, a round at a time. A problem with one round doesn't stop the rest. */
// ponytail: a round's read marks are held from when this Mac changed them, for up to 5 minutes. A
// mailbox whose rounds take the servers longer than that in all can show some unread again for a
// refresh, until its round is told. Holding from when each round goes to the server would fix it.
async function tellServers(refs: Movable[], action: "seen" | "unseen"): Promise<string | null> {
  let problem: string | null = null;
  for (let i = 0; i < refs.length; i += MARK_ALL_MOST) {
    const done = await api.bulkThreads(refs.slice(i, i + MARK_ALL_MOST), action, undefined, "servers");
    problem ??= done.problem ?? null;
  }
  return problem;
}
/** `note` goes before the toast's own words, such as "OTP copied." */
type Waiting = { threads: Movable[]; to: string; note?: string };
type Gone = Map<string, { thread: Movable; to: string; by: Waiting }>;

/** Rows on their way to this folder, so they show there before the server has caught up. */
export const comingTo = (gone: Gone, folder: { id: number; account_id: number; role: string; path: string }) =>
  [...gone.values()]
    .filter((m) => m.thread.account_id === folder.account_id && (m.to === folder.role || m.to === folder.path))
    // Here is where it now counts as being, so its menu and any next move treat it that way.
    .map((m) => ({ ...m.thread, folder_id: folder.id }));

/** Where mail may be sent back to the inbox from. Spam has its own "Not spam". */
export const canGoToInbox = (role?: string) => role === "trash" || role === "archive" || role === "folder";

/**
 * Moves that look instant and can be taken back. The rows go at once, and the slow part, a trip
 * to the mail server, waits out the undo time. Only one waits at a time, so a new move sends the
 * last one on its way.
 */
// ponytail: quitting during the wait leaves the mail where it was, with nothing lost. If that ever
// matters, the wait could be saved by the engine and picked up at the next launch.
export function useWaitingMove(finished: (threads: Movable[], problem: string | null, to: string) => void) {
  const [waiting, setWaiting] = useState<Waiting | null>(null);
  const current = useRef<Waiting | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Everything that has left the screen but not yet the server, for lists to leave out. */
  const gone = useRef<Gone>(new Map());
  const lastMove = useRef<Promise<void>>(Promise.resolve());
  const finishedRef = useRef(finished);
  finishedRef.current = finished;

  const settle = (next: Waiting | null) => {
    if (timer.current) clearTimeout(timer.current);
    current.current = next;
    setWaiting(next);
  };

  const moveNow = useCallback(async () => {
    const move = current.current;
    if (!move) return;
    settle(null);
    // One at a time and in order, as a later move goes by where the one before put the mail.
    const before = lastMove.current;
    let finish = () => {};
    lastMove.current = new Promise((done) => (finish = done));
    await before;
    let problem: string | null = null;
    const refs = move.threads.map((t) => ({
      account_id: t.account_id,
      thread_id: t.thread_id,
      folder_id: t.folder_id,
    }));
    // A few hundred at a time, so "Archive all" on a big tab never holds everything else up.
    for (let at = 0; at < refs.length; at += MOVE_CHUNK) {
      try {
        const reply = await api.bulkThreads(refs.slice(at, at + MOVE_CHUNK), "move", move.to);
        problem = problem ?? reply.problem ?? null;
      } catch (e) {
        problem = problem ?? (e instanceof ApiError ? e.message : String(e));
      }
    }
    // A newer move of the same row may have it on its way already, and it stays gone for that.
    move.threads.forEach((t) => {
      if (gone.current.get(threadKey(t))?.by === move) gone.current.delete(threadKey(t));
    });
    finishedRef.current(move.threads, problem, move.to);
    finish();
  }, []);

  const move = (threads: Movable[], to: string, note?: string) => {
    void moveNow();
    const next = { threads, to, note };
    threads.forEach((t) => gone.current.set(threadKey(t), { thread: t, to, by: next }));
    settle(next);
    timer.current = setTimeout(() => void moveNow(), UNDO_MOVE_MS);
  };

  const undo = () => {
    current.current?.threads.forEach((t) => gone.current.delete(threadKey(t)));
    settle(null);
  };

  return { waiting, gone, move, moveNow, undo };
}

export function MoveToast({
  move,
  onUndo,
  onDismiss,
}: {
  move: Waiting;
  onUndo: () => void;
  onDismiss: () => void;
}) {
  const count = move.threads.length;
  const what = count === 1 ? "Conversation" : `${count} conversations`;
  const where =
    move.to === "archive"
      ? "archived"
      : move.to === "trash"
        ? "moved to Trash"
        : move.to === "inbox"
          ? "moved to Inbox"
          : "moved";
  return (
    <div role="status" className="toast toast-undo">
      <span className="grow tnum">
        {move.note ? `${move.note} ` : ""}
        {what} {where}.
      </span>
      <button className="btn btn-secondary btn-sm" onClick={onUndo}>
        {tx("Undo")}</button>
      <IconButton icon={Cancel01Icon} label={tx("Dismiss")} small onClick={onDismiss} />
    </div>
  );
}

export function AllReadToast({ count, onUndo, onDismiss }: { count: number; onUndo: () => void; onDismiss: () => void }) {
  return (
    <div role="status" className="toast toast-undo">
      <span className="grow tnum">
        {count === 1 ? tx("Conversation marked as read.") : tx("{n} conversations marked as read.", { n: count })}
      </span>
      <button className="btn btn-secondary btn-sm" onClick={onUndo}>
        {tx("Undo")}
      </button>
      <IconButton icon={Cancel01Icon} label={tx("Dismiss")} small onClick={onDismiss} />
    </div>
  );
}
