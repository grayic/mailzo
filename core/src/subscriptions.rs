//! Subscriptions: what this person pays for again and again, worked out from their receipts,
//! renewal reminders, cancellations and failed payments. Shown on the Purchases page with what
//! each one costs, when it renews next, and what all of them add up to.
//!
//! Nothing is saved for it but the person's own changes: it's worked out afresh each time from
//! the mail that's here, so a new receipt or a cancellation shows the next time it's opened.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::store::Store;
use chrono::{Datelike, NaiveDate};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The person's own changes and the ones they added by hand, kept as one setting.
const SETTING: &str = "subscriptions";
/// How far back mail is read. A yearly plan shows up with one receipt from last year.
const LOOK_BACK_DAYS: i64 = 400;
/// How much of an email is read for prices and dates.
const READ_CHARS: usize = 8_000;
/// Receipts this close together are one payment sent twice, such as a receipt and an invoice.
const SAME_PAYMENT_DAYS: i64 = 3;
const DAY: i64 = 86_400;

/// Sent on behalf of the business that's really being paid. Their name is in the sender's name.
const PAYMENT_SERVICES: &[&str] = &[
    "stripe.com",
    "paddle.com",
    "paddle.net",
    "lemonsqueezy.com",
    "gumroad.com",
    "chargebee.com",
    "fastspring.com",
    "recurly.com",
    "braintreegateway.com",
    "2checkout.com",
    "memberful.com",
    "patreon.com",
];

/// One-off things bought or moved, never a subscription however often they come.
const NOT_SUBSCRIPTIONS: &[&str] = &[
    "transaction alert",
    "transaction notification",
    "statement",
    "debited",
    "credited",
    "transfer",
    "shipped",
    "on its way",
    "out for delivery",
    "delivered",
    "your order",
    "order #",
    "order confirm",
    "order placed",
    "order received",
    "booking",
    "your trip",
    "e-ticket",
    "top-up",
    "top up",
    "recharge",
    "refund",
    "money sent",
    "money received",
    "you sent",
    // Money moving between accounts, and bills paid to people: payments, but not subscriptions.
    "bill payment",
    "billpay",
    "credit card",
    "card payment",
    "card bill",
    "transaction",
    "rtgs",
    "beftn",
    "npsb",
    "you received",
    "you've received",
    "you have received",
    "received from",
    "payout",
    "deposit",
    "withdraw",
    "sent you",
    "paid you",
    "new sale",
    "made a sale",
    "earnings",
    "order summary",
    "maturity",
    "invoice payment",
    "remittance",
];

/// Selling something, rather than a receipt for it.
const SELLING: &[&str] = &[
    "offer",
    "% off",
    "discount",
    "cashback",
    "cash back",
    "deal",
    " sale",
    "save up to",
    "get up to",
    "coupon",
    "promo",
    "black friday",
    "limited time",
    "lifetime",
];

/// What a bank or money app writes about a transfer or a card, never about a subscription.
const BANK_WORDS: &[&str] = &[
    "transaction id",
    "reference number",
    "from account",
    "to account",
    "beneficiary",
    "cardmember",
    "card member",
    "routing no",
    "account number",
    "available balance",
    "minimum payment",
    "minimum amount due",
    "statement balance",
    "delivery eta",
];

/// Never a subscription, whatever else the email says: things bought once, credit top-ups,
/// refunds, card mirrors of other charges, and sales this person made to their own customers.
const NEVER_SUBJECT: &[&str] = &[
    "lifetime",
    "one-time",
    "one time",
    "refund",
    "renew now",
    "expire in",
    "will expire",
    "a charge was",
    "money added",
    "new sale",
    "made a sale",
    "sold for",
    "new order from",
    "trial cancelled:",
    "trial canceled:",
    "auto conversion",
    "direct debit",
    "deposited",
    "reinvestment",
];

/// Wording inside an email that rules it out the same way.
const NEVER_BODY: &[&str] = &[
    "lifetime access",
    "lifetime license",
    "lifetime deal",
    "lifetime plan",
    "one-time purchase",
    "one time purchase",
    "one-time payment",
    "no renewals",
    "pay once",
    "auto-recharge",
    "credits qty",
    "order type one-time",
];

/// A subject that says this is a bill or receipt for something of theirs.
const BILLING_SUBJECT: &[&str] = &[
    "receipt",
    "invoice",
    "subscription payment",
    "payment confirmation",
    "payment confirmed",
    "payment successful",
    "payment received",
    "renewed",
    "renewal confirmation",
    "thanks for your payment",
    "thank you for your payment",
    "your subscription",
    "subscription is confirmed",
    "subscription confirmed",
    "plan changed",
];

/// Words that say a payment repeats.
const REPEATS: &[&str] = &[
    "subscription",
    "subscribed",
    "renew",
    "membership",
    "billing period",
    "billing cycle",
    "next billing",
    "next payment",
    "next charge",
    "per month",
    "per year",
    "/month",
    "/mo",
    "/year",
    "/yr",
    "a month",
    "a year",
    "monthly",
    "annually",
    "yearly",
    "annual plan",
    "pro plan",
    "your plan",
    "recurring",
    "auto-renew",
    "per period",
    "per subscription",
    "until you cancel",
];

const FAILED: &[&str] = &[
    "payment failed",
    "payment declined",
    "payment was declined",
    "card was declined",
    "card declined",
    "couldn't process your payment",
    "could not process your payment",
    "unable to process your payment",
    "unable to charge",
    "couldn't charge",
    "could not charge",
    "update your payment",
    "update your card",
    "update your billing",
    "problem with your payment",
    "issue with your payment",
    "payment issue",
    "payment past due",
    "is past due",
    "was unsuccessful",
    "payment failure",
    "billing problem",
    "didn't go through",
    "did not go through",
    "update payment details",
    "auto-renewals failed",
    "renewal failed",
];

const CANCELLED: &[&str] = &[
    "subscription has been cancel",
    "subscription was cancel",
    "subscription is cancel",
    "subscription cancel",
    "subscription has ended",
    "subscription ended",
    "subscription has expired",
    "subscription expired",
    "plan has been cancel",
    "plan was cancel",
    "plan cancel",
    "membership has been cancel",
    "membership cancel",
    "cancellation confirm",
    "confirming your cancellation",
    "sorry to see you go",
    "you've cancelled",
    "you have cancelled",
    "you've canceled",
    "you have canceled",
    "will not renew",
    "won't renew",
    "auto-renew is off",
    "auto-renewal has been turned off",
    "auto-renew has been turned off",
    "cancelled your",
    "canceled your",
    "subscription canceled",
    "access has ended",
    "has been canceled",
    "has been cancelled",
];

const COMING_UP: &[&str] = &[
    "will renew",
    "renews on",
    "renews soon",
    "renew soon",
    "renewing soon",
    "about to renew",
    "renewal reminder",
    "upcoming renewal",
    "upcoming payment",
    "upcoming charge",
    "upcoming invoice",
    "renewal notice",
    "trial ends",
    "trial is ending",
    "trial will end",
    "will be charged",
    "will be billed",
    "will automatically renew",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Paid,
    ComingUp,
    Cancelled,
    Failed,
}

/// One email, as far as subscriptions go.
#[derive(Debug, Clone)]
pub(crate) struct Mail {
    pub account_id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub date: i64,
    /// What it says, as plain text: its body if that's here, otherwise its preview.
    pub text: String,
    /// Sent to a list, with a way to unsubscribe: an ad or a newsletter, unless its subject
    /// plainly says it's a bill.
    pub marketing: bool,
}

/// An amount of money in one currency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Money {
    pub amount: f64,
    pub currency: &'static str,
}

/// How often it's paid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Week,
    Month,
    Quarter,
    HalfYear,
    Year,
}

impl Period {
    fn days(self) -> i64 {
        match self {
            Period::Week => 7,
            Period::Month => 30,
            Period::Quarter => 91,
            Period::HalfYear => 182,
            Period::Year => 365,
        }
    }

    /// How late a payment can be before it's taken as stopped.
    fn grace_days(self) -> i64 {
        match self {
            Period::Week => 4,
            Period::Month => 10,
            Period::Quarter => 20,
            Period::HalfYear => 25,
            Period::Year => 30,
        }
    }

    fn months(self) -> u32 {
        match self {
            Period::Week | Period::Month => 1,
            Period::Quarter => 3,
            Period::HalfYear => 6,
            Period::Year => 12,
        }
    }

