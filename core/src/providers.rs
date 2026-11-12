//! Working out how to connect an email address, from the address alone.
//!
//! Our own list comes first, then the domain's public mail records, then
//! Thunderbird's public settings directory, then what the domain publishes about
//! itself, then common server names. Only the domain is ever looked up, never the
//! whole address, and nothing is sent to any mail server until the person connects.

use crate::engine::Engine;
use crate::error::Result;
use crate::imap;
use crate::model::SmtpSecurity;
use crate::net::Reach;
use hickory_resolver::Resolver;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub const DIRECTORY_URL: &str = "https://autoconfig.thunderbird.net/v1.1";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(4);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const IMAP_TLS_PORT: u16 = 993;

const NOT_OPEN_TIP: &str = "If it's refused, check that IMAP is switched on in your provider's settings, or whether it asks for an app password.";
const WORKSPACE_TIP: &str = "Company account? Your admin may need to allow app passwords.";

/// Stands in for the internet's mail records during tests and the demo.
pub type RecordLookup = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// The normal password works.
    Password,
    /// The provider requires a special password made in its settings.
    AppPassword,
    /// Through Proton's own app, running on this computer.
    Bridge,
    /// Only the provider's own sign-in page works, through the browser.
    Browser,
    /// The same, but this build isn't set up to do it yet.
    BrowserSoon,
    /// The provider doesn't let other mail apps connect.
    Unsupported,
    /// Nothing was found, so the person fills in the details.
    Manual,
}

#[derive(Clone, Debug, Serialize)]
pub struct Found {
    pub provider: String,
    pub name: String,
    pub method: Method,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    /// Which username to try, in order.
    pub usernames: Vec<String>,
    pub steps: Vec<String>,
    pub link: Option<String>,
    pub link_label: Option<String>,
    pub note: Option<String>,
    /// Where this came from: `list`, `mail_records`, `directory`, `domain`, `server_records`,
    /// `guess` or `none`.
    pub source: String,
    /// Offers signing in through the browser with this provider.
    pub browser: Option<String>,
}

struct Known {
    id: &'static str,
    name: &'static str,
    method: Method,
    domains: &'static [&'static str],
    /// Mail records that show a domain is hosted here.
    records: &'static [&'static str],
    imap: &'static str,
    /// A different server for domains hosted here, as Zoho has for companies.
    hosted_imap: Option<&'static str>,
    smtp: (&'static str, u16, SmtpSecurity),
    hosted_smtp: Option<&'static str>,
    local_username_first: bool,
    /// Each is `title|detail|link`. The detail is optional, and `link` puts the button there.
    /// A step can carry its own button instead: `title|detail|https://…|Button words`.
    steps: &'static [&'static str],
    link: Option<(&'static str, &'static str)>,
    note: Option<&'static str>,
}

