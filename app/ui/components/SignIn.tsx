import { FormEvent, useEffect, useRef, useState } from "react";
import { ApiError, api, type AuthStatus } from "../api";
import type { Account } from "../types";
import { Clock01Icon, Key01Icon, SecurityCheckIcon } from "../icons";
import { AddAccount } from "./AddAccount";
import { RecoveryKey } from "./RecoveryKey";
import { Avatar, Confirm, GoogleMark, Icon, Logo, Problem, Spinner } from "./ui";
import { language, tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

const CHECK_LINK_EVERY_MS = 2_000;
const CHECK_FOR_CODE_EVERY_MS = 3_000;

function Screen({
  children,
  logo = true,
  foot,
}: {
  children: React.ReactNode;
  logo?: boolean;
  foot?: React.ReactNode;
}) {
  return (
    <div className="welcome">
      <div className="welcome-drag" data-tauri-drag-region />
      <div className="welcome-inner">
        <div className="welcome-card stack">
          {logo && <Logo />}
          {children}
        </div>
        {foot && <div className="welcome-foot">{foot}</div>}
      </div>
    </div>
  );
}

/** Hidden until the account server has Google set up. */
function GoogleButton({ onDone }: { onDone: () => void }) {
  const [available, setAvailable] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .authOptions()
      .then((o) => live && setAvailable(o.google))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  const start = async () => {
    setWaiting(true);
    setError(null);
    try {
      await api.googleSignIn();
      onDone();
    } catch (err) {
      if (!(err instanceof ApiError && err.code === "cancelled")) setError(say(err));
      setWaiting(false);
    }
  };

  if (!available) return null;
  return (
    <div className="stack">
      {waiting ? (
        <div className="tip">
          <Spinner />
          <span className="grow">{tx("Finish signing in with Google in your browser.")}</span>
          <button type="button" className="link-btn" onClick={() => void api.googleCancel()}>
            {tx("Cancel")}</button>
        </div>
      ) : (
        <button type="button" className="btn btn-secondary btn-lg btn-block" onClick={() => void start()}>
          <GoogleMark />
          {tx("Continue with Google")}</button>
      )}
      <Problem text={error} />
      <div className="or">or</div>
    </div>
  );
}

function EmailStep({
  onSent,
  onSignedIn,
  onBack,
  backLabel,
}: {
  onSent: (email: string) => void;
  onSignedIn: () => void;
  onBack?: () => void;
  backLabel?: string;
}) {
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.authStart(email);
      onSent(email);
    } catch (err) {
      setError(say(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Screen>
      <div>
        <h1 className="welcome-title">{tx("Sign in to Mailzo")}</h1>
        <p className="welcome-text">
          {tx("One account keeps all your inboxes together on your devices. There is no password to remember.")}</p>
      </div>
      <GoogleButton onDone={onSignedIn} />
      <form onSubmit={submit} className="stack">
        <label className="field">
          <span className="field-label">{tx("Email address")}</span>
          <input
            type="email"
            required
            autoFocus
            placeholder={tx("you@company.com")}
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoComplete="off"
          />
        </label>
        <p className="fine">{tx("We'll email you a code. Any address works, but one only you read is best.")}</p>
        <Problem text={error} />
        <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
          {busy && <Spinner />}
          {busy ? "Sending…" : tx("Email me a code")}
        </button>
        {onBack && (
          <button type="button" className="btn btn-ghost btn-block" onClick={onBack}>
            {backLabel ?? "Back"}
          </button>
        )}
      </form>
    </Screen>
  );
}

function CodeStep({
  email,
  readInbox,
  onBack,
  onVerified,
}: {
  email: string;
  /** This address is connected here, so the app can find the code by itself. */
  readInbox: boolean;
  onBack: () => void;
  onVerified: () => void;
}) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [resent, setResent] = useState(false);
  const [resending, setResending] = useState(false);
  const tried = useRef(new Set<string>());
  const verifying = useRef(false);

  const verify = async (value: string, quietly = false) => {
    if (verifying.current) return;
    verifying.current = true;
    setBusy(true);
    if (!quietly) setError(null);
    try {
      await api.authVerify(email, value);
      onVerified();
    } catch (err) {
      // Nobody typed a code found in the inbox, so there's nothing for them to put right.
      if (!quietly) setError(say(err));
      setBusy(false);
    } finally {
      verifying.current = false;
    }
  };

  useEffect(() => {
    if (!readInbox) return;
    let live = true;
    const look = async () => {
      if (verifying.current) return;
      try {
        const { code: found } = await api.findSigninCode(email);
        if (!live || !found || tried.current.has(found)) return;
        tried.current.add(found);
        await verify(found, true);
      } catch {
        // The inbox can't be reached right now. Typing the code still works.
      }
    };
    void look();
    const timer = setInterval(() => void look(), CHECK_FOR_CODE_EVERY_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [email, readInbox]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    tried.current.add(code.trim());
    void verify(code);
  };

  const resend = async () => {
    // One new code at a time, or the first one emailed stops working.
    if (resending) return;
    setResending(true);
    setError(null);
    setResent(false);
    try {
      await api.authStart(email);
      setResent(true);
    } catch (err) {
      setError(say(err));
    } finally {
      setResending(false);
    }
  };

  return (
    <Screen>
      <form onSubmit={submit} className="stack">
        <div>
          <h1 className="welcome-title">{tx("Check your email")}</h1>
          <p className="welcome-text">{tx("We sent a 6-digit code to")}{" "}{email}.</p>
        </div>
        <input
          required
          autoFocus
          aria-label={tx("Code")}
          className="code-input"
          placeholder="000000"
          inputMode="numeric"
          autoComplete="one-time-code"
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        <Problem text={error} />
        {resent && <p className="fine">{tx("We sent a new code.")}</p>}
        <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
          {busy && <Spinner />}
          {busy ? "Checking…" : "Continue"}
        </button>
        <div className="hstack-m center">
          <button type="button" className="btn btn-ghost btn-sm" disabled={resending} onClick={() => void resend()}>
            {resending && <Spinner size="xs" />}
            {tx("Send again")}</button>
          <button type="button" className="btn btn-ghost btn-sm" onClick={onBack}>
            {tx("Change email")}</button>
        </div>
      </form>
    </Screen>
  );
}

/** Starts the optional account from inside the app, suggesting the inbox already here. */
function MainEmailStep({
  email,
  onYes,
  onSignedIn,
  onOther,
  onNotNow,
}: {
  email: string;
  onYes: () => void;
  /** Signing in to the inbox already proved the address, so there's no code to wait for. */
  onSignedIn: () => void;
  onOther: () => void;
  onNotNow?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const yes = async () => {
    setBusy(true);
    setError(null);
    try {
      if ((await api.authWithMailSignIn(email)).signed_in) {
        onSignedIn();
        return;
      }
      await api.authStart(email);
      onYes();
    } catch (err) {
      setError(say(err));
      setBusy(false);
    }
  };

  return (
    <Screen>
      <div>
        <h1 className="welcome-title">{tx("Use Mailzo on your other devices")}</h1>
        <p className="welcome-text">
          {tx("A free Mailzo account brings your inboxes and settings to your other devices, and lets you work with a team. They're locked on this device first, so not even Mailzo can read them.")}</p>
      </div>
      <div className="group">
        <div className="group-row">
          <Avatar name={email} />
          <span className="grow strong truncate-1">{email}</span>
        </div>
      </div>
      <p className="fine">
        {tx("There's no password: you sign in with this email. If other people read this inbox, one only you read is a better choice.")}</p>
      <Problem text={error} />
      <button className="btn btn-primary btn-lg btn-block" disabled={busy} onClick={() => void yes()}>
        {busy && <Spinner />}
        {busy ? tx("One moment…") : tx("Continue with this email")}
      </button>
      <button className="btn btn-ghost btn-block" disabled={busy} onClick={onOther}>
        {tx("Use a different email")}</button>
      {onNotNow && (
        <button className="btn btn-ghost btn-block" disabled={busy} onClick={onNotNow}>
          {tx("Not now")}</button>
      )}
    </Screen>
  );
}

/** First device: creates the account's vault and shows the recovery key once. */
function SetupStep({ onDone }: { onDone: () => void }) {
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);

  const run = () => {
    setError(null);
    api
      .finishSetup()
      .then((r) => {
        if (r.recovery_key) setRecoveryKey(r.recovery_key);
        else onDone();
      })
      .catch((e) => setError(say(e)));
  };

  // Only once, because each run makes a new key.
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (recoveryKey) return <RecoveryKey value={recoveryKey} onDone={onDone} />;
  return (
    <Screen>
      {error ? (
        <>
          <Problem text={error} />
          <button className="btn btn-primary btn-lg btn-block" onClick={run}>
            {tx("Try again")}</button>
        </>
      ) : (
        <div className="hstack-m center muted">
          <Spinner />
          {tx("Setting up your account…")}</div>
      )}
    </Screen>
  );
}

/** A further device: waits for approval, or takes the recovery key instead. */
function LinkStep({ status, onDone }: { status: AuthStatus; onDone: () => void }) {
  const [loaded, setLoaded] = useState(false);
  const [number, setNumber] = useState<string | null>(null);
  const [others, setOthers] = useState(1);
  const [freshAfter, setFreshAfter] = useState(0);
  const [useRecovery, setUseRecovery] = useState(false);
  const [recovery, setRecovery] = useState("");
  const [confirmFresh, setConfirmFresh] = useState(false);
  /** Starting fresh was booked, or already was: when it's due, if known. */
  const [freshBooked, setFreshBooked] = useState<{ waiting: boolean; due: number | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const check = async () => {
      try {
        const r = await api.linkStatus();
        if (!live) return;
        if (r.state !== "linking") onDone();
        else {
          setNumber(r.number ?? null);
          setOthers(r.other_devices ?? 1);
          setFreshAfter(r.fresh_after ?? 0);
        }
      } catch {
        // Offline for a moment. The next check will try again.
      }
      if (live) setLoaded(true);
    };
    void check();
    const timer = setInterval(() => void check(), CHECK_LINK_EVERY_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [onDone]);

  const submitRecovery = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.linkWithRecovery(recovery);
      onDone();
    } catch (err) {
      setError(say(err));
      setBusy(false);
    }
  };

  const startFresh = async () => {
    setConfirmFresh(false);
    setBusy(true);
    setError(null);
    try {
      const r = await api.startFresh();
      if (r.state === "fresh_booked" || r.state === "fresh_waiting") {
        setFreshBooked({ waiting: r.state === "fresh_waiting", due: r.due_at ?? null });
        setBusy(false);
        return;
      }
      onDone();
    } catch (err) {
      setError(say(err));
      setBusy(false);
    }
  };

  const cancel = async () => {
    try {
      await api.signOut();
      onDone();
    } catch (err) {
      setError(say(err));
    }
  };

  if (!loaded) {
    return (
      <Screen>
        <div className="hstack-m center muted">
          <Spinner />
          {tx("Signing in…")}</div>
      </Screen>
    );
  }

  if (freshBooked) {
    const hoursLeft = freshBooked.due ? Math.max(1, Math.ceil((freshBooked.due * 1000 - Date.now()) / 3_600_000)) : null;
    return (
      <Screen logo={false}>
        <div>
          <div className="empty-icon">
            <Icon icon={Clock01Icon} size="l" />
          </div>
          <h1 className="welcome-title">{tx("Start fresh is booked")}</h1>
          <p className="welcome-text">
            {!freshBooked.waiting
              ? tx("To keep your account safe, it happens in 48 hours unless you stop it from the email we sent. Come back then and tap Start fresh again.")
              : hoursLeft
                ? tx("To keep your account safe, it happens in about {hours} hours unless you stop it from the email we sent. Come back then and tap Start fresh again.", { hours: String(hoursLeft) })
                : tx("To keep your account safe, it happens 48 hours after you first asked, unless you stop it from the email we sent. Come back then and tap Start fresh again.")}
          </p>
        </div>
        <button className="btn btn-primary btn-block" onClick={() => setFreshBooked(null)}>
          {tx("Go back")}</button>
      </Screen>
    );
  }

  const onlyTheKey = others === 0;
  const canStartFresh = freshAfter * 1000 <= Date.now();
  const showKey = onlyTheKey || useRecovery;

  return (
    <Screen logo={false}>
      <div>
        <div className="empty-icon">
          <Icon icon={onlyTheKey ? Key01Icon : SecurityCheckIcon} size="l" />
        </div>
        <h1 className="welcome-title">{onlyTheKey ? tx("Welcome back") : tx("Allow this device")}</h1>
        <p className="welcome-text">
          {onlyTheKey
            ? tx("Signed in as {0}. Enter your recovery key and all your inboxes will appear here.", { "0": status.email })
            : tx("Open Mailzo on a device you already use and tap Allow. Your inboxes then appear here.")}
        </p>
      </div>
      {!onlyTheKey && (
        <>
          <p className="fine">{tx("Your other device shows the same number, so you know it's this one.")}</p>
          <div aria-label={tx("Number your other device shows")} className="selectable number-box">
            {number ?? "······"}
          </div>
          {!number && (
            <div className="hstack-m center muted">
              <Spinner />
              {tx("Waiting for one of your devices to come online…")}</div>
          )}
        </>
      )}

      {showKey ? (
        <form onSubmit={submitRecovery} className="stack">
          <label className="field">
            <span className="field-label">{tx("Recovery key")}</span>
            <input
              required
              autoFocus
              value={recovery}
              onChange={(e) => setRecovery(e.target.value)}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
            />
          </label>
          <button type="submit" className="btn btn-primary btn-block" disabled={busy}>
            {busy ? "Checking…" : tx("Use recovery key")}
          </button>
        </form>
      ) : (
        <button className="btn btn-ghost btn-block" onClick={() => setUseRecovery(true)}>
          {tx("No other device? Use your recovery key")}</button>
      )}
      <Problem text={error} />
      {showKey &&
        (canStartFresh ? (
          <button className="btn btn-ghost btn-block" disabled={busy} onClick={() => setConfirmFresh(true)}>
            {tx("Lost your key? Start fresh")}</button>
        ) : (
          <p className="fine">
            {tx("Lost your key? Because another of your devices was used recently, you can start fresh from {date}.", {
              date: new Date(freshAfter * 1000).toLocaleDateString(language, { day: "numeric", month: "long" }),
            })}
          </p>
        ))}
      <button className="btn btn-ghost btn-block" onClick={() => void cancel()}>
        {tx("Cancel")}</button>
      {confirmFresh && (
        <Confirm
          title={tx("Start fresh?")}
          confirm={tx("Yes, start fresh")}
          keep={tx("Go back")}
          danger
          onKeep={() => setConfirmFresh(false)}
          onConfirm={() => void startFresh()}
        >
          {tx("This permanently erases your saved list of inboxes and your settings. Your email isn't touched, because it stays with your email provider. You'll reconnect each inbox with its password and get a new recovery key. Your old key will stop working, and any other devices on this account will be removed.")}</Confirm>
      )}
    </Screen>
  );
}

/**
 * No account. The first time: connect an inbox, and the mail opens with no account at all.
 * From inside the app (an inbox already here): start the optional account.
 */
function GetStarted({ onChanged, onNotNow }: { onChanged: () => void; onNotNow?: () => void }) {
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [view, setView] = useState<"start" | "signin">("start");
  const [codeEmail, setCodeEmail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .listAccounts()
      .then(setAccounts)
      .catch((e) => setError(say(e)));
  }, []);

  if (error) {
    return (
      <Screen>
        <Problem text={error} />
      </Screen>
    );
  }
  if (!accounts) return null;

  const connected = accounts[0]?.email ?? null;

  if (codeEmail) {
    return (
      <CodeStep
        email={codeEmail}
        readInbox={accounts.some((a) => a.email.toLowerCase() === codeEmail.trim().toLowerCase())}
        onBack={() => {
          setCodeEmail(null);
          setView(connected ? "start" : "signin");
        }}
        onVerified={onChanged}
      />
    );
  }

  if (view === "signin") {
    return (
      <EmailStep
        onSent={setCodeEmail}
        onSignedIn={onChanged}
        onBack={() => setView("start")}
      />
    );
  }

  if (connected) {
    return (
      <MainEmailStep
        email={connected}
        onYes={() => setCodeEmail(connected)}
        onSignedIn={onChanged}
        onOther={() => setView("signin")}
        onNotNow={onNotNow}
      />
    );
  }

  return (
    <AddAccount
      title={tx("Welcome to Mailzo")}
      intro={tx("Connect the inbox you use and your mail opens right away. It stays on this device, and there's no account to make.")}
      onAdded={(a) => {
        // Fills in the background while the mail opens.
        void api.sync(a.id).catch(() => {});
        onChanged();
      }}
    >
      <button type="button" className="link-btn" onClick={() => setView("signin")}>
        {tx("Already have an account? Sign in")}</button>
    </AddAccount>
  );
}

export function SignIn({
  status,
  onChanged,
  onNotNow,
}: {
  status: AuthStatus;
  onChanged: () => void;
  /** Back to the mail, when setting up an account from inside the app. */
  onNotNow?: () => void;
}) {
  if (status.state === "needs_setup") return <SetupStep onDone={onChanged} />;
  if (status.state === "linking") return <LinkStep status={status} onDone={onChanged} />;
  return <GetStarted onChanged={onChanged} onNotNow={onNotNow} />;
}