    /// The day after `from` it's due again: the same day of the month where it has one.
    fn after(self, from: i64) -> i64 {
        if self == Period::Week {
            return from + 7 * DAY;
        }
        let Some(start) = chrono::DateTime::from_timestamp(from, 0) else {
            return from + self.days() * DAY;
        };
        let start = start.naive_utc();
        match start.checked_add_months(chrono::Months::new(self.months())) {
            Some(next) => next.and_utc().timestamp(),
            None => from + self.days() * DAY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    /// A payment didn't go through, so it may stop unless the card is fixed.
    Failed,
    /// Cancelled, but paid for until a date still to come.
    Ending,
    Cancelled,
    /// No payment came when one was due, so it's likely stopped. The person can say.
    Unsure,
    /// No payment for longer than a whole period after one was due: stopped, without an email
    /// saying so.
    Stopped,
}

/// One subscription, as the Purchases page shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Subscription {
    pub key: String,
    pub name: String,
    pub account_id: Option<i64>,
    /// Who sends its mail, for its logo and for showing its emails.
    pub addresses: Vec<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub period: Option<Period>,
    /// Said in an email or by the person, rather than guessed from the gaps between payments.
    pub period_sure: bool,
    pub last_paid: Option<i64>,
    /// When it's due next. Left out once it has stopped.
    pub next: Option<i64>,
    /// From a renewal reminder or the person, rather than worked out from the last payment.
    pub next_sure: bool,
    pub status: Status,
    /// For one that's ending: the last day it's paid for.
    pub ends: Option<i64>,
    /// How many payments were found.
    pub payments: usize,
    /// Added by the person, with no emails behind it.
    pub manual: bool,
    /// The person changed something about it.
    pub edited: bool,
}

/// What the person changed about one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Edit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<Period>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<i64>,
    /// "active" or "cancelled", which beats anything the emails say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// Not a subscription at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// One change the person makes; whatever's left out stays as it was.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Change {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub period: Option<Period>,
    #[serde(default)]
    pub next: Option<i64>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub hidden: Option<bool>,
    /// Forgets every change made before this one.
    #[serde(default)]
    pub forget: bool,
}

/// One added by hand, for something billed without an email.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manual {
    pub key: String,
    pub name: String,
    pub amount: f64,
    pub currency: String,
    pub period: Period,
    #[serde(default)]
    pub next: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    edits: BTreeMap<String, Edit>,
    #[serde(default)]
    manual: Vec<Manual>,
}

/// What the page gets: the subscriptions, and the ones hidden as not being any.
#[derive(Debug, Clone, Serialize)]
pub struct Subscriptions {
    pub items: Vec<Subscription>,
    pub hidden: Vec<Subscription>,
}

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// What an email is about, from its subject, or the start of what it says.
fn kind_of(subject: &str, text: &str, marketing: bool) -> Option<Kind> {
    let subject = subject.to_lowercase();
    let start: String = text.chars().take(1_500).collect::<String>().to_lowercase();
    if has(&subject, NEVER_SUBJECT) || has(&start, NEVER_BODY) || has(&subject, SELLING) {
        return None;
    }
    let billing = has(&subject, BILLING_SUBJECT);
    // Mail sent to a list counts only when its subject says it's about their own bill.
    if marketing && !billing && !has(&subject, FAILED) && !has(&subject, COMING_UP) {
        return None;
    }
    if has(&subject, FAILED) {
        return Some(Kind::Failed);
    }
    if has(&subject, CANCELLED) || (has(&start, CANCELLED) && cancel_in_subject(&subject)) {
        return Some(Kind::Cancelled);
    }
    if has(&subject, COMING_UP)
        || has(
            &subject,
            &["subscription is confirmed", "subscription confirmed"],
        )
    {
        return Some(Kind::ComingUp);
    }
    let says_so = has(&subject, &["subscription", "renew", "membership"]);
    if !says_so && (has(&subject, NOT_SUBSCRIPTIONS) || has(&start, BANK_WORDS)) {
        return None;
    }
    billing.then_some(Kind::Paid)
}

/// A subject that could be about a cancellation, so its words are read for one.
fn cancel_in_subject(subject: &str) -> bool {
    subject.contains("cancel") || subject.contains("ended") || subject.contains("expired")
}

/// The business behind an email: the sender's own domain, or, for mail sent by a payment
/// service on someone's behalf, the name it's sent as.
fn business_of(from_name: &str, from_addr: &str, subject: &str) -> (String, String) {
    let host = from_addr
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let domain = company_domain(&host);
    let name = clean_name(from_name);
    // Google Play names what it's for: "Your subscription for Google One will renew".
    if domain == "google.com" {
        if let Some(what) = named_in(subject, "subscription for ") {
            return (format!("{domain}:{}", what.to_lowercase()), what);
        }
    }
    if PAYMENT_SERVICES.contains(&domain.as_str()) {
        let behind = if name.is_empty()
            || PAYMENT_SERVICES
                .iter()
                .any(|p| p.split('.').next() == Some(name.to_lowercase().as_str()))
        {
            from_subject(subject).unwrap_or_default()
        } else {
            name.clone()
        };
        if !behind.is_empty() {
            return (format!("{domain}:{}", behind.to_lowercase()), behind);
        }
    }
    let name = if name.is_empty() {
        title_of(&domain)
    } else {
        name
    };
    (domain, name)
}

/// The words after `lead` in a subject, up to "will", "has", "is" or punctuation.
fn named_in(subject: &str, lead: &str) -> Option<String> {
    let lower = subject.to_lowercase();
    let at = lower.find(lead)? + lead.len();
    let rest = &subject[at..];
    let lower_rest = &lower[at..];
    let end = [" will ", " has ", " is ", " was ", ":", ",", " - ", " ("]
        .iter()
        .filter_map(|w| lower_rest.find(w))
        .min()
        .unwrap_or(rest.len());
    let name = rest[..end].trim();
    (name.len() >= 2).then(|| name.to_string())
}

