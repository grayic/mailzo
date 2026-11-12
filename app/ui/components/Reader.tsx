import type { IconSvgElement } from "@hugeicons/react";
import { Fragment, useEffect, useMemo, useState, useRef } from "react";
import { TeamBar, TeamNotes, useTeamThread, whoIsReplying, type TeamThreadRef } from "./Team";
import { ApiError, api, calendarApi, onTheWeb, type Explained, type Invite } from "../api";
import { useSettings } from "../settings";
import { fileSize, fullDate } from "../format";
import {
  SquareLock02Icon,
  AiMagicIcon,
  AlarmClockIcon,
  ArrowLeft01Icon,
  Calendar03Icon,
  Cancel01Icon,
  Tick02Icon,
  Clock01Icon,
  LanguageSquareIcon,
  PinIcon,
  PinOffIcon,
  PrinterIcon,
  SourceCodeIcon,
  SentIcon,
  AlertCircleIcon,
  InformationCircleIcon,
  Attachment01Icon,
  Link01Icon,
  Tag01Icon,
  Search01Icon,
  VolumeOffIcon,
  Archive02Icon,
  InboxIcon,
  ArrowRight01Icon,
  ArrowTurnForwardIcon,
  Delete02Icon,
  Doc01Icon,
  File01Icon,
  FileZipIcon,
  FolderExportIcon,
  Folder01Icon,
  Image01Icon,
  Alert02Icon,
  CheckmarkCircle02Icon,
  MailBlock01Icon,
  Mail01Icon,
  MailOpen01Icon,
  MailReply01Icon,
  MailReplyAll01Icon,
  MoreHorizontalIcon,
  MusicNote01Icon,
  Pdf01Icon,
  Ppt01Icon,
  StarIcon,
  Video01Icon,
  ViewOffSlashIcon,
  Xls01Icon,
  AiChat02Icon,
} from "../icons";
import { findTrackers, hasQuotedPart, hasRemoteImages, trackedLinks } from "../prepareEmail";
import { PickTime, WhenItems } from "./Later";
import { TagPills } from "../tags";
import { FollowUpBar, StartFollowUps, useFollowUps } from "./FollowUps";
import type { Addr, Attachment, Body, Draft, Folder, Message, PgpInfo, ReplyStart, SharedStuff, ThreadTag } from "../types";
import { MailBody, textHasQuote } from "./MailBody";
import { ReplyHelp } from "./Priority";
import { ShowOriginal } from "./ShowOriginal";
import { FOLDER_ICONS } from "./Sidebar";
import { canGoToInbox } from "./UndoMove";
import { Avatar, Confirm, Dialog, Icon, IconButton, Menu, MenuItem, Problem, Spinner } from "./ui";
import { language, tx } from "../i18n";
import { CodeChip } from "./ThreadList";

const label = (a: Addr) => a.name || a.address;
/** Nobody reads replies to these, so there's no point offering to write one. */
const AUTOMATIC_SENDER = /no-?reply|do-?not-?reply|notifications?@|mailer-daemon|failed-payments|billing@|receipts?\+?/i;

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

const COPIED_MS = 1_500;

/** An email address that copies itself when clicked. */
function CopyAddress({ address, children }: { address: string; children: React.ReactNode }) {
  const [copied, setCopied] = useState(0);
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!copied) return;
    // The click hid the tooltip, so this brings it back saying what happened.
    button.current?.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    const timer = setTimeout(() => setCopied(0), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);
  if (!address) return <>{children}</>;
  return (
    <button
      ref={button}
      type="button"
      className="copy-address"
      aria-label={`Copy ${address}`}
      data-tip={copied ? tx("Copied") : `Copy ${address}`}
      data-tip-now={copied ? "" : undefined}
      onClick={(e) => {
        e.stopPropagation();
        void navigator.clipboard
          .writeText(address)
          .then(() => setCopied((n) => n + 1))
          .catch(() => {});
      }}
    >
      {children}
    </button>
  );
}

function People({ list }: { list: Addr[] }) {
  return (
    <>
      {list.map((a, i) => (
        <Fragment key={i}>
          {i > 0 && ", "}
          <CopyAddress address={a.address}>{label(a)}</CopyAddress>
        </Fragment>
      ))}
    </>
  );
}

function fileIcon(a: Attachment): IconSvgElement {
  const name = a.name.toLowerCase();
  if (name.endsWith(".pdf")) return Pdf01Icon;
  if (a.mime.startsWith("image/")) return Image01Icon;
  if (a.mime.startsWith("video/")) return Video01Icon;
  if (a.mime.startsWith("audio/")) return MusicNote01Icon;
  if (/\.(zip|gz|tar|rar|7z)$/.test(name)) return FileZipIcon;
  if (/\.(xlsx?|csv|numbers)$/.test(name)) return Xls01Icon;
  if (/\.(pptx?|key)$/.test(name)) return Ppt01Icon;
  if (/\.(docx?|pages|rtf|txt)$/.test(name)) return Doc01Icon;
  return File01Icon;
}

/** Names that open in the preview window: pictures, PDFs and plain text. */
const PREVIEWABLE = /\.(pdf|png|jpe?g|gif|webp|heic|txt|csv|md|log|json|ics|vcf)$/i;

type Preview = Awaited<ReturnType<typeof api.previewAttachment>>;

/** An attachment shown right here, with nothing saved anywhere. */
function AttachmentPreview({ preview, onSave, onClose }: { preview: Preview; onSave: () => void; onClose: () => void }) {
  // A PDF shows from a link made here and dropped when the window closes.
  const pdf = useMemo(() => {
    if (preview.kind !== "pdf" || !preview.data) return null;
    const bytes = Uint8Array.from(atob(preview.data), (c) => c.charCodeAt(0));
    return URL.createObjectURL(new Blob([bytes], { type: "application/pdf" }));
  }, [preview]);
  useEffect(() => () => {
    if (pdf) URL.revokeObjectURL(pdf);
  }, [pdf]);
  return (
    <Dialog
      title={preview.name}
      width="l"
      className="preview-dialog"
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-secondary" onClick={onSave}>
            {onTheWeb ? "Download" : tx("Save to Downloads")}
          </button>
          <button type="button" className="btn btn-primary" onClick={onClose}>
            {tx("Done")}</button>
        </>
      }
    >
      {preview.kind === "image" && preview.data && (
        <img className="preview-image" alt={preview.name} src={`data:${preview.mime};base64,${preview.data}`} />
      )}
      {pdf && <iframe className="preview-pdf" title={preview.name} src={pdf} />}
      {preview.kind === "text" && (
        <pre className="preview-text selectable">
          {preview.text}
          {preview.cut_short ? "\n…" : ""}
        </pre>
      )}
      {preview.kind === "none" && <div className="muted">{tx("There's no preview for this file. Save it to open it.")}</div>}
    </Dialog>
  );
}

