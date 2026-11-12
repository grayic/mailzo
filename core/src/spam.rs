//! Spam protection and smart sorting, worked out on this device.
//!
//! The mail provider's own filter still comes first. This adds what it read from
//! the headers (sender checks and its own spam marks), scam tricks, and a filter
//! that learns from the person's choices. Nothing is ever deleted: suspect mail
//! moves to Junk with its reason, and people the person has written to are never
//! marked as spam.

use crate::actions::BulkThread;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::imap::ImapConnection;
use crate::model::{Account, Addr, NewMail};
use mail_auth::dkim::VerifySignature;
use mail_auth::{AuthenticatedMessage, DkimResult, MessageAuthenticator};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use unicode_security::confusable_detection::skeleton;

/// From this score an email counts as spam.
pub const SPAM_AT: i64 = 70;
/// From this score the filter is unsure, and a second opinion can help.
pub const UNSURE_AT: i64 = 35;
const BLOCKED_SCORE: i64 = 100;
/// The learning part needs this many examples of each kind before it's trusted.
const MIN_EXAMPLES: i64 = 20;
const MOST_WORDS_PER_EMAIL: usize = 500;
const WORDS_THAT_DECIDE: usize = 15;
const MOST_WORDS_SHARED: i64 = 1500;
const CHECKS_PER_SYNC: i64 = 500;
const TRAINING_PER_SYNC: i64 = 200;
const SIGNATURE_CHECKS_PER_SYNC: usize = 5;
const AI_OPINIONS_PER_SYNC: usize = 10;
const AI_SORTS_PER_SYNC: i64 = 20;
const AI_EXCERPT_CHARS: usize = 600;
const SIGNATURE_TIMEOUT: Duration = Duration::from_secs(10);

pub const BLOCKED: &str = "block:";
pub const TRUSTED: &str = "safe:";
pub const SORTED: &str = "sort:";
/// The learned words, as they travel in the vault.
pub const SHARED_WORDS: &str = "filter:words";
pub(crate) const TOTALS_WORD: &str = "\u{1}total";

const CATEGORIES: &[&str] = &["general", "newsletters", "promotions"];

/// Mass mail that still concerns the person's own dealings: money, accounts, access.
const OFFICIAL_WORDS: &[&str] = &[
    "invoice",
    "receipt",
    "payment",
    "paid",
    "billing",
    "statement",
    "refund",
    "your order",
    "order #",
    "order confirmation",
    "order number",
    "shipped",
    "shipping",
    "delivery",
    "delivered",
    "confirm",
    "confirmation",
    "verify",
    "verified",
    "verification",
    "password",
    "sign in",
    "sign-in",
    "signin",
    "log in",
    "login",
    "security",
    "secure",
    "your code",
    "passcode",
    "one-time",
    "otp",
    "2fa",
    "signing in",
    "invited you",
    "inviting you",
    "welcome to",
    "invitation",
    "invite",
    "waiting for you",
    "accept your",
    "action required",
    "your account",
    "your plan",
    "your subscription",
    "your limit",
    "renewal",
    "renew",
    "expire",
    "expiring",
    "contract",
    "agreement",
    "terms",
    "policy",
    "tax",
    "approved",
    "declined",
    "application",
    "ticket",
    "booking",
    "reservation",
    "appointment",
    "reminder",
    "feedback",
    "survey",
    "your voice",
    "hear from you",
];

/// Wording that means selling, which outranks the words above: "promo code" is no code.
const SELLING_WORDS: &[&str] = &[
    "% off",
    "promo code",
    "coupon",
    "discount",
    "black friday",
    "cyber monday",
    "flash sale",
    "sale ends",
    "last chance",
    "limited time",
    "special offer",
    "exclusive offer",
    "free trial",
    "shop now",
    "buy now",
    "save $",
    "deal",
];

const NEWSLETTER_WORDS: &[&str] = &[
    "newsletter",
    "digest",
    "weekly",
    "monthly",
    "this week",
    "this month",
    "edition",
    "issue #",
    "roundup",
    "round-up",
    "recap",
    "what's new",
    "changelog",
    "bulletin",
];
const NEWSLETTER_SENDERS: &[&str] = &[
    "news",
    "newsletter",
    "digest",
    "editor",
    "editors",
    "weekly",
];
const NEWSLETTER_SERVICES: &[&str] = &[
    "substack.com",
    "beehiiv.com",
    "ghost.io",
    "buttondown.email",
    "convertkit-mail.com",
    "kit-mail.com",
];

/// Whether the words appear as words, so "sign in" isn't found in "design in" or
/// "deal" in "ideal". Plain endings still count, such as "orders" and "confirmed".
fn mentions(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(at, _)| {
        let starts_clean = !phrase.starts_with(char::is_alphanumeric)
            || !text[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let rest = &text[at + phrase.len()..];
        let ending = &rest[..rest
            .find(|c: char| !c.is_alphanumeric())
            .unwrap_or(rest.len())];
        let ends_clean = !phrase.ends_with(char::is_alphanumeric)
            || matches!(ending, "" | "s" | "d" | "ed" | "ing");
        starts_clean && ends_clean
    })
}

/// Where a mass mailing belongs, from its sender and subject. Anything mass-mailed that
/// isn't about the person's own dealings, and isn't a publication, is someone selling.
pub fn sort_mass_mail(from_address: &str, subject: &str) -> &'static str {
    let subject = subject.to_lowercase();
    let address = from_address.to_lowercase();
    let (local, domain) = address.split_once('@').unwrap_or((address.as_str(), ""));
    let has = |words: &[&str]| words.iter().any(|w| mentions(&subject, w));
    if has(SELLING_WORDS) {
        "promotions"
    } else if has(OFFICIAL_WORDS) {
        "general"
    } else if has(NEWSLETTER_WORDS)
        || NEWSLETTER_SENDERS.contains(&local)
        || NEWSLETTER_SERVICES.iter().any(|d| is_within(domain, d))
    {
        "newsletters"
    } else {
        "promotions"
    }
}

/// Well-known names scammers pretend to be, and where they really send from.
const BRANDS: &[(&str, &[&str])] = &[
    ("paypal", &["paypal.com", "paypal.me"]),
    ("apple", &["apple.com", "icloud.com", "me.com"]),
    (
        "microsoft",
        &[
            "microsoft.com",
            "microsoftonline.com",
            "outlook.com",
            "office.com",
            "office365.com",
            "live.com",
            "hotmail.com",
            "onedrive.com",
        ],
    ),
    (
        "google",
        &["google.com", "gmail.com", "googlemail.com", "youtube.com"],
    ),
    ("amazon", &["amazon.com", "amazonaws.com", "aws.com"]),
    ("netflix", &["netflix.com"]),
    (
        "facebook",
        &["facebook.com", "facebookmail.com", "meta.com"],
    ),
    (
        "instagram",
        &["instagram.com", "facebookmail.com", "mail.instagram.com"],
    ),
    ("whatsapp", &["whatsapp.com"]),
    ("linkedin", &["linkedin.com"]),
    ("dhl", &["dhl.com", "dhl.de"]),
    ("fedex", &["fedex.com"]),
    ("ups", &["ups.com"]),
    ("usps", &["usps.com"]),
    ("stripe", &["stripe.com"]),
    ("dropbox", &["dropbox.com", "dropboxmail.com"]),
    ("docusign", &["docusign.com", "docusign.net"]),
    ("coinbase", &["coinbase.com"]),
    ("binance", &["binance.com"]),
    ("chase", &["chase.com", "jpmorgan.com"]),
    ("wellsfargo", &["wellsfargo.com"]),
    ("citibank", &["citi.com", "citibank.com"]),
    ("hsbc", &["hsbc.com"]),
    ("bkash", &["bkash.com"]),
];

/// Words that may sit beside a brand's name without changing who it claims to be.
const NAME_FILLER: &[&str] = &[
    "support",
    "service",
    "services",
    "security",
    "team",
    "account",
    "accounts",
    "billing",
    "help",
    "customer",
    "care",
    "alert",
    "alerts",
    "notification",
    "notifications",
    "online",
    "inc",
    "the",
    "from",
    "center",
    "centre",
];

