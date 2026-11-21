import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { language, tx } from "../i18n";
import { ArrowLeft01Icon, ArrowRight01Icon, Calendar03Icon } from "../icons";
import { Icon, IconButton } from "./ui";

/** "2026-11-16" for a day, as forms keep it. */
function key(d: Date): string {
  const two = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
}

function fromKey(text: string): Date | null {
  const [y, m, d] = text.split("-").map(Number);
  return y && m && d ? new Date(y, m - 1, d) : null;
}

/** Six weeks from the Sunday on or before the first of the month, so every month is the same height. */
function weeksOf(month: Date): Date[] {
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const start = new Date(first);
  start.setDate(1 - first.getDay());
  return Array.from({ length: 42 }, (_, i) => new Date(start.getFullYear(), start.getMonth(), start.getDate() + i));
}

/** A date box that opens a calendar. The value is "YYYY-MM-DD", or "" for none. */
export function DateField({
  value,
  onChange,
  label,
  placeholder = tx("Pick a date"),
}: {
  value: string;
  onChange: (value: string) => void;
  label: string;
  placeholder?: string;
}) {
  const chosen = fromKey(value);
  const [open, setOpen] = useState(false);
  const [month, setMonth] = useState(() => chosen ?? new Date());
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const today = key(new Date());

  const days = useMemo(() => weeksOf(month), [month]);
  const weekdays = useMemo(() => {
    const fmt = new Intl.DateTimeFormat(language, { weekday: "short" });
    // 2026-11-15 is a Sunday.
    return Array.from({ length: 7 }, (_, i) => fmt.format(new Date(2026, 10, 15 + i)).slice(0, 2));
  }, []);
  const title = new Intl.DateTimeFormat(language, { month: "long", year: "numeric" }).format(month);
  const shown = chosen
    ? chosen.toLocaleDateString(language, { weekday: "short", month: "short", day: "numeric", year: "numeric" })
    : "";

  // Under the box, or above it when there's no room below; inside the window either way.
  useLayoutEffect(() => {
    const el = panel.current;
    const anchor = button.current;
    if (!open || !el || !anchor) return;
    const r = anchor.getBoundingClientRect();
    const below = r.bottom + 6;
    const top = below + el.offsetHeight > window.innerHeight - 8 ? r.top - el.offsetHeight - 6 : below;
    el.style.top = `${Math.max(8, top)}px`;
    el.style.left = `${Math.min(Math.max(8, r.left), window.innerWidth - el.offsetWidth - 8)}px`;
    el.querySelector<HTMLElement>("[aria-selected='true'], [data-today]")?.focus();
  }, [open, month]);

  // Closes on a click outside or Escape, without closing the dialog it sits in.
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!panel.current?.contains(t) && !button.current?.contains(t)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
        button.current?.focus();
      }
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", key, true);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", key, true);
    };
  }, [open]);

  const pick = (d: Date | null) => {
    onChange(d ? key(d) : "");
    setOpen(false);
    button.current?.focus();
  };
  const step = (by: number) => setMonth((m) => new Date(m.getFullYear(), m.getMonth() + by, 1));

  return (
    <>
      <button
        ref={button}
        type="button"
        className="date-field"
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => {
          setMonth(chosen ?? new Date());
          setOpen((v) => !v);
        }}
      >
        <span className={`grow truncate-1 ${shown ? "" : "date-field-empty"}`}>{shown || placeholder}</span>
        <Icon icon={Calendar03Icon} size="s" />
      </button>
      {open &&
        createPortal(
          <div ref={panel} className="date-pop" role="dialog" aria-label={label}>
            <div className="date-pop-head">
              <span className="date-pop-title">{title}</span>
              <IconButton icon={ArrowLeft01Icon} label={tx("Previous month")} small onClick={() => step(-1)} />
              <IconButton icon={ArrowRight01Icon} label={tx("Next month")} small onClick={() => step(1)} />
            </div>
            <div className="date-grid" role="grid">
              {weekdays.map((w) => (
                <span key={w} className="date-weekday" role="columnheader">
                  {w}
                </span>
              ))}
              {days.map((d) => {
                const k = key(d);
                const inMonth = d.getMonth() === month.getMonth();
                return (
                  <button
                    key={k}
                    type="button"
                    role="gridcell"
                    className={`date-day tnum ${inMonth ? "" : "date-day-out"}`}
                    aria-selected={k === value}
                    data-today={k === today ? "" : undefined}
                    aria-label={d.toLocaleDateString(language, { dateStyle: "full" })}
                    onClick={() => pick(d)}
                  >
                    {d.getDate()}
                  </button>
                );
              })}
            </div>
            <div className="date-pop-foot">
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => pick(null)} disabled={!value}>
                {tx("Clear")}
              </button>
              <button type="button" className="btn btn-secondary btn-sm" onClick={() => pick(new Date())}>
                {tx("Today")}
              </button>
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
