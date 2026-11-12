//! Reads and writes calendar entries in the format every calendar uses (iCalendar).

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attendee {
    pub address: String,
    #[serde(default)]
    pub name: String,
    /// `ACCEPTED`, `DECLINED`, `TENTATIVE` or `NEEDS-ACTION`.
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IcsEvent {
    pub uid: String,
    pub title: String,
    pub location: String,
    pub notes: String,
    pub starts: i64,
    pub ends: i64,
    pub all_day: bool,
    pub rrule: Option<String>,
    pub exdates: Vec<i64>,
    pub organizer: Option<String>,
    pub organizer_name: String,
    pub attendees: Vec<Attendee>,
    pub sequence: i64,
    pub cancelled: bool,
    /// The named time zone its times follow, if there is one, so repeats keep their hour.
    pub tz: Option<String>,
    /// Which one of a repeating event's times this changes, if it's a single change.
    pub recurrence_id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IcsFile {
    /// `REQUEST` for an invitation, `CANCEL` for a cancellation, and so on.
    pub method: String,
    pub events: Vec<IcsEvent>,
}

/// Long lines are folded onto several, each continuing line starting with a space.
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
            }
        } else if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(';', r"\;")
        .replace(',', r"\,")
        .replace('\r', "")
        .replace('\n', r"\n")
}

/// `NAME;PARAM=x;PARAM2=y:value`, as name, params and value.
#[allow(clippy::type_complexity)]
fn split_line(line: &str) -> Option<(String, Vec<(String, String)>, String)> {
    // A colon inside a quoted parameter isn't the separator.
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        })
        .collect();
    Some((name, params, value.to_string()))
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Outlook names zones its own way. These cover most of the mail people get.
fn windows_zone(name: &str) -> Option<&'static str> {
    Some(match name {
        "Pacific Standard Time" => "America/Los_Angeles",
        "Mountain Standard Time" => "America/Denver",
        "Central Standard Time" => "America/Chicago",
        "Eastern Standard Time" => "America/New_York",
        "GMT Standard Time" => "Europe/London",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "Central Europe Standard Time" => "Europe/Budapest",
        "India Standard Time" => "Asia/Kolkata",
        "Bangladesh Standard Time" => "Asia/Dhaka",
        "Pakistan Standard Time" => "Asia/Karachi",
        "Arabian Standard Time" => "Asia/Dubai",
        "China Standard Time" => "Asia/Shanghai",
        "Singapore Standard Time" => "Asia/Singapore",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        _ => return None,
    })
}

fn zone(name: &str) -> Option<Tz> {
    let name = name.trim();
    name.parse::<Tz>()
        .ok()
        .or_else(|| windows_zone(name).and_then(|z| z.parse().ok()))
}

/// `(UTC+06:00) Astana`, as some mail apps name a zone, in seconds.
fn offset_in_name(name: &str) -> Option<i64> {
    let at = name.find("UTC").or_else(|| name.find("GMT"))? + 3;
    let rest = &name[at..];
    let sign = match rest.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return Some(0),
    };
    let digits: String = rest[1..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let (h, m) = digits.split_once(':').unwrap_or((&digits, "0"));
    Some(sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60))
}

/// The moment a clock in this zone shows this time. A time the clocks skip over when they
/// go forward counts as the hour after.
fn in_zone(tz: Tz, t: NaiveDateTime) -> Option<i64> {
    tz.from_local_datetime(&t)
        .earliest()
        .or_else(|| tz.from_local_datetime(&(t + Duration::hours(1))).earliest())
        .map(|m| m.timestamp())
}