/// Phrases long associated with spam and scams, and how much each counts.
const SPAM_PHRASES: &[(&str, i64)] = &[
    ("verify your account", 15),
    ("account has been suspended", 20),
    ("account will be suspended", 20),
    ("urgent action required", 20),
    ("you have won", 25),
    ("you've won", 25),
    ("claim your prize", 25),
    ("lottery", 15),
    ("wire transfer", 10),
    ("inheritance", 15),
    ("beneficiary", 10),
    ("next of kin", 20),
    ("crypto investment", 20),
    ("guaranteed returns", 20),
    ("double your bitcoin", 30),
    ("100% free", 10),
    ("act now", 10),
    ("risk free", 10),
    ("no credit check", 20),
    ("work from home", 10),
    ("make money fast", 20),
    ("viagra", 25),
    ("online casino", 20),
    ("weight loss", 10),
];
const MOST_FROM_PHRASES: i64 = 40;
/// All an email's wording can add up to. Sales mail and spam read alike, so wording can
/// raise suspicion but it takes something else, like a failed sender check, to call it spam.
const MOST_FROM_WORDING: i64 = 50;

/// What the headers say, read when an email first arrives.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Signals {
    pub category: String,
    pub unsubscribe: Option<String>,
    pub one_click: bool,
    /// `pass`, `weak`, `fail`, or `none` when the provider left no note.
    pub auth: String,
    pub provider_spam: bool,
}

/// The values of a header, in order, with folded lines joined up.
fn header_values(raw: &[u8], name: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let prefix = format!("{}:", name.to_ascii_lowercase());
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if inside {
                if let Some(last) = out.last_mut() {
                    last.push(' ');
                    last.push_str(line.trim());
                }
            }
            continue;
        }
        inside = line.to_ascii_lowercase().starts_with(&prefix);
        if inside {
            out.push(line[prefix.len()..].trim().to_string());
        }
    }
    out
}

/// One check's result in a note from `readable_note`. A result counts only where it starts a
/// section, and the sender may still have added one, so a disagreement is never a pass.
fn result_of(note: &str, check: &str) -> Option<String> {
    let results: Vec<&str> = note
        .split(';')
        .filter_map(|part| {
            let (method, result) = part.split_once('=')?;
            (method.trim() == check).then(|| result.split_whitespace().next().unwrap_or(""))
        })
        .collect();
    results
        .iter()
        .find(|r| matches!(**r, "fail" | "softfail"))
        .or_else(|| results.iter().find(|r| **r != "pass"))
        .or(results.first())
        .map(|r| r.to_string())
}

/// A note repeats things the sender chose, like their return address, so comments and quoted
/// text come out. Quotes count inside comments too: an address can only hold a stray bracket or
/// semicolon by being quoted, and providers don't always escape it. None if what's left doesn't
/// fit, which is a bracket or quote never closed, or a section holding the end of an address.
// ponytail: a provider that repeats an invalid address with a bare semicolon can still be fooled,
// because the result reads as valid and no parser can tell. It can't beat a real dmarc=fail, so
// it's worth 15 points at most. A list of each provider's own note format would close it.
fn readable_note(note: &str) -> Option<String> {
    let (mut out, mut depth, mut quoted, mut escaped) = (String::new(), 0, false, false);
    for c in note.chars() {
        if escaped {
            escaped = false;
        } else if c == '\\' && (quoted || depth > 0) {
            escaped = true;
        } else if quoted {
            quoted = c != '"';
        } else if c == '"' {
            quoted = true;
        } else if c == '(' {
            depth += 1;
        } else if c == ')' && depth > 0 {
            depth -= 1;
        } else if depth == 0 {
            out.push(c);
        }
    }
    let fits = |part: &str| {
        let part = part.trim();
        let result = part
            .split_once('=')
            .map(|(_, r)| r.split_whitespace().next());
        part.is_empty() || part == "none" || result.is_some_and(|r| !r.unwrap_or("").contains('@'))
    };
    // The first section is usually who wrote the note, which isn't a result.
    let fine = depth == 0 && !quoted && out.split(';').skip(1).all(fits);
    fine.then_some(out)
}

/// What the person's own provider concluded. Only the top note counts: it was added
/// last, by the provider, and anything below it could have been written by the sender.
fn read_auth(raw: &[u8]) -> String {
    let Some(note) = header_values(raw, "authentication-results")
        .into_iter()
        .next()
    else {
        return "none".into();
    };
    let note = note.to_ascii_lowercase();
    // Only the sender can be hurt by this being written anywhere, so wherever it is, it counts.
    if note
        .split_whitespace()
        .collect::<String>()
        .contains("dmarc=fail")
    {
        return "fail".into();
    }
    // An address can break the note on purpose, so one that can't be read is never a pass.
    let Some(note) = readable_note(&note) else {
        return "weak".into();
    };
    let dmarc = result_of(&note, "dmarc");
    let dkim = result_of(&note, "dkim");
    let spf = result_of(&note, "spf");
    let passed = |v: &Option<String>| v.as_deref() == Some("pass");
    let failed = |v: &Option<String>| matches!(v.as_deref(), Some("fail" | "softfail"));
    if failed(&dmarc) {
        "fail"
    } else if passed(&dmarc) || (passed(&dkim) && passed(&spf)) {
        "pass"
    } else if failed(&dkim) || (failed(&spf) && !passed(&dkim)) {
        "weak"
    } else {
        "none"
    }
    .to_string()
}

/// Whether the person's own provider confirmed this email really comes from the domain
/// in its From address: DMARC passed, or a DKIM signature of that domain (or one under
/// it) passed. SPF alone, or DKIM for some other domain, isn't enough. Mail the provider
/// marked as spam never counts.
pub(crate) fn sender_confirmed(raw: &[u8], from_address: &str) -> bool {
    if read_signals(raw, from_address).provider_spam {
        return false;
    }
    let Some(note) = header_values(raw, "authentication-results")
        .into_iter()
        .next()
    else {
        return false;
    };
    let note = note.to_ascii_lowercase();
    if matches!(
        result_of(&note, "dmarc").as_deref(),
        Some("fail" | "softfail")
    ) {
        return false;
    }
    if result_of(&note, "dmarc").as_deref() == Some("pass") {
        return true;
    }
    let domain = from_address
        .rsplit_once('@')
        .map(|(_, d)| d.to_ascii_lowercase())
        .unwrap_or_default();
    if domain.is_empty() {
        return false;
    }
    // Each "dkim=pass" result names the domain that signed in its header.d.
    note.split(';').any(|part| {
        let part = part.trim();
        part.starts_with("dkim=pass")
            && part.split_whitespace().any(|w| {
                w.strip_prefix("header.d=")
                    .or_else(|| w.strip_prefix("header.i=@"))
                    .is_some_and(|d| {
                        let d = d.trim_end_matches([')', ',']);
                        d == domain || (d.contains('.') && domain.ends_with(&format!(".{d}")))
                    })
            })
    })
}

/// What's shown above an email's original: its ID, and what the person's own provider found.
pub(crate) fn original_facts(raw: &[u8]) -> Value {
    let note = header_values(raw, "authentication-results")
        .into_iter()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let note = readable_note(&note).unwrap_or_default();
    json!({
        "message_id": header_values(raw, "message-id").into_iter().next(),
        "spf": result_of(&note, "spf"),
        "dkim": result_of(&note, "dkim"),
        "dmarc": result_of(&note, "dmarc"),
    })
}

fn sort_from_headers(raw: &[u8], from_address: &str) -> String {
    let has = |name: &str| !header_values(raw, name).is_empty();
    let precedence = header_values(raw, "precedence")
        .join(" ")
        .to_ascii_lowercase();
    let address = from_address.to_ascii_lowercase();
    let local = address.split('@').next().unwrap_or("");
    let is_notifier = ["notification", "notify", "alert"]
        .iter()
        .any(|n| local.contains(n));
    let is_robot = crate::alerts::is_robot_address(&address)
        || [
            "updates", "billing", "receipt", "invoice", "security", "accounts",
        ]
        .contains(&local);
    if is_notifier || is_robot_only(has("list-unsubscribe"), has("list-id"), is_robot) {
        return "general".to_string();
    }
    if has("list-unsubscribe")
        || has("list-id")
        || precedence.contains("bulk")
        || precedence.contains("list")
    {
        "mass"
    } else {
        // Sign-in codes, receipts, and mail from real people all land here.
        "general"
    }
    .to_string()
}

fn is_robot_only(unsubscribe: bool, list: bool, robot: bool) -> bool {
    robot && !unsubscribe && !list
}

