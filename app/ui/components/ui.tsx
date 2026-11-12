import appIcon from "../app-icon.svg";
import googleG from "../google-g.svg";
import gmailLogo from "../gmail.svg";
import microsoftLogo from "../microsoft.svg";
import appleLogo from "../apple.svg";

/** Each provider's own logo, for the screen that connects it. */
const PROVIDER_LOGOS: Record<string, string> = { google: gmailLogo, microsoft: microsoftLogo, icloud: appleLogo };
import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import {
  ComponentProps,
  ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { api } from "../api";
import { AlertCircleIcon, Cancel01Icon, Loading03Icon, SentIcon, ViewIcon, ViewOffSlashIcon } from "../icons";
import { tx } from "../i18n";

export type IconSize = "xs" | "s" | "m" | "l";

/** One icon, always the same weight, in one of the token sizes. */
/** Google asks for its own colours, so this one doesn't follow the theme. */
export function GoogleMark() {
  return <img src={googleG} alt="" className="google-mark" />;
}

export function Icon({
  icon,
  size = "m",
  className,
}: {
  icon: IconSvgElement;
  size?: IconSize;
  className?: string;
}) {
  return (
    <HugeiconsIcon
      icon={icon}
      strokeWidth={1.6}
      className={`icon icon-${size} ${className ?? ""}`}
      aria-hidden
    />
  );
}

export function Spinner({ size = "s" }: { size?: IconSize }) {
  return <Icon icon={Loading03Icon} size={size} className="spin" />;
}

/** How far the mail download has got. With no total yet, it just moves. */
export function DownloadBar({ done, total }: { done: number; total: number | null }) {
  const known = total !== null && total > 0;
  return (
    <div
      className="download-bar"
      role="progressbar"
      aria-label={tx("Downloading emails")}
      aria-valuenow={known ? Math.min(done, total) : undefined}
      aria-valuemax={known ? total : undefined}
      data-unknown={known ? undefined : ""}
    >
      <span style={known ? ({ "--download-done": `${Math.max(4, Math.min(100, (done / total) * 100))}%` } as React.CSSProperties) : undefined} />
    </div>
  );
}

export function Logo({ provider }: { provider?: string } = {}) {
  const own = provider ? PROVIDER_LOGOS[provider] : undefined;
  if (own) {
    return (
      <span className="welcome-logo provider-logo" data-provider={provider}>
        <img src={own} alt="" />
      </span>
    );
  }
  return <img src={appIcon} width="40" height="40" alt="" className="welcome-logo" />;
}

/** The app's icon, small enough to sit beside words on a button. */
export function LogoMark() {
  return <img src={appIcon} alt="" className="logo-mark" />;
}

const hue = (seed: string) => {
  let h = 0;
  for (const c of seed) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
};

export const initials = (name: string) => {
  const clean = name.replace(/<.*>/, "").replace(/["']/g, "").trim();
  const words = clean.split(/[\s._@-]+/).filter(Boolean);
  if (words.length === 0) return "?";
  if (clean.includes("@") || words.length === 1) return words[0].slice(0, 1).toUpperCase();
  return (words[0][0] + words[words.length - 1][0]).toUpperCase();
};

type Picture = { picture: string | null; kind?: "person" | "logo" };

// One answer per address for as long as the app is open.
const pictures = new Map<string, Promise<Picture>>();
const pictureListeners = new Set<() => void>();

/** Forgets every picture, after the setting changes. */
export function resetPictures() {
  pictures.clear();
  pictureListeners.forEach((listener) => listener());
}

function usePicture(email: string | undefined): Picture | null {
  const [found, setFound] = useState<Picture | null>(null);
  const [round, setRound] = useState(0);

  useEffect(() => {
    const again = () => setRound((n) => n + 1);
    pictureListeners.add(again);
    return () => {
      pictureListeners.delete(again);
    };
  }, []);

  useEffect(() => {
    setFound(null);
    if (!email) return;
    const key = email.toLowerCase();
    let request = pictures.get(key);
    if (!request) {
      request = api.avatar(key).catch(() => ({ picture: null }));
      pictures.set(key, request);
    }
    let live = true;
    void request.then((answer) => {
      if (live) setFound(answer);
    });
    return () => {
      live = false;
    };
  }, [email, round]);

  return found;
}

/** A sender's picture when there is one, otherwise their initials on a soft colour
 * that stays the same for that person. The initials sit underneath, so nothing
 * jumps when a picture arrives. */
export function Avatar({
  name,
  email,
  size = "m",
  hue: chosenHue,
}: {
  name: string;
  /** Leave out to always show initials, such as for mail in the spam folder. */
  email?: string;
  size?: "xs" | "s" | "m";
  /** A colour the person chose, for their own inboxes. */
  hue?: number;
}) {
  const found = usePicture(email);
  return (
    <span
      aria-hidden
      className={`avatar avatar-${size}`}
      style={{ "--h": chosenHue ?? hue(name.toLowerCase()) } as React.CSSProperties}
    >
      {initials(name)}
      {found?.picture && (
        <img
          alt=""
          src={found.picture}
          className={`avatar-picture avatar-picture-${found.kind ?? "person"}`}
          onError={(e) => e.currentTarget.remove()}
        />
      )}
    </span>
  );
}

/** An inbox's colour: the one chosen, or one that stays the same for its address. */
export function accountHue(account: { email: string; prefs: { color: number | null } }): number {
  return account.prefs.color ?? hue(account.email.toLowerCase());
}

/** A small mark in the colour of the inbox something belongs to. */
export function AccountDot({ email, hue: chosenHue }: { email: string; hue?: number }) {
  return (
    <span
      className="account-dot"
      data-account={email}
      style={{ "--h": chosenHue ?? hue(email.toLowerCase()) } as React.CSSProperties}
    />
  );
}

/** Opens a page of Settings; the main window sets it. Problems then link "Settings › AI". */
export const openSettings: { at: ((page: string) => void) | null } = { at: null };

/** "Settings › AI" in a problem, as a link to that page. */
function linked(text: string): ReactNode {
  const words = `${tx("Settings")} › ${tx("AI")}`;
  const i = text.indexOf(words);
  const open = openSettings.at;
  if (i < 0 || !open) return text;
  return (
    <>
      {text.slice(0, i)}
      <button type="button" className="link-btn" onClick={() => open("ai")}>
        {words}
      </button>
      {text.slice(i + words.length)}
    </>
  );
}

export function Problem({ text, children }: { text?: string | null; children?: ReactNode }) {
  if (!text && !children) return null;
  return (
    <div role="alert" className="problem">
      <Icon icon={AlertCircleIcon} size="s" />
      <div className="grow">
        {text && linked(text)} {children}
      </div>
    </div>
  );
}

export function IconButton({
  icon,
  label,
  tip,
  shortcut,
  onClick,
  disabled,
  pressed,
  small,
  className,
  iconClassName,
  spinning,
}: {
  icon: IconSvgElement;
  label: string;
  /** Says more than the name, when there's more worth saying. */
  tip?: string;
  shortcut?: string;
  onClick?: () => void;
  disabled?: boolean;
  pressed?: boolean;
  small?: boolean;
  className?: string;
  iconClassName?: string;
  spinning?: boolean;
}) {
  return (
    <button
      type="button"
      className={`icon-btn ${small ? "icon-btn-sm" : ""} ${className ?? ""}`}
      aria-label={label}
      aria-pressed={pressed}
      data-tip={tip ?? label}
      data-tip-key={shortcut}
      disabled={disabled}
      onClick={onClick}
    >
      {/* A round icon, as a lopsided one seems to wobble as it turns. */}
      <Icon
        icon={spinning ? Loading03Icon : icon}
        size={small ? "s" : "m"}
        className={`${spinning ? "spin" : ""} ${iconClassName ?? ""}`}
      />
    </button>
  );
}

/** A password box with a button to check what's been typed. Always starts hidden. */
export function PasswordInput(props: Omit<ComponentProps<"input">, "type">) {
  const [shown, setShown] = useState(false);
  return (
    <span
      className="password-field"
      // Clicking the button leaves the cursor in the box, so typing can carry on.
      onMouseDown={(e) => {
        if ((e.target as Element).closest("button")) e.preventDefault();
      }}
    >
      <input {...props} type={shown ? "text" : "password"} spellCheck={false} autoCapitalize="off" autoCorrect="off" />
      <IconButton
        icon={shown ? ViewOffSlashIcon : ViewIcon}
        label={shown ? tx("Hide password") : tx("Show password")}
        small
        onClick={() => setShown(!shown)}
      />
    </span>
  );
}

/** Draws one tooltip for the whole app, so scrolling panels can't cut it off. */
export function Tooltips() {
  const [tip, setTip] = useState<{ text: string; key?: string; x: number; y: number; below: boolean; right: boolean } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const shownRecently = useRef(0);
  const watching = useRef<MutationObserver | null>(null);

  useEffect(() => {
    const hide = () => {
      if (timer.current) clearTimeout(timer.current);
      watching.current?.disconnect();
      setTip((t) => {
        if (t) shownRecently.current = Date.now();
        return null;
      });
    };
    const over = (e: MouseEvent) => {
      const el = (e.target as Element | null)?.closest?.("[data-tip]") as HTMLElement | null;
      if (timer.current) clearTimeout(timer.current);
      if (!el) {
        hide();
        return;
      }
      // A rail of icons has no words of its own, so its names show at once, beside the icon.
      const right = !!el.closest(".app[data-rail] .sidebar");
      // Moving along a toolbar shows the next one straight away.
      const delay = right || el.dataset.tipNow !== undefined || Date.now() - shownRecently.current < 500 ? 0 : 420;
      timer.current = setTimeout(() => {
        const r = el.getBoundingClientRect();
        const below = r.top < 44 || r.bottom < window.innerHeight - 60;
        setTip({
          text: el.dataset.tip ?? "",
          key: el.dataset.tipKey,
          x: right ? r.right + 6 : r.left + r.width / 2,
          y: right ? r.top + r.height / 2 : below ? r.bottom + 6 : r.top - 6,
          below,
          right,
        });
        // Words that change while they're being read, such as how much is left, stay right.
        watching.current?.disconnect();
        watching.current = new MutationObserver(() => setTip((t) => t && { ...t, text: el.dataset.tip ?? "" }));
        watching.current.observe(el, { attributes: true, attributeFilter: ["data-tip"] });
      }, delay);
    };
    document.addEventListener("mouseover", over);
    document.addEventListener("mousedown", hide, true);
    document.addEventListener("keydown", hide);
    window.addEventListener("blur", hide);
    return () => {
      document.removeEventListener("mouseover", over);
      document.removeEventListener("mousedown", hide, true);
      document.removeEventListener("keydown", hide);
      window.removeEventListener("blur", hide);
    };
  }, []);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !tip) return;
    const w = el.offsetWidth;
    if (tip.right) {
      el.style.left = `${tip.x}px`;
      el.style.top = `${tip.y - el.offsetHeight / 2}px`;
      return;
    }
    const left = Math.min(Math.max(8, tip.x - w / 2), window.innerWidth - w - 8);
    el.style.left = `${left}px`;
    el.style.top = tip.below ? `${tip.y}px` : `${tip.y - el.offsetHeight}px`;
  }, [tip]);

  if (!tip) return null;
  return createPortal(
    <div ref={box} className={`tooltip ${tip.right ? "tooltip-now" : ""}`} role="tooltip">
      {tip.text}
      {tip.key && <span className="kbd">{tip.key}</span>}
    </div>,
    document.body,
  );
}