/// A time as a moment, and whether it was a whole day.
fn read_time(value: &str, params: &[(String, String)], local: i32) -> Option<(i64, bool)> {
    let value = value.trim();
    if param(params, "VALUE") == Some("DATE") || (value.len() == 8 && !value.contains('T')) {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        // A whole day has no time zone. It's kept as midnight UTC on that date.
        return Some((date.and_hms_opt(0, 0, 0)?.and_utc().timestamp(), true));
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let t = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((t.and_utc().timestamp(), false));
    }
    let t = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    if let Some(name) = param(params, "TZID") {
        if let Some(tz) = zone(name) {
            return Some((in_zone(tz, t)?, false));
        }
        if let Some(offset) = offset_in_name(name) {
            return Some((t.and_utc().timestamp() - offset, false));
        }
    }
    // A time with no zone means the same clock time wherever the person is.
    Some((t.and_utc().timestamp() - i64::from(local), false))
}

fn read_address(value: &str) -> String {
    let v = value.trim();
    v.get(..7)
        .filter(|p| p.eq_ignore_ascii_case("mailto:"))
        .map(|_| &v[7..])
        .unwrap_or(v)
        .to_string()
}

/// `local_offset` is the device's offset from UTC, in seconds, for floating times.
pub fn parse(text: &str, local_offset: i32) -> IcsFile {
    let mut file = IcsFile::default();
    let mut current: Option<IcsEvent> = None;
    let mut nested = 0;
    // A length can come before the start it's measured from.
    let mut pending_duration: Option<i64> = None;
    for line in unfold(text) {
        let Some((name, params, value)) = split_line(&line) else {
            continue;
        };
        if name == "BEGIN" {
            if value.eq_ignore_ascii_case("VEVENT") {
                current = Some(IcsEvent::default());
            } else if current.is_some() {
                nested += 1;
            }
            continue;
        }
        if name == "END" {
            if value.eq_ignore_ascii_case("VEVENT") {
                if let Some(mut e) = current.take() {
                    if let Some(d) = pending_duration.take() {
                        e.ends = e.starts + d;
                    }
                    if e.ends <= e.starts {
                        e.ends = e.starts + if e.all_day { 86_400 } else { 3_600 };
                    }
                    if !e.uid.is_empty() && e.starts != 0 {
                        file.events.push(e);
                    }
                }
                nested = 0;
            } else if current.is_some() {
                nested -= 1;
            }
            continue;
        }
        if name == "METHOD" {
            file.method = value.trim().to_ascii_uppercase();
        }
        let Some(e) = current.as_mut().filter(|_| nested == 0) else {
            continue;
        };
        match name.as_str() {
            "UID" => e.uid = value,
            "SUMMARY" => e.title = unescape(&value),
            "LOCATION" => e.location = unescape(&value),
            "DESCRIPTION" => e.notes = unescape(&value),
            "SEQUENCE" => e.sequence = value.trim().parse().unwrap_or(0),
            "STATUS" => e.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
            "RRULE" => e.rrule = Some(value.trim().to_string()),
            "DTSTART" => {
                if let Some((t, all_day)) = read_time(&value, &params, local_offset) {
                    e.starts = t;
                    e.all_day = all_day;
                    e.tz = param(&params, "TZID")
                        .and_then(zone)
                        .map(|z| z.name().to_string());
                }
            }
            "DTEND" => {
                if let Some((t, _)) = read_time(&value, &params, local_offset) {
                    e.ends = t;
                }
            }
            "DURATION" => {
                pending_duration = read_duration(value.trim());
            }
            "RECURRENCE-ID" => {
                e.recurrence_id = read_time(&value, &params, local_offset).map(|x| x.0);
            }
            "EXDATE" => {
                for one in value.split(',') {
                    if let Some((t, _)) = read_time(one, &params, local_offset) {
                        e.exdates.push(t);
                    }
                }
            }
            "ORGANIZER" => {
                e.organizer = Some(read_address(&value));
                e.organizer_name = param(&params, "CN").unwrap_or("").to_string();
            }
            "ATTENDEE" => e.attendees.push(Attendee {
                address: read_address(&value),
                name: param(&params, "CN").unwrap_or("").to_string(),
                status: param(&params, "PARTSTAT")
                    .unwrap_or("NEEDS-ACTION")
                    .to_ascii_uppercase(),
            }),
            _ => {}
        }
    }
    file
}

