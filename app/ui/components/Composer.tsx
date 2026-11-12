import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, api, onBeforeRestart, pgpApi, type MyTemplate, type TeamTemplate } from "../api";
import { PickTime, WhenItems } from "./Later";
import { useSettings } from "../settings";
import { readAsDataUrl } from "../files";
import { fileSize } from "../format";
import {
  AiMagicIcon,
  AlarmClockIcon,
  ArrowDown01Icon,
  ArrowExpandDiagonal01Icon,
  ArrowRight01Icon,
  ArrowShrink02Icon,
  Attachment01Icon,
  TextFontIcon,
  Clock01Icon,
  Note01Icon,
  Cancel01Icon,
  Delete02Icon,
  File01Icon,
  SentIcon,
  SquareLock02Icon,
  SquareUnlock02Icon,
  TextCheckIcon,
  Tick02Icon,
} from "../icons";
import type { Account, Addr, Draft, ReplyStart } from "../types";
import { MailBody } from "./MailBody";
import { RecipientInput, looksValid } from "./RecipientInput";
import { QUICK_ANSWERS } from "./Priority";
import { RichEditor } from "./RichEditor";
import { Avatar, Confirm, Icon, IconButton, Menu, MenuItem, Problem, Spinner } from "./ui";
import { tx } from "../i18n";
import { hasStyling, textAsHtml } from "../markdown";

const SAVE_AFTER_MS = 800;
const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const TITLES: Record<string, string> = {
  new: tx("New message"),
  reply: "Reply",
  reply_all: tx("Reply all"),
  forward: "Forward",
};

/** The signature a new message from this inbox starts with, as the engine picks it. */
function signatureOf(account: Account, mode: Draft["mode"]): string {
  const prefs = account.prefs;
  if (prefs.signatures.length === 0) {
    return account.signature.trim()
      ? account.signature
          .split("\n")
          .map((l) => (l.trim() ? `<p>${l.replace(/&/g, "&amp;").replace(/</g, "&lt;")}</p>` : "<p></p>"))
          .join("")
      : "";
  }
  const id = mode === "reply" || mode === "reply_all" ? prefs.signature_reply : prefs.signature_new;
  return prefs.signatures.find((s) => s.id === id)?.html ?? "";
}

const plainOf = (html: string) => {
  const box = document.createElement("div");
  box.innerHTML = html;
  return (box.textContent ?? "").replace(/\s+/g, " ").trim();
};

/**
 * The message with the old inbox's signature changed for the new one's. Left alone (null) if the
 * person has already changed the signature, so their own words are never lost.
 */
function withSignatureOf(html: string, from: Account, to: Account, mode: Draft["mode"]): string | null {
  const oldSignature = plainOf(signatureOf(from, mode));
  const newSignature = signatureOf(to, mode);
  const box = document.createElement("div");
  box.innerHTML = html;
  if (oldSignature) {
    const nodes = [...box.childNodes];
    let tail = "";
    let cut = -1;
    for (let i = nodes.length - 1; i >= 0; i -= 1) {
      tail = `${nodes[i].textContent ?? ""} ${tail}`;
      const seen = tail.replace(/\s+/g, " ").trim();
      if (seen === oldSignature) cut = i;
      if (seen.length > oldSignature.length) break;
    }
    if (cut < 0) return null;
    nodes.slice(cut).forEach((n) => n.remove());
  } else if (!newSignature) {
    return null;
  } else if (!plainOf(box.innerHTML)) {
    box.innerHTML = "<p></p><p></p>";
  }
  return box.innerHTML + newSignature;
}

/** One tap writes the reply; typing is only for anything else. */
const POLISH: [string, string][] = [
  ["fix", tx("Fix spelling and grammar")],
  ["shorter", tx("Make it shorter")],
  ["formal", tx("More formal")],
  ["friendly", tx("Friendlier")],
];

const AI_PICKS: [string, string][] = [
  [tx("Write it for me"), ""],
  ...QUICK_ANSWERS,
  [tx("Ask for details"), tx("Ask them for the details I need before I can answer.")],
  [tx("Say thanks"), tx("Thank them, warmly and briefly.")],
];

const isEmpty = (d: Draft) =>
  d.mode === "new" &&
  d.to.length + d.cc.length + d.bcc.length === 0 &&
  !d.subject.trim() &&
  !d.text.trim() &&
  d.attachments.length === 0;

/** Words that say a file comes with the email, in the languages people here write in. */
const MENTIONS_A_FILE =
  /\b(attach(ed|ment|ments|ing)?|enclosed|see the file|ci-joints?|pi[eè]ces? jointes?|adjunt[oa]s?|anbei|im anhang|in bijlage|allegat[oi])\b/i;

