import { createContext, ReactNode, useCallback, useContext, useEffect, useRef, useState } from "react";
import { api, hearFromPanel, native } from "./api";
import { setReadingSize } from "./prepareEmail";

export type Settings = Record<string, string>;

const SettingsContext = createContext<{
  settings: Settings;
  change: (key: string, value: string) => void;
  /** Reads them again, after another device may have changed them. */
  reload: () => void;
  /** Resolves once the latest choice is saved, for anything that reads it back. */
  whenSaved: () => Promise<unknown>;
}>({ settings: {}, change: () => {}, reload: () => {}, whenSaved: () => Promise.resolve() });

export const useSettings = () => useContext(SettingsContext);

function applyToPage(s: Settings) {
  const root = document.documentElement;
  if (s.theme === "light" || s.theme === "dark") root.dataset.theme = s.theme;
  else delete root.dataset.theme;
  root.dataset.density = s.density ?? "comfortable";
  root.dataset.preview = s.preview_lines ?? "1";
  root.dataset.reading = s.reading_size ?? "normal";
  setReadingSize(s.reading_size ?? "normal");
  void native("set_window_theme", { theme: s.theme ?? "system" });
}

/** Nothing shows until these are read, so the app never flashes the wrong theme. */
export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const saving = useRef<Promise<unknown>>(Promise.resolve());

  const reload = useCallback(() => {
    api
      .appSettings()
      .then((s) => {
        applyToPage(s);
        setSettings(s);
      })
      .catch(() => setSettings((current) => current ?? {}));
  }, []);

  useEffect(reload, [reload]);
  // The app can change one itself, such as when it's taken out of the Dock.
  useEffect(() => hearFromPanel("settings-changed", reload), [reload]);

  const change = useCallback(
    (key: string, value: string) => {
      setSettings((current) => {
        const next = { ...(current ?? {}), [key]: value };
        applyToPage(next);
        return next;
      });
      // If it can't be saved, show the truth again.
      saving.current = api.setSetting(key, value).catch(reload);
    },
    [reload],
  );

  const whenSaved = useCallback(() => saving.current, []);

  if (!settings) return null;
  return (
    <SettingsContext.Provider value={{ settings, change, reload, whenSaved }}>
      {children}
    </SettingsContext.Provider>
  );
}
