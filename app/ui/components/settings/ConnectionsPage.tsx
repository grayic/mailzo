import { useEffect, useState } from "react";
import { ApiError, api, type AiAppLink, type AiAppsStatus } from "../../api";
import { Tick02Icon } from "../../icons";
import { useSettings } from "../../settings";
import { Confirm, Dialog, Problem, Spinner } from "../ui";
import { Page, Row, Section, SettingToggle, Toggle } from "./controls";
import { language, tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** An email an AI reads can try to talk it into things, so these ask once before turning on. */
const WITH_CARE = [
  {
    setting: "mcp_send_unasked",
    label: tx("Send without your approval"),
    help: tx("Emails go out straight away. You don't see them first."),
    warning:
      tx("AI apps could send email in your name without you seeing it. An email they read could even trick them into it."),
  },
  {
    setting: "mcp_delete_forever",
    label: tx("Delete forever"),
    help: tx("Erases email that's already in the trash or spam."),
    warning: tx("AI apps could erase email from your trash and spam. You can't get it back."),
  },
  {
    setting: "mcp_suspicious",
    label: tx("Read suspicious email"),
    help: tx("Includes email this app marked as dangerous."),
    warning: tx("These emails are often written to fool people and AI. An AI app could follow what one says."),
  },
  {
    setting: "mcp_drafts_shared",
    label: tx("Read your drafts and shared inboxes"),
    help: tx("Includes what you haven't sent and your team's email."),
    warning: tx("AI apps could read what you're still writing, and email your team shares with you."),
  },
];

function WithCare() {
  const { settings, change } = useSettings();
  const [asking, setAsking] = useState<(typeof WITH_CARE)[number] | null>(null);
  return (
    <Section title={tx("Use with care")} note={tx("Off until you turn them on. Reopen the AI app after changes.")}>
      {WITH_CARE.map((item) => (
        <Row key={item.setting} label={item.label} help={item.help}>
          <Toggle
            label={item.label}
            on={settings[item.setting] === "on"}
            onChange={(on) => (on ? setAsking(item) : change(item.setting, "off"))}
          />
        </Row>
      ))}
      {asking && (
        <Confirm
          title={`Turn on "${asking.label}"?`}
          confirm={tx("Yes, turn it on")}
          keep={tx("Keep it off")}
          danger
          onKeep={() => setAsking(null)}
          onConfirm={() => {
            change(asking.setting, "on");
            setAsking(null);
          }}
        >
          {asking.warning}
        </Confirm>
      )}
    </Section>
  );
}

/** The setup for any app we don't connect by ourselves. */
function CopySetup({ setup, onClose }: { setup: string; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <Dialog
      title={tx("Copy setup")}
      subtitle={tx("Paste this into the app's MCP settings.")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tx("Close")}</button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => void navigator.clipboard.writeText(setup).then(() => setCopied(true))}
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </>
      }
    >
      <pre className="setup-code">{setup}</pre>
    </Dialog>
  );
}

/** Connects the person's other AI apps to their email, one click each. */
export function ConnectionsPage({ onSeeActivity }: { onSeeActivity: () => void }) {
  const { settings, reload } = useSettings();
  const [status, setStatus] = useState<AiAppsStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  /** The app just connected, which still needs the person to do one last thing. */
  const [justConnected, setJustConnected] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .mcpStatus()
      .then(setStatus)
      .catch((e) => setError(say(e)));
  }, []);

  const change = async (app: AiAppLink, connect: boolean) => {
    setBusy(app.id);
    setError(null);
    try {
      setStatus(await (connect ? api.mcpConnect(app.id) : api.mcpDisconnect(app.id)));
      setJustConnected(connect ? app.id : null);
      if (connect) reload();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(null);
    }
  };

  const allowed = settings.mcp_on === "on";
  const anyConnected = status?.apps.some((a) => a.link === "connected") ?? false;
  const lastUsed = status?.last_used?.split(" ")[0];

  const helpFor = (app: AiAppLink) => {
    if (app.link === "outdated") return tx("Needs reconnecting");
    if (app.link === "taken") return tx("Already has a connection with this name");
    if (app.link === "unreadable") return tx("Set this one up by hand");
    if (app.link !== "connected") return undefined;
    if (!allowed) return tx("Paused. Turn on Allow connected apps to use it.");
    if (justConnected === app.id) return app.after;
    return <span className="link-on">{tx("Connected")}</span>;
  };

  const buttonFor = (app: AiAppLink) => {
    if (app.link === "taken" || app.link === "unreadable") {
      return (
        <button className="btn btn-secondary btn-sm" onClick={() => setCopying(true)}>
          {tx("Copy setup")}</button>
      );
    }
    const connected = app.link === "connected";
    return (
      <button
        className={`btn btn-sm ${connected ? "btn-ghost" : "btn-secondary"}`}
        disabled={busy !== null}
        onClick={() => void change(app, !connected)}
      >
        {busy === app.id && <Spinner size="xs" />}
        {connected ? "Disconnect" : app.link === "outdated" ? "Reconnect" : "Connect"}
      </button>
    );
  };

  return (
    <Page title={tx("MCP")} intro={tx("Connect Claude, Cursor and other AI apps to your email.")}>
      <Section title={tx("Apps")} note={tx("Keep this app open while you use them.")}>
        {status?.apps.map((app) => (
          <Row key={app.id} label={app.name} help={helpFor(app)}>
            {buttonFor(app)}
          </Row>
        ))}
        {status && (
          <Row label={tx("Another app")} help={tx("Any app that supports MCP")}>
            <button className="btn btn-secondary btn-sm" onClick={() => setCopying(true)}>
              {tx("Copy setup")}</button>
          </Row>
        )}
        {lastUsed && (
          <Row label={tx("Last used")}>
            <span className="muted">{new Date(Number(lastUsed) * 1000).toLocaleString(language)}</span>
          </Row>
        )}
      </Section>
      <Problem text={error} />
      {(anyConnected || lastUsed) && (
        <Section>
          <Row label={tx("AI activity")} help={tx("See what connected apps have done in your email.")}>
            <button className="btn btn-secondary btn-sm" onClick={onSeeActivity}>
              {tx("See activity")}
            </button>
          </Row>
        </Section>
      )}
      <Section title={tx("Always allowed")}>
        {(anyConnected || allowed) && <SettingToggle setting="mcp_on" label={tx("Allow connected apps")} />}
        <Row icon={Tick02Icon} label={tx("Search your email")} />
        <Row icon={Tick02Icon} label={tx("See your newest email")} />
        <Row icon={Tick02Icon} label={tx("Read whole conversations")} />
      </Section>
      <Section title={tx("Your choice")} note={tx("Reopen the AI app after changes. Pick inboxes in Settings › AI.")}>
        <SettingToggle setting="mcp_drafts" label={tx("Save drafts")} help={tx("You check them first.")} />
        <SettingToggle setting="mcp_send" label={tx("Send email")} help={tx("You approve each one first.")} />
        <SettingToggle setting="mcp_organize" label={tx("Move, archive and mark as read")} />
        <SettingToggle setting="mcp_delete" label={tx("Delete")} help={tx("It goes to Trash, so you can get it back.")} />
        <SettingToggle setting="mcp_attachments" label={tx("Open attachments")} />
        <SettingToggle setting="mcp_spam_trash" label={tx("Read spam and trash")} />
      </Section>
      <WithCare />
      {copying && status && <CopySetup setup={status.setup} onClose={() => setCopying(false)} />}
    </Page>
  );
}