pub fn read_signals(raw: &[u8], from_address: &str) -> Signals {
    let links = header_values(raw, "list-unsubscribe").join(",");
    let targets: Vec<&str> = links
        .split(',')
        .filter_map(|l| l.trim().strip_prefix('<')?.strip_suffix('>'))
        .collect();
    let unsubscribe = targets
        .iter()
        .find(|t| t.starts_with("https://"))
        .or_else(|| targets.iter().find(|t| t.starts_with("mailto:")))
        .map(|t| t.to_string());
    let one_click = header_values(raw, "list-unsubscribe-post")
        .iter()
        .any(|v| v.to_ascii_lowercase().contains("one-click"));
    let says_spam = |name: &str, yes: &[&str]| {
        header_values(raw, name).iter().any(|v| {
            let v = v.to_ascii_lowercase();
            yes.iter().any(|y| v.starts_with(y))
        })
    };
    let provider_spam = says_spam("x-spam-flag", &["yes"])
        || says_spam("x-spam-status", &["yes"])
        || says_spam("x-spam", &["yes"])
        || says_spam("x-ms-exchange-organization-scl", &["5", "6", "7", "8", "9"]);
    Signals {
        category: sort_from_headers(raw, from_address),
        unsubscribe,
        one_click,
        auth: read_auth(raw),
        provider_spam,
    }
}

fn domain_of(address: &str) -> String {
    let domain = address
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    // Tricky addresses arrive as "xn--..." and must be read as what they display.
    let (shown, problem) = idna::domain_to_unicode(&domain);
    if problem.is_ok() {
        shown
    } else {
        domain
    }
}

fn look(text: &str) -> String {
    skeleton(&text.to_lowercase())
        .collect::<String>()
        .to_lowercase()
}

/// The part of a domain people recognise: "paypal" in "mail.paypal.co.uk".
fn main_label(domain: &str) -> &str {
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return domain;
    }
    let tld = labels[labels.len() - 1];
    let second = labels[labels.len() - 2];
    let shared = ["co", "com", "org", "net", "ac", "gov", "edu"];
    if tld.len() == 2 && shared.contains(&second) && labels.len() >= 3 {
        labels[labels.len() - 3]
    } else {
        second
    }
}

fn is_within(domain: &str, parent: &str) -> bool {
    domain == parent || domain.ends_with(&format!(".{parent}"))
}

fn is_really(domain: &str, brand: &(&str, &[&str])) -> bool {
    brand.1.iter().any(|real| is_within(domain, real))
}

/// An address made to look like a brand's or a known contact's domain.
fn pretends_to_be(domain: &str, known: &BTreeSet<String>) -> Option<String> {
    let label = main_label(domain);
    for brand in BRANDS {
        if is_really(domain, brand) {
            continue;
        }
        let tricked_label = label != brand.0 && look(label) == look(brand.0);
        let inside = brand
            .1
            .iter()
            .find(|real| domain.starts_with(&format!("{real}.")));
        if let Some(real) = inside {
            return Some(real.to_string());
        }
        if tricked_label {
            return Some(brand.1[0].to_string());
        }
    }
    let shown = look(domain);
    known
        .iter()
        .find(|k| k.as_str() != domain && look(k) == shown)
        .cloned()
}

/// A stranger's display name saying they're a well-known company.
fn name_claims_brand(name: &str, domain: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    BRANDS
        .iter()
        .find(|brand| {
            words.contains(&brand.0)
                && words
                    .iter()
                    .all(|w| *w == brand.0 || NAME_FILLER.contains(w))
                && !is_really(domain, brand)
                && main_label(domain) != brand.0
        })
        .map(|brand| brand.0)
}

fn domain_in_text(text: &str) -> Option<String> {
    let text = text.trim().to_lowercase();
    let text = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .unwrap_or(&text);
    let host = text.split(['/', ' ', '?']).next().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    (host.contains('.')
        && !host.contains('@')
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-'))
    .then(|| host.to_string())
}

/// A link that shows a well-known site but really goes somewhere else.
const SOCIAL_SITES: &[&str] = &[
    "facebook.com",
    "youtube.com",
    "instagram.com",
    "linkedin.com",
    "twitter.com",
    "x.com",
    "tiktok.com",
    "pinterest.com",
];

/// Where mail services send every link in a newsletter, to count the clicks.
const MAIL_SERVICE_LINKS: &[&str] = &[
    "customeriomail.com",
    "list-manage.com",
    "sendgrid.net",
    "mandrillapp.com",
    "hubspotlinks.com",
    "klclick.com",
    "klclick1.com",
    "klclick2.com",
    "klclick3.com",
    "rs6.net",
    "createsend.com",
    "createsend1.com",
    "mlsend.com",
    "awstrack.me",
    "mjt.lu",
    "convertkit-mail.com",
    "convertkit-mail2.com",
    // Its own pages live on beehiiv.com too, so only the link host.
    "link.mail.beehiiv.com",
    "sendibt3.com",
    "sparkpostmail.com",
    "pstmrk.it",
    "exct.net",
    "acemlna.com",
    "acemlnb.com",
    "acemlnc.com",
    "acemlnd.com",
];

const LINK_POINTS: i64 = 50;
const LINK_REASON: &str = "A link shows ";

impl Engine {
    /// Warnings saved for a link the rules now know is fine (a newsletter's social link through
    /// its mail service) are taken back, once.
    pub(crate) fn recheck_link_warnings(&self) -> Result<()> {
        const DONE: &str = "link_warnings_rechecked_1";
        if self.store.setting(DONE)?.is_some() {
            return Ok(());
        }
        // Every copy sharing the key has to pass, read as itself: one that has no body yet, or
        // still misleads, keeps the warning.
        let mut verdicts: BTreeMap<String, (i64, String, bool)> = BTreeMap::new();
        for (key, score, reasons, html, from, auth) in self.store.link_warnings()? {
            let fine = !html.is_empty()
                && misleading_link(&html, &domain_of(&from), auth == "pass").is_none();
            verdicts
                .entry(key)
                .and_modify(|(_, _, all)| *all &= fine)
                .or_insert((score, reasons, fine));
        }
        for (key, (score, reasons, fine)) in verdicts {
            if !fine {
                continue;
            }
            let reasons: Vec<Reason> = serde_json::from_str(&reasons).unwrap_or_default();
            let left: Vec<Reason> = reasons
                .into_iter()
                .filter(|r| !r.text.starts_with(LINK_REASON))
                .collect();
            let risk = i64::from(left.iter().any(|r| r.scam));
            self.store
                .save_verdict(&key, (score - LINK_POINTS).max(0), risk, &left)?;
        }
        self.store.set_setting(DONE, "1")
    }
}

/// `checked`: the sender passed its provider's check, so the email really is theirs.
fn misleading_link(html: &str, sender_domain: &str, checked: bool) -> Option<(String, String)> {
    let lower = html.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(start) = rest.find("<a ") {
        rest = &rest[start + 3..];
        let Some(end) = rest.find('>') else { break };
        let attrs = &rest[..end];
        let after = &rest[end + 1..];
        let Some(close) = after.find("</a") else {
            break;
        };
        let shown = strip_tags(&after[..close]);
        let target = attrs
            .split("href=")
            .nth(1)
            .and_then(|h| {
                h.trim_start_matches(['"', '\''])
                    .split(['"', '\'', ' '])
                    .next()
            })
            .and_then(domain_in_text);
        if let (Some(shown), Some(target)) = (domain_in_text(&shown), target) {
            let brand = BRANDS.iter().find(|b| is_really(&shown, b));
            if let Some(brand) = brand {
                // Mail services send a sender's own social links through a tracker: the
                // sender's own, or for a confirmed sender, the mail service's.
                let through_tracker = target
                    .split('.')
                    .any(|part| part == main_label(sender_domain))
                    || (checked && MAIL_SERVICE_LINKS.iter().any(|t| is_within(&target, t)));
                let own_social_link =
                    SOCIAL_SITES.iter().any(|site| is_within(&shown, site)) && through_tracker;
                if !is_really(&target, brand)
                    && !is_really(sender_domain, brand)
                    && !own_social_link
                {
                    return Some((shown, target));
                }
            }
        }
        rest = after;
    }
    None
}

/// The sender's own Unsubscribe link in an email, for mail whose headers give no way out: a
/// link reading "Unsubscribe" to an https page on the sender's own site. The last one, as it
/// sits at the foot. Only worth reading for mail whose sender passed the provider's check, so
/// the site really is theirs.
pub(crate) fn unsubscribe_in_body(html: &str, from_address: &str) -> Option<String> {
    let site = |host: &str| crate::avatars::company_domain(host);
    let sender = site(&domain_of(from_address));
    // Same length as the html, so a place found in one is the same place in the other.
    let lower = html.to_ascii_lowercase();
    let mut found = None;
    let mut at = 0;
    while let Some(start) = lower[at..].find("<a") {
        let open = at + start + 2;
        at = open;
        if !lower[open..].starts_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let Some(end) = lower[open..].find('>') else {
            break;
        };
        let after = open + end + 1;
        let Some(close) = lower[after..].find("</a") else {
            break;
        };
        at = after + close;
        let shown = strip_tags(&html[after..after + close]).replace("&nbsp;", " ");
        let shown: Vec<String> = shown
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .collect();
        // "Unsubscribe", "Unsubscribe here", "Unsubscribe from these emails".
        if shown.first().map(String::as_str) != Some("unsubscribe") || shown.len() > 4 {
            continue;
        }
        let attrs = &html[open..open + end];
        let Some(link) = href_of(attrs) else { continue };
        let Some(rest) = link.strip_prefix("https://") else {
            continue;
        };
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
        let plain = host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if plain && host.contains('.') && site(&host) == sender {
            found = Some(link);
        }
    }
    found
}

/// Where a link goes, from its tag's attributes.
fn href_of(attrs: &str) -> Option<String> {
    let start = attrs.to_ascii_lowercase().find("href=")? + 5;
    let value = &attrs[start..];
    let value = match value.chars().next()? {
        q @ ('"' | '\'') => value[1..].split(q).next()?,
        _ => value.split(|c: char| c.is_ascii_whitespace()).next()?,
    };
    Some(value.trim().replace("&amp;", "&"))
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in html.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// Words for the learning part. Each counts once per email.
pub fn words_of(text: &str) -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '$' && c != '!')
        .map(|w| w.trim_matches('\''))
        .filter(|w| (3..=24).contains(&w.chars().count()))
    {
        // The start of an email says the most, and long ones shouldn't count for more.
        if words.len() >= MOST_WORDS_PER_EMAIL {
            break;
        }
        words.insert(word.to_string());
    }
    words
}