/** A small list that opens from a button. Closes on a click away or Escape. */
/** Closes a menu on a click away or Esc, and moves through it with the arrow keys. */
function useMenuKeys(
  open: boolean,
  panel: React.RefObject<HTMLDivElement | null>,
  close: () => void,
  button?: React.RefObject<HTMLButtonElement | null>,
) {
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!panel.current?.contains(target) && !button?.current?.contains(target)) close();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
        button?.current?.focus();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const items = [...(panel.current?.querySelectorAll<HTMLElement>("button") ?? [])];
        const at = items.indexOf(document.activeElement as HTMLElement);
        const next = e.key === "ArrowDown" ? at + 1 : at - 1;
        items[(next + items.length) % items.length]?.focus();
      }
    };
    // Heard on the way down, as a window drag area stops a press from going any further.
    document.addEventListener("mousedown", away, true);
    document.addEventListener("keydown", key, true);
    // A click in an email lands in its own frame, so here it only looks like losing focus.
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("mousedown", away, true);
      document.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", close);
    };
  }, [open, close, panel, button]);
}

export function Menu({
  label,
  trigger,
  children,
  align = "start",
  placement = "below",
  fullWidth = false,
}: {
  /** As wide as the button that opens it, for menus in the sidebar. */
  fullWidth?: boolean;
  label: string;
  trigger: (props: {
    "aria-haspopup": "menu";
    "aria-expanded": boolean;
    onClick: () => void;
    ref: React.RefObject<HTMLButtonElement | null>;
  }) => ReactNode;
  children: (close: () => void) => ReactNode;
  align?: "start" | "end";
  placement?: "below" | "above";
}) {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const close = useCallback(() => setOpen(false), []);

  useLayoutEffect(() => {
    const el = panel.current;
    const anchor = button.current;
    if (!open || !el || !anchor) return;
    const r = anchor.getBoundingClientRect();
    if (fullWidth) {
      el.style.minWidth = "0";
      el.style.width = `${r.width}px`;
    }
    const left = align === "end" ? r.right - el.offsetWidth : r.left;
    el.style.left = `${Math.min(Math.max(8, left), window.innerWidth - el.offsetWidth - 8)}px`;
    const room = placement === "above" ? r.top - el.offsetHeight - 6 : r.bottom + 6;
    const top =
      room + el.offsetHeight > window.innerHeight - 8
        ? r.top - el.offsetHeight - 6
        : room < 8
          ? r.bottom + 6
          : room;
    el.style.top = `${top}px`;
    el.querySelector<HTMLElement>("button")?.focus();
  }, [open, align, placement, fullWidth]);

  useMenuKeys(open, panel, close, button);

  return (
    <>
      {trigger({
        "aria-haspopup": "menu",
        "aria-expanded": open,
        onClick: () => setOpen((v) => !v),
        ref: button,
      })}
      {open &&
        createPortal(
          <div ref={panel} className="menu" role="menu" aria-label={label}>
            {children(close)}
          </div>,
          document.body,
        )}
    </>
  );
}