const fn hosted(
    id: &'static str,
    name: &'static str,
    records: &'static [&'static str],
    imap: &'static str,
    smtp: &'static str,
) -> Known {
    Known {
        id,
        name,
        method: Method::Password,
        domains: &[],
        records,
        imap,
        hosted_imap: None,
        smtp: (smtp, 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[],
        link: None,
        note: None,
    }
}

const KNOWN: &[Known] = &[
    Known {
        id: "google",
        name: "Gmail",
        method: Method::AppPassword,
        domains: &["gmail.com", "googlemail.com"],
        records: &["google.com", "googlemail.com"],
        imap: "imap.gmail.com",
        hosted_imap: None,
        smtp: ("smtp.gmail.com", 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Turn on 2-Step Verification|Skip if it's already on.|https://myaccount.google.com/signinoptions/twosv|Open Google security",
            "Create an app password|Name it Mailzo.|link",
            "Paste it here",
        ],
        link: Some(("https://myaccount.google.com/apppasswords", "Open App passwords")),
        note: Some(WORKSPACE_TIP),
    },
    Known {
        id: "microsoft",
        name: "Outlook",
        method: Method::BrowserSoon,
        domains: &[
            "outlook.com",
            "hotmail.com",
            "live.com",
            "msn.com",
            "outlook.co.uk",
            "hotmail.co.uk",
            "live.co.uk",
            "outlook.fr",
            "hotmail.fr",
            "live.fr",
            "outlook.de",
            "hotmail.de",
            "outlook.in",
        ],
        records: &["protection.outlook.com", "outlook.com", "hotmail.com"],
        imap: "outlook.office365.com",
        hosted_imap: None,
        smtp: ("smtp-mail.outlook.com", 587, SmtpSecurity::StartTls),
        hosted_smtp: Some("smtp.office365.com"),
        local_username_first: false,
        steps: &[],
        link: None,
        note: Some("Microsoft only allows sign-in through its own page. That's being added to this app next."),
    },
    Known {
        id: "icloud",
        name: "iCloud Mail",
        method: Method::AppPassword,
        domains: &["icloud.com", "me.com", "mac.com"],
        records: &["mail.icloud.com"],
        imap: "imap.mail.me.com",
        hosted_imap: None,
        smtp: ("smtp.mail.me.com", 587, SmtpSecurity::StartTls),
        hosted_smtp: None,
        local_username_first: true,
        steps: &[
            "Sign in to your Apple account||link",
            "Create an app-specific password|It's under Sign-In and Security.",
            "Paste it here",
        ],
        link: Some(("https://account.apple.com", "Open your Apple account")),
        note: None,
    },
    Known {
        id: "yahoo",
        name: "Yahoo Mail",
        method: Method::AppPassword,
        domains: &[
            "yahoo.com",
            "ymail.com",
            "rocketmail.com",
            "yahoo.co.uk",
            "yahoo.ca",
            "yahoo.fr",
            "yahoo.de",
            "yahoo.co.in",
            "yahoo.com.au",
        ],
        records: &["yahoodns.net"],
        imap: "imap.mail.yahoo.com",
        hosted_imap: None,
        smtp: ("smtp.mail.yahoo.com", 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Open Yahoo security||link",
            "Choose Generate app password",
            "Paste it here",
        ],
        link: Some(("https://login.yahoo.com/account/security", "Open Yahoo security")),
        note: None,
    },
    Known {
        id: "aol",
        name: "AOL Mail",
        method: Method::AppPassword,
        domains: &["aol.com", "aim.com"],
        records: &[],
        imap: "imap.aol.com",
        hosted_imap: None,
        smtp: ("smtp.aol.com", 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Open AOL security||link",
            "Choose Generate app password",
            "Paste it here",
        ],
        link: Some(("https://login.aol.com/account/security", "Open AOL security")),
        note: None,
    },
    Known {
        id: "att",
        name: "AT&T Mail",
        method: Method::AppPassword,
        domains: &["att.net", "sbcglobal.net", "bellsouth.net"],
        records: &[],
        imap: "imap.mail.att.net",
        hosted_imap: None,
        smtp: ("smtp.mail.att.net", 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Open your AT&T profile|Then choose Sign-in info.|link",
            "Add a secure mail key|One for this app is enough.",
            "Paste it here",
        ],
        link: Some(("https://www.att.com/my/#/profile/overview", "Open your AT&T profile")),
        note: None,
    },
    Known {
        id: "zoho",
        name: "Zoho Mail",
        method: Method::Password,
        domains: &["zoho.com", "zohomail.com"],
        records: &["zoho.com"],
        imap: "imap.zoho.com",
        hosted_imap: Some("imappro.zoho.com"),
        smtp: ("smtp.zoho.com", 465, SmtpSecurity::Tls),
        hosted_smtp: Some("smtppro.zoho.com"),
        local_username_first: false,
        steps: &[],
        link: Some(("https://accounts.zoho.com/home#security/app_password", "Make a Zoho app password")),
        note: Some("If you use two-step sign-in, you'll need an app password. IMAP must also be switched on in Zoho Mail's settings."),
    },
    Known {
        id: "zoho_eu",
        name: "Zoho Mail",
        method: Method::Password,
        domains: &["zoho.eu", "zohomail.eu"],
        records: &["zoho.eu"],
        imap: "imap.zoho.eu",
        hosted_imap: Some("imappro.zoho.eu"),
        smtp: ("smtp.zoho.eu", 465, SmtpSecurity::Tls),
        hosted_smtp: Some("smtppro.zoho.eu"),
        local_username_first: false,
        steps: &[],
        link: Some(("https://accounts.zoho.eu/home#security/app_password", "Make a Zoho app password")),
        note: Some("If you use two-step sign-in, you'll need an app password. IMAP must also be switched on in Zoho Mail's settings."),
    },
    Known {
        id: "zoho_in",
        name: "Zoho Mail",
        method: Method::Password,
        domains: &["zoho.in", "zohomail.in"],
        records: &["zoho.in"],
        imap: "imap.zoho.in",
        hosted_imap: Some("imappro.zoho.in"),
        smtp: ("smtp.zoho.in", 465, SmtpSecurity::Tls),
        hosted_smtp: Some("smtppro.zoho.in"),
        local_username_first: false,
        steps: &[],
        link: Some(("https://accounts.zoho.in/home#security/app_password", "Make a Zoho app password")),
        note: Some("If you use two-step sign-in, you'll need an app password. IMAP must also be switched on in Zoho Mail's settings."),
    },
    Known {
        id: "fastmail",
        name: "Fastmail",
        method: Method::AppPassword,
        domains: &["fastmail.com", "fastmail.fm", "messagingengine.com"],
        records: &["messagingengine.com"],
        imap: "imap.fastmail.com",
        hosted_imap: None,
        smtp: ("smtp.fastmail.com", 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Open Fastmail security||link",
            "Create an app password|It's under Manage app passwords. Choose email.",
            "Paste it here",
        ],
        link: Some(("https://app.fastmail.com/settings/security", "Open Fastmail security")),
        note: None,
    },
    Known {
        id: "yandex",
        name: "Yandex Mail",
        method: Method::AppPassword,
        domains: &["yandex.com", "yandex.ru", "ya.ru"],
        records: &["yandex.net", "yandex.ru"],
        imap: "imap.yandex.com",
        hosted_imap: None,
        smtp: ("smtp.yandex.com", 465, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Create an app password|Choose Mail.|link",
            "Paste it here",
        ],
        link: Some(("https://id.yandex.com/security/app-passwords", "Open Yandex App passwords")),
        note: Some("IMAP must also be switched on in Yandex Mail's settings."),
    },
    Known {
        id: "proton",
        name: "Proton Mail",
        method: Method::Bridge,
        domains: &["proton.me", "protonmail.com", "protonmail.ch", "pm.me"],
        records: &["protonmail.ch"],
        imap: "127.0.0.1",
        hosted_imap: None,
        smtp: ("127.0.0.1", 1025, SmtpSecurity::Plain),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[
            "Install Proton Bridge|It needs a paid Proton plan. Sign in and leave it running.|link",
            "Copy the password from Bridge|It's in this mailbox's settings, and isn't your Proton password.",
            "Paste it here",
        ],
        link: Some(("https://proton.me/mail/bridge", "Get Proton Mail Bridge")),
        note: None,
    },
    Known {
        id: "tuta",
        name: "Tuta Mail",
        method: Method::Unsupported,
        domains: &["tuta.com", "tuta.io", "tutanota.com", "tutanota.de", "tutamail.com", "keemail.me"],
        records: &["tutanota.de"],
        imap: "",
        hosted_imap: None,
        smtp: ("", 0, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[],
        link: None,
        note: Some("Tuta doesn't let other mail apps connect, so this app can't reach it. You can forward your mail to another inbox and connect that one."),
    },
    Known {
        id: "hey",
        name: "HEY",
        method: Method::Unsupported,
        domains: &["hey.com"],
        records: &["hey.com"],
        imap: "",
        hosted_imap: None,
        smtp: ("", 0, SmtpSecurity::Tls),
        hosted_smtp: None,
        local_username_first: false,
        steps: &[],
        link: None,
        note: Some("HEY doesn't let other mail apps connect, so this app can't reach it. You can forward your mail to another inbox and connect that one."),
    },
    hosted("migadu", "Migadu", &["migadu.com"], "imap.migadu.com", "smtp.migadu.com"),
    hosted("godaddy", "GoDaddy", &["secureserver.net"], "imap.secureserver.net", "smtpout.secureserver.net"),
    hosted("hostinger", "Hostinger", &["hostinger.com"], "imap.hostinger.com", "smtp.hostinger.com"),
    hosted("namecheap", "Namecheap Private Email", &["privateemail.com"], "mail.privateemail.com", "mail.privateemail.com"),
    hosted("titan", "Titan", &["titan.email"], "imap.titan.email", "smtp.titan.email"),
    hosted("rackspace", "Rackspace", &["emailsrvr.com"], "secure.emailsrvr.com", "secure.emailsrvr.com"),
    hosted("ionos", "IONOS", &["ionos.com", "kundenserver.de", "ui-dns.com"], "imap.ionos.com", "smtp.ionos.com"),
];

/// Letters, digits, dots and dashes only, so a domain can't change what's asked for.
fn clean_domain(domain: &str) -> Option<String> {
    let domain = domain.trim().trim_end_matches('.').to_lowercase();
    let (ascii, problem) = idna::domain_to_unicode(&domain);
    let ascii = if problem.is_ok() { ascii } else { domain };
    let ascii = idna::domain_to_ascii(&ascii).ok()?;
    let good = ascii.contains('.')
        && !ascii.starts_with(['.', '-'])
        && !ascii.contains("..")
        && ascii
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    good.then_some(ascii)
}

/// A server name from the internet. Never this computer or a bare address, which could
/// point someone's password at something on their own network.
fn real_host(host: &str) -> bool {
    let host = host.trim();
    clean_domain(host).is_some_and(|h| {
        h != "localhost"
            && !h.ends_with(".local")
            && !h.ends_with(".localhost")
            && !h.chars().all(|c| c.is_ascii_digit() || c == '.')
    })
}

fn usernames(email: &str, local_first: bool) -> Vec<String> {
    let local = email.split('@').next().unwrap_or(email).to_string();
    if local_first {
        vec![local, email.to_string()]
    } else {
        vec![email.to_string()]
    }
}

fn from_known(k: &Known, email: &str, via_records: bool) -> Found {
    let imap_host = match (via_records, k.hosted_imap) {
        (true, Some(h)) => h,
        _ => k.imap,
    };
    let smtp_host = match (via_records, k.hosted_smtp) {
        (true, Some(h)) => h,
        _ => k.smtp.0,
    };
    Found {
        provider: k.id.to_string(),
        name: if via_records && k.id == "google" {
            "Google Workspace".to_string()
        } else if via_records && k.id == "microsoft" {
            "Microsoft 365".to_string()
        } else {
            k.name.to_string()
        },
        method: k.method,
        imap_host: imap_host.to_string(),
        imap_port: if k.method == Method::Bridge {
            1143
        } else {
            IMAP_TLS_PORT
        },
        smtp_host: smtp_host.to_string(),
        smtp_port: k.smtp.1,
        smtp_security: k.smtp.2,
        usernames: usernames(email, k.local_username_first),
        steps: k.steps.iter().map(|s| s.to_string()).collect(),
        link: k.link.map(|l| l.0.to_string()),
        link_label: k.link.map(|l| l.1.to_string()),
        note: k.note.map(str::to_string),
        source: if via_records { "mail_records" } else { "list" }.to_string(),
        browser: None,
    }
}

/// Which known provider a mail record belongs to.
fn known_from_records(records: &[String]) -> Option<&'static Known> {
    records.iter().find_map(|record| {
        let record = record.trim_end_matches('.').to_lowercase();
        KNOWN.iter().find(|k| {
            k.records
                .iter()
                .any(|r| record == *r || record.ends_with(&format!(".{r}")))
        })
    })
}

fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&format!("</{name}>"))? + start;
    Some(block[start..end].trim())
}

