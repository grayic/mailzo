import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ApiError, api, type AskFact, type AskSource } from "../api";
import { myLanguage, shortDate } from "../format";
import {
  AiChat02Icon,
  AiMagicIcon,
  ArrowDown01Icon,
  ArrowReloadHorizontalIcon,
  ArrowRight01Icon,
  ArrowUp02Icon,
  BubbleChatAddIcon,
  Calendar03Icon,
  Copy01Icon,
  Menu01Icon,
  MailAdd01Icon,
  Search01Icon,
  Cancel01Icon,
  InformationCircleIcon,
  Mail01Icon,
  Settings02Icon,
} from "../icons";
import type { Account } from "../types";
import { Choice } from "./settings/controls";
import { Avatar, Icon, IconButton, Spinner } from "./ui";
import { language, tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
/** Citations after a point: a few numbers, then how many more. */
const MOST_CITES = 3;

type Point = { text: string; ids: number[] };
type Turn =
  | { id: number; role: "user"; text: string }
  | {
      id: number;
      role: "assistant";
      /** What it's doing: looking for emails, reading the ones found, or done. */
      state: "looking" | "reading" | "done" | "failed";
      question: string;
      found: AskSource[];
      text: string;
      points: Point[];
      /** The few emails worth opening, shown in full. */
      key: AskSource[];
      /** Emails without the question's words, found as being about the same thing. */
      related?: AskSource[];
      sources: AskSource[];
      suggestions: string[];
      /** What to know about the answer: details left out, or only the closest emails read. */
      notes?: string[];
      /** What was made of the question, for a follow-up to start from. */
      plan?: unknown;
      problem?: string;
    };

/** The chat stays while they look at an email and come back, until they start a new one. */
let kept: { turns: Turn[]; scope: string; about: string } = { turns: [], scope: "all", about: "" };

/** Said when asked how Ask AI works, which isn't explained. */
const ABOUT_ITSELF = "I help with what's in your email, like who said what, dates, amounts and files.";

/** The email it was asked from: everything with the people in its conversation. */
export type AskAbout = { account_id: number; thread_id: string; label: string };
let nextId = 1;

const STARTERS: { icon: typeof AiMagicIcon; text: string }[] = [
  { icon: MailAdd01Icon, text: tx("What needs my reply this week?") },
  { icon: Search01Icon, text: tx("Find invoices and receipts from last month") },
  { icon: Calendar03Icon, text: tx("Any bookings or meetings coming up?") },
  { icon: AiMagicIcon, text: tx("Summarise what happened in my inbox today") },
];

/** Chat with your inbox: every question finds the emails it's about and answers from them. */
export function AskChat({
  accounts,
  onOpen,
  onOpenAiSettings,
  onPhoneNav,
  about = null,
  onClearAbout,
  search = null,
}: {
  accounts: Account[];
  onOpen: (s: AskSource) => void;
  onOpenAiSettings: () => void;
  onPhoneNav?: () => void;
  about?: AskAbout | null;
  onClearAbout?: () => void;
  /** Asked beside search results: a question of its own, not about the search, starting in the
   * inbox searched (null for all of them). */
  search?: { account: number | null } | null;
}) {
  // Asking from another email starts a new chat about it.
  const aboutKey = about ? `${about.account_id}:${about.thread_id}` : search ? `search:${search.account}` : "";
  if (kept.about !== aboutKey) kept = { ...kept, about: aboutKey, turns: [] };
  const [turns, setTurnsState] = useState<Turn[]>(kept.turns);
  const [scope, setScopeState] = useState(
    search ? (search.account === null ? "all" : String(search.account)) : kept.scope,
  );
  const [draft, setDraft] = useState("");
  const scroller = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLTextAreaElement>(null);
  const busy = turns.some((t) => t.role === "assistant" && (t.state === "looking" || t.state === "reading"));

  const setTurns = (change: (list: Turn[]) => Turn[]) =>
    setTurnsState((list) => {
      const next = change(list);
      kept = { ...kept, turns: next };
      return next;
    });
  const setScope = (value: string) => {
    // Beside search results it starts from the search's inbox, so the Ask page keeps its own.
    if (!search) kept = { ...kept, scope: value };
    setScopeState(value);
  };
  const update = (id: number, change: Partial<Extract<Turn, { role: "assistant" }>>) =>
    setTurns((list) => list.map((t) => (t.id === id && t.role === "assistant" ? { ...t, ...change } : t)));

  // The newest message stays in view.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [turns]);

  // Beside search results it waits to be clicked, so typing in the search box stays there.
  useEffect(() => {
    if (!search) box.current?.focus();
  }, []);

  // The box grows with what's written, up to a few lines.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
    el.style.overflowY = el.scrollHeight > 160 ? "auto" : "hidden";
  }, [draft]);

  const ask = async (question: string, history: Turn[], replyId?: number) => {
    const id = replyId ?? nextId++;
    const said = history
      .map((t) => (t.role === "user" ? { role: "user" as const, text: t.text } : t.state === "done" ? { role: "assistant" as const, text: t.text } : null))
      .filter((t): t is { role: "user" | "assistant"; text: string } => t !== null);
    if (replyId === undefined) {
      setTurns((list) => [
        ...list,
        { id: nextId++, role: "user", text: question },
        { id, role: "assistant", state: "looking", question, found: [], text: "", points: [], key: [], sources: [], suggestions: [] },
      ]);
    } else {
      update(id, { state: "looking", problem: undefined, found: [] });
    }
    try {
      // A follow-up ("and last year?") builds on what this chat has asked so far.
      const before = said.filter((t) => t.role === "user").map((t) => t.text);
      const last = [...history].reverse().find((t) => t.role === "assistant" && t.plan !== undefined);
      const looked = await api.aiAskSources(
        question,
        null,
        scope === "all" ? null : Number(scope),
        true,
        before,
        about ? { account_id: about.account_id, thread_id: about.thread_id } : null,
        last?.role === "assistant" ? last.plan : null,
      );
      update(id, { plan: looked.plan });
      const found = looked.sources;
      // How it works isn't said.
      if (looked.kind === "about_itself") {
        update(id, { state: "done", text: tx(ABOUT_ITSELF), points: [], key: [], sources: [], suggestions: [] });
        return;
      }
      // A fact the email's own markup states needs no AI either.
      if (looked.kind === "fact" && looked.fact) {
        update(id, {
          state: "done",
          text: factText(looked.fact),
          points: [],
          key: found,
          sources: [],
          suggestions: [],
        });
        return;
      }
      // Showing emails needs no AI: they're the answer. Only exact matches count; the
      // rest are shown apart as maybe related.
      const exact = found.filter((s) => !s.related);
      const total = Math.max(looked.total ?? 0, exact.length);
      const others = (looked.others ?? []).map((o) =>
        tx("Someone else is called {name} too: {other}. Write their full name to see theirs.", o),
      );
      // "How many" with nothing matching needs no AI either.
      const counted = looked.kind === "count";
      if (looked.kind === "list" || (counted && exact.length === 0)) {
        update(id, {
          state: "done",
          text:
            exact.length === 0
              ? tx("No emails match.")
              : total > exact.length
                ? tx("The newest {n} of {total} emails", { n: exact.length, total })
                : exact.length === 1
                  ? tx("1 email")
                  : tx("{n} emails", { n: exact.length }),
          notes: others,
          points: [],
          key: exact,
          related: found.filter((s) => s.related),
          sources: [],
          suggestions: [],
        });
        return;
      }
      update(id, { state: "reading", found });
      const related = found.filter((s) => s.related).map((s) => s.id);
      const reply = await api.aiChat(question, said, found.map((s) => s.id), myLanguage(), related, {
        words: looked.words,
        kind: looked.kind,
        total,
      });
      const notes: string[] = [...others];
      if (reply.sources.length > 0 && reply.sources.every((s) => related.includes(s.id)))
        notes.push(tx("No email has these exact words, so this comes from related emails."));
      if ((reply.removed ?? []).length > 0) notes.push(tx("Some details weren't in the emails, so they were left out."));
      update(id, {
        state: "done",
        text: reply.about_itself
          ? tx(ABOUT_ITSELF)
          : reply.not_found && !reply.answer
            ? (reply.removed ?? []).length > 0 && found.length > 0
              ? tx("I found emails about this, but couldn't check the answer against them. They're below.")
              : tx("I couldn't find that in your mail.")
            : reply.answer || (counted ? (total === 1 ? tx("1 email") : tx("{n} emails", { n: total })) : ""),
        notes,
        points: reply.points,
        // What was counted is shown, as a list is.
        key: counted ? exact : (reply.key ?? []),
        related: counted ? found.filter((s) => s.related) : undefined,
        sources: counted ? [] : reply.sources.length > 0 ? reply.sources : found.slice(0, 3),
        suggestions: reply.suggestions,
      });
    } catch (e) {
      update(id, { state: "failed", problem: say(e) });
    }
  };

  const send = (text = draft) => {
    const question = text.trim();
    if (!question || busy) return;
    setDraft("");
    void ask(question, turns);
  };

  const scopeOptions = [
    { value: "all", label: tx("All inboxes") },
    ...accounts.map((a) => ({ value: String(a.id), label: a.prefs.label || a.email })),
  ];

  // Which AI answers, from those this Mac has: Apple Intelligence, a plan signed in, or a key.
  const [ais, setAis] = useState<{ value: string; label: string }[]>([]);
  const [ai, setAi] = useState("");
  const [switching, setSwitching] = useState(false);
  // Picking Apple Intelligence says what to expect of it, so nobody expects too much.
  const [appleNote, setAppleNote] = useState(false);
  const loadAis = async () => {
    try {
      const [status, plans] = await Promise.all([api.aiStatus(), api.aiPlans()]);
      const options: { value: string; label: string }[] = [];
      if (status.on_device === "ready") options.push({ value: "apple", label: tx("Apple Intelligence") });
      // The AI in Settings › AI, which everything else uses. Picking here changes Ask AI alone.
      const plan = plans.find((p) => p.id === status.via);
      const service = status.services.find((s) => s.id === status.service)?.name ?? "AI";
      const main = plan ? plan.name : status.ending ? tx("{0} API key", { "0": service }) : null;
      if (main) options.push({ value: "main", label: main });
      setAis(options);
      setAi(status.ask_on_device || !main ? "apple" : "main");
    } catch {
      setAis([]);
    }
  };
  useEffect(() => {
    void loadAis();
  }, []);
  const switchAi = async (to: string) => {
    setSwitching(true);
    try {
      await api.setSetting("ask_ai_apple", to === "apple" ? "on" : "off");
    } catch {
      // What's in use stays as it was, and shows so.
    } finally {
      await loadAis();
      setSwitching(false);
      setAppleNote(to === "apple");
    }
  };

  return (
    <main className="chat">
      <header className="chat-head" data-tauri-drag-region="deep">
        {onPhoneNav && <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />}
        <h2 className="chat-title">{tx("Ask AI")}</h2>
        <span className="pill pill-new chat-beta">{tx("Beta")}</span>
        {accounts.length > 1 && <Choice label={tx("Look in")} value={scope} options={scopeOptions} onChange={setScope} />}
        <span className="spacer" />
        {ais.length > 0 && (
          <span className="hstack">
            {switching && <Spinner size="xs" />}
            <Choice label={tx("AI")} value={ai} options={ais} onChange={(to) => void switchAi(to)} />
          </span>
        )}
        <IconButton icon={Settings02Icon} label={tx("AI settings")} small onClick={onOpenAiSettings} />
        {turns.length > 0 && (
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={busy}
            onClick={() => {
              setTurns(() => []);
              box.current?.focus();
            }}
          >
            <Icon icon={BubbleChatAddIcon} size="s" />
            {tx("New chat")}
          </button>
        )}
      </header>

      <div className="chat-scroll" ref={scroller}>
        <div className="chat-thread">
          {turns.length === 0 ? (
            <div className="chat-start">
              <div className="chat-start-icon">
                <Icon icon={AiChat02Icon} size="l" />
              </div>
              <h3 className="chat-start-title">
                {search ? tx("Didn't find what you're looking for?") : tx("Ask anything about your email")}
              </h3>
              <p className="chat-start-text">
                {search
                  ? tx("Ask AI to look through all your mail.")
                  : tx("Mailzo finds the emails and answers from them, with a link to each one.")}
              </p>
              {!search && (
                <div className="chat-starters">
                  {STARTERS.map((s) => (
                    <button key={s.text} type="button" className="chat-starter" onClick={() => send(s.text)}>
                      <Icon icon={s.icon} size="s" />
                      <span>{s.text}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          ) : (
            turns.map((t, i) =>
              t.role === "user" ? (
                <div key={t.id} className="chat-turn chat-turn-me">
                  <div className="chat-bubble selectable">{t.text}</div>
                </div>
              ) : (
                <AnswerTurn
                  key={t.id}
                  turn={t}
                  onOpen={onOpen}
                  onRetry={() => void ask(t.question, turns.slice(0, i - 1), t.id)}
                  onOpenAiSettings={onOpenAiSettings}
                />
              ),
            )
          )}
        </div>
      </div>

      <form
        className="chat-compose"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        {/* Who it's about sits on the box itself, where the question is written. */}
        {about && (
          <div className="chat-about">
            <Icon icon={Mail01Icon} size="s" />
            <span className="grow truncate-1">{tx("About {name}", { name: about.label })}</span>
            {onClearAbout && (
              <IconButton icon={Cancel01Icon} label={tx("Ask about everything")} small onClick={onClearAbout} />
            )}
          </div>
        )}
        {appleNote && ai === "apple" && (
          <div className="chat-note" role="status">
            <Icon icon={InformationCircleIcon} size="s" />
            <div className="grow">
              <strong>{tx("Apple Intelligence is a small AI on your Mac.")}</strong>
              <div>{tx("It remembers less of a long chat and can miss details. For big questions, use Claude or ChatGPT.")}</div>
            </div>
            <IconButton icon={Cancel01Icon} label={tx("Close")} small onClick={() => setAppleNote(false)} />
          </div>
        )}
        <div className="chat-box">
          <textarea
            ref={box}
            rows={1}
            className="chat-input"
            aria-label={tx("Ask about your email")}
            placeholder={turns.length ? tx("Ask a follow-up…") : tx("Ask about your email…")}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
          />
          <button type="submit" className="chat-send" aria-label={tx("Send")} disabled={!draft.trim() || busy}>
            <Icon icon={ArrowUp02Icon} size="s" />
          </button>
        </div>
        <p className="chat-foot">{tx("Answers come from your saved email. Check anything important in the email itself.")}</p>
      </form>
    </main>
  );
}

/** A day and time where the person is: "Wed 4 Mar, 09:40". */
/** A time as the email writes it, at that place: a flight leaves at the airport's own time, and
 * a date alone stays a date. */
const when = (written?: string | number | null) => {
  if (written == null || written === "") return "";
  if (typeof written === "number")
    return new Date(written * 1000).toLocaleString(language, {
      weekday: "short",
      day: "numeric",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
    });
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2}))?/.exec(written);
  if (!m) return written;
  const [, y, mo, d, h, mi] = m;
  return new Date(Date.UTC(+y, +mo - 1, +d, h ? +h : 0, mi ? +mi : 0)).toLocaleString(language, {
    weekday: "short",
    day: "numeric",
    month: "short",
    ...(h ? { hour: "2-digit", minute: "2-digit" } : {}),
    timeZone: "UTC",
  });
};

/** One line from a fact an email states: a flight, a stay, an order, a bill or a parcel. */
function factText(f: AskFact): string {
  const booking = f.booking ? ` ${tx("Booking {code}.", { code: f.booking })}` : "";
  switch (f.kind) {
    case "flight":
      return tx("Your flight {flight} to {to} leaves {when} from {from}.", {
        flight: f.flight ?? "",
        to: f.to ?? f.place,
        when: when(f.departs),
        from: f.from ?? "",
      }) + booking;
    case "lodging":
      return tx("You check in at {name} on {when}.", { name: f.name ?? f.place, when: when(f.checkin) }) + booking;
    case "invoice":
      return tx("{provider}: {amount}, due {when}.", {
        provider: f.provider ?? f.place,
        amount: f.amount ?? "",
        when: when(f.due),
      });
    case "parcel":
      return tx("Your {carrier} parcel is expected {when}.", { carrier: f.carrier ?? f.place, when: when(f.expected) });
    default:
      return tx("Order {number} from {seller}: {amount}.", {
        number: f.number ?? "",
        seller: f.seller ?? f.place,
        amount: f.amount ?? "",
      });
  }
}

function AnswerTurn({
  turn,
  onOpen,
  onRetry,
  onOpenAiSettings,
}: {
  turn: Extract<Turn, { role: "assistant" }>;
  onOpen: (s: AskSource) => void;
  onRetry: () => void;
  onOpenAiSettings: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const number = (id: number) => turn.sources.findIndex((s) => s.id === id) + 1;
  const needsAi = turn.problem?.includes("Settings › AI");
  return (
    <div className="chat-turn chat-turn-ai">
      <span className="chat-avatar" aria-hidden="true">
        <Icon icon={AiMagicIcon} size="s" />
      </span>
      <div className="chat-ai">
        {(turn.state === "looking" || turn.state === "reading") && (
          <div className="chat-working" role="status">
            <span className="chat-dots" aria-hidden="true">
              <span />
              <span />
              <span />
            </span>
            {turn.state === "looking"
              ? tx("Looking through your email…")
              : turn.found.length === 1
                ? tx("Reading 1 email…")
                : tx("Reading {n} emails…", { n: turn.found.length })}
          </div>
        )}

        {turn.state === "failed" && (
          <div className="chat-failed" role="alert">
            <span>{turn.problem}</span>
            <span className="hstack-m">
              {needsAi && (
                <button type="button" className="btn btn-ghost btn-sm" onClick={onOpenAiSettings}>
                  {tx("Open AI settings")}
                </button>
              )}
              {/* Always there, so after fixing AI in Settings the same question can go again. */}
              <button type="button" className="btn btn-secondary btn-sm" onClick={onRetry}>
                <Icon icon={ArrowReloadHorizontalIcon} size="s" />
                {tx("Try again")}
              </button>
            </span>
          </div>
        )}

        {turn.state === "done" && (
          <>
            <p className="chat-answer selectable">{turn.text}</p>
            {(turn.notes ?? []).map((n) => (
              <p key={n} className="chat-note">
                {n}
              </p>
            ))}
            {turn.points.length > 0 && (
              <ul className="chat-points selectable">
                {turn.points.map((p, i) => (
                  <li key={i}>
                    <span>{p.text}</span>
                    {p.ids.slice(0, MOST_CITES).map((id) => {
                      const n = number(id);
                      const s = turn.sources[n - 1];
                      return n > 0 && s ? (
                        <button
                          key={id}
                          type="button"
                          className="chat-cite tnum"
                          data-tip={`${s.from} · ${s.subject || tx("(no subject)")}`}
                          onClick={() => onOpen(s)}
                        >
                          {n}
                        </button>
                      ) : null;
                    })}
                    {p.ids.length > MOST_CITES && (
                      <span className="chat-cite chat-cite-more tnum">+{p.ids.length - MOST_CITES}</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
            {turn.key.length > 0 && (
              <div className="chat-key">
                <div className="chat-sources-label">{tx("Worth opening")}</div>
                {turn.key.map((s) => (
                  <KeyCard key={s.id} source={s} onOpen={onOpen} />
                ))}
              </div>
            )}
            {/* Loose matches wait behind a button, unless they're all there is. */}
            {(turn.related ?? []).length > 0 && (
              <details className="chat-key chat-maybe" open={turn.key.length === 0}>
                <summary className="chat-sources-toggle">
                  <Icon icon={ArrowDown01Icon} size="xs" />
                  {turn.related!.length === 1
                    ? tx("1 more that may be related")
                    : tx("{n} more that may be related", { n: turn.related!.length })}
                </summary>
                {turn.related!.map((s) => (
                  <KeyCard key={s.id} source={s} onOpen={onOpen} />
                ))}
              </details>
            )}
            {turn.sources.length > 0 && (
              <details className="chat-sources">
                <summary className="chat-sources-toggle">
                  <Icon icon={ArrowDown01Icon} size="xs" />
                  {turn.sources.length === 1 ? tx("1 source") : tx("{n} sources", { n: turn.sources.length })}
                </summary>
                <div className="chat-source-rows">
                  {turn.sources.map((s, i) => (
                    <button key={s.id} type="button" className="chat-source-row" onClick={() => onOpen(s)}>
                      <span className="chat-source-n tnum">{i + 1}</span>
                      <span className="chat-source-subject truncate-1">{s.subject || tx("(no subject)")}</span>
                      <span className="chat-source-meta truncate-1">{s.from}</span>
                      <span className="chat-source-meta tnum">{shortDate(s.date)}</span>
                    </button>
                  ))}
                </div>
              </details>
            )}
            <div className="chat-tools">
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={() => {
                  const text = [turn.text, ...turn.points.map((p) => `• ${p.text}`)].join("\n");
                  void navigator.clipboard?.writeText(text).then(() => {
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  });
                }}
              >
                <Icon icon={Copy01Icon} size="s" />
                {copied ? tx("Copied") : tx("Copy")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** An email worth opening, as a card. */
function KeyCard({ source: s, onOpen }: { source: AskSource; onOpen: (s: AskSource) => void }) {
  return (
    <button type="button" className="chat-key-card" onClick={() => onOpen(s)}>
      <Avatar name={s.from} size="s" />
      <span className="chat-source-text">
        <span className="chat-key-subject truncate-1">{s.subject || tx("(no subject)")}</span>
        <span className="chat-source-meta truncate-1">
          {s.from} · {shortDate(s.date)}
        </span>
      </span>
      <Icon icon={ArrowRight01Icon} size="s" />
    </button>
  );
}
