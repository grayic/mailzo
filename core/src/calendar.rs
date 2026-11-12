//! The calendar. Everything is kept on the device, and a calendar connected to a
//! provider is kept in step with it directly, with nothing passing through our servers.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::ics::{self, Attendee, IcsEvent};
use crate::identity::unix_now;
use crate::model::*;
use crate::net::Reach;
use crate::parse;
use crate::secrets::account_secret;
use crate::store::Store;
use reqwest::Method;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const GOOGLE_CALENDAR: &str =
    "https://www.googleapis.com/calendar/v3/calendars/primary/events";
pub(crate) const MICROSOFT_CALENDAR: &str = "https://graph.microsoft.com/v1.0/me";
const GRAPH_SCOPE: &str = "https://graph.microsoft.com/Calendars.ReadWrite";
const SYNC_EVERY_SECONDS: i64 = 15 * 60;
const SYNC_BACK_DAYS: i64 = 365;
const SYNC_AHEAD_DAYS: i64 = 2 * 365;
const REMIND_BEFORE_SECONDS: i64 = 10 * 60;

#[derive(Debug, Clone, Serialize)]
pub struct Calendar {
    pub id: i64,
    pub account_id: Option<i64>,
    /// `local`, `caldav`, `google` or `microsoft`.
    pub kind: String,
    pub name: String,
    pub hue: i64,
    pub problem: Option<String>,
}

/// One time an event happens.
#[derive(Debug, Clone, Serialize)]
pub struct Occurrence {
    pub event_id: i64,
    pub calendar_id: i64,
    pub title: String,
    pub location: String,
    pub notes: String,
    pub starts: i64,
    pub ends: i64,
    pub all_day: bool,
    pub repeats: bool,
    pub my_status: String,
    pub organizer: Option<String>,
    pub attendees: Vec<Attendee>,
    pub hue: i64,
    /// A change to one time of a repeating event.
    pub is_override: bool,
}

#[derive(Debug, Deserialize)]
pub struct EventInput {
    #[serde(default)]
    pub id: Option<i64>,
    pub calendar_id: i64,
    pub title: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub notes: String,
    pub starts: i64,
    pub ends: i64,
    #[serde(default)]
    pub all_day: bool,
    /// `daily`, `weekly`, `monthly`, `yearly`, or nothing. Left out, an edit keeps what it has.
    #[serde(default)]
    pub repeat: Option<String>,
    /// Which time of a repeating event was being changed.
    #[serde(default)]
    pub occurrence: Option<i64>,
}

struct SavedEvent {
    id: i64,
    calendar_id: i64,
    href: Option<String>,
    etag: Option<String>,
    event: IcsEvent,
    my_status: String,
    raw: Option<String>,
    is_override: bool,
}

/// What's stored besides the event itself.
#[derive(Default)]
struct EventExtras<'a> {
    href: Option<&'a str>,
    etag: Option<&'a str>,
    raw: Option<&'a str>,
    is_override: bool,
}

const EVENT_COLUMNS: &str =
    "id, calendar_id, uid, href, etag, title, location, notes, starts, ends, all_day,
    rrule, exdates_json, organizer, attendees_json, my_status, sequence, tz, raw, is_override";

fn row_to_event(r: &Row<'_>) -> rusqlite::Result<SavedEvent> {
    Ok(SavedEvent {
        id: r.get(0)?,
        calendar_id: r.get(1)?,
        href: r.get(3)?,
        etag: r.get(4)?,
        my_status: r.get(15)?,
        raw: r.get(18)?,
        is_override: r.get(19)?,
        event: IcsEvent {
            uid: r.get(2)?,
            title: r.get(5)?,
            location: r.get(6)?,
            notes: r.get(7)?,
            starts: r.get(8)?,
            ends: r.get(9)?,
            all_day: r.get(10)?,
            rrule: r.get(11)?,
            exdates: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
            organizer: r.get(13)?,
            organizer_name: String::new(),
            attendees: serde_json::from_str(&r.get::<_, String>(14)?).unwrap_or_default(),
            sequence: r.get(16)?,
            cancelled: false,
            tz: r.get(17)?,
            recurrence_id: None,
        },
    })
}

