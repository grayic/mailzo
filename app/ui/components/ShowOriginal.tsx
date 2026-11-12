import { useEffect, useState } from "react";
import { ApiError, api, onTheWeb, type MessageSource } from "../api";
import { fullDate } from "../format";
import { Copy01Icon, Download04Icon, Tick02Icon } from "../icons";
import type { Addr, Message } from "../types";
import { Dialog, Icon, Problem, Spinner } from "./ui";
import { tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const person = (a: Addr) => (a.name ? `${a.name} <${a.address}>` : a.address);

function Check({ result }: { result: string | null }) {
  if (!result) return <span className="muted">{tx("Not checked")}</span>;
  const kind = result === "pass" ? "pass" : result === "fail" || result === "softfail" ? "fail" : "";
  return <span className={`original-check ${kind && `original-${kind}`}`}>{result.toUpperCase()}</span>;
}

/** An email exactly as it came from the server, for checking where it really came from. */
export function ShowOriginal({ message, onClose }: { message: Message; onClose: () => void }) {
  const [source, setSource] = useState<MessageSource | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let live = true;
    api
      .messageSource(message.id)
      .then((s) => live && setSource(s))
      .catch((e) => live && setProblem(say(e)));
    return () => {
      live = false;
    };
  }, [message.id]);

  const download = async () => {
    // A second click while it saves would leave a second copy in Downloads.
    if (saving) return;
    setSaving(true);
    setProblem(null);
    try {
      setSaved((await api.saveOriginal(message.id)).name);
    } catch (e) {
      setProblem(say(e));
    } finally {
      setSaving(false);
    }
  };

  const copy = async () => {
    if (!source) return;
    await navigator.clipboard.writeText(source.text);
    setCopied(true);
  };

  return (
    <Dialog
      title={tx("Original message")}
      width="l"
      onClose={onClose}
      footer={
        <>
          {saved && <span role="status" className="grow muted truncate-1">{tx("Saved to Downloads as")}{" "}{saved}</span>}
          {onTheWeb ? (
            <a className="btn btn-secondary" href={api.originalUrl(message.id)} download>
              <Icon icon={Download04Icon} size="s" />
              {tx("Download original")}</a>
          ) : (
            <button type="button" className="btn btn-secondary" disabled={saving} onClick={() => void download()}>
              {saving ? <Spinner size="xs" /> : <Icon icon={Download04Icon} size="s" />}
              {tx("Download original")}</button>
          )}
          <button type="button" className="btn btn-primary" disabled={!source} onClick={() => void copy()}>
            <Icon icon={copied ? Tick02Icon : Copy01Icon} size="s" />
            {copied ? "Copied" : tx("Copy to clipboard")}
          </button>
        </>
      }
    >
      <dl className="original-facts">
        <dt>{tx("Message ID")}</dt>
        <dd className="selectable">{source?.message_id ?? "—"}</dd>
        <dt>{tx("Date")}</dt>
        <dd className="selectable">{fullDate(message.date)}</dd>
        <dt>{tx("From")}</dt>
        <dd className="selectable">{person(message.from)}</dd>
        <dt>{tx("To")}</dt>
        <dd className="selectable">{message.to.map(person).join(", ") || "—"}</dd>
        <dt>{tx("Subject")}</dt>
        <dd className="selectable">{message.subject || tx("(no subject)")}</dd>
        {source && (
          <>
            <dt>{tx("SPF")}</dt>
            <dd><Check result={source.spf} /></dd>
            <dt>{tx("DKIM")}</dt>
            <dd><Check result={source.dkim} /></dd>
            <dt>{tx("DMARC")}</dt>
            <dd><Check result={source.dmarc} /></dd>
          </>
        )}
      </dl>
      <Problem text={problem} />
      {!source && !problem && (
        <div role="status" className="original-waiting muted">
          <Spinner size="xs" />
          {tx("Getting the original from your mail server…")}</div>
      )}
      {source?.cut_short && (
        <p className="original-note muted">{tx("This email is very large, so only the start is shown. Download it to see it all.")}</p>
      )}
      {source && <pre className="original-text selectable">{source.text}</pre>}
    </Dialog>
  );
}
