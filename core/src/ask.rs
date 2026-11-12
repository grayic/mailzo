//! Ask AI's finding: the emails a question is about, picked by code. Exact matches only;
//! emails of a similar meaning only when almost nothing matches, and kept apart as related.
//! The AI then reads short passages of these, never whole mailboxes.

use crate::engine::Engine;
use crate::error::Result;
use crate::people::Resolved;
use crate::question::{Kind, Question};
use crate::search::{fold, SearchFilters};
use std::collections::HashMap;

/// How many emails a question can be answered from, at most.
const MOST_PASSAGES: usize = 20;
/// A list shows emails, not what's in them, so it can show many more.
const MOST_LISTED: usize = 200;
/// How many matches are looked at before the best are picked.
const CANDIDATES: i64 = 60;
/// As many search results as the list shows, for asking about them.
const RESULTS_SHOWN: i64 = 300;
/// Fewer exact matches than this, and similar-meaning emails are looked for too.
const FEW_EXACT: usize = 3;
/// About 300 tokens: enough around the words to answer from.
pub(crate) const PASSAGE_CHARS: usize = 1_200;
/// A question with nothing to find mail by is about the last month.
const RECENT_SECONDS: i64 = 30 * 86_400;

/// A short piece of one email, for the AI to answer from.
#[derive(Debug, Clone)]
pub(crate) struct Passage {
    pub id: i64,
    pub account_id: i64,
    pub thread_id: String,
    pub folder_id: i64,
    pub from: String,
    pub date: i64,
    pub subject: String,
    // ponytail: read by the answer check once Ask AI answers from passages (Task 11).
    #[allow(dead_code)]
    pub text: String,
    /// Found by meaning, not by the question's words.
    pub related: bool,
}