/// "Your receipt from Figma #1234" → "Figma".
fn from_subject(subject: &str) -> Option<String> {
    let lower = subject.to_lowercase();
    let at = lower.find(" from ")? + " from ".len();
    let rest = &subject[at..];
    let end = rest
        .find([' ', '#', '[', '(', '-', ',', '.'])
        .unwrap_or(rest.len());
    let name = rest[..end].trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// mail.figma.com and figma.com are one business; so are a.b.co.uk and b.co.uk.
fn company_domain(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    let keep = if parts.len() > 2
        && parts[parts.len() - 1].len() == 2
        && ["co", "com", "org", "net", "ac", "gov"].contains(&parts[parts.len() - 2])
    {
        3
    } else {
        2
    };
    parts[parts.len().saturating_sub(keep)..].join(".")
}

fn title_of(domain: &str) -> String {
    let first = domain.split('.').next().unwrap_or(domain);
    let mut chars = first.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "Figma Billing" → "Figma"; "The Notion Team" → "Notion"; "no-reply" → "".
fn clean_name(name: &str) -> String {
    const NOISE: &[&str] = &[
        "billing",
        "receipts",
        "receipt",
        "payments",
        "payment",
        "invoices",
        "invoice",
        "team",
        "support",
        "accounts",
        "account",
        "no-reply",
        "noreply",
        "no",
        "reply",
        "notifications",
        "the",
        "inc",
        "inc.",
        "llc",
        "ltd",
        "ltd.",
        "via",
        "subscriptions",
        "subscription",
        "store",
        "orders",
        "hq",
        "app",
        "-",
        "|",
        "·",
    ];
    let words: Vec<&str> = name
        .trim_matches(|c: char| c == '"' || c == '\'')
        .split_whitespace()
        .filter(|w| !NOISE.contains(&w.to_lowercase().trim_end_matches(',')))
        .collect();
    // "Stripe receipts for Figma" style names keep what follows "for".
    words.join(" ").trim_matches([',', ' ']).to_string()
}

const SYMBOLS: &[(&str, &str)] = &[
    ("US$", "USD"),
    ("CA$", "CAD"),
    ("C$", "CAD"),
    ("A$", "AUD"),
    ("AU$", "AUD"),
    ("NZ$", "NZD"),
    ("R$", "BRL"),
    ("HK$", "HKD"),
    ("S$", "SGD"),
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("₹", "INR"),
    ("৳", "BDT"),
    ("₩", "KRW"),
    ("₱", "PHP"),
    ("₺", "TRY"),
    ("₦", "NGN"),
    ("Rs.", "PKR"),
    ("Rs", "PKR"),
    ("Tk.", "BDT"),
    ("Tk", "BDT"),
];

const CODES: &[&str] = &[
    "USD", "EUR", "GBP", "CAD", "AUD", "NZD", "INR", "BDT", "PKR", "JPY", "CNY", "SGD", "HKD",
    "CHF", "SEK", "NOK", "DKK", "PLN", "BRL", "MXN", "ZAR", "AED", "SAR", "TRY", "KRW", "PHP",
    "IDR", "MYR", "THB", "VND", "NGN", "EGP", "KES",
];

/// Reads "1,234.56", "1.234,56", "12,99" or "4,500" as a number, and how many characters it took.
fn number_at(text: &str) -> Option<(f64, usize)> {
    let taken: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    let taken = taken.trim_end_matches([',', '.']);
    if taken.is_empty() || !taken.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let after = |at: usize| taken.len() - at - 1;
    let decimal = match (taken.rfind('.'), taken.rfind(',')) {
        (Some(d), Some(c)) => Some(d.max(c)),
        // "12.99" or "12.5"; "1.234" and "1.234.567" are thousands, as some countries write them.
        (Some(d), None) => (taken.matches('.').count() == 1 && after(d) <= 2).then_some(d),
        // "12,99" is a decimal; "4,500" is thousands.
        (None, Some(c)) => (taken.matches(',').count() == 1 && after(c) <= 2).then_some(c),
        (None, None) => None,
    };
    let mut cleaned = String::new();
    for (i, ch) in taken.char_indices() {
        if ch.is_ascii_digit() {
            cleaned.push(ch);
        } else if Some(i) == decimal {
            cleaned.push('.');
        }
    }
    let value: f64 = cleaned.parse().ok()?;
    Some((value, taken.len()))
}

/// Every amount of money written in some text, with where it starts.
fn amounts_in(text: &str) -> Vec<(usize, Money)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        // A symbol or code, then the number.
        let before = SYMBOLS
            .iter()
            .find(|(s, _)| rest.starts_with(s))
            .map(|(s, code)| (s.len(), *code))
            .or_else(|| {
                CODES
                    .iter()
                    .find(|c| {
                        rest.starts_with(*c)
                            && !text[..i].ends_with(|ch: char| ch.is_ascii_alphabetic())
                    })
                    .map(|c| (c.len(), *c))
            });
        if let Some((len, code)) = before {
            // A symbol right after a letter is part of a word, as in "Rs" inside "Users".
            let in_word = text[..i].ends_with(|ch: char| ch.is_ascii_alphanumeric())
                && rest.starts_with(|ch: char| ch.is_ascii_alphabetic());
            let after = rest[len..].trim_start_matches([' ', '\u{a0}']);
            let gap = rest.len() - len - after.len();
            if !in_word && gap <= 1 {
                if let Some((value, used)) = number_at(after) {
                    if value > 0.0 {
                        found.push((
                            i,
                            Money {
                                amount: value,
                                currency: code,
                            },
                        ));
                    }
                    i += len + gap + used;
                    continue;
                }
            }
        }
        // A number, then a code: "12.99 USD".
        if rest.starts_with(|c: char| c.is_ascii_digit())
            && !text[..i].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '.' || c == ',')
        {
            if let Some((value, used)) = number_at(rest) {
                let after = rest[used..].trim_start_matches([' ', '\u{a0}']);
                if let Some(code) = CODES.iter().find(|c| {
                    after.starts_with(*c)
                        && !after[c.len()..].starts_with(|ch: char| ch.is_ascii_alphabetic())
                }) {
                    if value > 0.0 {
                        found.push((
                            i,
                            Money {
                                amount: value,
                                currency: code,
                            },
                        ));
                    }
                }
                i += used;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// What was paid, out of all the amounts in a receipt: the total if it says one, otherwise
/// what it says was charged or paid, otherwise the first amount.
fn price_in(text: &str) -> Option<Money> {
    let amounts = amounts_in(text);
    // The words just before an amount, which say what it is.
    let before = |at: usize| -> String {
        let mut from = at.saturating_sub(60);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        text[from..at].to_lowercase()
    };
    let total = amounts.iter().rev().find(|(at, _)| {
        let words = before(*at);
        words
            .rfind("total")
            .is_some_and(|t| !words[..t].ends_with("sub"))
    });
    if let Some((_, money)) = total {
        return Some(*money);
    }
    const PAID: &[&str] = &[
        "charged",
        "amount paid",
        "you paid",
        "amount due",
        "paid",
        "amount",
        "renew",
        "price",
    ];
    amounts
        .iter()
        .find(|(at, _)| has(&before(*at), PAID))
        .or_else(|| amounts.first())
        .map(|(_, m)| *m)
}

/// How often, if the email says.
fn period_in(text: &str) -> Option<Period> {
    let lower = text.to_lowercase();
    let year = has(
        &lower,
        &[
            "/year",
            "/yr",
            "per year",
            "a year",
            "annual",
            "yearly",
            "every year",
            "12 months",
            "1 year",
            "one year",
        ],
    );
    let month = has(
        &lower,
        &[
            "/month",
            "/mo",
            "per month",
            "a month",
            "monthly",
            "every month",
            "1 month",
            "one month",
        ],
    );
    let week = has(
        &lower,
        &["/week", "per week", "weekly", "every week", "(7 days)"],
    );
    let quarter = has(&lower, &["quarterly", "every 3 months", "3 months"]);
    match (year, month, quarter, week) {
        (true, false, _, _) => Some(Period::Year),
        (false, true, false, _) => Some(Period::Month),
        (false, false, true, _) => Some(Period::Quarter),
        (false, false, false, true) => Some(Period::Week),
        // Both is often "billed annually, $8/month", which is yearly.
        (true, true, _, _) => Some(Period::Year),
        _ => None,
    }
}

/// How often, from the days between payments, if they're regular.
fn period_from_gaps(paid: &[i64]) -> Option<Period> {
    if paid.len() < 2 {
        return None;
    }
    let mut gaps: Vec<i64> = paid.windows(2).map(|w| (w[1] - w[0]) / DAY).collect();
    gaps.sort_unstable();
    let middle = gaps[gaps.len() / 2];
    match middle {
        5..=9 => Some(Period::Week),
        25..=35 => Some(Period::Month),
        80..=100 => Some(Period::Quarter),
        170..=200 => Some(Period::HalfYear),
        330..=400 => Some(Period::Year),
        _ => None,
    }
}

/// How often, from how long one paid period lasts.
fn period_of_days(days: i64) -> Option<Period> {
    match days {
        6..=8 => Some(Period::Week),
        27..=32 => Some(Period::Month),
        85..=95 => Some(Period::Quarter),
        175..=190 => Some(Period::HalfYear),
        358..=372 => Some(Period::Year),
        _ => None,
    }
}

/// The period a receipt is for: "Oct 25–Nov 25, 2026", "Sep 16, 2026 – Sep 16, 2027" or
/// "Start: Oct 16, 2026 End: Nov 16, 2026". Its start and end, when they're a whole period.
fn range_in(text: &str, sent: i64) -> Option<(i64, i64)> {
    let spaced = text.replace(['–', '—'], " – ");
    let dates = dates_in(&spaced, sent);
    dates.windows(2).find_map(|pair| {
        let ((at, start), (next_at, end)) = (pair[0], pair[1]);
        let close = next_at > at && next_at - at < 40;
        (close && end > start && period_of_days((end - start) / DAY).is_some())
            .then_some((start, end))
    })
}

const MONTHS: &[&str] = &[
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_of(word: &str) -> Option<u32> {
    let w = word.to_lowercase();
    let w = w.trim_end_matches('.');
    if w.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| w.starts_with(m) && (w.len() == 3 || "january february march april may june july august september october november december sept".contains(w)))
        .map(|i| i as u32 + 1)
}

/// Every date written in some text, in the ways receipts write them: "May 3, 2027",
/// "3 May 2027", "May 3" (the next one after `sent`) and "2027-05-03".
fn dates_in(text: &str, sent: i64) -> Vec<(usize, i64)> {
    let sent_day = chrono::DateTime::from_timestamp(sent, 0).map(|d| d.date_naive());
    let words: Vec<(usize, &str)> = text
        .split_inclusive(|c: char| c.is_whitespace())
        .scan(0usize, |at, w| {
            let start = *at;
            *at += w.len();
            Some((start, w.trim()))
        })
        .filter(|(_, w)| !w.is_empty())
        .collect();
    let clean = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    let day_of = |w: &str| -> Option<u32> {
        let w = clean(w);
        let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        let n: u32 = digits.parse().ok()?;
        (1..=31).contains(&n).then_some(n)
    };
    let year_of = |w: &str| -> Option<i32> {
        let n: i32 = clean(w).parse().ok()?;
        (2000..=2100).contains(&n).then_some(n)
    };
    let make = |y: Option<i32>, m: u32, d: u32| -> Option<i64> {
        let year = match y {
            Some(y) => y,
            None => {
                // No year: the next time that day comes after the email was sent.
                let sent = sent_day?;
                let this = NaiveDate::from_ymd_opt(sent.year(), m, d)?;
                if this >= sent {
                    sent.year()
                } else {
                    sent.year() + 1
                }
            }
        };
        NaiveDate::from_ymd_opt(year, m, d)?
            .and_hms_opt(12, 0, 0)
            .map(|t| t.and_utc().timestamp())
    };
    let mut found = Vec::new();
    for i in 0..words.len() {
        let (at, w) = words[i];
        // 2027-05-03
        let iso = clean(w);
        let parts: Vec<&str> = iso.split('-').collect();
        if parts.len() == 3 && parts[0].len() == 4 {
            if let (Ok(y), Ok(m), Ok(d)) = (parts[0].parse(), parts[1].parse(), parts[2].parse()) {
                if let Some(t) = make(Some(y), m, d) {
                    found.push((at, t));
                    continue;
                }
            }
        }
        let next = words.get(i + 1).map(|(_, w)| *w);
        let after = words.get(i + 2).map(|(_, w)| *w);
        // May 3, 2027 / May 3
        if let Some(m) = month_of(&clean(w)) {
            if let Some(d) = next.and_then(day_of) {
                let y = after.and_then(year_of);
                if let Some(t) = make(y, m, d) {
                    found.push((at, t));
                }
            }
            continue;
        }
        // 3 May 2027 / 3rd of May
        if let Some(d) = day_of(w) {
            let (month_word, year_word) = match next.map(clean).as_deref() {
                Some("of") => (after, words.get(i + 3).map(|(_, w)| *w)),
                _ => (next, after),
            };
            if let Some(m) = month_word.map(clean).and_then(|w| month_of(&w)) {
                let y = year_word.and_then(year_of);
                if let Some(t) = make(y, m, d) {
                    found.push((at, t));
                }
            }
        }
    }
    found
}

/// The first date after some words, such as when a reminder says it renews.
fn date_after(text: &str, sent: i64, words: &[&str]) -> Option<i64> {
    let lower = text.to_lowercase();
    let dates = dates_in(text, sent);
    let marks: Vec<usize> = words
        .iter()
        .flat_map(|w| lower.match_indices(w).map(|(i, _)| i))
        .collect();
    dates
        .iter()
        .filter(|(at, when)| *when >= sent - DAY && marks.iter().any(|m| m <= at && at - m < 120))
        .map(|(_, when)| *when)
        .next()
        .or_else(|| dates.iter().map(|(_, w)| *w).find(|w| *w > sent))
}

/// What one email adds to the business it came from.
#[derive(Debug, Clone)]
struct Seen {
    kind: Kind,
    date: i64,
    price: Option<Money>,
    period: Option<Period>,
    /// A date in it that matters: when it renews, or when a cancelled one ends.
    when: Option<i64>,
    repeats: bool,
    /// Its subject names a subscription, a renewal or a membership.
    says_so: bool,
}

fn read(mail: &Mail) -> Option<Seen> {
    let kind = kind_of(&mail.subject, &mail.text, mail.marketing)?;
    let everything = format!("{}\n{}", mail.subject, mail.text);
    let lower = everything.to_lowercase();
    let when = match kind {
        Kind::ComingUp => date_after(
            &everything,
            mail.date,
            &[
                "renew",
                "charged",
                "billed",
                "due",
                "on ",
                "trial ends",
                "payment",
                "starting",
            ],
        ),
        Kind::Cancelled => date_after(
            &everything,
            mail.date,
            &[
                "until", "through", "ends on", "end on", "expires", "access", "end of",
            ],
        )
        .filter(|w| *w > mail.date + DAY),
        // A receipt for a period, "Oct 25–Nov 25, 2026": it renews when that ends.
        Kind::Paid if range_in(&everything, mail.date).is_some() => {
            range_in(&everything, mail.date).map(|(_, end)| end)
        }
        // A receipt that says when it renews next, as Apple's and many others do.
        Kind::Paid
            if has(
                &lower,
                &["renews", "next billing", "next payment", "next charge"],
            ) =>
        {
            date_after(
                &everything,
                mail.date,
                &["renews", "next billing", "next payment", "next charge"],
            )
            .filter(|w| *w > mail.date + DAY)
        }
        _ => None,
    };
    let subject = mail.subject.to_lowercase();
    Some(Seen {
        kind,
        date: mail.date,
        price: price_in(&everything),
        period: range_in(&everything, mail.date)
            .and_then(|(start, end)| period_of_days((end - start) / DAY))
            .or_else(|| period_in(&everything)),
        when,
        // A receipt for a whole period says it repeats as plainly as any word.
        repeats: has(&lower, REPEATS)
            || (kind == Kind::Paid && range_in(&everything, mail.date).is_some()),
        says_so: has(&subject, &["subscription", "renew", "membership"]),
    })
}

/// How app stores write how often: "X Premium (Monthly)".
const STORE_PERIODS: &[&str] = &[
    "(monthly)",
    "(yearly)",
    "(annual)",
    "(annually)",
    "(weekly)",
    "(1 month)",
    "(1 year)",
    "(3 months)",
    "(6 months)",
    "(7 days)",
];

/// One app-store receipt can be for several subscriptions, each with its own price and renewal
/// date, so each becomes its own email here, named after what it's for. Other mail stays as it is.
fn store_items(mail: &Mail) -> Vec<(Mail, Option<String>)> {
    let host = mail
        .from_addr
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let store = matches!(company_domain(&host).as_str(), "apple.com" | "google.com");
    let lower = mail.text.to_lowercase();
    let marks: Vec<(usize, usize)> = if store {
        STORE_PERIODS
            .iter()
            .flat_map(|p| lower.match_indices(p).map(|(i, w)| (i, i + w.len())))
            .collect()
    } else {
        Vec::new()
    };
    let mut marks = marks;
    marks.sort_unstable();
    let mut out: Vec<(Mail, Option<String>)> = Vec::new();
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut last_end = 0;
    for (start, end) in marks {
        // The name is what's written just before, on the same line.
        let before = &mail.text[last_end.min(start)..start];
        let name = before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_end_matches(|c: char| c == '(' || c.is_whitespace())
            .to_string();
        last_end = end;
        // "X Premium (Monthly) (Monthly)" names the same thing twice.
        if name.len() < 2 || name.len() > 80 {
            continue;
        }
        found.push((name, start));
    }
    for (i, (name, start)) in found.iter().enumerate() {
        let until = found.get(i + 1).map_or(mail.text.len(), |(_, s)| *s);
        let mut part = mail.clone();
        part.text = mail.text[*start..until].to_string();
        out.push((part, Some(name.clone())));
    }
    if out.is_empty() {
        out.push((mail.clone(), None));
    }
    out
}

/// Works subscriptions out from a set of emails, as of `now`.
pub(crate) fn work_out(mail: &[Mail], now: i64) -> Vec<Subscription> {
    struct Group {
        name_votes: HashMap<String, usize>,
        account_id: i64,
        addresses: Vec<String>,
        seen: Vec<Seen>,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let items: Vec<(Mail, Option<String>)> = mail.iter().flat_map(store_items).collect();
    for (m, item) in &items {
        let Some(seen) = read(m) else { continue };
        let (business, name) = match item {
            Some(item) => {
                let host = m
                    .from_addr
                    .rsplit('@')
                    .next()
                    .unwrap_or_default()
                    .to_lowercase();
                (
                    format!("{}:{}", company_domain(&host), item.to_lowercase()),
                    item.clone(),
                )
            }
            None => business_of(&m.from_name, &m.from_addr, &m.subject),
        };
        let key = format!("{}:{business}", m.account_id);
        let g = groups.entry(key).or_insert_with(|| Group {
            name_votes: HashMap::new(),
            account_id: m.account_id,
            addresses: Vec::new(),
            seen: Vec::new(),
        });
        *g.name_votes.entry(name).or_default() += 1;
        let address = m.from_addr.to_lowercase();
        if !g.addresses.contains(&address) {
            g.addresses.push(address);
        }
        g.seen.push(seen);
    }
    let mut out = Vec::new();
    for (key, mut g) in groups {
        g.seen.sort_by_key(|s| s.date);
        // Payments, with a receipt and an invoice for the same one counted once.
        let mut paid: Vec<&Seen> = Vec::new();
        for s in g.seen.iter().filter(|s| s.kind == Kind::Paid) {
            match paid.last_mut() {
                Some(last) if s.date - last.date < SAME_PAYMENT_DAYS * DAY => *last = s,
                _ => paid.push(s),
            }
        }
        let paid_dates: Vec<i64> = paid.iter().map(|s| s.date).collect();
        let repeats = g.seen.iter().any(|s| s.repeats);
        let regular = period_from_gaps(&paid_dates);
        let reminder = g
            .seen
            .iter()
            .rev()
            .find(|s| s.kind == Kind::ComingUp && s.price.is_some());
        // Something has to say it repeats: payments at regular times alone are often a
        // card bill or a transfer, which also come every month.
        let says_so = g.seen.iter().any(|s| s.says_so);
        // Only a bill, or a reminder of one, says there's a subscription; a cancellation or a
        // failed payment on its own is about one already known.
        if !(repeats && (!paid.is_empty() || reminder.is_some())) {
            continue;
        }
        // A subscription costs about the same each time. Payments that jump around are sales,
        // orders or bills, unless the emails plainly say it's a subscription.
        let recent: Vec<f64> = paid
            .iter()
            .rev()
            .take(4)
            .filter_map(|s| s.price)
            .map(|m| m.amount)
            .collect();
        let steady = recent.len() < 2 || {
            let low = recent.iter().cloned().fold(f64::MAX, f64::min);
            let high = recent.iter().cloned().fold(0.0, f64::max);
            high <= low * 1.6
        };
        if !steady && !says_so {
            continue;
        }
        let latest_price = paid
            .iter()
            .rev()
            .find_map(|s| s.price)
            .or(reminder.and_then(|s| s.price));
        let said_period = g
            .seen
            .iter()
            .rev()
            .filter(|s| matches!(s.kind, Kind::Paid | Kind::ComingUp))
            .find_map(|s| s.period);
        // Hardly anything is billed weekly, so a week is taken only when an email says so.
        let regular = regular.filter(|p| *p != Period::Week || said_period == Some(Period::Week));
        // What the bills say beats the days between them: two plans from one company, billed
        // on different days of the month, would look weekly.
        let period = said_period.or(regular);
        let last_paid = paid.last().map(|s| s.date);
        // A reminder newer than the last payment says when the next one is.
        let reminded = g
            .seen
            .iter()
            .rev()
            .find(|s| s.kind == Kind::ComingUp && s.date >= last_paid.unwrap_or(0))
            .and_then(|s| s.when);
        let receipt_says = paid.last().and_then(|s| s.when);
        let reminded = reminded.or(receipt_says);
        let mut next = reminded.or_else(|| last_paid.zip(period).map(|(at, p)| p.after(at)));
        let after_paid = |kind: Kind| {
            g.seen
                .iter()
                .rev()
                .find(|s| s.kind == kind && s.date > last_paid.unwrap_or(0))
        };
        let mut ends = None;
        let status = if let Some(cancel) = after_paid(Kind::Cancelled) {
            let end = cancel.when.or(next).filter(|e| *e > cancel.date);
            match end {
                Some(e) if e > now => {
                    ends = Some(e);
                    Status::Ending
                }
                _ => Status::Cancelled,
            }
        } else if after_paid(Kind::Failed).is_some() {
            Status::Failed
        } else {
            // Just missed is worth asking about; missed for longer than a whole period, it stopped.
            let (grace, length) = period.map_or((30, 30), |p| (p.grace_days(), p.days()));
            match next {
                Some(n) if now > n + (grace + length) * DAY => Status::Stopped,
                Some(n) if now > n + grace * DAY => Status::Unsure,
                _ => Status::Active,
            }
        };
        if matches!(
            status,
            Status::Cancelled | Status::Unsure | Status::Ending | Status::Stopped
        ) {
            next = None;
        }
        let name = g
            .name_votes
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(n, _)| n)
            .unwrap_or_default();
        out.push(Subscription {
            key,
            name,
            account_id: Some(g.account_id),
            addresses: g.addresses,
            amount: latest_price.map(|m| m.amount),
            currency: latest_price.map(|m| m.currency.to_string()),
            period,
            period_sure: regular.is_some() || said_period.is_some(),
            last_paid,
            next,
            next_sure: reminded.is_some(),
            status,
            ends,
            payments: paid.len(),
            manual: false,
            edited: false,
        });
    }
    out
}

/// The person's changes laid over what the emails say.
fn apply(sub: &mut Subscription, edit: &Edit, now: i64) {
    sub.edited = true;
    if let Some(name) = edit.name.as_ref().filter(|n| !n.trim().is_empty()) {
        sub.name = name.trim().to_string();
    }
    if let Some(amount) = edit.amount {
        sub.amount = Some(amount);
    }
    if let Some(currency) = &edit.currency {
        sub.currency = Some(currency.clone());
    }
    if let Some(period) = edit.period {
        if sub.period != Some(period) {
            sub.period = Some(period);
            if !sub.next_sure {
                sub.next = sub.last_paid.map(|at| period.after(at));
            }
        }
        sub.period_sure = true;
    }
    if let Some(next) = edit.next {
        sub.next = Some(next);
        sub.next_sure = true;
    }
    match edit.status {
        Some(Status::Cancelled) => {
            sub.status = Status::Cancelled;
            sub.next = None;
            sub.ends = None;
        }
        Some(Status::Active) => {
            sub.status = Status::Active;
            sub.ends = None;
            if sub.next.is_none() {
                sub.next = sub.last_paid.zip(sub.period).map(|(at, p)| p.after(at));
            }
        }
        _ => {}
    }
    // Kept moving to the next one still to come, for a plan the person said is still on.
    if sub.status == Status::Active {
        if let (Some(mut n), Some(p)) = (sub.next, sub.period) {
            while n + p.grace_days() * DAY < now && edit.status == Some(Status::Active) {
                n = p.after(n);
            }
            sub.next = Some(n);
        }
    }
}

fn from_manual(m: &Manual, now: i64) -> Subscription {
    let mut next = m.next;
    if let Some(mut n) = next {
        while n < now - DAY {
            n = m.period.after(n);
        }
        next = Some(n);
    }
    Subscription {
        key: m.key.clone(),
        name: m.name.clone(),
        account_id: None,
        addresses: Vec::new(),
        amount: Some(m.amount),
        currency: Some(m.currency.clone()),
        period: Some(m.period),
        period_sure: true,
        last_paid: None,
        next,
        next_sure: next.is_some(),
        status: Status::Active,
        ends: None,
        payments: 0,
        manual: true,
        edited: false,
    }
}

/// How many emails' words are kept between visits to the page.
const READ_CACHE_MOST: usize = 20_000;

/// What each email says, worked out once. Keyed by the email itself (its Message-ID, sender,
/// date and subject), so it's right for every copy of it, wherever it's stored.
fn read_cache() -> &'static std::sync::Mutex<HashMap<String, std::sync::Arc<str>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, std::sync::Arc<str>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Whether an email's subject could make it count, so only those have their body read.
/// Every kind `kind_of` finds is named in the subject.
fn worth_reading(subject: &str) -> bool {
    let subject = subject.to_lowercase();
    if has(&subject, NEVER_SUBJECT) || has(&subject, SELLING) {
        return false;
    }
    has(&subject, BILLING_SUBJECT)
        || has(&subject, FAILED)
        || has(&subject, CANCELLED)
        || cancel_in_subject(&subject)
        || has(&subject, COMING_UP)
        || has(
            &subject,
            &["subscription is confirmed", "subscription confirmed"],
        )
}

impl Store {
    /// Receipts, renewal reminders, cancellations and failed payments from the last while,
    /// oldest first, each email once however many folders it's in.
    ///
    /// Only emails whose subject could make them count are read further, and what each one says
    /// is kept for next time, as working out the words in a long HTML receipt is the slow part.
    pub(crate) fn subscription_mail(&self, since: i64) -> Result<Vec<Mail>> {
        struct Head {
            id: i64,
            key: String,
            account_id: i64,
            from_name: String,
            from_addr: String,
            subject: String,
            date: i64,
            snippet: String,
            marketing: bool,
        }
        let heads: Vec<Head> = {
            let db = self.reading_db();
            let mut st = db.prepare(
                "SELECT m.id, COALESCE(m.rfc_id, '#' || m.id), m.account_id, m.from_name,
                        m.from_addr, m.subject, m.date, m.snippet,
                        m.unsubscribe IS NOT NULL OR m.category IN ('promotions', 'newsletters')
                 FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 WHERE m.date > ?1 AND m.is_draft = 0
                   AND f.role NOT IN ('trash', 'junk', 'drafts', 'sent')
                   AND (m.purchase = 1
                        OR m.subject LIKE '%renew%' OR m.subject LIKE '%subscription%'
                        OR m.subject LIKE '%membership%' OR m.subject LIKE '%cancel%'
                        OR m.subject LIKE '%trial%' OR m.subject LIKE '%payment%'
                        OR m.subject LIKE '%your plan%' OR m.subject LIKE '%receipt%'
                        OR m.subject LIKE '%invoice%' OR m.subject LIKE '%billing%'
                        OR m.subject LIKE '%unsuccessful%' OR m.subject LIKE '%ended%')
                 ORDER BY m.date",
            )?;
            let rows = st.query_map(params![since], |r| {
                Ok(Head {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    account_id: r.get(2)?,
                    from_name: r.get(3)?,
                    from_addr: r.get(4)?,
                    subject: r.get(5)?,
                    date: r.get(6)?,
                    snippet: r.get(7)?,
                    marketing: r.get(8)?,
                })
            })?;
            let mut once = HashSet::new();
            rows.filter_map(|row| row.ok())
                .filter(|h| once.insert((h.account_id, h.key.clone())))
                .filter(|h| worth_reading(&h.subject))
                .collect()
        };
        // Each email's words, from what was worked out before or from its body now.
        let cache = read_cache();
        let known = |h: &Head| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                h.key, h.from_addr, h.date, h.subject
            )
        };
        let missing: Vec<i64> = {
            let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
            heads
                .iter()
                .filter(|h| !cache.contains_key(&known(h)))
                .map(|h| h.id)
                .collect()
        };
        let mut bodies: HashMap<i64, String> = HashMap::new();
        for chunk in missing.chunks(500) {
            let db = self.reading_db();
            let marks = vec!["?"; chunk.len()].join(",");
            let mut st = db.prepare(&format!(
                "SELECT message_id, text, html FROM bodies WHERE message_id IN ({marks})"
            ))?;
            let rows = st.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (id, text, html) = row?;
                let body = match (text.filter(|t| !t.trim().is_empty()), html) {
                    (Some(t), _) => t,
                    (None, Some(h)) => crate::visible::visible_text(&h),
                    (None, None) => continue,
                };
                bodies.insert(id, body.chars().take(READ_CHARS).collect());
            }
        }
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > READ_CACHE_MOST {
            cache.clear();
        }
        let mut out = Vec::with_capacity(heads.len());
        for h in heads {
            let key = known(&h);
            let text = match cache.get(&key) {
                Some(text) => text.to_string(),
                None => {
                    // A body not downloaded yet isn't kept, so it's read once it arrives.
                    match bodies.remove(&h.id) {
                        Some(body) => {
                            cache.insert(key, body.as_str().into());
                            body
                        }
                        None => h.snippet.clone(),
                    }
                }
            };
            out.push(Mail {
                account_id: h.account_id,
                from_name: h.from_name,
                from_addr: h.from_addr,
                subject: h.subject,
                date: h.date,
                text,
                marketing: h.marketing,
            });
        }
        Ok(out)
    }

    fn saved_subscriptions(&self) -> Result<Saved> {
        Ok(self
            .setting(SETTING)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
    }

    fn save_subscriptions(&self, saved: &Saved) -> Result<()> {
        let text = serde_json::to_string(saved).map_err(|e| CoreError::Invalid(e.to_string()))?;
        self.set_setting(SETTING, &text)
    }

    /// Every subscription, with the person's changes, soonest renewal first.
    pub fn subscriptions(&self, now: i64) -> Result<Subscriptions> {
        let mail = self.subscription_mail(now - LOOK_BACK_DAYS * DAY)?;
        let saved = self.saved_subscriptions()?;
        let mut items = Vec::new();
        let mut hidden = Vec::new();
        for mut sub in work_out(&mail, now) {
            match saved.edits.get(&sub.key) {
                Some(edit) if edit.hidden => {
                    sub.edited = true;
                    hidden.push(sub);
                }
                Some(edit) => {
                    apply(&mut sub, edit, now);
                    items.push(sub);
                }
                None => items.push(sub),
            }
        }
        for m in &saved.manual {
            let mut sub = from_manual(m, now);
            if let Some(edit) = saved.edits.get(&m.key) {
                apply(&mut sub, edit, now);
                sub.manual = true;
            }
            items.push(sub);
        }
        let rank = |s: &Subscription| match s.status {
            Status::Failed => 0,
            Status::Active | Status::Ending => 1,
            Status::Unsure => 2,
            Status::Cancelled | Status::Stopped => 3,
        };
        items.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then(
                    a.next
                        .or(a.ends)
                        .unwrap_or(i64::MAX)
                        .cmp(&b.next.or(b.ends).unwrap_or(i64::MAX)),
                )
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        hidden.sort_by_key(|s| s.name.to_lowercase());
        Ok(Subscriptions { items, hidden })
    }

    /// Saves what the person changed about one, on top of what they changed before.
    pub fn edit_subscription(&self, key: &str, change: Change) -> Result<()> {
        let mut saved = self.saved_subscriptions()?;
        let mut edit = saved.edits.remove(key).unwrap_or_default();
        if change.forget {
            edit = Edit::default();
        }
        if let Some(name) = change.name {
            edit.name = Some(name);
        }
        if let Some(amount) = change.amount.filter(|a| a.is_finite() && *a > 0.0) {
            edit.amount = Some(amount);
        }
        if let Some(currency) = change.currency {
            edit.currency = Some(currency.trim().to_uppercase());
        }
        if change.period.is_some() {
            edit.period = change.period;
        }
        if change.next.is_some() {
            edit.next = change.next;
        }
        if let Some(status) = change.status {
            edit.status = Some(status);
            // Said again, so an old date from before doesn't hold it back.
            if status == Status::Cancelled {
                edit.next = None;
            }
        }
        if let Some(hidden) = change.hidden {
            edit.hidden = hidden;
        }
        if edit != Edit::default() {
            saved.edits.insert(key.to_string(), edit);
        }
        self.save_subscriptions(&saved)
    }

    /// Adds one billed without any email, such as a gym or a phone plan.
    pub fn add_subscription(
        &self,
        name: &str,
        amount: f64,
        currency: &str,
        period: Period,
        next: Option<i64>,
    ) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || !(amount.is_finite() && amount > 0.0) {
            return Err(CoreError::Invalid("Add a name and a price.".into()));
        }
        let mut saved = self.saved_subscriptions()?;
        let key = format!("manual:{:016x}", rand::random::<u64>());
        saved.manual.push(Manual {
            key: key.clone(),
            name: name.to_string(),
            amount,
            currency: currency.trim().to_uppercase(),
            period,
            next,
        });
        self.save_subscriptions(&saved)?;
        Ok(key)
    }

    /// Takes away one the person added by hand.
    pub fn remove_subscription(&self, key: &str) -> Result<()> {
        let mut saved = self.saved_subscriptions()?;
        saved.manual.retain(|m| m.key != key);
        saved.edits.remove(key);
        self.save_subscriptions(&saved)
    }
}