/// `PT1H30M`, `P1D` and the like, in seconds.
fn read_duration(text: &str) -> Option<i64> {
    let text = text.strip_prefix('+').unwrap_or(text);
    let rest = text.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    for c in rest.chars() {
        match c {
            'T' => {}
            d if d.is_ascii_digit() => number.push(d),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match unit {
                    'W' => 604_800,
                    'D' => 86_400,
                    'H' => 3_600,
                    'M' => 60,
                    'S' => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(total)
}

fn stamp(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

fn day(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .unwrap_or_default()
        .format("%Y%m%d")
        .to_string()
}

/// Lines over 75 bytes have to be folded.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars() {
        if count + c.len_utf8() > 74 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(c);
        count += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

fn time_lines(e: &IcsEvent) -> Vec<String> {
    if e.all_day {
        return vec![
            format!("DTSTART;VALUE=DATE:{}", day(e.starts)),
            format!("DTEND;VALUE=DATE:{}", day(e.ends)),
        ];
    }
    // A repeat keeps its clock time through daylight saving only if it names its zone.
    if let Some(tz) = e
        .rrule
        .as_ref()
        .and(e.tz.as_ref())
        .and_then(|n| n.parse::<Tz>().ok())
    {
        let local = |t: i64| {
            tz.timestamp_opt(t, 0)
                .earliest()
                .map(|m| m.format("%Y%m%dT%H%M%S").to_string())
                .unwrap_or_default()
        };
        return vec![
            format!("DTSTART;TZID={}:{}", tz.name(), local(e.starts)),
            format!("DTEND;TZID={}:{}", tz.name(), local(e.ends)),
        ];
    }
    vec![
        format!("DTSTART:{}", stamp(e.starts)),
        format!("DTEND:{}", stamp(e.ends)),
    ]
}

fn event_lines(e: &IcsEvent) -> Vec<String> {
    let mut lines = vec![
        "BEGIN:VEVENT".to_string(),
        format!("UID:{}", e.uid),
        format!("DTSTAMP:{}", stamp(crate::identity::unix_now())),
        format!("SEQUENCE:{}", e.sequence),
    ];
    lines.extend(time_lines(e));
    lines.push(format!("SUMMARY:{}", escape(&e.title)));
    if !e.location.is_empty() {
        lines.push(format!("LOCATION:{}", escape(&e.location)));
    }
    if !e.notes.is_empty() {
        lines.push(format!("DESCRIPTION:{}", escape(&e.notes)));
    }
    if let Some(rule) = &e.rrule {
        lines.push(format!("RRULE:{rule}"));
    }
    for ex in &e.exdates {
        lines.push(if e.all_day {
            format!("EXDATE;VALUE=DATE:{}", day(*ex))
        } else {
            format!("EXDATE:{}", stamp(*ex))
        });
    }
    if let Some(o) = &e.organizer {
        lines.push(format!("ORGANIZER:mailto:{o}"));
    }
    for a in &e.attendees {
        lines.push(format!(
            "ATTENDEE;PARTSTAT={};CN={}:mailto:{}",
            a.status,
            a.name.replace([';', ':', ','], " "),
            a.address
        ));
    }
    lines.push("END:VEVENT".to_string());
    lines
}

fn wrap(method: Option<&str>, body: Vec<String>) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Mailzo//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
    ];
    if let Some(m) = method {
        lines.push(format!("METHOD:{m}"));
    }
    lines.extend(body);
    lines.push("END:VCALENDAR".to_string());
    lines.iter().map(|l| fold(l)).collect()
}

pub fn write_event(e: &IcsEvent) -> String {
    wrap(None, event_lines(e))
}

/// Changes what a person can change in an existing file, and keeps everything else:
/// reminders, attendees, and changes to single repeats.
pub fn patch_event(raw: &str, e: &IcsEvent) -> String {
    const REPLACED: &[&str] = &[
        "DTSTART",
        "DTEND",
        "DURATION",
        "SUMMARY",
        "LOCATION",
        "DESCRIPTION",
        "RRULE",
        "SEQUENCE",
    ];
    let lines = unfold(raw);
    // The event itself, not a change to one of its repeats.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (n, line) in lines.iter().enumerate() {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:VEVENT") {
            start = Some(n);
        } else if upper.starts_with("END:VEVENT") {
            if let Some(s) = start.take() {
                blocks.push((s, n));
            }
        }
    }
    let master = blocks.iter().find(|(s, n)| {
        !lines[*s..*n]
            .iter()
            .any(|l| l.to_ascii_uppercase().starts_with("RECURRENCE-ID"))
    });
    let Some(&(from, to)) = master else {
        return write_event(e);
    };
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0;
    for (n, line) in lines.iter().enumerate() {
        if n <= from || n > to {
            out.push(line.clone());
            continue;
        }
        if n == to {
            out.extend(time_lines(e));
            out.push(format!("SUMMARY:{}", escape(&e.title)));
            if !e.location.is_empty() {
                out.push(format!("LOCATION:{}", escape(&e.location)));
            }
            if !e.notes.is_empty() {
                out.push(format!("DESCRIPTION:{}", escape(&e.notes)));
            }
            if let Some(rule) = &e.rrule {
                out.push(format!("RRULE:{rule}"));
            }
            out.push(format!("SEQUENCE:{}", e.sequence));
            out.push(line.clone());
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
        }
        let name = upper.split([':', ';']).next().unwrap_or("");
        if depth > 0 || !REPLACED.contains(&name) {
            out.push(line.clone());
        }
        if upper.starts_with("END:") {
            depth -= 1;
        }
    }
    out.iter().map(|l| fold(l)).collect()
}

