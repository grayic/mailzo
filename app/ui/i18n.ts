// The app's words in the person's language. Every sentence on screen goes through tx(),
// written in English; each language has a file mapping that English to its own words.
// Anything not translated yet stays in English, so nothing ever shows blank.
import de from "./locales/de.json";
import es from "./locales/es.json";
import fr from "./locales/fr.json";
import ja from "./locales/ja.json";
import ko from "./locales/ko.json";
import pt from "./locales/pt.json";
import zh from "./locales/zh.json";

type Words = Record<string, string>;

/** The languages Mailzo comes in, by code, with each one's own name for itself. */
export const LANGUAGES: { code: string; name: string }[] = [
  { code: "en", name: "English" },
  { code: "de", name: "Deutsch" },
  { code: "es", name: "Español" },
  { code: "fr", name: "Français" },
  { code: "pt", name: "Português" },
  { code: "ja", name: "日本語" },
  { code: "ko", name: "한국어" },
  { code: "zh", name: "中文" },
];

const ALL: Record<string, Words> = { de, es, fr, ja, ko, pt, zh };

/** Kept on this device, so the right words show from the very first screen. */
const CHOSEN = "dakly-language";

/** "auto" follows the Mac; anything else is a language the person picked. */
export function chosenLanguage(): string {
  try {
    return localStorage.getItem(CHOSEN) ?? "auto";
  } catch {
    return "auto";
  }
}

/** Saves the choice. The screens are drawn again in it straight after. */
export function chooseLanguage(code: string) {
  try {
    if (code === "auto") localStorage.removeItem(CHOSEN);
    else localStorage.setItem(CHOSEN, code);
  } catch {
    // It falls back to following the Mac.
  }
}

function pick(): string {
  const chosen = chosenLanguage();
  if (chosen !== "auto") return chosen;
  for (const tag of navigator.languages ?? [navigator.language]) {
    const code = tag.toLowerCase().split("-")[0];
    if (code === "en" || code in ALL) return code;
  }
  return "en";
}

/** The language showing now, decided once as the app starts. */
export const language = pick();
const words: Words = ALL[language] ?? {};

/**
 * The person's own words for an English sentence. {name} or {0} in it are filled from
 * `fill`, after translating, so each language can put them where its grammar needs.
 */
export function tx(english: string, fill?: Record<string, unknown>): string {
  let out = words[english] || english;
  if (fill) {
    for (const [key, value] of Object.entries(fill)) out = out.split(`{${key}}`).join(String(value));
  }
  return out;
}

/** Built-in folders in the person's language. Folders they made keep their own names. */
const FOLDER_NAMES: Record<string, string> = {
  inbox: "Inbox",
  sent: "Sent",
  drafts: "Drafts",
  archive: "Archive",
  junk: "Spam",
  trash: "Trash",
};

export function folderLabel(folder: { role: string; name: string }): string {
  const english = FOLDER_NAMES[folder.role];
  if (!english) return folder.name;
  // "Archive" is also a button, so the folder has its own entry: the noun, not the verb.
  if (folder.role === "archive") {
    const named = tx("folder:Archive");
    return named === "folder:Archive" ? english : named;
  }
  return tx(english);
}