impl Store {
    pub fn calendars(&self) -> Result<Vec<Calendar>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, account_id, kind, name, hue, problem FROM calendars ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Calendar {
                id: r.get(0)?,
                account_id: r.get(1)?,
                kind: r.get(2)?,
                name: r.get(3)?,
                hue: r.get(4)?,
                problem: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// There's always somewhere to put an event.
    pub fn own_calendar(&self) -> Result<i64> {
        let db = self.db();
        let found: Option<i64> = db
            .query_row(
                "SELECT id FROM calendars WHERE kind = 'local' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = found {
            return Ok(id);
        }
        db.execute(
            "INSERT INTO calendars (kind, name, hue) VALUES ('local', 'My calendar', 230)",
            [],
        )?;
        Ok(db.last_insert_rowid())
    }

    fn add_calendar(
        &self,
        account_id: i64,
        kind: &str,
        url: &str,
        name: &str,
        hue: i64,
    ) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT OR IGNORE INTO calendars (account_id, kind, url, name, hue) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account_id, kind, url, name, hue],
        )?;
        Ok(db.query_row(
            "SELECT id FROM calendars WHERE account_id = ?1 AND kind = ?2 AND url = ?3",
            params![account_id, kind, url],
            |r| r.get(0),
        )?)
    }

    fn calendar_url(&self, id: i64) -> Result<(Option<i64>, String, String)> {
        self.db()
            .query_row(
                "SELECT account_id, kind, COALESCE(url, '') FROM calendars WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("calendar".into()))
    }

    fn calendar_rows_to_sync(&self) -> Result<Vec<(i64, i64, String, String)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, account_id, kind, COALESCE(url, '') FROM calendars
             WHERE kind != 'local' AND account_id IS NOT NULL ORDER BY id",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn note_calendar_problem(&self, id: i64, problem: Option<&str>) -> Result<()> {
        self.db().execute(
            "UPDATE calendars SET problem = ?2 WHERE id = ?1",
            params![id, problem],
        )?;
        Ok(())
    }

    fn save_event(
        &self,
        calendar_id: i64,
        extras: EventExtras<'_>,
        e: &IcsEvent,
        my_status: &str,
    ) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO events (calendar_id, uid, href, etag, title, location, notes, starts, ends, all_day,
                                 rrule, exdates_json, organizer, attendees_json, my_status, sequence, tz, raw, is_override)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT (calendar_id, uid) DO UPDATE SET
               href = COALESCE(?3, href), etag = COALESCE(?4, etag), title = ?5, location = ?6, notes = ?7,
               starts = ?8, ends = ?9, all_day = ?10, rrule = ?11, exdates_json = ?12, organizer = ?13,
               attendees_json = ?14, my_status = ?15, sequence = ?16, tz = ?17, raw = COALESCE(?18, raw),
               is_override = ?19",
            params![
                calendar_id,
                e.uid,
                extras.href,
                extras.etag,
                e.title,
                e.location,
                e.notes,
                e.starts,
                e.ends,
                e.all_day,
                e.rrule,
                serde_json::to_string(&e.exdates).unwrap_or_else(|_| "[]".into()),
                e.organizer,
                serde_json::to_string(&e.attendees).unwrap_or_else(|_| "[]".into()),
                my_status,
                e.sequence,
                e.tz,
                extras.raw,
                extras.is_override,
            ],
        )?;
        Ok(db.query_row(
            "SELECT id FROM events WHERE calendar_id = ?1 AND uid = ?2",
            params![calendar_id, e.uid],
            |r| r.get(0),
        )?)
    }

    fn event(&self, id: i64) -> Result<SavedEvent> {
        self.db()
            .query_row(
                &format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = ?1"),
                [id],
                row_to_event,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("event".into()))
    }

    /// Which inbox an invitation came to.
    pub(crate) fn set_event_inbox(&self, event_id: i64, account_id: i64) -> Result<()> {
        self.db().execute(
            "UPDATE events SET from_account_id = ?2 WHERE id = ?1",
            [event_id, account_id],
        )?;
        Ok(())
    }

    /// Events on these inboxes' calendars, or that came to them as invitations.
    pub(crate) fn events_of_inboxes(
        &self,
        account_ids: &[i64],
    ) -> Result<std::collections::HashSet<i64>> {
        if account_ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let list = account_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT e.id FROM events e JOIN calendars c ON c.id = e.calendar_id
             WHERE c.account_id IN ({list}) OR e.from_account_id IN ({list})"
        ))?;
        let ids = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// An event as a question about mail reads it: when, where, who and what it's about.
    pub(crate) fn event_for_question(&self, id: i64) -> Result<Option<serde_json::Value>> {
        let Ok(saved) = self.event(id) else {
            return Ok(None);
        };
        let e = &saved.event;
        Ok(Some(serde_json::json!({
            "event_id": id, "title": e.title, "starts": e.starts, "location": e.location,
            "notes": e.notes, "organizer": e.organizer, "attendees": e.attendees,
        })))
    }

    /// The copy on a connected calendar if there is one, over one kept only here.
    fn event_with_uid(&self, uid: &str) -> Result<Option<SavedEvent>> {
        Ok(self
            .db()
            .query_row(
                &format!(
                    "SELECT {EVENT_COLUMNS} FROM events WHERE uid = ?1
                     ORDER BY (SELECT kind = 'local' FROM calendars c WHERE c.id = events.calendar_id), id LIMIT 1"
                ),
                [uid],
                row_to_event,
            )
            .optional()?)
    }

    fn add_exdate(&self, event_id: i64, when: i64) -> Result<()> {
        let mut saved = self.event(event_id)?;
        if !saved.event.exdates.contains(&when) {
            saved.event.exdates.push(when);
            self.db().execute(
                "UPDATE events SET exdates_json = ?2 WHERE id = ?1",
                params![
                    event_id,
                    serde_json::to_string(&saved.event.exdates).unwrap_or_else(|_| "[]".into())
                ],
            )?;
        }
        Ok(())
    }

    fn delete_event_row(&self, id: i64) -> Result<()> {
        self.db()
            .execute("DELETE FROM events WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The single changed times of a repeating event go when it does.
    fn delete_changed_times(&self, calendar_id: i64, uid: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM events WHERE calendar_id = ?1 AND is_override = 1 AND substr(uid, 1, ?3) = ?2",
            params![calendar_id, format!("{uid}#"), uid.chars().count() as i64 + 1],
        )?;
        Ok(())
    }

    /// Events from a provider that are no longer there have gone from this device too.
    fn remove_events_not_in(&self, calendar_id: i64, kept: &[String]) -> Result<()> {
        let held: Vec<(i64, String)> = {
            let db = self.db();
            let mut st = db.prepare("SELECT id, uid FROM events WHERE calendar_id = ?1")?;
            let rows = st.query_map([calendar_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, uid) in held {
            if !kept.contains(&uid) {
                self.delete_event_row(id)?;
            }
        }
        Ok(())
    }

    fn events_touching(&self, from: i64, to: i64) -> Result<Vec<(SavedEvent, i64)>> {
        let db = self.db();
        // Connected calendars first, so a copy kept only here gives way to the real one.
        let mut st = db.prepare(&format!(
            "SELECT e.{}, c.hue FROM events e JOIN calendars c ON c.id = e.calendar_id
             WHERE e.starts < ?2 AND (e.ends > ?1 OR e.rrule IS NOT NULL)
             ORDER BY c.kind = 'local'",
            EVENT_COLUMNS.replace(", ", ", e.").replace("\n    ", " ")
        ))?;
        let rows = st.query_map(params![from, to], |r| {
            Ok((row_to_event(r)?, r.get::<_, i64>(20)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// True the first time it's asked about each occurrence, for one reminder each.
    fn first_reminder(&self, event_id: i64, starts: i64) -> Result<bool> {
        Ok(self.db().execute(
            "INSERT OR IGNORE INTO event_alerts (event_id, starts) VALUES (?1, ?2)",
            params![event_id, starts],
        )? == 1)
    }
}

fn rule_for(repeat: Option<&str>) -> Result<Option<String>> {
    Ok(match repeat {
        None | Some("") | Some("none") => None,
        Some("daily") => Some("FREQ=DAILY".into()),
        Some("weekly") => Some("FREQ=WEEKLY".into()),
        Some("monthly") => Some("FREQ=MONTHLY".into()),
        Some("yearly") => Some("FREQ=YEARLY".into()),
        Some(_) => return Err(CoreError::Invalid("Bad request.".into())),
    })
}

fn local_offset() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}

/// Text inside every tag with this name, whatever prefix the server gives it.
fn texts_in(xml: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        let bare = tag.split_whitespace().next().unwrap_or("");
        let local = bare.rsplit(':').next().unwrap_or(bare);
        if local != name || tag.ends_with('/') || bare.starts_with('/') {
            continue;
        }
        let after = &rest[end + 1..];
        let close = format!("</{bare}>");
        let Some(stop) = after.find(&close) else {
            continue;
        };
        out.push(after[..stop].to_string());
    }
    out
}

fn xml_text(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(inner) = raw
        .strip_prefix("<![CDATA[")
        .and_then(|r| r.strip_suffix("]]>"))
    {
        return inner.to_string();
    }
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#13;", "")
        .replace("&amp;", "&")
}

/// Each `<response>` in a reply, split apart.
fn responses(xml: &str) -> Vec<String> {
    texts_in(xml, "response")
}

fn absolute(base: &str, href: &str) -> String {
    match reqwest::Url::parse(base).and_then(|b| b.join(href.trim())) {
        Ok(url) => url.to_string(),
        Err(_) => href.trim().to_string(),
    }
}

/// This exact host or one under it, so `imap.acme.com` isn't taken for `me.com`.
pub(crate) fn host_is(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// The part of a host name a company owns, near enough: `p12-caldav.icloud.com` is `icloud.com`.
fn owned_domain(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    let keep = match labels.as_slice() {
        [.., second, tld]
            if tld.len() == 2 && matches!(*second, "co" | "com" | "org" | "net" | "ac" | "gov") =>
        {
            3
        }
        _ => 2,
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

impl Engine {
    async fn dav(
        &self,
        account: &Account,
        method: &str,
        url: &str,
        depth: Option<&str>,
        body: String,
    ) -> Result<(u16, String, Option<String>)> {
        self.dav_if(account, method, url, depth, body, None).await
    }

    /// Where a person's password may be sent for this inbox's calendar. Nothing a server
    /// says can widen this, and it's checked again on every request.
    fn calendar_host_allowed(&self, account: &Account, url: &str) -> bool {
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return false;
        };
        let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
            return false;
        };
        if let Some(base) = &self.caldav_base {
            // Stand-ins for tests, on this computer.
            return reqwest::Url::parse(base).is_ok_and(|b| {
                b.host_str() == parsed.host_str()
                    && b.port_or_known_default() == parsed.port_or_known_default()
            });
        }
        if parsed.scheme() != "https" {
            return false;
        }
        self.caldav_start_points(account).iter().any(|start| {
            reqwest::Url::parse(start)
                .ok()
                .and_then(|s| s.host_str().map(owned_domain))
                .is_some_and(|d| host_is(&host, &d))
        })
    }

    /// With `if_match`, the server refuses if it's been changed since this copy.
    async fn dav_if(
        &self,
        account: &Account,
        method: &str,
        url: &str,
        depth: Option<&str>,
        body: String,
        if_match: Option<&str>,
    ) -> Result<(u16, String, Option<String>)> {
        let password = self
            .secrets
            .get(&account_secret(account.id))?
            .ok_or_else(|| CoreError::Secret("The password for this account is missing.".into()))?;
        let verb = Method::from_bytes(method.as_bytes())
            .map_err(|_| CoreError::Invalid("Bad request.".into()))?;
        let kind = if method == "PUT" {
            "text/calendar; charset=utf-8"
        } else {
            "application/xml; charset=utf-8"
        };
        let mut target = url.to_string();
        // Calendars often sit behind a redirect or two. Each hop is checked like the first.
        for _ in 0..4 {
            if !self.calendar_host_allowed(account, &target) {
                return Err(CoreError::Invalid(
                    "That calendar pointed somewhere else, so your password wasn't sent there."
                        .into(),
                ));
            }
            let client = match self.reach() {
                Reach::Anywhere => self.ai_http.clone(),
                // Every hop's addresses are checked, and that hop goes to exactly the
                // address checked, whether it's the first or a redirect.
                Reach::PublicOnly => {
                    crate::net::public_pinned_client(&target, true, crate::ai::AI_TIMEOUT)
                        .await
                        .ok_or_else(|| {
                            CoreError::Invalid(
                                "That calendar isn't on the public internet, so it can't be reached from here."
                                    .into(),
                            )
                        })?
                }
            };
            let mut request = client
                .request(verb.clone(), &target)
                .basic_auth(&account.username, Some(&password))
                .header("content-type", kind)
                .body(body.clone());
            if let Some(depth) = depth {
                request = request.header("depth", depth);
            }
            if let Some(tag) = if_match {
                request = request.header("if-match", tag);
            }
            let reply = request
                .send()
                .await
                .map_err(|e| CoreError::Network(e.to_string()))?;
            let status = reply.status().as_u16();
            if (300..400).contains(&status) {
                let Some(next) = reply
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                else {
                    return Err(CoreError::Invalid(format!(
                        "The calendar answered with {status}."
                    )));
                };
                target = absolute(&target, next);
                continue;
            }
            if status == 401 || status == 403 {
                return Err(CoreError::Auth);
            }
            let etag = reply
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            // A reply cut short mustn't look like an empty calendar.
            let text = reply
                .text()
                .await
                .map_err(|e| CoreError::Network(e.to_string()))?;
            return Ok((status, text, etag));
        }
        Err(CoreError::Invalid("The calendar kept redirecting.".into()))
    }

    /// Finds an inbox's calendars. Providers publish where they are in a standard way.
    pub(crate) async fn calendar_connect(&self, account_id: i64) -> Result<Value> {
        let account = self.store.account(account_id)?;
        match account.auth.as_str() {
            "google" => {
                self.store
                    .add_calendar(account_id, "google", "primary", &account.email, 145)?;
            }
            "microsoft" => {
                self.store
                    .add_calendar(account_id, "microsoft", "me", &account.email, 205)?;
            }
            _ => {
                let found = self.caldav_discover(&account).await?;
                if found.is_empty() {
                    return Err(CoreError::Invalid(format!(
                        "{} doesn't offer a calendar this app can connect to.",
                        account.email
                    )));
                }
                let mut hue = 175;
                for (url, name) in found {
                    self.store
                        .add_calendar(account_id, "caldav", &url, &name, hue)?;
                    hue = (hue + 40) % 360;
                }
            }
        }
        self.calendar_sync(true).await?;
        self.calendar_list()
    }

    fn caldav_start_points(&self, account: &Account) -> Vec<String> {
        if let Some(fixed) = &self.caldav_base {
            return vec![fixed.clone()];
        }
        let domain = account
            .email
            .rsplit('@')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let host = account.imap_host.to_ascii_lowercase();
        if host_is(&host, "icloud.com") || host_is(&host, "me.com") {
            return vec!["https://caldav.icloud.com/".to_string()];
        }
        if host_is(&host, "fastmail.com") {
            return vec!["https://caldav.fastmail.com/dav/".to_string()];
        }
        if host_is(&host, "zoho.com") {
            return vec!["https://calendar.zoho.com/caldav/".to_string()];
        }
        if host_is(&host, "yahoo.com") {
            return vec!["https://caldav.calendar.yahoo.com/".to_string()];
        }
        // A company's own domain is only trusted with the mail password if the mail server
        // is theirs too. Otherwise it may just be a website hosted by somebody else.
        if owned_domain(&host) == owned_domain(&domain) {
            return vec![format!("https://{domain}/.well-known/caldav")];
        }
        vec![]
    }

    async fn caldav_discover(&self, account: &Account) -> Result<Vec<(String, String)>> {
        const PRINCIPAL: &str = r#"<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
        const HOME: &str = r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#;
        const LIST: &str = r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:displayname/><d:resourcetype/><c:supported-calendar-component-set/></d:prop></d:propfind>"#;
        let mut last_problem = None;
        for start in self.caldav_start_points(account) {
            let attempt: Result<Vec<(String, String)>> = async {
                let (_, xml, _) = self
                    .dav(account, "PROPFIND", &start, Some("0"), PRINCIPAL.into())
                    .await?;
                let principal = texts_in(&xml, "current-user-principal")
                    .first()
                    .and_then(|p| texts_in(p, "href").into_iter().next())
                    .ok_or_else(|| CoreError::Invalid("No calendar here.".into()))?;
                let principal = absolute(&start, &xml_text(&principal));
                let (_, xml, _) = self
                    .dav(account, "PROPFIND", &principal, Some("0"), HOME.into())
                    .await?;
                let home = texts_in(&xml, "calendar-home-set")
                    .first()
                    .and_then(|p| texts_in(p, "href").into_iter().next())
                    .ok_or_else(|| CoreError::Invalid("No calendar here.".into()))?;
                let home = absolute(&principal, &xml_text(&home));
                let (_, xml, _) = self
                    .dav(account, "PROPFIND", &home, Some("1"), LIST.into())
                    .await?;
                let mut out = Vec::new();
                for one in responses(&xml) {
                    let is_calendar = texts_in(&one, "resourcetype")
                        .iter()
                        .any(|t| t.contains("calendar"));
                    // Reminders and task lists aren't calendars.
                    let holds_events = {
                        let set = texts_in(&one, "supported-calendar-component-set");
                        set.is_empty() || set.iter().any(|s| s.contains("VEVENT"))
                    };
                    if !is_calendar || !holds_events {
                        continue;
                    }
                    let Some(href) = texts_in(&one, "href").into_iter().next() else {
                        continue;
                    };
                    let name = texts_in(&one, "displayname")
                        .into_iter()
                        .next()
                        .map(|n| xml_text(&n))
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| account.email.clone());
                    out.push((absolute(&home, &xml_text(&href)), name));
                }
                Ok(out)
            }
            .await;
            match attempt {
                Ok(found) if !found.is_empty() => return Ok(found),
                Ok(_) => {}
                Err(CoreError::Auth) => return Err(CoreError::Auth),
                Err(e) => last_problem = Some(e),
            }
        }
        match last_problem {
            Some(CoreError::Network(e)) => Err(CoreError::Network(e)),
            _ => Ok(vec![]),
        }
    }

    /// Brings connected calendars up to date, at most every so often unless asked.
    pub(crate) async fn calendar_sync(&self, now: bool) -> Result<()> {
        let last: i64 = self
            .store
            .setting("calendar_synced_at")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if !now && unix_now() - last < SYNC_EVERY_SECONDS {
            return Ok(());
        }
        self.store
            .set_setting("calendar_synced_at", &unix_now().to_string())?;
        for (id, account_id, kind, url) in self.store.calendar_rows_to_sync()? {
            let Ok(account) = self.store.account(account_id) else {
                continue;
            };
            let done = match kind.as_str() {
                "caldav" => self.sync_caldav(id, &account, &url).await,
                "google" => self.sync_google(id, &account).await,
                "microsoft" => self.sync_microsoft(id, &account).await,
                _ => Ok(()),
            };
            self.store
                .note_calendar_problem(id, done.as_ref().err().map(|e| e.to_string()).as_deref())?;
        }
        Ok(())
    }

    async fn sync_caldav(&self, id: i64, account: &Account, url: &str) -> Result<()> {
        let (from, to) = (
            unix_now() - SYNC_BACK_DAYS * 86_400,
            unix_now() + SYNC_AHEAD_DAYS * 86_400,
        );
        let stamp = |t: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0)
                .unwrap_or_default()
                .format("%Y%m%dT%H%M%SZ")
                .to_string()
        };
        let body = format!(
            r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{}" end="{}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#,
            stamp(from),
            stamp(to)
        );
        let (status, xml, _) = self.dav(account, "REPORT", url, Some("1"), body).await?;
        // Anything else isn't the list, and treating it as one would empty the calendar.
        if status != 207 {
            return Err(CoreError::Invalid(format!(
                "The calendar answered with error {status}."
            )));
        }
        let mut kept = Vec::new();
        for one in responses(&xml) {
            let (Some(href), Some(data)) = (
                texts_in(&one, "href").into_iter().next(),
                texts_in(&one, "calendar-data").into_iter().next(),
            ) else {
                continue;
            };
            let etag = texts_in(&one, "getetag")
                .into_iter()
                .next()
                .map(|e| xml_text(&e));
            let href = absolute(url, &xml_text(&href));
            let text = xml_text(&data);
            let parsed = ics::parse(&text, local_offset());
            // The main entry holds the pattern. Changes to single repeats arrive with the same
            // name, each saying which time it replaces.
            let replaced: Vec<i64> = parsed
                .events
                .iter()
                .filter_map(|e| e.recurrence_id)
                .collect();
            for mut e in parsed.events {
                if let Some(when) = e.recurrence_id {
                    e.uid = format!("{}#{when}", e.uid);
                    if e.cancelled {
                        continue;
                    }
                } else {
                    e.exdates.extend(&replaced);
                    if e.cancelled {
                        continue;
                    }
                }
                let mine = my_status_in(&e, account);
                kept.push(e.uid.clone());
                self.store.save_event(
                    id,
                    EventExtras {
                        href: Some(&href),
                        etag: etag.as_deref(),
                        raw: Some(&text),
                        is_override: e.recurrence_id.is_some(),
                    },
                    &e,
                    &mine,
                )?;
            }
        }
        self.store.remove_events_not_in(id, &kept)
    }

    async fn sync_google(&self, id: i64, account: &Account) -> Result<()> {
        let token = self.password(account.id).await?;
        let (from, to) = (
            unix_now() - SYNC_BACK_DAYS * 86_400,
            unix_now() + SYNC_AHEAD_DAYS * 86_400,
        );
        let rfc = |t: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0)
                .unwrap_or_default()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        };
        let mut kept = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut query: Vec<(&str, String)> = vec![
                ("singleEvents", "true".into()),
                ("maxResults", "2500".into()),
                ("timeMin", rfc(from)),
                ("timeMax", rfc(to)),
            ];
            if let Some(p) = &page {
                query.push(("pageToken", p.clone()));
            }
            let url = reqwest::Url::parse_with_params(&self.google_calendar, &query)
                .map_err(|_| CoreError::Invalid("Bad request.".into()))?;
            let value = self
                .calendar_json(self.ai_http.get(url).bearer_auth(&token), account)
                .await?;
            // Not the list it should be, so nothing here is taken to have gone.
            let Some(items) = value["items"].as_array() else {
                return Err(CoreError::Invalid(
                    "The calendar gave an unexpected answer.".into(),
                ));
            };
            for item in items {
                if item["status"] == "cancelled" {
                    continue;
                }
                let time = |side: &str| -> Option<(i64, bool)> {
                    if let Some(d) = item[side]["date"].as_str() {
                        let date = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
                        return Some((date.and_hms_opt(0, 0, 0)?.and_utc().timestamp(), true));
                    }
                    let t = chrono::DateTime::parse_from_rfc3339(item[side]["dateTime"].as_str()?)
                        .ok()?;
                    Some((t.timestamp(), false))
                };
                let (Some((starts, all_day)), Some((ends, _))) = (time("start"), time("end"))
                else {
                    continue;
                };
                let provider_id = item["id"].as_str().unwrap_or("").to_string();
                // The name calendars share, so an invitation in email is matched to this.
                let uid = item["iCalUID"].as_str().unwrap_or(&provider_id).to_string();
                let attendees: Vec<Attendee> = item["attendees"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| Attendee {
                        address: a["email"].as_str().unwrap_or("").to_string(),
                        name: a["displayName"].as_str().unwrap_or("").to_string(),
                        status: match a["responseStatus"].as_str().unwrap_or("") {
                            "accepted" => "ACCEPTED",
                            "declined" => "DECLINED",
                            "tentative" => "TENTATIVE",
                            _ => "NEEDS-ACTION",
                        }
                        .to_string(),
                    })
                    .collect();
                let e = IcsEvent {
                    uid: uid.clone(),
                    title: item["summary"].as_str().unwrap_or("(no title)").to_string(),
                    location: item["location"].as_str().unwrap_or("").to_string(),
                    notes: item["description"].as_str().unwrap_or("").to_string(),
                    starts,
                    ends,
                    all_day,
                    organizer: item["organizer"]["email"].as_str().map(str::to_string),
                    attendees,
                    ..Default::default()
                };
                let mine = my_status_in(&e, account);
                kept.push(uid.clone());
                self.store.save_event(
                    id,
                    EventExtras {
                        href: Some(&provider_id),
                        etag: item["etag"].as_str(),
                        ..Default::default()
                    },
                    &e,
                    &mine,
                )?;
            }
            page = value["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                break;
            }
        }
        self.store.remove_events_not_in(id, &kept)
    }

    async fn graph_calendar_token(&self, account: &Account) -> Result<String> {
        let refresh = self
            .secrets
            .get(&account_secret(account.id))?
            .ok_or_else(|| CoreError::NeedsSignIn(account.email.clone()))?;
        let config = self.sign_in_for_provider(&account.auth)?;
        let reply = self
            .tokens_for_scope(&config, &refresh, GRAPH_SCOPE)
            .await?;
        if !reply.status().is_success() {
            return Err(CoreError::NeedsSignIn(account.email.clone()));
        }
        let value: Value = reply.json().await.unwrap_or(Value::Null);
        if let Some(next) = value["refresh_token"].as_str().filter(|n| *n != refresh) {
            self.secrets.set(&account_secret(account.id), next)?;
        }
        value["access_token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| CoreError::NeedsSignIn(account.email.clone()))
    }

    async fn sync_microsoft(&self, id: i64, account: &Account) -> Result<()> {
        let token = self.graph_calendar_token(account).await?;
        let (from, to) = (
            unix_now() - SYNC_BACK_DAYS * 86_400,
            unix_now() + SYNC_AHEAD_DAYS * 86_400,
        );
        let rfc = |t: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0)
                .unwrap_or_default()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        };
        let mut url = format!(
            "{}/calendarView?startDateTime={}&endDateTime={}&$top=500",
            self.microsoft_calendar,
            rfc(from),
            rfc(to)
        );
        let mut kept = Vec::new();
        loop {
            let value = self
                .calendar_json(
                    self.ai_http
                        .get(&url)
                        .bearer_auth(&token)
                        .header("Prefer", "outlook.timezone=\"UTC\""),
                    account,
                )
                .await?;
            let Some(items) = value["value"].as_array() else {
                return Err(CoreError::Invalid(
                    "The calendar gave an unexpected answer.".into(),
                ));
            };
            for item in items {
                if item["isCancelled"] == true {
                    continue;
                }
                let time = |side: &str| -> Option<i64> {
                    let text = item[side]["dateTime"].as_str()?;
                    let trimmed = text.split('.').next()?;
                    let t =
                        chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S").ok()?;
                    Some(t.and_utc().timestamp())
                };
                let (Some(starts), Some(ends)) = (time("start"), time("end")) else {
                    continue;
                };
                let provider_id = item["id"].as_str().unwrap_or("").to_string();
                let uid = item["iCalUId"].as_str().unwrap_or(&provider_id).to_string();
                let mine = match item["responseStatus"]["response"].as_str().unwrap_or("") {
                    "accepted" => "ACCEPTED",
                    "tentativelyAccepted" => "TENTATIVE",
                    "declined" => "DECLINED",
                    "notResponded" => "NEEDS-ACTION",
                    _ => "",
                };
                let e = IcsEvent {
                    uid: uid.clone(),
                    title: item["subject"].as_str().unwrap_or("(no title)").to_string(),
                    location: item["location"]["displayName"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    notes: item["bodyPreview"].as_str().unwrap_or("").to_string(),
                    starts,
                    ends,
                    all_day: item["isAllDay"] == true,
                    organizer: item["organizer"]["emailAddress"]["address"]
                        .as_str()
                        .map(str::to_string),
                    ..Default::default()
                };
                kept.push(uid.clone());
                self.store.save_event(
                    id,
                    EventExtras {
                        href: Some(&provider_id),
                        ..Default::default()
                    },
                    &e,
                    mine,
                )?;
            }
            match value["@odata.nextLink"].as_str() {
                Some(next) => url = next.to_string(),
                None => break,
            }
        }
        self.store.remove_events_not_in(id, &kept)
    }

    async fn calendar_json(
        &self,
        request: reqwest::RequestBuilder,
        account: &Account,
    ) -> Result<Value> {
        let reply = request
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let status = reply.status().as_u16();
        if status == 401 || status == 403 {
            return Err(CoreError::NeedsSignIn(account.email.clone()));
        }
        if !reply.status().is_success() {
            return Err(CoreError::Invalid(format!(
                "The calendar answered with error {status}."
            )));
        }
        let text = reply
            .text()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        // Removing something answers with nothing at all, which is fine.
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|_| CoreError::Invalid("The calendar gave an unexpected answer.".into()))
    }

    pub(crate) fn calendar_list(&self) -> Result<Value> {
        self.store.own_calendar()?;
        let calendars = self.store.calendars()?;
        let connected: Vec<i64> = calendars.iter().filter_map(|c| c.account_id).collect();
        let can_connect: Vec<Value> = self
            .store
            .list_accounts()?
            .iter()
            .filter(|a| !connected.contains(&a.id) && !a.prefs.team_shared)
            .map(|a| json!({ "account_id": a.id, "email": a.email }))
            .collect();
        Ok(json!({ "calendars": calendars, "can_connect": can_connect }))
    }

    pub(crate) fn calendar_events(&self, from: i64, to: i64) -> Result<Value> {
        if to <= from || to - from > 800 * 86_400 {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (saved, hue) in self.store.events_touching(from, to)? {
            // The same event kept here and on a connected calendar shows once.
            if !seen.insert(saved.event.uid.clone()) {
                continue;
            }
            let length = saved.event.ends - saved.event.starts;
            for start in ics::occurrences(&saved.event, from, to) {
                out.push(Occurrence {
                    event_id: saved.id,
                    calendar_id: saved.calendar_id,
                    title: saved.event.title.clone(),
                    location: saved.event.location.clone(),
                    notes: saved.event.notes.clone(),
                    starts: start,
                    ends: start + length,
                    all_day: saved.event.all_day,
                    repeats: saved.event.rrule.is_some(),
                    my_status: saved.my_status.clone(),
                    organizer: saved.event.organizer.clone(),
                    attendees: saved.event.attendees.clone(),
                    hue,
                    is_override: saved.is_override,
                });
            }
        }
        out.sort_by_key(|o| o.starts);
        Ok(json!(out))
    }

    pub(crate) async fn calendar_save(&self, input: EventInput) -> Result<Value> {
        let title = input.title.trim();
        if title.is_empty() {
            return Err(CoreError::Invalid("Give it a name first.".into()));
        }
        if input.ends <= input.starts {
            return Err(CoreError::Invalid("It has to end after it starts.".into()));
        }
        let existing = input.id.map(|id| self.store.event(id)).transpose()?;
        if let Some(old) = &existing {
            if old.is_override {
                return Err(CoreError::Invalid(
                    "This is one changed time of a repeating event. Change it in the calendar it came from.".into(),
                ));
            }
            if old.calendar_id != input.calendar_id {
                return Err(CoreError::Invalid("Bad request.".into()));
            }
        }
        let mut event = existing
            .as_ref()
            .map(|s| s.event.clone())
            .unwrap_or_else(|| IcsEvent {
                uid: format!("{}@inbox", crate::crypto::random_hex(16)),
                ..Default::default()
            });
        // Leaving out the repeat keeps it, and changing one time moves them all by as much.
        let moved_by = match (&existing, input.occurrence) {
            (Some(old), Some(at)) if old.event.rrule.is_some() => input.starts - at,
            _ => 0,
        };
        let series = existing.as_ref().filter(|old| old.event.rrule.is_some());
        let (starts, ends) = match series {
            Some(old) => (
                old.event.starts + moved_by,
                old.event.starts + moved_by + (input.ends - input.starts),
            ),
            None => (input.starts, input.ends),
        };
        if input.repeat.is_some() {
            event.rrule = rule_for(input.repeat.as_deref())?;
        }
        event.title = title.to_string();
        event.location = input.location.trim().to_string();
        event.notes = input.notes.clone();
        event.starts = starts;
        event.ends = ends;
        event.all_day = input.all_day;
        if event.rrule.is_some() && event.tz.is_none() && !event.all_day {
            event.tz = iana_time_zone::get_timezone().ok();
        }
        event.sequence += i64::from(existing.is_some());
        let status = existing
            .as_ref()
            .map(|s| s.my_status.clone())
            .unwrap_or_default();
        let (account_id, kind, url) = self.store.calendar_url(input.calendar_id)?;
        let mut href = existing.as_ref().and_then(|s| s.href.clone());
        let mut etag = None;
        let mut new_raw: Option<String> = None;
        if let Some(account_id) = account_id {
            let account = self.store.account(account_id)?;
            match kind.as_str() {
                "caldav" => {
                    let target = href.clone().unwrap_or_else(|| {
                        format!(
                            "{}/{}.ics",
                            url.trim_end_matches('/'),
                            event.uid.replace(['/', '@'], "-")
                        )
                    });
                    // Anything this app doesn't understand in the file stays as it was.
                    let file = match existing.as_ref().and_then(|s| s.raw.as_deref()) {
                        Some(raw) => ics::patch_event(raw, &event),
                        None => ics::write_event(&event),
                    };
                    let known = existing.as_ref().and_then(|s| s.etag.as_deref());
                    let (code, _, tag) = self
                        .dav_if(&account, "PUT", &target, None, file.clone(), known)
                        .await?;
                    if code == 412 {
                        let _ = self.calendar_sync(true).await;
                        return Err(CoreError::Invalid(
                            "This was changed somewhere else just now, so here's the latest. Try your change again.".into(),
                        ));
                    }
                    if code >= 300 {
                        return Err(CoreError::Invalid(format!(
                            "The calendar refused that (error {code})."
                        )));
                    }
                    href = Some(target);
                    etag = tag;
                    new_raw = Some(file);
                }
                "google" => {
                    let token = self.password(account_id).await?;
                    let when = |t: i64| {
                        if event.all_day {
                            json!({ "date": chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0).unwrap_or_default().format("%Y-%m-%d").to_string() })
                        } else {
                            // Google needs to know the zone for a repeat to keep its hour.
                            json!({
                                "dateTime": chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "timeZone": event.tz.clone().unwrap_or_else(|| "UTC".into()),
                            })
                        }
                    };
                    let mut body = json!({
                        "summary": event.title, "location": event.location, "description": event.notes,
                        "start": when(event.starts), "end": when(event.ends),
                    });
                    if let Some(rule) = &event.rrule {
                        body["recurrence"] = json!([format!("RRULE:{rule}")]);
                    }
                    let request = match &href {
                        Some(id) => self.ai_http.patch(format!("{}/{id}", self.google_calendar)),
                        None => self.ai_http.post(&self.google_calendar),
                    };
                    let reply = self
                        .calendar_json(request.bearer_auth(token).json(&body), &account)
                        .await?;
                    if let Some(id) = reply["id"].as_str() {
                        event.uid = id.to_string();
                        href = Some(id.to_string());
                    }
                }
                "microsoft" => {
                    let token = self.graph_calendar_token(&account).await?;
                    let when = |t: i64| {
                        json!({
                            "dateTime": chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S").to_string(),
                            "timeZone": "UTC",
                        })
                    };
                    let mut body = json!({
                        "subject": event.title,
                        "location": { "displayName": event.location },
                        "body": { "contentType": "text", "content": event.notes },
                        "start": when(event.starts), "end": when(event.ends),
                        "isAllDay": event.all_day,
                    });
                    if let Some(pattern) = graph_repeat(&event) {
                        body["recurrence"] = pattern;
                    }
                    let request = match &href {
                        Some(id) => self
                            .ai_http
                            .patch(format!("{}/events/{id}", self.microsoft_calendar)),
                        None => self
                            .ai_http
                            .post(format!("{}/events", self.microsoft_calendar)),
                    };
                    let reply = self
                        .calendar_json(request.bearer_auth(token).json(&body), &account)
                        .await?;
                    if let Some(id) = reply["id"].as_str() {
                        event.uid = id.to_string();
                        href = Some(id.to_string());
                    }
                }
                _ => {}
            }
        }
        // A new name from the provider replaces the row made under the old one.
        if let Some(old) = existing.as_ref().filter(|s| s.event.uid != event.uid) {
            self.store.delete_event_row(old.id)?;
        }
        let id = self.store.save_event(
            input.calendar_id,
            EventExtras {
                href: href.as_deref(),
                etag: etag.as_deref(),
                raw: new_raw.as_deref(),
                is_override: false,
            },
            &event,
            &status,
        )?;
        Ok(json!({ "id": id }))
    }

    pub(crate) async fn calendar_delete(&self, id: i64) -> Result<Value> {
        let saved = self.store.event(id)?;
        // One changed time lives in the same file as all the others on the server.
        if saved.is_override && saved.href.is_some() {
            return Err(CoreError::Invalid(
                "This is one changed time of a repeating event. Delete it in the calendar it came from.".into(),
            ));
        }
        let (account_id, kind, _) = self.store.calendar_url(saved.calendar_id)?;
        if let (Some(account_id), Some(href)) = (account_id, &saved.href) {
            let account = self.store.account(account_id)?;
            match kind.as_str() {
                "caldav" => {
                    let (code, _, _) = self
                        .dav(&account, "DELETE", href, None, String::new())
                        .await?;
                    if code >= 300 && code != 404 {
                        return Err(CoreError::Invalid(format!(
                            "The calendar refused that (error {code})."
                        )));
                    }
                }
                "google" => {
                    let token = self.password(account_id).await?;
                    self.calendar_json(
                        self.ai_http
                            .delete(format!("{}/{href}", self.google_calendar))
                            .bearer_auth(token),
                        &account,
                    )
                    .await?;
                }
                "microsoft" => {
                    let token = self.graph_calendar_token(&account).await?;
                    self.calendar_json(
                        self.ai_http
                            .delete(format!("{}/events/{href}", self.microsoft_calendar))
                            .bearer_auth(token),
                        &account,
                    )
                    .await?;
                }
                _ => {}
            }
        }
        self.store.delete_event_row(id)?;
        self.store
            .delete_changed_times(saved.calendar_id, &saved.event.uid)?;
        Ok(json!({ "deleted": true }))
    }

    /// An event in the next few minutes gets one notification.
    pub(crate) fn calendar_reminders(&self, quiet_now: bool) -> Result<()> {
        let now = unix_now();
        let Some(sender) = &self.alert_sender else {
            return Ok(());
        };
        for (saved, _) in self
            .store
            .events_touching(now - 86_400, now + REMIND_BEFORE_SECONDS + 1)?
        {
            if saved.event.all_day || saved.my_status == "DECLINED" {
                continue;
            }
            for start in ics::occurrences(&saved.event, now, now + REMIND_BEFORE_SECONDS + 1) {
                if start > now
                    && start <= now + REMIND_BEFORE_SECONDS
                    && self.store.first_reminder(saved.id, start)?
                {
                    if quiet_now || self.setting("alerts")? == "off" {
                        continue;
                    }
                    sender(&crate::alerts::Alert {
                        subtitle: String::new(),
                        title: saved.event.title.clone(),
                        body: format!("Starts in {} minutes", ((start - now) / 60).max(1)),
                        sound: self.setting("alert_sound")? == "on",
                        thread: None,
                    });
                }
            }
        }
        Ok(())
    }

    /// The calendar invitation inside an email, if there is one.
    pub(crate) async fn invite_for_message(&self, message_id: i64) -> Result<Value> {
        let Some((file, detail)) = self.read_invite(message_id).await? else {
            return Ok(Value::Null);
        };
        let Some(e) = file.events.first() else {
            return Ok(Value::Null);
        };
        let saved = self.store.event_with_uid(&e.uid)?;
        let organizer = e.organizer.clone().unwrap_or_default();
        let in_junk = self
            .store
            .list_folders(detail.account_id)?
            .iter()
            .any(|f| f.id == detail.folder_id && f.role == "junk");
        Ok(json!({
            "method": file.method,
            "uid": e.uid,
            "title": e.title,
            "location": e.location,
            "starts": e.starts,
            "ends": e.ends,
            "all_day": e.all_day,
            "organizer": if e.organizer_name.is_empty() { e.organizer.clone() } else { Some(e.organizer_name.clone()) },
            "organizer_address": organizer,
            // A stranger can name anyone as the organiser, so this is worth pointing out.
            "other_sender": !organizer.is_empty() && !same_sender(&organizer, &detail.from.address),
            "cancelled": file.method == "CANCEL" || e.cancelled,
            "my_status": saved.map(|s| s.my_status),
            // Answering sends an email, which isn't offered for mail that looks suspicious.
            "can_answer": detail.risk == 0 && !in_junk,
        }))
    }

    async fn read_invite(&self, message_id: i64) -> Result<Option<(ics::IcsFile, MessageDetail)>> {
        let detail = self.store.message_detail(message_id)?;
        let body = self.fetch_body(message_id).await?;
        let wanted: Vec<u32> = body
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                a.mime.eq_ignore_ascii_case("text/calendar")
                    || a.mime.eq_ignore_ascii_case("application/ics")
                    || a.name.to_ascii_lowercase().ends_with(".ics")
            })
            .map(|(i, a)| a.part.unwrap_or(i as u32))
            .collect();
        // Invitations often come as an inline part rather than an attachment.
        let raw_mentions = body.attachments.is_empty();
        if wanted.is_empty() && !raw_mentions {
            return Ok(None);
        }
        let raw = self.readable_message(message_id).await?;
        let text = match wanted.first() {
            Some(part) => parse::attachment_part(&raw, *part)
                .map(|(_, _, b)| String::from_utf8_lossy(&b).to_string()),
            None => parse::calendar_part(&raw),
        };
        let Some(text) = text else { return Ok(None) };
        let file = ics::parse(&text, local_offset());
        if file.events.is_empty() {
            return Ok(None);
        }
        Ok(Some((file, detail)))
    }

    /// An invitation can only change an event on the calendar if it's from whoever set
    /// that event up, and isn't older than what's there.
    fn may_change_event(&self, saved: &SavedEvent, new: &IcsEvent, sender: &str) -> Result<()> {
        let Some(owner) = saved.event.organizer.as_deref() else {
            return Ok(());
        };
        let same_owner = new
            .organizer
            .as_deref()
            .is_some_and(|o| o.eq_ignore_ascii_case(owner));
        if !same_owner || !same_sender(owner, sender) {
            return Err(CoreError::Invalid(
                "This doesn't come from whoever set up that event, so it can't change it.".into(),
            ));
        }
        if new.sequence < saved.event.sequence {
            return Err(CoreError::Invalid(
                "A newer version of this invitation is already on your calendar.".into(),
            ));
        }
        Ok(())
    }

    /// Answers an invitation. It goes on the calendar, and the organiser gets a reply.
    pub(crate) async fn invite_respond(&self, message_id: i64, status: &str) -> Result<Value> {
        let code = match status {
            "accept" => "ACCEPTED",
            "maybe" => "TENTATIVE",
            "decline" => "DECLINED",
            _ => return Err(CoreError::Invalid("Bad request.".into())),
        };
        let (file, detail) = self
            .read_invite(message_id)
            .await?
            .ok_or_else(|| CoreError::NotFound("That invitation is no longer there.".into()))?;
        let in_junk = self
            .store
            .list_folders(detail.account_id)?
            .iter()
            .any(|f| f.id == detail.folder_id && f.role == "junk");
        if detail.risk > 0 || in_junk {
            return Err(CoreError::Invalid(
                "This email looks suspicious, so it can't be answered from here.".into(),
            ));
        }
        let mut event = file.events[0].clone();
        let account = self.store.account(detail.account_id)?;
        let master_uid = event.uid.clone();
        if let Some(when) = event.recurrence_id {
            event.uid = format!("{master_uid}#{when}");
        }
        let previous = self.store.event_with_uid(&event.uid)?;
        if let Some(old) = &previous {
            self.may_change_event(old, &event, &detail.from.address)?;
        }
        // Whichever of this inbox's addresses was invited.
        let mine: Vec<String> = std::iter::once(account.email.clone())
            .chain(account.prefs.aliases.iter().map(|a| a.address.clone()))
            .collect();
        let me = event
            .attendees
            .iter()
            .find(|a| mine.iter().any(|m| m.eq_ignore_ascii_case(&a.address)))
            .map_or_else(|| account.email.clone(), |a| a.address.clone());
        for a in &mut event.attendees {
            if a.address.eq_ignore_ascii_case(&me) {
                a.status = code.to_string();
            }
        }
        let connected = self
            .store
            .calendars()?
            .into_iter()
            .find(|c| c.account_id == Some(account.id));
        let mut warnings = Vec::new();
        let declined_new = code == "DECLINED" && previous.is_none();
        if !declined_new {
            let calendar = match &previous {
                Some(old) => old.calendar_id,
                None => self.store.own_calendar()?,
            };
            let mut extras = EventExtras {
                is_override: event.recurrence_id.is_some(),
                ..Default::default()
            };
            let mut place = calendar;
            let mut stored_file = None;
            // A calendar with its own server gets it there too. Otherwise the next refresh
            // would find it missing and take it away. Google and Microsoft add invitations
            // by themselves, so a copy kept here does until theirs arrives.
            if let Some(c) = connected
                .as_ref()
                .filter(|c| c.kind == "caldav" && event.recurrence_id.is_none())
            {
                let (_, _, url) = self.store.calendar_url(c.id)?;
                let target = previous
                    .as_ref()
                    .and_then(|p| p.href.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "{}/{}.ics",
                            url.trim_end_matches('/'),
                            event.uid.replace(['/', '@'], "-")
                        )
                    });
                let body = ics::write_event(&event);
                match self.dav(&account, "PUT", &target, None, body.clone()).await {
                    Ok((put, _, _)) if put < 300 => {
                        place = c.id;
                        stored_file = Some((target, body));
                    }
                    Ok(_) | Err(_) => warnings.push(
                        "It's on your calendar here, but couldn't be added to your connected calendar.".to_string(),
                    ),
                }
            }
            if let Some((href, _)) = &stored_file {
                extras.href = Some(href);
            }
            if let Some((_, body)) = &stored_file {
                extras.raw = Some(body);
            }
            let saved = self.store.save_event(place, extras, &event, code)?;
            self.store.set_event_inbox(saved, account.id)?;
            if let (Some(when), Some(master)) =
                (event.recurrence_id, self.store.event_with_uid(&master_uid)?)
            {
                self.store.add_exdate(master.id, when)?;
            }
        }
        if let Some(organizer) = event
            .organizer
            .as_deref()
            .filter(|o| !mine.iter().any(|m| m.eq_ignore_ascii_case(o)))
        {
            if let Err(e) = self
                .send_invite_reply(&account, &me, organizer, &event, code)
                .await
            {
                warnings.push(format!(
                    "It's on your calendar, but your answer couldn't be sent: {e}"
                ));
            }
        }
        Ok(json!({ "status": code, "warnings": warnings }))
    }

    /// When an invitation is called off, it comes off the calendar: one date, or all of it.
    pub(crate) async fn invite_cancelled(&self, message_id: i64) -> Result<Value> {
        let (file, detail) = self
            .read_invite(message_id)
            .await?
            .ok_or_else(|| CoreError::NotFound("That invitation is no longer there.".into()))?;
        let event = &file.events[0];
        if let Some(saved) = self.store.event_with_uid(&event.uid)? {
            self.may_change_event(&saved, event, &detail.from.address)?;
            match event.recurrence_id {
                Some(when) => self.store.add_exdate(saved.id, when)?,
                None => self.store.delete_event_row(saved.id)?,
            }
        }
        Ok(json!({ "removed": true }))
    }
}

