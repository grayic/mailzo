//! Turns raw mail bytes into the fields we store.

use crate::model::{Addr, AttachmentInfo, Body};
use data_encoding::BASE64;
use mail_parser::{AddressList, MessageParser};

/// Images shown inside a message are kept with it, up to this size per message,
/// so they work offline and with no request to anyone's server.
const MAX_EMBEDDED_BYTES: usize = 3 * 1024 * 1024;
const EMBEDDABLE: &[&str] = &["png", "jpeg", "jpg", "gif", "webp"];

/// The parts of a message's header that the engine keeps.
#[derive(Debug, Clone, Default)]
pub struct ParsedHeaders {
    pub rfc_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub from: Option<Addr>,
    pub reply_to: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub date: Option<i64>,
}

pub fn parse_headers(raw: &[u8]) -> ParsedHeaders {
    let Some(m) = MessageParser::default().parse_headers(raw) else {
        return ParsedHeaders::default();
    };
    ParsedHeaders {
        rfc_id: m.message_id().map(str::to_string),
        in_reply_to: m
            .in_reply_to()
            .map(|l| l.iter().map(str::to_string).collect())
            .unwrap_or_default(),
        references: m
            .references()
            .map(|l| l.iter().map(str::to_string).collect())
            .unwrap_or_default(),
        subject: m.subject().unwrap_or("").to_string(),
        from: m.from().as_ref().and_then(|l| addrs(l).into_iter().next()),
        reply_to: m.reply_to().as_ref().map(addrs).unwrap_or_default(),
        to: m.to().as_ref().map(addrs).unwrap_or_default(),
        cc: m.cc().as_ref().map(addrs).unwrap_or_default(),
        bcc: m.bcc().as_ref().map(addrs).unwrap_or_default(),
        date: m.date().map(|d| d.to_timestamp()),
    }
}

fn addrs(list: &AddressList<'_>) -> Vec<Addr> {
    list.mailboxes()
        .map(|m| Addr {
            name: m.name().unwrap_or("").to_string(),
            address: m.address().unwrap_or("").to_string(),
        })
        .collect()
}

pub fn parse_body(raw: &[u8]) -> Body {
    let Some(m) = MessageParser::default().parse(raw) else {
        return Body {
            text: Some(String::from_utf8_lossy(raw).into_owned()),
            html: None,
            attachments: vec![],
            pgp: None,
        };
    };
    // The parser builds HTML for plain-text mail. Keep only HTML the sender wrote,
    // so plain mail can follow the app's own theme.
    let has_real_html = m.html_body().any(|p| p.is_content_type("text", "html"));
    let mut html = has_real_html
        .then(|| m.body_html(0).map(|c| c.into_owned()))
        .flatten();
    let mut budget = MAX_EMBEDDED_BYTES;
    let attachments = m
        .attachments()
        .enumerate()
        .map(|(i, p)| {
            let (ctype, subtype) = p
                .content_type()
                .map(|c| (c.ctype().to_string(), c.subtype().unwrap_or("").to_string()))
                .unwrap_or_default();
            let mut inline = false;
            if let (Some(doc), Some(cid)) = (html.as_mut(), p.content_id()) {
                let needle = format!("cid:{}", cid.trim_matches(['<', '>']));
                let image = ctype.eq_ignore_ascii_case("image")
                    && EMBEDDABLE.iter().any(|t| subtype.eq_ignore_ascii_case(t));
                if image && doc.contains(&needle) && p.decoded_len() <= budget {
                    budget -= p.decoded_len();
                    let data = format!(
                        "data:image/{};base64,{}",
                        subtype.to_ascii_lowercase(),
                        BASE64.encode(p.decoded().as_ref())
                    );
                    *doc = doc.replace(&needle, &data);
                    inline = true;
                }
            }
            AttachmentInfo {
                name: p.attachment_name().unwrap_or("attachment").to_string(),
                mime: if ctype.is_empty() {
                    "application/octet-stream".into()
                } else {
                    format!(
                        "{ctype}/{}",
                        if subtype.is_empty() {
                            "octet-stream"
                        } else {
                            &subtype
                        }
                    )
                },
                size: p.decoded_len() as u64,
                part: Some(i as u32),
                inline,
            }
        })
        .collect();
    Body {
        text: m.body_text(0).map(|c| c.into_owned()),
        html,
        attachments,
        pgp: None,
    }
}

