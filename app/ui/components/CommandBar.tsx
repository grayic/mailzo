import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Search01Icon } from "../icons";
import { Icon } from "./ui";
import { tx } from "../i18n";

export type Command = {
  id: string;
  label: string;
  /** Shown at the end, if there's a key that does the same thing. */
  keys?: string;
  section: string;
  run: () => void;
  /** Shown switched off, with this said beside it. */
  locked?: string;
};

/** Type a few letters of anything the app can do. */
export function CommandBar({
  commands,
  onSearch,
  onAsk,
  askLocked = false,
  onClose,
}: {
  commands: Command[];
  onSearch: (text: string) => void;
  /** Left out when asking isn't possible. */
  onAsk?: (text: string) => void;
  /** Asking comes with Mailzo Cloud, which isn't here: shown switched off, saying so. */
  askLocked?: boolean;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [at, setAt] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);

  const list = useMemo(() => {
    const wanted = text.trim().toLowerCase();
    const found = commands.filter((c) => {
      if (!wanted) return true;
      const label = c.label.toLowerCase();
      return wanted.split(/\s+/).every((w) => label.includes(w));
    });
    if (!wanted) return found;
    const extra: Command[] = [
      {
        id: "search",
        label: tx("Search email for “{0}”", { "0": text.trim() }),
        section: "Search",
        run: () => onSearch(text.trim()),
      },
    ];
    if (onAsk) {
      extra.push({
        id: "ask",
        label: tx("Ask AI: {0}", { "0": text.trim() }),
        section: "Search",
        run: () => onAsk(text.trim()),
      });
    } else if (askLocked) {
      extra.push({
        id: "ask",
        label: tx("Ask AI: {0}", { "0": text.trim() }),
        section: "Search",
        run: () => {},
        locked: tx("Included with Mailzo Cloud"),
      });
    }
    return [...found, ...extra];
  }, [commands, text, onSearch, onAsk, askLocked]);

  useEffect(() => setAt(0), [text]);

  const go = (c: Command | undefined) => {
    if (!c || c.locked) return;
    onClose();
    c.run();
  };

  return createPortal(
    <div
      className="scrim scrim-top"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div role="dialog" aria-label={tx("Command bar")} className="command-bar">
        <div className="command-input">
          <Icon icon={Search01Icon} size="s" />
          <input
            ref={input}
            aria-label={tx("Type a command")}
            placeholder={tx("Type a command or search")}
            value={text}
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                onClose();
              } else if (e.key === "ArrowDown") {
                e.preventDefault();
                setAt((n) => Math.min(n + 1, list.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setAt((n) => Math.max(n - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                go(list[at]);
              }
            }}
          />
        </div>
        <div className="command-list" role="listbox" aria-label={tx("Commands")}>
          {list.map((c, i) => (
            <button
              key={c.id}
              role="option"
              aria-selected={i === at}
              aria-disabled={c.locked ? true : undefined}
              disabled={!!c.locked}
              className="command-row"
              ref={(el) => {
                if (i === at) el?.scrollIntoView({ block: "nearest" });
              }}
              onMouseMove={() => setAt(i)}
              onClick={() => go(c)}
            >
              <span className="grow truncate-1">{c.label}</span>
              {c.locked && <span className="menu-key">{c.locked}</span>}
              {c.keys && <span className="menu-key">{c.keys}</span>}
            </button>
          ))}
          {list.length === 0 && <div className="command-none">{tx("Nothing matches")}</div>}
        </div>
      </div>
    </div>,
    document.body,
  );
}
