//! Everyone sending mail with a way out, across all inboxes, and who this person has left.

use crate::compose::escape_html;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::model::{Addr, ThreadSummary};
use crate::store::Store;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// Followed by the inbox's address, a space, and the sender's. Each inbox subscribes on its own.
pub(crate) const UNSUBSCRIBED: &str = "unsub:";

/// Senders get this long to act on an unsubscribe before they're called out for ignoring it.
const IGNORED_AFTER_SECONDS: i64 = 14 * 86_400;

/// Where mail doesn't count: it's either unwanted already, or this person's own.
macro_rules! not_their_mail {
    () => {
        "(SELECT id FROM folders WHERE role IN ('junk', 'trash', 'sent', 'drafts'))"
    };
}
pub(crate) use not_their_mail;
const NOT_THEIR_MAIL: &str = not_their_mail!();

/// Whether Mailzo offers to unsubscribe from email `m`, with its verdict `v`: it has a way
/// out, doesn't look suspect (that would only confirm the address), isn't held back or
/// unwanted already, and isn't from the person's own address. Newsletters and the
/// Unsubscribe beside a sender both go by this, so they always agree.
macro_rules! can_unsubscribe {
    () => {
        concat!(
            "(m.unsubscribe IS NOT NULL AND m.screened = 0 AND COALESCE(v.risk, 0) = 0
              AND m.folder_id NOT IN ",
            $crate::newsletters::not_their_mail!(),
            " AND lower(m.from_addr) NOT IN (SELECT lower(email) FROM accounts))"
        )
    };
}
pub(crate) use can_unsubscribe;
const CAN_UNSUBSCRIBE: &str = can_unsubscribe!();

/// What an email says about leaving its list.
pub struct WayOut {
    pub link: Option<String>,
    pub one_click: bool,
    pub key: String,
    pub account_id: i64,
    pub address: String,
    pub name: String,
}

/// When this person unsubscribed, and how far that got: `done`, `sent` or `opened`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Left {
    pub at: i64,
    pub how: String,
    pub name: String,
}

/// One sender reaching one inbox.
#[derive(Debug, Clone, Serialize)]
pub struct Newsletter {
    pub address: String,
    pub name: String,
    pub account_id: i64,
    pub folder_id: i64,
    pub threads: i64,
    pub last_date: i64,
    pub subject: String,
    /// Their newest email, whose link is used. Nothing once all their mail has gone.
    pub message_id: Option<i64>,
    /// `one_click`, `mail` or `page`, or `none` for a sender with no way out at all.
    pub how: String,
    pub left: Option<Left>,
    /// Left, yet still sending well after a sender should have stopped.
    #[serde(default)]
    pub still_sending: bool,
}

fn way_out_kind(link: &str, one_click: bool) -> &'static str {
    if link.starts_with("mailto:") {
        "mail"
    } else if one_click {
        "one_click"
    } else {
        "page"
    }
}

/// The address, subject and body a list asks to be sent. The sender wrote this link, so it
/// counts only for those three, and for one address, so it can't copy in anyone else.
fn read_mailto(link: &str) -> Option<(String, String, String)> {
    let url = reqwest::Url::parse(link).ok()?;
    if url.scheme() != "mailto" {
        return None;
    }
    let to: lettre::Address = url.path().parse().ok()?;
    let field = |name: &str| {
        url.query_pairs()
            .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.trim().is_empty())
            .map_or_else(
                || "Unsubscribe".to_string(),
                |(_, value)| value.into_owned(),
            )
    };
    Some((to.to_string(), field("subject"), field("body")))
}

