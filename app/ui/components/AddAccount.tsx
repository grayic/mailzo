import { FormEvent, useEffect, useState } from "react";
import { ApiError, api, type FoundProvider } from "../api";
import { ArrowDown01Icon, ArrowLeft01Icon, ArrowRight01Icon, InformationCircleIcon, LinkSquare02Icon, Tick02Icon } from "../icons";
import type { Account } from "../types";
import { GoogleMark, Icon, Logo, PasswordInput, Problem, Spinner } from "./ui";
import { tx } from "../i18n";

const isThisComputer = (host: string) => ["localhost", "127.0.0.1", "::1"].includes(host);

const BROWSER_NAME: Record<string, string> = { microsoft: "Microsoft", google: "Google" };

const PASSWORD_LABEL: Record<string, string> = {
  password: "Password",
  app_password: tx("App password"),
  bridge: tx("Bridge password"),
  manual: "Password",
};

export function AddAccount({
  onAdded,
  onCancel,
  title = tx("Connect an inbox"),
  intro = tx("Your mail is saved only on your own devices. You can connect as many inboxes as you like."),
  startEmail = "",
  children,
}: {
  onAdded: (a: Account) => void;
  onCancel?: () => void;
  title?: string;
  intro?: string;
  /** Filled in ready to connect, such as the address the person signed in with. */
  startEmail?: string;
  children?: React.ReactNode;
}) {
  const [email, setEmail] = useState(startEmail);
  /** What was worked out from the address. Nothing else shows until then. */
  const [found, setFound] = useState<FoundProvider | null>(null);
  const [password, setPassword] = useState("");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("993");
  const [username, setUsername] = useState("");
  const [showServer, setShowServer] = useState(false);
  /** The setup steps whose page has been opened, so the next one can be shown as the one to do. */
  const [opened, setOpened] = useState<number[]>([]);
  /** Choosing a password even though browser sign-in is on offer. */
  const [usePassword, setUsePassword] = useState(false);
  const [inBrowser, setInBrowser] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** This build can sign in to Google, so there's no need to type an address first. */
  const [google, setGoogle] = useState(false);

  useEffect(() => {
    void api
      .signInProviders()
      .then((providers) => setGoogle(!!providers.google))
      .catch(() => {});
  }, []);

  const domain = email.split("@")[1]?.toLowerCase() ?? "";

  const lookUp = async () => {
    setError(null);
    setBusy(true);
    try {
      const provider = await api.findProvider(email);
      setFound(provider);
      setHost(provider.imap_host);
      setPort(String(provider.imap_port));
      setUsername("");
      setShowServer(provider.method === "manual");
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const startOver = () => {
    setFound(null);
    setOpened([]);
    setPassword("");
    setError(null);
    setShowServer(false);
    setUsePassword(false);
  };

  /** From the app password steps to the sign-in choice, and from there to the address. */
  const back = () => {
    if (!usePassword) return startOver();
    setUsePassword(false);
    setPassword("");
    setError(null);
    setShowServer(false);
  };

  const signInWithBrowser = async (address: string, provider: string) => {
    setError(null);
    setInBrowser(true);
    try {
      onAdded(await api.mailSignIn(address, provider));
    } catch (err) {
      // Stopping on purpose needs no telling off.
      if (!(err instanceof ApiError && err.code === "cancelled")) {
        setError(err instanceof ApiError ? err.message : String(err));
      }
    } finally {
      setInBrowser(false);
    }
  };

  const connect = async () => {
    if (!found) return;
    setError(null);
    const server = host.trim().toLowerCase();
    if (!server) {
      setShowServer(true);
      setError(tx("Enter your mail server. Your email provider's help pages will list it."));
      return;
    }
    setBusy(true);
    // The settings found may offer more than one username. Typing one overrides them.
    const names = username.trim() ? [username.trim()] : found.usernames.length ? found.usernames : [""];
    const sameServer = server === found.imap_host;
    try {
      let lastProblem: unknown = null;
      for (const name of names) {
        try {
          onAdded(
            await api.addAccount({
              email,
              imap_host: server,
              imap_port: Number(port),
              security: isThisComputer(server) ? "plain" : "tls",
              username: name === email ? "" : name,
              // App passwords are shown in groups ("abcd efgh ijkl mnop"), and copied with the spaces.
              password: found.method === "app_password" ? password.replace(/\s+/g, "") : password,
              smtp_host: sameServer ? found.smtp_host : "",
              smtp_port: sameServer ? found.smtp_port : 0,
              smtp_security: sameServer ? found.smtp_security : "tls",
            }),
          );
          return;
        } catch (err) {
          lastProblem = err;
          if (!(err instanceof ApiError && err.code === "auth")) break;
        }
      }
      const refused = lastProblem instanceof ApiError && lastProblem.code === "auth";
      if (refused && found.method === "app_password") {
        setError(tx("{0} refused that. It needs an app password, not your normal password.", { "0": found.name }));
      } else if (refused && found.method === "bridge") {
        setError(tx("Bridge refused that. Use the password Bridge shows, not your Proton password, and check Bridge is running."));
      } else {
        setError(lastProblem instanceof ApiError ? lastProblem.message : String(lastProblem));
      }
      if (!refused) setShowServer(true);
    } finally {
      setBusy(false);
    }
  };

  const browserFirst = !!found?.browser && !usePassword;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (browserFirst && found?.browser) void signInWithBrowser(email, found.browser);
    else void (found ? connect() : lookUp());
  };

  const method = found?.method;
  const hasPasswordPath =
    method !== undefined && method !== "browser" && method !== "browser_soon" && method !== "unsupported";
  const canConnect = hasPasswordPath && (!found?.browser || usePassword);
  const guided = !!found && found.steps.length > 0 && !browserFirst;
  // Once the provider is known and there's one clear way in, the screen is about that provider.
  const focused = guided || (!!found && browserFirst);
  // The password box goes in the last step ("Paste it here"), so it reads as one path.
  const boxInSteps = guided && canConnect;
  const passwordLabel = found ? (PASSWORD_LABEL[found.method] ?? "Password") : "";
  // Known providers never need server settings unless something went wrong.
  const serverOffered = !!found && (found.source !== "list" || showServer || !!error);
  const lastStep = (found?.steps.length ?? 0) - 1;
  // The step to do now: the first one not done yet. The last is done once something's pasted.
  const pasted = password.trim() !== "";
  const stepDone = (i: number) =>
    pasted || (i !== lastStep && (opened.includes(i) || i < Math.max(-1, ...opened)));
  const current = found ? found.steps.findIndex((_, i) => !stepDone(i)) : -1;

  const passwordBox = (
    <PasswordInput
      required
      autoFocus
      aria-label={passwordLabel}
      placeholder={boxInSteps ? tx("Paste here") : undefined}
      value={password}
      onChange={(e) => setPassword(e.target.value)}
      autoComplete="off"
    />
  );

  return (
    <div className="welcome">
      <div className="welcome-drag" data-tauri-drag-region />
      <div className="welcome-inner">
        <form onSubmit={submit} className={`welcome-card stack ${guided ? "welcome-card-guide" : ""}`} data-provider={found?.provider}>
          <div>
            {found && !inBrowser && (
              <button type="button" className="btn btn-ghost btn-sm welcome-back" onClick={back}>
                <Icon icon={ArrowLeft01Icon} size="s" />
                {tx("Back")}</button>
            )}
            <Logo provider={found?.provider} />
            <h1 className="welcome-title">{focused && found ? tx("Connect {0}", { "0": found.name }) : title}</h1>
            {!(focused && found) ? (
              <p className="welcome-text">{intro}</p>
            ) : browserFirst ? (
              <p className="welcome-text">
                {tx("You'll sign in on {0}'s own page.", { "0": BROWSER_NAME[found.browser ?? ""] ?? found.name })}
              </p>
            ) : found.method === "app_password" ? (
              <p className="welcome-text">{tx("Your {0} password stays private.", { "0": found.name })}</p>
            ) : null}
          </div>
          {google && !found && (
            <div className="stack">
              {inBrowser ? (
                <div role="status" className="tip">
                  <Spinner size="xs" />
                  <span className="grow">{tx("Finish signing in on the page that opened in your browser.")}</span>
                  <button type="button" className="link-btn" onClick={() => void api.mailSignInCancel()}>
                    {tx("Cancel")}</button>
                </div>
              ) : (
                <button
                  type="button"
                  className="btn btn-secondary btn-lg btn-block"
                  onClick={() => void signInWithBrowser("", "google")}
                >
                  <GoogleMark />
                  {tx("Continue with Google")}</button>
              )}
              <div className="or">or</div>
            </div>
          )}
          {focused && found ? (
            <div className="address-chip">
              <span className="address-chip-text">{email}</span>
              <button type="button" className="link-btn" onClick={startOver}>
                {tx("Change")}</button>
            </div>
          ) : (
          <label className="field">
            <span className="field-label">{tx("Email address")}</span>
            <input
              type="email"
              required
              autoFocus
              placeholder={tx("you@company.com")}
              value={email}
              onChange={(e) => {
                setEmail(e.target.value);
                if (found) startOver();
              }}
              autoComplete="off"
            />
          </label>
          )}

          {found && !browserFirst && (
            <div className={guided ? "provider-guide" : "provider-found"} data-provider={found.provider}>
              {!guided && <span className="strong">{found.name}</span>}
              {guided && (
                <ol className="guide">
                  {found.steps.map((step, i) => {
                    const [title, detail, withLink, ownLabel] = step.split("|");
                    // A step's own link, or the one shared link where the step says "link".
                    const href = withLink?.startsWith("https://") ? withLink : withLink ? found.link : undefined;
                    const label = withLink?.startsWith("https://") ? ownLabel : found.link_label;
                    const state = stepDone(i) ? "done" : i === current ? "current" : "next";
                    return (
                      <li key={step} className="guide-step" data-state={state}>
                        <span className="guide-number tnum">
                          {state === "done" ? <Icon icon={Tick02Icon} size="xs" /> : i + 1}
                        </span>
                        <div className="guide-words">
                          <span className="guide-title">{title}</span>
                          {detail && <span className="guide-detail">{detail}</span>}
                          {href && (
                            <a
                              className="btn btn-secondary btn-sm guide-link"
                              href={href}
                              target="_blank"
                              rel="noreferrer"
                              onClick={() => setOpened((list) => (list.includes(i) ? list : [...list, i]))}
                            >
                              {label}
                              <Icon icon={LinkSquare02Icon} size="xs" />
                            </a>
                          )}
                          {boxInSteps && i === lastStep && <div className="guide-box">{passwordBox}</div>}
                        </div>
                      </li>
                    );
                  })}
                </ol>
              )}
              {found.link && !browserFirst && !found.steps.some((s) => s.endsWith("|link")) && (
                <a className="btn btn-secondary btn-sm guide-link" href={found.link} target="_blank" rel="noreferrer">
                  {found.link_label}
                  <Icon icon={LinkSquare02Icon} size="xs" />
                </a>
              )}
              {guided && found.note && error && (
                <span className="guide-note">
                  <Icon icon={InformationCircleIcon} size="xs" />
                  {found.note}
                </span>
              )}
            </div>
          )}

          {found?.note && !guided && !browserFirst && (
            <div className="tip">
              <Icon icon={InformationCircleIcon} size="s" />
              <span>{found.note}</span>
            </div>
          )}

          {found && canConnect && (
            <>
              {!boxInSteps && (
                <label className="field">
                  <span className="field-label">{passwordLabel}</span>
                  {passwordBox}
                </label>
              )}
              {serverOffered && (
                <button type="button" className="disclosure" onClick={() => setShowServer((v) => !v)}>
                  <Icon icon={showServer ? ArrowDown01Icon : ArrowRight01Icon} size="xs" />
                  {tx("Server settings")}</button>
              )}
              {showServer && (
                <div className="stack">
                  <div className="settings-server">
                    <label className="field grow">
                      <span className="field-label">{tx("Mail server (IMAP)")}</span>
                      <input
                        placeholder={domain ? `imap.${domain}` : "imap.example.com"}
                        value={host}
                        onChange={(e) => setHost(e.target.value)}
                        autoCapitalize="off"
                        autoCorrect="off"
                        spellCheck={false}
                      />
                    </label>
                    <label className="field settings-port">
                      <span className="field-label">{tx("Port")}</span>
                      <input
                        required
                        inputMode="numeric"
                        value={port}
                        onChange={(e) => setPort(e.target.value)}
                      />
                    </label>
                  </div>
                  <label className="field">
                    <span className="field-label">
                      {tx("Username")}{" "}<span className="field-optional">{tx("if not your email address")}</span>
                    </span>
                    <input
                      value={username}
                      onChange={(e) => setUsername(e.target.value)}
                      autoCapitalize="off"
                      autoCorrect="off"
                      spellCheck={false}
                    />
                  </label>
                </div>
              )}
            </>
          )}

          <Problem text={error} />

          {browserFirst && found?.browser && (
            <>
              {inBrowser && (
                <div role="status" className="tip">
                  <Spinner size="xs" />
                  <span>{tx("Finish signing in on the page that opened in your browser.")}</span>
                </div>
              )}
              <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={inBrowser}>
                {tx("Continue with")}{" "}{BROWSER_NAME[found.browser] ?? found.name}
              </button>
              {inBrowser && (
                <button type="button" className="btn btn-ghost btn-block" onClick={() => void api.mailSignInCancel()}>
                  {tx("Stop waiting")}</button>
              )}
              {hasPasswordPath && !inBrowser && (
                <button type="button" className="link-btn" onClick={() => setUsePassword(true)}>
                  {tx("Use an app password instead")}</button>
              )}
            </>
          )}

          {!found && (
            <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
              {busy && <Spinner />}
              {busy ? tx("Finding your settings…") : "Continue"}
            </button>
          )}
          {found && canConnect && (
            <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
              {busy && <Spinner />}
              {busy ? "Connecting…" : tx("Connect inbox")}
            </button>
          )}
          {found && !canConnect && !found.browser && (
            <button type="button" className="btn btn-secondary btn-lg btn-block" onClick={() => { setEmail(""); startOver(); }}>
              {tx("Use another email")}</button>
          )}
          {onCancel && (
            <button type="button" className="btn btn-ghost btn-block" onClick={onCancel}>
              {tx("Cancel")}</button>
          )}
        </form>
        {children && <div className="welcome-foot">{children}</div>}
      </div>
    </div>
  );
}
