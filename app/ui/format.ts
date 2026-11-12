import { language, tx } from "./i18n";
export function shortDate(unix: number): string {
  const d = new Date(unix * 1000);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString(language, { hour: "numeric", minute: "2-digit" });
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString(language, { month: "short", day: "numeric" });
  }
  return d.toLocaleDateString(language, { year: "numeric", month: "short", day: "numeric" });
}

export function fullDate(unix: number): string {
  return new Date(unix * 1000).toLocaleString(language, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** How long ago something happened, to go in the middle of a sentence. */
export function timeAgo(unix: number): string {
  const minutes = Math.floor((Date.now() / 1000 - unix) / 60);
  if (minutes < 1) return "just now";
  if (minutes < 60) return tx("{minutes} min ago", { minutes });
  return new Date(unix * 1000).toLocaleString(language, { dateStyle: "medium", timeStyle: "short" });
}

/** The language this Mac is set to, in its own words. */
export function myLanguage(): string {
  try {
    const code = navigator.language.split("-")[0];
    return new Intl.DisplayNames(["en"], { type: "language" }).of(code) ?? "English";
  } catch {
    return "English";
  }
}