impl Store {
    /// Senders offering a way out, newest first. One whose newest email looks suspect is left
    /// out, as unsubscribing would only confirm this address to them.
    pub fn newsletter_rows(&self) -> Result<Vec<Newsletter>> {
        let db = self.reading_db();
        let mut st = db.prepare_cached(&format!(
            "WITH newest AS (
                 SELECT m.id AS id, MAX(m.date) AS last_date, COUNT(DISTINCT m.thread_id) AS threads
                 FROM messages m INDEXED BY messages_newsletters
                 WHERE m.unsubscribe IS NOT NULL AND m.screened = 0
                   AND m.folder_id NOT IN {NOT_THEIR_MAIL}
                 GROUP BY m.account_id, m.from_addr COLLATE NOCASE
             )
             SELECT lower(m.from_addr), m.from_name, m.account_id, m.folder_id, n.threads,
                    n.last_date, m.subject, m.id, m.unsubscribe, m.unsubscribe_one_click
             FROM newest n JOIN messages m ON m.id = n.id
             LEFT JOIN verdicts v ON v.message_key = m.message_key
             WHERE {CAN_UNSUBSCRIBE}
             ORDER BY n.last_date DESC"
        ))?;
        let rows = st.query_map([], |r| {
            let link: String = r.get(8)?;
            Ok(Newsletter {
                address: r.get(0)?,
                name: r.get(1)?,
                account_id: r.get(2)?,
                folder_id: r.get(3)?,
                threads: r.get(4)?,
                last_date: r.get(5)?,
                subject: r.get(6)?,
                message_id: r.get(7)?,
                how: way_out_kind(&link, r.get(9)?).to_string(),
                left: None,
                still_sending: false,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Conversations with someone, wherever they've been filed: what they sent, and what was
    /// sent to them. Newest first, in one inbox or in all of them. Newsletters and Contacts
    /// both show a person's mail with this, so the two always agree.
    pub fn mail_with(
        &self,
        account_id: Option<i64>,
        addresses: &[String],
    ) -> Result<Vec<ThreadSummary>> {
        let them = serde_json::to_string(addresses).unwrap_or_else(|_| "[]".into());
        let threads: Vec<(i64, String)> = {
            let db = self.reading_db();
            let mut st = db.prepare(&format!(
                "WITH them AS (SELECT lower(value) AS address FROM json_each(?2)),
                 hits AS (
                     SELECT m.account_id, m.thread_id, m.date FROM messages m INDEXED BY messages_from_addr
                     WHERE m.from_addr COLLATE NOCASE IN (SELECT address FROM them)
                       AND m.screened = 0 AND m.folder_id NOT IN {NOT_THEIR_MAIL}
                       AND (?1 IS NULL OR m.account_id = ?1)
                     UNION ALL
                     SELECT m.account_id, m.thread_id, m.date
                     FROM messages m JOIN folders f ON f.id = m.folder_id,
                          json_each(json_array(json(m.to_json), json(m.cc_json))) l, json_each(l.value) j
                     WHERE f.role = 'sent' AND (?1 IS NULL OR m.account_id = ?1)
                       AND lower(json_extract(j.value, '$.address')) IN (SELECT address FROM them)
                 )
                 SELECT account_id, thread_id FROM hits
                 GROUP BY account_id, thread_id ORDER BY MAX(date) DESC LIMIT 500"
            ))?;
            let rows = st.query_map(params![account_id, them], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for (account, thread) in threads {
            if let Some(row) = self.summary_of(account, &thread)? {
                out.push(row);
            }
        }
        Ok(out)
    }
}

impl Engine {
    /// Everyone from `newsletter_rows`, marked if they've been left, and those left whose
    /// mail has since gone, because it's still worth knowing they were. Only senders with a
    /// way to unsubscribe are newsletters: security alerts and receipts have none.
    pub(crate) fn newsletters(&self) -> Result<Vec<Newsletter>> {
        let mut rows = self.store.newsletter_rows()?;
        let accounts = self.store.list_accounts()?;
        for (key, value) in self.store.all_settings()? {
            let Some((email, address)) = key
                .strip_prefix(UNSUBSCRIBED)
                .and_then(|k| k.split_once(' '))
            else {
                continue;
            };
            let Some(account) = accounts
                .iter()
                .find(|a| a.email.eq_ignore_ascii_case(email))
            else {
                continue;
            };
            let Ok(left) = serde_json::from_str::<Left>(&value) else {
                continue;
            };
            match rows
                .iter_mut()
                .find(|r| r.account_id == account.id && r.address == address)
            {
                Some(row) => row.left = Some(left),
                None => rows.push(Newsletter {
                    address: address.to_string(),
                    name: left.name.clone(),
                    account_id: account.id,
                    folder_id: 0,
                    threads: 0,
                    last_date: 0,
                    subject: String::new(),
                    message_id: None,
                    how: String::new(),
                    left: Some(left),
                    still_sending: false,
                }),
            }
        }
        for row in &mut rows {
            row.still_sending = row
                .left
                .as_ref()
                .is_some_and(|l| row.last_date > l.at + IGNORED_AFTER_SECONDS);
        }
        Ok(rows)
    }

    /// The senders this inbox unsubscribed from, and when.
    pub(crate) fn left_senders(&self, account_email: &str) -> Result<HashMap<String, i64>> {
        let prefix = format!("{UNSUBSCRIBED}{} ", account_email.to_lowercase());
        let mut out = HashMap::new();
        for (key, value) in self.store.all_settings()? {
            let Some(address) = key.strip_prefix(&prefix) else {
                continue;
            };
            if let Ok(left) = serde_json::from_str::<Left>(&value) {
                out.insert(address.to_string(), left.at);
            }
        }
        Ok(out)
    }

    /// Lets a sender back in: they're no longer counted as left, and their new mail stays
    /// in the inbox.
    pub(crate) async fn forget_left(&self, account_id: i64, address: &str) -> Result<()> {
        let account = self.store.account(account_id)?;
        self.store.delete_setting(&format!(
            "{UNSUBSCRIBED}{} {}",
            account.email.to_lowercase(),
            address.to_lowercase()
        ))?;
        self.settings_changed().await
    }

    pub(crate) async fn remember_left(
        &self,
        account_id: i64,
        address: &str,
        name: &str,
        how: &str,
    ) -> Result<()> {
        let account = self.store.account(account_id)?;
        let left = Left {
            at: crate::identity::unix_now(),
            how: how.to_string(),
            name: name.to_string(),
        };
        self.store.set_setting(
            &format!(
                "{UNSUBSCRIBED}{} {}",
                account.email.to_lowercase(),
                address.to_lowercase()
            ),
            &json!(left).to_string(),
        )?;
        self.settings_changed().await
    }

    /// Sends what the list asks for from this inbox, and leaves no draft behind if it can't.
    // ponytail: the sender chooses where this goes and what it says, as with every mail app's
    // unsubscribe. Suspect mail never gets here. Asking which domains may be written to would
    // close the rest, but real lists often use their mail service's domain.
    pub(crate) async fn send_unsubscribe_mail(&self, account_id: i64, link: &str) -> Result<()> {
        let (to, subject, body) = read_mailto(link).ok_or_else(|| {
            CoreError::Invalid("This sender doesn't offer a safe way to unsubscribe.".into())
        })?;
        let mut draft = self.draft_new(account_id, "new", None, None).await?;
        draft.to = vec![Addr {
            name: String::new(),
            address: to,
        }];
        draft.subject = subject;
        // Not the person's signature, which a list's robot has no use for.
        draft.html = escape_html(&body);
        draft.text = body;
        self.store.update_draft(&draft)?;
        let sent = self.send_draft(draft.id, false).await;
        if sent.is_err() {
            self.draft_discard(draft.id)?;
        }
        sent.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mailto_link_counts_only_for_one_address_a_subject_and_a_body() {
        let read = |link: &str| read_mailto(link);
        assert_eq!(
            read("mailto:leave@x.test?subject=Stop%20please&body=list%20one&cc=evil@x.test"),
            Some((
                "leave@x.test".to_string(),
                "Stop please".to_string(),
                "list one".to_string()
            ))
        );
        assert_eq!(
            read("mailto:leave@x.test?subject="),
            Some((
                "leave@x.test".to_string(),
                "Unsubscribe".to_string(),
                "Unsubscribe".to_string()
            )),
            "something still has to say what it's about"
        );
        assert_eq!(read("mailto:a@x.test,b@x.test"), None, "one address only");
        assert_eq!(read("mailto:"), None);
        assert_eq!(read("https://x.test/leave"), None);
    }

    #[test]
    fn a_sender_whose_newest_email_looks_suspect_isnt_offered() {
        use crate::model::Security;
        use crate::parse::ParsedHeaders;
        use crate::spam::Signals;
        use crate::store::IncomingMessage;

        let s = Store::open_in_memory().unwrap();
        let account = s
            .insert_account("me@x.test", "Me", "x.test", 993, Security::Tls, "me")
            .unwrap()
            .id;
        let folder = s
            .replace_folders(account, &[("INBOX".into(), "Inbox".into(), "inbox".into())])
            .unwrap()[0]
            .0;
        let email = |uid: u32, address: &str| IncomingMessage {
            uid,
            is_draft: false,
            seen: true,
            flagged: false,
            size: 10,
            internal_date: Some(uid as i64),
            signals: Some(Signals {
                unsubscribe: Some("https://x.test/out".into()),
                ..Default::default()
            }),
            headers: ParsedHeaders {
                rfc_id: Some(format!("{uid}@x.test")),
                subject: format!("Issue {uid}"),
                from: Some(Addr {
                    name: String::new(),
                    address: address.into(),
                }),
                ..Default::default()
            },
        };
        s.insert_messages(
            account,
            folder,
            &[
                email(1, "fine@x.test"),
                email(2, "spam@x.test"),
                email(3, "me@x.test"),
            ],
        )
        .unwrap();
        let listed = || -> Vec<String> {
            s.newsletter_rows()
                .unwrap()
                .into_iter()
                .map(|r| r.address)
                .collect()
        };
        assert_eq!(
            listed(),
            ["spam@x.test", "fine@x.test"],
            "and never oneself"
        );

        let key: String = s
            .db()
            .query_row("SELECT message_key FROM messages WHERE uid = 2", [], |r| {
                r.get(0)
            })
            .unwrap();
        s.save_verdict(&key, 90, 2, &[]).unwrap();
        assert_eq!(listed(), ["fine@x.test"]);

        // An email offers Unsubscribe exactly when its sender is listed here.
        let offers = |uid: u32| {
            let id: i64 = s
                .db()
                .query_row("SELECT id FROM messages WHERE uid = ?1", [uid], |r| {
                    r.get(0)
                })
                .unwrap();
            s.message_detail(id).unwrap().can_unsubscribe
        };
        assert_eq!([offers(1), offers(2), offers(3)], [true, false, false]);
    }

    #[test]
    fn a_checked_sender_with_its_own_unsubscribe_link_in_the_email_is_listed() {
        use crate::model::{Body, Security};
        use crate::parse::ParsedHeaders;
        use crate::spam::Signals;
        use crate::store::IncomingMessage;

        let s = Store::open_in_memory().unwrap();
        let account = s
            .insert_account("me@x.test", "Me", "x.test", 993, Security::Tls, "me")
            .unwrap()
            .id;
        let folder = s
            .replace_folders(account, &[("INBOX".into(), "Inbox".into(), "inbox".into())])
            .unwrap()[0]
            .0;
        let email = |uid: u32, address: &str, auth: &str| IncomingMessage {
            uid,
            is_draft: false,
            seen: true,
            flagged: false,
            size: 10,
            internal_date: Some(uid as i64),
            signals: Some(Signals {
                auth: auth.into(),
                ..Default::default()
            }),
            headers: ParsedHeaders {
                rfc_id: Some(format!("{uid}@x.test")),
                subject: format!("Digest {uid}"),
                from: Some(Addr {
                    name: String::new(),
                    address: address.into(),
                }),
                ..Default::default()
            },
        };
        s.insert_messages(
            account,
            folder,
            &[
                email(1, "hello@inspectlet.com", "pass"),
                email(2, "hello@unchecked.test", "none"),
            ],
        )
        .unwrap();
        let footer = |site: &str| Body {
            text: None,
            html: Some(format!(
                "<p>Hi</p><a href=\"https://{site}/unsubscribe?t=1\">Unsubscribe</a>"
            )),
            attachments: Vec::new(),
            pgp: None,
        };
        s.save_body_by_uid(folder, 1, &footer("hn.inspectlet.com"))
            .unwrap();
        s.save_body_by_uid(folder, 2, &footer("unchecked.test"))
            .unwrap();
        let listed = || -> Vec<(String, String)> {
            s.newsletter_rows()
                .unwrap()
                .into_iter()
                .map(|r| (r.address, r.how))
                .collect()
        };
        let inspectlet = vec![("hello@inspectlet.com".to_string(), "page".to_string())];
        assert_eq!(
            listed(),
            inspectlet,
            "the one whose sender passed the check"
        );

        // Reading its headers again, which have no link, keeps the one from the email.
        s.save_signals(
            folder,
            1,
            &Signals {
                auth: "pass".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(listed(), inspectlet);

        // Mail saved before links were read from the email is gone through after a refresh.
        s.insert_messages(account, folder, &[email(3, "news@older.test", "pass")])
            .unwrap();
        s.db()
            .execute(
                "INSERT INTO bodies (message_id, html, attachments_json)
                 SELECT id, '<a href=\"https://older.test/out\">Unsubscribe</a>', '[]'
                 FROM messages WHERE uid = 3",
                [],
            )
            .unwrap();
        assert_eq!(listed(), inspectlet);
        s.fill_some_ways_out(300).unwrap();
        assert_eq!(
            listed()[0],
            ("news@older.test".to_string(), "page".to_string())
        );
    }

    #[test]
    fn each_kind_of_link_is_named_for_what_a_click_will_do() {
        assert_eq!(way_out_kind("mailto:a@x.test", true), "mail");
        assert_eq!(way_out_kind("https://x.test", true), "one_click");
        assert_eq!(way_out_kind("https://x.test", false), "page");
    }
}