/// The answer to an invitation, naming only this person.
pub fn write_reply(e: &IcsEvent, me: &str, my_name: &str, status: &str) -> String {
    let mut answer = e.clone();
    answer.attendees = vec![Attendee {
        address: me.to_string(),
        name: my_name.to_string(),
        status: status.to_string(),
    }];
    answer.notes = String::new();
    wrap(Some("REPLY"), event_lines(&answer))
}

const MOST_STEPS: i64 = 40_000;
const MOST_INTERVAL: i64 = 1_000;

fn weekday_of(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// The nth such weekday of a month, counting from the end when negative.
fn nth_weekday(year: i32, month: u32, day: Weekday, nth: i32) -> Option<NaiveDate> {
    if nth > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let ahead = (7 + day.num_days_from_monday() as i64
            - first.weekday().num_days_from_monday() as i64)
            % 7;
        let date = first.checked_add_signed(Duration::days(ahead + (i64::from(nth) - 1) * 7))?;
        (date.month() == month).then_some(date)
    } else {
        let next = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?
        };
        let last = next.pred_opt()?;
        let back = (7 + last.weekday().num_days_from_monday() as i64
            - day.num_days_from_monday() as i64)
            % 7;
        let date = last.checked_sub_signed(Duration::days(back + (i64::from(-nth) - 1) * 7))?;
        (date.month() == month).then_some(date)
    }
}