/** A menu that opens where the mouse is, from a right-click. */
export function ContextMenu({
  x,
  y,
  label,
  page,
  onClose,
  children,
}: {
  x: number;
  y: number;
  label: string;
  /** Changes when the menu shows something else, so it's fitted on screen again. */
  page?: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = panel.current;
    if (!el) return;
    el.style.left = `${Math.max(8, Math.min(x, window.innerWidth - el.offsetWidth - 8))}px`;
    el.style.top = `${Math.max(8, Math.min(y, window.innerHeight - el.offsetHeight - 8))}px`;
  }, [x, y, page]);

  useMenuKeys(true, panel, onClose);

  return createPortal(
    <div
      ref={panel}
      className="menu"
      role="menu"
      aria-label={label}
      onContextMenu={(e) => e.preventDefault()}
    >
      {children}
    </div>,
    document.body,
  );
}

export function MenuItem({
  icon,
  leading,
  trailing,
  children,
  onClick,
  danger,
  disabled,
}: {
  icon?: IconSvgElement;
  leading?: ReactNode;
  trailing?: ReactNode;
  children: ReactNode;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      className={`menu-item ${danger ? "menu-item-danger" : ""}`}
      disabled={disabled}
      onClick={onClick}
    >
      {icon && <Icon icon={icon} size="s" />}
      {leading}
      <span className="truncate-1 grow">{children}</span>
      {trailing}
    </button>
  );
}

