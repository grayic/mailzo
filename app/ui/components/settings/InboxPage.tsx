import { FormEvent, useEffect, useRef, useState } from "react";
import { ApiError, api, type AutoReply } from "../../api";
import { Add01Icon, Delete02Icon, Tick02Icon } from "../../icons";
import type { Account, AccountPrefs, Addr, Folder, Signature } from "../../types";
import { RichEditor } from "../RichEditor";
import { Confirm, Icon, IconButton, PasswordInput, Problem, Spinner, accountHue } from "../ui";
import { Choice, Page, Row, Section, Segmented, Toggle } from "./controls";
import { EncryptionSection } from "./Encryption";
import { tx } from "../../i18n";
import { useSettings } from "../../settings";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const SAVE_AFTER_MS = 500;
const HUES = [4, 28, 45, 145, 175, 205, 230, 265, 315];
const JOBS: [string, string][] = [
  ["sent", tx("Sent")],
  ["drafts", tx("Drafts")],
  ["archive", tx("Archive")],
  ["junk", tx("Spam")],
  ["trash", tx("Trash")],
];
const escapeHtml = (t: string) =>
  t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** Older versions kept one plain-text signature. It starts as the first rich one. */
function startingPrefs(account: Account): AccountPrefs {
  const prefs = account.prefs;
  if (prefs.signatures.length > 0 || !account.signature.trim()) return prefs;
  const html = account.signature
    .split("\n")
    .map((line) => `<p>${escapeHtml(line)}</p>`)
    .join("");
  return {
    ...prefs,
    signatures: [{ id: "first", name: "Signature", html }],
    signature_new: "first",
    signature_reply: "first",
  };
}

function TextField({
  label,
  value,
  placeholder,
  onCommit,
}: {
  label: string;
  value: string;
  placeholder?: string;
  onCommit: (value: string) => void;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <input
        value={text}
        placeholder={placeholder}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => text !== value && onCommit(text)}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
        autoCapitalize="off"
        autoCorrect="off"
      />
    </label>
  );
}

function SignatureEditor({
  signature,
  onChange,
  onRemove,
}: {
  signature: Signature;
  onChange: (next: Signature) => void;
  onRemove: () => void;
}) {
  const latest = useRef(signature);
  latest.current = signature;
  return (
    <div className="signature" data-signature={signature.name}>
      <div className="signature-head">
        <input
          aria-label={tx("Signature name")}
          className="plain-input strong"
          value={signature.name}
          onChange={(e) => onChange({ ...signature, name: e.target.value })}
        />
        <IconButton icon={Delete02Icon} label={`Delete ${signature.name}`} small onClick={onRemove} />
      </div>
      <div className="signature-box">
        <RichEditor
          initial={signature.html}
          autoFocus={false}
          placeholder={tx("Write your signature")}
          label={tx("Signature text")}
          onChange={(html) => onChange({ ...latest.current, html })}
          onProblem={() => {}}
          onDropFiles={() => {}}
        />
      </div>
    </div>
  );
}

function ConnectionSection({ account, onChanged }: { account: Account; onChanged: () => void }) {
  const [password, setPassword] = useState("");
  const [host, setHost] = useState(account.imap_host);
  const [port, setPort] = useState(String(account.imap_port));
  const [username, setUsername] = useState(account.username);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setMessage(null);
    const server = host.trim().toLowerCase();
    try {
      await api.updateConnection({
        account_id: account.id,
        imap_host: server,
        imap_port: Number(port) || 0,
        security: ["localhost", "127.0.0.1", "::1"].includes(server) ? "plain" : "tls",
        username,
        password: password || null,
      });
      setPassword("");
      setMessage(tx("Connected, and saved."));
      onChanged();
    } catch (err) {
      setError(say(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={save}>
      <div className="section-title">{tx("Signing in")}</div>
      <div className="group group-pad stack">
        <label className="field">
          <span className="field-label">{tx("Password")}</span>
          <PasswordInput
            aria-label={tx("New password")}
            value={password}
            placeholder={tx("Only if it has changed")}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="off"
          />
        </label>
        <div className="settings-server">
          <label className="field grow">
            <span className="field-label">{tx("Mail server (IMAP)")}</span>
            <input
              value={host}
              onChange={(e) => setHost(e.target.value)}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
            />
          </label>
          <label className="field settings-port">
            <span className="field-label">{tx("Port")}</span>
            <input inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value)} />
          </label>
        </div>
        <label className="field">
          <span className="field-label">{tx("Username")}</span>
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
        </label>
        <div className="hstack-m">
          <button type="submit" className="btn btn-secondary btn-sm" disabled={busy}>
            {busy && <Spinner size="xs" />}
            {tx("Test and save")}</button>
          {message && (
            <span role="status" className="fine hstack">
              <Icon icon={Tick02Icon} size="xs" />
              {message}
            </span>
          )}
        </div>
        <Problem text={error} />
      </div>
    </form>
  );
}

