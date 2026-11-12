//! Every file and link shared in one conversation, gathered in one place. No AI.

use crate::engine::Engine;
use crate::error::Result;
use crate::model::{Body, MessageDetail};
use serde::Serialize;
use std::collections::HashSet;

/// Enough for any real conversation, and a cap on a newsletter full of links.
const MOST_LINKS: usize = 200;
const LONGEST_LINK_TEXT: usize = 120;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SharedFile {
    pub message_id: i64,
    /// Which attachment of that email, for saving it.
    pub part: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub date: i64,
    pub from: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SharedLink {
    pub url: String,
    /// The words the link was on, if they say more than the address.
    pub text: String,
    pub host: String,
    pub message_id: i64,
    pub date: i64,
    pub from: String,
    /// From an email that looked suspicious, so it's shown with a warning.
    pub risky: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Shared {
    pub files: Vec<SharedFile>,
    pub links: Vec<SharedLink>,
}

/// Links people don't mean to share: leaving a mailing list, settings, the email in a browser.
fn housekeeping(url: &str, text: &str) -> bool {
    let both = format!("{} {}", url.to_lowercase(), text.to_lowercase());
    [
        "unsubscribe",
        "opt-out",
        "optout",
        "email preferences",
        "manage preferences",
        "view in browser",
        "view this email",
        "view online",
    ]
    .iter()
    .any(|w| both.contains(w))
}

fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?.split(':').next()?;
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host.trim_start_matches("www.").to_lowercase())
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

fn without_tags(html: &str) -> String {
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
    decode_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The links in an email's HTML, with the words each sits on.
pub(crate) fn links_in_html(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lower = html.to_lowercase();
    let mut at = 0;
    while let Some(found) = lower[at..].find("<a ") {
        let start = at + found;
        let Some(tag_end) = lower[start..].find('>').map(|e| start + e) else {
            break;
        };
        let tag = &html[start..tag_end];
        let close = lower[tag_end..]
            .find("</a")
            .map(|e| tag_end + e)
            .unwrap_or(tag_end + 1);
        at = close.max(tag_end + 1);
        let tag_lower = tag.to_lowercase();
        let Some(href_at) = tag_lower.find("href=") else {
            continue;
        };
        let value = &tag[href_at + 5..];
        let url = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split([' ', '>']).next().unwrap_or(""),
        };
        let url = decode_entities(url.trim());
        let words = without_tags(&html[(tag_end + 1).min(close)..close]);
        out.push((url, words));
    }
    out
}

/// Web addresses written out in plain text.
pub(crate) fn links_in_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')) {
        let Some(start) = word.find("http://").or_else(|| word.find("https://")) else {
            continue;
        };
        let url = word[start..].trim_end_matches(['.', ',', ')', ']', ';', ':', '!', '?', '\'']);
        out.push(url.to_string());
    }
    out
}

fn sender(m: &MessageDetail) -> String {
    if m.from.name.is_empty() {
        m.from.address.clone()
    } else {
        m.from.name.clone()
    }
}

/// Gathers files and links, oldest email first so a link is credited to whoever shared it first.
pub(crate) fn gather(mail: &[(MessageDetail, Body)]) -> Shared {
    let mut shared = Shared::default();
    let mut seen_links = HashSet::new();
    for (m, body) in mail {
        for (i, a) in body.attachments.iter().enumerate() {
            if a.inline {
                continue;
            }
            shared.files.push(SharedFile {
                message_id: m.id,
                part: a.part.unwrap_or(i as u32),
                name: a.name.clone(),
                mime: a.mime.clone(),
                size: a.size,
                date: m.date,
                from: sender(m),
            });
        }
        let found: Vec<(String, String)> = match &body.html {
            Some(html) if !html.trim().is_empty() => links_in_html(html),
            _ => links_in_text(body.text.as_deref().unwrap_or(""))
                .into_iter()
                .map(|u| (u, String::new()))
                .collect(),
        };
        for (url, words) in found {
            if shared.links.len() >= MOST_LINKS {
                break;
            }
            let Some(host) = host_of(&url) else {
                continue;
            };
            if housekeeping(&url, &words) || !seen_links.insert(url.clone()) {
                continue;
            }
            // Words that are just the address again, or an image, add nothing.
            let text = if words == url || words.is_empty() {
                String::new()
            } else {
                words.chars().take(LONGEST_LINK_TEXT).collect()
            };
            shared.links.push(SharedLink {
                url,
                text,
                host,
                message_id: m.id,
                date: m.date,
                from: sender(m),
                risky: m.risk > 0,
            });
        }
    }
    // Newest first, which is what people are usually after.
    shared.files.reverse();
    shared.links.reverse();
    shared
}

impl Engine {
    pub(crate) async fn thread_shared(&self, account_id: i64, thread_id: &str) -> Result<Shared> {
        let thread = self.store.real_thread(thread_id)?;
        let mut mail = self.store.get_thread(account_id, &thread, None)?;
        mail.sort_by_key(|m| m.date);
        let mut with_bodies = Vec::new();
        let mut seen = HashSet::new();
        for m in mail {
            // The same email in two folders counts once.
            if let Some(rfc) = &m.rfc_id {
                if !seen.insert(rfc.clone()) {
                    continue;
                }
            }
            let body = match m.body.clone() {
                Some(b) => b,
                // An email whose words can't be fetched now still shows the rest.
                None => match self.fetch_body(m.id).await {
                    Ok(b) => b,
                    Err(_) => continue,
                },
            };
            with_bodies.push((m, body));
        }
        Ok(gather(&with_bodies))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_come_with_their_words_and_housekeeping_is_left_out() {
        let html = r##"<p>See <a href="https://docs.example.com/brief?id=1&amp;v=2">the <b>brief</b></a>
            and <A HREF='https://figma.com/file/abc'>https://figma.com/file/abc</A>.
            <a href="mailto:ana@x.test">Ana</a> <a href="#top">top</a>
            <a href="https://news.example.com/u?x=1">Unsubscribe</a></p>"##;
        let found = links_in_html(html);
        assert_eq!(found[0].0, "https://docs.example.com/brief?id=1&v=2");
        assert_eq!(found[0].1, "the brief");
        assert_eq!(found[1].0, "https://figma.com/file/abc");
        assert!(housekeeping(&found[4].0, &found[4].1));
        assert_eq!(host_of("mailto:ana@x.test"), None);
        assert_eq!(host_of("#top"), None);
        assert_eq!(
            host_of("https://www.Example.com:8080/a").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn plain_text_links_lose_the_punctuation_after_them() {
        assert_eq!(
            links_in_text("Here: https://a.test/x. And (https://b.test/y), done"),
            vec!["https://a.test/x", "https://b.test/y"]
        );
    }
}