export function Dialog({
  label,
  title,
  subtitle,
  width = "m",
  className = "",
  onClose,
  children,
  footer,
  onSubmit,
}: {
  className?: string;
  label?: string;
  title: ReactNode;
  subtitle?: ReactNode;
  width?: "s" | "m" | "l";
  onClose?: () => void;
  children: ReactNode;
  footer?: ReactNode;
  onSubmit?: (e: React.FormEvent) => void;
}) {
  useEffect(() => {
    if (!onClose) return;
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, [onClose]);

  const inner = (
    <>
      <div className="dialog-head">
        <div className="grow">
          <h2 className="dialog-title">{title}</h2>
          {subtitle && <div className="dialog-sub truncate-1">{subtitle}</div>}
        </div>
        {onClose && <IconButton icon={Cancel01Icon} label={tx("Close")} small onClick={onClose} />}
      </div>
      <div className="dialog-body">{children}</div>
      {footer && <div className="dialog-foot">{footer}</div>}
    </>
  );

  const common = {
    role: "dialog",
    "aria-modal": true,
    "aria-label": label ?? (typeof title === "string" ? title : undefined),
    className: `dialog dialog-${width} ${className}`,
  } as const;

  return createPortal(
    <div className="scrim">
      {onSubmit ? (
        <form {...common} onSubmit={onSubmit}>
          {inner}
        </form>
      ) : (
        <div {...common}>{inner}</div>
      )}
    </div>,
    document.body,
  );
}

/** Asks before something that can't easily be undone. */
export function Confirm({
  title,
  children,
  confirm,
  keep,
  danger,
  onConfirm,
  onKeep,
}: {
  title: string;
  children: ReactNode;
  confirm: string;
  keep: string;
  danger?: boolean;
  onConfirm: () => void;
  onKeep: () => void;
}) {
  return (
    <Dialog
      title={title}
      width="s"
      onClose={onKeep}
      footer={
        <>
          <button type="button" className="btn btn-secondary" onClick={onKeep}>
            {keep}
          </button>
          <button
            type="button"
            autoFocus
            className={`btn ${danger ? "btn-danger" : "btn-primary"}`}
            onClick={onConfirm}
          >
            {confirm}
          </button>
        </>
      }
    >
      <p className="body-text">{children}</p>
    </Dialog>
  );
}

/** A sent email waiting out its undo time: big enough to catch the eye, with the time running down. */
export function SendingToast({
  from,
  sendAt,
  clock,
  onUndo,
  onSendNow,
}: {
  /** When it was sent, and when it really goes. */
  from: number;
  sendAt: number;
  clock: number;
  onUndo: () => void;
  onSendNow: () => void;
}) {
  const left = Math.max(0, sendAt - clock);
  const share = sendAt > from ? left / (sendAt - from) : 0;
  return (
    <div role="status" className="toast toast-undo toast-sending">
      <Icon icon={SentIcon} size="s" />
      <span className="grow tnum toast-sending-text">{tx("Sending in {n}s", { n: Math.max(1, Math.ceil(left / 1000)) })}</span>
      <button type="button" className="btn btn-sm toast-sending-now" onClick={onSendNow}>
        {tx("Send now")}
      </button>
      <button type="button" className="btn btn-sm toast-sending-undo" onClick={onUndo}>
        {tx("Undo")}
      </button>
      <span className="toast-sending-time" aria-hidden="true">
        <span style={{ "--sending-left": share } as React.CSSProperties} />
      </span>
    </div>
  );
}