fn fill_in(template: &str, email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    template
        .replace("%EMAILADDRESS%", email)
        .replace("%EMAILLOCALPART%", local)
        .replace("%EMAILDOMAIN%", domain)
}

/// Reads the settings format Thunderbird's directory and many domains publish.
/// Only encrypted servers count, and only those that accept a password.
pub fn parse_settings(xml: &str, email: &str) -> Option<Found> {
    let blocks = |kind: &str| -> Vec<String> {
        xml.split(&format!("<{kind}"))
            .skip(1)
            .filter_map(|part| part.split(&format!("</{kind}>")).next())
            .map(str::to_string)
            .collect()
    };
    let accepts_password =
        |b: &str| b.contains("password-cleartext") || b.contains("<authentication>plain");
    let incoming = blocks("incomingServer");
    let imap = incoming
        .iter()
        .find(|b| b.contains(r#"type="imap""#) && tag(b, "socketType") == Some("SSL"))?;
    let host = tag(imap, "hostname")?;
    let port: u16 = tag(imap, "port")?.parse().ok()?;
    if !real_host(host) {
        return None;
    }
    let outgoing = blocks("outgoingServer");
    let smtp = outgoing
        .iter()
        .find(|b| {
            tag(b, "socketType") == Some("SSL") && real_host(tag(b, "hostname").unwrap_or(""))
        })
        .or_else(|| {
            outgoing.iter().find(|b| {
                tag(b, "socketType") == Some("STARTTLS")
                    && real_host(tag(b, "hostname").unwrap_or(""))
            })
        });
    let username = fill_in(tag(imap, "username").unwrap_or("%EMAILADDRESS%"), email);
    let mut names = vec![username];
    if !names.contains(&email.to_string()) {
        names.push(email.to_string());
    }
    let imap_ok = incoming.iter().any(|b| {
        b.contains(r#"type="imap""#) && tag(b, "socketType") == Some("SSL") && accepts_password(b)
    });
    Some(Found {
        provider: "other".into(),
        name: xml
            .split("<displayName>")
            .nth(1)
            .and_then(|r| r.split("</displayName>").next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Your email provider".into()),
        method: if imap_ok {
            Method::Password
        } else {
            Method::BrowserSoon
        },
        imap_host: host.to_lowercase(),
        imap_port: port,
        smtp_host: smtp
            .and_then(|b| tag(b, "hostname"))
            .unwrap_or("")
            .to_lowercase(),
        smtp_port: smtp
            .and_then(|b| tag(b, "port"))
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
        smtp_security: match smtp.and_then(|b| tag(b, "socketType")) {
            Some("STARTTLS") => SmtpSecurity::StartTls,
            _ => SmtpSecurity::Tls,
        },
        usernames: names,
        steps: vec![],
        link: None,
        link_label: None,
        note: Some(if imap_ok {
            NOT_OPEN_TIP.to_string()
        } else {
            "This provider only allows sign-in through its own page, which this app can't do yet."
                .to_string()
        }),
        source: "directory".into(),
        browser: None,
    })
}

fn found_by_server(email: &str, host: String, source: &str) -> Found {
    Found {
        provider: "other".into(),
        name: "Your email provider".into(),
        method: Method::Password,
        imap_host: host,
        imap_port: IMAP_TLS_PORT,
        smtp_host: String::new(),
        smtp_port: 0,
        smtp_security: SmtpSecurity::Tls,
        usernames: vec![email.to_string()],
        steps: vec![],
        link: None,
        link_label: None,
        note: Some(NOT_OPEN_TIP.to_string()),
        source: source.to_string(),
        browser: None,
    }
}

fn nothing_found(email: &str) -> Found {
    Found {
        provider: "other".into(),
        name: "Your email provider".into(),
        method: Method::Manual,
        imap_host: String::new(),
        imap_port: IMAP_TLS_PORT,
        smtp_host: String::new(),
        smtp_port: 0,
        smtp_security: SmtpSecurity::Tls,
        usernames: vec![email.to_string()],
        steps: vec![],
        link: None,
        link_label: None,
        note: Some("We couldn't find the settings for this address. Your email provider's help pages will list them.".into()),
        source: "none".into(),
        browser: None,
    }
}

/// The two last parts of a name: "mx.example.com" gives "example.com".
fn parent_domain(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.');
    let labels: Vec<&str> = host.split('.').collect();
    (labels.len() >= 2).then(|| labels[labels.len() - 2..].join("."))
}

async fn mail_records(domain: &str) -> Vec<String> {
    let Ok(builder) = Resolver::builder_tokio() else {
        return vec![];
    };
    let Ok(resolver) = builder.build() else {
        return vec![];
    };
    let lookup = tokio::time::timeout(LOOKUP_TIMEOUT, resolver.mx_lookup(domain)).await;
    match lookup {
        Ok(Ok(found)) => {
            let mut records: Vec<(u16, String)> = found
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    hickory_resolver::proto::rr::RData::MX(mx) => {
                        Some((mx.preference, mx.exchange.to_utf8()))
                    }
                    _ => None,
                })
                .collect();
            records.sort();
            records.into_iter().map(|r| r.1).collect()
        }
        _ => vec![],
    }
}

async fn published_server(domain: &str) -> Option<String> {
    let resolver = Resolver::builder_tokio().ok()?.build().ok()?;
    let lookup = tokio::time::timeout(
        LOOKUP_TIMEOUT,
        resolver.srv_lookup(format!("_imaps._tcp.{domain}.")),
    )
    .await
    .ok()?
    .ok()?;
    lookup
        .answers()
        .iter()
        .find_map(|r| match &r.data {
            hickory_resolver::proto::rr::RData::SRV(srv) if srv.port == IMAP_TLS_PORT => {
                Some(srv.target.to_utf8().trim_end_matches('.').to_string())
            }
            _ => None,
        })
        .filter(|h| real_host(h))
}

/// Whether a server name found for an address may be suggested and probed. On a web host
/// only one that leads to public addresses, never one that a domain's records point inside.
async fn server_may_be_used(host: &str, port: u16, reach: Reach) -> bool {
    match reach {
        Reach::Anywhere => true,
        Reach::PublicOnly => crate::net::public_server_address(host, port).await.is_ok(),
    }
}

impl Engine {
    async fn settings_at(&self, url: String, email: &str, source: &str) -> Option<Found> {
        // The address's own domain says where its settings are, so on a web host that
        // must lead somewhere public, and to exactly the address checked. Our own
        // directory is ours.
        let client = match (self.reach(), source) {
            (Reach::PublicOnly, "domain") => {
                crate::net::public_pinned_client(&url, false, LOOKUP_TIMEOUT).await?
            }
            _ => self.ai_http.clone(),
        };
        let reply = client.get(&url).timeout(LOOKUP_TIMEOUT).send().await.ok()?;
        if !reply.status().is_success() {
            return None;
        }
        let mut found = parse_settings(&reply.text().await.ok()?, email)?;
        found.source = source.to_string();
        Some(found)
    }

    /// Works out how to connect this address. Always gives an answer: at worst, asking.
    pub async fn find_provider(&self, email: &str) -> Found {
        let mut found = self.look_up_provider(email).await;
        if self.can_sign_in_with(&found.provider) {
            found.browser = Some(found.provider.clone());
            if found.method == Method::BrowserSoon {
                found.method = Method::Browser;
                found.note = None;
            } else {
                // A password path is still there, but it's no longer the easy one.
                found.note = found.note.filter(|n| !n.starts_with("Sign in with Google"));
            }
        }
        found
    }

    async fn look_up_provider(&self, email: &str) -> Found {
        let email = email.trim();
        let Some(domain) = email.rsplit_once('@').and_then(|(_, d)| clean_domain(d)) else {
            return nothing_found(email);
        };
        if let Some(k) = KNOWN.iter().find(|k| k.domains.contains(&domain.as_str())) {
            return from_known(k, email, false);
        }
        let stand_in = self.record_lookup.clone();
        let records = match &stand_in {
            Some(lookup) => lookup(&domain),
            None => mail_records(&domain).await,
        };
        if let Some(k) = known_from_records(&records) {
            return from_known(k, email, true);
        }
        let directory = &self.directory_url;
        if let Some(found) = self
            .settings_at(format!("{directory}/{domain}"), email, "directory")
            .await
        {
            return found;
        }
        if let Some(parent) = records.first().and_then(|r| parent_domain(r)) {
            if parent != domain {
                if let Some(found) = self
                    .settings_at(format!("{directory}/{parent}"), email, "directory")
                    .await
                {
                    return found;
                }
            }
        }
        // Tests don't reach out to the real internet from here on.
        if stand_in.is_some() {
            return nothing_found(email);
        }
        let own = format!("https://autoconfig.{domain}/mail/config-v1.1.xml");
        if let Some(found) = self.settings_at(own, email, "domain").await {
            return found;
        }
        if let Some(host) = published_server(&domain).await {
            if server_may_be_used(&host, IMAP_TLS_PORT, self.reach()).await {
                return found_by_server(email, host, "server_records");
            }
        }
        for host in [format!("imap.{domain}"), format!("mail.{domain}")] {
            if imap::answers(&host, IMAP_TLS_PORT, PROBE_TIMEOUT, self.reach()).await {
                return found_by_server(email, host, "guess");
            }
        }
        nothing_found(email)
    }

    pub(crate) async fn find_provider_reply(&self, email: &str) -> Result<Value> {
        Ok(json!(self.find_provider(email).await))
    }
}

/// Every server on our own list, for checking they still answer.
pub fn known_servers() -> Vec<(String, String, u16, SmtpSecurity)> {
    KNOWN
        .iter()
        .filter(|k| !k.imap.is_empty() && k.method != Method::Bridge)
        .flat_map(|k| {
            let mut list = vec![(
                k.id.to_string(),
                k.imap.to_string(),
                IMAP_TLS_PORT,
                SmtpSecurity::Tls,
            )];
            list.push((k.id.to_string(), k.smtp.0.to_string(), k.smtp.1, k.smtp.2));
            if let Some(h) = k.hosted_imap {
                list.push((
                    k.id.to_string(),
                    h.to_string(),
                    IMAP_TLS_PORT,
                    SmtpSecurity::Tls,
                ));
            }
            if let Some(h) = k.hosted_smtp {
                list.push((k.id.to_string(), h.to_string(), k.smtp.1, k.smtp.2));
            }
            list
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_published_server_inside_the_network_isnt_used_on_a_web_host() {
        // As if a domain's records named a server leading to this machine.
        for host in ["localhost", "127.0.0.1", "no-such-host.invalid"] {
            assert!(
                !server_may_be_used(host, IMAP_TLS_PORT, Reach::PublicOnly).await,
                "{host}"
            );
        }
        // On a Mac the name is used as before; connecting to it is what tells.
        assert!(server_may_be_used("localhost", IMAP_TLS_PORT, Reach::Anywhere).await);
    }
}
