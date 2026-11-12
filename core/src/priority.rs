//! Which emails need the person. Works without AI, from who wrote, who it was sent to and what
//! it says. Every decision can be corrected, and carries the reason shown beside it.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::model::{Account, Addr, ThreadSummary};
use crate::store::Store;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Sorted at most this many per refresh, so a big first download can't hold mail up.
const SORTS_PER_SYNC: i64 = 500;
const RECENT_DAYS: i64 = 60;
const NO_REPLY_AFTER_DAYS: i64 = 3;
const NO_REPLY_LOOK_BACK_DAYS: i64 = 30;
const DAY: i64 = 86400;
/// How long a "may need action" notice stays in Needs you.
const NOTICE_DAYS: i64 = 3;
pub(crate) const VOICE_EXAMPLES: i64 = 5;
const VOICE_CHARS: usize = 400;

pub const NEEDS_YOU: &str = "needs_you";
pub const GOOD_TO_KNOW: &str = "good_to_know";
pub const THE_REST: &str = "rest";

/// The person's choice for one sender, kept in settings.
pub(crate) const CHOSEN: &str = "priority:";

pub struct Facts<'a> {
    pub from: &'a str,
    pub to: &'a [Addr],
    pub subject: &'a str,
    pub text: &'a str,
    /// `general`, `newsletters` or `promotions`.
    pub category: &'a str,
    pub risk: i64,
    pub is_contact: bool,
    /// The sender is answering something this person wrote.
    pub answers_me: bool,
    /// The sender wrote earlier in this conversation too and was never answered: a follow-up
    /// chain, which is how sales tools write.
    pub chases: bool,
    /// What the provider said about the sender being who they claim: `pass`, `weak`, `fail`, `none`.
    pub auth: &'a str,
    pub mine: &'a BTreeSet<String>,
    pub chosen: Option<&'a str>,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Decision {
    pub group: &'static str,
    pub why: Option<&'static str>,
    /// Worth a second opinion from AI, if the person has asked for that.
    pub unsure: bool,
}

pub(crate) const fn sure(group: &'static str, why: Option<&'static str>) -> Decision {
    Decision {
        group,
        why,
        unsure: false,
    }
}

/// Mail that's written to everyone and only pretends to be a question.
const COLD_PITCH: &[&str] = &[
    "book a call",
    "book a demo",
    "schedule a call",
    "schedule a demo",
    "quick call",
    "15 minutes",
    "15-minute",
    "20 minutes",
    "30-minute",
    "guest post",
    "link exchange",
    "backlink",
    "seo ",
    "our services",
    "our agency",
    "our team can",
    "we help companies",
    "we help businesses",
    "i came across your",
    "i found your",
    "i noticed your website",
    "i noticed that your",
    "increase your",
    "grow your",
    "boost your",
    "generate more",
    "more leads",
    "outsourc",
    "partnership opportunity",
    "collaboration opportunity",
    "sponsored post",
    "unsubscribe",
    "reply stop",
    "not interested",
    "came across",
    "stumbled upon",
    "any interest",
    "are you interested",
    "would you be interested",
    "would you be open",
    "are you open to",
    "open to a",
    "worth a chat",
    "worth exploring",
    "quick question",
    "idea for ",
    "no fee",
    "no cost",
    "free of charge",
    "free trial",
    "free audit",
    "free quote",
    "signing up",
    "your business",
    "your company",
    "your website",
    "your site",
    "your brand",
    "your listing",
    "your reviews",
    "google reviews",
    "more reviews",
    "more customers",
    "more clients",
    "new customers",
    "new clients",
    "booked appointments",
    "recent bid",
    "case study",
    "we can help",
    "i can help",
    "we'd love to",
    "i'd love to show",
    "just following up",
    "following up on my",
    "bumping this",
    "circling back",
    "last email",
    "ihr unternehmen",
    "ihre website",
    "ihren betrieb",
    "bewertungen",
    "kostenlos",
    "i can rank",
    "rank your",
    "rank above",
    "outrank",
    "first page of google",
    "page one of google",
    "top of google",
    "your competitors",
    "web design",
    "redesign your",
    "i can build",
    "i can get you",
    "i'd love to help",
];

/// The start of a bank or wallet alert, whatever its subject says.
const BANK_ALERT: &[&str] = &[
    "transaction alert",
    "transaction notification",
    "you have transferred",
    "you have received",
    "has been debited",
    "has been credited",
];