/// Whether an email from this sender can speak for this organiser. Calendars often send
/// invitations from their own address, in the organiser's name, so it's the company that
/// has to match.
/// Anyone can have an address at a shared provider, so there only the address itself counts,
/// and only the calendar services' own domains, where nobody gets a personal address.
fn same_sender(organizer: &str, sender: &str) -> bool {
    let domain = |a: &str| a.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
    let company = owned_domain(&domain(organizer));
    let shared = crate::avatars::SHARED_PROVIDERS.contains(&company.as_str());
    organizer.eq_ignore_ascii_case(sender)
        || (!shared && company == owned_domain(&domain(sender)))
        || matches!(
            owned_domain(&domain(sender)).as_str(),
            "google.com" | "office365.com" | "apple.com"
        )
}

/// A repeat in the form Microsoft's calendar wants.
fn graph_repeat(e: &IcsEvent) -> Option<Value> {
    let rule = e.rrule.as_deref()?;
    let start = chrono::DateTime::<chrono::Utc>::from_timestamp(e.starts, 0)?;
    use chrono::Datelike;
    let pattern = match rule {
        "FREQ=DAILY" => json!({ "type": "daily", "interval": 1 }),
        "FREQ=WEEKLY" => json!({
            "type": "weekly", "interval": 1,
            "daysOfWeek": [start.weekday().to_string().to_lowercase()],
        }),
        "FREQ=MONTHLY" => {
            json!({ "type": "absoluteMonthly", "interval": 1, "dayOfMonth": start.day() })
        }
        "FREQ=YEARLY" => json!({
            "type": "absoluteYearly", "interval": 1, "dayOfMonth": start.day(), "month": start.month(),
        }),
        _ => return None,
    };
    Some(json!({
        "pattern": pattern,
        "range": { "type": "noEnd", "startDate": start.format("%Y-%m-%d").to_string() },
    }))
}