const TOOLS_KEY = "dakly.composer-tools";
const toolsWereOpen = () => {
  try {
    return localStorage.getItem(TOOLS_KEY) === "on";
  } catch {
    return false;
  }
};

export function Composer({
  draftId,
  accounts,
  initialProblem,
  inline = false,
  onClose,
  onSent,
  onQueue,
  onOpenSettings,
  onLeft,
  blankDrafts,
  onInboxGone,
  onScheduled,
  start,
  startAgain = 0,
  onStarted,
  onSendDone,
}: {
  /** What a reply opens with. */
  start?: ReplyStart | null;
  /** Goes up when another one-tap answer is chosen while this reply is already open, so it's added here. */
  startAgain?: number;
  onStarted?: () => void;
  /** Sending from the button that also finishes with the conversation. */
  onSendDone?: () => void;
  /** It's been set to go by itself at this time. */
  onScheduled?: (at: number) => void;
  /** Replies not touched yet, kept outside so the answer survives this panel reopening. */
  blankDrafts?: Set<number>;
  /** The inbox this was being written in has left this device. */
  onInboxGone?: () => void;
  draftId: number;
  /** A saved or thrown-away draft has settled after the panel went away. */
  onLeft?: () => void;
  /** Sits under the conversation it replies to, instead of floating over the window. */
  inline?: boolean;
  accounts: Account[];
  /** Why an earlier attempt to send this didn't work. */
  initialProblem?: { text: string; code?: string };
  onClose: () => void;
  onSent: (warnings: string[]) => void;
  /** Hands over to the undo-send wait, rather than sending now. */
  onQueue: (draftId: number) => void;
  onOpenSettings: (accountId: number) => void;
}) {
  const { settings } = useSettings();
  const [draft, setDraft] = useState<Draft | null>(null);
  const [showCopies, setShowCopies] = useState(false);
  const [showQuote, setShowQuote] = useState(false);
  const [sending, setSending] = useState(false);
  const [scheduling, setScheduling] = useState(false);
  const [attaching, setAttaching] = useState(0);
  const [dragging, setDragging] = useState(false);
  const [problem, setProblem] = useState<{ text: string; code?: string } | null>(initialProblem ?? null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  // Mentions a file but has none: asked once before it goes.
  const [askFile, setAskFile] = useState<{ andFinish: boolean } | null>(null);
  const [askPlain, setAskPlain] = useState<{ andFinish: boolean; withoutFile: boolean } | null>(null);
  const filePicker = useRef<HTMLInputElement>(null);
  const [tools, setTools] = useState(toolsWereOpen);
  const toggleTools = () =>
    setTools((on) => {
      try {
        localStorage.setItem(TOOLS_KEY, on ? "off" : "on");
      } catch {
        // It just starts closed next time.
      }
      return !on;
    });
  const [saved, setSaved] = useState(false);
  const [touched, setTouched] = useState(false);

  // Always the newest words, so a save never sends older ones.
  const box = useRef<HTMLDivElement>(null);
  const insertText = useRef<((text: string) => void) | null>(null);
  const replaceHtml = useRef<((html: string) => void) | null>(null);
  const [templates, setTemplates] = useState<TeamTemplate[]>([]);
  const [myTemplates, setMyTemplates] = useState<MyTemplate[]>([]);
  const [pickingTime, setPickingTime] = useState(false);
  const [aiNote, setAiNote] = useState<string | null>(null);
  const [aiBusy, setAiBusy] = useState(false);
  // Closing the reply stops the AI's words: nothing is typed into a closed reply.
  const open = useRef(true);
  useEffect(
    () => () => {
      open.current = false;
    },
    [],
  );
  const typeText = useRef<((text: string) => Promise<void>) | null>(null);
  /** The words as they were before AI changed them, so one click puts them back. */
  const [beforeAi, setBeforeAi] = useState<string | null>(null);
  const [hasAi, setHasAi] = useState(false);
  /** Whether this inbox has a PGP key, and who here has no key to encrypt to. */
  const [pgp, setPgp] = useState<{ own_key: boolean; missing: string[] } | null>(null);
  useEffect(() => {
    api
      .myTemplates()
      .then(setMyTemplates)
      .catch(() => {});
    api
      .aiStatus()
      .then((s) => setHasAi(s.has_key))
      .catch(() => {});
  }, []);
  // Who it's to, so the lock knows whether everyone has a key to encrypt to.
  const recipientKey = draft
    ? [...draft.to, ...draft.cc, ...draft.bcc].map((a) => a.address.trim().toLowerCase()).filter(Boolean).join(",")
    : "";
  const recipientList = recipientKey ? recipientKey.split(",") : [];
  const accountForKey = draft?.account_id;
  useEffect(() => {
    if (accountForKey === undefined) return;
    let live = true;
    pgpApi
      .canEncrypt(accountForKey, recipientKey ? recipientKey.split(",") : [])
      .then((r) => live && setPgp(r))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [accountForKey, recipientKey]);
  const onLeftRef = useRef(onLeft);
  onLeftRef.current = onLeft;
  const latest = useRef<Draft | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dirty = useRef(false);
  const finished = useRef(false);
  // A reply opened and never touched leaves nothing behind in Drafts.
  const isUntouched = () => !!blankDrafts?.has(draftId);
  const markTouched = () => {
    blankDrafts?.delete(draftId);
    setTouched(true);
  };
  const isUntouchedRef = useRef(isUntouched);
  isUntouchedRef.current = isUntouched;

  useEffect(() => {
    api
      .draftGet(draftId)
      .then((d) => {
        latest.current = d;
        setDraft(d);
        setShowCopies(d.cc.length > 0 || d.bcc.length > 0);
        // A scheduled message that came back for a reason says why.
        if (d.send_problem) setProblem((now) => now ?? { text: d.send_problem ?? "" });
      })
      .catch((e) => setProblem({ text: say(e) }));
  }, [draftId]);

  const accountId = draft?.account_id;
  const inboxGone = accountId !== undefined && !accounts.some((a) => a.id === accountId);
  useEffect(() => {
    if (!inboxGone) return;
    finished.current = true;
    onInboxGone?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inboxGone]);

  const [large, setLarge] = useState(false);

  // Brings the whole reply into view, under the conversation, down to its Send button. Again
  // a moment later, as the emails above finish laying out and push it down.
  const loaded = draft !== null;
  useEffect(() => {
    if (!inline || !loaded) return;
    const show = () => box.current?.scrollIntoView({ block: "end" });
    show();
    const soon = setTimeout(show, 300);
    const later = setTimeout(show, 1200);
    return () => {
      clearTimeout(soon);
      clearTimeout(later);
    };
  }, [inline, loaded]);

  // While writing, the reply grows with the words. Its bottom stays in view, so the Send
  // button and the line being typed never slide out of sight.
  useEffect(() => {
    const el = box.current;
    if (!inline || !loaded || !el) return;
    let tall = el.offsetHeight;
    const watch = new ResizeObserver(() => {
      const grew = el.offsetHeight > tall;
      tall = el.offsetHeight;
      if (grew && el.getBoundingClientRect().bottom > window.innerHeight) el.scrollIntoView({ block: "end" });
    });
    watch.observe(el);
    return () => watch.disconnect();
  }, [inline, loaded]);

  const saveNow = useCallback(async () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (!dirty.current || !latest.current) return;
    dirty.current = false;
    const d = latest.current;
    await api.draftSave({
      id: d.id,
      account_id: d.account_id,
      from_address: d.from_address,
      to: d.to,
      cc: d.cc,
      bcc: d.bcc,
      subject: d.subject,
      html: d.html,
      text: d.text,
      encrypt: d.encrypt ?? false,
      sign: d.sign ?? false,
      allow_plain: d.plain_ok ?? false,
    });
    setSaved(true);
  }, []);

  // An update restarting the app doesn't close this panel first, so what's typed is saved now.
  useEffect(() => onBeforeRestart(() => (finished.current ? Promise.resolve() : saveNow())), [saveNow]);

  // Saves what's there if the app closes or the panel goes away unexpectedly.
  useEffect(
    () => () => {
      if (finished.current || !latest.current) return;
      const settled = isUntouchedRef.current() ? api.draftDiscard(latest.current.id) : saveNow();
      void settled.catch(() => {}).finally(() => onLeftRef.current?.());
    },
    [saveNow],
  );

  const change = (patch: Partial<Draft>) => {
    if (!latest.current) return;
    markTouched();
    latest.current = { ...latest.current, ...patch };
    dirty.current = true;
    setSaved(false);
    // The editor keeps its own words, so typing doesn't need to redraw this panel.
    if (!("html" in patch)) setDraft(latest.current);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => void saveNow().catch(() => {}), SAVE_AFTER_MS);
  };

  /** Saved as it stands, and leaves by itself at that time. */
  const sendLater = async (at: number) => {
    // Sending and scheduling the same draft at once would send it twice.
    if (sending || scheduling) return;
    setScheduling(true);
    setProblem(null);
    try {
      await saveNow();
      await api.scheduleDraft(draftId, at);
      finished.current = true;
      blankDrafts?.delete(draftId);
      onScheduled?.(at);
    } catch (e) {
      setProblem({ text: say(e), code: e instanceof ApiError ? e.code : undefined });
    } finally {
      setScheduling(false);
    }
  };

  const setReminder = async (days: number | null) => {
    try {
      const d = await api.draftRemind(draftId, days);
      if (latest.current) latest.current = { ...latest.current, remind_days: d.remind_days };
      setDraft((now) => (now ? { ...now, remind_days: d.remind_days } : now));
    } catch (e) {
      setProblem({ text: say(e) });
    }
  };

  const writeWithAi = async (note: string = aiNote ?? "") => {
    const now = latest.current;
    if (!now) return;
    const parent = now.parent_message_id;
    // A new email has nothing to answer, so it's written from what they say it should say.
    if (!parent && !note.trim()) return;
    setAiBusy(true);
    setProblem(null);
    try {
      const reply = parent
        ? (await api.aiWriteReply(parent, note)).reply
        : (await api.aiPolish(note, "write", now.account_id)).text;
      if (!open.current) return;
      setAiBusy(false);
      markTouched();
      setAiNote(null);
      // Words appear as they come; a reply with lists or bold goes in whole, formatted.
      if (typeText.current && !hasStyling(reply)) await typeText.current(reply);
      else insertText.current?.(reply);
    } catch (e) {
      setProblem({ text: say(e) });
    } finally {
      setAiBusy(false);
    }
  };

  /** Their own words, made better. The old words stay one click away. */
  const polish = async (how: string) => {
    const now = latest.current;
    if (!now || !now.text.trim()) return;
    setAiBusy(true);
    setProblem(null);
    try {
      const { text } = await api.aiPolish(now.text, how, now.account_id);
      setBeforeAi(now.html);
      replaceHtml.current?.(textAsHtml(text));
      markTouched();
    } catch (e) {
      setProblem({ text: say(e) });
    } finally {
      setAiBusy(false);
    }
  };

  // Once, when there's an editor to put the words in, and again for each answer chosen after.
  useEffect(() => {
    if (!draft || !start) return;
    onStarted?.();
    const timer = setTimeout(() => {
      if ("text" in start) {
        insertText.current?.(start.text);
        markTouched();
      } else {
        void writeWithAi(start.note);
      }
    });
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft?.id, startAgain]);

  const attach = async (files: File[]) => {
    setProblem(null);
    markTouched();
    setAttaching((n) => n + files.length);
    for (const file of files) {
      try {
        await saveNow();
        const url = await readAsDataUrl(file);
        const updated = await api.draftAddAttachment(
          draftId,
          file.name,
          file.type,
          url.slice(url.indexOf(",") + 1),
        );
        if (latest.current) {
          latest.current = { ...latest.current, attachments: updated.attachments };
          setDraft(latest.current);
        }
      } catch (e) {
        setProblem({ text: say(e) });
      } finally {
        setAttaching((n) => n - 1);
      }
    }
  };

  const removeAttachment = async (id: string) => {
    try {
      await saveNow();
      const updated = await api.draftRemoveAttachment(draftId, id);
      if (latest.current) {
        latest.current = { ...latest.current, attachments: updated.attachments };
        setDraft(latest.current);
      }
    } catch (e) {
      setProblem({ text: say(e) });
    }
  };

  useEffect(() => {
    api
      .teamTemplates()
      .then(setTemplates)
      .catch(() => {});
  }, []);

  /** Sends despite a teammate having replied, once the person has seen the warning. */
  const sendAnyway = async () => {
    if (sending || scheduling) return;
    markTouched();
    setSending(true);
    setProblem(null);
    try {
      await saveNow();
      const reply = await api.sendDraft(draftId, true);
      finished.current = true;
      onSent(reply.warnings);
    } catch (e) {
      setProblem({ text: say(e), code: e instanceof ApiError ? e.code : undefined });
      setSending(false);
    }
  };

  const send = async (andFinish = false, withoutFile = false) => {
    markTouched();
    const d = latest.current;
    if (!d || sending || scheduling) return;
    if (!withoutFile && d.attachments.length === 0 && attaching === 0 && MENTIONS_A_FILE.test(`${d.subject}\n${d.text}`)) {
      setAskFile({ andFinish });
      return;
    }
    const everyone: Addr[] = [...d.to, ...d.cc, ...d.bcc];
    const bad = everyone.find((a) => !looksValid(a));
    if (bad) {
      setProblem({ text: tx("\"{0}\" doesn't look like an email address.", { "0": bad.address }) });
      return;
    }
    if (everyone.length === 0) {
      setProblem({ text: tx("Add at least one person to send this to.") });
      return;
    }
    if (d.from_encrypted && !d.encrypt && !d.plain_ok) {
      setAskPlain({ andFinish, withoutFile });
      return;
    }
    setSending(true);
    setProblem(null);
    try {
      await saveNow();
      if (Number(settings.undo_send ?? "0") > 0) {
        finished.current = true;
        onQueue(draftId);
        if (andFinish) onSendDone?.();
        return;
      }
      const reply = await api.sendDraft(draftId);
      finished.current = true;
      onSent(reply.warnings);
      if (andFinish) onSendDone?.();
    } catch (e) {
      setProblem({ text: say(e), code: e instanceof ApiError ? e.code : undefined });
      setSending(false);
    }
  };

  const close = () => {
    open.current = false;
    const d = latest.current;
    // The box goes at once. The draft is tidied up behind it, which can wait a moment
    // while AI is still busy.
    const tidy = async () => {
      try {
        // Nothing was written, so there's nothing worth keeping.
        if ((d && isEmpty(d)) || isUntouched()) {
          finished.current = true;
          await api.draftDiscard(draftId);
        } else {
          await saveNow();
        }
      } catch {
        // Closing still works. What was typed stays as last saved.
      }
    };
    void tidy();
    onClose();
  };

  const discard = async () => {
    open.current = false;
    finished.current = true;
    try {
      await api.draftDiscard(draftId);
    } catch (e) {
      finished.current = false;
      setProblem({ text: say(e) });
      setConfirmDiscard(false);
      return;
    }
    onClose();
  };

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const files = [...e.dataTransfer.files];
    if (files.length) void attach(files);
  };

  if (!draft) {
    return problem ? (
      <div className="composer">
        <div className="composer-head">
          <span className="composer-title">{tx("New message")}</span>
          <IconButton icon={Cancel01Icon} label={tx("Close")} small onClick={onClose} />
        </div>
        <div className="composer-problem">
          <Problem text={problem.text} />
        </div>
      </div>
    ) : null;
  }

  const account = accounts.find((a) => a.id === draft.account_id);
  const fromChoices = accounts.flatMap((a) => [
    { account: a, address: a.email, alias: "" },
    ...a.prefs.aliases.map((x) => ({ account: a, address: x.address, alias: x.address })),
  ]);
  const settingsHelp = problem?.code === "smtp_auth" || problem?.code === "smtp_connect";
  const canChooseFrom = fromChoices.length > 1 && (!inline || (account?.prefs.aliases.length ?? 0) > 0);

  return (
    <>
      {large && !inline && <div className="composer-dim" onClick={() => setLarge(false)} />}
      <div
        ref={box}
        role={inline ? "region" : "dialog"}
        aria-label={TITLES[draft.mode] ?? tx("New message")}
        className={`composer ${inline ? "composer-inline" : ""} ${large && !inline ? "composer-large" : ""} ${dragging ? "composer-drop" : ""}`}
        data-font={settings.font}
        data-font-size={settings.font_size}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            void send(e.shiftKey && !!onSendDone);
          } else if (e.key === "Escape") {
            e.preventDefault();
            void close();
          }
        }}
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={(e) => {
          if (e.currentTarget === e.target) setDragging(false);
        }}
        onDrop={onDrop}
      >
        <header className="composer-head">
          <span className="hstack">
            <span className="composer-title">{TITLES[draft.mode] ?? tx("New message")}</span>
            {!canChooseFrom && account && (
              <span className="composer-sender truncate-1">from {draft.from_address || account.email}</span>
            )}
            {saved && <span className="saved-note">{tx("Draft saved")}</span>}
          </span>
          <span className="hstack">
            {!inline && (
              <IconButton
                icon={large ? ArrowShrink02Icon : ArrowExpandDiagonal01Icon}
                label={large ? tx("Exit full screen") : tx("Full screen")}
                small
                className="composer-expand"
                onClick={() => setLarge((on) => !on)}
              />
            )}
            <IconButton icon={Cancel01Icon} label={tx("Close")} shortcut="esc" small onClick={() => void close()} />
          </span>
        </header>
        {draft.made_by_ai && (
          <div role="note" className="composer-problem fine">
            {tx("An AI tool you connected wrote this. Read it carefully before you send it.")}</div>
        )}

        {canChooseFrom && (
          <div className="composer-row">
            <span className="composer-label">{tx("From")}</span>
            <Menu
              label={tx("Send from")}
              trigger={(props) => (
                <button {...props} type="button" className="from-btn" aria-label={tx("From")}>
                  <Avatar name={account?.email ?? "?"} size="xs" hue={account?.prefs.color ?? undefined} />
                  <span className="truncate-1 grow">{draft.from_address || account?.email}</span>
                  <Icon icon={ArrowDown01Icon} size="xs" />
                </button>
              )}
            >
              {(close) => (
                <div className="switcher-menu">
                  {fromChoices.map((c) => (
                    <MenuItem
                      key={`${c.account.id}:${c.address}`}
                      leading={<Avatar name={c.account.email} size="xs" hue={c.account.prefs.color ?? undefined} />}
                      trailing={
                        c.account.id === draft.account_id && c.alias === draft.from_address ? (
                          <Icon icon={Tick02Icon} size="s" />
                        ) : undefined
                      }
                      onClick={() => {
                        close();
                        const before = accounts.find((a) => a.id === draft.account_id);
                        change({ account_id: c.account.id, from_address: c.alias });
                        if (before && before.id !== c.account.id && latest.current) {
                          const swapped = withSignatureOf(latest.current.html, before, c.account, draft.mode);
                          if (swapped !== null) replaceHtml.current?.(swapped);
                        }
                      }}
                    >
                      {c.address}
                    </MenuItem>
                  ))}
                </div>
              )}
            </Menu>
          </div>
        )}

        <RecipientInput
          label={tx("To")}
          accountId={draft.account_id}
          value={draft.to}
          onChange={(to) => change({ to })}
          autoFocus={draft.to.length === 0}
          action={
            !showCopies ? (
              <button className="btn btn-ghost btn-sm" onClick={() => setShowCopies(true)}>
                {tx("Cc Bcc")}</button>
            ) : (
              // Stays out of reach while they hold addresses, so nobody gets a copy unseen.
              draft.cc.length + draft.bcc.length === 0 && (
                <button className="btn btn-ghost btn-sm" onClick={() => setShowCopies(false)}>
                  {tx("Hide Cc Bcc")}</button>
              )
            )
          }
        />
        {showCopies && (
          <>
            <RecipientInput
              label={tx("Cc")}
              accountId={draft.account_id}
              value={draft.cc}
              onChange={(cc) => change({ cc })}
            />
            <RecipientInput
              label={tx("Bcc")}
              accountId={draft.account_id}
              value={draft.bcc}
              onChange={(bcc) => change({ bcc })}
            />
          </>
        )}
        {/* A reply keeps its subject, so there's no need to show it under the conversation. */}
        <div className="composer-row" hidden={inline && draft.mode !== "forward"}>
          <span className="composer-label">{tx("Subject")}</span>
          <input
            aria-label={tx("Subject")}
            className="plain-input strong"
            spellCheck={settings.spell_check === "on"}
            value={draft.subject}
            onChange={(e) => change({ subject: e.target.value })}
          />
        </div>

        <RichEditor
          toolbar={tools}
          insertRef={insertText}
          typeRef={typeText}
          busy={aiBusy && !draft.text.trim() ? tx("Writing your reply") : undefined}
          replaceRef={replaceHtml}
          initial={draft.html}
          autoFocus={draft.to.length > 0}
          spellCheck={settings.spell_check === "on"}
          onChange={(html, text) => change({ html, text })}
          onProblem={(text) => setProblem({ text })}
          onDropFiles={(files) => void attach(files)}
          belowText={
            draft.quoted_html && (
              <div className="earlier">
                <button className="earlier-toggle" onClick={() => setShowQuote((v) => !v)}>
                  <Icon icon={showQuote ? ArrowDown01Icon : ArrowRight01Icon} size="xs" />
                  {showQuote ? tx("Hide the earlier message") : tx("Show the earlier message")}
                </button>
                {showQuote && (
                  <div className="earlier-body">
                    <MailBody html={draft.quoted_html} text={draft.quoted_text} />
                  </div>
                )}
              </div>
            )
          }
          aboveToolbar={
            <>
              {/* In the flow above the toolbar, so nothing sits on top of it. */}
              {aiBusy && draft.text.trim() !== "" && (
                <div className="ai-writing" role="status">
                  <Spinner size="xs" />
                  {tx("Writing your reply… It shows up above in a few seconds, for you to check before sending.")}</div>
              )}
              {(draft.attachments.length > 0 || attaching > 0) && (
                <ul aria-label={tx("Attachments")} className="composer-files">
                  {draft.attachments.map((a) => (
                    <li key={a.id} className="file-chip">
                      <Icon icon={File01Icon} size="s" />
                      <span className="truncate-1" data-tip={a.name}>
                        {a.name}
                      </span>
                      <span className="file-meta">{fileSize(a.size)}</span>
                      <button
                        type="button"
                        className="chip-x"
                        aria-label={`Remove ${a.name}`}
                        onClick={() => void removeAttachment(a.id)}
                      >
                        <Icon icon={Cancel01Icon} size="xs" />
                      </button>
                    </li>
                  ))}
                  {attaching > 0 && (
                    <li className="file-chip muted">
                      <Spinner />
                      {tx("Adding…")}</li>
                  )}
                </ul>
              )}
              {problem && (
                <div className="composer-problem">
                  <Problem text={problem.text}>
                    {settingsHelp && account && (
                      <button className="link-btn" onClick={() => onOpenSettings(account.id)}>
                        {tx("Check the outgoing server settings")}</button>
                    )}
                    {(problem.code === "teammate_replied" || problem.code === "teammate_sending") && (
                      <button className="link-btn" disabled={sending} onClick={() => void sendAnyway()}>
                        {tx("Send anyway")}</button>
                    )}
                  </Problem>
                </div>
              )}
            </>
          }
        />

        <div className="composer-bottom">
        {draft.encrypt && pgp && pgp.missing.length > 0 && (
          <div role="status" className="tip">
            {tx("There's no encryption key for {who}, so they couldn't read it. Remove them, or turn encryption off.", { who: pgp.missing.join(", ") })}</div>
        )}
        {aiNote !== null && (
          <div className="ai-write">
            <div className="ai-picks" hidden={draft.parent_message_id === null}>
              {AI_PICKS.map(([label, note]) => (
                <button key={label} type="button" className="brief-chip" disabled={aiBusy} onClick={() => void writeWithAi(note)}>
                  {label}
                </button>
              ))}
            </div>
            <form
              className={`ai-write-row${aiBusy ? " is-busy" : ""}`}
              onSubmit={(e) => {
                e.preventDefault();
                void writeWithAi();
              }}
            >
              <Icon icon={AiMagicIcon} size="s" />
              <input
                autoFocus
                aria-label={tx("What should the reply say")}
                placeholder={
                  draft.parent_message_id === null
                    ? tx("Say what the email should say, like: ask Ana about dinner on Saturday")
                    : tx("Or say what it should say, like: yes, Friday works")
                }
                value={aiNote}
                onChange={(e) => setAiNote(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.stopPropagation();
                    setAiNote(null);
                  }
                }}
              />
              <button type="submit" className="btn btn-primary btn-sm" disabled={aiBusy}>
                {aiBusy && <Spinner size="xs" />}
                {tx("Write")}</button>
            </form>
          </div>
        )}
        <footer className="composer-foot">
          <button
            className="btn btn-primary"
            disabled={sending || scheduling || attaching > 0}
            data-tip={onSendDone ? tx("Send. Add ⇧ to archive and open the next one") : tx("Send")}
            data-tip-key="⌘↩"
            onClick={() => void send()}
          >
            {sending ? <Spinner /> : <Icon icon={SentIcon} size="s" />}
            {sending ? "Sending…" : "Send"}
          </button>
          <label className="btn btn-secondary composer-attach">
            <Icon icon={Attachment01Icon} size="s" />
            {tx("Attach")}<input
              ref={filePicker}
              type="file"
              multiple
              aria-label={tx("Attach files")}
              className="hidden"
              onChange={(e) => {
                const files = [...(e.target.files ?? [])];
                e.target.value = "";
                if (files.length) void attach(files);
              }}
            />
          </label>
          {hasAi && settings.ai_replies === "on" && draft.mode !== "forward" && (
            <button
              type="button"
              className="btn btn-secondary"
              aria-label={aiNote === null ? tx("Use AI") : tx("Hide AI")}
              aria-pressed={aiNote !== null}
              disabled={aiBusy}
              onClick={() => setAiNote((n) => (n === null ? "" : null))}
            >
              {aiBusy ? <Spinner size="xs" /> : <Icon icon={AiMagicIcon} size="s" />}
              {/* Opens and closes the AI box; its own Write button is what writes. */}
              {aiNote === null ? tx("Use AI") : tx("Hide AI")}</button>
          )}
          {hasAi && settings.ai_replies === "on" && (
            <Menu
              label={tx("Polish")}
              placement="above"
              trigger={(props) => (
                <button
                  {...props}
                  type="button"
                  className="tool"
                  aria-label={tx("Polish with AI")}
                  data-tip={tx("Fix spelling, or change the length and tone, with AI")}
                  disabled={aiBusy}
                >
                  <Icon icon={TextCheckIcon} size="m" />
                </button>
              )}
            >
              {(closeMenu) =>
                POLISH.map(([how, label]) => (
                  <MenuItem
                    key={how}
                    onClick={() => {
                      closeMenu();
                      void polish(how);
                    }}
                  >
                    {label}
                  </MenuItem>
                ))
              }
            </Menu>
          )}
          {beforeAi !== null && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              onClick={() => {
                replaceHtml.current?.(beforeAi);
                setBeforeAi(null);
              }}
            >
              {tx("Undo AI change")}</button>
          )}
          <Menu
            label={tx("Send later")}
            placement="above"
            trigger={(props) => (
              <button
                {...props}
                type="button"
                className="tool"
                aria-label={tx("Send later")}
                data-tip={tx("Send later")}
                disabled={sending || scheduling}
              >
                {scheduling ? <Spinner size="m" /> : <Icon icon={Clock01Icon} size="m" />}
              </button>
            )}
          >
            {(closeMenu) => <WhenItems close={closeMenu} onPick={(at) => void sendLater(at)} onOther={() => setPickingTime(true)} />}
          </Menu>
          {pgp?.own_key && (
            <button
              type="button"
              className={`tool ${draft.encrypt ? "tool-on" : ""}`}
              aria-label={draft.encrypt ? tx("Encrypted") : tx("Encrypt")}
              aria-pressed={!!draft.encrypt}
              data-tip={
                draft.encrypt
                  ? tx("Encrypted: only the people it's to can read it")
                  : pgp.missing.length === 0 && recipientList.length > 0
                    ? tx("Encrypt: everyone here can read encrypted email")
                    : tx("Encrypt")
              }
              onClick={() => change({ encrypt: !draft.encrypt, sign: !draft.encrypt })}
            >
              <Icon icon={draft.encrypt ? SquareLock02Icon : SquareUnlock02Icon} size="m" />
            </button>
          )}
          <Menu
            label={tx("Remind me")}
            placement="above"
            trigger={(props) => (
              <button
                {...props}
                type="button"
                className={`tool ${draft.remind_days ? "tool-on" : ""}`}
                aria-label={tx("Remind me if there's no reply")}
                data-tip={draft.remind_days ? tx("Reminding after {0} days with no reply", { "0": draft.remind_days }) : tx("Remind me if there's no reply")}
              >
                <Icon icon={AlarmClockIcon} size="m" />
              </button>
            )}
          >
            {(closeMenu) =>
              [
                [null, tx("Don't remind me")],
                [1, tx("After 1 day")],
                [3, tx("After 3 days")],
                [7, tx("After a week")],
              ].map(([days, name]) => (
                <MenuItem
                  key={String(name)}
                  trailing={(draft.remind_days ?? null) === days ? <Icon icon={Tick02Icon} size="s" /> : undefined}
                  onClick={() => {
                    closeMenu();
                    void setReminder(days as number | null);
                  }}
                >
                  {name}
                </MenuItem>
              ))
            }
          </Menu>
          {(templates.length > 0 || myTemplates.length > 0) && (
            <Menu
              label={tx("Templates")}
              placement="above"
              trigger={(props) => (
                <button {...props} type="button" className="tool" aria-label={tx("Templates")} data-tip={tx("Templates")}>
                  <Icon icon={Note01Icon} size="m" />
                </button>
              )}
            >
              {(closeMenu) =>
                [...myTemplates, ...templates].map((t) => (
                  <MenuItem
                    key={t.id}
                    onClick={() => {
                      closeMenu();
                      insertText.current?.(t.body);
                    }}
                  >
                    {t.name}
                  </MenuItem>
                ))
              }
            </Menu>
          )}
          <IconButton
            icon={TextFontIcon}
            label={tools ? tx("Hide formatting") : tx("Formatting")}
            pressed={tools}
            onClick={toggleTools}
          />
          <span className="spacer" />
          {(touched || !(isUntouched() || isEmpty(draft))) && (
            <IconButton icon={Delete02Icon} label={tx("Discard")} onClick={() => setConfirmDiscard(true)} />
          )}
        </footer>
        </div>
      </div>
      {pickingTime && (
        <PickTime
          title={tx("Send later")}
          confirm={tx("Schedule")}
          note={tx("It goes when the time comes, as long as this app is open then. If it isn't, it goes when you next open it.")}
          onClose={() => setPickingTime(false)}
          onPick={(at) => {
            setPickingTime(false);
            void sendLater(at);
          }}
        />
      )}
      {askFile && (
        <Confirm
          title={tx("Send without an attachment?")}
          confirm={tx("Send anyway")}
          keep={tx("Attach a file")}
          onKeep={() => {
            setAskFile(null);
            filePicker.current?.click();
          }}
          onConfirm={() => {
            const { andFinish } = askFile;
            setAskFile(null);
            void send(andFinish, true);
          }}
        >
          {tx("Your message talks about an attachment, but nothing is attached yet.")}</Confirm>
      )}
      {askPlain && (
        <Confirm
          title={tx("Send without encryption?")}
          confirm={tx("Send unencrypted")}
          keep={tx("Keep it encrypted")}
          danger
          onKeep={() => {
            setAskPlain(null);
            if (latest.current) {
              latest.current = { ...latest.current, encrypt: true, sign: true };
              setDraft(latest.current);
              dirty.current = true;
            }
          }}
          onConfirm={() => {
            const { andFinish, withoutFile } = askPlain;
            setAskPlain(null);
            if (latest.current) {
              latest.current = { ...latest.current, plain_ok: true };
              dirty.current = true;
            }
            void send(andFinish, withoutFile);
          }}
        >
          {tx("This includes words from an encrypted email. Sent unencrypted, anyone who handles the email on the way could read them.")}</Confirm>
      )}
      {confirmDiscard && (
        <Confirm
          title={tx("Throw this draft away?")}
          confirm={tx("Yes, discard")}
          keep={tx("Keep")}
          danger
          onKeep={() => setConfirmDiscard(false)}
          onConfirm={() => void discard()}
        >
          {tx("What you've written and any attached files will be gone.")}</Confirm>
      )}
    </>
  );
}
