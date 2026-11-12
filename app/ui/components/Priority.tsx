import { useEffect, useState } from "react";
import { ApiError, api, type BriefCounts } from "../api";
import { AiMagicIcon, Archive02Icon, Cancel01Icon, Clock01Icon, MailRemove01Icon, MailReply01Icon, SpamIcon } from "../icons";
import { useSettings } from "../settings";
import { myLanguage } from "../format";
import type { Message, ReplyStart } from "../types";
import { Confirm, Icon, Spinner } from "./ui";
import { language, tx } from "../i18n";
import { reloadPaid, usePaid } from "../paid";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
/** Brief me waits, hidden, until Mailzo Cloud's AI is live on the server. */
const BRIEF_ME_LIVE = false;
const HIDDEN_FOR = "brief-hidden-for";
const today = () => new Date().toDateString();

function hiddenToday(): boolean {
  try {
    return localStorage.getItem(HIDDEN_FOR) === today();
  } catch {
    return false;
  }
}

/** Today at a glance, above what needs the person. Each part goes where it's about. */
export function Brief({
  refreshKey,
  onDeadlines,
  onNoReply,
  onSpam,
}: {
  refreshKey: number;
  onDeadlines: () => void;
  onNoReply: () => void;
  onSpam: () => void;
}) {
  const [counts, setCounts] = useState<BriefCounts | null>(null);
  const [hidden, setHidden] = useState(hiddenToday);
  const { settings } = useSettings();
  // The brief comes with Mailzo Cloud, and needs no AI key of the person's own.
  const paid = usePaid();
  const [aiText, setAiText] = useState<string | null>(null);
  const [aiBusy, setAiBusy] = useState(false);
  const [aiProblem, setAiProblem] = useState<string | null>(null);
  const aiWanted = BRIEF_ME_LIVE && settings.ai_summaries === "on";
  const aiOn = paid && aiWanted;

  useEffect(() => {
    if (hidden) return;
    api
      .brief()
      .then(setCounts)
      .catch(() => {});
    void reloadPaid();
    // Only one already made today for this same mail: opening the app never asks the AI.
    if (!BRIEF_ME_LIVE) return;
    api
      .aiBrief(true, myLanguage())
      .then((b) => setAiText(b?.text ?? null))
      .catch(() => {});
  }, [hidden, refreshKey]);

  const briefMe = async () => {
    setAiBusy(true);
    setAiProblem(null);
    try {
      setAiText((await api.aiBrief(false, myLanguage()))?.text ?? null);
    } catch (e) {
      setAiProblem(say(e));
    } finally {
      setAiBusy(false);
    }
  };

  if (hidden || !counts) return null;
  const worthShowing = counts.deadlines + counts.no_reply + counts.maybe_real + counts.sorted > 0 || aiWanted;
  if (!worthShowing) return null;
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  return (
    <div className="brief" role="status">
      {aiText && (
        <div className="brief-ai selectable">
          <Icon icon={AiMagicIcon} size="xs" />
          <span>{aiText}</span>
        </div>
      )}
      {aiOn && !aiText && (
        <button
          type="button"
          className="brief-chip"
          data-tip={tx("A few sentences on what needs you today")}
          disabled={aiBusy}
          onClick={() => void briefMe()}
        >
          {aiBusy ? <Spinner size="xs" /> : <Icon icon={AiMagicIcon} size="xs" />}
          {tx("Brief me")}</button>
      )}
      {aiWanted && !paid && !aiText && (
        <>
          <button type="button" className="brief-chip" disabled>
            <Icon icon={AiMagicIcon} size="xs" />
            {tx("Brief me")}</button>
          <span className="muted brief-note">{tx("Included with Mailzo Cloud")}</span>
        </>
      )}
      {aiProblem && <span className="muted brief-note">{aiProblem}</span>}
      {counts.deadlines > 0 && (
        <button
          type="button"
          className="brief-chip"
          data-tip={tx("Emails that ask for something by a date")}
          onClick={onDeadlines}
        >
          <Icon icon={Clock01Icon} size="xs" />
          {plural(counts.deadlines, "deadline coming up", "deadlines coming up")}
        </button>
      )}
      {counts.no_reply > 0 && (
        <button
          type="button"
          className="brief-chip"
          data-tip={tx("Emails you sent that are still waiting for an answer")}
          onClick={onNoReply}
        >
          <Icon icon={MailReply01Icon} size="xs" />
          {counts.no_reply === 1 ? tx("1 person hasn't answered you yet") : tx("{0} people haven't answered you yet", { "0": counts.no_reply })}
        </button>
      )}
      {counts.maybe_real > 0 && (
        <button type="button" className="brief-chip" data-tip={tx("Have a look, in case it isn't spam")} onClick={onSpam}>
          <Icon icon={SpamIcon} size="xs" />
          {plural(counts.maybe_real, "email in Spam may be real", "emails in Spam may be real")}
        </button>
      )}
      {counts.sorted > 0 && (
        <span className="muted brief-note">
          {counts.sorted === 1
            ? tx("1 less important email is in the other tabs")
            : tx("{0} less important emails are in the other tabs", { "0": counts.sorted })}
        </span>
      )}
      <span className="grow" />
      <button
        type="button"
        className="brief-close"
        aria-label={tx("Hide for today")}
        data-tip={tx("Hide for today")}
        onClick={() => {
          try {
            localStorage.setItem(HIDDEN_FOR, today());
          } catch {
            // It just comes back next time.
          }
          setHidden(true);
        }}
      >
        <Icon icon={Cancel01Icon} size="s" />
      </button>
    </div>
  );
}

