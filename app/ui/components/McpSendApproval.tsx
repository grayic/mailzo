import { useEffect, useState } from "react";
import { ApiError, api, native, type McpSendAsk } from "../api";
import { AiMagicIcon, SentIcon } from "../icons";
import { useSettings } from "../settings";
import { Avatar, Dialog, Icon, Problem, Spinner } from "./ui";
import { tx } from "../i18n";

const RETRY_MS = 5000;

/**
 * An AI app wants to send an email, and this is what lets it go. It shows the plain addresses
 * and has no close button or Enter key, so nothing goes out or gets lost by accident.
 */
/** `onSent`: an AI app's message went, so the lists can show it. */
export function McpSendApproval({ onSent }: { onSent?: () => void }) {
  const { settings } = useSettings();
  const [asked, setAsked] = useState<McpSendAsk | null>(null);
  const [busy, setBusy] = useState<"send" | "no" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const on = settings.mcp_send === "on";

  useEffect(() => {
    if (!on || asked) return;
    let stopped = false;
    const watch = async () => {
      while (!stopped) {
        try {
          const next = await api.mcpSendNext();
          if (stopped) return;
          if (next) {
            setAsked(next);
            void native("show_main_window", {});
            return;
          }
        } catch {
          await new Promise((done) => setTimeout(done, RETRY_MS));
        }
      }
    };
    void watch();
    return () => {
      stopped = true;
    };
  }, [on, asked]);

  if (!asked) return null;

  const answer = async (send: boolean) => {
    setBusy(send ? "send" : "no");
    setError(null);
    try {
      await api.mcpSendDecide(asked.id, send);
      setAsked(null);
      if (send) onSent?.();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  const person = (address: string) => (
    <span key={address} className="send-ask-person">
      <Avatar name={address} size="xs" />
      <span>{address}</span>
    </span>
  );

  return (
    <Dialog
      className="send-ask-dialog"
      label={tx("Send this email?")}
      title={
        <>
          <span className="send-ask-eyebrow">
            <Icon icon={AiMagicIcon} size="xs" />
            {tx("Written by an AI app")}</span>
          {tx("Send this email?")}</>
      }
      subtitle={tx("It only goes out if you say so.")}
      footer={
        <>
          <button type="button" className="btn btn-secondary" disabled={busy !== null} onClick={() => void answer(false)}>
            {busy === "no" && <Spinner size="xs" />}
            {tx("Don't send")}</button>
          <button type="button" className="btn btn-primary" disabled={busy !== null} onClick={() => void answer(true)}>
            {busy === "send" ? <Spinner size="xs" /> : <Icon icon={SentIcon} size="s" />}
            {tx("Send")}</button>
        </>
      }
    >
      <div className="send-ask">
        <dl className="send-ask-people">
          <dt>{tx("From")}</dt>
          <dd>{person(asked.from)}</dd>
          <dt>{tx("To")}</dt>
          <dd>{asked.to.map(person)}</dd>
          {asked.cc.length > 0 && (
            <>
              <dt>{tx("Cc")}</dt>
              <dd>{asked.cc.map(person)}</dd>
            </>
          )}
        </dl>
        <div className="send-ask-mail">
          <div className="send-ask-subject">{asked.subject || tx("No subject")}</div>
          <div className="send-ask-text">{asked.text}</div>
          {asked.quoted && (
            <>
              <div className="send-ask-quote-label">{tx("Also sent, quoted below it")}</div>
              <div className="send-ask-text send-ask-quote">{asked.quoted}</div>
            </>
          )}
        </div>
      </div>
      <Problem text={error} />
    </Dialog>
  );
}
