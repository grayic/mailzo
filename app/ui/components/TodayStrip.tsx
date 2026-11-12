import { useEffect, useState } from "react";
import { calendarApi, type Occurrence } from "../api";
import { Icon } from "./ui";
import { Video01Icon } from "../icons";
import { language, tx } from "../i18n";

const LOOK_AGAIN_MS = 60_000;
const MOST_SHOWN = 6;
const MEETING_HOSTS = ["zoom.us", "meet.google.com", "teams.microsoft.com", "teams.live.com", "whereby.com", "webex.com"];
const WEB_LINK = /https:\/\/[^\s"'<>]+/gi;

/** The first link whose own host is a meeting service (or a part of one, like us02web.zoom.us). */
function meetingLink(text: string): string | undefined {
  for (const candidate of text.match(WEB_LINK) ?? []) {
    try {
      const host = new URL(candidate).hostname.toLowerCase();
      if (MEETING_HOSTS.some((h) => host === h || host.endsWith(`.${h}`))) return candidate;
    } catch {
      // Not a real address: skip it.
    }
  }
  return undefined;
}

const startOfToday = () => {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
};
const clock = (unix: number) => new Date(unix * 1000).toLocaleTimeString(language, { hour: "numeric", minute: "2-digit" });

/** What's left of today's calendar, along the top of the full-width list. Shows nothing on a free day. */
export function TodayStrip() {
  const [events, setEvents] = useState<Occurrence[]>([]);
  const [now, setNow] = useState(() => Date.now() / 1000);

  useEffect(() => {
    let live = true;
    const look = () => {
      setNow(Date.now() / 1000);
      const from = startOfToday();
      calendarApi
        .events(from, from + 86_400)
        .then((list) => live && setEvents(list))
        .catch(() => {});
    };
    look();
    const timer = setInterval(look, LOOK_AGAIN_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, []);

  const shown = events
    .filter((e) => e.all_day || e.ends > now)
    .filter((e) => e.my_status !== "declined")
    .sort((a, b) => Number(b.all_day) - Number(a.all_day) || a.starts - b.starts)
    .slice(0, MOST_SHOWN);
  if (shown.length === 0) return null;

  return (
    <div role="list" aria-label={tx("Today's calendar")} className="today-strip">
      {shown.map((e) => {
        const happening = !e.all_day && e.starts <= now && e.ends > now;
        const link = meetingLink(`${e.location} ${e.notes}`);
        const soon = !e.all_day && e.starts - now < 15 * 60;
        return (
          <div
            key={`${e.event_id}:${e.starts}`}
            role="listitem"
            className="today-event"
            data-now={happening || undefined}
            style={{ "--h": e.hue } as React.CSSProperties}
          >
            <span className="today-event-text">
              <span className="today-event-when">
                {e.all_day ? tx("All day") : happening ? `Now, until ${clock(e.ends)}` : clock(e.starts)}
              </span>
              <span className="today-event-title truncate-1">{e.title || tx("(no title)")}</span>
            </span>
            {link && soon && (
              <a className="btn btn-primary btn-sm" href={link} target="_blank" rel="noreferrer">
                <Icon icon={Video01Icon} size="xs" />
                {tx("Join")}</a>
            )}
          </div>
        );
      })}
    </div>
  );
}