impl Engine {
    pub(crate) fn subscriptions(&self) -> Result<Subscriptions> {
        self.store.subscriptions(chrono::Utc::now().timestamp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_000_000; // 2026-10-26

    fn at(y: i32, m: u32, d: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn mail(from: &str, subject: &str, text: &str, date: i64) -> Mail {
        let (name, addr) = from
            .split_once('<')
            .map_or(("", from), |(n, a)| (n.trim(), a.trim_end_matches('>')));
        Mail {
            account_id: 1,
            from_name: name.to_string(),
            from_addr: addr.to_string(),
            subject: subject.to_string(),
            date,
            text: text.to_string(),
            marketing: false,
        }
    }

    #[test]
    fn only_likely_mail_is_read_and_its_words_are_kept() {
        use crate::model::{Addr, Security};
        use crate::parse::ParsedHeaders;
        use crate::store::IncomingMessage;
        let s = Store::open_in_memory().unwrap();
        let a = s
            .insert_account("a@x.test", "A", "x.test", 993, Security::Tls, "a")
            .unwrap();
        let f = s
            .replace_folders(a.id, &[("INBOX".into(), "Inbox".into(), "inbox".into())])
            .unwrap()[0]
            .0;
        let one = |uid: u32, subject: &str, date: i64| IncomingMessage {
            uid,
            is_draft: false,
            seen: true,
            flagged: false,
            size: 10,
            internal_date: Some(date),
            signals: None,
            headers: ParsedHeaders {
                rfc_id: Some(format!("m{uid}@x.test")),
                subject: subject.into(),
                from: Some(Addr {
                    name: "Drawly".into(),
                    address: "billing@drawly.test".into(),
                }),
                date: Some(date),
                ..Default::default()
            },
        };
        s.insert_messages(
            a.id,
            f,
            &[
                one(1, "Your Drawly receipt", at(2026, 9, 5)),
                one(2, "Your Drawly receipt", at(2026, 10, 5)),
                one(3, "Drawly payment tips for your team", at(2026, 10, 9)),
            ],
        )
        .unwrap();
        let body = |uid: i64, html: &str| {
            s.db()
                .execute(
                    "INSERT INTO bodies (message_id, text, html, attachments_json)
                     SELECT id, NULL, ?2, '[]' FROM messages WHERE uid = ?1",
                    params![uid, html],
                )
                .unwrap();
        };
        body(1, "<p>Pro plan, billed monthly.</p><p>Total $12.00</p>");
        body(2, "<p>Pro plan, billed monthly.</p><p>Total $12.00</p>");
        body(3, "<p>Pro plan, billed monthly.</p><p>Total $99.00</p>");
        let mail = s.subscription_mail(0).unwrap();
        assert_eq!(mail.len(), 2, "tips don't need their body read");
        assert!(mail[0].text.contains("Total $12.00"), "{}", mail[0].text);
        // Kept: the same words come back even with the bodies gone.
        s.db().execute("DELETE FROM bodies", []).unwrap();
        let again = s.subscription_mail(0).unwrap();
        assert!(again[1].text.contains("Total $12.00"));
        let subs = s.subscriptions(at(2026, 10, 20)).unwrap();
        assert_eq!(subs.items.len(), 1);
        assert_eq!(subs.items[0].amount, Some(12.0));
    }

    #[test]
    fn money_is_read_the_ways_receipts_write_it() {
        let one = |t: &str| price_in(t).map(|m| (m.amount, m.currency));
        assert_eq!(one("Total: $12.99"), Some((12.99, "USD")));
        assert_eq!(
            one("Subtotal $10.00 Tax $2.00 Total $12.00"),
            Some((12.0, "USD"))
        );
        assert_eq!(one("Amount paid €9,99"), Some((9.99, "EUR")));
        assert_eq!(
            one("You paid PKR 4,500 for your plan"),
            Some((4500.0, "PKR"))
        );
        assert_eq!(
            one("Charged 1.234,56 EUR to your card"),
            Some((1234.56, "EUR"))
        );
        assert_eq!(one("Your plan renews for US$ 144.00"), Some((144.0, "USD")));
        assert_eq!(one("৳500 for Netflix"), Some((500.0, "BDT")));
        assert_eq!(one("Users: 3, nothing to pay"), None);
        assert_eq!(one("No money here, 2026"), None);
    }

    #[test]
    fn dates_are_read_the_ways_reminders_write_them() {
        let sent = at(2026, 10, 1);
        let first = |t: &str| dates_in(t, sent).first().map(|(_, w)| *w);
        let day = |y, m, d| {
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        };
        assert_eq!(first("renews on November 3, 2026."), Some(day(2026, 11, 3)));
        assert_eq!(first("renews on 3 Nov 2026"), Some(day(2026, 11, 3)));
        assert_eq!(first("renews on 2026-11-03"), Some(day(2026, 11, 3)));
        assert_eq!(
            first("renews on Jan 5"),
            Some(day(2027, 1, 5)),
            "the next January"
        );
        assert_eq!(first("may we help?"), None);
    }

    #[test]
    fn monthly_receipts_become_one_subscription_with_its_next_date() {
        let figma = "Figma <billing@figma.com>";
        let receipts: Vec<Mail> = (7..=10)
            .map(|m| {
                mail(
                    figma,
                    "Your Figma receipt",
                    "Professional plan, monthly. Total $15.00",
                    at(2026, m, 5),
                )
            })
            .collect();
        let subs = work_out(&receipts, NOW);
        assert_eq!(subs.len(), 1);
        let s = &subs[0];
        assert_eq!(s.name, "Figma");
        assert_eq!(s.amount, Some(15.0));
        assert_eq!(s.currency.as_deref(), Some("USD"));
        assert_eq!(s.period, Some(Period::Month));
        assert_eq!(s.status, Status::Active);
        assert_eq!(s.next, Some(at(2026, 11, 5)));
        assert_eq!(s.payments, 4);
    }

    #[test]
    fn a_yearly_plan_shows_from_a_single_receipt() {
        let subs = work_out(
            &[mail(
                "Notion Team <team@makenotion.com>",
                "Your Notion receipt",
                "Plus plan, billed annually. Amount paid $96.00",
                at(2026, 3, 1),
            )],
            NOW,
        );
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "Notion");
        assert_eq!(subs[0].period, Some(Period::Year));
        assert_eq!(subs[0].next, Some(at(2027, 3, 1)));
    }

    #[test]
    fn stripe_receipts_are_told_apart_by_who_was_paid() {
        let mut mails = Vec::new();
        for m in 8..=10 {
            mails.push(mail(
                "Linear <invoice+statements@stripe.com>",
                "Your receipt from Linear #1",
                "Total $8.00 per month",
                at(2026, m, 2),
            ));
            mails.push(mail(
                "Stripe <receipts@stripe.com>",
                "Your receipt from Loom #22",
                "Total $12.50 per month",
                at(2026, m, 9),
            ));
        }
        let subs = work_out(&mails, NOW);
        let names: Vec<&str> = subs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(subs.len(), 2, "{names:?}");
        assert!(
            names.contains(&"Linear") && names.contains(&"Loom"),
            "{names:?}"
        );
    }

    #[test]
    fn one_off_orders_and_bank_alerts_are_not_subscriptions() {
        let mails = [
            mail(
                "Amazon <shipment-tracking@amazon.com>",
                "Your Amazon.com order has shipped",
                "Total $40.00",
                at(2026, 8, 1),
            ),
            mail(
                "Amazon <auto-confirm@amazon.com>",
                "Your order #112",
                "Total $22.00",
                at(2026, 9, 1),
            ),
            mail(
                "UCB <alerts@ucb.test>",
                "Transaction alert",
                "BDT 500 debited",
                at(2026, 9, 3),
            ),
            mail(
                "Gumroad <hello@gumroad.com>",
                "Thank you for your purchase",
                "Icon pack. Total $19",
                at(2026, 9, 9),
            ),
        ];
        assert!(work_out(&mails, NOW).is_empty());
    }

    #[test]
    fn bills_paid_transfers_and_money_coming_in_are_not_subscriptions() {
        let mut mails = Vec::new();
        for (i, m) in (6..=10).enumerate() {
            let amount = 1_000 + i * 937;
            mails.push(mail(
                "QuickBooks <quickbooks@notifications.intuit.com>",
                "Quickbooks Bill Payment Confirmation",
                &format!("Invoice Payment Sent Via: QuickBooks BillPay. Delivery ETA: Friday. Amount ${amount}.00, recurring vendor bill"),
                at(2026, m, 6),
            ));
            mails.push(mail(
                "CITYTOUCH <citytouch@thecitybank.com>",
                "You made a Credit Card Bill Payment",
                "Payment Recieved. You made a Credit Card payment of BDT 9,562.86 Cardmember Name. Monthly card bill.",
                at(2026, m, 3),
            ));
            mails.push(mail(
                "UCB <noreply@ucb.com.bd>",
                "Your Transaction Has Been Successful",
                "Your transaction of BDT 30,000.00 with Transaction ID IB01 has been successfully processed. Monthly",
                at(2026, m, 9),
            ));
            mails.push(mail(
                "Wise <noreply@wise.com>",
                "Money received from MAARKT",
                "You received $2,400.00 monthly retainer",
                at(2026, m, 1),
            ));
            mails.push(mail(
                "Namecheap Support <support@namecheap.com>",
                "Namecheap Order Summary (Order# 2187)",
                &format!("Domain renewal, 1 year. Total ${}.98", 10 + i * 30),
                at(2026, m, 5),
            ));
        }
        let subs = work_out(&mails, NOW);
        assert!(
            subs.is_empty(),
            "{:?}",
            subs.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn each_app_store_subscription_is_its_own() {
        let apple = "Apple <no_reply@email.apple.com>";
        let mails = [
            mail(
                apple,
                "Your receipt from Apple.",
                "Receipt\nOrder ID: MT581\nX\nX Premium (Monthly)\n(Monthly)\nRenews December 3, 2026\n$5.49\nSubtotal $5.49",
                at(2026, 11, 3),
            ),
            mail(
                apple,
                "Your receipt from Apple.",
                "Receipt\nOrder ID: MT580\nAha World\nAha SVIP Monthly Pass (Monthly)\nRenews November 24, 2026\n$9.99",
                at(2026, 10, 24),
            ),
        ];
        let subs = work_out(&mails, NOW);
        let mut found: Vec<(String, Option<f64>)> =
            subs.iter().map(|s| (s.name.clone(), s.amount)).collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            found,
            vec![
                ("Aha SVIP Monthly Pass".to_string(), Some(9.99)),
                ("X Premium".to_string(), Some(5.49))
            ]
        );
        let x = subs.iter().find(|s| s.name == "X Premium").unwrap();
        assert!(x.next_sure);
        assert_eq!(x.next.map(|n| n / DAY), Some(at(2026, 12, 3) / DAY));
    }

    #[test]
    fn sales_that_jump_around_are_not_a_subscription() {
        let mails: Vec<Mail> = [40.0, 129.0, 19.0, 300.0]
            .iter()
            .enumerate()
            .map(|(i, amount)| {
                mail(
                    "Polar <receipts@polar.sh>",
                    "Your receipt",
                    &format!("Monthly report. Total ${amount}"),
                    at(2026, 7 + i as u32, 2),
                )
            })
            .collect();
        assert!(work_out(&mails, NOW).is_empty());
    }

    /// The kinds of mail a real inbox has, as they're written, with names made up.
    #[test]
    fn a_real_inbox_shows_only_its_real_subscriptions() {
        let now = at(2026, 11, 15);
        let ad = |from: &str, subject: &str, text: &str, date: i64| Mail {
            marketing: true,
            ..mail(from, subject, text, date)
        };
        let mut mails = vec![
            // A plan billed through Stripe, on two days of the month, one later refunded.
            mail("Chirp <invoice+statements@stripe.com>", "Your receipt from Chirp #2538-2124", "Receipt from Chirp ৳600.00 Paid September 18, 2026 Sep 18–Oct 18, 2026 Chirp Premium (per Period) Qty 1 ৳600.00 Total ৳600.00", at(2026, 9, 18)),
            mail("Chirp <invoice+statements@stripe.com>", "Your receipt from Chirp #2557-7225", "Receipt from Chirp ৳600.00 Paid September 25, 2026 Sep 25–Oct 25, 2026 Chirp Premium (per Period) Qty 1 ৳600.00 Total ৳600.00", at(2026, 9, 25)),
            mail("Chirp <invoice+statements@stripe.com>", "Your receipt from Chirp #2105-2874", "Receipt from Chirp ৳600.00 Paid on October 18, 2026 Oct 18–Nov 18, 2026 Chirp Premium (per Period) Qty 1 ৳600.00 Total ৳600.00", at(2026, 10, 18)),
            mail("Chirp <invoice+statements@stripe.com>", "Your refund from Chirp #3730-3350", "Refund from Chirp ৳428.77 Oct 25–Nov 25, 2026 Chirp Premium (per Period)", at(2026, 11, 3)),
            // A monthly AI plan, and the pay-as-you-go credits from the same company.
            mail("Thinkco <invoice+statements@mail.thinkco.test>", "Your receipt from Thinkco #2677-6962", "Receipt from Thinkco $200.00 Paid on October 25, 2026 Oct 25–Nov 25, 2026 Max plan Qty 1 $200.00 Total $200.00", at(2026, 10, 25)),
            mail("Thinkco <invoice+statements@mail.thinkco.test>", "Your receipt from Thinkco #2619-7861", "Receipt from Thinkco $10.00 Auto-recharge credits Qty 1 $10.00 Total $10.00", at(2026, 11, 3)),
            // A yearly plan, bought the day after a one-off setup fee.
            mail("Formco <invoice+statements@stripe.com>", "Your receipt from Formco #2364-1272", "Receipt from Formco $99.00 Start - LLC Qty 1 $399.00 Total $99.00", at(2026, 9, 15)),
            mail("Formco <invoice+statements@stripe.com>", "Your receipt from Formco #2438-6621", "Receipt from Formco $299.00 Sep 16, 2026–Sep 16, 2027 Agent Autopilot Qty 1 $299.00", at(2026, 9, 16)),
            // A design tool, cancelled, paid until its period ends.
            mail("Drawly <support+notifications@drawly.test>", "Receipt for subscription payment Oct 16, 2026", "Thanks for your recurring Professional team (monthly) subscription. Start: Oct 16, 2026 End: Nov 16, 2026 Total: $100.00 USD", at(2026, 10, 16)),
            mail("Drawly <support+notifications@drawly.test>", "You've cancelled your Professional plan", "Your plan ends on Nov 16, 2026.", at(2026, 10, 30)),
            // An app store: one subscription and one one-off purchase.
            mail("Apple <no_reply@email.apple.com>", "Your receipt from Apple.", "Receipt\niCloud\niCloud+ with 50 GB (Monthly)\nRenews November 24, 2026\n$0.99", at(2026, 10, 25)),
            mail("Apple <no_reply@email.apple.com>", "Your receipt from Apple.", "Your Apple Account was used to purchase Baby Care. In-App Purchase TOTAL $9.53", at(2026, 9, 10)),
            // Reminders: a yearly plan and a domain that only renews if you do it.
            mail("Google Play <googleplay-noreply@google.com>", "Your subscription for Cloud One will renew on Nov 22, 2026", "Next payment BDT 8,100.00/year on Nov 22, 2026", at(2026, 11, 8)),
            mail("Domainco Renewals <renewals@domainco.test>", "example.com will expire in 7 days - renew now", "Renew for $14.58 a year", at(2026, 11, 10)),
            // A failed payment with nothing else from them.
            mail("Bitco (via Paddle.com) <help@paddle.com>", "Your Bitco Basic subscription payment failed", "We couldn't charge $39.00 per month", at(2026, 11, 2)),
            // Money in and out.
            mail("Wise <noreply@wise.com>", "Money received from Client", "You received 750 USD monthly", at(2026, 11, 1)),
            mail("Cardco <notifications@cardco.test>", "A charge was authorized", "$0.99 at APPLE.COM/BILL monthly", at(2026, 10, 24)),
            mail("Polar <notifications@polar.sh>", "You've made a new sale ($34.30)!", "Order Type One-time purchase", at(2026, 11, 4)),
        ];
        // Ads and newsletters, however much they talk about prices.
        mails.extend([
            ad(
                "City Bank | Card offers <offers@bank.test>",
                "Enjoy 15% savings with your card",
                "Pay BDT 50 a month",
                at(2026, 9, 6),
            ),
            ad(
                "StackSocial <deals@stacksocial.test>",
                "1min.AI: Lifetime Subscription (89% OFF)",
                "$55.00 a week",
                at(2026, 9, 16),
            ),
            ad(
                "StackSocial <deals@stacksocial.test>",
                "VPN Starter Plan: 2-Yr Subscription",
                "$2.99 per month",
                at(2026, 10, 2),
            ),
            ad(
                "Surfshark <hello@surfshark.test>",
                "Upgrade your plan for only $0.99/mo",
                "Deal expires on Sunday. $0.99/mo",
                at(2026, 10, 5),
            ),
            ad(
                "Indie Hackers <hello@indiehackers.test>",
                "How she got to $7.5k/mo",
                "$9.50 a month in revenue, monthly subscription business",
                at(2026, 8, 15),
            ),
        ]);
        let subs = work_out(&mails, now);
        let mut found: Vec<(String, Option<f64>, Option<Period>, Status)> = subs
            .iter()
            .map(|s| (s.name.clone(), s.amount, s.period, s.status))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            found,
            vec![
                (
                    "Chirp".to_string(),
                    Some(600.0),
                    Some(Period::Month),
                    Status::Active
                ),
                (
                    "Cloud One".to_string(),
                    Some(8100.0),
                    Some(Period::Year),
                    Status::Active
                ),
                (
                    "Drawly".to_string(),
                    Some(100.0),
                    Some(Period::Month),
                    Status::Ending
                ),
                (
                    "Formco".to_string(),
                    Some(299.0),
                    Some(Period::Year),
                    Status::Active
                ),
                (
                    "Thinkco".to_string(),
                    Some(200.0),
                    Some(Period::Month),
                    Status::Active
                ),
                (
                    "iCloud+ with 50 GB".to_string(),
                    Some(0.99),
                    Some(Period::Month),
                    Status::Active
                ),
            ]
        );
        let formco = subs.iter().find(|s| s.name == "Formco").unwrap();
        assert_eq!(
            formco.next.map(|n| n / DAY),
            Some(at(2027, 9, 16) / DAY),
            "renews when the paid year ends"
        );
    }

    #[test]
    fn a_cancellation_or_a_failed_payment_changes_its_status() {
        let spotify = "Spotify <no-reply@spotify.com>";
        let mut mails: Vec<Mail> = (7..=9)
            .map(|m| {
                mail(
                    spotify,
                    "Your Spotify Premium receipt",
                    "Premium Individual, monthly. Total $11.99",
                    at(2026, m, 12),
                )
            })
            .collect();
        let mut failed = mails.clone();
        failed.push(mail(
            spotify,
            "Your payment failed",
            "Update your card to keep Premium",
            at(2026, 10, 12),
        ));
        assert_eq!(work_out(&failed, NOW)[0].status, Status::Failed);

        mails.push(mail(
            spotify,
            "Your subscription has been cancelled",
            "You'll keep Premium until October 12, 2026.",
            at(2026, 9, 20),
        ));
        let s = &work_out(&mails, at(2026, 10, 1))[0];
        assert_eq!(s.status, Status::Ending);
        assert_eq!(s.ends.map(|e| e / DAY), Some(at(2026, 10, 12) / DAY));
        assert_eq!(
            work_out(&mails, NOW)[0].status,
            Status::Cancelled,
            "once the date has passed"
        );
    }

    #[test]
    fn a_missed_renewal_is_marked_unsure() {
        let mails: Vec<Mail> = (6..=9)
            .map(|m| {
                mail(
                    "Canva <billing@canva.com>",
                    "Your Canva Pro receipt",
                    "Canva Pro monthly. Total $14.99",
                    at(2026, m, 1),
                )
            })
            .collect();
        let s = &work_out(&mails, NOW)[0];
        assert_eq!(s.status, Status::Unsure);
        assert_eq!(s.next, None);
    }

    #[test]
    fn long_missed_ones_have_stopped_and_offers_are_left_out() {
        let apple = "Apple <no_reply@email.apple.com>";
        let mails = [
            mail(
                apple,
                "Your receipt from Apple.",
                "Receipt\nDessert Cafe\nDessert Cafe for Toddlers (Monthly)\nRenews August 5, 2026\n$21.19",
                at(2026, 7, 5),
            ),
            mail(
                "City Bank Card offers <offers@citybank.com.bd>",
                "Get up to BDT 20,000 cashback with your card",
                "Pay just BDT 50 a month",
                at(2026, 9, 6),
            ),
        ];
        let subs = work_out(&mails, NOW);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].status, Status::Stopped);
        assert_eq!(subs[0].next, None);
    }

