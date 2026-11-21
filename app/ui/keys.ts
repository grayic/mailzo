import { onTheWeb } from "./api";

/** On a Mac the shortcuts use ⌘; in a browser on Windows or Linux, Ctrl. */
const onMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** Shortcuts a browser keeps for itself (new window, switch tab, reload, its own settings,
 *  back and forward), so on the web these go with ⌥ (Alt) instead. */
const BROWSER_KEEPS = new Set(["n", "r", ",", "[", "]", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);

/** The key of the browser's own key code, as ⌥ changes the letter a Mac types (⌥N types ˜). */
function plainKey(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  return { Comma: ",", BracketLeft: "[", BracketRight: "]" }[code] ?? null;
}

/** The key pressed with ⌘ (Ctrl in a browser off a Mac, ⌥ for the ones a browser keeps),
 *  or null when this isn't one of the app's shortcuts. */
export function shortcutKey(e: KeyboardEvent): string | null {
  if (!onTheWeb) return e.metaKey && !e.altKey ? e.key : null;
  const primary = onMac ? e.metaKey : e.ctrlKey;
  const other = onMac ? e.ctrlKey : e.metaKey;
  if (other) return null;
  if (primary && !e.altKey) return BROWSER_KEEPS.has(e.key.toLowerCase()) ? null : e.key;
  if (e.altKey && !primary) {
    // While typing, ⌥ makes letters (“ ® and, on many layouts, [ and ]), so it's left to type.
    if ((document.activeElement as HTMLElement | null)?.closest("input, textarea, select, [contenteditable]")) return null;
    const key = plainKey(e.code);
    return key && BROWSER_KEEPS.has(key) ? key : null;
  }
  return null;
}

/** A shortcut as it's shown, written the Mac way ("⌘N"), as it works where this runs. */
export function keys(label: string): string {
  if (!onTheWeb) return label;
  return label.replace(/⌘(.)/gu, (_, key: string) => {
    if (BROWSER_KEEPS.has(key.toLowerCase())) return (onMac ? "⌥" : "Alt+") + key;
    return onMac ? `⌘${key}` : key === " " ? "Ctrl " : `Ctrl+${key}`;
  });
}