fn my_status_in(e: &IcsEvent, account: &Account) -> String {
    e.attendees
        .iter()
        .find(|a| a.address.eq_ignore_ascii_case(&account.email))
        .map(|a| a.status.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{host_is, owned_domain, same_sender};

    #[test]
    fn only_the_organiser_or_their_company_can_change_an_invite() {
        assert!(same_sender("ann@acme.com", "bob@mail.acme.com"));
        assert!(same_sender("ann@gmail.com", "Ann@gmail.com"));
        assert!(same_sender(
            "ann@gmail.com",
            "calendar-notification@google.com"
        ));
        // Anyone can have an address at these.
        assert!(!same_sender("ann@gmail.com", "mallory@gmail.com"));
        assert!(!same_sender("ann@acme.com", "mallory@outlook.com"));
        assert!(!same_sender("ann@acme.com", "mallory@icloud.com"));
    }

    #[test]
    fn a_host_only_counts_if_it_really_is_that_company() {
        assert!(host_is("imap.mail.me.com", "me.com"));
        assert!(host_is("me.com", "me.com"));
        assert!(!host_is("imap.acme.com", "me.com"));
        assert!(!host_is("me.com.evil.test", "me.com"));
        assert_eq!(owned_domain("p12-caldav.icloud.com"), "icloud.com");
        assert_eq!(owned_domain("mail.acme.co.uk"), "acme.co.uk");
        assert_ne!(
            owned_domain("mail.hostcompany.net"),
            owned_domain("acme.com")
        );
    }

    use super::*;

    #[test]
    fn tags_are_found_whatever_prefix_the_server_uses() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/a/</d:href><cal:calendar-data>x &amp; y</cal:calendar-data></d:response><D:response><D:href>/b/</D:href></D:response></d:multistatus>"#;
        let list = responses(xml);
        assert_eq!(list.len(), 2);
        assert_eq!(texts_in(&list[0], "href"), vec!["/a/"]);
        assert_eq!(xml_text(&texts_in(&list[0], "calendar-data")[0]), "x & y");
        assert_eq!(xml_text("<![CDATA[a<b]]>"), "a<b");
    }

    #[test]
    fn repeat_choices_are_checked() {
        assert_eq!(
            rule_for(Some("weekly")).unwrap().as_deref(),
            Some("FREQ=WEEKLY")
        );
        assert!(rule_for(None).unwrap().is_none());
        assert!(rule_for(Some("hourly")).is_err());
    }
}