function BrowserSignInSection({ account, onChanged }: { account: Account; onChanged: () => void }) {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const name = account.auth === "google" ? "Google" : "Microsoft";

  const again = async () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await api.mailSignIn(account.email, account.auth);
      setMessage(tx("Signed in."));
      onChanged();
    } catch (err) {
      if (!(err instanceof ApiError && err.code === "cancelled")) setError(say(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <div className="section-title">{tx("Signing in")}</div>
      <div className="group">
        <Row
          label={tx("Signed in with {name}", { name })}
          help={tx("This app never sees your password. Remove its access any time in your account.")}
        >
          <button type="button" className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void again()}>
            {busy && <Spinner size="xs" />}
            {tx("Sign in again")}</button>
        </Row>
      </div>
      {message && <p className="fine note">{message}</p>}
      <Problem text={error} />
    </section>
  );
}

function OutgoingSection({ account, onChanged }: { account: Account; onChanged: () => void }) {
  const [host, setHost] = useState(account.smtp_host);
  const [port, setPort] = useState(account.smtp_port ? String(account.smtp_port) : "465");
  const [security, setSecurity] = useState<Account["smtp_security"]>(account.smtp_security);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [checked, setChecked] = useState<string | null>(null);

  const check = async () => {
    setBusy(true);
    setError(null);
    setChecked(null);
    try {
      await api.updateAccountSettings({
        account_id: account.id,
        display_name: account.display_name,
        signature: account.signature,
        smtp_host: host,
        smtp_port: Number(port) || 0,
        smtp_security: security,
      });
      const r = await api.checkSmtp(account.id);
      setHost(r.host);
      setPort(String(r.port));
      setSecurity(r.security);
      setChecked(tx("Connected to {0}. Sending should work.", { "0": r.host }));
      onChanged();
    } catch (err) {
      setError(say(err));
    } finally {
      setBusy(false);
    }
  };

  const local = ["localhost", "127.0.0.1"].includes(host.trim());
  return (
    <section>
      <div className="section-title">{tx("Outgoing mail server")}</div>
      <div className="group group-pad stack">
        <p className="fine">{tx("Found automatically. Leave the name empty to look again.")}</p>
        <div className="settings-server">
          <label className="field grow">
            <span className="field-label">{tx("Server name (SMTP)")}</span>
            <input
              value={host}
              placeholder={tx("Found automatically")}
              onChange={(e) => setHost(e.target.value)}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
            />
          </label>
          <label className="field settings-port">
            <span className="field-label">{tx("Port")}</span>
            <input inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value)} />
          </label>
        </div>
        <div className="field">
          <span className="field-label">{tx("Security")}</span>
          <Segmented
            label={tx("Outgoing security")}
            value={security}
            onChange={(v) => setSecurity(v as Account["smtp_security"])}
            options={[
              { value: "tls", label: "SSL/TLS" },
              { value: "starttls", label: tx("STARTTLS") },
              ...(local ? [{ value: "plain", label: tx("None") }] : []),
            ]}
          />
        </div>
        <div className="hstack-m">
          <button type="button" className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void check()}>
            {busy && <Spinner size="xs" />}
            {tx("Test connection")}</button>
          {checked && (
            <span role="status" className="fine hstack">
              <Icon icon={Tick02Icon} size="xs" />
              <span className="truncate-1">{checked}</span>
            </span>
          )}
        </div>
        <Problem text={error} />
      </div>
    </section>
  );
}