/// The calendar part of an email, when it's sent inline and not as an attachment.
pub fn calendar_part(raw: &[u8]) -> Option<String> {
    let m = MessageParser::default().parse(raw)?;
    let found = m
        .parts()
        .find(|p| p.is_content_type("text", "calendar"))
        .map(|p| String::from_utf8_lossy(p.decoded().as_ref()).to_string());
    found
}

/// One attachment's name, type and bytes, by its position in the message.
pub fn attachment_part(raw: &[u8], part: u32) -> Option<(String, String, Vec<u8>)> {
    let m = MessageParser::default().parse(raw)?;
    let p = m.attachments().nth(part as usize)?;
    let mime = p
        .content_type()
        .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
        .unwrap_or_else(|| "application/octet-stream".into());
    Some((
        p.attachment_name().unwrap_or("attachment").to_string(),
        mime,
        p.decoded().to_vec(),
    ))
}

/// A one-line preview of the message for the thread list.
pub fn snippet(text: &str) -> String {
    let text = without_tags(text);
    let joined = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('>'))
        .collect::<Vec<_>>()
        .join(" ")
        // A link's address says nothing, and a long one can take up the whole preview.
        .split_whitespace()
        .filter(|w| {
            let w = w.trim_start_matches(['<', '(', '[']);
            !(w.starts_with("http://") || w.starts_with("https://"))
        })
        .collect::<Vec<_>>()
        .join(" ");
    // Long enough to reach a sign-in code after a greeting and a sentence.
    joined.chars().take(300).collect()
}