/// The part of `text` around the first of `words` it has, at most `most_chars` long, cut at
/// spaces. With none of the words, its start.
pub(crate) fn passage_around(text: &str, words: &[String], most_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= most_chars {
        return text.to_string();
    }
    let folded = fold(text);
    let at = words
        .iter()
        .filter_map(|w| folded.find(&fold(w)))
        .min()
        .map_or(0, |byte| folded[..byte].chars().count().min(chars.len()));
    let room = most_chars.saturating_sub(2);
    let mut start = at.saturating_sub(room / 3);
    let mut end = (start + room).min(chars.len());
    start = end.saturating_sub(room);
    // Whole words at both ends.
    while start > 0 && start < at && !chars[start - 1].is_whitespace() {
        start += 1;
    }
    while end < chars.len() && end > at && !chars[end].is_whitespace() {
        end -= 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Words that ask about a kind of fact an email's markup can state.
const FACT_WORDS: &[(&str, &str)] = &[
    ("flight", "flight"),
    ("flights", "flight"),
    ("fly", "flight"),
    ("plane", "flight"),
    ("hotel", "lodging"),
    ("stay", "lodging"),
    ("check-in", "lodging"),
    ("order", "order"),
    ("orders", "order"),
    ("package", "parcel"),
    ("parcel", "parcel"),
    ("delivery", "parcel"),
    ("shipment", "parcel"),
    ("tracking", "parcel"),
    ("invoice", "invoice"),
    ("bill", "invoice"),
];

/// Words that make a catch-up a brief before a call.
const MEETING_WORDS: &[&str] = &["call", "meeting", "meet", "appointment"];
/// Capitalised words in an event's title that aren't anyone's name.
const NOT_NAMES_IN_TITLES: &[&str] = &[
    "call", "meeting", "with", "and", "the", "re", "sync", "catch-up", "intro", "weekly", "daily",
];
/// How many conversations a catch-up reads.
const CATCH_UP_CONVERSATIONS: usize = 6;
/// How far ahead a plan with a topic looks on the calendar.
const PLAN_AHEAD_SECONDS: i64 = 30 * 86_400;

/// An event as something the AI can read and cite. Its id is the event's, made negative so it
/// never meets an email's.
pub(crate) fn event_passage(o: &serde_json::Value) -> Passage {
    let starts = o["starts"].as_i64().unwrap_or(0);
    let when = chrono::DateTime::from_timestamp(starts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a %-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let with: Vec<String> = o["attendees"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let name = a["name"].as_str().unwrap_or("").trim();
            if name.is_empty() {
                a["address"].as_str().unwrap_or("").to_string()
            } else {
                name.to_string()
            }
        })
        .collect();
    let notes: String = o["notes"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(300)
        .collect();
    Passage {
        id: -o["event_id"].as_i64().unwrap_or(0),
        account_id: 0,
        thread_id: String::new(),
        folder_id: 0,
        from: o["organizer"].as_str().unwrap_or("").to_string(),
        date: starts,
        subject: o["title"].as_str().unwrap_or("").to_string(),
        text: format!(
            "When: {when}\nWhere: {}\nWith: {}\n{notes}",
            o["location"].as_str().unwrap_or(""),
            with.join(", ")
        ),
        related: false,
    }
}

/// Where an email comes among the found ones; smaller first.
type Rank = (
    bool,
    std::cmp::Reverse<usize>,
    bool,
    bool,
    bool,
    bool,
    bool,
    i64,
);

fn message_ids(found: &[crate::model::ThreadSummary]) -> Vec<i64> {
    found
        .iter()
        .filter_map(|t| {
            t.thread_id
                .strip_prefix("msg:")
                .and_then(|i| i.parse().ok())
        })
        .collect()
}

impl Engine {
    /// Events AI may not read: on the calendar of an inbox AI may not read, or an
    /// invitation that came to one. An event made here and kept here has no inbox.
    pub(crate) fn events_ai_may_not_read(&self) -> Result<std::collections::HashSet<i64>> {
        let closed: Vec<i64> = self
            .store
            .list_accounts()?
            .into_iter()
            .filter(|a| !self.ai_may_read_inbox(a))
            .map(|a| a.id)
            .collect();
        self.store.events_of_inboxes(&closed)
    }

    /// The calendar between two times, without events AI may not read.
    fn events_ai_may_read(&self, from: i64, to: i64) -> Result<Vec<serde_json::Value>> {
        let hidden = self.events_ai_may_not_read()?;
        Ok(self
            .calendar_events(from, to)?
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| !hidden.contains(&o["event_id"].as_i64().unwrap_or(0)))
            .cloned()
            .collect())
    }

    /// The emails a question is about, best first, as short passages. A list question's
    /// people and dates are hard filters. Any other question filters by a person only when
    /// the name meant one person, and by dates only when real dates were written; otherwise
    /// they only rank. A company only ranks.
    pub(crate) fn ask_find(
        &self,
        q: &Question,
        people: &[Resolved],
        scope: Option<i64>,
        around: Option<&Around>,
    ) -> Result<(Vec<Passage>, usize)> {
        let readable = self.folders_where(|a, role, path| self.ai_may_read(a, role, path))?;
        let keep = |t: &crate::model::ThreadSummary| {
            t.risk == 0
                && readable.contains(&t.folder_id)
                && scope.is_none_or(|a| t.account_id == a)
        };
        if q.waiting_on_me {
            return self
                .waiting_on_me(&keep, MOST_LISTED, &q.groups)
                .map(counted);
        }
        if q.kind == Kind::Plan && q.words.is_empty() {
            return self.plan_for_today(&keep).map(counted);
        }
        // Any of the ways the question says it: the AI gives the English word among them.
        let about_a_call = q
            .groups
            .iter()
            .flatten()
            .chain(&q.words)
            .any(|w| MEETING_WORDS.contains(&w.as_str()));
        if q.kind == Kind::CatchUp && about_a_call {
            if let Some(brief) = self.brief_before_a_call(q, &keep)? {
                return Ok(counted(brief));
            }
        }
        let list = q.kind == Kind::List;
        // Each thing asked about, with the other words emails use for it: an email has a
        // group when it has any one of its words.
        let groups: Vec<Vec<String>> = if q.groups.is_empty() {
            q.words.iter().map(|w| vec![w.clone()]).collect()
        } else {
            q.groups.clone()
        };
        // The inbox asked about goes into the search itself, so the database narrows it.
        let mut f = SearchFilters {
            account: scope,
            ..Default::default()
        };
        for p in people {
            let one_person = !p.addresses.is_empty() && p.other_match.is_none();
            if list || one_person {
                f.from.extend(p.addresses.iter().cloned());
            }
            if list {
                f.from.extend(p.domains.iter().cloned());
            }
        }
        // "What happened today" is about today's mail, not the newest whenever it came.
        if list || q.exact_dates || q.kind == Kind::CatchUp {
            f.after = q.after;
            f.before = q.before;
        }
        if let Some(a) = around {
            f.with = a.with.clone();
        }
        let within = around.and_then(|a| a.within.as_ref());
        if groups.is_empty() && f.is_empty() && within.is_none() {
            f.after = Some(crate::identity::unix_now() - RECENT_SECONDS);
        }
        let search = |one: &SearchFilters| -> Result<Vec<i64>> {
            Ok(message_ids(
                &self
                    .store
                    .search_threads_where(one, &[], CANDIDATES, false, keep)?
                    .0,
            ))
        };
        // The emails with something from every group.
        let every = |base: &SearchFilters| -> Result<Vec<i64>> {
            if groups.is_empty() {
                return if base.is_empty() {
                    Ok(within.cloned().unwrap_or_default())
                } else {
                    search(base)
                };
            }
            let mut found: Option<Vec<i64>> = None;
            for g in &groups {
                let ids = search(&SearchFilters {
                    similar: g.clone(),
                    ..base.clone()
                })?;
                found = Some(match found {
                    None => ids,
                    Some(before) => before.into_iter().filter(|id| ids.contains(id)).collect(),
                });
                if found.as_ref().is_some_and(Vec::is_empty) {
                    break;
                }
            }
            Ok(found.unwrap_or_default())
        };
        let matching = every(&f)?;
        // How many of the things asked about each email has, to put those with most first.
        let mut hits: HashMap<i64, usize> = HashMap::new();
        // Asked about search results, every result is one of the emails; the question's own
        // words and people only put those that have them first, unless it asks for a list.
        let (mut ids, full): (Vec<i64>, Vec<i64>) = match within {
            Some(within) => {
                let has: Vec<i64> = matching
                    .into_iter()
                    .filter(|id| within.contains(id))
                    .collect();
                if list {
                    (has.clone(), has)
                } else {
                    // Every result, those with most of the question's words first.
                    for g in &groups {
                        for id in search(&SearchFilters {
                            similar: g.clone(),
                            ..f.clone()
                        })? {
                            *hits.entry(id).or_default() += 1;
                        }
                    }
                    let mut ranked = within.clone();
                    ranked.sort_by_key(|id| std::cmp::Reverse(hits.get(id).copied().unwrap_or(0)));
                    (ranked, has)
                }
            }
            // Nothing in the dates asked about: the closest outside them, so the answer can say
            // there was none then and when the last one was.
            None if matching.is_empty() && q.exact_dates && !list && !groups.is_empty() => {
                let found = every(&SearchFilters {
                    after: None,
                    before: None,
                    ..f.clone()
                })?;
                (found.clone(), found)
            }
            // Nothing from the person named ("what did I promise Huzaifa?" when nothing was
            // ever sent him): their words anywhere, the person only ranking them.
            None if matching.is_empty() && !list && !f.from.is_empty() && !groups.is_empty() => {
                let found = every(&SearchFilters {
                    from: Vec::new(),
                    ..f.clone()
                })?;
                (found.clone(), found)
            }
            None => (matching.clone(), matching),
        };
        // Few emails have every group: those with most of them come next. A question no AI
        // could read is searched as typed, so its list takes them too.
        if within.is_none() && (!list || !q.sure) && ids.len() < FEW_EXACT && groups.len() >= 2 {
            let need = groups.len().div_ceil(2).max(2);
            for g in &groups {
                for id in search(&SearchFilters {
                    similar: g.clone(),
                    ..f.clone()
                })? {
                    *hits.entry(id).or_default() += 1;
                }
            }
            let mut most: Vec<(i64, usize)> = hits
                .iter()
                .map(|(id, n)| (*id, *n))
                .filter(|(id, n)| *n >= need && !ids.contains(id))
                .collect();
            most.sort_by_key(|(id, n)| (std::cmp::Reverse(*n), std::cmp::Reverse(*id)));
            ids.extend(most.into_iter().map(|(id, _)| id));
        }
        let named: Vec<String> = people
            .iter()
            .flat_map(|p| p.addresses.iter().chain(&p.domains))
            .map(|a| a.to_lowercase())
            .collect();
        let due: Vec<i64> = if q.upcoming {
            self.store.upcoming_due_messages(CANDIDATES)?
        } else {
            Vec::new()
        };
        // Ranked first from what's cheap to know, so only the emails kept are read.
        let mut ranked: Vec<(crate::model::MessageDetail, bool, Rank)> = Vec::new();
        for id in ids {
            let Some((m, _)) = self.source_for_ai(id)? else {
                continue;
            };
            // Search results can come from every inbox; a question asked of one keeps to it.
            if scope.is_some_and(|a| m.account_id != a) {
                continue;
            }
            let from = m.from.address.to_lowercase();
            let by_named = named.iter().any(|n| {
                from == *n || from.ends_with(&format!("@{n}")) || from.ends_with(&format!(".{n}"))
            });
            let subject = fold(&m.subject);
            let in_subject =
                !q.words.is_empty() && q.words.iter().all(|w| subject.contains(&fold(w)));
            let in_dates =
                q.after.is_none_or(|a| m.date >= a) && q.before.is_none_or(|b| m.date < b);
            // Best first: every word, then the words in the subject, the person named, the
            // dates asked about, something still to come, a person rather than a company, newest.
            let here =
                around.is_some_and(|a| m.account_id == a.account_id && m.thread_id == a.thread_id);
            let rank = (
                !full.contains(&id),
                std::cmp::Reverse(hits.get(&id).copied().unwrap_or(0)),
                !here,
                !in_subject,
                !by_named,
                !(in_dates && (due.is_empty() || due.contains(&id))),
                crate::priority::is_company_address(&m.from.address),
                -m.date,
            );
            // Most of the words but not all is only maybe related.
            let related = within.is_none() && !full.contains(&id);
            ranked.push((m, related, rank));
        }
        if list {
            ranked.sort_by_key(|(m, _, _)| -m.date);
            // One row a conversation, by its newest email from someone else when there is one,
            // so a back-and-forth isn't a dozen rows of it and your own replies.
            let own: Vec<String> = self
                .own_addresses()?
                .iter()
                .map(|a| a.to_lowercase())
                .collect();
            let mine =
                |m: &crate::model::MessageDetail| own.contains(&m.from.address.to_lowercase());
            let mut chosen: HashMap<(i64, String), usize> = HashMap::new();
            for (i, (m, _, _)) in ranked.iter().enumerate() {
                let key = (m.account_id, m.thread_id.clone());
                match chosen.get(&key) {
                    None => {
                        chosen.insert(key, i);
                    }
                    Some(&at) if mine(&ranked[at].0) && !mine(m) => {
                        chosen.insert(key, i);
                    }
                    _ => {}
                }
            }
            let keep: std::collections::HashSet<usize> = chosen.into_values().collect();
            ranked = ranked
                .into_iter()
                .enumerate()
                .filter(|(i, _)| keep.contains(i))
                .map(|(_, r)| r)
                .collect();
        } else {
            ranked.sort_by_key(|(_, _, rank)| *rank);
        }
        let total = ranked.iter().filter(|(_, related, _)| !related).count();
        ranked.truncate(if list { MOST_LISTED } else { MOST_PASSAGES });
        // A list shows emails, not what's in them, so they aren't read.
        let passages: Vec<Passage> = ranked
            .iter()
            .map(|(m, related, _)| {
                if list {
                    self.listed(m, *related)
                } else {
                    self.passage_of(m, &q.words, *related)
                }
            })
            .collect();
        let mut out = passages;
        // Whether almost nothing matched is about the matches, before they're regrouped.
        let matched = out.iter().filter(|p| !p.related).count();
        // A catch-up reads where each conversation stands now, not every email that matched.
        if q.kind == Kind::CatchUp && !out.is_empty() {
            out = self.newest_of_each(&out, &q.words)?;
        }
        // A plan with a topic ("prepare for my trip") counts what's coming on the calendar too.
        if q.kind == Kind::Plan {
            let now = crate::identity::unix_now();
            let mut coming: Vec<Passage> = self
                .events_ai_may_read(now, now + PLAN_AHEAD_SECONDS)?
                .iter()
                .filter(|o| {
                    let said = fold(&format!(
                        "{} {}",
                        o["title"].as_str().unwrap_or(""),
                        o["location"].as_str().unwrap_or("")
                    ));
                    q.words.iter().any(|w| said.contains(&fold(w)))
                })
                .map(event_passage)
                .collect();
            coming.append(&mut out);
            out = coming;
        }
        // Almost nothing has the words: emails about the same thing, apart, and only close ones.
        if within.is_none() && matched < FEW_EXACT && !q.words.is_empty() {
            let meaning = self.search_meaning(&q.words.join(" "), Some(&readable))?;
            let close = meaning["ids"].as_array().cloned().unwrap_or_default();
            for id in close.iter().filter_map(serde_json::Value::as_i64) {
                if out.iter().any(|p| p.id == id) {
                    continue;
                }
                if let Some((m, _)) = self.source_for_ai(id)? {
                    if scope.is_none_or(|a| m.account_id == a)
                        && around.is_none_or(|a| talks_with(&m, &a.with))
                    {
                        out.push(self.passage_of(&m, &q.words, true));
                    }
                }
            }
        }
        Ok((out, total))
    }

    /// A fact question answered from what an email's markup states, with no AI: the email
    /// and the fact, or None when no saved fact fits.
    pub(crate) fn fact_for(
        &self,
        q: &Question,
        scope: Option<i64>,
    ) -> Result<Option<(serde_json::Value, i64)>> {
        if q.kind != Kind::Fact {
            return Ok(None);
        }
        // Someone else's flight isn't one of the person's own bookings.
        if q.names
            .iter()
            .any(|n| !matches!(n.to_lowercase().as_str(), "me" | "my" | "i" | "myself"))
        {
            return Ok(None);
        }
        let Some(kind) = q.words.iter().find_map(|w| {
            FACT_WORDS
                .iter()
                .find(|(word, _)| w == word)
                .map(|(_, k)| *k)
        }) else {
            return Ok(None);
        };
        let others: Vec<String> = q
            .words
            .iter()
            .filter(|w| !FACT_WORDS.iter().any(|(word, _)| *w == word))
            .map(|w| fold(w))
            .collect();
        // Flights, stays and parcels are asked about before they happen.
        let ahead = q.upcoming || matches!(kind, "flight" | "lodging" | "parcel");
        let now = crate::identity::unix_now();
        let (mut after, mut before) = (q.after, q.before);
        // "In December", asked in November, is the coming one for something still ahead.
        if ahead && !q.latest && before.is_some_and(|b| b <= now) {
            let a_year_on = |t: i64| {
                use chrono::{Datelike, TimeZone};
                chrono::Local
                    .timestamp_opt(t, 0)
                    .single()
                    .and_then(|d| d.with_year(d.year() + 1))
                    .map_or(t + 365 * 86_400, |d| d.timestamp())
            };
            after = after.map(a_year_on);
            before = before.map(a_year_on);
        }
        let undated = after.is_none() && before.is_none();
        // The last one is the newest that has happened; otherwise what's still to come.
        if q.latest {
            before = before.or(Some(now));
        } else if ahead && undated {
            after = Some(now - 6 * 3600);
        }
        let newest_first = q.latest || (!ahead && undated);
        for (id, place, value) in self.store.facts_of(kind, after, before, newest_first)? {
            let Some((m, _)) = self.source_for_ai(id)? else {
                continue;
            };
            if scope.is_some_and(|a| m.account_id != a) {
                continue;
            }
            let said = fold(&format!("{place} {} {value}", m.subject));
            if others.iter().all(|w| said.contains(w.as_str())) {
                let mut fact = value;
                fact["kind"] = kind.into();
                fact["place"] = place.into();
                return Ok(Some((fact, id)));
            }
        }
        Ok(None)
    }

    /// Today: what's on the calendar, what's waiting on the person, and what's due by tomorrow.
    fn plan_for_today(
        &self,
        keep: &dyn Fn(&crate::model::ThreadSummary) -> bool,
    ) -> Result<Vec<Passage>> {
        use chrono::TimeZone;
        let today = chrono::Local::now().date_naive();
        let start_of = |d: chrono::NaiveDate| {
            chrono::Local
                .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
                .earliest()
                .map_or(0, |t| t.timestamp())
        };
        let (from, to) = (start_of(today), start_of(today + chrono::Duration::days(1)));
        let mut out: Vec<Passage> = self
            .events_ai_may_read(from, to)?
            .iter()
            .map(event_passage)
            .collect();
        for p in self.waiting_on_me(keep, MOST_PASSAGES, &[])? {
            out.push(p);
        }
        for id in self.store.upcoming_due_messages(MOST_PASSAGES as i64)? {
            if out.iter().any(|p| p.id == id) {
                continue;
            }
            if let Some((m, _)) = self.source_for_ai(id)? {
                if m.due_at.is_some_and(|d| d < to + 86_400) {
                    out.push(self.passage_of(&m, &[], false));
                }
            }
        }
        out.truncate(MOST_PASSAGES);
        Ok(out)
    }

    /// Before a call: the next one on the calendar in the coming week, then the last few emails
    /// from the people in it, and the last one sent to each. Who they are comes from the
    /// event's attendees, or else from the names in its title ("Call with Ana").
    fn brief_before_a_call(
        &self,
        q: &Question,
        keep: &dyn Fn(&crate::model::ThreadSummary) -> bool,
    ) -> Result<Option<Vec<Passage>>> {
        let now = crate::identity::unix_now();
        // What the call is about, leaving out the words that only say it's a call.
        let topic: Vec<String> = q
            .groups
            .iter()
            .filter(|g| !g.iter().any(|w| MEETING_WORDS.contains(&w.as_str())))
            .flatten()
            .chain(
                q.groups
                    .is_empty()
                    .then_some(&q.words)
                    .into_iter()
                    .flatten(),
            )
            .filter(|w| !MEETING_WORDS.contains(&w.as_str()))
            .map(|w| fold(w))
            .collect();
        let events = self.events_ai_may_read(now, now + 7 * 86_400)?;
        let Some(next) = events.iter().find(|o| {
            let said = fold(o["title"].as_str().unwrap_or(""));
            topic.is_empty() || topic.iter().any(|w| said.contains(w.as_str()))
        }) else {
            return Ok(None);
        };
        let own: Vec<String> = self
            .own_addresses()?
            .iter()
            .map(|a| a.to_lowercase())
            .collect();
        let mut addresses: Vec<String> = next["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["address"].as_str())
            .map(str::to_lowercase)
            .filter(|a| !own.contains(a))
            .collect();
        if addresses.is_empty() {
            let names: Vec<String> = next["title"]
                .as_str()
                .unwrap_or("")
                .split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                .filter(|w| w.chars().next().is_some_and(char::is_uppercase))
                .filter(|w| !NOT_NAMES_IN_TITLES.contains(&w.to_lowercase().as_str()))
                .collect();
            for p in self.resolve_names(&names)? {
                addresses.extend(p.addresses);
            }
        }
        let mut out = vec![event_passage(next)];
        for a in addresses.iter().take(5) {
            let theirs = SearchFilters {
                from: vec![a.clone()],
                ..Default::default()
            };
            let mine = SearchFilters {
                to: vec![a.clone()],
                folder: Some("sent".into()),
                ..Default::default()
            };
            let mut ids = message_ids(
                &self
                    .store
                    .search_threads_where(&theirs, &[], 3, false, keep)?
                    .0,
            );
            ids.extend(message_ids(
                &self
                    .store
                    .search_threads_where(&mine, &[], 1, false, keep)?
                    .0,
            ));
            for id in ids {
                if out.iter().any(|p| p.id == id) {
                    continue;
                }
                if let Some((m, _)) = self.source_for_ai(id)? {
                    out.push(self.passage_of(&m, &[], false));
                }
            }
        }
        Ok(Some(out))
    }

    /// Each conversation found, by its newest two emails; with just one conversation, the
    /// person's own last reply in it too, for "what did I commit to".
    fn newest_of_each(&self, found: &[Passage], words: &[String]) -> Result<Vec<Passage>> {
        let mut threads: Vec<(i64, String)> = Vec::new();
        for p in found.iter().filter(|p| !p.related && p.id > 0) {
            let key = (p.account_id, p.thread_id.clone());
            if !threads.contains(&key) {
                threads.push(key);
            }
        }
        threads.truncate(CATCH_UP_CONVERSATIONS);
        let own: Vec<String> = self
            .own_addresses()?
            .iter()
            .map(|a| a.to_lowercase())
            .collect();
        let single = threads.len() == 1;
        let mut out = Vec::new();
        for (account, thread) in threads {
            let Ok(mail) = self.readable_thread(account, &thread) else {
                continue;
            };
            let mut picked: Vec<i64> = mail.iter().rev().take(2).map(|m| m.id).collect();
            if single {
                let mine = mail
                    .iter()
                    .rev()
                    .find(|m| own.contains(&m.from.address.to_lowercase()));
                if let Some(m) = mine.filter(|m| !picked.contains(&m.id)) {
                    picked.push(m.id);
                }
            }
            for id in picked {
                if let Some((m, _)) = self.source_for_ai(id)? {
                    out.push(self.passage_of(&m, words, false));
                }
            }
        }
        Ok(out)
    }

    /// Conversations in Needs you, each by its newest email from someone else. Asked about
    /// something ("refund requests waiting on me"), only the ones about it.
    fn waiting_on_me(
        &self,
        keep: &dyn Fn(&crate::model::ThreadSummary) -> bool,
        most: usize,
        groups: &[Vec<String>],
    ) -> Result<Vec<Passage>> {
        let fetched = if groups.is_empty() { most } else { most * 5 };
        let own: Vec<String> = self
            .own_addresses()?
            .iter()
            .map(|a| a.to_lowercase())
            .collect();
        let threads = self
            .store
            // ponytail: a fixed look further back when filtering by topic; page if it falls short.
            .list_all_on_tab("inbox", fetched as i64, 0, true, Some("needs_you"))?;
        let mut out = Vec::new();
        for t in threads.iter().filter(|t| keep(t)) {
            let thread = self.store.real_thread(&t.thread_id)?;
            let theirs = self
                .store
                .get_thread(t.account_id, &thread, None)?
                .into_iter()
                .filter(|m| !own.contains(&m.from.address.to_lowercase()))
                .max_by_key(|m| m.date);
            if let Some(newest) = theirs {
                if let Some((m, _)) = self.source_for_ai(newest.id)? {
                    let text = crate::priority::without_quotes(&crate::assist::plain_of(&m));
                    // Something from every group asked about, any one of its words.
                    let about = groups.iter().all(|g| {
                        g.iter().any(|w| {
                            crate::search::matches_exactly(
                                std::slice::from_ref(w),
                                &m.subject,
                                "",
                                &text,
                            )
                        })
                    });
                    if about && out.len() < most {
                        let words: Vec<String> = groups.iter().flatten().cloned().collect();
                        out.push(self.passage_of(&m, &words, false));
                    }
                }
            }
        }
        Ok(out)
    }

    fn passage_of(
        &self,
        m: &crate::model::MessageDetail,
        words: &[String],
        related: bool,
    ) -> Passage {
        let text = crate::priority::without_quotes(&crate::assist::plain_of(m));
        Passage {
            text: passage_around(&text, words, PASSAGE_CHARS),
            ..self.listed(m, related)
        }
    }

    /// An email as a list shows it: who, when and what about, without reading it.
    fn listed(&self, m: &crate::model::MessageDetail, related: bool) -> Passage {
        Passage {
            id: m.id,
            account_id: m.account_id,
            thread_id: m.thread_id.clone(),
            folder_id: m.folder_id,
            from: if m.from.name.is_empty() {
                m.from.address.clone()
            } else {
                m.from.name.clone()
            },
            date: m.date,
            subject: m.subject.clone(),
            text: String::new(),
            related,
        }
    }
}

/// Asking from an email: everyone in its conversation, and the conversation itself first.
pub(crate) struct Around {
    pub with: Vec<String>,
    pub account_id: i64,
    pub thread_id: String,
    /// Asked about search results: these emails and no others.
    pub within: Option<Vec<i64>>,
}

/// Whether an email is from, to or copied to any of these people.
fn talks_with(m: &crate::model::MessageDetail, with: &[String]) -> bool {
    std::iter::once(&m.from).chain(&m.to).chain(&m.cc).any(|a| {
        let a = a.address.to_lowercase();
        with.iter()
            .any(|w| a == *w || (w.starts_with('@') && a.ends_with(w.as_str())))
    })
}

impl Engine {
    /// The people of a conversation, to look for: a company's whole domain, or the address
    /// itself at a provider everyone uses. Never the person's own.
    pub(crate) fn around(&self, account_id: i64, thread_id: &str) -> Result<Around> {
        let own: Vec<String> = self
            .own_addresses()?
            .iter()
            .map(|a| a.to_lowercase())
            .collect();
        let thread = self.store.real_thread(thread_id)?;
        let mut with: Vec<String> = Vec::new();
        for m in self.store.get_thread(account_id, &thread, None)? {
            for a in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
                let address = a.address.to_lowercase();
                if !address.contains('@') || own.contains(&address) {
                    continue;
                }
                let domain = address.rsplit('@').next().unwrap_or("");
                let who = if crate::avatars::SHARED_PROVIDERS.contains(&domain) {
                    address.clone()
                } else {
                    format!("@{domain}")
                };
                if !with.contains(&who) {
                    with.push(who);
                }
            }
        }
        Ok(Around {
            with,
            account_id,
            thread_id: thread_id.to_string(),
            within: None,
        })
    }

    /// Asking about what a search found: those emails, with the question only ranking them.
    pub(crate) fn search_results(&self, f: &SearchFilters, scope: Option<i64>) -> Result<Around> {
        let found = self
            .store
            .search_threads_where(
                f,
                &[],
                RESULTS_SHOWN,
                false,
                |t: &crate::model::ThreadSummary| scope.is_none_or(|a| t.account_id == a),
            )?
            .0;
        Ok(Around {
            with: Vec::new(),
            account_id: 0,
            thread_id: String::new(),
            within: Some(message_ids(&found)),
        })
    }
}

/// What was found, with how many matched exactly.
fn counted(found: Vec<Passage>) -> (Vec<Passage>, usize) {
    let exact = found.iter().filter(|p| !p.related).count();
    (found, exact)
}

/// What's left of an answer once everything the emails don't back up is taken out.
#[derive(Debug, Clone)]
pub(crate) struct Checked {
    pub answer: String,
    pub points: Vec<(String, Vec<i64>)>,
    /// The facts taken out, as the AI wrote them.
    pub removed: Vec<String>,
    pub not_found: bool,
    /// Follow-up questions that don't bring in anything the emails don't say.
    pub suggestions: Vec<String>,
}

/// Text as compared: no case, no accents, single spaces.
fn plainly(text: &str) -> String {
    fold(text).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The answer cut into sentences, so one with a wrong fact can go on its own.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut now = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        now.push(*c);
        let ends =
            matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if ends || matches!(c, '。' | '！' | '？') {
            out.push(now.trim().to_string());
            now.clear();
        }
    }
    if !now.trim().is_empty() {
        out.push(now.trim().to_string());
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Numbers in an answer worth checking: amounts, times, dates and codes with two digits or more.
fn numbers_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<String>| {
        let kept = run.trim_end_matches(['.', ',', ':', '/', '-']).to_string();
        if kept.chars().filter(char::is_ascii_digit).count() >= 2 && !out.contains(&kept) {
            out.push(kept);
        }
        run.clear();
    };
    for c in text.chars() {
        let joins = matches!(c, '.' | ',' | ':' | '/' | '-') && !run.is_empty();
        if c.is_ascii_digit()
            || joins
            || (matches!(c, '$' | '€' | '£' | '¥' | '₹' | '৳') && run.is_empty())
        {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// A number's digits in groups, without zeros in front: "$1,234.50" is 1 234 50, and so is
/// "1.234,50 $"; "09:40" is 9 40.
fn digit_groups(n: &str) -> Vec<&str> {
    n.split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .map(|g| match g.trim_start_matches('0') {
            "" => "0",
            g => g,
        })
        .collect()
}

/// Whether a text writes this number: the same groups of digits in a row, never part of a
/// longer number, so "$12" isn't in "$125".
/// A date may be written in another order ("07-11-2026" for "2026-11-07").
fn has_number(text: &str, n: &str) -> bool {
    let want = digit_groups(n);
    let date = want.len() == 3 && want.iter().any(|g| g.len() == 4);
    let mut sorted_want = want.clone();
    sorted_want.sort_unstable();
    !want.is_empty()
        && numbers_in(text).iter().any(|t| {
            digit_groups(t).windows(want.len()).any(|w| {
                w == want.as_slice()
                    || (date && {
                        let mut w = w.to_vec();
                        w.sort_unstable();
                        w == sorted_want
                    })
            })
        })
}

/// Booking codes and email addresses in a text, which have to be copied from an email.
fn codes_in(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '"'))
        .map(|w| w.trim_matches(|c: char| matches!(c, '.' | ':' | '!' | '?' | '\'' | '\u{2019}')))
        .filter(|w| {
            let code = (5..=8).contains(&w.len())
                && w.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && w.chars().any(|c| c.is_ascii_uppercase());
            let address = w.contains('@') && w.rsplit('@').next().is_some_and(|d| d.contains('.'));
            code || address
        })
        .map(str::to_string)
        .collect()
}

/// Keeps only what the emails the AI was given back up. `sent` is each email's id and all
/// the AI saw of it. A fact must be copied from the email it names; a number, booking code
/// or address in the answer must be in the emails, and in a point in the emails it names; a
/// sentence, point or suggested question holding anything else goes.
/// Asked to do something ("can you add up…"), the answer is only "yes" or "no": it says
/// nothing, as a small model sometimes answers the wording, not the request.
pub(crate) fn bare_yes_or_no(question: &str, answer: &str) -> bool {
    let q = question.trim().to_lowercase();
    let asks_to_do = [
        "can you ",
        "could you ",
        "will you ",
        "would you ",
        "please ",
    ]
    .iter()
    .any(|start| q.starts_with(start));
    let a = answer
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    asks_to_do && ["yes", "no", "yeah", "nope", "sure"].contains(&a.as_str())
}

pub(crate) fn check_answer(reply: &serde_json::Value, sent: &[(i64, String)]) -> Checked {
    let given: HashMap<i64, String> = sent.iter().map(|(id, t)| (*id, plainly(t))).collect();
    let all: Vec<&String> = given.values().collect();
    let mut removed: Vec<String> = Vec::new();
    let take_out = |text: &str, removed: &mut Vec<String>| {
        if !removed.iter().any(|r| plainly(r) == plainly(text)) {
            removed.push(text.to_string());
        }
    };
    for f in reply["facts"].as_array().into_iter().flatten() {
        let text = f["text"].as_str().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let backed = f["source"]
            .as_i64()
            .and_then(|id| given.get(&id))
            .is_some_and(|t| t.contains(&plainly(text)));
        if !backed {
            take_out(text, &mut removed);
        }
    }
    let wrong_facts = removed.clone();
    // What a text says that none of these emails do.
    let unbacked = |text: &str, from: &[&String]| -> Vec<String> {
        let mut out: Vec<String> = numbers_in(text)
            .into_iter()
            .filter(|n| !from.iter().any(|t| has_number(t, n)))
            .collect();
        out.extend(
            codes_in(text)
                .into_iter()
                .filter(|c| !from.iter().any(|t| t.contains(&fold(c)))),
        );
        out
    };
    let says = |s: &str, bad: &[String]| {
        let p = plainly(s);
        bad.iter().any(|r| p.contains(&plainly(r)))
    };
    let answer_text = reply["answer"].as_str().unwrap_or("");
    let mut answer_bad = wrong_facts.clone();
    answer_bad.extend(unbacked(answer_text, &all));
    let answer = sentences(answer_text)
        .into_iter()
        .filter(|s| !says(s, &answer_bad))
        .collect::<Vec<_>>()
        .join(" ");
    for r in &answer_bad {
        take_out(r, &mut removed);
    }
    let mut points: Vec<(String, Vec<i64>)> = Vec::new();
    for p in reply["points"].as_array().into_iter().flatten() {
        let text = p["text"].as_str().unwrap_or("").trim().to_string();
        let ids: Vec<i64> = p["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_i64)
            .filter(|id| given.contains_key(id))
            .collect();
        let from: Vec<&String> = if ids.is_empty() {
            all.clone()
        } else {
            ids.iter().filter_map(|id| given.get(id)).collect()
        };
        let own_bad = unbacked(&text, &from);
        for r in &own_bad {
            take_out(r, &mut removed);
        }
        if !text.is_empty() && own_bad.is_empty() && !says(&text, &wrong_facts) {
            points.push((text, ids));
        }
    }
    let suggestions: Vec<String> = reply["suggestions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|s| !says(s, &wrong_facts) && unbacked(s, &all).is_empty())
        .take(3)
        .map(str::to_string)
        .collect();
    let not_found =
        reply["not_found"].as_bool().unwrap_or(false) || (answer.is_empty() && points.is_empty());
    Checked {
        answer,
        points,
        removed,
        not_found,
        suggestions,
    }
}

/// The strongest first and the next strongest last, as models read the ends of what they're
/// given most carefully.
pub(crate) fn ends_first<T>(items: Vec<T>) -> Vec<T> {
    let mut front = Vec::new();
    let mut back = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if i % 2 == 0 {
            front.push(item);
        } else {
            back.push(item);
        }
    }
    back.reverse();
    front.extend(back);
    front
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_time_or_a_date_written_another_way_is_still_backed() {
        let sent = vec![(
            1,
            "<email id=\"1\">\nFrom: Salman\nSubject: Website\nDate: 2026-11-07 06:12 (6:12 AM)\nHi team.\n</email>"
                .to_string(),
        )];
        let reply = serde_json::json!({
            "answer": "Salman wrote at 6:12 AM on 07-11-2026 about your website.",
            "points": [], "facts": [], "not_found": false, "suggestions": []
        });
        let checked = check_answer(&reply, &sent);
        assert!(checked.answer.contains("Salman"), "{:?}", checked.removed);
        // A number no email has is still taken out.
        let reply = serde_json::json!({
            "answer": "Salman asked for $450.", "points": [], "facts": [], "not_found": false, "suggestions": []
        });
        assert!(check_answer(&reply, &sent).answer.is_empty());
    }

    #[test]
    fn a_request_answered_only_yes_or_no_says_nothing() {
        assert!(bare_yes_or_no(
            "can you calculate my thailand trip cost?",
            "no"
        ));
        assert!(bare_yes_or_no("Could you add up the invoices?", "Yes."));
        // A real yes/no question can be answered so.
        assert!(!bare_yes_or_no("did Ana pay?", "yes"));
        assert!(!bare_yes_or_no(
            "can you calculate my trip cost?",
            "About $1,240 in all."
        ));
    }
    use super::*;

    fn reply_saying(answer: &str) -> serde_json::Value {
        serde_json::json!({ "answer": answer, "points": [], "facts": [], "suggestions": [], "not_found": false })
    }

    #[test]
    fn a_number_must_be_a_whole_number_from_the_emails() {
        let sent = vec![(
            1,
            "The fee is $125.00, due 2030-03-04 at 09:40.".to_string(),
        )];
        let check = |answer: &str| check_answer(&reply_saying(answer), &sent).answer;
        assert_eq!(check("The fee is $12."), "", "part of a bigger number");
        assert_eq!(
            check("Die Gebühr beträgt 125,00 $."),
            "Die Gebühr beträgt 125,00 $.",
            "the same amount written the local way"
        );
        assert_eq!(check("It leaves at 9:40."), "It leaves at 9:40.");
    }

    #[test]
    fn a_code_or_an_address_must_be_in_the_emails() {
        let sent = vec![(1, "Booking ABCDEF for ana@acme.test.".to_string())];
        let check = |answer: &str| check_answer(&reply_saying(answer), &sent).answer;
        assert_eq!(check("Your booking is QWERTY."), "");
        assert_eq!(check("Write to bob@evil.test."), "");
        assert_eq!(check("Your booking is ABCDEF."), "Your booking is ABCDEF.");
    }

    #[test]
    fn a_point_is_checked_against_the_emails_it_names() {
        let sent = vec![(1, "Fee $125.".to_string()), (2, "Fee $50.".to_string())];
        let reply = serde_json::json!({
            "answer": "", "facts": [], "suggestions": [], "not_found": false,
            "points": [{ "text": "The fee is $50.", "ids": [1] }],
        });
        assert!(check_answer(&reply, &sent).points.is_empty());
    }

    #[test]
    fn a_suggestion_with_made_up_details_is_dropped() {
        let sent = vec![(1, "Fee $125.".to_string())];
        let mut reply = reply_saying("The fee is $125.");
        reply["suggestions"] = serde_json::json!(["Was the $900 refund sent?", "When is it due?"]);
        assert_eq!(check_answer(&reply, &sent).suggestions, ["When is it due?"]);
    }

    #[test]
    fn a_passage_stays_short_and_around_the_words() {
        let text = format!(
            "{} the Wyoming fee is $125 {}",
            "filler ".repeat(4_000),
            "more ".repeat(4_000)
        );
        let p = passage_around(&text, &["wyoming".to_string()], PASSAGE_CHARS);
        assert!(p.chars().count() <= PASSAGE_CHARS, "{}", p.chars().count());
        assert!(p.contains("Wyoming fee is $125"));
        assert_eq!(passage_around("short", &[], PASSAGE_CHARS), "short");
    }

    #[test]
    fn numbers_are_found_however_they_are_written() {
        assert_eq!(
            numbers_in("The fee was $900. Paid on 3 Nov at 09:40."),
            ["$900", "09:40"]
        );
        assert_eq!(numbers_in("ローマ行きは09:40です"), ["09:40"]);
        assert_eq!(numbers_in("Invoice 1042, 2 emails"), ["1042"]);
    }

    #[test]
    fn the_strongest_go_at_both_ends() {
        assert_eq!(ends_first(vec![1, 2, 3, 4, 5]), [1, 3, 5, 4, 2]);
    }
}