export function AttachmentTile({
  messageId,
  attachment,
  index,
}: {
  messageId: number;
  attachment: Attachment;
  index: number;
}) {
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState<{ path: string; name: string; can_open: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [looking, setLooking] = useState(false);
  const canPreview = PREVIEWABLE.test(attachment.name);

  const look = async () => {
    setLooking(true);
    setError(null);
    try {
      setPreview(await api.previewAttachment(messageId, attachment.part ?? index));
    } catch (e) {
      setError(say(e));
    } finally {
      setLooking(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      setSaved(await api.saveAttachment(messageId, attachment.part ?? index));
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const use = (open: boolean) => {
    if (saved) void api.useSavedFile(saved.path, open).catch((e) => setError(say(e)));
  };

  return (
    <li className="file">
      <span className="file-icon">
        <Icon icon={fileIcon(attachment)} size="m" />
      </span>
      <span className="grow">
        <span className="file-name truncate-1 selectable" data-tip={attachment.name}>
          {attachment.name}
        </span>
        <span role={saved ? "status" : undefined} className="file-meta truncate-1">
          {error ??
            (saved ? tx("Saved to Downloads as {0}", { "0": saved.name }) : fileSize(attachment.size))}
        </span>
      </span>
      {onTheWeb ? (
        <span className="hstack">
          {canPreview && (
            <button className="btn btn-ghost btn-sm" disabled={looking} onClick={() => void look()}>
              {looking ? "Opening…" : "Preview"}
            </button>
          )}
          <a className="btn btn-ghost btn-sm" href={api.attachmentUrl(messageId, attachment.part ?? index)} download>
            {tx("Download")}</a>
        </span>
      ) : saved ? (
        <span className="hstack">
          {saved.can_open && (
            <button className="btn btn-ghost btn-sm" onClick={() => use(true)}>
              {tx("Open")}</button>
          )}
          <button className="btn btn-ghost btn-sm" onClick={() => use(false)}>
            {tx("Show in Finder")}</button>
        </span>
      ) : (
        <span className="hstack">
          {canPreview && (
            <button className="btn btn-ghost btn-sm" disabled={looking} onClick={() => void look()}>
              {looking ? "Opening…" : "Preview"}
            </button>
          )}
          <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => void save()}>
            {busy ? "Saving…" : "Save"}
          </button>
        </span>
      )}
      {preview && (
        <AttachmentPreview
          preview={preview}
          onClose={() => setPreview(null)}
          onSave={() => {
            setPreview(null);
            if (onTheWeb) window.location.assign(api.attachmentUrl(messageId, attachment.part ?? index));
            else void save();
          }}
        />
      )}
    </li>
  );
}

/** A calendar invitation inside an email, answered right here. */
/** What encryption did to this email, in a line. */
function PgpNote({ pgp }: { pgp: PgpInfo }) {
  if (pgp.problem) {
    return (
      <div role="note" className="tracker-note health-bad">
        <Icon icon={SquareLock02Icon} size="xs" />
        {pgp.problem}
      </div>
    );
  }
  const unconfirmed = pgp.signed && !pgp.verified && pgp.signer_unconfirmed;
  const signedPart = pgp.verified
    ? pgp.signer
      ? tx("Signed by {signer}, and the signature checks out", { signer: pgp.signer })
      : tx("Signed, and the signature checks out")
    : unconfirmed
      ? tx("Signed with a key from their earlier email, not confirmed")
      : tx("Signed, but the signature couldn't be checked");
  const parts = [pgp.encrypted ? tx("Encrypted: only you could read this") : null, pgp.signed ? signedPart : null].filter(Boolean);
  const bad = pgp.signed && !pgp.verified && !unconfirmed;
  return (
    <div
      role="note"
      className={`tracker-note ${bad ? "health-bad" : ""}`}
      data-tip={
        unconfirmed
          ? tx("Their key came with an earlier email from them. To confirm it, add their key yourself in Settings.")
          : bad
            ? tx("Their key isn't known here, or the email was changed after it was signed.")
            : undefined
      }
    >
      <Icon icon={SquareLock02Icon} size="xs" />
      {parts.join(" · ")}
    </div>
  );
}

/** An email that asks for something by a date: put it on the calendar, or be reminded. */
function DueCard({ message }: { message: Message }) {
  const [calendar, setCalendar] = useState<number | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const due = message.due_at;
  useEffect(() => {
    let live = true;
    calendarApi
      .list()
      .then((c) => live && setCalendar(c.calendars.find((x) => !x.problem)?.id ?? null))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  if (due === null || due < Date.now() / 1000 - 86_400) return null;
  // The date as written in the email, whatever this computer's time zone.
  const day = new Date(due * 1000);
  const [y, m, d] = [day.getUTCFullYear(), day.getUTCMonth(), day.getUTCDate()];
  const named = new Date(y, m, d).toLocaleDateString(language, { weekday: "long", day: "numeric", month: "long" });
  const run = async (work: () => Promise<string>) => {
    // One at a time, so a second click can't add the same day twice.
    if (busy) return;
    setBusy(true);
    setProblem(null);
    try {
      setDone(await work());
    } catch (e) {
      setProblem(say(e));
    } finally {
      setBusy(false);
    }
  };
  const addToCalendar = () =>
    run(async () => {
      if (calendar === null) return tx("No calendar to add it to.");
      const starts = new Date(y, m, d).getTime() / 1000;
      await calendarApi.save({
        calendar_id: calendar,
        title: message.subject || tx("From an email"),
        location: "",
        notes: tx("From an email from {0}", { "0": message.from.name || message.from.address }),
        starts,
        ends: starts + 86_400,
        all_day: true,
      });
      return tx("Added to your calendar.");
    });
  const remind = () =>
    run(async () => {
      // 9 in the morning the day before, or soon if that's already gone.
      const at = Math.max(new Date(y, m, d - 1, 9).getTime() / 1000, Date.now() / 1000 + 3600);
      await api.threadSnooze(message.account_id, message.thread_id, Math.round(at));
      return tx("Snoozed: it leaves your inbox now and comes back the morning before.");
    });
  return (
    <div role="group" aria-label={tx("Due date")} className="invite-card">
      <Icon icon={Calendar03Icon} size="m" />
      <div className="grow">
        <div className="strong">{tx("Asks for this by")}{" "}{named}</div>
        {done ? (
          <div className="muted">{done}</div>
        ) : (
          <div className="invite-answers">
            {calendar !== null && (
              <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void addToCalendar()}>
                {tx("Add to calendar")}</button>
            )}
            <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void remind()}>
              {tx("Remind me the day before")}</button>
          </div>
        )}
        <Problem text={problem} />
      </div>
    </div>
  );
}

function InviteCard({ messageId, attachments }: { messageId: number; attachments: Attachment[] }) {
  const [invite, setInvite] = useState<Invite | null>(null);
  const [removed, setRemoved] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const mayHaveOne =
    attachments.length === 0 ||
    attachments.some((a) => /calendar|ics/i.test(a.mime) || a.name.toLowerCase().endsWith(".ics"));
  useEffect(() => {
    if (!mayHaveOne) return;
    let live = true;
    calendarApi
      .invite(messageId)
      .then((found) => {
        if (!live || !found) return;
        setInvite(found);
        setStatus(found.my_status ?? null);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [messageId, mayHaveOne]);
  if (!invite) return null;
  const when = invite.all_day
    ? new Date(invite.starts * 1000).toLocaleDateString(language, { dateStyle: "full", timeZone: "UTC" })
    : `${new Date(invite.starts * 1000).toLocaleString(language, { dateStyle: "full", timeStyle: "short" })} – ${new Date(invite.ends * 1000).toLocaleTimeString(language, { hour: "numeric", minute: "2-digit" })}`;
  const answer = async (choice: "accept" | "maybe" | "decline") => {
    setBusy(true);
    setProblem(null);
    try {
      const reply = await calendarApi.respond(messageId, choice);
      setStatus(reply.status);
      if (reply.warnings[0]) setProblem(reply.warnings[0]);
    } catch (e) {
      setProblem(say(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div role="group" aria-label={tx("Invitation")} className="invite-card">
      <Icon icon={Calendar03Icon} size="m" />
      <div className="grow">
        <div className="strong">{invite.title}</div>
        <div className="muted">{when}</div>
        {invite.location && <div className="muted">{invite.location}</div>}
        {invite.organizer && (
          <div className="muted">
            {tx("From")}{" "}{invite.organizer}
            {invite.organizer_address && invite.organizer_address !== invite.organizer
              ? ` (${invite.organizer_address})`
              : ""}
          </div>
        )}
        {invite.other_sender && (
          <div className="fine health-bad">
            {tx("This email didn't come from the organiser's address. Only answer if you were expecting it.")}</div>
        )}
        {!invite.can_answer ? (
          <div className="fine">{tx("This email looks suspicious, so it can't be answered from here.")}</div>
        ) : invite.cancelled ? (
          <div className="invite-answers">
            <span>{tx("This was called off.")}</span>
            {removed ? (
              <span className="muted">{tx("Removed from your calendar.")}</span>
            ) : (
              <button
                className="btn btn-secondary btn-sm"
                onClick={() =>
                  void calendarApi
                    .cancelled(messageId)
                    .then(() => setRemoved(true))
                    .catch((e) => setProblem(say(e)))
                }
              >
                {tx("Remove from calendar")}</button>
            )}
          </div>
        ) : (
          <div className="invite-answers">
            {(
              [
                ["accept", "ACCEPTED", "Yes"],
                ["maybe", "TENTATIVE", "Maybe"],
                ["decline", "DECLINED", "No"],
              ] as const
            ).map(([choice, code, name]) => (
              <button
                key={choice}
                className={`btn btn-sm ${status === code ? "btn-primary" : "btn-secondary"}`}
                aria-pressed={status === code}
                disabled={busy}
                onClick={() => void answer(choice)}
              >
                {name}
              </button>
            ))}
          </div>
        )}
        <Problem text={problem} />
      </div>
    </div>
  );
}

export function MessageCard({
  message,
  startOpen,
  showPicture,
  trustedSender,
  onTrustSender,
  onCompose,
  inJunk,
  onSpam,
  onNotSpam,
  onUnsubscribe,
  onShowOriginal,
  find = "",
  translated,
  keyFacts = false,
}: {
  /** Left out where there's no room to show it. */
  onShowOriginal?: (message: Message) => void;
  /** Mark amounts, dates and questions, in mail waiting on this person. */
  keyFacts?: boolean;
  /** Words being looked for in the conversation. */
  find?: string;
  /** This email in another language, if asked for. */
  translated?: string;
  inJunk: boolean;
  onSpam: () => void;
  onNotSpam: () => void;
  /** Left out where unsubscribing isn't offered. */
  onUnsubscribe?: (message: Message) => void;
  message: Message;
  startOpen: boolean;
  showPicture: boolean;
  /** The person already chose to see this sender's pictures. */
  trustedSender: boolean;
  /** Left out in spam, where the choice isn't remembered. */
  onTrustSender?: (allowed: boolean) => void;
  onCompose: (mode: Draft["mode"], message: Message) => void;
}) {
  const [wasOpened, setOpen] = useState(startOpen);
  const [body, setBody] = useState<Body | null>(message.body);
  const lookedFor = find.trim().toLowerCase();
  const holdsIt =
    lookedFor !== "" &&
    `${message.subject} ${label(message.from)} ${message.body?.text ?? ""}`.toLowerCase().includes(lookedFor);
  // A match shows itself, without losing what was open or shut.
  const open = wasOpened || holdsIt;
  const [error, setError] = useState<string | null>(null);
  const { settings } = useSettings();
  const [showOnce, setShowOnce] = useState(false);
  // Suspect mail never loads pictures by itself, whatever was chosen before.
  const careful = message.risk > 0;
  const showImages =
    showOnce || (!careful && (trustedSender || (settings.remote_images === "always" && !!onTrustSender)));
  // Taking a sender off the allowed list hides their pictures again.
  useEffect(() => {
    if (!trustedSender) setShowOnce(false);
  }, [trustedSender]);
  const [leaving, setLeaving] = useState(false);
  const [showQuoted, setShowQuoted] = useState(false);

  useEffect(() => {
    if (body || !open) return;
    let live = true;
    api
      .fetchBody(message.id)
      .then((b) => live && setBody(b))
      .catch((e) => live && setError(say(e)));
    return () => {
      live = false;
    };
  }, [message.id, body, open]);

  const remote = useMemo(() => !!body?.html && hasRemoteImages(body.html), [body]);
  const quoted = useMemo(
    () => (body?.html ? hasQuotedPart(body.html) : textHasQuote(body?.text ?? null)),
    [body],
  );
  const trackers = useMemo(() => (body?.html ? findTrackers(body.html) : []), [body]);
  const trackerNames = trackers.join(", ");
  const cleanedLinks = useMemo(() => (body?.html ? trackedLinks(body.html) : 0), [body]);
  useEffect(() => {
    if (trackerNames) void api.noteTrackers(message.id, trackerNames.split(", ")).catch(() => {});
  }, [trackerNames, message.id]);
  // Counts toward the total shown in Privacy.
  const holdingImages = remote && !showImages;
  useEffect(() => {
    if (holdingImages) void api.noteImagesHeld(message.id).catch(() => {});
  }, [holdingImages, message.id]);
  const files = (body?.attachments ?? []).map((a, i) => ({ a, i })).filter(({ a }) => !a.inline);
  const preview = (body?.text ?? "").replace(/\s+/g, " ").slice(0, 140);
  const sender = label(message.from) || tx("(unknown sender)");
  const pictureFor = showPicture && !careful ? message.from.address : undefined;

  if (!open) {
    return (
      <article className="card">
        <button className="card-collapsed" onClick={() => setOpen(true)}>
          <Avatar name={sender} email={pictureFor} size="s" />
          <span className="card-sender truncate-1">{sender}</span>
          <span className="muted truncate-1 grow">{preview}</span>
          <time className="card-date">{fullDate(message.date)}</time>
        </button>
      </article>
    );
  }

  return (
    <article className="card">
      {leaving && (
        <Confirm
          title={`Unsubscribe from ${sender}?`}
          confirm={tx("Yes, unsubscribe")}
          keep={tx("Stay subscribed")}
          onKeep={() => setLeaving(false)}
          onConfirm={() => {
            setLeaving(false);
            onUnsubscribe?.(message);
          }}
        >
          {tx("This tells the sender to stop emailing this address.")}</Confirm>
      )}
      <header className="card-head">
        <Avatar name={sender} email={pictureFor} />
        <div className="grow" onClick={() => setOpen(false)} data-tip={tx("Collapse")}>
          <div className="truncate-1">
            <span className="card-sender selectable">
              {message.from.name ? sender : <CopyAddress address={message.from.address}>{sender}</CopyAddress>}
            </span>{" "}
            {message.from.name && (
              <span className="card-address">
                <CopyAddress address={message.from.address}>{message.from.address}</CopyAddress>
              </span>
            )}
            {/* The same rule as Newsletters: not for suspect mail, Junk, Trash or your own. */}
            {onUnsubscribe && message.can_unsubscribe && (
              <button
                type="button"
                className="link-btn card-unsubscribe"
                onClick={(e) => {
                  e.stopPropagation();
                  setLeaving(true);
                }}
              >
                {tx("Unsubscribe")}</button>
            )}
          </div>
          <div className="card-to truncate-1">
            {tx("To")}{" "}{message.to.length > 0 ? <People list={message.to} /> : tx("(not listed)")}
            {message.cc.length > 0 && (
              <>
                {tx(", cc")}{" "}<People list={message.cc} />
              </>
            )}
            {message.bcc.length > 0 && (
              <>
                {tx(", bcc")}{" "}<People list={message.bcc} />
              </>
            )}
          </div>
        </div>
        <time className="card-date">{fullDate(message.date)}</time>
        <span className="card-actions">
          <IconButton icon={MailReply01Icon} label={tx("Reply to this message")} small onClick={() => onCompose("reply", message)} />
          <IconButton icon={MailReplyAll01Icon} label={tx("Reply all to this message")} small onClick={() => onCompose("reply_all", message)} />
          <IconButton icon={ArrowTurnForwardIcon} label={tx("Forward this message")} small onClick={() => onCompose("forward", message)} />
          {onShowOriginal && (
            <IconButton icon={SourceCodeIcon} label={tx("Show original")} small onClick={() => onShowOriginal(message)} />
          )}
        </span>
      </header>
      {error ? (
        <div className="pictures-note">
          <Problem text={error} />
        </div>
      ) : body ? (
        <>
          {message.warnings.length > 0 && (
            <div role="alert" className="scam-note">
              <Icon icon={Alert02Icon} size="s" />
              <div className="grow">
                <div className="strong">
                  {inJunk
                    ? tx("Why this is in Spam")
                    : message.risk === 2
                      ? tx("This looks like spam")
                      : tx("Be careful with this email")}
                </div>
                {message.warnings.map((w) => (
                  <div key={w}>{w}</div>
                ))}
              </div>
              {!inJunk && (
                <button className="btn btn-secondary btn-sm" onClick={onSpam}>
                  {tx("Mark as spam")}</button>
              )}
              <button className="btn btn-ghost btn-sm" onClick={onNotSpam}>
                {inJunk ? tx("Not spam") : tx("It's fine")}
              </button>
            </div>
          )}
          {remote && !showImages && (
            <div className="pictures-note">
              <Icon icon={ViewOffSlashIcon} size="s" />
              <span className="grow">{tx("Pictures are hidden, so the sender can't tell you opened this.")}</span>
              <button
                className="btn btn-ghost btn-sm"
                onClick={() => {
                  setShowOnce(true);
                  if (!careful) onTrustSender?.(true);
                }}
              >
                {tx("Show pictures")}</button>
            </div>
          )}
          <InviteCard messageId={message.id} attachments={body.attachments} />
          <DueCard message={message} />
          {trackers.length > 0 && (
            <div className="tracker-note" data-tip={tx("Known trackers stay blocked even if you show the pictures")}>
              <Icon icon={ViewOffSlashIcon} size="xs" />
              {tx("Blocked a tracker from {who}", { who: trackers.join(", ") })}
            </div>
          )}
          {body.pgp && <PgpNote pgp={body.pgp} />}
          {cleanedLinks > 0 && (
            <div
              className="tracker-note"
              data-tip={tx("Links still go to the same page. The codes only told the sender who clicked, in which email.")}
            >
              <Icon icon={ViewOffSlashIcon} size="xs" />
              {cleanedLinks === 1 ? tx("Removed tracking codes from 1 link") : tx("Removed tracking codes from {n} links", { n: cleanedLinks })}
            </div>
          )}
          {translated && (
            <div role="note" className="translated selectable">
              <div className="section-title">{tx("Translation")}</div>
              {translated}
            </div>
          )}
          <MailBody
            html={body.html}
            text={body.text}
            showImages={showImages}
            hideQuoted={quoted && !showQuoted && !lookedFor}
            highlight={lookedFor}
            keyFacts={keyFacts}
          />
          {remote && trustedSender && onTrustSender && settings.remote_images !== "always" && (
            <div className="trusted-note">
              {tx("Pictures from this sender are always shown.")}<button
                className="link-btn"
                onClick={() => {
                  setShowOnce(false);
                  onTrustSender(false);
                }}
              >
                {tx("Stop showing them")}</button>
            </div>
          )}
          {quoted && (
            <button
              className={`quote-toggle ${body.html ? "quote-toggle-frame" : ""}`}
              aria-label={showQuoted ? tx("Hide the earlier messages") : tx("Show the earlier messages")}
              data-tip={showQuoted ? tx("Hide the earlier messages") : tx("Show the earlier messages")}
              onClick={() => setShowQuoted((v) => !v)}
            >
              <Icon icon={MoreHorizontalIcon} size="s" />
            </button>
          )}
          {files.length > 0 && (
            <ul aria-label={tx("Attachments")} className="files">
              {files.map(({ a, i }) => (
                <AttachmentTile key={i} messageId={message.id} attachment={a} index={i} />
              ))}
            </ul>
          )}
        </>
      ) : null}
    </article>
  );
}

/**
 * Senders whose pictures are always shown, as last read. Each conversation opens a new reader,
 * often at once from memory, and without this it would show "Pictures are hidden" for a moment
 * until the list came back, even for a sender the person has just chosen to trust.
 */
let knownTrusted: string[] = [];
export const forgetTrustedSenders = () => {
  knownTrusted = [];
};

/** Longer conversations fold the emails between the first and the ones to read. */
const FOLD_FROM = 4;

export function Reader({
  onBackToAnswer,
  onAskAbout,
  code,
  onCopyCode,
  messages,
  loading,
  folder,
  folders,
  accountEmail,
  onGoToInbox,
  starred,
  onCompose,
  onReplyWith,
  needsYou = false,
  onArchive,
  onDelete,
  onDeleteForever,
  settingsOpen,
  replyBox,
  replyStarting,
  team,
  onSpam,
  onNotSpam,
  onUnsubscribe,
  onMarkUnread,
  onMarkRead,
  unread,
  replyMode,
  onStar,
  onMove,
  marks,
  ai,
  onPrint,
  onTags,
  tags,
  canFollowUp = false,
  findRequest = 0,
  onPhoneBack,
}: {
  /** Back to the list, on a phone where it isn't beside this. */
  onPhoneBack?: () => void;
  /** Asking about everything with the people in this conversation. */
  onAskAbout?: () => void;
  /** Opened from an answer about search results: back to that answer. */
  onBackToAnswer?: () => void;
  /** A sign-in code the conversation gives, and copying it. */
  code?: string;
  onCopyCode?: () => void;
  /** Goes up each time someone asks to search within this conversation. */
  findRequest?: number;
  /** Pin, mute and setting aside. Left out where they make no sense. */
  marks?: {
    pinned: boolean;
    muted: boolean;
    snoozed: boolean;
    reminding: boolean;
    onPin: () => void;
    onMute: () => void;
    onSnooze: (at: number | null) => void;
    onFollowUp: (at: number | null) => void;
    /** In or out of Needs you, where the tabs are on. */
    needsYou?: boolean;
    onNeedsMe?: () => void;
  };
  /** What AI may help with here, if it's set up. */
  ai?: { summaries: boolean; translate: boolean; language: string };
  onPrint?: () => void;
  /** The newest email is one the person sent, so follow-ups can go after it. */
  canFollowUp?: boolean;
  /** Chooses the person's own tags for this conversation. */
  onTags?: () => void;
  tags?: ThreadTag[];
  messages: Message[] | null;
  loading: boolean;
  folder: Folder | null;
  folders: Folder[];
  /** Named when more than one inbox is connected. */
  accountEmail?: string;
  onGoToInbox: () => void;
  starred: boolean;
  onCompose: (mode: Draft["mode"], message: Message) => void;
  /** Starts a reply with words already in it, or asks AI to write them. */
  onReplyWith?: (message: Message, start: ReplyStart) => void;
  /** Waiting on this person, so helping them answer is worth it. */
  needsYou?: boolean;
  onArchive: () => void;
  onDelete: () => void;
  onDeleteForever: () => void;
  onMarkUnread: () => void;
  onMarkRead: () => void;
  /** Still marked unread, which happens when marking read is left to the person. */
  unread: boolean;
  /** What the main reply action does. */
  replyMode: "reply" | "reply_all";
  onStar: () => void;
  onMove: (path: string) => void;
  /** A reply being written, which sits under the conversation. */
  replyBox?: React.ReactNode;
  /** A reply is being made, so the Reply button shows it's working. */
  replyStarting?: boolean;
  /** The conversation, if it's in an inbox shared with a team. */
  team?: { thread: TeamThreadRef; tick: number; replying: boolean; onChanged: () => void } | null;
  onSpam: () => void;
  onNotSpam: () => void;
  onUnsubscribe: (message: Message) => void;
  /** The allowed senders can change in Settings. */
  settingsOpen: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  const [askingSpam, setAskingSpam] = useState(false);
  const [original, setOriginal] = useState<Message | null>(null);
  const [trusted, setTrusted] = useState<string[]>(knownTrusted);
  const [picking, setPicking] = useState<"snooze" | "follow" | null>(null);
  // A long conversation opens on its newest email, or the first one not read yet, with the
  // ones in between folded into one line, the way Gmail does.
  const conversation = messages?.[0]?.id ?? null;
  const [allShown, setAllShown] = useState<number | null>(null);
  const focusIndex = messages
    ? (() => {
        const unread = messages.findIndex((m) => !m.seen);
        return unread >= 0 ? unread : messages.length - 1;
      })()
    : -1;
  // Read once per conversation, so marking it read as it opens doesn't move the view.
  const focusRef = useRef<{ conversation: number | null; index: number }>({ conversation: null, index: -1 });
  if (focusRef.current.conversation !== conversation) focusRef.current = { conversation, index: focusIndex };
  const focusAt = focusRef.current.index;
  const folding = !!messages && messages.length > FOLD_FROM && allShown !== conversation;
  // The first email stays, then the fold, then everything from just before the one to read.
  const keepFrom = Math.max(1, Math.min(focusAt, (messages?.length ?? 0) - 1) - 1);
  const focusCard = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!conversation || focusAt <= 0) return;
    // After the cards are drawn, so it lands on the right place.
    const frame = requestAnimationFrame(() => focusCard.current?.scrollIntoView({ block: "start" }));
    return () => cancelAnimationFrame(frame);
  }, [conversation, focusAt]);
  const [finding, setFinding] = useState(false);
  // Opening another email starts this again, and that's not asking for the find bar.
  const findSeen = useRef(findRequest);
  const [find, setFind] = useState("");
  const [summary, setSummary] = useState<{ summary: string; points: string[]; next?: string; needs_reply: boolean } | null>(null);
  const [explained, setExplained] = useState<Explained | null>(null);
  const [aiBusy, setAiBusy] = useState(false);
  const [aiProblem, setAiProblem] = useState<string | null>(null);
  const [translated, setTranslated] = useState<{ id: number; text: string } | null>(null);
  const [showShared, setShowShared] = useState(false);
  const [startingFollowUps, setStartingFollowUps] = useState(false);
  const firstMessage = messages?.[0];
  const followUps = useFollowUps(firstMessage?.account_id ?? 0, firstMessage?.thread_id ?? "", messages?.length ?? 0);
  const followUp = followUps.runs.find((r) => r.state === "running" || r.state === "failed");
  useEffect(() => {
    if (findRequest > findSeen.current) setFinding(true);
    findSeen.current = findRequest;
  }, [findRequest]);
  const { view: teamView, reload: reloadTeam } = useTeamThread(
    team?.thread ?? null,
    team?.tick ?? 0,
    team?.replying ?? false,
  );
  const teamChanged = () => {
    reloadTeam();
    team?.onChanged();
  };

  // A summary made before comes back for free, until a new email or reply joins the thread.
  const newest = messages?.[messages.length - 1];
  useEffect(() => {
    if (!newest || !ai?.summaries) return;
    let live = true;
    api
      .aiSummarize(newest.account_id, newest.thread_id, ai.language, true)
      .then((saved) => live && setSummary(saved))
      .catch(() => {});
    api
      .aiExplain(newest.account_id, newest.thread_id, ai.language, true)
      .then((saved) => live && setExplained(saved))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [newest?.id, ai?.summaries, ai?.language]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    api
      .imageSenders()
      .then((list) => {
        knownTrusted = list;
        setTrusted(list);
      })
      .catch(() => {});
  }, [settingsOpen]);

  const trustSender = (address: string, allowed: boolean) => {
    const key = address.toLowerCase();
    const others = trusted.filter((a) => a !== key);
    knownTrusted = allowed ? [...others, key] : others;
    setTrusted(knownTrusted);
    void api.setSenderImages(key, allowed).catch(() => {});
  };

  if (!messages || messages.length === 0) {
    if (loading) {
      return (
        <div className="reader-waiting">
          {onPhoneBack && (
            <IconButton icon={ArrowLeft01Icon} label={tx("Back to the list")} className="phone-only reader-back" onClick={onPhoneBack} />
          )}
        </div>
      );
    }
    return (
      <div className="empty" data-tauri-drag-region>
        <div className="empty-icon">
          <Icon icon={Mail01Icon} size="l" />
        </div>
        <div className="empty-title">{tx("Pick a conversation to read it")}</div>
        <div className="empty-keys">
          <span>
            <span className="kbd">⌘N</span>{tx("New message")}</span>
          <span>
            <span className="kbd">⌘R</span>{tx("Reply")}</span>
        </div>
      </div>
    );
  }

  const subject = messages[0].subject;
  const last = messages[messages.length - 1];
  const wanted = find.trim().toLowerCase();
  const matches = wanted
    ? messages.filter((m) =>
        `${m.subject} ${label(m.from)} ${m.body?.text ?? ""}`.toLowerCase().includes(wanted),
      ).length
    : 0;

  // One AI job at a time: they share the one spinner and answer box.
  const summarize = async () => {
    if (aiBusy) return;
    setAiBusy(true);
    setAiProblem(null);
    try {
      setSummary(await api.aiSummarize(last.account_id, last.thread_id, ai?.language));
    } catch (e) {
      setAiProblem(say(e));
    } finally {
      setAiBusy(false);
    }
  };

  const explain = async () => {
    if (aiBusy) return;
    setAiBusy(true);
    setAiProblem(null);
    try {
      setExplained(await api.aiExplain(last.account_id, last.thread_id, ai?.language));
    } catch (e) {
      setAiProblem(say(e));
    } finally {
      setAiBusy(false);
    }
  };

  const translate = async () => {
    if (!ai || aiBusy) return;
    setAiBusy(true);
    setAiProblem(null);
    try {
      const reply = await api.aiTranslate(last.id, ai.language);
      setTranslated({ id: last.id, text: reply.text });
    } catch (e) {
      setAiProblem(say(e));
    } finally {
      setAiBusy(false);
    }
  };
  const inBin = folder?.role === "trash" || folder?.role === "junk";

  return (
    <div className="reader-page">
      <div role="toolbar" aria-label={tx("Conversation actions")} className="reader-bar" data-tauri-drag-region="deep">
        {onPhoneBack && (
          <IconButton icon={ArrowLeft01Icon} label={tx("Back to the list")} className="phone-only reader-back" onClick={onPhoneBack} />
        )}
        {canGoToInbox(folder?.role) && (
          <IconButton icon={InboxIcon} label={tx("Move to inbox")} onClick={() => onMove("inbox")} />
        )}
        {folder?.role !== "archive" && !inBin && (
          <IconButton icon={Archive02Icon} label={tx("Archive")} onClick={onArchive} />
        )}
        {inBin ? (
          <IconButton icon={Delete02Icon} label={tx("Delete for good")} onClick={() => setConfirming(true)} />
        ) : (
          <IconButton icon={Delete02Icon} label={tx("Delete")} onClick={onDelete} />
        )}
        {unread ? (
          <IconButton icon={MailOpen01Icon} label={tx("Mark read")} onClick={onMarkRead} />
        ) : (
          <IconButton icon={Mail01Icon} label={tx("Mark unread")} onClick={onMarkUnread} />
        )}
        {folder?.role === "junk" ? (
          <IconButton icon={CheckmarkCircle02Icon} label={tx("Not spam")} onClick={onNotSpam} />
        ) : folder?.role !== "trash" ? (
          <IconButton icon={MailBlock01Icon} label={tx("Mark as spam")} onClick={() => setAskingSpam(true)} />
        ) : null}
        <IconButton
          icon={StarIcon}
          label={starred ? tx("Remove star") : tx("Star")}
          pressed={starred}
          iconClassName={starred ? "star-on" : undefined}
          className={starred ? "star-on" : undefined}
          onClick={onStar}
        />
        <Menu
          label={tx("Move to folder")}
          trigger={(props) => (
            <button {...props} type="button" className="icon-btn" aria-label={tx("Move to folder")} data-tip={tx("Move to folder")}>
              <Icon icon={FolderExportIcon} />
            </button>
          )}
        >
          {(close) =>
            folders
              .filter((f) => f.id !== folder?.id)
              .map((f) => (
                <MenuItem
                  key={f.id}
                  icon={FOLDER_ICONS[f.role] ?? Folder01Icon}
                  onClick={() => {
                    close();
                    onMove(f.path);
                  }}
                >
                  {f.name}
                </MenuItem>
              ))
          }
        </Menu>
        {marks && (
          <Menu
            label={tx("Snooze")}
            trigger={(props) => (
              <button
                {...props}
                type="button"
                className="icon-btn"
                aria-label={tx("Snooze")}
                data-tip={marks.snoozed ? tx("Snoozed") : tx("Snooze")}
              >
                <Icon icon={Clock01Icon} />
              </button>
            )}
          >
            {(close) =>
              marks.snoozed ? (
                <MenuItem
                  icon={Clock01Icon}
                  onClick={() => {
                    close();
                    marks.onSnooze(null);
                  }}
                >
                  {tx("Bring back now")}</MenuItem>
              ) : (
                <WhenItems close={close} onPick={marks.onSnooze} onOther={() => setPicking("snooze")} />
              )
            }
          </Menu>
        )}
        <Menu
          label={tx("More actions")}
          trigger={(props) => (
            <button {...props} type="button" className="icon-btn" aria-label={tx("More actions")} data-tip={tx("More")}>
              <Icon icon={MoreHorizontalIcon} />
            </button>
          )}
        >
          {(close) => (
            <>
              {marks && (
                <>
                  <MenuItem
                    icon={marks.pinned ? PinOffIcon : PinIcon}
                    onClick={() => {
                      close();
                      marks.onPin();
                    }}
                  >
                    {marks.pinned ? "Unpin" : tx("Pin to the top")}
                  </MenuItem>
                  <MenuItem
                    icon={AlarmClockIcon}
                    onClick={() => {
                      close();
                      if (marks.reminding) marks.onFollowUp(null);
                      else setPicking("follow");
                    }}
                  >
                    {marks.reminding ? tx("Cancel reminder") : canFollowUp ? tx("Remind me if there's no reply") : tx("Remind me to reply")}
                  </MenuItem>
                  <MenuItem
                    icon={VolumeOffIcon}
                    onClick={() => {
                      close();
                      marks.onMute();
                    }}
                  >
                    {marks.muted ? "Unmute" : tx("Mute conversation")}
                  </MenuItem>
                  {marks.onNeedsMe && (
                    <MenuItem
                      icon={marks.needsYou ? Cancel01Icon : Tick02Icon}
                      onClick={() => {
                        close();
                        marks.onNeedsMe?.();
                      }}
                    >
                      {marks.needsYou ? tx("Doesn't need me") : tx("Needs me")}
                    </MenuItem>
                  )}
                  <div className="menu-sep" />
                </>
              )}
              {onTags && (
                <MenuItem
                  icon={Tag01Icon}
                  onClick={() => {
                    close();
                    onTags();
                  }}
                >
                  {tx("Tags…")}</MenuItem>
              )}
              {canFollowUp && followUp?.state !== "running" && (
                <MenuItem
                  icon={SentIcon}
                  onClick={() => {
                    close();
                    setStartingFollowUps(true);
                  }}
                >
                  {tx("Follow up automatically…")}</MenuItem>
              )}
              <MenuItem
                icon={Attachment01Icon}
                onClick={() => {
                  close();
                  setShowShared(true);
                }}
              >
                {tx("Files and links")}</MenuItem>
              <MenuItem
                icon={Search01Icon}
                onClick={() => {
                  close();
                  setFinding(true);
                }}
              >
                {tx("Find in this conversation")}</MenuItem>
              {onPrint && (
                <MenuItem
                  icon={PrinterIcon}
                  onClick={() => {
                    close();
                    onPrint();
                  }}
                >
                  {tx("Print")}</MenuItem>
              )}
              <MenuItem
                icon={SourceCodeIcon}
                onClick={() => {
                  close();
                  setOriginal(last);
                }}
              >
                {tx("Show original")}</MenuItem>
              {ai?.summaries && (
                <MenuItem
                  icon={AiMagicIcon}
                  disabled={aiBusy}
                  onClick={() => {
                    close();
                    void summarize();
                  }}
                >
                  {tx("Summarize")}</MenuItem>
              )}
              {ai?.summaries && (
                <MenuItem
                  icon={InformationCircleIcon}
                  disabled={aiBusy}
                  onClick={() => {
                    close();
                    void explain();
                  }}
                >
                  {tx("Explain this email")}</MenuItem>
              )}
              {ai?.translate && (
                <MenuItem
                  icon={LanguageSquareIcon}
                  disabled={aiBusy}
                  onClick={() => {
                    close();
                    void translate();
                  }}
                >
                  {tx("Translate into")}{" "}{ai.language}
                </MenuItem>
              )}
            </>
          )}
        </Menu>
        <span className="spacer" />
        <IconButton icon={MailReply01Icon} label={tx("Reply")} shortcut="⌘R" onClick={() => onCompose("reply", last)} />
        <IconButton icon={MailReplyAll01Icon} label={tx("Reply all")} onClick={() => onCompose("reply_all", last)} />
        <IconButton icon={ArrowTurnForwardIcon} label={tx("Forward")} onClick={() => onCompose("forward", last)} />
      </div>
      {finding && (
        <div className="reader-find">
          <Icon icon={Search01Icon} size="s" />
          <input
            autoFocus
            aria-label={tx("Find in this conversation")}
            placeholder={tx("Find in this conversation")}
            value={find}
            onChange={(e) => setFind(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                setFinding(false);
                setFind("");
              }
            }}
          />
          {wanted && (
            <span role="status" className="muted tnum">
              {matches} of {messages.length}
            </span>
          )}
          <IconButton
            icon={Cancel01Icon}
            label={tx("Close find")}
            small
            onClick={() => {
              setFinding(false);
              setFind("");
            }}
          />
        </div>
      )}
      <div className="reader-body">
        {onBackToAnswer && (
          <button type="button" className="btn btn-ghost btn-sm back-to-answer" onClick={onBackToAnswer}>
            <Icon icon={ArrowLeft01Icon} size="s" />
            {tx("Back to the answer")}
          </button>
        )}
        {accountEmail && (
          <button
            type="button"
            className="inbox-crumb"
            data-account={accountEmail}
            data-tip={tx("Go to this inbox")}
            onClick={onGoToInbox}
          >
            <Avatar name={accountEmail} size="xs" />
            <span className="truncate-1">{accountEmail}</span>
          </button>
        )}
        {/* The subject, with the few things you can do to the whole conversation at its end. */}
        <div className="reader-head">
          <h1 className="reader-subject selectable">{subject || tx("(no subject)")}</h1>
          <div className="reader-head-actions">
            {code && onCopyCode && <CodeChip code={code} onCopied={onCopyCode} />}
            {!canFollowUp && marks && (
              <Menu
                label={tx("Remind me to reply")}
                align="end"
                trigger={(props) => (
                  <button
                    {...props}
                    type="button"
                    className="btn btn-ghost btn-sm"
                    data-on={marks.reminding || undefined}
                    data-tip={marks.reminding ? tx("Reminder set") : tx("Bring this back so you remember to reply")}
                  >
                    <Icon icon={AlarmClockIcon} size="s" />
                    {marks.reminding ? tx("Reminder set") : tx("Remind me")}
                  </button>
                )}
              >
                {(close) =>
                  marks.reminding ? (
                    <MenuItem
                      icon={AlarmClockIcon}
                      onClick={() => {
                        close();
                        marks.onFollowUp(null);
                      }}
                    >
                      {tx("Cancel reminder")}
                    </MenuItem>
                  ) : (
                    <WhenItems close={close} onPick={marks.onFollowUp} onOther={() => setPicking("follow")} />
                  )
                }
              </Menu>
            )}
            {canFollowUp && followUp?.state !== "running" && (
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                data-tip={tx("Send follow-ups until they reply")}
                onClick={() => setStartingFollowUps(true)}
              >
                <Icon icon={SentIcon} size="s" />
                {tx("Follow up")}
              </button>
            )}
            {ai?.summaries && (!summary || !explained) && (
              <Menu
                label={tx("Summarize or explain")}
                align="end"
                trigger={(props) => (
                  <button
                    {...props}
                    type="button"
                    className="btn btn-ghost btn-sm"
                    disabled={aiBusy}
                    data-tip={tx("Summarize or explain")}
                  >
                    {aiBusy ? <Spinner size="xs" /> : <Icon icon={AiMagicIcon} size="s" />}
                    {tx("AI")}
                  </button>
                )}
              >
                {(close) => (
                  <>
                    {!summary && (
                      <MenuItem
                        icon={AiMagicIcon}
                        onClick={() => {
                          close();
                          void summarize();
                        }}
                      >
                        {tx("Summarize")}</MenuItem>
                    )}
                    {!explained && (
                      <MenuItem
                        icon={InformationCircleIcon}
                        onClick={() => {
                          close();
                          void explain();
                        }}
                      >
                        {tx("Explain this email")}</MenuItem>
                    )}
                    {onAskAbout && (
                      <MenuItem
                        icon={AiChat02Icon}
                        onClick={() => {
                          close();
                          onAskAbout();
                        }}
                      >
                        {tx("Ask about this conversation")}</MenuItem>
                    )}
                  </>
                )}
              </Menu>
            )}
          </div>
        </div>
        {/* Who has it sits right under the subject. */}
        {team && teamView && <TeamBar thread={team.thread} view={teamView} onChanged={teamChanged} />}
        {tags && tags.length > 0 && (
          <button type="button" className="reader-tags" data-tip={tx("Change tags")} onClick={onTags}>
            <TagPills tags={tags} />
          </button>
        )}
        {followUp && <FollowUpBar run={followUp} onChanged={followUps.reload} />}
        {aiBusy && (
          <div role="status" className="ai-card muted">
            <Icon icon={AiMagicIcon} size="s" />
            {tx("Reading…")}</div>
        )}
        {aiProblem && (
          <div className="ai-card">
            <Problem text={aiProblem} />
          </div>
        )}
        {summary && (
          <section aria-label={tx("Summary")} className="summary-card">
            <div className="summary-head">
              <Icon icon={AiMagicIcon} size="s" />
              <span className="summary-label">{tx("Summary")}</span>
              {summary.needs_reply && <span className="pill pill-later">{tx("Waiting on you")}</span>}
              <span className="grow" />
              <IconButton icon={Cancel01Icon} label={tx("Close summary")} small onClick={() => setSummary(null)} />
            </div>
            <p className="summary-text selectable">{summary.summary}</p>
            {summary.points.length > 0 && (
              <ul className="summary-points selectable">
                {summary.points.map((x) => (
                  <li key={x}>{x}</li>
                ))}
              </ul>
            )}
            {summary.next && (
              <div className="summary-next">
                <Icon icon={ArrowRight01Icon} size="s" />
                <span className="selectable">{summary.next}</span>
              </div>
            )}
          </section>
        )}
        {explained && (
          <section aria-label={tx("What this email means")} className="summary-card">
            <div className="summary-head">
              <Icon icon={InformationCircleIcon} size="s" />
              <span className="summary-label">{tx("What this means")}</span>
              {explained.tone && <span className="pill">{explained.tone}</span>}
              <span className="grow" />
              <IconButton icon={Cancel01Icon} label={tx("Close explanation")} small onClick={() => setExplained(null)} />
            </div>
            <p className="summary-text selectable">{explained.meaning}</p>
            {explained.wants && (
              <div className="summary-next">
                <Icon icon={ArrowRight01Icon} size="s" />
                <span className="selectable">{explained.wants}</span>
              </div>
            )}
            {explained.watch_out && (
              <div className="summary-next explain-watch">
                <Icon icon={AlertCircleIcon} size="s" />
                <span className="selectable">{explained.watch_out}</span>
              </div>
            )}
          </section>
        )}
        <div className="reader-gap" />
        {messages.map((m, i) => {
          if (folding && i > 0 && i < keepFrom) {
            if (i !== 1) return null;
            const hidden = keepFrom - 1;
            return (
              <button key="fold" type="button" className="card-fold" onClick={() => setAllShown(conversation)}>
                <span className="card-fold-count">{hidden}</span>
                {hidden === 1 ? "earlier email" : "earlier emails"}
              </button>
            );
          }
          return (
          <div key={m.id} ref={i === focusAt ? focusCard : undefined} className="card-slot">
          <MessageCard
            key={m.id}
            message={m}
            startOpen={i === messages.length - 1 || !m.seen}
            showPicture={folder?.role !== "junk"}
            keyFacts={needsYou && i === messages.length - 1}
            trustedSender={folder?.role !== "junk" && trusted.includes(m.from.address.toLowerCase())}
            onTrustSender={
              folder?.role !== "junk" && m.from.address
                ? (allowed) => trustSender(m.from.address, allowed)
                : undefined
            }
            onCompose={onCompose}
            inJunk={folder?.role === "junk"}
            onSpam={onSpam}
            onNotSpam={onNotSpam}
            onUnsubscribe={onUnsubscribe}
            onShowOriginal={setOriginal}
            find={wanted}
            translated={translated?.id === m.id ? translated.text : undefined}
          />
          </div>
          );
        })}
        {team && teamView && <TeamNotes thread={team.thread} view={teamView} onChanged={teamChanged} />}
        {whoIsReplying(teamView) && (
          <div role="status" className="team-replying">
            <span className="team-dot" />
            {whoIsReplying(teamView)}
          </div>
        )}
        {!replyBox && needsYou && ai && onReplyWith && !AUTOMATIC_SENDER.test(last.from.address) && (
          <ReplyHelp key={last.id} message={last} onUse={(start) => onReplyWith(last, start)} />
        )}
        {replyBox}
      </div>
      {!replyBox && (
        <div className="quick-reply-row">
          <button
            className="btn btn-primary quick-reply"
            aria-busy={replyStarting || undefined}
            onClick={() => onCompose(replyMode, last)}
          >
            {replyStarting ? <Spinner size="s" /> : <Icon icon={MailReply01Icon} size="s" />}
            {replyMode === "reply_all" ? tx("Reply all") : "Reply"}
            <span className="kbd">⌘R</span>
          </button>
        </div>
      )}
      {original && <ShowOriginal message={original} onClose={() => setOriginal(null)} />}
      {startingFollowUps && (
        <StartFollowUps messageId={last.id} onClose={() => setStartingFollowUps(false)} onStarted={followUps.reload} />
      )}
      {showShared && (
        <SharedPanel accountId={last.account_id} threadId={last.thread_id} onClose={() => setShowShared(false)} />
      )}
      {picking && marks && (
        <PickTime
          title={picking === "snooze" ? tx("Snooze until") : canFollowUp ? tx("Remind me if there's no reply by") : tx("Remind me to reply on")}
          confirm={picking === "snooze" ? tx("Snooze") : tx("Remind me")}
          onClose={() => setPicking(null)}
          onPick={(at) => {
            setPicking(null);
            if (picking === "snooze") marks.onSnooze(at);
            else marks.onFollowUp(at);
          }}
        />
      )}
      {askingSpam && (
        <Confirm
          title={tx("Mark as spam?")}
          confirm={tx("Yes, mark as spam")}
          keep={tx("Cancel")}
          danger
          onKeep={() => setAskingSpam(false)}
          onConfirm={() => {
            setAskingSpam(false);
            onSpam();
          }}
        >
          {tx("This conversation moves to Spam, and Mailzo learns to catch emails like it. You can bring it back from Spam with Not spam.")}</Confirm>
      )}
      {confirming && (
        <Confirm
          title={tx("Delete for good?")}
          confirm={tx("Yes, delete for good")}
          keep={tx("Keep")}
          danger
          onKeep={() => setConfirming(false)}
          onConfirm={() => {
            setConfirming(false);
            onDeleteForever();
          }}
        >
          {tx("This conversation will be removed from this folder for good. This can't be undone.")}</Confirm>
      )}
    </div>
  );
}

/** Every file and link shared in this conversation, newest first. */
function SharedPanel({ accountId, threadId, onClose }: { accountId: number; threadId: string; onClose: () => void }) {
  const [shared, setShared] = useState<SharedStuff | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    api
      .threadShared(accountId, threadId)
      .then((s) => live && setShared(s))
      .catch((e) => live && setProblem(say(e)));
    return () => {
      live = false;
    };
  }, [accountId, threadId]);
  const day = (unix: number) => new Date(unix * 1000).toLocaleDateString(language, { month: "short", day: "numeric" });
  return (
    <Dialog title={tx("Files and links")} subtitle={tx("Everything shared in this conversation")} width="m" onClose={onClose}>
      <Problem text={problem} />
      {!shared && !problem && (
        <div role="status" className="muted hstack">
          <Spinner size="s" /> {" "}{tx("Looking through the conversation…")}</div>
      )}
      {shared && shared.files.length === 0 && shared.links.length === 0 && (
        <p className="muted">{tx("No files or links in this conversation.")}</p>
      )}
      {shared && shared.files.length > 0 && (
        <section aria-label={tx("Files")} className="shared-section">
          <h3 className="shared-head">{tx("Files")}</h3>
          <ul className="files shared-files">
            {shared.files.map((f) => (
              <AttachmentTile
                key={`${f.message_id}:${f.part}`}
                messageId={f.message_id}
                attachment={{ name: f.name, mime: f.mime, size: f.size, part: f.part, inline: false }}
                index={f.part}
              />
            ))}
          </ul>
        </section>
      )}
      {shared && shared.links.length > 0 && (
        <section aria-label={tx("Links")} className="shared-section">
          <h3 className="shared-head">{tx("Links")}</h3>
          <ul className="shared-links">
            {shared.links.map((l) => (
              <li key={l.url}>
                <a className="shared-link" href={l.url} target="_blank" rel="noreferrer noopener" data-tip={l.url}>
                  <Icon icon={Link01Icon} size="s" />
                  <span className="grow">
                    <span className="shared-link-text truncate-1">{l.text || l.host}</span>
                    <span className="file-meta truncate-1">
                      {l.host} · {l.from} · {day(l.date)}
                    </span>
                  </span>
                  {l.risky && <span className="pill pill-risk">{tx("Be careful")}</span>}
                </a>
              </li>
            ))}
          </ul>
        </section>
      )}
    </Dialog>
  );
}
