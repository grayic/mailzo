import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ApiError, calendarApi, type Calendar, type EventInput, type Occurrence } from "../api";
import type { IconSvgElement } from "@hugeicons/react";
import {
  Add01Icon,
  ArrowLeft01Icon,
  ArrowRight01Icon,
  Calendar03Icon,
  Clock01Icon,
  Delete02Icon,
  Location01Icon,
  Menu01Icon,
  Note01Icon,
  RepeatIcon,
  UserGroupIcon,
} from "../icons";
import { Confirm, Dialog, Icon, IconButton, Problem, Spinner } from "./ui";
import { Choice, Toggle } from "./settings/controls";
import { language, tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
const addDays = (d: Date, n: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
const unix = (d: Date) => Math.floor(d.getTime() / 1000);
const sameDay = (a: Date, b: Date) => startOfDay(a).getTime() === startOfDay(b).getTime();

/** A whole-day event is stored as that date in UTC, so it's read the same way everywhere. */
const eventDay = (o: { starts: number; all_day: boolean }) => {
  const d = new Date(o.starts * 1000);
  return o.all_day ? new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()) : startOfDay(d);
};

/** The date a whole-day time is kept as, shown as that same date here. */
const utcDate = (seconds: number) => {
  const d = new Date(seconds * 1000);
  return new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
};

/** The last day an event is on. A whole-day event's end is the start of the day after. */
const eventLastDay = (o: { starts: number; ends: number; all_day: boolean }) => {
  const first = eventDay(o);
  const last = o.all_day ? utcDate(o.ends - 86_400) : startOfDay(new Date((o.ends - 1) * 1000));
  return last < first ? first : last;
};

const localInput = (d: Date) => {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

const datePart = (value: string) => value.slice(0, 10);
const timePart = (value: string) => value.slice(11, 16);

const REPEATS = [
  { value: "none", label: tx("Doesn't repeat") },
  { value: "daily", label: tx("Every day") },
  { value: "weekly", label: tx("Every week") },
  { value: "monthly", label: tx("Every month") },
  { value: "yearly", label: tx("Every year") },
];

/** How long an event lasts, in short words. */
function lengthText(from: string, to: string, allDay: boolean) {
  if (allDay) {
    const days = Math.round((new Date(datePart(to)).getTime() - new Date(datePart(from)).getTime()) / 86_400_000) + 1;
    return days <= 1 ? tx("1 day") : tx("{n} days", { n: days });
  }
  const minutes = Math.round((new Date(to).getTime() - new Date(from).getTime()) / 60_000);
  if (minutes <= 0) return "";
  if (minutes >= 24 * 60 && minutes % (24 * 60) === 0) return tx("{n} days", { n: minutes / (24 * 60) });
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return tx("{n} min", { n: m });
  return m === 0 ? tx("{n} hr", { n: h }) : tx("{h} hr {m} min", { h, m });
}

/** One line of the event form: its icon, then what it holds. */
function Field({ icon, hue, top, children }: { icon?: IconSvgElement; hue?: number; top?: boolean; children: React.ReactNode }) {
  return (
    <div className={`event-field ${top ? "event-field-top" : ""}`}>
      <span className="event-field-icon" aria-hidden="true">
        {hue !== undefined ? <span className="event-swatch" style={{ "--h": hue } as React.CSSProperties} /> : icon && <Icon icon={icon} size="s" />}
      </span>
      <div className="event-field-body">{children}</div>
    </div>
  );
}

function EventEditor({
  start,
  existing,
  calendars,
  onClose,
  onSaved,
}: {
  start: Date;
  existing: Occurrence | null;
  calendars: Calendar[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const [title, setTitle] = useState(existing?.title ?? "");
  const [location, setLocation] = useState(existing?.location ?? "");
  const [notes, setNotes] = useState(existing?.notes ?? "");
  const [allDay, setAllDay] = useState(existing?.all_day ?? false);
  const [from, setFrom] = useState(
    localInput(existing ? (existing.all_day ? utcDate(existing.starts) : new Date(existing.starts * 1000)) : start),
  );
  const [to, setTo] = useState(
    localInput(
      existing
        ? existing.all_day
          ? utcDate(existing.ends - 86_400)
          : new Date(existing.ends * 1000)
        : new Date(start.getTime() + 3_600_000),
    ),
  );
  const readOnly = existing?.is_override ?? false;
  const [repeat, setRepeat] = useState<string>("none");
  const [picked, setCalendar] = useState<number | null>(existing?.calendar_id ?? null);
  // The list can still be on its way when this opens.
  const calendar = picked ?? calendars[0]?.id ?? 0;
  const chosen = calendars.find((c) => c.id === calendar);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);

  // Moving the start moves the end with it, so the event stays the same length.
  const changeStart = (date: string, time: string) => {
    if (!date || !time) return;
    const next = `${date}T${time}`;
    const length = new Date(to).getTime() - new Date(from).getTime();
    setFrom(next);
    if (length >= 0) setTo(localInput(new Date(new Date(next).getTime() + length)));
  };
  const endsTooSoon = allDay
    ? datePart(to) < datePart(from)
    : new Date(to).getTime() <= new Date(from).getTime();
  const length = endsTooSoon ? "" : lengthText(from, to, allDay);

  const save = async () => {
    setBusy(true);
    setProblem(null);
    const starts = new Date(from);
    const ends = new Date(to);
    // A whole day is kept as that date, midnight to midnight, in UTC.
    const dayOf = (d: Date) => Math.floor(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / 1000);
    const input: EventInput = {
      id: existing?.event_id,
      calendar_id: calendar,
      title,
      location,
      notes,
      all_day: allDay,
      starts: allDay ? dayOf(starts) : unix(starts),
      ends: allDay ? dayOf(ends) + 86_400 : unix(ends),
      repeat: existing ? undefined : repeat,
      occurrence: existing?.repeats ? existing.starts : undefined,
    };
    try {
      await calendarApi.save(input);
      onSaved();
    } catch (e) {
      setProblem(say(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={existing ? tx("Event") : tx("New event")}
      className="event-dialog"
      onClose={onClose}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
      footer={
        <>
          {existing && !readOnly && (
            <button type="button" className="btn btn-ghost event-delete" onClick={() => setDeleting(true)}>
              <Icon icon={Delete02Icon} size="s" />
              {tx("Delete")}
            </button>
          )}
          <span className="spacer" />
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || readOnly || !calendar || !title.trim() || endsTooSoon}>
            {busy && <Spinner size="xs" />}
            {existing ? tx("Save") : tx("Add event")}
          </button>
        </>
      }
    >
      <div className="event-form">
        <input
          autoFocus={!existing}
          className="event-title"
          aria-label={tx("Event name")}
          placeholder={tx("Add a title")}
          value={title}
          disabled={readOnly}
          onChange={(e) => setTitle(e.target.value)}
        />

        <div className="event-fields">
          <Field icon={Clock01Icon}>
            <div className="event-when">
              <div className="event-when-line">
                <span className="event-when-label">{tx("Starts")}</span>
                <input
                  type="date"
                  className="event-pill tnum"
                  aria-label={tx("Starts")}
                  value={datePart(from)}
                  disabled={readOnly}
                  onChange={(e) => changeStart(e.target.value, timePart(from))}
                />
                {!allDay && (
                  <input
                    type="time"
                    className="event-pill tnum"
                    aria-label={tx("Start time")}
                    value={timePart(from)}
                    disabled={readOnly}
                    onChange={(e) => changeStart(datePart(from), e.target.value)}
                  />
                )}
              </div>
              <div className="event-when-line">
                <span className="event-when-label">{tx("Ends")}</span>
                <input
                  type="date"
                  className="event-pill tnum"
                  aria-label={tx("Ends")}
                  value={datePart(to)}
                  disabled={readOnly}
                  onChange={(e) => setTo(`${e.target.value}T${timePart(to)}`)}
                />
                {!allDay && (
                  <input
                    type="time"
                    className="event-pill tnum"
                    aria-label={tx("End time")}
                    value={timePart(to)}
                    disabled={readOnly}
                    onChange={(e) => setTo(`${datePart(to)}T${e.target.value}`)}
                  />
                )}
                {length && <span className="event-length tnum">{length}</span>}
              </div>
              {endsTooSoon && <div className="event-warn">{tx("It ends before it starts.")}</div>}
              <label className="event-all-day">
                <span>{tx("All day")}</span>
                <Toggle label={tx("All day")} on={allDay} disabled={readOnly} onChange={setAllDay} />
              </label>
            </div>
          </Field>

          {!existing ? (
            <Field icon={RepeatIcon}>
              <Choice label={tx("Repeats")} value={repeat} options={REPEATS} onChange={setRepeat} />
            </Field>
          ) : (
            existing.repeats && (
              <Field icon={RepeatIcon}>
                <span className="event-note">{tx("Repeats. Changes apply to every time.")}</span>
              </Field>
            )
          )}

          {chosen && (
            <Field hue={chosen.hue}>
              {!existing && calendars.length > 1 ? (
                <Choice
                  label={tx("Calendar")}
                  value={String(calendar)}
                  options={calendars.map((c) => ({ value: String(c.id), label: c.name }))}
                  onChange={(id) => setCalendar(Number(id))}
                />
              ) : (
                <span className="event-note">{chosen.name}</span>
              )}
            </Field>
          )}

          <Field icon={Location01Icon}>
            <input
              className="event-plain"
              aria-label={tx("Where")}
              placeholder={tx("Add a place")}
              value={location}
              disabled={readOnly}
              onChange={(e) => setLocation(e.target.value)}
            />
          </Field>

          {existing && (existing.organizer || existing.attendees.length > 0) && (
            <Field icon={UserGroupIcon} top>
              <div className="event-people">
                {existing.organizer && (
                  <span>
                    {tx("Organised by")} {existing.organizer}
                  </span>
                )}
                {existing.attendees.length > 0 && (
                  <span className="event-note">{existing.attendees.map((a) => a.name || a.address).join(", ")}</span>
                )}
              </div>
            </Field>
          )}

          <Field icon={Note01Icon} top>
            <textarea
              aria-label={tx("Notes")}
              className="event-plain event-notes"
              placeholder={tx("Add notes")}
              rows={3}
              value={notes}
              disabled={readOnly}
              onChange={(e) => setNotes(e.target.value)}
            />
          </Field>
        </div>

        {readOnly && <div className="event-note">{tx("This is one changed time of a repeating event. Change it in the calendar it came from.")}</div>}
        <Problem text={problem} />
      </div>
      {deleting && existing && (
        <Confirm
          title={tx("Delete {0}?", { "0": existing.title })}
          confirm={tx("Yes, delete")}
          keep={tx("Keep")}
          danger
          onKeep={() => setDeleting(false)}
          onConfirm={() => {
            setDeleting(false);
            void calendarApi
              .remove(existing.event_id)
              .then(onSaved)
              .catch((e) => setProblem(say(e)));
          }}
        >
          {existing.repeats ? tx("Every time it repeats will go too.") : tx("This can't be undone.")}
        </Confirm>
      )}
    </Dialog>
  );
}

/** One event in the day's list: its colour, when, what and where. */
function AgendaItem({ o, when, onOpen }: { o: Occurrence; when: [string, string] | null; onOpen: () => void }) {
  const status = o.my_status === "DECLINED" ? tx("Declined") : o.my_status === "TENTATIVE" ? tx("Maybe") : null;
  return (
    <button
      type="button"
      className="calendar-event"
      data-declined={o.my_status === "DECLINED" || undefined}
      style={{ "--h": o.hue } as React.CSSProperties}
      onClick={onOpen}
    >
      <span className="calendar-event-bar" aria-hidden="true" />
      <span className="calendar-event-text">
        <span className="calendar-event-title truncate-1">{o.title}</span>
        <span className="calendar-event-meta truncate-1 tnum">
          {when ? `${when[0]} – ${when[1]}` : tx("All day")}
          {o.location && ` · ${o.location}`}
        </span>
      </span>
      {status && <span className="pill">{status}</span>}
    </button>
  );
}

/** A month at a time, with the chosen day's events beside it. */
export function CalendarView({ top, onPhoneNav }: { top?: React.ReactNode; onPhoneNav?: () => void }) {
  const [month, setMonth] = useState(() => {
    const now = new Date();
    return new Date(now.getFullYear(), now.getMonth(), 1);
  });
  const [day, setDay] = useState(() => startOfDay(new Date()));
  const [events, setEvents] = useState<Occurrence[]>([]);
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [canConnect, setCanConnect] = useState<{ account_id: number; email: string }[]>([]);
  const [editing, setEditing] = useState<{ existing: Occurrence | null; start: Date } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);

  // Six rows starting on a Monday, so the grid doesn't jump between months.
  const cells = useMemo(() => {
    const back = (month.getDay() + 6) % 7;
    const first = addDays(month, -back);
    return Array.from({ length: 42 }, (_, i) => addDays(first, i));
  }, [month]);

  // Moving through months quickly mustn't let an older answer replace a newer one.
  const loadRun = useRef(0);
  const load = useCallback(async () => {
    const run = ++loadRun.current;
    try {
      const list = await calendarApi.list();
      const found = await calendarApi.events(unix(cells[0]), unix(addDays(cells[41], 1)));
      if (run !== loadRun.current) return;
      setCalendars(list.calendars);
      setCanConnect(list.can_connect);
      setEvents(found);
    } catch (e) {
      if (run === loadRun.current) setProblem(say(e));
    }
  }, [cells]);

  useEffect(() => {
    void load();
  }, [load]);

  const onDay = (d: Date) => events.filter((o) => eventDay(o) <= d && d <= eventLastDay(o));
  const problems = calendars.filter((c) => c.problem);

  const connect = async (accountId: number) => {
    setConnecting(true);
    setProblem(null);
    try {
      await calendarApi.connect(accountId);
      await load();
    } catch (e) {
      setProblem(say(e));
    } finally {
      setConnecting(false);
    }
  };

  const newAt = (on: Date = day) => {
    const start = new Date(on);
    const now = new Date();
    start.setHours(sameDay(on, now) ? Math.min(now.getHours() + 1, 23) : 9, 0, 0, 0);
    setEditing({ existing: null, start });
  };

  const today = new Date();
  const dayEvents = onDay(day).sort((x, y) => Number(y.all_day) - Number(x.all_day) || x.starts - y.starts);
  const allDayEvents = dayEvents.filter((o) => o.all_day);
  const timedEvents = dayEvents.filter((o) => !o.all_day);
  const hourOf = (seconds: number) =>
    new Date(seconds * 1000).toLocaleTimeString(language, { hour: "numeric", minute: "2-digit" });
  // Short on the month grid: "8 AM", or "8:30 AM" when it isn't on the hour.
  const chipTime = (seconds: number) => {
    const d = new Date(seconds * 1000);
    return d.toLocaleTimeString(language, d.getMinutes() === 0 ? { hour: "numeric" } : { hour: "numeric", minute: "2-digit" });
  };
  const goToday = () => {
    const now = new Date();
    setMonth(new Date(now.getFullYear(), now.getMonth(), 1));
    setDay(startOfDay(now));
  };

  return (
    <div className="calendar">
      <main className="calendar-month">
        <div className="calendar-head" data-tauri-drag-region="deep">
          {onPhoneNav && (
            <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />
          )}
          <h2 className="calendar-title">
            {month.toLocaleDateString(language, { month: "long" })}{" "}
            <span className="calendar-year tnum">{month.getFullYear()}</span>
          </h2>
          <span className="spacer" />
          <div className="calendar-nav">
            <IconButton
              icon={ArrowLeft01Icon}
              label={tx("Previous month")}
              small
              onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}
            />
            <button type="button" className="btn btn-ghost btn-sm" onClick={goToday}>
              {tx("Today")}
            </button>
            <IconButton
              icon={ArrowRight01Icon}
              label={tx("Next month")}
              small
              onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}
            />
          </div>
          <button type="button" className="btn btn-primary btn-sm" onClick={() => newAt()}>
            <Icon icon={Add01Icon} size="s" />
            {tx("New event")}
          </button>
        </div>
        {top}
        <div className="calendar-grid" role="grid" aria-label={tx("Month")}>
          {DAY_NAMES.map((n, i) => (
            <div key={n} className="calendar-dayname" data-weekend={i >= 5 || undefined}>
              {tx(n)}
            </div>
          ))}
          {cells.map((d) => {
            const list = onDay(d).sort((x, y) => Number(y.all_day) - Number(x.all_day) || x.starts - y.starts);
            const weekend = d.getDay() === 0 || d.getDay() === 6;
            return (
              <button
                key={d.getTime()}
                type="button"
                role="gridcell"
                aria-selected={sameDay(d, day)}
                aria-label={d.toLocaleDateString(language, { dateStyle: "long" })}
                className="calendar-cell"
                data-other={d.getMonth() !== month.getMonth() || undefined}
                data-today={sameDay(d, today) || undefined}
                data-weekend={weekend || undefined}
                onClick={() => setDay(startOfDay(d))}
                onDoubleClick={() => {
                  setDay(startOfDay(d));
                  newAt(startOfDay(d));
                }}
              >
                <span className="calendar-date tnum">{d.getDate()}</span>
                {list.slice(0, 3).map((o) => (
                  <span
                    key={`${o.event_id}:${o.starts}`}
                    className="calendar-chip"
                    data-all-day={o.all_day || undefined}
                    style={{ "--h": o.hue } as React.CSSProperties}
                  >
                    {!o.all_day && <span className="calendar-chip-time tnum">{chipTime(o.starts)}</span>}
                    <span className="truncate-1">{o.title}</span>
                  </span>
                ))}
                {list.length > 3 && <span className="calendar-more">{tx("{n} more", { n: list.length - 3 })}</span>}
              </button>
            );
          })}
        </div>
      </main>
      <aside className="calendar-day" aria-label={tx("Selected day")}>
        <div className="calendar-day-head">
          <div className="calendar-day-weekday" data-today={sameDay(day, today) || undefined}>
            {sameDay(day, today) ? tx("Today") : day.toLocaleDateString(language, { weekday: "long" })}
          </div>
          <div className="calendar-day-date">
            <span className="calendar-day-number tnum">{day.getDate()}</span>
            <span className="calendar-day-month">{day.toLocaleDateString(language, { month: "long", year: "numeric" })}</span>
          </div>
        </div>
        <Problem text={problem} />
        {problems.map((c) => (
          <div key={c.id} className="calendar-problem">
            {tx("{0} couldn't refresh: {1}", { "0": c.name, "1": c.problem ?? "" })}
          </div>
        ))}
        {dayEvents.length === 0 ? (
          <div className="calendar-day-empty">
            <div className="empty-icon">
              <Icon icon={Calendar03Icon} size="l" />
            </div>
            <div className="empty-title">{tx("Nothing planned")}</div>
            <button type="button" className="btn btn-secondary btn-sm" onClick={() => newAt()}>
              <Icon icon={Add01Icon} size="s" />
              {tx("Add an event")}
            </button>
          </div>
        ) : (
          <div className="calendar-agenda">
            {allDayEvents.length > 0 && <div className="calendar-agenda-label">{tx("All day")}</div>}
            {allDayEvents.map((o) => (
              <AgendaItem key={`${o.event_id}:${o.starts}`} o={o} when={null} onOpen={() => setEditing({ existing: o, start: new Date(o.starts * 1000) })} />
            ))}
            {timedEvents.length > 0 && allDayEvents.length > 0 && <div className="calendar-agenda-label">{tx("Schedule")}</div>}
            {timedEvents.map((o) => (
              <AgendaItem
                key={`${o.event_id}:${o.starts}`}
                o={o}
                when={[hourOf(o.starts), hourOf(o.ends)]}
                onOpen={() => setEditing({ existing: o, start: new Date(o.starts * 1000) })}
              />
            ))}
          </div>
        )}
        {canConnect.length > 0 && (
          <div className="calendar-connect">
            <div className="calendar-connect-title">{tx("Connect a calendar")}</div>
            <div className="calendar-connect-text">{tx("See events from your inboxes here.")}</div>
            {canConnect.map((a) => (
              <button
                key={a.account_id}
                type="button"
                className="calendar-connect-row"
                disabled={connecting}
                onClick={() => void connect(a.account_id)}
              >
                {connecting ? <Spinner size="xs" /> : <Icon icon={Calendar03Icon} size="s" />}
                <span className="grow truncate-1">{a.email}</span>
                <span className="calendar-connect-go">{tx("Connect")}</span>
              </button>
            ))}
          </div>
        )}
      </aside>
      {editing && (
        <EventEditor
          start={editing.start}
          existing={editing.existing}
          calendars={calendars}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void load();
          }}
        />
      )}
    </div>
  );
}
