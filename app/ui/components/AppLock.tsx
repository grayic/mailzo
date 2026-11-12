import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, appLocked, hearFromPanel, unlockApp } from "../api";
import { Logo, Problem, Spinner } from "./ui";
import { tx } from "../i18n";

/**
 * Shows the mail only while the app is unlocked. The lock itself is kept by the app, which
 * refuses to show any mail while it's on, locks at start when locking is turned on, and
 * locks again after time away, sleep or the screen locking. This only follows it.
 */
export function LockGate({ children }: { children: React.ReactNode }) {
  const [locked, setLocked] = useState<boolean | null>(null);
  const check = useCallback(() => {
    void appLocked().then(setLocked);
  }, []);
  useEffect(() => {
    check();
    return hearFromPanel("lock-changed", check);
  }, [check]);

  if (locked === null) return null;
  if (locked) return <LockScreen onUnlocked={check} />;
  return <>{children}</>;
}

function LockScreen({ onUnlocked }: { onUnlocked: () => void }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const asked = useRef(false);

  const unlock = useCallback(async () => {
    setBusy(true);
    setProblem(null);
    try {
      if (await unlockApp("unlock Mailzo")) onUnlocked();
    } catch (e) {
      setProblem(e instanceof ApiError ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [onUnlocked]);

  // Asks once by itself, as the window opens.
  useEffect(() => {
    if (asked.current) return;
    asked.current = true;
    void unlock();
  }, [unlock]);

  return (
    <div className="welcome">
      <div className="welcome-drag" data-tauri-drag-region />
      <div className="welcome-inner">
        <div className="welcome-card stack">
          <Logo />
          <div>
            <h1 className="welcome-title">{tx("Mailzo is locked")}</h1>
            <p className="welcome-text">{tx("Use Touch ID or your Mac's password to open your mail.")}</p>
          </div>
          <Problem text={problem} />
          <button className="btn btn-primary btn-lg btn-block" disabled={busy} onClick={() => void unlock()}>
            {busy && <Spinner />}
            {tx("Unlock")}</button>
        </div>
      </div>
    </div>
  );
}