/** Replies the provider sends by itself, because this device may be off. */
function AutoReplySection({ account }: { account: Account }) {
  const [state, setState] = useState<{ supported: boolean; reply?: AutoReply; provider?: string; link?: string | null } | null>(null);
  const [reply, setReply] = useState<AutoReply>({ on: false, subject: "", message: "", starts: null, ends: null, contacts_only: false });
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api
      .autoReplyGet(account.id)
      .then((s) => {
        setState(s);
        if (s.reply) setReply(s.reply);
      })
      .catch((e) => setError(e instanceof ApiError ? e.message : String(e)));
  }, [account.id]);
  const save = async () => {
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      await api.autoReplySet(account.id, reply);
      setSaved(true);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  if (!state) return error ? <Problem text={error} /> : null;
  if (!state.supported) {
    return (
      <Section title={tx("Out-of-office replies")}>
        <Row
          label={tx("Set on your provider's website")}
          help={tx("Email has no standard way to ask for these, so {0} handles it itself.", { "0": state.provider ?? "your provider" })}
        >
          {state.link && (
            <a className="btn btn-secondary btn-sm" href={state.link} target="_blank" rel="noreferrer">
              {tx("Open")}</a>
          )}
        </Row>
      </Section>
    );
  }
  return (
    <Section title={tx("Out-of-office replies")} note={tx("Your provider sends these, so they go even when this device is off.")}>
      <Row label={tx("Reply automatically")}>
        <Toggle label={tx("Reply automatically")} on={reply.on} onChange={(on) => setReply({ ...reply, on })} />
      </Row>
      {reply.on && (
        <div className="group-row group-row-stacked stack">
          <textarea
            aria-label={tx("Out-of-office message")}
            rows={4}
            placeholder={tx("I'm away until Monday.")}
            value={reply.message}
            onChange={(e) => setReply({ ...reply, message: e.target.value })}
          />
        </div>
      )}
      <div className="group-row">
        <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void save()}>
          {busy && <Spinner size="xs" />}
          {tx("Save")}</button>
        {saved && <span role="status" className="fine">{tx("Saved.")}</span>}
        <Problem text={error} />
      </div>
    </Section>
  );
}