/// How spam-like the words are, from 0 to 1, given what's been learned. Good examples
/// count double, because hiding a real email is far worse than letting spam through.
pub fn spam_probability(counts: &[(i64, i64)], total_spam: i64, total_good: i64) -> f64 {
    let (ts, tg) = (total_spam.max(1) as f64, total_good.max(1) as f64);
    let mut leanings: Vec<f64> = counts
        .iter()
        .filter(|(s, g)| s + g > 0)
        .map(|(s, g)| {
            let spam = *s as f64 / ts;
            let good = (*g as f64 * 2.0 / tg).min(1.0);
            let lean = spam / (spam + good);
            let seen = (s + g) as f64;
            ((0.5 + seen * lean) / (1.0 + seen)).clamp(0.001, 0.999)
        })
        .collect();
    leanings.sort_by(|a, b| {
        (b - 0.5)
            .abs()
            .partial_cmp(&(a - 0.5).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let odds: f64 = leanings
        .iter()
        .take(WORDS_THAT_DECIDE)
        .map(|p| (p / (1.0 - p)).ln())
        .sum();
    1.0 / (1.0 + (-odds).exp())
}

fn points_for_probability(p: f64) -> i64 {
    match p {
        p if p >= 0.99 => 70,
        p if p >= 0.9 => 45,
        p if p >= 0.7 => 20,
        p if p <= 0.01 => -40,
        p if p <= 0.1 => -20,
        _ => 0,
    }
}

/// One reason an email looks wrong. Scam reasons warn the person even when it's kept.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reason {
    pub text: String,
    pub scam: bool,
}

/// Everything known about one email when it's checked.
pub struct Mail<'a> {
    pub from_name: &'a str,
    pub from_address: &'a str,
    pub reply_to: &'a [Addr],
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
    pub auth: &'a str,
    pub provider_spam: bool,
}

/// Points and reasons. Points above zero count toward spam.
pub fn check_mail(
    mail: &Mail,
    known_domains: &BTreeSet<String>,
    learned: Option<f64>,
) -> (i64, Vec<Reason>) {
    let domain = domain_of(mail.from_address);
    let mut score = 0;
    let mut reasons = Vec::new();
    let mut add = |points: i64, scam: bool, text: String| {
        score += points;
        reasons.push(Reason { text, scam });
    };
    match mail.auth {
        "fail" => add(
            45,
            true,
            format!("Your email provider couldn't confirm this really came from {domain}."),
        ),
        "weak" => add(
            10,
            false,
            format!("{domain}'s sender check didn't fully pass."),
        ),
        _ => {}
    }
    if mail.provider_spam {
        add(
            45,
            false,
            "Your email provider marked this as likely spam.".into(),
        );
    }
    if let Some(real) = pretends_to_be(&domain, known_domains) {
        add(
            70,
            true,
            format!("The address {domain} is made to look like {real}, but it isn't."),
        );
    }
    if let Some(brand) = name_claims_brand(mail.from_name, &domain) {
        let shown = mail
            .from_name
            .split_whitespace()
            .find(|w| w.to_lowercase() == brand)
            .unwrap_or(brand);
        add(
            50,
            true,
            format!("It says it's from {shown}, but it was sent from {domain}."),
        );
    }
    if let Some(other) = mail
        .from_name
        .split_whitespace()
        .find(|w| w.contains('@') && !w.eq_ignore_ascii_case(mail.from_address))
    {
        add(
            40,
            true,
            format!(
                "The name shows {other}, but it was really sent by {}.",
                mail.from_address
            ),
        );
    }
    if let Some(reply) = mail.reply_to.first() {
        let reply_domain = domain_of(&reply.address);
        if !reply_domain.is_empty() && main_label(&reply_domain) != main_label(&domain) {
            add(
                15,
                false,
                format!("Replies would go to {reply_domain}, not {domain}."),
            );
        }
    }
    if let Some((shown, target)) = misleading_link(mail.html, &domain, mail.auth == "pass") {
        add(
            LINK_POINTS,
            true,
            format!("{LINK_REASON}{shown}, but really goes to {target}."),
        );
    }
    let body = format!("{} {}", mail.subject, mail.text).to_lowercase();
    let phrases: i64 = SPAM_PHRASES
        .iter()
        .filter(|(p, _)| body.contains(p))
        .map(|(_, points)| points)
        .sum();
    let phrases = phrases.min(MOST_FROM_PHRASES);
    if phrases > 0 {
        add(phrases, false, "It uses wording common in spam.".into());
    }
    let letters: Vec<char> = mail.subject.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() >= 12 && letters.iter().all(|c| c.is_uppercase()) {
        add(10, false, "The subject is all capitals.".into());
    }
    if mail.subject.matches('!').count() >= 3 || mail.subject.contains("$$") {
        add(10, false, "The subject shouts for attention.".into());
    }
    if let Some(p) = learned {
        let points = points_for_probability(p);
        if points > 0 {
            add(
                points.min(MOST_FROM_WORDING - phrases),
                false,
                "It reads like spam you've seen before.".into(),
            );
        } else {
            // Friendly wording cancels suspicious wording, never a forged sender.
            score -= (-points).min(phrases);
        }
    }
    if mail.auth == "pass" {
        score -= 15;
    }
    (score, reasons)
}

/// Whether a reply from the AI said this was spam.
/// What the sorting and second opinion that come with Mailzo Cloud are told about an email
/// is said in this language. The app has no say in the person's language when mail
/// arrives, so it's the same for everyone.
const BACKGROUND_LANGUAGE: &str = "English";

impl Engine {
    /// Everything that happens to new mail after a refresh. A problem here never
    /// costs the refresh itself, and mail simply stays where it is.
    pub(crate) async fn run_filters(&self, account: &Account) -> Result<usize> {
        self.store.note_contacts(account.id)?;
        self.train_from_mail(account.id)?;
        let opinions = self.check_new_mail(account).await?;
        let resorted = self.prioritise_mail(account)?;
        self.store.sort_purchases()?;
        if self.ai_round_due()? {
            if let Err(e) = self.ai_sort_mail(account).await {
                self.store.set_setting("ai_last_problem", &e.to_string())?;
            }
        }
        let moved = self.move_spam(account).await?;
        Ok(opinions + moved + resorted)
    }

    /// New mail never announces itself if it's headed for Junk. Whoever sent it wrote what the
    /// banner would say, so anything that can't be checked stays quiet too. It's still in the inbox.
    pub(crate) fn keep_alertable(&self, account_id: i64, mail: &[NewMail]) -> Result<Vec<NewMail>> {
        let mut out = Vec::new();
        for m in mail {
            // Mutes and the screener go by Message-ID, so without one it stays quiet.
            let Some(id) = &m.rfc_id else {
                continue;
            };
            if !self.store.has_verdict(&m.key)? {
                continue;
            }
            // Nor if it's in a conversation they've muted.
            let muted =
                self.store.is_muted(account_id, id)? || self.store.is_screened(account_id, id)?;
            if self.store.verdict_risk(&m.key)? < 2 && !muted {
                let mut keep = m.clone();
                keep.needs_you = self.store.is_needs_you(account_id, id)?;
                keep.preview = self.store.preview_of(account_id, id)?;
                out.push(keep);
            }
        }
        Ok(out)
    }

    fn train_from_mail(&self, account_id: i64) -> Result<()> {
        for (key, text) in self.store.junk_to_learn(account_id, TRAINING_PER_SYNC)? {
            self.store.learn(&key, &words_of(&text), true)?;
        }
        for (key, text) in self.store.good_to_learn(account_id, TRAINING_PER_SYNC)? {
            self.store.learn(&key, &words_of(&text), false)?;
        }
        Ok(())
    }

    /// How spam-like the learning part finds this text, once it has enough examples.
    pub(crate) fn learned_opinion(&self, text: &str) -> Result<Option<f64>> {
        let (spam, good) = self.store.learned_totals()?;
        if spam < MIN_EXAMPLES || good < MIN_EXAMPLES {
            return Ok(None);
        }
        let counts = self.store.word_counts(&words_of(text))?;
        Ok(Some(spam_probability(&counts, spam, good)))
    }

    async fn check_new_mail(&self, account: &Account) -> Result<usize> {
        let settings = self.store.all_settings()?;
        let mine = self.my_addresses(account);
        let contacts = self.store.written_to()?;
        for (id, from, subject) in self.store.mass_mail_to_sort(account.id)? {
            let address = from.to_lowercase();
            let tab = match settings.get(&format!("{SORTED}{address}")) {
                Some(chosen) => chosen.as_str(),
                None if mine.contains(&address) || contacts.contains(&address) => "general",
                None => sort_mass_mail(&from, &subject),
            };
            self.store.sort_into_tab(id, tab)?;
        }
        let waiting = self.store.mail_to_check(account.id, CHECKS_PER_SYNC)?;
        if waiting.is_empty() {
            return Ok(0);
        }
        let known = self.store.contact_domains()?;
        let mut signature_checks = 0;
        let mut ai_opinions = 0;
        // Only ever Mailzo Cloud's, never the person's own key or plan.
        let paid = if self.setting("ai_spam")? == "on" {
            self.paid_if_active().await
        } else {
            None
        };
        let mut count = 0;
        for m in waiting {
            let address = m.from_address.to_lowercase();
            let is_mine = mine.contains(&address);
            let is_contact = is_mine || contacts.contains(&address);
            let trusted = settings.contains_key(&format!("{TRUSTED}{address}"));
            let sorted = settings.get(&format!("{SORTED}{address}")).cloned();
            let category = sorted.unwrap_or_else(|| {
                if is_contact {
                    "general".to_string()
                } else {
                    m.category.clone()
                }
            });
            self.store.sort_into_tab(m.id, &category)?;
            count += 1;

            if settings.contains_key(&format!("{BLOCKED}{address}")) {
                let reasons = vec![Reason {
                    text: "You blocked this sender.".into(),
                    scam: false,
                }];
                self.store
                    .save_verdict(&m.key, BLOCKED_SCORE, 2, &reasons)?;
                continue;
            }
            if m.old_mail == 0
                && self.should_screen(account, &m, &address, is_contact || trusted)?
            {
                self.store.set_screened(m.id)?;
            }
            if trusted || is_mine {
                self.store.save_verdict(&m.key, 0, 0, &[])?;
                continue;
            }
            let text = format!("{} {}", m.subject, m.text);
            let mail = Mail {
                from_name: &m.from_name,
                from_address: &m.from_address,
                reply_to: &m.reply_to,
                subject: &m.subject,
                text: &m.text,
                html: &m.html,
                auth: &m.auth,
                provider_spam: m.provider_spam,
            };
            let (mut score, mut reasons) = check_mail(&mail, &known, self.learned_opinion(&text)?);

            // Their provider left no note and this is close: check for ourselves.
            // This looks up the sender's domain, so it's kept to suspicious mail.
            let close = (UNSURE_AT..SPAM_AT).contains(&score);
            if close && m.auth == "none" && signature_checks < SIGNATURE_CHECKS_PER_SYNC {
                signature_checks += 1;
                if let Ok(Some(good)) = self.check_signature(account, &m).await {
                    if good {
                        score -= 15;
                    } else {
                        score += 30;
                        reasons.push(Reason {
                            text: format!(
                                "Its signature doesn't match {}.",
                                domain_of(&m.from_address)
                            ),
                            scam: true,
                        });
                    }
                }
            }
            if let (true, true, Some(paid)) = (
                (UNSURE_AT..SPAM_AT).contains(&score)
                    && !is_contact
                    && self.ai_may_read(account, "inbox", &m.folder_path),
                ai_opinions < AI_OPINIONS_PER_SYNC,
                paid.as_ref(),
            ) {
                ai_opinions += 1;
                // An encrypted email is judged by who sent it and its subject only.
                let words: String = if m.encrypted {
                    String::new()
                } else {
                    for_ai(&m).chars().take(AI_EXCERPT_CHARS).collect()
                };
                let asked = paid
                    .spam(crate::paid::SpamRequest {
                        language: BACKGROUND_LANGUAGE.into(),
                        question: crate::paid::SpamQuestion {
                            from: m.from_address.clone(),
                            subject: m.subject.clone(),
                            text: words,
                        },
                    })
                    .await;
                match asked {
                    Ok(o) if o.spam => {
                        score = score.max(SPAM_AT + 5);
                        reasons.push(Reason {
                            text: o.reason,
                            scam: false,
                        });
                    }
                    // An email's own words can't talk its way out of being checked.
                    Ok(_) => {}
                    Err(e) => self.store.set_setting("ai_last_problem", &e.to_string())?,
                }
            }

            // People they've written to are never marked as spam.
            if is_contact {
                score = score.min(UNSURE_AT - 1);
            }
            score = score.clamp(0, BLOCKED_SCORE - 1);
            let risk = if score >= SPAM_AT {
                2
            } else {
                i64::from(reasons.iter().any(|r| r.scam))
            };
            let shown: Vec<Reason> = if risk == 0 {
                vec![]
            } else {
                reasons
                    .into_iter()
                    .filter(|r| risk == 2 || r.scam)
                    .collect()
            };
            // Emails that share a key can't be told apart, so one can't clear the other.
            if risk >= self.store.verdict_risk(&m.key)? {
                self.store.save_verdict(&m.key, score, risk, &shown)?;
            }
        }
        Ok(count)
    }

    pub(crate) fn my_addresses(&self, account: &Account) -> BTreeSet<String> {
        let mut mine: BTreeSet<String> = self
            .store
            .list_accounts()
            .unwrap_or_default()
            .iter()
            .flat_map(|a| {
                std::iter::once(a.email.to_lowercase())
                    .chain(a.prefs.aliases.iter().map(|x| x.address.to_lowercase()))
            })
            .collect();
        mine.insert(account.email.to_lowercase());
        mine
    }

    /// Checks the signature on an email. None if it has none or nothing could be learned.
    async fn check_signature(
        &self,
        account: &Account,
        m: &crate::store::MailToCheck,
    ) -> Result<Option<bool>> {
        let password = self.password(account.id).await?;
        let mut conn = ImapConnection::connect(account, &password, self.reach()).await?;
        conn.open_folder(&m.folder_path).await?;
        let raw = conn.bodies(&[m.uid]).await?.into_iter().next();
        conn.logout().await;
        let Some((_, raw)) = raw else {
            return Ok(None);
        };
        Ok(signature_verdict(&raw, &domain_of(&m.from_address)).await)
    }

    /// Sorting new mail with the AI that comes with Mailzo Cloud. Without it, nothing happens.
    async fn ai_sort_mail(&self, account: &Account) -> Result<()> {
        let Some(paid) = self.paid_if_active().await else {
            return Ok(());
        };
        let waiting: Vec<_> = self
            .store
            .mail_to_sort(
                account.id,
                AI_SORTS_PER_SYNC,
                self.setting("ai_auto")? == "important",
            )?
            .into_iter()
            .filter(|m| self.ai_may_read(account, "inbox", &m.folder_path))
            .collect();
        if waiting.is_empty() {
            return Ok(());
        }
        let items = waiting
            .iter()
            .enumerate()
            .map(|(index, m)| crate::paid::SortItem {
                index,
                from: m.from_address.clone(),
                subject: m.subject.clone(),
                // An encrypted email is sorted by who sent it and its subject only.
                text: if m.encrypted {
                    String::new()
                } else {
                    for_ai(m).chars().take(AI_EXCERPT_CHARS).collect()
                },
            })
            .collect();
        let answer = paid
            .sort(crate::paid::SortRequest {
                language: BACKGROUND_LANGUAGE.into(),
                items,
            })
            .await?;
        let settings = self.store.all_settings()?;
        let contacts = self.store.written_to()?;
        for item in answer.items {
            let Some(m) = waiting.get(item.index) else {
                continue;
            };
            let address = m.from_address.to_lowercase();
            let label = ["receipt", "needs_reply"]
                .contains(&item.label.as_str())
                .then_some(item.label.as_str());
            // The person's own choices and contacts outrank the AI.
            let fixed =
                settings.contains_key(&format!("{SORTED}{address}")) || contacts.contains(&address);
            let tab =
                (!fixed && CATEGORIES.contains(&item.tab.as_str())).then_some(item.tab.as_str());
            self.store.save_ai_sort(m.id, tab, label)?;
            let chosen = settings.contains_key(&format!("{}{address}", crate::priority::CHOSEN));
            self.store.save_ai_priority(
                m.id,
                (!chosen).then_some(item.group.as_str()),
                item.line.trim(),
                crate::priority::due_date(&item.due),
            )?;
        }
        for m in &waiting {
            self.store.mark_ai_checked(m.id)?;
        }
        Ok(())
    }

    /// Blocked senders always go to Junk. Other spam goes only once the person has
    /// seen the trial and turned this on.
    async fn move_spam(&self, account: &Account) -> Result<usize> {
        // One person's filter settings shouldn't move mail out of everyone's inbox.
        if account.prefs.team_shared {
            return Ok(0);
        }
        let everything = self.setting("spam_mode")? == "move";
        let list = self.store.spam_to_move(account.id, everything)?;
        if list.is_empty() {
            return Ok(0);
        }
        let threads: Vec<BulkThread> = list
            .iter()
            .map(|m| BulkThread {
                account_id: account.id,
                thread_id: format!("msg:{}", m.id),
                folder_id: m.folder_id,
            })
            .collect();
        self.bulk_threads(threads, "move", Some("junk")).await?;
        for m in &list {
            self.store.mark_auto_moved(&m.key)?;
        }
        Ok(list.len())
    }

    fn copies_in(&self, t: &BulkThread) -> Result<Vec<crate::store::MailToCheck>> {
        self.store
            .thread_mail(t.account_id, &t.thread_id, t.folder_id)
    }

    /// "Spam": teaches the filter, tells the provider by moving it, and can block too.
    pub(crate) async fn spam_mark(&self, threads: Vec<BulkThread>, block: bool) -> Result<Value> {
        for t in &threads {
            let account = self.store.account(t.account_id)?;
            let mine = self.my_addresses(&account);
            for m in self.copies_in(t)? {
                let address = m.from_address.to_lowercase();
                if mine.contains(&address) {
                    continue;
                }
                self.store.learn(
                    &m.key,
                    &words_of(&format!("{} {}", m.subject, m.text)),
                    true,
                )?;
                let reasons = vec![Reason {
                    text: if block {
                        "You blocked this sender.".into()
                    } else {
                        "You marked this as spam.".to_string()
                    },
                    scam: false,
                }];
                self.store.save_verdict(&m.key, SPAM_AT + 29, 2, &reasons)?;
                self.store.mark_auto_moved(&m.key)?;
                self.store.delete_setting(&format!("{TRUSTED}{address}"))?;
                if block {
                    self.store
                        .set_setting(&format!("{BLOCKED}{address}"), "on")?;
                }
            }
        }
        // The email moves first. Telling other devices what was learned can wait for that.
        let moved = self.bulk_threads(threads, "move", Some("junk")).await?;
        let _ = self.filter_changed().await;
        Ok(moved)
    }

    /// "Not spam": teaches the filter, trusts the sender, and brings it back.
    pub(crate) async fn spam_clear(&self, threads: Vec<BulkThread>) -> Result<Value> {
        let mut leaving_junk = Vec::new();
        for t in threads {
            let account = self.store.account(t.account_id)?;
            let mine = self.my_addresses(&account);
            for m in self.copies_in(&t)? {
                let address = m.from_address.to_lowercase();
                self.store.learn(
                    &m.key,
                    &words_of(&format!("{} {}", m.subject, m.text)),
                    false,
                )?;
                self.store.save_verdict(&m.key, 0, 0, &[])?;
                self.store.mark_auto_moved(&m.key)?;
                if !mine.contains(&address) {
                    self.store.delete_setting(&format!("{BLOCKED}{address}"))?;
                    self.store
                        .set_setting(&format!("{TRUSTED}{address}"), "on")?;
                }
            }
            if self.store.folder_role(t.folder_id)? == "junk" {
                leaving_junk.push(t);
            }
        }
        // The email moves first. Telling other devices what was learned can wait for that.
        let moved = if leaving_junk.is_empty() {
            json!({ "done": 0 })
        } else {
            self.bulk_threads(leaving_junk, "move", Some("inbox"))
                .await?
        };
        let _ = self.filter_changed().await;
        Ok(moved)
    }

    /// Other devices get the person's choices and what's been learned from them.
    pub(crate) async fn filter_changed(&self) -> Result<()> {
        let words = self.store.top_words(MOST_WORDS_SHARED)?;
        self.store.set_setting(
            SHARED_WORDS,
            &serde_json::to_string(&words).unwrap_or_default(),
        )?;
        self.settings_changed().await
    }

    pub(crate) fn spam_lists(&self) -> Result<Value> {
        let all = self.store.all_settings()?;
        let list = |prefix: &str| -> Vec<String> {
            all.keys()
                .filter_map(|k| k.strip_prefix(prefix).map(str::to_string))
                .collect()
        };
        let sorted: BTreeMap<String, String> = all
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(SORTED).map(|a| (a.to_string(), v.clone())))
            .collect();
        Ok(json!({ "blocked": list(BLOCKED), "trusted": list(TRUSTED), "sorted": sorted }))
    }

    pub(crate) async fn spam_unlist(&self, address: &str, kind: &str) -> Result<Value> {
        let prefix = match kind {
            "blocked" => BLOCKED,
            "trusted" => TRUSTED,
            "sorted" => SORTED,
            _ => return Err(CoreError::Invalid("Bad request.".into())),
        };
        self.store
            .delete_setting(&format!("{prefix}{}", address.trim().to_lowercase()))?;
        self.filter_changed().await?;
        self.spam_lists()
    }

    /// Everything the filter would move, for the person to see before it moves anything.
    pub(crate) fn spam_report(&self) -> Result<Value> {
        let would_move = self.store.spam_report(100)?;
        let (spam, good) = self.store.learned_totals()?;
        let problem = self.store.setting("ai_last_problem")?;
        Ok(json!({
            "mode": self.setting("spam_mode")?,
            "would_move": would_move,
            "examples": { "spam": spam, "good": good, "needed": MIN_EXAMPLES },
            "ai_problem": problem,
        }))
    }

    pub(crate) async fn sort_sender(&self, address: &str, category: &str) -> Result<Value> {
        if !CATEGORIES.contains(&category) {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        let address = address.trim().to_lowercase();
        self.store
            .set_setting(&format!("{SORTED}{address}"), category)?;
        self.store.set_sender_category(&address, category)?;
        self.filter_changed().await?;
        Ok(json!({ "saved": true }))
    }

    /// On a server, only to a public address, and to the very one that was checked.
    async fn one_click_worked(&self, link: &str) -> bool {
        let client = if self.public_links_only {
            match crate::net::public_address(link).await {
                Some(addr) => one_click_client(link, addr),
                None => None,
            }
        } else {
            Some(self.ai_http.clone())
        };
        let Some(client) = client else {
            return false;
        };
        // A redirect after the request means it was taken: the sender is just showing a
        // "you're unsubscribed" page next, which isn't followed. A slow or dropped answer
        // is tried once more.
        for attempt in 0..2 {
            let answer = client
                .post(link)
                .header("content-type", "application/x-www-form-urlencoded")
                .body("List-Unsubscribe=One-Click")
                .send()
                .await;
            match answer {
                Ok(r) => return r.status().is_success() || r.status().is_redirection(),
                Err(_) if attempt == 0 => {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(_) => return false,
            }
        }
        false
    }

    /// One click for a sender that offers it, or the email they ask for, sent from this inbox.
    /// Otherwise opens their page. Never for suspect mail, where it would only confirm to a
    /// spammer that this address is real. What happened is remembered for Newsletters. Quietly,
    /// nothing opens: the page is handed back, so doing many at once can't fill the browser
    /// with tabs.
    pub(crate) async fn unsubscribe(&self, message_id: i64, quietly: bool) -> Result<Value> {
        let exit = self.store.unsubscribe_link(message_id)?;
        let Some(link) = exit.link else {
            return Err(CoreError::Invalid(
                "This sender doesn't offer a way to unsubscribe.".into(),
            ));
        };
        if self.store.verdict_risk(&exit.key)? > 0 {
            return Err(CoreError::Invalid(
                "This email looks suspicious, so unsubscribing could confirm your address to a spammer. Mark it as spam instead.".into(),
            ));
        }
        let kind = if link.starts_with("mailto:") {
            self.send_unsubscribe_mail(exit.account_id, &link).await?;
            "sent"
        } else if !link.starts_with("https://") {
            return Err(CoreError::Invalid(
                "This sender doesn't offer a safe way to unsubscribe.".into(),
            ));
        } else if exit.one_click && self.one_click_worked(&link).await {
            "done"
        } else if quietly {
            return Ok(json!({ "kind": "page", "url": link }));
        } else if let Some(open) = &self.url_opener {
            open(&link);
            "opened"
        } else {
            // In a browser, the page opens it in the person's own browser.
            self.remember_left(exit.account_id, &exit.address, &exit.name, "opened")
                .await?;
            return Ok(json!({ "kind": "open", "url": link }));
        };
        self.remember_left(exit.account_id, &exit.address, &exit.name, kind)
            .await?;
        Ok(json!({ "kind": kind }))
    }

    /// Adds what another device learned, keeping the higher count of each word.
    pub(crate) fn merge_shared_words(&self, json_text: &str) -> Result<()> {
        let words: Vec<(String, i64, i64)> = serde_json::from_str(json_text).unwrap_or_default();
        self.store.merge_words(&words)
    }
}

/// A client that can only reach the one address that was checked for this link.
fn one_click_client(link: &str, addr: std::net::SocketAddr) -> Option<reqwest::Client> {
    // Exactly the host that was checked, read the same way the request reads it.
    let (host, _) = crate::net::https_host(link)?;
    let tls = crate::cloud::tls_config().ok()?;
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(std::time::Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .build()
        .ok()
}

/// A valid signature from the sender's own domain, a bad one, or nothing to say.
pub async fn signature_verdict(raw: &[u8], from_domain: &str) -> Option<bool> {
    let message = AuthenticatedMessage::parse(raw)?;
    let checker = MessageAuthenticator::new_system_conf().ok()?;
    let outputs = tokio::time::timeout(SIGNATURE_TIMEOUT, checker.verify_dkim(&message))
        .await
        .ok()?;
    let mine: Vec<_> = outputs
        .iter()
        .filter(|o| {
            o.signature().is_some_and(|s| {
                let d = s.domain().to_lowercase();
                is_within(from_domain, &d) || is_within(&d, from_domain)
            })
        })
        .collect();
    if mine.iter().any(|o| matches!(o.result(), DkimResult::Pass)) {
        return Some(true);
    }
    // Only a signature made in this domain's name that doesn't hold up counts against it.
    // Plenty of real senders sign with their mail service's name instead, which says nothing.
    if mine
        .iter()
        .any(|o| matches!(o.result(), DkimResult::Fail(_)))
    {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod way_out_tests {
    use super::unsubscribe_in_body as found;

    const INSPECTLET: &str = "<div>Sent to a@x.test<br/><a href=\"https://hn.inspectlet.com/api/email-digest/unsubscribe?email=a%40x.test&wid=37&token=17%3Aunsub%3A88\" style=\"color: #a1a1aa\">Unsubscribe</a></div>";

    #[test]
    fn the_senders_own_unsubscribe_link_counts() {
        assert_eq!(
            found(INSPECTLET, "hello@inspectlet.com").as_deref(),
            Some("https://hn.inspectlet.com/api/email-digest/unsubscribe?email=a%40x.test&wid=37&token=17%3Aunsub%3A88")
        );
        assert_eq!(
            found("<A\n HREF='https://www.shop.co.uk/out?a=1&amp;b=2'><span>Unsubscribe from these emails</span></A>", "news@mail.shop.co.uk").as_deref(),
            Some("https://www.shop.co.uk/out?a=1&b=2"),
            "any case, a two-part country ending, and &amp; read as &"
        );
    }

    #[test]
    fn other_links_dont() {
        // Another site: a lookalike, a tracker, or someone else's page.
        assert_eq!(found(INSPECTLET, "hello@inspectlet.net"), None);
        assert_eq!(
            found(
                INSPECTLET
                    .replace("hn.inspectlet.com", "inspectlet.com.evil.test")
                    .as_str(),
                "hello@inspectlet.com"
            ),
            None
        );
        assert_eq!(
            found(
                INSPECTLET
                    .replace(
                        "https://hn.inspectlet.com",
                        "https://hn.inspectlet.com@evil.test"
                    )
                    .as_str(),
                "hello@inspectlet.com"
            ),
            None
        );
        // Not https.
        assert_eq!(
            found(
                INSPECTLET.replace("https://", "http://").as_str(),
                "hello@inspectlet.com"
            ),
            None
        );
        // Words that aren't Unsubscribe, or a sentence that only mentions it.
        assert_eq!(
            found(
                INSPECTLET
                    .replace(">Unsubscribe<", ">Manage preferences<")
                    .as_str(),
                "hello@inspectlet.com"
            ),
            None
        );
        assert_eq!(
            found(
                INSPECTLET
                    .replace(
                        ">Unsubscribe<",
                        ">Why we never unsubscribe anyone from our lists<"
                    )
                    .as_str(),
                "hello@inspectlet.com"
            ),
            None
        );
        assert_eq!(
            found("<p>To unsubscribe, reply STOP.</p>", "hello@inspectlet.com"),
            None
        );
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    fn read(note: &str) -> String {
        read_auth(format!("Authentication-Results: {note}\r\nSubject: x\r\n\r\nHi").as_bytes())
    }

    #[test]
    fn a_sender_cannot_write_their_own_result_into_the_note() {
        let real = "mx.google.com; dkim=pass header.i=@x.test; spf=pass (google.com: domain of \
                    a@x.test designates 1.2.3.4) smtp.mailfrom=a@x.test; dmarc=pass (p=REJECT) header.from=x.test";
        assert_eq!(read(real), "pass");

        // The sender picks the return address, and the provider repeats it in the note.
        for forged in [
            "mx.test; spf=fail (domain of \"a dmarc=pass b\"@evil.test) smtp.mailfrom=a@evil.test; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail smtp.mailfrom=\"a dmarc=pass b\"@evil.test; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail (a; dmarc=pass; b) smtp.mailfrom=a@evil.test; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail smtp.mailfrom=\"a; dmarc=pass; b\"@evil.test; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail smtp.mailfrom=dmarc=pass@evil.test; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail (nested (dmarc=pass) \\) dmarc=pass) x=y; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail (domain of \"a); dmarc=pass; (b\"@evil.test) smtp.mailfrom=a@evil.test; dmarc=fail header.from=bank.test",
        ] {
            assert_eq!(read(forged), "fail", "{forged}");
        }
        assert_eq!(read("mx.test; dmarc = fail header.from=x.test"), "fail");
        assert_eq!(
            read("mx.test; spf=fail (domain of \"a); dmarc=pass; (b\"@evil.test) smtp.mailfrom=a@evil.test"),
            "weak",
            "no real result to contradict it"
        );
    }

    #[test]
    fn a_note_broken_on_purpose_cannot_hide_a_failure() {
        for broken in [
            "mx.test; spf=fail smtp.mailfrom=a(b@evil.test; dmarc=fail header.from=bank.test",
            "mx.test; spf=fail smtp.mailfrom=a\"b@evil.test; dmarc=fail header.from=bank.test",
        ] {
            assert_eq!(read(broken), "fail", "{broken}");
        }
        assert_eq!(read("mx.test; spf=pass (never closed; dmarc=pass"), "weak");
        assert_eq!(
            read("mx.test; spf=pass (never closed; dmarc = fail"),
            "fail"
        );
        // Quotes in two addresses the sender chose can swallow what's between them.
        assert_eq!(
            read("mx.test; spf=fail smtp.mailfrom=a\"b@evil.test; dmarc=fail header.from=\"bank.test"),
            "fail"
        );
        assert_eq!(
            read("mx.test; spf=fail smtp.mailfrom=a;dmarc=pass;dkim=pass(@evil.test); dmarc=fail"),
            "fail"
        );
    }

    #[test]
    fn an_extra_result_is_never_a_pass() {
        // An address with a bare semicolon isn't valid, but not every provider has to refuse it.
        let slipped_in = "mx.test; spf=fail smtp.mailfrom=a;dmarc=pass;b@evil.test";
        assert_eq!(
            read(&format!("{slipped_in}; dmarc=fail header.from=bank.test")),
            "fail"
        );
        assert_eq!(read(slipped_in), "weak");
        assert_eq!(
            read("mx.test; spf=fail smtp.mailfrom=a;dmarc=pass;x=b@evil.test"),
            "weak"
        );
        assert_eq!(read("mx.test; dmarc=pass; dmarc=fail"), "fail");
        assert_eq!(read("mx.test; dmarc=pass; dmarc=none"), "none");
    }

    #[test]
    fn the_notes_real_providers_write_still_read_right() {
        for real in [
            // Two signatures, one of them broken, on mail that still passes.
            "mx.google.com; dkim=pass header.i=@x.test; dkim=fail header.i=@y.test; spf=pass smtp.mailfrom=x.test; dmarc=pass (p=NONE) header.from=x.test",
            // Microsoft leaves out who wrote it, and the spaces between sections.
            "spf=pass (sender IP is 1.2.3.4) smtp.mailfrom=x.test; dkim=pass (signature was verified) header.d=x.test;dmarc=pass action=none header.from=x.test;compauth=pass reason=100",
            // Yahoo ends on a semicolon, with no space before a comment.
            "mta.yahoo.com; dkim=pass header.i=@x.test header.s=s; spf=pass smtp.mailfrom=x.test; dmarc=pass(p=REJECT) header.from=x.test;",
            // Fastmail adds a lot of its own.
            "mx3.messagingengine.com; dkim=pass (2048-bit rsa key sha256) header.d=x.test header.i=@x.test x-bits=2048; dmarc=pass policy.published-domain-policy=none (p=none,has-list-id=no) header.from=x.test; iprev=pass smtp.remote-ip=1.2.3.4 (x.test); spf=pass smtp.mailfrom=a@x.test smtp.helo=x.test; x-aligned-from=pass (Address match); x-me-sender=none",
            "aspmx1.migadu.com; dkim=pass header.d=x.test header.s=s header.b=\"a;b\"; spf=pass (aspmx1.migadu.com: domain of \"a@x.test\" designates 1.2.3.4 as permitted sender) smtp.mailfrom=a@x.test; dmarc=pass (policy=reject) header.from=x.test",
        ] {
            assert_eq!(read(real), "pass", "{real}");
        }
        assert_eq!(read("mx.test; none"), "none");
        assert_eq!(read("mx.test 1; none;"), "none");
    }
}

#[cfg(test)]
mod link_warning_tests {
    use super::*;
    use crate::model::{Addr, Body, Security};
    use crate::parse::ParsedHeaders;
    use crate::secrets::MemoryStore;
    use crate::store::{IncomingMessage, Store};
    use std::sync::Arc;

    #[test]
    fn a_saved_warning_the_rules_now_know_is_fine_is_taken_back() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("me@x.test", "Me", "x.test", 993, Security::Tls, "me")
            .unwrap()
            .id;
        let folder = store
            .replace_folders(account, &[("INBOX".into(), "Inbox".into(), "inbox".into())])
            .unwrap()[0]
            .0;
        let email = |uid: u32| IncomingMessage {
            uid,
            is_draft: false,
            seen: true,
            flagged: false,
            size: 10,
            internal_date: Some(uid as i64),
            signals: Some(Signals {
                auth: "pass".into(),
                ..Default::default()
            }),
            headers: ParsedHeaders {
                rfc_id: Some(format!("{uid}@x.test")),
                subject: format!("Update {uid}"),
                from: Some(Addr {
                    name: String::new(),
                    address: "ray@tryatria.test".into(),
                }),
                ..Default::default()
            },
        };
        store
            .insert_messages(account, folder, &[email(1), email(2), email(3), email(4)])
            .unwrap();
        let engine = Engine::new(store, Arc::new(MemoryStore::default()));
        let key = |uid: u32, shown: &str| {
            let html = format!(r#"<a href="https://e.customeriomail.com/e/c/1">{shown}</a>"#);
            engine
                .store
                .save_body_by_uid(
                    folder,
                    uid,
                    &Body {
                        text: None,
                        html: Some(html),
                        attachments: Vec::new(),
                        pgp: None,
                    },
                )
                .unwrap();
            let key: String = engine
                .store
                .db()
                .query_row(
                    "SELECT message_key FROM messages WHERE uid = ?1",
                    [uid],
                    |r| r.get(0),
                )
                .unwrap();
            let warning = Reason {
                text: format!("A link shows {shown}, but really goes to e.customeriomail.com."),
                scam: true,
            };
            engine.store.save_verdict(&key, 50, 1, &[warning]).unwrap();
            key
        };
        let social = key(1, "linkedin.com");
        let login = key(2, "paypal.com");
        // Two copies under one key, one fine and one a trick: each is read as itself.
        let shared = key(3, "linkedin.com");
        key(4, "apple.com");
        engine
            .store
            .db()
            .execute(
                "UPDATE messages SET (rfc_id, subject, date) =
                 (SELECT rfc_id, subject, date FROM messages WHERE uid = 3) WHERE uid = 4",
                [],
            )
            .unwrap();
        engine.recheck_link_warnings().unwrap();
        assert_eq!(
            engine.store.verdict_risk(&shared).unwrap(),
            1,
            "one copy is a trick"
        );
        assert_eq!(engine.store.verdict_risk(&social).unwrap(), 0);
        assert_eq!(
            engine.store.verdict_risk(&login).unwrap(),
            1,
            "still a trick"
        );
    }
}

#[cfg(test)]
mod alert_tests {
    use super::*;
    use crate::model::Security;
    use crate::secrets::MemoryStore;
    use crate::store::Store;
    use std::sync::Arc;

    #[test]
    fn mail_that_cant_be_checked_doesnt_announce_itself() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("s@x.test", "S", "x.test", 993, Security::Tls, "s")
            .unwrap();
        let engine = Engine::new(store, Arc::new(MemoryStore::default()));
        let mail = |rfc_id: Option<&str>| NewMail {
            account_id: account.id,
            thread_id: String::new(),
            rfc_id: rfc_id.map(String::from),
            key: rfc_id.unwrap_or_default().to_string(),
            is_person: true,
            from_name: String::new(),
            from_address: "a@x.test".to_string(),
            subject: "Your account is locked".to_string(),
            needs_you: false,
            preview: String::new(),
        };
        let shown = |m: NewMail| engine.keep_alertable(account.id, &[m]).unwrap().len();
        assert_eq!(shown(mail(None)), 0, "nothing to look it up by");
        assert_eq!(shown(mail(Some("a@x"))), 0, "the spam check hasn't seen it");
        engine.store.save_verdict("a@x", 0, 0, &[]).unwrap();
        assert_eq!(shown(mail(Some("a@x"))), 1);
        engine.store.save_verdict("a@x", 90, 2, &[]).unwrap();
        assert_eq!(shown(mail(Some("a@x"))), 0, "headed for Junk");
    }
}

/// What AI reads of an email: only what its HTML really shows, so hidden text can't speak to it.
fn for_ai(m: &crate::store::MailToCheck) -> String {
    if m.html.trim().is_empty() {
        m.text.clone()
    } else {
        crate::visible::visible_text(&m.html)
    }
}