/** Above the mail that doesn't need much attention, to get rid of it in one go. */
export function RestBar({
  count,
  onCleanUp,
  onCountAll,
  onArchiveAll,
}: {
  /** How many have loaded. Only whether there are any matters, as more may not have loaded yet. */
  count: number;
  onCleanUp: () => void;
  /** Finds every email in the tab, loaded or not, and says how many. */
  onCountAll: () => Promise<number>;
  onArchiveAll: () => void;
}) {
  const [counting, setCounting] = useState(false);
  const [asking, setAsking] = useState<number | null>(null);
  const ask = async () => {
    setCounting(true);
    try {
      const all = await onCountAll();
      if (all > 0) setAsking(all);
    } finally {
      setCounting(false);
    }
  };
  return (
    <div className="brief rest-bar">
      <span className="muted truncate-1 grow">{tx("Newsletters, offers and updates. Nothing here needs you.")}</span>
      <button
        type="button"
        className="brief-chip"
        data-tip={tx("See who sends you the most here, then unsubscribe or archive their emails")}
        onClick={onCleanUp}
      >
        <Icon icon={MailRemove01Icon} size="xs" />
        {tx("Unsubscribe from senders")}</button>
      {count > 0 && (
        <button
          type="button"
          className="brief-chip"
          data-tip={tx("Moves every email in this tab to Archive. You can still find them there.")}
          disabled={counting}
          onClick={() => void ask()}
        >
          {counting ? <Spinner size="xs" /> : <Icon icon={Archive02Icon} size="xs" />}
          {tx("Archive all")}</button>
      )}
      {asking !== null && (
        <Confirm
          title={asking === 1 ? tx("Archive 1 email?") : tx("Archive all {0} emails?", { "0": asking.toLocaleString(language) })}
          confirm={tx("Yes, archive")}
          keep={tx("Keep")}
          onKeep={() => setAsking(null)}
          onConfirm={() => {
            setAsking(null);
            onArchiveAll();
          }}
        >
          {tx("They leave your inbox and stay in Archive, where you can still find them.")}</Confirm>
      )}
    </div>
  );
}

export const QUICK_ANSWERS: [string, string][] = [
  [tx("Yes"), tx("Say yes, politely.")],
  [tx("No"), tx("Say no, politely.")],
  [tx("Need more time"), tx("Say I need a bit more time and will get back to them.")],
];