const ASKS: &[&str] = &[
    "can you",
    "could you",
    "would you",
    "will you",
    "please",
    "let me know",
    "need your",
    "your thoughts",
    "what do you think",
    "are you",
    "do you",
    "by tomorrow",
    "by monday",
    "by tuesday",
    "by wednesday",
    "by thursday",
    "by friday",
    "by end of",
    "deadline",
    "asap",
    "urgent",
    "approve",
    "confirm",
    "waiting for",
    "waiting on",
];

const NOT_REALLY_MAIL: &[&str] = &[
    "automatic reply",
    "auto-reply",
    "autoreply",
    "out of office",
    "out of the office",
    "undeliverable",
    "delivery status",
    "delivery has failed",
    "mail delivery",
    "accepted:",
    "declined:",
    "tentative:",
];

/// A company's mailbox, not a person's.
const SHARED_MAILBOX: &[&str] = &[
    "hello",
    "hi",
    "info",
    "support",
    "help",
    "sales",
    "team",
    "contact",
    "admin",
    "office",
    "mail",
    "email",
    "news",
    "accounts",
    "account",
    "orders",
    "care",
    "community",
    "welcome",
    "feedback",
    "enquiries",
    "inquiries",
    "membership",
    "members",
    "partners",
    "editor",
];

const COMPANY_WORDS: &[&str] = &[
    "invoice",
    "statement",
    "receipt",
    "billing",
    "payment",
    "payout",
    "verify",
    "verification",
    "service",
    "support",
    "notification",
    "notify",
    "announce",
    "newsletter",
    "marketing",
    "update",
    "alert",
    "security",
    "customer",
    "authors",
];

/// Companies often send bulk mail from a side address such as `t.example.com`.
const BULK_SUBDOMAIN: &[&str] = &[
    "t",
    "e",
    "em",
    "mg",
    "m",
    "mail",
    "email",
    "news",
    "info",
    "marketing",
    "go",
    "send",
];

/// Mail about a sign-in or a purchase, which is worth seeing but isn't waiting on an answer.
const JUST_A_RECORD: &[&str] = &[
    "verification code",
    "security code",
    "login code",
    "sign-in",
    "sign in",
    "log in",
    "login",
    "new device",
    "password",
    "confirm your",
    "verify your",
    "secure link",
    "magic link",
    "sign-in link",
    "login link",
    "one-time code",
    "one-time password",
    "your receipt",
    "receipt from",
    "invoice",
    "order confirmation",
    "your order",
    "has shipped",
    "privacy policy",
    "terms of",
];

/// Someone's account or money is affected, and mail like this can't wait, whoever sent it.
const NEEDS_ACTION: &[&str] = &[
    "payment failed",
    "failed payment",
    "was unsuccessful",
    "payment declined",
    "card declined",
    "action required",
    "action needed",
    "overdue",
    "past due",
    "final notice",
    "suspended",
    "will expire",
    "is expiring",
    "has expired",
    "expires in",
    "expire in",
];

pub(crate) fn is_company_address(address: &str) -> bool {
    crate::alerts::is_robot_address(address) || is_shared_or_company(address)
}

fn is_shared_or_company(address: &str) -> bool {
    let address = address.to_ascii_lowercase();
    let Some((local, domain)) = address.split_once('@') else {
        return true;
    };
    SHARED_MAILBOX.contains(&local)
        || COMPANY_WORDS.iter().any(|w| local.contains(w))
        || (domain.matches('.').count() >= 2
            && BULK_SUBDOMAIN
                .iter()
                .any(|b| domain.starts_with(&format!("{b}."))))
}

/// Only what this email adds, not the earlier ones quoted below it.
fn new_words(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('>') {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let quote_starts = lower.starts_with("-----original message")
            || lower.starts_with("from: ")
            || (lower.starts_with("on ") && lower.ends_with("wrote:"));
        if quote_starts {
            break;
        }
        out.push_str(&lower);
        out.push('\n');
        if out.len() > 3000 {
            break;
        }
    }
    out
}

/// What someone wrote, without what they were replying to.
fn own_words(text: &str) -> String {
    without_quotes(text).chars().take(VOICE_CHARS).collect()
}