    #[test]
    fn a_renewal_reminder_says_when_it_is_due() {
        let mails = [mail(
            "Namecheap <support@namecheap.com>",
            "Renewal reminder: framerbite.com",
            "Your domain will renew on November 20, 2026 for $14.58 a year.",
            at(2026, 10, 20),
        )];
        let s = &work_out(&mails, NOW)[0];
        assert_eq!(s.amount, Some(14.58));
        assert_eq!(s.period, Some(Period::Year));
        assert!(s.next_sure);
        assert_eq!(s.next.map(|n| n / DAY), Some(at(2026, 11, 20) / DAY));
    }

    #[test]
    fn the_persons_changes_win() {
        let mails: Vec<Mail> = (4..=7)
            .map(|m| {
                mail(
                    "Canva <billing@canva.com>",
                    "Your Canva Pro receipt",
                    "Total $14.99 monthly",
                    at(2026, m, 1),
                )
            })
            .collect();
        let mut s = work_out(&mails, NOW).remove(0);
        apply(
            &mut s,
            &Edit {
                status: Some(Status::Active),
                amount: Some(10.0),
                ..Edit::default()
            },
            NOW,
        );
        assert_eq!(s.status, Status::Active);
        assert_eq!(s.amount, Some(10.0));
        assert!(
            s.next.is_some_and(|n| n > NOW - 10 * DAY),
            "rolled on to the next one to come"
        );
    }
}