/** Replies written once per email, so reopening it doesn't ask again. */
const written = new Map<number, string>();
/** Short replies suggested once per email, for the same reason. */
const suggested = new Map<number, string[]>();

/** Under an email waiting on this person: one-tap answers, and a reply if they've asked for those. */
export function ReplyHelp({ message, onUse }: { message: Message; onUse: (start: ReplyStart) => void }) {
  const { settings } = useSettings();
  const automatic = settings.ai_draft_ahead === "on" && settings.ai_replies === "on";
  const [ready, setReady] = useState<string | null>(written.get(message.id) ?? null);
  const [dismissed, setDismissed] = useState(false);
  const [short, setShort] = useState<string[] | null>(suggested.get(message.id) ?? null);
  // Where AI may not read, there's nothing coming, so nothing is shown.
  const [blocked, setBlocked] = useState(false);

  useEffect(() => {
    if (!automatic || written.has(message.id)) return;
    let stale = false;
    api
      .aiWriteReply(message.id, "")
      .then((r) => {
        written.set(message.id, r.reply);
        if (!stale) setReady(r.reply);
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [automatic, message.id]);

  // Short replies written for this email. None while they're written; if none can be,
  // there's still a way to have AI write the whole reply.
  useEffect(() => {
    if (settings.ai_replies !== "on" || suggested.has(message.id)) return;
    let stale = false;
    api
      .aiShortReplies(message.id)
      .then((r) => {
        suggested.set(message.id, r.replies);
        if (!stale) setShort(r.replies);
      })
      .catch((e) => {
        // Where AI may not read, a button to have it write would only fail.
        if (e instanceof ApiError && (e.code === "ai_blocked" || e.code === "ai_off" || e.code === "ai_key")) {
          if (!stale) setBlocked(true);
          return;
        }
        suggested.set(message.id, []);
        if (!stale) setShort([]);
      });
    return () => {
      stale = true;
    };
  }, [settings.ai_replies, message.id]);

  if (settings.ai_replies !== "on") return null;
  return (
    <div className="reply-help">
      {ready && !dismissed && (
        <div className="reply-ready">
          <div className="section-title">{tx("Reply ready")}</div>
          <div className="reply-ready-text selectable">{ready}</div>
          <div className="hstack-m">
            <button type="button" className="btn btn-primary btn-sm" onClick={() => onUse({ text: ready })}>
              {tx("Use this")}</button>
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => setDismissed(true)}>
              {tx("Not this")}</button>
          </div>
        </div>
      )}
      {/* Said at once, so it's clear replies are on their way. */}
      {!short && !blocked && (
        <div className="quick-answers" role="status">
          <span className="quick-answers-label">
            <Icon icon={AiMagicIcon} size="xs" />
            {tx("Quick reply")}</span>
          <span className="quick-answers-waiting">
            <Spinner size="xs" />
            <span>
              {tx("Writing quick replies")}
              <span className="typing-dots" aria-hidden="true">
                <span>.</span>
                <span>.</span>
                <span>.</span>
              </span>
            </span>
          </span>
        </div>
      )}
      {/* Only replies written for this email: a bare "Yes" doesn't say to what. */}
      {short && (
        <div className="quick-answers">
          <span className="quick-answers-label">
            <Icon icon={AiMagicIcon} size="xs" />
            {tx("Quick reply")}</span>
          {short.length === 0 && (
            <button
              type="button"
              className="quick-answer"
              data-tip={tx("AI writes a reply to this email for you to check before sending")}
              onClick={() => onUse({ note: "" })}
            >
              <Icon icon={AiMagicIcon} size="xs" />
              {tx("Write a reply")}
            </button>
          )}
          {short.map((text) => (
            <button
              key={text}
              type="button"
              className="quick-answer quick-answer-text"
              data-tip={tx("AI writes a full reply that says this, for you to check before sending")}
              onClick={() => onUse({ note: text })}
            >
              <span className="truncate-1">{text}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