/// What someone wrote themselves, without the earlier emails quoted under it.
/// Answers written between the quoted lines keep their own words: then only the quoted
/// lines are old.
pub(crate) fn without_quotes(text: &str) -> String {
    let header = |lower: &str| lower.starts_with("on ") && lower.ends_with("wrote:");
    let mut quoted = false;
    let between = text.lines().any(|line| {
        let t = line.trim();
        if t.starts_with('>') {
            quoted = true;
            false
        } else {
            quoted && !t.is_empty() && !header(&t.to_ascii_lowercase())
        }
    });
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let old = trimmed.starts_with('>') || header(&lower);
        if lower.starts_with("-----original message") || (old && !between) {
            break;
        }
        if !old {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn asks_something(words: &str) -> bool {
    words.contains('?') || ASKS.iter().any(|a| words.contains(a))
}

/// A thank-you doesn't need an answer.
fn only_thanks(words: &str) -> bool {
    let short = words.trim().len() < 80;
    short && !words.contains('?') && (words.contains("thank") || words.contains("thx"))
}

pub fn decide(f: &Facts) -> Decision {
    if let Some(chosen) = f.chosen {
        for group in [NEEDS_YOU, GOOD_TO_KNOW, THE_REST] {
            if chosen == group {
                return sure(group, Some("You chose this for this sender"));
            }
        }
    }
    if f.risk > 0 {
        return sure(GOOD_TO_KNOW, None);
    }
    if f.category != "general" {
        return sure(THE_REST, None);
    }
    let subject = f.subject.to_ascii_lowercase();
    // A sign-in code "expires in 15 minutes" too, but there's nothing to do once it's used.
    let sign_in = JUST_A_RECORD.iter().any(|r| subject.contains(r));
    // Only when the sender is proven, because scams love these exact words.
    if (f.auth == "pass" || f.is_contact)
        && !sign_in
        && NEEDS_ACTION.iter().any(|n| subject.contains(n))
    {
        return sure(NEEDS_YOU, Some("May need action"));
    }
    if crate::alerts::is_robot_address(f.from)
        || NOT_REALLY_MAIL.iter().any(|n| subject.contains(n))
    {
        return sure(GOOD_TO_KNOW, None);
    }
    let direct =
        f.to.iter()
            .any(|a| f.mine.contains(&a.address.to_lowercase()));
    let words = new_words(f.text);
    let company = is_company_address(f.from);
    // Receipts, orders, bills and bank alerts are a record of something done, even when
    // their small print asks "Questions?" or says to contact them.
    let record = sign_in
        || crate::purchases::is_purchase(f.subject, f.from)
        || BANK_ALERT.iter().any(|b| words.trim_start().starts_with(b));
    if f.answers_me || (f.is_contact && !company) {
        if !direct {
            return sure(GOOD_TO_KNOW, Some("You were copied in"));
        }
        // A mailing from someone they've written to, such as their own newsletter.
        let mailing = f.text.to_ascii_lowercase().contains("unsubscribe");
        if only_thanks(&words) || record || mailing {
            return sure(GOOD_TO_KNOW, None);
        }
        let why = if f.answers_me {
            "Replied to you"
        } else {
            "You've written to them before"
        };
        return sure(NEEDS_YOU, Some(why));
    }
    let bulk = f.text.to_ascii_lowercase().contains("unsubscribe");
    let everything = format!("{subject}\n{words}");
    // A plain-text part that's really HTML was written by a sending tool, not typed.
    let tool_made = words.contains("<p style=") || words.contains("<div") || words.contains("<br");
    let cold = f.chases || tool_made || COLD_PITCH.iter().any(|p| everything.contains(p));
    if company || !direct || bulk || cold || record {
        return sure(GOOD_TO_KNOW, None);
    }
    if asks_something(&everything) {
        return Decision {
            group: NEEDS_YOU,
            why: Some("Asks you something"),
            unsure: true,
        };
    }
    Decision {
        group: GOOD_TO_KNOW,
        why: None,
        unsure: true,
    }
}

/// `YYYY-MM-DD`, as a time at the start of that day. Anything else, or a date long gone, is nothing.
pub(crate) fn due_date(text: &str) -> Option<i64> {
    let mut parts = text.trim().split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since 1970, from the civil calendar.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let at = (era * 146097 + doe - 719468) * DAY;
    (at > crate::identity::unix_now() - 30 * DAY).then_some(at)
}

struct Waiting {
    id: i64,
    from: String,
    to: String,
    subject: String,
    text: String,
    category: String,
    risk: i64,
    auth: String,
    answers_me: bool,
    chases: bool,
}

/// How many conversations in this inbox need the person, or only the unread ones.
pub(crate) fn needs_you_sql(unread: bool) -> String {
    let still_unread = if unread {
        "AND SUM(m.seen = 0) > 0"
    } else {
        ""
    };
    const ACCOUNT: &str = "(SELECT account_id FROM folders WHERE id = ?1)";
    format!(
        "SELECT COUNT(*) FROM (
               SELECT 1 FROM messages m JOIN folders f ON f.id = m.folder_id
               LEFT JOIN thread_marks k ON k.account_id = m.account_id AND k.thread_id = m.thread_id
               WHERE m.account_id = {ACCOUNT}
                 AND m.thread_id IN (
                   SELECT thread_id FROM messages WHERE folder_id = ?1 AND priority = 'needs_you'
                   UNION SELECT thread_id FROM thread_marks WHERE account_id = {ACCOUNT}
                     AND (priority = 'needs_you' OR returned_at IS NOT NULL OR follow_due = 1))
                 AND m.thread_id NOT IN (SELECT thread_id FROM thread_marks
                   WHERE account_id = {ACCOUNT} AND (muted = 1 OR snooze_until IS NOT NULL))
                 AND f.role NOT IN ('trash', 'junk', 'drafts') AND m.is_draft = 0
               GROUP BY m.thread_id
               HAVING {needs} AND SUM(f.role = 'inbox' AND m.screened = 0) > 0 {still_unread})",
        needs = crate::store::NEEDS_YOU_SQL,
    )
}

impl Store {
    /// Their own words from this many of their latest sent emails, leaving out the
    /// folders named. Whether AI may read this inbox at all is checked before asking.
    pub(crate) fn how_they_write_where(
        &self,
        account_id: i64,
        most: i64,
        skip_folders: &[String],
    ) -> Result<Vec<String>> {
        let skip = serde_json::to_string(skip_folders).unwrap_or_else(|_| "[]".into());
        let db = self.db();
        // A few more than asked for, as some turn out to have none of their own words.
        // Mail they sent encrypted was meant for its reader only, so it isn't an example.
        let mut st = db.prepare_cached(&format!(
            "SELECT b.text FROM messages m JOIN folders f ON f.id = m.folder_id
             JOIN bodies b ON b.message_id = m.id
             WHERE m.account_id = ?1 AND f.role = 'sent' AND b.text IS NOT NULL AND b.text != ''
               AND f.path NOT IN (SELECT value FROM json_each(?3))
               AND NOT {}
             ORDER BY m.date DESC LIMIT ?2",
            crate::store::ENCRYPTED_SQL
        ))?;
        let rows = st.query_map(params![account_id, most * 2, skip], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows
            .filter_map(rusqlite::Result::ok)
            .map(|text| own_words(&text))
            .filter(|text| !text.is_empty())
            .take(most.max(0) as usize)
            .collect())
    }

    /// A notice like "expires in 24 hours" has had its moment after a few days. Returns how many.
    fn retire_old_notices(&self, account_id: i64) -> Result<usize> {
        Ok(self.db().execute(
            "UPDATE messages SET priority = 'good_to_know', priority_why = NULL
             WHERE account_id = ?1 AND priority = 'needs_you' AND priority_why = 'May need action'
               AND date <= ?2",
            params![account_id, crate::identity::unix_now() - NOTICE_DAYS * DAY],
        )?)
    }

    fn mail_to_prioritise(&self, account_id: i64) -> Result<Vec<Waiting>> {
        let db = self.db();
        let cutoff = crate::identity::unix_now() - RECENT_DAYS * DAY;
        // Too old to be waiting on anyone now.
        db.execute(
            "UPDATE messages SET priority_done = 1
             WHERE account_id = ?1 AND priority_done = 0 AND date <= ?2",
            params![account_id, cutoff],
        )?;
        // Waits for the spam check, so a scam never gets called important.
        let mut st = db.prepare_cached(
            "SELECT m.id, m.from_addr, m.to_json, m.subject, COALESCE(b.text, m.snippet), m.category,
                    COALESCE(v.risk, 0), m.auth,
                    EXISTS (SELECT 1 FROM messages s JOIN folders sf ON sf.id = s.folder_id
                            WHERE s.account_id = m.account_id AND s.thread_id = m.thread_id
                              AND sf.role = 'sent' AND s.date < m.date),
                    EXISTS (SELECT 1 FROM messages e
                            WHERE e.account_id = m.account_id AND e.thread_id = m.thread_id
                              AND e.id != m.id AND e.date < m.date
                              AND lower(e.from_addr) = lower(m.from_addr))
             FROM messages m JOIN folders f ON f.id = m.folder_id
             LEFT JOIN bodies b ON b.message_id = m.id
             LEFT JOIN verdicts v ON v.message_key = m.message_key
             WHERE m.account_id = ?1 AND m.priority_done = 0 AND f.role = 'inbox'
               AND (m.old_mail != 0 OR v.message_key IS NOT NULL)
             ORDER BY m.date DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![account_id, SORTS_PER_SYNC], |r| {
            Ok(Waiting {
                id: r.get(0)?,
                from: r.get(1)?,
                to: r.get(2)?,
                subject: r.get(3)?,
                text: r.get(4)?,
                category: r.get(5)?,
                risk: r.get(6)?,
                auth: r.get(7)?,
                answers_me: r.get(8)?,
                chases: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether the group it's in changed, which the lists on screen need to hear about.
    fn save_priority(&self, id: i64, d: &Decision) -> Result<bool> {
        let db = self.db();
        let before: Option<String> =
            db.query_row("SELECT priority FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })?;
        db.execute(
            "UPDATE messages SET priority = ?2, priority_why = ?3, priority_unsure = ?4,
                    priority_done = 1 WHERE id = ?1",
            params![id, d.group, d.why, d.unsure],
        )?;
        Ok(before.is_some_and(|b| b != d.group))
    }

    /// Once a stranger's follow-up shows it's a sales chain, their earlier emails in that
    /// conversation stop needing the person too. A choice the person made stays.
    fn quiet_earlier_pitches(&self, id: i64) -> Result<usize> {
        let quieted = self.db().execute(
            "UPDATE messages SET priority = 'good_to_know', priority_why = NULL, priority_unsure = 0
             WHERE priority = 'needs_you'
               AND (priority_why IS NULL OR priority_why IN ('Asks you something', 'AI thinks this needs you'))
               AND (account_id, thread_id, lower(from_addr)) =
                   (SELECT account_id, thread_id, lower(from_addr) FROM messages WHERE id = ?1)
               AND id != ?1",
            [id],
        )?;
        Ok(quieted)
    }

    /// `None` leaves whether it's waiting on them to the mail rules. A date with no line is kept.
    pub(crate) fn save_ai_priority(
        &self,
        id: i64,
        group: Option<&str>,
        line: &str,
        due_at: Option<i64>,
    ) -> Result<()> {
        let group = group.filter(|g| *g == NEEDS_YOU || *g == GOOD_TO_KNOW);
        self.db().execute(
            "UPDATE messages SET
                 priority = COALESCE(?2, priority),
                 priority_why = CASE WHEN ?2 = 'needs_you' AND priority != 'needs_you'
                                     THEN 'AI thinks this needs you'
                                     WHEN ?2 = 'good_to_know' THEN NULL ELSE priority_why END,
                 priority_unsure = 0,
                 ai_line = NULLIF(?3, ''),
                 due_at = ?4
             WHERE id = ?1",
            params![id, group, line, due_at],
        )?;
        Ok(())
    }

    /// Nothing clears a choice of ours by itself, since it's the person's.
    pub fn set_thread_priority(
        &self,
        account_id: i64,
        thread_id: &str,
        group: Option<&str>,
    ) -> Result<()> {
        if group.is_some_and(|g| g != NEEDS_YOU && g != GOOD_TO_KNOW) {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        let thread = self.real_thread(thread_id)?;
        self.db().execute(
            "INSERT INTO thread_marks (account_id, thread_id, priority, priority_upto)
             VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(date), 0) FROM messages
                                  WHERE account_id = ?1 AND thread_id = ?2))
             ON CONFLICT (account_id, thread_id) DO UPDATE SET
                 priority = excluded.priority, priority_upto = excluded.priority_upto",
            params![account_id, thread, group],
        )?;
        // Back from snooze or a reminder that came up puts it in Needs you too, so saying it
        // doesn't need them settles those as well. A later reminder still brings it back.
        if group == Some(GOOD_TO_KNOW) {
            self.clear_returned(account_id, &thread)?;
        }
        Ok(())
    }

    pub(crate) fn is_needs_you(&self, account_id: i64, rfc_id: &str) -> Result<bool> {
        Ok(self.db().query_row(
            "SELECT COUNT(*) FROM messages
                 WHERE account_id = ?1 AND rfc_id = ?2 AND priority = 'needs_you'",
            params![account_id, rfc_id],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

    /// The start of what an email says, as the list shows it, for a notification's preview.
    /// Empty while its text hasn't downloaded yet.
    pub(crate) fn preview_of(&self, account_id: i64, rfc_id: &str) -> Result<String> {
        Ok(self
            .db()
            .query_row(
                "SELECT COALESCE(snippet, '') FROM messages
                 WHERE account_id = ?1 AND rfc_id = ?2 ORDER BY id DESC LIMIT 1",
                params![account_id, rfc_id],
                |r| r.get(0),
            )
            .unwrap_or_default())
    }

    /// Who's waiting, whether or not it's been opened: reading an email doesn't answer it.
    /// With `unread`, only those with something still to read, for when that's all that's showing.
    pub fn needs_you_count(&self, folder_id: i64, unread: bool) -> Result<i64> {
        let db = self.reading_db();
        let sql = needs_you_sql(unread);
        Ok(db.query_row(&sql, [folder_id], |r| r.get(0))?)
    }

    /// Conversations where this person wrote last, a while ago, and nobody has answered.
    pub(crate) fn no_reply_threads(&self) -> Result<Vec<ThreadSummary>> {
        let now = crate::identity::unix_now();
        let found: Vec<(i64, String)> = {
            let db = self.db();
            let mut st = db.prepare_cached(
                "SELECT m.account_id, m.thread_id FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 LEFT JOIN thread_marks k ON k.account_id = m.account_id AND k.thread_id = m.thread_id
                 WHERE m.thread_id IN (
                     SELECT s.thread_id FROM messages s JOIN folders sf ON sf.id = s.folder_id
                     WHERE sf.role = 'sent' AND s.date BETWEEN ?1 AND ?2)
                   AND f.role NOT IN ('trash', 'junk', 'drafts') AND m.is_draft = 0
                 GROUP BY m.account_id, m.thread_id
                 HAVING MAX(CASE WHEN f.role = 'sent' THEN m.date END) >= MAX(m.date)
                    AND COALESCE(MAX(k.priority), '') = '' AND COALESCE(MAX(k.muted), 0) = 0
                 ORDER BY MAX(m.date) DESC LIMIT 50",
            )?;
            let rows = st.query_map(
                params![
                    now - NO_REPLY_LOOK_BACK_DAYS * DAY,
                    now - NO_REPLY_AFTER_DAYS * DAY
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for (account, thread) in found {
            if let Some(summary) = self.summary_of(account, &thread)? {
                out.push(summary);
            }
        }
        Ok(out)
    }

    /// Anything still in the inbox with a date coming up, soonest first.
    /// Emails with a due date from yesterday to two months ahead, soonest first.
    pub(crate) fn upcoming_due_messages(&self, limit: i64) -> Result<Vec<i64>> {
        let now = crate::identity::unix_now();
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT m.id FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.due_at BETWEEN ?1 AND ?2 AND f.role NOT IN ('junk', 'trash', 'drafts')
             ORDER BY m.due_at LIMIT ?3",
        )?;
        let rows = st.query_map(params![now - DAY, now + 60 * DAY, limit], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn threads_with_deadlines(&self) -> Result<Vec<ThreadSummary>> {
        let now = crate::identity::unix_now();
        let found: Vec<(i64, String)> = {
            let db = self.db();
            let mut st = db.prepare_cached(
                "SELECT DISTINCT m.account_id, m.thread_id FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 WHERE m.due_at BETWEEN ?1 AND ?2 AND f.role = 'inbox'
                 ORDER BY m.due_at LIMIT 50",
            )?;
            let rows = st.query_map(params![now - 7 * DAY, now + 30 * DAY], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for (account, thread) in found {
            if let Some(summary) = self.summary_of(account, &thread)? {
                out.push(summary);
            }
        }
        Ok(out)
    }

    fn brief_counts(&self) -> Result<(i64, i64, i64)> {
        let now = crate::identity::unix_now();
        // Only reads, so it doesn't hold up saving, replying or anything else meanwhile.
        let db = self.reading_db();
        let deadlines = db.query_row(
            "SELECT COUNT(DISTINCT m.thread_id) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.due_at BETWEEN ?1 AND ?2 AND f.role = 'inbox'",
            params![now - 2 * DAY, now + 2 * DAY],
            |r| r.get(0),
        )?;
        // In Spam, but from someone they've written to, or answering them.
        let maybe_real = db.query_row(
            "SELECT COUNT(*) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE f.role = 'junk' AND m.date > ?1
               AND (lower(m.from_addr) IN (SELECT address FROM contacts)
                    OR EXISTS (SELECT 1 FROM messages s JOIN folders sf ON sf.id = s.folder_id
                               WHERE s.account_id = m.account_id AND s.thread_id = m.thread_id
                                 AND sf.role = 'sent'))",
            [now - 14 * DAY],
            |r| r.get(0),
        )?;
        let sorted = db.query_row(
            "SELECT COUNT(*) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE f.role = 'inbox' AND m.date > ?1 AND COALESCE(m.priority, '') != 'needs_you'",
            [now - DAY],
            |r| r.get(0),
        )?;
        Ok((deadlines, maybe_real, sorted))
    }
}

impl Engine {
    /// Returns how many already-sorted emails moved to another group, so the lists showing
    /// them are read again.
    pub(crate) fn prioritise_mail(&self, account: &Account) -> Result<usize> {
        let mut moved = self.store.retire_old_notices(account.id)?;
        let waiting = self.store.mail_to_prioritise(account.id)?;
        if waiting.is_empty() {
            return Ok(moved);
        }
        let settings = self.store.all_settings()?;
        let mine = self.my_addresses(account);
        let contacts = self.store.written_to()?;
        for m in waiting {
            let address = m.from.to_lowercase();
            let to: Vec<Addr> = serde_json::from_str(&m.to).unwrap_or_default();
            let decision = if mine.contains(&address) {
                sure(GOOD_TO_KNOW, None)
            } else {
                decide(&Facts {
                    from: &m.from,
                    to: &to,
                    subject: &m.subject,
                    text: &m.text,
                    category: &m.category,
                    risk: m.risk,
                    is_contact: contacts.contains(&address),
                    answers_me: m.answers_me,
                    chases: m.chases,
                    auth: &m.auth,
                    mine: &mine,
                    chosen: settings
                        .get(&format!("{CHOSEN}{address}"))
                        .map(String::as_str),
                })
            };
            moved += usize::from(self.store.save_priority(m.id, &decision)?);
            if m.chases && !m.answers_me && decision.group == GOOD_TO_KNOW && decision.why.is_none()
            {
                moved += self.store.quiet_earlier_pitches(m.id)?;
            }
        }
        Ok(moved)
    }

    /// Always for this sender, including what's already here.
    pub(crate) fn set_sender_priority(&self, address: &str, group: &str) -> Result<()> {
        if ![NEEDS_YOU, GOOD_TO_KNOW, THE_REST, ""].contains(&group) {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        let address = address.trim().to_lowercase();
        let key = format!("{CHOSEN}{address}");
        if group.is_empty() {
            self.store.delete_setting(&key)?;
        } else {
            self.store.set_setting(&key, group)?;
        }
        self.store.db().execute(
            "UPDATE messages SET priority_done = 0 WHERE lower(from_addr) = ?1 AND date > ?2",
            params![address, crate::identity::unix_now() - RECENT_DAYS * DAY],
        )?;
        for account in self.store.list_accounts()? {
            self.prioritise_mail(&account)?;
        }
        Ok(())
    }

    /// Whether new mail is to be sorted by the AI that comes with Mailzo Cloud on this refresh.
    pub(crate) fn ai_round_due(&self) -> Result<bool> {
        let level = self.setting("ai_auto")?;
        // Older setting from before there were levels.
        Ok(level != "off" || self.setting("ai_sort")? == "on")
    }

    /// Today at a glance.
    pub(crate) fn brief(&self) -> Result<Value> {
        let mut needs_you = 0;
        for (folder, prefs) in self.store.inbox_folder_ids()? {
            let prefs: crate::model::AccountPrefs =
                serde_json::from_str(&prefs).unwrap_or_default();
            if prefs.in_all_inbox {
                needs_you += self.store.needs_you_count(folder, false)?;
            }
        }
        let (deadlines, maybe_real, sorted) = self.store.brief_counts()?;
        Ok(json!({
            "needs_you": needs_you,
            "deadlines": deadlines,
            "no_reply": self.store.no_reply_threads()?.len(),
            "maybe_real": maybe_real,
            "sorted": sorted,
        }))
    }
}