export function InboxPage({
  account,
  folders,
  onChanged,
  onDisconnect,
}: {
  account: Account;
  folders: Folder[];
  onChanged: () => void;
  onDisconnect: () => void;
}) {
  const [prefs, setPrefs] = useState<AccountPrefs>(() => startingPrefs(account));
  const [name, setName] = useState(account.display_name === account.email ? "" : account.display_name);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newAlias, setNewAlias] = useState<Addr>({ name: "", address: "" });
  const [teamAdmin, setTeamAdmin] = useState(false);
  const [teamName, setTeamName] = useState("");
  const { settings } = useSettings();
  // What "Default" means right now, from Settings › Notifications.
  const appAlerts =
    settings.alerts !== "on"
      ? tx("Nothing")
      : settings.alert_which === "important"
        ? tx("Only what needs me")
        : settings.alert_which === "people"
          ? tx("People only")
          : tx("All new mail");
  useEffect(() => {
    if (!account.prefs.team_shared) return;
    api
      .teamStatus(account.prefs.team_id || undefined)
      .then((s) => {
        setTeamAdmin(s.role === "owner" || s.role === "admin");
        setTeamName(s.name ?? "");
      })
      .catch(() => {});
  }, [account.prefs.team_shared, account.prefs.team_id]);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const waiting = useRef<AccountPrefs | null>(null);

  const saveNow = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const next = waiting.current;
    waiting.current = null;
    if (!next) return;
    api
      .updateAccountPrefs(account.id, next)
      .then(() => {
        setError(null);
        onChanged();
      })
      .catch((e) => setError(say(e)));
  };

  // Saves what's waiting when the page goes away.
  useEffect(() => saveNow, []); // eslint-disable-line react-hooks/exhaustive-deps

  /** Saves after a short pause, so typing a signature isn't a save per letter. */
  const update = (change: Partial<AccountPrefs>) => {
    setPrefs((current) => {
      const next = { ...current, ...change };
      waiting.current = next;
      return next;
    });
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(saveNow, SAVE_AFTER_MS);
  };

  const saveName = (value: string) => {
    const clean = value.trim();
    setName(clean);
    api
      .updateAccountSettings({
        account_id: account.id,
        display_name: clean || account.email,
        signature: account.signature,
        smtp_host: account.smtp_host,
        smtp_port: account.smtp_port,
        smtp_security: account.smtp_security,
      })
      .then(onChanged)
      .catch((e) => setError(say(e)));
  };

  const addSignature = () => {
    const id = crypto.randomUUID();
    const first = prefs.signatures.length === 0;
    update({
      signatures: [...prefs.signatures, { id, name: first ? "Signature" : "New signature", html: "" }],
      ...(first ? { signature_new: id, signature_reply: id } : {}),
    });
  };

  const removeSignature = (id: string) => {
    update({
      signatures: prefs.signatures.filter((s) => s.id !== id),
      signature_new: prefs.signature_new === id ? null : prefs.signature_new,
      signature_reply: prefs.signature_reply === id ? null : prefs.signature_reply,
    });
  };

  const signatureOptions = [
    { value: "", label: tx("None") },
    ...prefs.signatures.map((s) => ({ value: s.id, label: s.name || "Signature" })),
  ];

  const addAlias = (e: FormEvent) => {
    e.preventDefault();
    if (!newAlias.address.trim()) return;
    update({ aliases: [...prefs.aliases, { name: newAlias.name.trim(), address: newAlias.address.trim() }] });
    setNewAlias({ name: "", address: "" });
  };

  const jobFolder = (job: string) =>
    Object.entries(prefs.folder_roles).find(([, role]) => role === job)?.[0] ?? "";

  const setJob = (job: string, path: string) => {
    const roles = Object.fromEntries(
      Object.entries(prefs.folder_roles).filter(([p, role]) => role !== job && p !== path),
    );
    if (path) roles[path] = job;
    update({ folder_roles: roles });
  };

  const folderOptions = [
    { value: "", label: tx("Automatic") },
    ...folders.filter((f) => f.role !== "inbox").map((f) => ({ value: f.path, label: f.path })),
  ];

  const hue = accountHue({ ...account, prefs });

  return (
    <Page title={prefs.label || account.email} intro={prefs.label ? account.email : undefined}>
      <section>
        <div className="section-title">{tx("Identity")}</div>
        <div className="group group-pad stack">
          <TextField
            label={tx("Your name, as people see it")}
            value={name}
            placeholder={account.email}
            onCommit={saveName}
          />
          <TextField
            label={tx("Short label")}
            value={prefs.label}
            placeholder={tx("Work")}
            onCommit={(label) => update({ label: label.trim() })}
          />
        </div>
      </section>

      <Section>
        <Row label={tx("Colour")} help={tx("Marks this inbox's email in All Inbox.")}>
          <div role="radiogroup" aria-label={tx("Inbox colour")} className="hue-row">
            {HUES.map((h) => (
              <button
                key={h}
                type="button"
                role="radio"
                aria-checked={hue === h}
                aria-label={`Colour ${h}`}
                className="hue"
                style={{ "--h": h } as React.CSSProperties}
                onClick={() => update({ color: h })}
              />
            ))}
          </div>
        </Row>
        <Row label={tx("Include in All Inbox")}>
          <Toggle
            label={tx("Include in All Inbox")}
            on={prefs.in_all_inbox}
            onChange={(in_all_inbox) => update({ in_all_inbox })}
          />
        </Row>
      </Section>

      <section>
        <div className="section-title">{tx("Signatures")}</div>
        {prefs.signatures.map((s) => (
          <SignatureEditor
            key={s.id}
            signature={s}
            onChange={(next) =>
              update({ signatures: prefs.signatures.map((x) => (x.id === s.id ? next : x)) })
            }
            onRemove={() => removeSignature(s.id)}
          />
        ))}
        <button type="button" className="btn btn-secondary btn-sm" onClick={addSignature}>
          <Icon icon={Add01Icon} size="xs" />
          {tx("Add a signature")}</button>
        {prefs.signatures.length > 0 && (
          <div className="group section-gap">
            <Row label={tx("For new messages and forwards")}>
              <Choice
                label={tx("Signature for new messages")}
                options={signatureOptions}
                value={prefs.signature_new ?? ""}
                onChange={(v) => update({ signature_new: v || null })}
              />
            </Row>
            <Row label={tx("For replies")}>
              <Choice
                label={tx("Signature for replies")}
                options={signatureOptions}
                value={prefs.signature_reply ?? ""}
                onChange={(v) => update({ signature_reply: v || null })}
              />
            </Row>
          </div>
        )}
      </section>

      <Section
        title={tx("About you")}
        note={tx("AI uses this to write your replies from this inbox. Say what you do, and what you'd turn down.")}
      >
        <div className="group-row group-row-stacked">
          <textarea
            aria-label={tx("About you")}
            rows={4}
            maxLength={1000}
            placeholder={tx("I run a small design studio. We don't buy marketing or web design services.")}
            value={prefs.about ?? ""}
            onChange={(e) => update({ about: e.target.value })}
          />
        </div>
      </Section>

      <section>
        <div className="section-title">{tx("Other addresses you send from")}</div>
        <div className="group">
          {prefs.aliases.map((a) => (
            <div key={a.address} className="group-row">
              <div className="grow">
                <div className="row-label truncate-1">{a.address}</div>
                {a.name && <div className="row-help">{a.name}</div>}
              </div>
              <IconButton
                icon={Delete02Icon}
                label={`Remove ${a.address}`}
                small
                onClick={() => update({ aliases: prefs.aliases.filter((x) => x !== a) })}
              />
            </div>
          ))}
          <form onSubmit={addAlias} className="group-row alias-form">
            <input
              aria-label={tx("Other address")}
              type="email"
              placeholder={tx("hello@company.com")}
              value={newAlias.address}
              onChange={(e) => setNewAlias({ ...newAlias, address: e.target.value })}
            />
            <input
              aria-label={tx("Name for that address")}
              placeholder={tx("Name (optional)")}
              value={newAlias.name}
              onChange={(e) => setNewAlias({ ...newAlias, name: e.target.value })}
            />
            <button type="submit" className="btn btn-secondary btn-sm">
              {tx("Add")}</button>
          </form>
          {prefs.aliases.length > 0 && (
            <Row label={tx("Reply from")}>
              <Segmented
                label={tx("Reply from")}
                value={prefs.reply_from}
                onChange={(reply_from) => update({ reply_from })}
                options={[
                  { value: "arrived", label: tx("Where it arrived") },
                  { value: "main", label: tx("This inbox") },
                ]}
              />
            </Row>
          )}
        </div>
      </section>

      <AutoReplySection account={account} />
      <EncryptionSection account={account} />
      <Section title={tx("Notifications")}>
        <Row label={tx("Notify me about")} help={tx("Default follows Settings › Notifications.")}>
          <Choice
            label={tx("Notify me about")}
            value={prefs.alerts}
            onChange={(alerts) => update({ alerts })}
            options={[
              { value: "default", label: tx("Default ({0})", { "0": appAlerts }) },
              { value: "all", label: tx("All new mail") },
              { value: "people", label: tx("People only") },
              { value: "none", label: tx("Nothing") },
            ]}
          />
        </Row>
      </Section>

      {folders.length > 0 && (
        <Section title={tx("Folders")} note={tx("Change only if your provider names folders oddly.")}>
          {JOBS.map(([job, label]) => (
            <Row key={job} label={label}>
              <Choice
                label={`${label} folder`}
                options={folderOptions}
                value={jobFolder(job)}
                onChange={(path) => setJob(job, path)}
              />
            </Row>
          ))}
        </Section>
      )}

      {account.prefs.team_shared && (
        <Section
          note={
            teamAdmin
              ? tx("Team inbox. A new password here reaches everyone. Stop sharing in Team settings.")
              : tx("Team inbox. An admin can change how it connects.")
          }
        >
          <div className="group-row strong">{teamName ? tx("Shared with {teamName}", { teamName }) : tx("Shared with your team")}</div>
        </Section>
      )}
      {account.prefs.team_shared && !teamAdmin ? null : account.auth === "password" ? (
        <>
          <ConnectionSection account={account} onChanged={onChanged} />
          <OutgoingSection account={account} onChanged={onChanged} />
        </>
      ) : (
        <BrowserSignInSection account={account} onChanged={onChanged} />
      )}

      <Problem text={error} />

      {!account.prefs.team_shared && (
        <Section title={tx("Disconnect")}>
          <Row label={tx("Disconnect this inbox")} help={tx("Removes it from all your devices. Your mail stays with your email provider.")}>
            <button className="btn btn-secondary btn-sm" onClick={() => setConfirming(true)}>
              {tx("Disconnect this inbox")}</button>
          </Row>
        </Section>
      )}

      {confirming && (
        <Confirm
          title={tx("Disconnect this inbox?")}
          confirm={tx("Yes, disconnect")}
          keep={tx("Keep")}
          danger
          onKeep={() => setConfirming(false)}
          onConfirm={onDisconnect}
        >
          {account.email} {" "}{tx("will be removed from all your devices. Your mail stays with your email provider.")}</Confirm>
      )}
    </Page>
  );
}
