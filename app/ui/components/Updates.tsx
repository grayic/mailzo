import { useEffect, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { finishBeforeRestart, finishPanelBeforeRestart } from "../api";
import { Download04Icon } from "../icons";
import { useSettings } from "../settings";
import { Icon, LogoMark, Spinner } from "./ui";
import { tx } from "../i18n";

/** New versions, fetched and put in place from inside the app. Only the Mac app has them. */
const inMacApp = "__TAURI_INTERNALS__" in window;
const EVERY = 60 * 60 * 1000;
/** Coming back to the app looks again, but not more often than this. */
const ON_RETURN_AFTER = 15 * 60 * 1000;
/** "Later" puts the bar away for this long. The note in the sidebar stays. */
const LATER_FOR = 24 * 60 * 60 * 1000;
const LATER_KEY = "mailzo-update-later";
let lastLook = 0;

type Ready = { version: string; notes: string | null };
type State = {
  version: string | null;
  checking: boolean;
  ready: Ready | null;
  restarting: boolean;
  /** What happened when the person last asked, said in Settings. */
  said: string | null;
  /** Why the last Restart didn't put the update in place. The update stays ready to try again. */
  failed: string | null;
};

let state: State = { version: null, checking: false, ready: null, restarting: false, said: null, failed: null };
const listeners = new Set<() => void>();
const set = (next: Partial<State>) => {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
};
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

if (inMacApp) {
  void invoke<string>("app_version")
    .then((version) => set({ version }))
    .catch(() => {});
}

/** Looks for a newer version. `asked` is when the person pressed the button, so they're told the result. */
export async function checkForUpdate(channel: string, asked = false): Promise<void> {
  if (!inMacApp || state.checking || state.restarting) return;
  lastLook = Date.now();
  set({ checking: true, said: asked ? null : state.said });
  try {
    const ready = await invoke<Ready | null>("update_check", { channel });
    set({
      ready,
      said: asked ? (ready ? null : tx("You have the newest version.")) : state.said,
      // A fresh look at the update means the last failed install is old news.
      failed: ready ? null : state.failed,
    });
  } catch (e) {
    if (asked) set({ said: typeof e === "string" ? e : tx("Couldn't check for updates. Try again later.") });
  } finally {
    set({ checking: false });
  }
}

/** Longer than both windows can take to finish, so this only trips if something hangs. */
const FINISH_ALL_MS = 100_000;

/**
 * Sends what's waiting out its undo time, finishes moves still waiting and saves what's being
 * written, here and in the menu bar panel, and only then restarts into the new version. While a
 * message is still on its way to the mail server, nothing restarts.
 */
export async function restartToUpdate(channel: string): Promise<void> {
  if (state.restarting) return;
  set({ restarting: true, said: null, failed: null });
  try {
    const finished = await Promise.race([
      Promise.all([finishBeforeRestart(), finishPanelBeforeRestart()]),
      new Promise<"late">((done) => setTimeout(() => done("late"), FINISH_ALL_MS)),
    ]);
    const all = finished === "late" ? ["timeout"] : finished;
    if (all.includes("sending")) {
      set({ restarting: false, failed: tx("A message is still sending, so Mailzo didn't restart. Try again in a moment.") });
      return;
    }
    if (all.includes("timeout")) {
      set({ restarting: false, failed: tx("Couldn't finish what was going on. Try again in a moment.") });
      return;
    }
    await invoke("update_install", { channel });
  } catch (e) {
    const failed = typeof e === "string" ? e : tx("The update couldn't be put in place. Your current version still works.");
    // The update was for the other channel, or is gone, so there's nothing to try again.
    const gone = e === "There's no update ready yet." || e === OTHER_CHANNEL;
    set({ restarting: false, failed: gone ? null : failed, said: gone ? failed : state.said, ready: gone ? null : state.ready });
  }
}

/** What the app says when the update ready is for the channel no longer chosen. Matches updates.rs. */
const OTHER_CHANNEL = tx("That update was for the other channel, so it wasn't put in place. Check for updates to get this one's.");

/** The channel last checked, so a change can throw away an update ready for the other one. */
let lastChannel: string | null = null;

export function useUpdates(): State {
  return useSyncExternalStore(subscribe, () => state);
}

/** Checks when the app opens, every hour, and on coming back to it, if the person leaves automatic updates on. */
export function useAutomaticUpdates() {
  const { settings } = useSettings();
  const on = settings.auto_update !== "off";
  const channel = settings.update_channel ?? "stable";
  useEffect(() => {
    if (!inMacApp) return;
    // Switching channel throws away an update downloaded for the other one, here and in the app.
    if (lastChannel !== null && lastChannel !== channel) {
      set({ ready: null, failed: null });
      void invoke("update_forget").catch(() => {});
    }
    lastChannel = channel;
  }, [channel]);
  useEffect(() => {
    if (!inMacApp || !on) return;
    const first = setTimeout(() => void checkForUpdate(channel), 20_000);
    const again = setInterval(() => void checkForUpdate(channel), EVERY);
    const back = () => {
      if (document.visibilityState === "visible" && Date.now() - lastLook > ON_RETURN_AFTER) void checkForUpdate(channel);
    };
    window.addEventListener("focus", back);
    document.addEventListener("visibilitychange", back);
    return () => {
      clearTimeout(first);
      clearInterval(again);
      window.removeEventListener("focus", back);
      document.removeEventListener("visibilitychange", back);
    };
  }, [on, channel]);
}

/** A small note at the bottom of the sidebar once a new version is downloaded. Never restarts by itself. */
export function UpdateNote({ rail }: { rail: boolean }) {
  const { ready, restarting, failed } = useUpdates();
  const { settings } = useSettings();
  const channel = settings.update_channel ?? "stable";
  if (!ready) return null;
  const tip = failed ?? tx("Mailzo {0} is ready. Restart to use it. Your drafts are kept.", { "0": ready.version });
  if (rail)
    return (
      <button
        type="button"
        className="update-note update-note-rail"
        aria-label={tip}
        data-tip={tip}
        disabled={restarting}
        onClick={() => void restartToUpdate(channel)}
      >
        {restarting ? <Spinner size="xs" /> : <Icon icon={Download04Icon} size="s" />}
      </button>
    );
  return (
    <div className="update-note" role="status">
      <Icon icon={Download04Icon} size="s" />
      <span className="grow truncate-1" data-tip={failed ?? undefined}>
        {failed ? tx("Update didn't install") : tx("Update ready")}
      </span>
      <button
        type="button"
        className="btn btn-primary btn-sm"
        data-tip={tip}
        disabled={restarting}
        onClick={() => void restartToUpdate(channel)}
      >
        {restarting && <Spinner size="xs" />}
        {failed ? tx("Try again") : "Restart"}
      </button>
    </div>
  );
}

/** When "Later" was pressed for which version, kept on this Mac only. */
function laterFor(version: string): boolean {
  try {
    const saved = JSON.parse(localStorage.getItem(LATER_KEY) ?? "null") as { version: string; until: number } | null;
    return !!saved && saved.version === version && Date.now() < saved.until;
  } catch {
    return false;
  }
}

/**
 * A bar across the top once a new version is downloaded, so it isn't missed. Restart is one
 * press; Later puts it away for a day (the sidebar note stays). Never restarts by itself.
 */
export function UpdateBar() {
  const { ready, restarting, failed } = useUpdates();
  const { settings } = useSettings();
  const channel = settings.update_channel ?? "stable";
  const [, redraw] = useState(0);
  if (!ready || (laterFor(ready.version) && !failed)) return null;
  const later = () => {
    try {
      localStorage.setItem(LATER_KEY, JSON.stringify({ version: ready.version, until: Date.now() + LATER_FOR }));
    } catch {
      // Without storage it simply shows again next time.
    }
    redraw((n) => n + 1);
  };
  return (
    <div role="status" className="update-bar" data-failed={failed ? true : undefined}>
      <span className="update-bar-icon">
        <LogoMark />
        <span className="update-bar-dot" />
      </span>
      <span className="update-bar-words">
        <span className="update-bar-title">
          {failed ? tx("Update didn't install") : tx("Mailzo {0} is ready", { "0": ready.version })}
        </span>
        <span className="update-bar-text">
          {failed ?? (ready.notes?.trim() || tx("Restart takes a few seconds. Your drafts are kept."))}
        </span>
      </span>
      <span className="update-bar-actions">
        {!restarting && !failed && (
          <button type="button" className="btn btn-ghost btn-sm" onClick={later}>
            {tx("Later")}</button>
        )}
        <button
          type="button"
          className="btn btn-primary btn-sm"
          data-tip={tx("Your drafts are kept.")}
          disabled={restarting}
          onClick={() => void restartToUpdate(channel)}
        >
          {restarting && <Spinner size="xs" />}
          {failed ? tx("Try again") : tx("Restart")}
        </button>
      </span>
    </div>
  );
}