/// HTML that some sending tools put in the plain-text part, which would show as code.
fn without_tags(text: &str) -> String {
    const TAGS: &[&str] = &[
        "p", "div", "span", "br", "a", "b", "i", "u", "strong", "em", "table", "tr", "td", "th",
        "tbody", "ul", "ol", "li", "h1", "h2", "h3", "h4", "img", "font", "html", "body", "head",
        "style", "center", "hr",
    ];
    if !text.contains('<') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let name: String = after
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match after.find('>') {
            Some(end) if TAGS.contains(&name.as_str()) => {
                out.push(' ');
                rest = &after[end + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Removes "Re:" and "Fwd:" so a thread shows its real subject.
pub fn clean_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let stripped = ["re:", "fw:", "fwd:"]
            .iter()
            .find(|p| lower.starts_with(**p))
            .map(|p| s[p.len()..].trim_start());
        match stripped {
            Some(rest) => s = rest,
            None => return s.to_string(),
        }
    }
}

/// The subject with reply prefixes removed and case ignored, for comparing.
pub fn base_subject(subject: &str) -> String {
    clean_subject(subject).to_lowercase()
}

/// Whether the sender's own mail app marked this as a reply.
pub fn is_reply_subject(subject: &str) -> bool {
    subject.trim_start().to_ascii_lowercase().starts_with("re:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_images_are_kept_with_the_message_and_not_listed() {
        let raw = b"Subject: a\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=b\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:pic@x\"><img src=\"cid:gone@x\">\r\n--b\r\nContent-Type: image/png\r\nContent-ID: <pic@x>\r\nContent-Disposition: inline; filename=a.png\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b--\r\n";
        let body = parse_body(raw);
        let html = body.html.unwrap();
        assert!(html.contains("data:image/png;base64,aGVsbG8="), "{html}");
        assert!(html.contains("cid:gone@x"));
        assert!(body.attachments[0].inline);
    }

    #[test]
    fn attachments_can_be_read_back_by_position() {
        let raw = b"Subject: a\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nHi\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"plan.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b--\r\n";
        let body = parse_body(raw);
        assert_eq!(body.attachments[0].name, "plan.pdf");
        assert_eq!(body.attachments[0].size, 5);
        assert!(!body.attachments[0].inline);
        let (name, mime, bytes) = attachment_part(raw, 0).unwrap();
        assert_eq!(
            (name.as_str(), mime.as_str(), bytes.as_slice()),
            ("plan.pdf", "application/pdf", b"hello".as_slice())
        );
        assert!(attachment_part(raw, 1).is_none());
    }

    #[test]
    fn reads_headers_and_decodes_encoded_words() {
        let raw = b"From: =?UTF-8?Q?Ren=C3=A9e?= <r@example.test>\r\nTo: a@example.test, B <b@example.test>\r\nSubject: =?UTF-8?B?4pi6IGhp?=\r\nMessage-ID: <x@y>\r\nReferences: <a@y> <b@y>\r\nIn-Reply-To: <b@y>\r\nDate: Sat, 20 Nov 2021 14:22:01 -0800\r\n\r\n";
        let h = parse_headers(raw);
        assert_eq!(h.rfc_id.as_deref(), Some("x@y"));
        assert_eq!(h.references, ["a@y", "b@y"]);
        assert_eq!(h.in_reply_to, ["b@y"]);
        assert_eq!(h.subject, "☺ hi");
        assert_eq!(h.from.unwrap().name, "Renée");
        assert_eq!(h.to.len(), 2);
        assert_eq!(h.date, Some(1637446921));
    }

    #[test]
    fn garbage_does_not_panic() {
        let h = parse_headers(b"\xff\xfe not a header at all");
        assert!(h.rfc_id.is_none());
        let b = parse_body(b"\xff\xfe");
        assert!(b.attachments.is_empty());
    }

    #[test]
    fn plain_mail_has_no_html_and_html_mail_has_both() {
        let plain = parse_body(b"Subject: a\r\nContent-Type: text/plain\r\n\r\nHello\r\n");
        assert!(plain.html.is_none());
        assert_eq!(plain.text.unwrap().trim(), "Hello");

        let html = parse_body(b"Subject: a\r\nContent-Type: text/html\r\n\r\n<p>Hello</p>\r\n");
        assert!(html.html.unwrap().contains("<p>Hello</p>"));
        assert_eq!(html.text.unwrap().trim(), "Hello");
    }

    #[test]
    fn snippet_leaves_out_html_tags() {
        let s = snippet("compare <p style=\"margin: 0 0 14px 0;\">bring a job</p> to <ana@x.test>");
        assert_eq!(s, "compare bring a job to <ana@x.test>");
    }

    #[test]
    fn snippet_skips_quotes_and_blank_lines() {
        assert_eq!(snippet("> old\n\n  Hello \n there\n"), "Hello there");
        assert_eq!(snippet(&"é".repeat(500)).chars().count(), 300);
    }

    /// A link's address says nothing, and a long one pushed the sign-in code out of the preview.
    #[test]
    fn a_preview_leaves_out_link_addresses() {
        let long = format!("https://mandrillapp.com/track/click/{}", "x".repeat(260));
        let text = format!(
            "One-time verification code <{long}>\nHere is your one-time login code. This code is \
             valid for 15 minutes only:\n644122\nSee {long} for help."
        );
        let preview = snippet(&text);
        assert!(!preview.contains("http"), "{preview}");
        assert!(preview.contains("644122"), "{preview}");
        assert_eq!(
            crate::codes::one_time_code("Your authentication code", &preview).as_deref(),
            Some("644122")
        );
    }

    #[test]
    fn clean_subject_strips_reply_prefixes() {
        assert_eq!(clean_subject("Re: FWD: re: Plan"), "Plan");
        assert_eq!(clean_subject("Regarding plans"), "Regarding plans");
    }
}
