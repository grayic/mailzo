import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { QUOTE_INTRO, fitEmail, prepareEmail, routeImagesThrough } from "../prepareEmail";
import { useSettings } from "../settings";
import { tx } from "../i18n";

if ("__TAURI_INTERNALS__" in window) routeImagesThrough((address) => convertFileSrc(address, "inboximage"));

const FRAME_START_HEIGHT = 120;
const FRAME_CHECK_MS = 16;
const FRAME_CHECKS = 300;
/** How tall each email turned out, so opening it again doesn't jump. */
const fittedHeights = new Map<string, number>();

export function MailBody({
  html,
  text,
  showImages = false,
  hideQuoted = false,
  highlight = "",
  keyFacts = false,
}: {
  keyFacts?: boolean;
  html: string | null;
  text: string | null;
  showImages?: boolean;
  hideQuoted?: boolean;
  /** Words to mark, when searching within a conversation. */
  highlight?: string;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(FRAME_START_HEIGHT);
  const [fitted, setFitted] = useState(false);
  const observer = useRef<ResizeObserver | null>(null);
  const onLoaded = useRef<() => void>(() => {});
  const { settings } = useSettings();
  const darkApp = useDarkApp(settings.theme);
  const prepared = useMemo(
    () => (html ? prepareEmail(html, { showImages, hideQuoted, highlight, keyFacts, dark: darkApp }) : null),
    // The text size is read inside prepareEmail, and it has to redo the email when it changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [html, showImages, hideQuoted, highlight, settings.reading_size, darkApp],
  );
  const doc = prepared?.doc ?? null;

  // "Loaded" only comes once every picture has, which can take seconds. So this fits the frame
  // as soon as the words are in, and shows nothing until then rather than a cut-off email.
  useEffect(() => {
    if (!doc) return;
    const known = fittedHeights.get(doc);
    setHeight(known ?? FRAME_START_HEIGHT);
    setFitted(known !== undefined);
    let tries = 0;
    let watched: HTMLElement | null = null;
    const watch = (finished: boolean) => {
      const page = frame.current?.contentDocument;
      const body = page?.body;
      const parsed = !!body && page?.URL !== "about:blank" && (finished || page?.readyState !== "loading" || !!body.firstChild);
      if (!parsed || body === watched) return;
      watched = body;
      const fit = () => {
        const shown = frame.current;
        if (!shown) return;
        const tall = fitEmail(shown);
        setHeight(tall);
        fittedHeights.set(doc, tall);
        setFitted(true);
      };
      fit();
      // Keeps it right as pictures arrive and when the window changes width.
      observer.current?.disconnect();
      observer.current = new ResizeObserver(fit);
      observer.current.observe(body);
    };
    onLoaded.current = () => watch(true);
    const timer = setInterval(() => {
      tries += 1;
      watch(false);
      if (watched || tries > FRAME_CHECKS) clearInterval(timer);
    }, FRAME_CHECK_MS);
    return () => {
      clearInterval(timer);
      observer.current?.disconnect();
    };
  }, [doc]);

  if (!doc) {
    const shown = hideQuoted ? withoutQuotedText(text ?? "") : text;
    return (
      <pre className="selectable mail-text">
        {shown ? <Linked text={shown} words={highlight} /> : tx("(This message has no text.)")}
      </pre>
    );
  }

  // No allow-scripts: nothing in the email can run. allow-popups is what lets a
  // clicked link reach the app, which sends it to the real browser. Handlers
  // inside this frame would not work, because Safari's engine blocks them.
  return (
    <iframe
      ref={frame}
      title={tx("Email content")}
      sandbox="allow-same-origin allow-popups"
      srcDoc={doc}
      onLoad={() => {
        onLoaded.current();
        setFitted(true);
      }}
      className={`mail-frame ${prepared?.dark ? "mail-frame-dark" : ""} ${fitted ? "" : "mail-frame-waiting"}`}
      style={{ height }}
    />
  );
}

/** Whether the app is showing its dark colours now, from the Appearance choice or the Mac's. */
function useDarkApp(theme: string | undefined): boolean {
  const query = tx("(prefers-color-scheme: dark)");
  const [systemDark, setSystemDark] = useState(() => window.matchMedia?.(query).matches ?? false);
  useEffect(() => {
    const list = window.matchMedia?.(query);
    if (!list) return;
    const changed = () => setSystemDark(list.matches);
    list.addEventListener("change", changed);
    return () => list.removeEventListener("change", changed);
  }, []);
  return theme === "dark" || (theme !== "light" && systemDark);
}

/** Where web addresses sit in plain text. A full stop, comma or closing bracket right after
 * one ends the sentence, not the address. */
export function webAddresses(text: string): { at: number; url: string }[] {
  const found: { at: number; url: string }[] = [];
  for (const m of text.matchAll(/https?:\/\/[^\s<>"]+/g)) {
    let url = m[0];
    const opens = (c: string) => url.split(c).length - 1;
    while (/[.,;:!?'\]}>]$/.test(url) || (url.endsWith(")") && opens("(") < opens(")"))) {
      url = url.slice(0, -1);
    }
    if (url.length > "https://".length) found.push({ at: m.index ?? 0, url });
  }
  return found;
}

/** Plain-text mail writes links out, so they're made into links that open in the browser. */
function Linked({ text, words }: { text: string; words: string }) {
  const parts: React.ReactNode[] = [];
  let at = 0;
  for (const { at: start, url } of webAddresses(text)) {
    parts.push(
      <Marked key={`t${start}`} text={text.slice(at, start)} words={words} />,
      <a key={start} className="mail-text-link" href={url} target="_blank" rel="noreferrer noopener">
        <Marked text={url} words={words} />
      </a>,
    );
    at = start + url.length;
  }
  parts.push(<Marked key="end" text={text.slice(at)} words={words} />);
  return <>{parts}</>;
}

function Marked({ text, words }: { text: string; words: string }) {
  const wanted = words.trim().toLowerCase();
  if (!wanted) return <>{text}</>;
  const parts: React.ReactNode[] = [];
  const lower = text.toLowerCase();
  let at = 0;
  for (let i = lower.indexOf(wanted); i >= 0; i = lower.indexOf(wanted, at)) {
    parts.push(text.slice(at, i), <mark key={i}>{text.slice(i, i + wanted.length)}</mark>);
    at = i + wanted.length;
  }
  parts.push(text.slice(at));
  return <>{parts}</>;
}

/** Everything before the quoted earlier message in a plain-text email. */
export function withoutQuotedText(text: string): string {
  const lines = text.split("\n");
  const start = lines.findIndex(
    (line, i) =>
      QUOTE_INTRO.test(line.trim()) ||
      (line.startsWith(">") &&
        (lines[i + 1] === undefined || lines[i + 1].startsWith(">") || !lines[i + 1].trim())),
  );
  return start > 0 ? lines.slice(0, start).join("\n").trimEnd() : text;
}

export function textHasQuote(text: string | null): boolean {
  return !!text && withoutQuotedText(text) !== text;
}
