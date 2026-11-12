import { KeyboardEvent, useEffect, useRef, useState } from "react";
import { api } from "../api";
import { Cancel01Icon } from "../icons";
import type { Addr, Contact } from "../types";
import { Avatar, Icon } from "./ui";

/** Reads "Name <a@b.com>" or "a@b.com". */
function parseOne(raw: string): Addr | null {
  const text = raw.trim().replace(/^["']|["']$/g, "");
  if (!text) return null;
  const match = text.match(/^(.*)<([^<>]+)>$/);
  if (match) {
    return { name: match[1].trim().replace(/^"|"$/g, ""), address: match[2].trim() };
  }
  return { name: "", address: text };
}

export const looksValid = (a: Addr) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(a.address);

export function RecipientInput({
  label,
  accountId,
  value,
  onChange,
  autoFocus,
  action,
}: {
  label: string;
  accountId: number;
  value: Addr[];
  onChange: (next: Addr[]) => void;
  autoFocus?: boolean;
  action?: React.ReactNode;
}) {
  const [text, setText] = useState("");
  const [suggestions, setSuggestions] = useState<Contact[]>([]);
  const [highlight, setHighlight] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const query = text.trim();
    if (query.length < 2) {
      setSuggestions([]);
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      api
        .contacts(accountId, query)
        .then((list) => {
          if (!live) return;
          const taken = new Set(value.map((a) => a.address.toLowerCase()));
          setSuggestions(list.filter((c) => !taken.has(c.address.toLowerCase())));
          setHighlight(0);
        })
        .catch(() => {});
    }, 120);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [text, accountId, value]);

  const add = (people: Addr[]) => {
    const seen = new Set(value.map((a) => a.address.toLowerCase()));
    const fresh = people.filter((p) => {
      const key = p.address.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
    if (fresh.length) onChange([...value, ...fresh]);
    setText("");
    setSuggestions([]);
  };

  const commit = () => {
    const people = text
      .split(/[,;\n]/)
      .map(parseOne)
      .filter((a): a is Addr => a !== null);
    if (people.length) add(people);
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (suggestions.length && e.key === "ArrowDown") {
      e.preventDefault();
      setHighlight((h) => (h + 1) % suggestions.length);
    } else if (suggestions.length && e.key === "ArrowUp") {
      e.preventDefault();
      setHighlight((h) => (h - 1 + suggestions.length) % suggestions.length);
    } else if (e.key === "Enter" || e.key === "Tab" || e.key === ",") {
      if (suggestions.length && e.key !== ",") {
        e.preventDefault();
        add([suggestions[highlight]]);
      } else if (text.trim()) {
        e.preventDefault();
        commit();
      }
    } else if (e.key === "Backspace" && !text && value.length) {
      onChange(value.slice(0, -1));
    } else if (e.key === "Escape" && suggestions.length) {
      e.stopPropagation();
      setSuggestions([]);
    }
  };

  return (
    <div className="composer-row">
      <span className="composer-label">{label}</span>
      <div className="chips" onClick={() => input.current?.focus()}>
        {value.map((a) => (
          <span key={a.address} data-tip={a.address} className={`chip ${looksValid(a) ? "" : "chip-bad"}`}>
            <Avatar name={a.name || a.address} email={a.address} size="xs" />
            <span className="chip-label">{a.name || a.address}</span>
            <button
              type="button"
              className="chip-x"
              aria-label={`Remove ${a.address}`}
              onClick={() => onChange(value.filter((x) => x !== a))}
            >
              <Icon icon={Cancel01Icon} size="xs" />
            </button>
          </span>
        ))}
        <input
          ref={input}
          aria-label={label}
          className="plain-input"
          autoFocus={autoFocus}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKey}
          onBlur={() => setTimeout(commit, 150)}
          onPaste={(e) => {
            const pasted = e.clipboardData.getData("text");
            if (/[,;\n]/.test(pasted) || pasted.includes("@")) {
              e.preventDefault();
              const people = (text + pasted)
                .split(/[,;\n]/)
                .map(parseOne)
                .filter((a): a is Addr => a !== null);
              add(people);
            }
          }}
        />
      </div>
      {action}
      {suggestions.length > 0 && (
        <ul role="listbox" aria-label={`${label} suggestions`} className="suggest">
          {suggestions.map((c, i) => (
            <li key={c.address}>
              <button
                type="button"
                role="option"
                aria-selected={i === highlight}
                className="suggest-item"
                onMouseEnter={() => setHighlight(i)}
                onMouseDown={(e) => {
                  e.preventDefault();
                  add([{ name: c.name, address: c.address }]);
                }}
              >
                <Avatar name={c.name || c.address} email={c.address} size="s" />
                <span className="grow">
                  <span className="profile-name truncate-1">{c.name || c.address}</span>
                  {c.name && <span className="profile-sub truncate-1">{c.address}</span>}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
