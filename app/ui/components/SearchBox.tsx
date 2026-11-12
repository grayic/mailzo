import { RefObject, useLayoutEffect } from "react";
import { AiMagicIcon, Cancel01Icon, Search01Icon } from "../icons";
import { Icon, IconButton, Spinner } from "./ui";
import { tx } from "../i18n";

export function SearchBox({
  value,
  busy,
  aiOn,
  focusOnShow,
  inputRef,
  onChange,
  onSubmit,
  onClear,
}: {
  value: string;
  busy: boolean;
  /** AI will read what's typed, so a whole sentence works. */
  aiOn: boolean;
  /** Keeps typing going when the list underneath changes. */
  focusOnShow: boolean;
  inputRef: RefObject<HTMLInputElement | null>;
  onChange: (text: string) => void;
  onSubmit: () => void;
  onClear: () => void;
}) {
  useLayoutEffect(() => {
    if (focusOnShow) inputRef.current?.focus();
    // Only on arriving, not while typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <form
      role="search"
      className="search"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit();
      }}
    >
      <Icon icon={Search01Icon} size="s" />
      <input
        ref={inputRef}
        type="search"
        aria-label={tx("Search all email")}
        placeholder={aiOn ? tx("Search, or describe the email") : tx("Search all email")}
        value={value}
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && value) {
            e.stopPropagation();
            onClear();
          }
        }}
      />
      {busy && <Spinner size="xs" />}
      {value ? (
        <IconButton icon={Cancel01Icon} label={tx("Clear search")} small onClick={onClear} />
      ) : (
        <span className="kbd-quiet">⌘F</span>
      )}
    </form>
  );
}

/** How the search was understood, and how it's getting on. */
/** What the AI made of the search, right above the results. */
export function SearchNote({
  summary,
  aiThinking,
  problem,
  onExactWords,
}: {
  summary: string | null;
  aiThinking: boolean;
  problem: string | null;
  onExactWords: () => void;
}) {
  if (!summary && !aiThinking && !problem) return null;
  return (
    <div className="search-note" aria-live="polite">
      {aiThinking ? (
        <div className="search-ai">
          <Spinner size="xs" />
          <span className="grow">{tx("Understanding your search…")}</span>
        </div>
      ) : summary ? (
        <div className="search-ai">
          <Icon icon={AiMagicIcon} size="xs" />
          <span className="grow">{summary}</span>
          <IconButton icon={Cancel01Icon} label={tx("Use my exact words")} small onClick={onExactWords} />
        </div>
      ) : null}
      {problem && <div className="search-problem">{tx("The AI didn't help this time:")}{" "}{problem}</div>}
    </div>
  );
}

/** The quieter things about a search, after the results. */
export function SearchFoot({
  checking,
  unreached,
  inBin,
  byMeaning = 0,
  onShowBin,
}: {
  checking: boolean;
  unreached: string[];
  inBin: number;
  /** How many of these came from searching by meaning. */
  byMeaning?: number;
  onShowBin: () => void;
}) {
  if (!checking && unreached.length === 0 && inBin === 0 && !byMeaning) return null;
  return (
    <div className="search-foot" aria-live="polite">
      {byMeaning > 0 && <div>{byMeaning === 1 ? tx("Includes 1 about the same thing.") : tx("Includes {n} about the same thing.", { n: byMeaning })}</div>}
      {checking && (
        <div className="search-line">
          <Spinner size="xs" />
          {tx("Checking your mail servers…")}</div>
      )}
      {unreached.length > 0 && <div>{tx("Couldn't check older email in")}{" "}{unreached.join(", ")}.</div>}
      {inBin > 0 && (
        <div className="search-line">
          <span className="grow">{inBin} {" "}{tx("more in Trash or Spam")}</span>
          <button type="button" className="link-btn" onClick={onShowBin}>
            {tx("Show them")}</button>
        </div>
      )}
    </div>
  );
}