/// Every time an event happens that overlaps this period. Repeats follow the clock where
/// the event was set up, so a 10 o'clock meeting stays at 10 through daylight saving.
pub fn occurrences(e: &IcsEvent, from: i64, to: i64) -> Vec<i64> {
    let length = (e.ends - e.starts).max(0);
    let once = || {
        if e.starts < to && e.starts + length > from {
            vec![e.starts]
        } else {
            vec![]
        }
    };
    let Some(rule) = &e.rrule else {
        return once();
    };
    let mut freq = String::new();
    let mut interval = 1i64;
    let mut count: Option<usize> = None;
    let mut until: Option<i64> = None;
    let mut by_day: Vec<(i32, Weekday)> = Vec::new();
    let mut by_month_day: Vec<i32> = Vec::new();
    for part in rule.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => freq = v.to_ascii_uppercase(),
            "INTERVAL" => interval = v.parse().unwrap_or(1),
            "COUNT" => count = v.parse().ok(),
            "UNTIL" => until = read_time(v, &[], 0).map(|x| x.0),
            "BYMONTHDAY" => by_month_day = v.split(',').filter_map(|d| d.parse().ok()).collect(),
            "BYDAY" => {
                by_day = v
                    .split(',')
                    .filter_map(|d| {
                        let d = d.trim();
                        let split = d.len().checked_sub(2)?;
                        let nth = d.get(..split)?.parse().unwrap_or(0);
                        Some((nth, weekday_of(d.get(split..)?)?))
                    })
                    .collect();
            }
            _ => {}
        }
    }
    // A number this far out only comes from a mistake, or from someone trying to cause one.
    if !(1..=MOST_INTERVAL).contains(&interval) {
        return once();
    }
    let tz: Tz =
        e.tz.as_deref()
            .and_then(|n| n.parse().ok())
            .unwrap_or(chrono_tz::UTC);
    let Some(first) = tz.timestamp_opt(e.starts, 0).earliest() else {
        return once();
    };
    let (first_date, first_time) = (first.date_naive(), first.time());
    let mut out = Vec::new();
    let mut seen = 0usize;
    for step in 0..MOST_STEPS {
        let n = step * interval;
        // One period's dates, in order.
        let mut dates: Vec<NaiveDate> = match freq.as_str() {
            "DAILY" => first_date
                .checked_add_signed(Duration::days(n))
                .filter(|d| by_day.is_empty() || by_day.iter().any(|(_, w)| *w == d.weekday()))
                .into_iter()
                .collect(),
            "WEEKLY" => {
                let monday = first_date
                    .checked_sub_signed(Duration::days(i64::from(
                        first_date.weekday().num_days_from_monday(),
                    )))
                    .and_then(|m| m.checked_add_signed(Duration::weeks(n)));
                let Some(monday) = monday else { break };
                if by_day.is_empty() {
                    monday
                        .checked_add_signed(Duration::days(i64::from(
                            first_date.weekday().num_days_from_monday(),
                        )))
                        .into_iter()
                        .collect()
                } else {
                    by_day
                        .iter()
                        .filter_map(|(_, w)| {
                            monday.checked_add_signed(Duration::days(i64::from(
                                w.num_days_from_monday(),
                            )))
                        })
                        .collect()
                }
            }
            "MONTHLY" => {
                let months = i64::from(first_date.year()) * 12 + i64::from(first_date.month0()) + n;
                let (Ok(year), Ok(month)) = (
                    i32::try_from(months.div_euclid(12)),
                    u32::try_from(months.rem_euclid(12) + 1),
                ) else {
                    break;
                };
                if !by_day.is_empty() {
                    by_day
                        .iter()
                        .flat_map(|(nth, w)| {
                            if *nth != 0 {
                                nth_weekday(year, month, *w, *nth)
                                    .into_iter()
                                    .collect::<Vec<_>>()
                            } else {
                                (1..=5)
                                    .filter_map(|k| nth_weekday(year, month, *w, k))
                                    .collect()
                            }
                        })
                        .collect()
                } else if !by_month_day.is_empty() {
                    by_month_day
                        .iter()
                        .filter_map(|d| {
                            if *d > 0 {
                                NaiveDate::from_ymd_opt(year, month, *d as u32)
                            } else {
                                let next = if month == 12 {
                                    NaiveDate::from_ymd_opt(year + 1, 1, 1)?
                                } else {
                                    NaiveDate::from_ymd_opt(year, month + 1, 1)?
                                };
                                next.checked_add_signed(Duration::days(i64::from(*d)))
                            }
                        })
                        .collect()
                } else {
                    // Months without this date are skipped, as calendars do.
                    NaiveDate::from_ymd_opt(year, month, first_date.day())
                        .into_iter()
                        .collect()
                }
            }
            "YEARLY" => i32::try_from(i64::from(first_date.year()) + n)
                .ok()
                .and_then(|y| NaiveDate::from_ymd_opt(y, first_date.month(), first_date.day()))
                .into_iter()
                .collect(),
            _ => return once(),
        };
        dates.sort();
        dates.dedup();
        for date in dates {
            let Some(t) = in_zone(tz, date.and_time(first_time)) else {
                continue;
            };
            if t < e.starts {
                continue;
            }
            if until.is_some_and(|u| t > u) || count.is_some_and(|c| seen >= c) || t >= to {
                return out;
            }
            seen += 1;
            if t + length > from && !e.exdates.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Asia/Dhaka\r\nBEGIN:STANDARD\r\n\
DTSTART:19700101T000000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc@x\r\nSEQUENCE:2\r\n\
DTSTART;TZID=Asia/Dhaka:20261110T100000\r\nDTEND;TZID=Asia/Dhaka:20261110T110000\r\n\
SUMMARY:Launch\\, review\r\nLOCATION:Room 1\r\nDESCRIPTION:Line one\\nLine two with a very long tail that goes on and\r\n  on past the limit\r\n\
ORGANIZER;CN=\"Ana: Boss\":mailto:ana@x.test\r\nATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION:MAILTO:me@x.test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn an_invitation_is_read() {
        let f = parse(INVITE, 0);
        assert_eq!(f.method, "REQUEST");
        let e = &f.events[0];
        assert_eq!(e.uid, "abc@x");
        assert_eq!(e.title, "Launch, review");
        assert_eq!(e.location, "Room 1");
        assert!(e.notes.contains("Line two") && e.notes.contains("and on past"));
        assert_eq!(e.starts, 1_794_283_200, "10:00 in Dhaka is 04:00 UTC");
        assert_eq!(e.ends - e.starts, 3_600);
        assert_eq!(e.organizer.as_deref(), Some("ana@x.test"));
        assert_eq!(e.organizer_name, "Ana: Boss");
        assert_eq!(e.attendees[0].address, "me@x.test");
        assert_eq!(e.sequence, 2);
    }

    #[test]
    fn whole_days_durations_and_outlook_zones_work() {
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART;VALUE=DATE:20261225\nEND:VEVENT\n",
            0,
        );
        assert!(f.events[0].all_day);
        assert_eq!(f.events[0].ends - f.events[0].starts, 86_400);
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART:20261225T100000Z\nDURATION:PT1H30M\nEND:VEVENT\n",
            0,
        );
        assert_eq!(f.events[0].ends - f.events[0].starts, 5_400);
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=Eastern Standard Time:20260115T090000\nEND:VEVENT\n",
            0,
        );
        assert_eq!(f.events[0].starts, 1_768_485_600);
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let e = IcsEvent {
            uid: "u1".into(),
            title: "Lunch, with; friends\nand more".into(),
            location: "Café".into(),
            notes: "x".repeat(300),
            starts: 1_800_000_000,
            ends: 1_800_003_600,
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            ..Default::default()
        };
        let back = &parse(&write_event(&e), 0).events[0];
        assert_eq!(back.title, e.title);
        assert_eq!(back.location, e.location);
        assert_eq!(back.notes, e.notes);
        assert_eq!(back.starts, e.starts);
        assert_eq!(back.rrule, e.rrule);
    }

    #[test]
    fn replies_name_only_this_person() {
        let e = &parse(INVITE, 0).events[0];
        let reply = parse(&write_reply(e, "me@x.test", "Me", "ACCEPTED"), 0);
        assert_eq!(reply.method, "REPLY");
        assert_eq!(reply.events[0].attendees.len(), 1);
        assert_eq!(reply.events[0].attendees[0].status, "ACCEPTED");
    }

    #[test]
    fn repeats_are_worked_out() {
        let monday = Utc
            .with_ymd_and_hms(2026, 11, 2, 10, 0, 0)
            .unwrap()
            .timestamp();
        let week = 7 * 86_400;
        let e = IcsEvent {
            uid: "r".into(),
            starts: monday,
            ends: monday + 3_600,
            rrule: Some("FREQ=WEEKLY;COUNT=3".into()),
            ..Default::default()
        };
        assert_eq!(
            occurrences(&e, monday, monday + 10 * week),
            vec![monday, monday + week, monday + 2 * week]
        );
        let mut skip = e.clone();
        skip.exdates = vec![monday + week];
        assert_eq!(occurrences(&skip, monday, monday + 10 * week).len(), 2);
        let mut days = e.clone();
        days.rrule = Some("FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4".into());
        let list = occurrences(&days, monday, monday + 10 * week);
        assert_eq!(list.len(), 4);
        assert_eq!(list[1] - list[0], 2 * 86_400);
        let mut daily = e.clone();
        daily.rrule = Some(format!("FREQ=DAILY;UNTIL={}", stamp(monday + 2 * 86_400)));
        assert_eq!(occurrences(&daily, monday, monday + week).len(), 3);
        let mut monthly = e.clone();
        monthly.rrule = Some("FREQ=MONTHLY".into());
        assert_eq!(occurrences(&monthly, monday, monday + 70 * 86_400).len(), 3);
        // Only what's in the period asked for, however long it's been repeating.
        let mut forever = e.clone();
        forever.rrule = Some("FREQ=DAILY".into());
        assert_eq!(
            occurrences(&forever, monday + 400 * 86_400, monday + 402 * 86_400).len(),
            2
        );
    }

    #[test]
    fn repeats_keep_their_clock_time_through_daylight_saving() {
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=America/New_York:20260301T100000\nDTEND;TZID=America/New_York:20260301T110000\nRRULE:FREQ=WEEKLY;COUNT=3\nEND:VEVENT\n",
            0,
        );
        let e = &f.events[0];
        let list = occurrences(e, e.starts, e.starts + 40 * 86_400);
        assert_eq!(list.len(), 3);
        // The clocks change on 8 March, so the UTC gap is an hour short.
        assert_eq!(list[1] - list[0], 7 * 86_400 - 3_600);
    }

    #[test]
    fn weekdays_are_counted_where_the_event_is() {
        // 00:30 on a Monday in Dhaka is still Sunday in UTC.
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=Asia/Dhaka:20261102T003000\nDTEND;TZID=Asia/Dhaka:20261102T013000\nRRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=2\nEND:VEVENT\n",
            0,
        );
        let e = &f.events[0];
        let list = occurrences(e, e.starts, e.starts + 30 * 86_400);
        assert_eq!(list, vec![e.starts, e.starts + 7 * 86_400]);
    }

    #[test]
    fn monthly_and_weekday_rules_work() {
        let start = Utc
            .with_ymd_and_hms(2026, 11, 10, 10, 0, 0)
            .unwrap()
            .timestamp();
        let mut e = IcsEvent {
            uid: "m".into(),
            starts: start,
            ends: start + 3_600,
            rrule: Some("FREQ=MONTHLY;BYDAY=2TU;COUNT=3".into()),
            ..Default::default()
        };
        let list = occurrences(&e, start, start + 200 * 86_400);
        assert_eq!(list.len(), 3);
        let second = DateTime::<Utc>::from_timestamp(list[1], 0).unwrap();
        assert_eq!(
            (second.month(), second.day(), second.weekday()),
            (12, 8, Weekday::Tue)
        );
        e.rrule = Some("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;COUNT=7".into());
        let list = occurrences(&e, start, start + 30 * 86_400);
        assert_eq!(list.len(), 7);
        assert!(list.iter().all(|t| {
            let d = DateTime::<Utc>::from_timestamp(*t, 0).unwrap();
            !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
        }));
        e.rrule = Some("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=2".into());
        assert_eq!(occurrences(&e, start, start + 100 * 86_400).len(), 2);
    }

    #[test]
    fn numbers_meant_to_break_it_dont() {
        let start = 1_800_000_000;
        for rule in [
            "FREQ=DAILY;INTERVAL=100000000",
            "FREQ=YEARLY;INTERVAL=2000000000",
            "FREQ=MONTHLY;INTERVAL=-5",
            "FREQ=WEEKLY;INTERVAL=999999999999999999999",
            "FREQ=DAILY;BYDAY=X",
            "FREQ=MONTHLY;BYMONTHDAY=9999",
            "FREQ=MONTHLY;BYDAY=99MO",
        ] {
            let e = IcsEvent {
                uid: "x".into(),
                starts: start,
                ends: start + 60,
                rrule: Some(rule.into()),
                ..Default::default()
            };
            occurrences(&e, start, start + 86_400 * 700);
        }
        // Very far from the event's start, it still answers quickly.
        let e = IcsEvent {
            uid: "x".into(),
            starts: 0,
            ends: 60,
            rrule: Some("FREQ=DAILY;INTERVAL=1000".into()),
            ..Default::default()
        };
        occurrences(&e, i64::MAX - 10_000_000, i64::MAX - 1);
    }

    #[test]
    fn zones_named_by_their_offset_and_times_the_clocks_skip_still_work() {
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=\"(UTC+06:00) Astana\":20270115T100000\nEND:VEVENT\n",
            0,
        );
        assert_eq!(
            f.events[0].starts,
            Utc.with_ymd_and_hms(2027, 1, 15, 4, 0, 0)
                .unwrap()
                .timestamp()
        );
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=America/New_York:20260308T023000\nEND:VEVENT\n",
            0,
        );
        assert_eq!(
            f.events.len(),
            1,
            "2:30 doesn't exist that night, and it's kept anyway"
        );
        let f = parse(
            "BEGIN:VEVENT\nUID:a\nDURATION:PT2H\nDTSTART:20261225T100000Z\nEND:VEVENT\n",
            0,
        );
        assert_eq!(f.events[0].ends - f.events[0].starts, 7_200);
    }

    #[test]
    fn changing_an_event_keeps_what_it_doesnt_understand() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20270101T100000Z\r\nDTEND:20270101T110000Z\r\nSUMMARY:Old\r\nRRULE:FREQ=WEEKLY\r\nBEGIN:VALARM\r\nDESCRIPTION:Ding\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID:20270108T100000Z\r\nDTSTART:20270108T120000Z\r\nDTEND:20270108T130000Z\r\nSUMMARY:Moved one\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut e = parse(raw, 0).events[0].clone();
        e.title = "New".into();
        let out = patch_event(raw, &e);
        assert!(out.contains("SUMMARY:New") && !out.contains("SUMMARY:Old"));
        assert!(
            out.contains("BEGIN:VALARM") && out.contains("DESCRIPTION:Ding"),
            "{out}"
        );
        assert!(out.contains("Moved one") && out.contains("RECURRENCE-ID"));
        assert_eq!(out.matches("RRULE:FREQ=WEEKLY").count(), 1);
    }

    #[test]
    fn nonsense_doesnt_break_it() {
        assert!(parse("not a calendar", 0).events.is_empty());
        assert!(parse("BEGIN:VEVENT\nSUMMARY:No time\nEND:VEVENT", 0)
            .events
            .is_empty());
        assert!(parse("BEGIN:VEVENT\nUID:x\nDTSTART:garbage\nEND:VEVENT", 0)
            .events
            .is_empty());
    }
}
