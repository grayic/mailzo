//! Writing and sending: drafts, replies, forwards, attachments.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::imap::ImapConnection;
use crate::model::*;
use crate::parse;
use crate::smtp;
use data_encoding::BASE64;
use lettre::message::header::{ContentDisposition, ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::{Address, Message};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// What most mail providers accept. Larger mail tends to bounce days later.
const MAX_MESSAGE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_REFERENCES: usize = 20;
const QUOTE_STYLE: &str =
    "margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex;color:#555";

fn body_style(font: &str, size: &str) -> String {
    let family = match font {
        "arial" => "Arial,Helvetica,sans-serif",
        "georgia" => "Georgia,'Times New Roman',serif",
        "times" => "'Times New Roman',Times,serif",
        "courier" => "'Courier New',Courier,monospace",
        _ => "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif",
    };
    format!("font-family:{family};font-size:{size}px;line-height:1.5;color:#18181b")
}

pub(crate) fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Removes anything that can run or act on its own, and keeps the look.
pub(crate) fn clean_html(html: &str) -> String {
    let mut schemes = std::collections::HashSet::new();
    schemes.extend(["http", "https", "mailto", "data", "cid", "tel"]);
    let mut tags = std::collections::HashSet::new();
    tags.extend(["font", "center"]);
    let mut attrs = std::collections::HashSet::new();
    attrs.extend([
        "style",
        "align",
        "valign",
        "bgcolor",
        "width",
        "height",
        "dir",
        "color",
        "face",
        "size",
        "border",
        "cellpadding",
        "cellspacing",
        "colspan",
        "rowspan",
    ]);
    // The usual mark on a quoted earlier message, so mail apps can fold it.
    let quote_mark = std::collections::HashSet::from(["gmail_quote"]);
    let classes =
        std::collections::HashMap::from([("div", quote_mark.clone()), ("blockquote", quote_mark)]);
    ammonia::Builder::default()
        .add_tags(tags)
        .add_generic_attributes(attrs)
        .allowed_classes(classes)
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer"))
        .clean(html)
        .to_string()
}

fn addr_text(a: &Addr) -> String {
    if a.name.is_empty() {
        a.address.clone()
    } else {
        format!("{} <{}>", a.name, a.address)
    }
}

fn addr_list_text(list: &[Addr]) -> String {
    list.iter().map(addr_text).collect::<Vec<_>>().join(", ")
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let clean = parse::clean_subject(subject);
    if prefix == "Re:" && parse::is_reply_subject(subject) {
        subject.trim().to_string()
    } else {
        format!("{prefix} {clean}").trim().to_string()
    }
}

/// Who a reply goes to. Never includes the person's own address, unless they are
/// replying to mail they sent, when it goes back to the same people.
fn reply_recipients(parent: &MessageDetail, own: &[&str], all: bool) -> (Vec<Addr>, Vec<Addr>) {
    let is_mine = |a: &Addr| own.iter().any(|o| a.address.eq_ignore_ascii_case(o));
    let sent_by_me = is_mine(&parent.from);

    let mut to: Vec<Addr> = if sent_by_me {
        parent.to.clone()
    } else if !parent.reply_to.is_empty() {
        parent.reply_to.clone()
    } else {
        vec![parent.from.clone()]
    };
    let mut cc: Vec<Addr> = Vec::new();
    if all {
        if !sent_by_me {
            to.extend(parent.to.iter().cloned());
        }
        cc.extend(parent.cc.iter().cloned());
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut keep = |a: &Addr| {
        !a.address.is_empty() && !is_mine(a) && seen.insert(a.address.to_ascii_lowercase())
    };
    to.retain(&mut keep);
    cc.retain(&mut keep);
    (to, cc)
}

/// The addresses that count as the person's own in this inbox.
fn own_addresses(account: &Account) -> Vec<&str> {
    std::iter::once(account.email.as_str())
        .chain(account.prefs.aliases.iter().map(|a| a.address.as_str()))
        .collect()
}

/// Pulls images embedded in the HTML out into proper inline parts, because many
/// mail apps refuse to show them otherwise. Returns the HTML and the images.
fn extract_inline_images(html: &str, domain: &str) -> (String, Vec<(String, String, Vec<u8>)>) {
    const MARK: &str = "src=\"data:image/";
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(MARK) {
        let after = &rest[start + MARK.len()..];
        let parsed = after.find('"').and_then(|end| {
            let (kind, data) = after[..end].split_once(";base64,")?;
            let bytes = BASE64.decode(data.as_bytes()).ok()?;
            let safe = ["png", "jpeg", "jpg", "gif", "webp"].contains(&kind);
            safe.then(|| (end, kind.to_string(), bytes))
        });
        match parsed {
            Some((end, kind, bytes)) => {
                let cid = format!("{}@{domain}", crate::crypto::random_hex(12));
                out.push_str(&rest[..start]);
                out.push_str(&format!("src=\"cid:{cid}\""));
                images.push((cid, format!("image/{kind}"), bytes));
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[..start + MARK.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, images)
}

fn content_type(mime: &str) -> ContentType {
    ContentType::parse(mime).unwrap_or(ContentType::parse("application/octet-stream").unwrap())
}

/// A file name that is safe to create: no folders, no hidden files.
pub(crate) fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base
        .chars()
        // Hidden direction marks too, so "invoice\u{202E}fdp.zip" can't pass for a PDF.
        .filter(|c| {
            !c.is_control()
                && !matches!(c, ':' | '/' | '\\')
                && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    if clean.is_empty() {
        "attachment".to_string()
    } else {
        clean.chars().take(200).collect()
    }
}

fn mailboxes(list: &[Addr]) -> Result<Vec<Mailbox>> {
    list.iter()
        .map(|a| {
            let address: Address = a.address.trim().parse().map_err(|_| CoreError::Send {
                code: "bad_address".into(),
                message: format!("\"{}\" isn't a valid email address.", a.address),
            })?;
            let name = (!a.name.trim().is_empty()).then(|| a.name.trim().to_string());
            Ok(Mailbox::new(name, address))
        })
        .collect()
}

impl Engine {
    fn draft_dir(&self, draft_id: i64) -> PathBuf {
        self.data_dir.join("drafts").join(draft_id.to_string())
    }

    pub(crate) fn remove_draft_files(&self, draft_id: i64) {
        let _ = std::fs::remove_dir_all(self.draft_dir(draft_id));
    }

    pub(crate) fn remove_all_draft_files(&self) {
        let _ = std::fs::remove_dir_all(self.data_dir.join("drafts"));
    }

    pub(crate) fn write_draft_file(
        &self,
        draft_id: i64,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<DraftAttachment> {
        let dir = self.draft_dir(draft_id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Invalid(format!("Could not save the attachment: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let id = crate::crypto::random_hex(12);
        std::fs::write(dir.join(&id), bytes)
            .map_err(|e| CoreError::Invalid(format!("Could not save the attachment: {e}")))?;
        Ok(DraftAttachment {
            id,
            name: safe_file_name(name),
            mime: mime.to_string(),
            size: bytes.len() as u64,
        })
    }

    /// Who a plain reply to this email would go to.
    pub(crate) fn reply_goes_to(&self, account_id: i64, message_id: i64) -> Result<Vec<String>> {
        let account = self.store.account(account_id)?;
        let parent = self.store.message_detail(message_id)?;
        let (to, _) = reply_recipients(&parent, &own_addresses(&account), false);
        Ok(to.into_iter().map(|a| a.address).collect())
    }

    pub(crate) async fn draft_new(
        &self,
        account_id: i64,
        mode: &str,
        message_id: Option<i64>,
        date_text: Option<&str>,
    ) -> Result<Draft> {
        let account = self.store.account(account_id)?;
        let mut draft = Draft {
            id: 0,
            account_id,
            mode: mode.to_string(),
            parent_message_id: message_id,
            from_address: String::new(),
            to: vec![],
            cc: vec![],
            bcc: vec![],
            subject: String::new(),
            html: starting_signature(&account, mode),
            text: String::new(),
            quoted_html: String::new(),
            quoted_text: String::new(),
            attachments: vec![],
            updated_at: 0,
            send_at: None,
            send_problem: None,
            remind_days: None,
            made_by_ai: false,
            encrypt: false,
            sign: false,
            from_encrypted: false,
            plain_ok: false,
        };
        let mut forward_from = None;
        match mode {
            "new" => draft.parent_message_id = None,
            "reply" | "reply_all" | "forward" => {
                let id = message_id
                    .ok_or_else(|| CoreError::Invalid("Choose a message first.".into()))?;
                let parent = self.store.message_detail(id)?;
                // Without the original's text, the reply can still be written.
                let body = self.fetch_body(id).await.ok();
                // What was sent encrypted stays encrypted: the reply starts encrypted
                // and signed, and is kept off the server's Drafts.
                if body
                    .as_ref()
                    .and_then(|b| b.pgp.as_ref())
                    .is_some_and(|p| p.encrypted)
                {
                    draft.from_encrypted = true;
                    draft.encrypt = true;
                    draft.sign = true;
                }
                let date = date_text.map(str::to_string).unwrap_or_else(|| {
                    mail_parser::DateTime::from_timestamp(parent.date).to_rfc822()
                });
                // A forward is nothing without what it forwards, so this only
                // applies to replies.
                if mode == "forward" || self.setting("quote")? == "on" {
                    let (html, text) = quote_of(&parent, body.as_ref(), mode, &date);
                    draft.quoted_html = html;
                    draft.quoted_text = text;
                }
                if mode == "forward" {
                    draft.subject = prefixed("Fwd:", &parent.subject);
                    forward_from = body;
                } else {
                    draft.subject = prefixed("Re:", &parent.subject);
                    let own = own_addresses(&account);
                    let (to, cc) = reply_recipients(&parent, &own, mode == "reply_all");
                    draft.to = to;
                    draft.cc = cc;
                    // Reply from the address the email actually came to.
                    if account.prefs.reply_from == "arrived" {
                        if let Some(alias) = account.prefs.aliases.iter().find(|alias| {
                            parent
                                .to
                                .iter()
                                .chain(&parent.cc)
                                .any(|a| a.address.eq_ignore_ascii_case(&alias.address))
                        }) {
                            draft.from_address = alias.address.clone();
                        }
                    }
                }
            }
            _ => return Err(CoreError::Invalid("Bad request.".into())),
        }
        draft.id = self.store.insert_draft(&draft)?;
        if let (Some(body), Some(id)) = (forward_from, message_id) {
            // The note above names any file that couldn't be copied.
            let wanted: Vec<u32> = body
                .attachments
                .iter()
                .enumerate()
                .filter(|(_, a)| !a.inline)
                .map(|(i, a)| a.part.unwrap_or(i as u32))
                .collect();
            if !wanted.is_empty() {
                let raw = self.readable_message(id).await?;
                for part in wanted {
                    if let Some((name, mime, bytes)) = parse::attachment_part(&raw, part) {
                        draft
                            .attachments
                            .push(self.write_draft_file(draft.id, &name, &mime, &bytes)?);
                    }
                }
                self.store.update_draft(&draft)?;
            }
        }
        self.store.draft(draft.id)
    }

    pub(crate) fn draft_save(&self, input: DraftUpdate) -> Result<Draft> {
        let mut draft = self.store.draft(input.id)?;
        // Moving a draft to another inbox must name one that exists here.
        let account = self.store.account(input.account_id)?;
        // Its copy on the old inbox's server has to go, from that inbox.
        if draft.account_id != account.id {
            if let Some(old) = self.store.draft_server_rfc(draft.id)? {
                self.store.add_draft_leftover(draft.account_id, &old)?;
                self.store.forget_server_copy(draft.id)?;
            }
        }
        draft.account_id = account.id;
        // An address only stays if this inbox can send from it.
        draft.from_address = account
            .prefs
            .aliases
            .iter()
            .find(|a| a.address.eq_ignore_ascii_case(&input.from_address))
            .map(|a| a.address.clone())
            .unwrap_or_default();
        draft.to = input.to;
        draft.cc = input.cc;
        draft.bcc = input.bcc;
        draft.subject = input.subject;
        draft.html = input.html;
        draft.text = input.text;
        if let Some(encrypt) = input.encrypt {
            draft.encrypt = encrypt;
        }
        if input.allow_plain {
            draft.plain_ok = true;
        }
        if let Some(sign) = input.sign {
            draft.sign = sign;
        }
        self.store.update_draft(&draft)?;
        self.store.draft(draft.id)
    }

    pub(crate) fn draft_discard(&self, id: i64) -> Result<()> {
        self.store.delete_draft(id)?;
        self.remove_draft_files(id);
        Ok(())
    }

    pub(crate) fn draft_add_attachment(
        &self,
        id: i64,
        name: &str,
        mime: &str,
        data: &str,
    ) -> Result<Draft> {
        let bytes = BASE64
            .decode(data.as_bytes())
            .map_err(|_| CoreError::Invalid("That file couldn't be read.".into()))?;
        let mut draft = self.store.draft(id)?;
        let used: u64 = draft.attachments.iter().map(|a| a.size).sum();
        if used + bytes.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(CoreError::Send {
                code: "too_big".into(),
                message:
                    "Attachments can add up to 25 MB. Most mail providers refuse anything larger."
                        .into(),
            });
        }
        let mime = if mime.is_empty() {
            "application/octet-stream"
        } else {
            mime
        };
        draft
            .attachments
            .push(self.write_draft_file(id, name, mime, &bytes)?);
        self.store.update_draft(&draft)?;
        self.store.draft(id)
    }

    pub(crate) fn draft_remove_attachment(&self, id: i64, attachment_id: &str) -> Result<Draft> {
        let mut draft = self.store.draft(id)?;
        if let Some(i) = draft.attachments.iter().position(|a| a.id == attachment_id) {
            let removed = draft.attachments.remove(i);
            let _ = std::fs::remove_file(self.draft_dir(id).join(removed.id));
            self.store.update_draft(&draft)?;
        }
        self.store.draft(id)
    }

    /// Builds the message. `keep_bcc` is for the saved copy: recipients must never
    /// see Bcc, but the sender should.
    pub(crate) fn build_message(
        &self,
        draft: &Draft,
        account: &Account,
        message_id: &str,
        date: SystemTime,
        keep_bcc: bool,
    ) -> Result<Message> {
        let content = self.message_content(draft, account)?;
        self.message_head(draft, account, message_id, date, keep_bcc)?
            .multipart(content)
            .map_err(|e| CoreError::Send {
                code: "bad_message".into(),
                message: format!("This message couldn't be prepared: {e}"),
            })
    }

    /// Who it's from and to, its subject and the conversation it belongs to.
    fn message_head(
        &self,
        draft: &Draft,
        account: &Account,
        message_id: &str,
        date: SystemTime,
        keep_bcc: bool,
    ) -> Result<lettre::message::MessageBuilder> {
        let alias = account
            .prefs
            .aliases
            .iter()
            .find(|a| a.address.eq_ignore_ascii_case(&draft.from_address));
        let inbox_name = (!account.display_name.is_empty()
            && !account.display_name.eq_ignore_ascii_case(&account.email))
        .then(|| account.display_name.clone());
        let from_name = match alias {
            Some(a) if !a.name.trim().is_empty() => Some(a.name.trim().to_string()),
            _ => inbox_name,
        };
        let sender = alias.map_or(account.email.as_str(), |a| a.address.as_str());
        let from_address: Address = sender.parse().map_err(|_| CoreError::Send {
            code: "bad_address".into(),
            message: format!("\"{sender}\" isn't a valid email address."),
        })?;

        let mut b = Message::builder()
            .from(Mailbox::new(from_name, from_address))
            .message_id(Some(message_id.to_string()))
            .date(date)
            .subject(draft.subject.replace(['\r', '\n'], " "));
        for m in mailboxes(&draft.to)? {
            b = b.to(m);
        }
        for m in mailboxes(&draft.cc)? {
            b = b.cc(m);
        }
        for m in mailboxes(&draft.bcc)? {
            b = b.bcc(m);
        }
        if keep_bcc {
            b = b.keep_bcc();
        }

        // Replies carry the conversation's IDs, which is what threads them.
        if draft.mode != "new" && draft.mode != "forward" {
            if let Some(parent) = draft
                .parent_message_id
                .and_then(|id| self.store.message_detail(id).ok())
            {
                if let Some(rfc) = &parent.rfc_id {
                    let ids = self.store.thread_rfc_ids(
                        parent.account_id,
                        &parent.thread_id,
                        parent.date,
                    )?;
                    let mut chain: Vec<String> = ids;
                    if !chain.contains(rfc) {
                        chain.push(rfc.clone());
                    }
                    // Keep the first and the most recent, as the standard advises.
                    if chain.len() > MAX_REFERENCES {
                        let tail = chain.split_off(chain.len() - (MAX_REFERENCES - 1));
                        chain.truncate(1);
                        chain.extend(tail);
                    }
                    let refs = chain
                        .iter()
                        .map(|r| format!("<{r}>"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    b = b.in_reply_to(format!("<{rfc}>")).references(refs);
                }
            }
        }

        // Lets the other person's mail app encrypt its replies.
        if let Some(key) = self.pgp_own_key(account.id).ok().flatten() {
            let sender = sender.to_lowercase();
            if crate::pgp::key_emails(&key.details).contains(&sender) {
                if let Some(value) = crate::pgp::autocrypt_value(&sender, &key) {
                    let name = HeaderName::new_from_ascii_str("Autocrypt");
                    b = b.raw_header(HeaderValue::dangerous_new_pre_encoded(
                        name,
                        value.clone(),
                        value,
                    ));
                }
            }
        }
        Ok(b)
    }

    /// What the person wrote, with its pictures and attachments.
    fn message_content(&self, draft: &Draft, account: &Account) -> Result<MultiPart> {
        let domain = account.email.rsplit('@').next().unwrap_or("localhost");
        let clean = email_spacing(&clean_html(&draft.html));
        let quoted = clean_html(&draft.quoted_html);
        let style = body_style(&self.setting("font")?, &self.setting("font_size")?);
        let document = format!(
            "<!doctype html><html><body><div style=\"{style}\">{clean}{quoted}</div></body></html>"
        );
        let (html, images) = extract_inline_images(&document, domain);
        let mut text = draft.text.trim_end().to_string();
        if !draft.quoted_text.is_empty() {
            text.push_str("\n\n");
            text.push_str(&draft.quoted_text);
        }

        let html_part = if images.is_empty() {
            None
        } else {
            let mut related = MultiPart::related().singlepart(SinglePart::html(html.clone()));
            for (cid, mime, bytes) in &images {
                related = related.singlepart(
                    Attachment::new_inline(cid.clone()).body(bytes.clone(), content_type(mime)),
                );
            }
            Some(related)
        };
        let alternative = MultiPart::alternative().singlepart(SinglePart::plain(text));
        let alternative = match html_part {
            Some(related) => alternative.multipart(related),
            None => alternative.singlepart(SinglePart::html(html)),
        };

        let total: u64 = draft.attachments.iter().map(|a| a.size).sum::<u64>()
            + images.iter().map(|i| i.2.len() as u64).sum::<u64>();
        if total > MAX_MESSAGE_BYTES {
            return Err(CoreError::Send {
                code: "too_big".into(),
                message: "This message is over 25 MB. Most mail providers refuse anything larger, so remove an attachment.".into(),
            });
        }

        if draft.attachments.is_empty() {
            Ok(alternative)
        } else {
            let mut mixed = MultiPart::mixed().multipart(alternative);
            for a in &draft.attachments {
                let bytes = std::fs::read(self.draft_dir(draft.id).join(&a.id)).map_err(|_| {
                    CoreError::Send {
                        code: "attachment_missing".into(),
                        message: format!("The attachment \"{}\" is no longer available. Remove it and add it again.", a.name),
                    }
                })?;
                mixed = mixed
                    .singlepart(Attachment::new(a.name.clone()).body(bytes, content_type(&a.mime)));
            }
            Ok(mixed)
        }
    }

    /// Encrypts or signs what the person wrote, as the draft asks (RFC 3156). Encrypted mail
    /// goes to everyone's key and this inbox's own, so the Sent copy can be read here too.
    fn protect(&self, draft: &Draft, account: &Account, content: MultiPart) -> Result<MultiPart> {
        if !draft.encrypt && !draft.sign {
            return Ok(content);
        }
        let Some(own) = self.pgp_own_key(account.id)? else {
            return Err(CoreError::Send {
                code: "pgp_no_key".into(),
                message: "Set up an encryption key for this inbox before sending encrypted or signed email.".into(),
            });
        };
        let entity = content.formatted();
        let kind = |t: &str| {
            ContentType::parse(t).map_err(|e| CoreError::Send {
                code: "bad_message".into(),
                message: e.to_string(),
            })
        };
        if draft.encrypt {
            let people: Vec<String> = draft
                .to
                .iter()
                .chain(&draft.cc)
                .chain(&draft.bcc)
                .map(|a| a.address.clone())
                .collect();
            let (mut keys, missing) = self.pgp_recipient_keys(account, Some(&own), &people)?;
            if !missing.is_empty() {
                return Err(CoreError::Send {
                    code: "pgp_missing_keys".into(),
                    message: format!(
                        "This can't be sent encrypted, because there's no encryption key for {}. Ask for their public key, or turn encryption off.",
                        missing.join(", ")
                    ),
                });
            }
            keys.push(own.to_public_key());
            let armored = crate::pgp::encrypt_entity(&entity, &keys, &own)?;
            return Ok(MultiPart::encrypted("application/pgp-encrypted".into())
                .singlepart(
                    SinglePart::builder()
                        .header(kind("application/pgp-encrypted")?)
                        .body(String::from("Version: 1\r\n")),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(kind("application/octet-stream; name=\"encrypted.asc\"")?)
                        .header(ContentDisposition::inline_with_name("encrypted.asc"))
                        .body(armored),
                ));
        }
        // The line break before the next boundary isn't part of what's signed.
        let signed = entity.strip_suffix(b"\r\n").unwrap_or(&entity);
        let signature = crate::pgp::sign_detached(signed, &own)?;
        Ok(
            MultiPart::signed("application/pgp-signature".into(), "pgp-sha256".into())
                .multipart(content)
                .singlepart(
                    SinglePart::builder()
                        .header(kind("application/pgp-signature; name=\"signature.asc\"")?)
                        .header(ContentDisposition::attachment("signature.asc"))
                        .body(signature),
                ),
        )
    }

    /// The answer to a calendar invitation, in the form calendars understand.
    pub(crate) async fn send_invite_reply(
        &self,
        account: &Account,
        me: &str,
        organizer: &str,
        event: &crate::ics::IcsEvent,
        status: &str,
    ) -> Result<()> {
        let word = match status {
            "ACCEPTED" => "Accepted",
            "TENTATIVE" => "Maybe",
            _ => "Declined",
        };
        let from: Address = me.parse().map_err(|_| CoreError::Send {
            code: "bad_message".into(),
            message: "This inbox's address isn't valid.".into(),
        })?;
        let to: Address = organizer.parse().map_err(|_| CoreError::Send {
            code: "bad_message".into(),
            message: "The organiser's address isn't valid.".into(),
        })?;
        let name = (!account.display_name.eq_ignore_ascii_case(&account.email))
            .then(|| account.display_name.clone());
        let ics = crate::ics::write_reply(event, me, &account.display_name, status);
        let calendar = ContentType::parse("text/calendar; charset=utf-8; method=REPLY")
            .map_err(|e| CoreError::Invalid(e.to_string()))?;
        // The name came from someone else's file, so it's kept to a sensible length.
        let title: String = event.title.chars().take(120).collect();
        let message = Message::builder()
            .from(Mailbox::new(name, from))
            .to(Mailbox::new(None, to))
            .subject(format!("{word}: {title}"))
            .multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::plain(format!("{word}: {title}")))
                    .singlepart(SinglePart::builder().header(calendar).body(ics)),
            )
            .map_err(|e| CoreError::Send {
                code: "bad_message".into(),
                message: e.to_string(),
            })?;
        let raw = message.formatted();
        let password = self.password(account.id).await?;
        let (host, port, security) = self.smtp_settings(account, &password).await?;
        match smtp::send(
            account,
            &password,
            &host,
            port,
            security,
            message,
            self.reach(),
        )
        .await
        {
            Err(e)
                if account.auth == "microsoft"
                    && matches!(&e, CoreError::Send { code, .. } if code == "smtp_auth" || code == "smtp_refused") =>
            {
                self.graph_send(account, &raw).await
            }
            other => other,
        }
    }

    /// Tries the outgoing server, finding it first if needed. Sends nothing.
    pub(crate) async fn check_smtp(&self, account_id: i64) -> Result<Value> {
        let account = self.store.account(account_id)?;
        let password = self.password(account.id).await?;
        let (host, port, security) = self.smtp_settings(&account, &password).await?;
        smtp::check(&account, &password, &host, port, security, self.reach()).await?;
        Ok(json!({ "host": host, "port": port, "security": security }))
    }

    /// The outgoing server, finding and remembering it the first time.
    async fn smtp_settings(
        &self,
        account: &Account,
        password: &str,
    ) -> Result<(String, u16, SmtpSecurity)> {
        if !account.smtp_host.is_empty() {
            return Ok((
                account.smtp_host.clone(),
                account.smtp_port,
                account.smtp_security,
            ));
        }
        let found = smtp::discover(account, password, self.reach()).await?;
        self.store
            .set_smtp(account.id, &found.0, found.1, found.2)?;
        Ok(found)
    }

    pub(crate) async fn send_draft(&self, id: i64, force: bool) -> Result<Value> {
        {
            let mut running = self.sending.lock().unwrap_or_else(|e| e.into_inner());
            if !running.insert(id) {
                return Err(CoreError::Send {
                    code: "already_sending".into(),
                    message: "This message is already being sent.".into(),
                });
            }
        }
        let result = self.send_draft_inner(id, force).await;
        if result.is_err() {
            self.release_send(id);
        }
        result
    }

    /// Clears away drafts that went but couldn't be cleared at the time, and their files.
    pub(crate) fn clear_sent_drafts(&self) {
        let Ok(left) = self.store.sent_drafts_left() else {
            return;
        };
        for id in left {
            if self.store.delete_draft(id).is_ok() {
                self.remove_draft_files(id);
                let _ = self.store.clear_draft_team_base(id);
                self.release_send(id);
            }
        }
    }

    fn release_send(&self, id: i64) {
        self.sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    async fn send_draft_inner(&self, id: i64, force: bool) -> Result<Value> {
        if self.store.draft_was_sent(id)? {
            return Err(CoreError::Send {
                code: "already_sent".into(),
                message: "This message has already been sent.".into(),
            });
        }
        let draft = self.store.draft(id)?;
        if draft.from_encrypted && !draft.encrypt && !draft.plain_ok {
            return Err(CoreError::Send {
                code: "plain_reply_to_encrypted".into(),
                message: "This quotes an encrypted email, so sending it unencrypted would let \
                          anyone on the way read those words. Turn encryption back on, or \
                          confirm that you want to send it unencrypted."
                    .into(),
            });
        }
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            return Err(CoreError::Send {
                code: "no_recipients".into(),
                message: "Add at least one person to send this to.".into(),
            });
        }
        let account = self.store.account(draft.account_id)?;
        let password = self.password(account.id).await?;

        let domain = account.email.rsplit('@').next().unwrap_or("localhost");
        let rfc_id = format!("{}@{domain}", crate::crypto::random_hex(16));
        let wrapped = format!("<{rfc_id}>");
        let date = SystemTime::now();
        // Built once, so the Sent copy is exactly what went, encrypted if it was.
        let content = self.message_content(&draft, &account)?;
        let content = self.protect(&draft, &account, content)?;
        let prepared = |e: lettre::error::Error| CoreError::Send {
            code: "bad_message".into(),
            message: format!("This message couldn't be prepared: {e}"),
        };
        let message = self
            .message_head(&draft, &account, &wrapped, date, false)?
            .multipart(content.clone())
            .map_err(prepared)?;
        let copy = self
            .message_head(&draft, &account, &wrapped, date, true)?
            .multipart(content)
            .map_err(prepared)?
            .formatted();

        let (host, port, security) = self.smtp_settings(&account, &password).await?;
        // The last word on whether a teammate got there first.
        let hold = self.team_before_send(&draft, &account, force).await?;
        if let Err(e) = smtp::send(
            &account,
            &password,
            &host,
            port,
            security,
            message,
            self.reach(),
        )
        .await
        {
            // Microsoft lets companies switch off this way of sending. Its own mail service
            // does the same job with the same sign-in.
            let refused = matches!(&e, CoreError::Send { code, .. } if code == "smtp_auth" || code == "smtp_refused");
            let sent_another_way = refused
                && account.auth == "microsoft"
                && self.graph_send(&account, &copy).await.is_ok();
            if !sent_another_way {
                if let Some((key, thread_key)) = &hold {
                    self.team_after_send(key, thread_key, false).await;
                }
                return Err(e);
            }
        }

        // From here it has gone. Nothing below can undo that or may report failure.
        // Marked as gone first, so even if clearing it away fails, it's never sent again.
        if let Err(e) = self.store.mark_draft_sent(id) {
            tracing::warn!("A sent draft couldn't be marked as sent: {e}");
        }
        match self.store.delete_draft(id) {
            Ok(()) => {
                // Its files only go once the draft itself has.
                self.remove_draft_files(id);
                let _ = self.store.clear_draft_team_base(id);
                // The next draft can reuse this number while the Sent copy is still saving.
                self.release_send(id);
            }
            // Still reported as sent, because it was. It's cleared away later, and until
            // then the number stays marked as being sent here too.
            Err(e) => tracing::warn!("A sent draft couldn't be cleared away: {e}"),
        }
        // After the draft is gone, so a slow server of ours can't bring it back to send twice.
        if let Some((key, thread_key)) = &hold {
            self.team_after_send(key, thread_key, true).await;
        }
        // A reply means it's been dealt with, and a reminder may have been asked for.
        let thread = match draft.parent_message_id.filter(|_| draft.mode != "forward") {
            Some(parent) => self.store.message_thread(parent).ok().flatten(),
            None => None,
        };
        if let Some(thread) = &thread {
            let _ = self.store.clear_returned(account.id, thread);
        }
        if let Some(days) = draft.remind_days.filter(|d| *d > 0) {
            let thread = thread.unwrap_or_else(|| rfc_id.clone());
            let at = crate::identity::unix_now() + days * 86_400;
            if self
                .store
                .set_follow_up(account.id, &thread, Some(at))
                .is_ok()
            {
                let _ = self.marks_changed().await;
            }
        }
        let mut warnings = Vec::new();
        if let Err(e) = self
            .save_sent_copy(&account, &password, &rfc_id, &copy)
            .await
        {
            warnings.push(format!(
                "Your message was sent, but a copy couldn't be saved to Sent: {e}"
            ));
        }
        if draft.mode == "reply" || draft.mode == "reply_all" {
            if let Some(parent) = draft.parent_message_id {
                let _ = self.mark_answered(&account, &password, parent).await;
            }
        }
        Ok(json!({ "sent": true, "warnings": warnings, "rfc_id": rfc_id }))
    }

    /// Many providers file sent mail themselves, so this checks first. Adding a
    /// second copy would show the message twice.
    async fn save_sent_copy(
        &self,
        account: &Account,
        password: &str,
        rfc_id: &str,
        raw: &[u8],
    ) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(self.sent_copy_wait_ms)).await;
        let mut conn = ImapConnection::connect(account, password, self.reach()).await?;
        let folders = crate::sync::with_chosen_roles(conn.folders().await?, account);
        let sent = match folders.iter().find(|f| f.role == "sent") {
            Some(f) => f.path.clone(),
            None => conn.ensure_folder("Sent").await?,
        };
        if !conn.folder_has_message(&sent, rfc_id).await? {
            conn.append(&sent, raw).await?;
        }
        // Brought in now, so the reply shows in its conversation straight away rather than
        // after the next full refresh. If this fails, that refresh still brings it.
        if let Some(folder) = self
            .store
            .list_folders(account.id)?
            .into_iter()
            .find(|f| f.path == sent)
        {
            let ring = self.pgp_ring(account.id);
            if let Err(e) = crate::sync::look_for_new_mail(
                &mut conn,
                &self.store,
                account.id,
                folder.id,
                &folder.path,
                "sent",
                &self.options,
                &ring,
            )
            .await
            {
                tracing::debug!("bringing in the Sent copy: {e}");
            }
        }
        conn.logout().await;
        Ok(())
    }

    async fn mark_answered(
        &self,
        account: &Account,
        password: &str,
        message_id: i64,
    ) -> Result<()> {
        let loc = self.store.message_location(message_id)?;
        let mut conn = ImapConnection::connect(account, password, self.reach()).await?;
        let mailbox = conn.select_folder(&loc.folder_path).await?;
        if mailbox.uid_validity == loc.uid_validity {
            conn.store_flags(&[loc.uid], "+FLAGS.SILENT (\\Answered)")
                .await?;
        }
        conn.logout().await;
        Ok(())
    }
}

/// A draft that's on the mail server, ready to become one here.
pub(crate) struct DraftFromMail<'a> {
    pub account_id: i64,
    pub parent: Option<i64>,
    pub detail: &'a MessageDetail,
    pub body: &'a Body,
}

pub(crate) fn parse_people_draft(d: DraftFromMail<'_>) -> Draft {
    let text = d.body.text.clone().unwrap_or_default();
    Draft {
        id: 0,
        account_id: d.account_id,
        mode: if d.parent.is_some() { "reply" } else { "new" }.to_string(),
        parent_message_id: d.parent,
        from_address: String::new(),
        to: d.detail.to.clone(),
        cc: d.detail.cc.clone(),
        bcc: d.detail.bcc.clone(),
        subject: d.detail.subject.clone(),
        html: match &d.body.html {
            Some(html) => clean_html(html),
            None => escape_html(&text).replace('\n', "<br>"),
        },
        text,
        quoted_html: String::new(),
        quoted_text: String::new(),
        attachments: vec![],
        updated_at: 0,
        send_at: None,
        send_problem: None,
        remind_days: None,
        made_by_ai: false,
        from_encrypted: false,
        plain_ok: false,
        encrypt: false,
        sign: false,
    }
}

/// What a person may change on a draft.
pub(crate) struct DraftUpdate {
    pub id: i64,
    pub account_id: i64,
    pub from_address: String,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub html: String,
    pub text: String,
    /// None leaves it as it was.
    pub encrypt: Option<bool>,
    pub sign: Option<bool>,
    /// The person confirmed sending this unencrypted though it quotes encrypted mail.
    pub allow_plain: bool,
}

/// The editor shows paragraphs with no gaps and keeps empty lines. Without this,
/// mail apps would add gaps and collapse the empty lines.
fn email_spacing(html: &str) -> String {
    html.replace("<p></p>", "<p style=\"margin:0\"><br></p>")
        .replace("<p>", "<p style=\"margin:0\">")
        .replace("<p style=\"text-align", "<p style=\"margin:0;text-align")
}

/// The signature a new draft starts with: the chosen one for this kind of message,
/// or the plain-text signature from older versions.
fn starting_signature(account: &Account, mode: &str) -> String {
    let prefs = &account.prefs;
    if prefs.signatures.is_empty() {
        return signature_html(&account.signature);
    }
    let chosen = if mode == "reply" || mode == "reply_all" {
        &prefs.signature_reply
    } else {
        &prefs.signature_new
    };
    chosen
        .as_ref()
        .and_then(|id| prefs.signatures.iter().find(|s| s.id == *id))
        .map(|s| format!("<p></p><p></p>{}", clean_html(&s.html)))
        .unwrap_or_default()
}

fn signature_html(signature: &str) -> String {
    if signature.trim().is_empty() {
        return String::new();
    }
    let lines: String = signature
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                "<p></p>".to_string()
            } else {
                format!("<p>{}</p>", escape_html(l))
            }
        })
        .collect();
    format!("<p></p><p></p>{lines}")
}

/// The earlier message, as it appears below a reply or forward.
fn quote_of(
    parent: &MessageDetail,
    body: Option<&Body>,
    mode: &str,
    date: &str,
) -> (String, String) {
    let content_html = match body {
        Some(Body { html: Some(h), .. }) => clean_html(h),
        Some(Body { text: Some(t), .. }) => {
            format!(
                "<div style=\"white-space:pre-wrap\">{}</div>",
                escape_html(t)
            )
        }
        _ => String::new(),
    };
    let content_text = body.and_then(|b| b.text.clone()).unwrap_or_default();
    let sender = addr_text(&parent.from);
    if mode == "forward" {
        let mut lines = vec![
            format!("From: {sender}"),
            format!("Date: {date}"),
            format!("Subject: {}", parent.subject),
            format!("To: {}", addr_list_text(&parent.to)),
        ];
        if !parent.cc.is_empty() {
            lines.push(format!("Cc: {}", addr_list_text(&parent.cc)));
        }
        let header_html = lines
            .iter()
            .map(|l| escape_html(l))
            .collect::<Vec<_>>()
            .join("<br>");
        (
            format!(
                "<br><div class=\"gmail_quote\"><div>---------- Forwarded message ----------<br>{header_html}</div><br>{content_html}</div>"
            ),
            format!(
                "---------- Forwarded message ----------\n{}\n\n{content_text}",
                lines.join("\n")
            ),
        )
    } else {
        let name = if parent.from.name.is_empty() {
            &parent.from.address
        } else {
            &parent.from.name
        };
        let intro = format!("On {date}, {name} wrote:");
        let quoted_text = content_text
            .lines()
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        (
            format!(
                "<br><div class=\"gmail_quote\"><div>{}</div><blockquote style=\"{QUOTE_STYLE}\">{content_html}</blockquote></div>",
                escape_html(&intro)
            ),
            format!("{intro}\n{quoted_text}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(address: &str) -> Addr {
        Addr {
            name: String::new(),
            address: address.into(),
        }
    }

    fn parent(from: &str, to: &[&str], cc: &[&str], reply_to: &[&str]) -> MessageDetail {
        MessageDetail {
            id: 1,
            account_id: 1,
            thread_id: "t".into(),
            rfc_id: Some("x@y".into()),
            subject: "Hi".into(),
            from: addr(from),
            reply_to: reply_to.iter().map(|a| addr(a)).collect(),
            to: to.iter().map(|a| addr(a)).collect(),
            cc: cc.iter().map(|a| addr(a)).collect(),
            bcc: vec![],
            date: 0,
            seen: true,
            flagged: false,
            folder_id: 1,
            body: None,
            risk: 0,
            warnings: vec![],
            can_unsubscribe: false,
            due_at: None,
        }
    }

    fn addresses(list: &[Addr]) -> Vec<&str> {
        list.iter().map(|a| a.address.as_str()).collect()
    }

    #[test]
    fn reply_goes_to_the_sender_only() {
        let p = parent("a@x", &["me@x", "b@x"], &["c@x"], &[]);
        let (to, cc) = reply_recipients(&p, &["me@x"], false);
        assert_eq!(addresses(&to), ["a@x"]);
        assert!(cc.is_empty());
    }

    #[test]
    fn reply_respects_reply_to() {
        let p = parent("a@x", &["me@x"], &[], &["list@x"]);
        let (to, _) = reply_recipients(&p, &["me@x"], false);
        assert_eq!(addresses(&to), ["list@x"]);
    }

    #[test]
    fn reply_all_keeps_everyone_except_me_with_no_repeats() {
        let p = parent("a@x", &["ME@x", "b@x", "A@x"], &["c@x", "b@x", "me@x"], &[]);
        let (to, cc) = reply_recipients(&p, &["me@x"], true);
        assert_eq!(addresses(&to), ["a@x", "b@x"]);
        assert_eq!(addresses(&cc), ["c@x"]);
    }

    #[test]
    fn replying_to_my_own_message_goes_back_to_the_same_people() {
        let p = parent("me@x", &["a@x"], &["b@x"], &[]);
        let (to, cc) = reply_recipients(&p, &["me@x"], true);
        assert_eq!(addresses(&to), ["a@x"]);
        assert_eq!(addresses(&cc), ["b@x"]);
    }

    #[test]
    fn empty_lines_and_spacing_survive_in_other_mail_apps() {
        assert_eq!(
            email_spacing("<p>a</p><p></p><p style=\"text-align:center\">b</p>"),
            "<p style=\"margin:0\">a</p><p style=\"margin:0\"><br></p><p style=\"margin:0;text-align:center\">b</p>"
        );
    }

    #[test]
    fn subjects_are_prefixed_once() {
        assert_eq!(prefixed("Re:", "Hello"), "Re: Hello");
        assert_eq!(prefixed("Re:", "RE: Hello"), "RE: Hello");
        assert_eq!(prefixed("Fwd:", "Re: Hello"), "Fwd: Hello");
    }

    #[test]
    fn scripts_are_stripped_and_the_look_is_kept() {
        let out = clean_html(
            "<p style=\"color:red\" onclick=\"x()\">Hi<script>bad()</script></p><a href=\"javascript:x()\">l</a><img src=\"data:image/png;base64,AA==\">",
        );
        assert!(out.contains("color:red"));
        assert!(!out.contains("onclick") && !out.contains("script") && !out.contains("javascript"));
        assert!(out.contains("data:image/png"));
    }

    #[test]
    fn embedded_images_become_inline_parts() {
        let (html, images) = extract_inline_images(
            "<img src=\"data:image/png;base64,aGVsbG8=\"><img src=\"data:image/svg+xml;base64,aGVsbG8=\">",
            "x.test",
        );
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].2, b"hello");
        assert!(html.contains(&format!("cid:{}", images[0].0)));
        assert!(html.contains("svg+xml"), "unsafe types stay untouched");
    }

    #[test]
    fn file_names_cannot_escape_the_folder() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(safe_file_name(".hidden"), "hidden");
        assert_eq!(safe_file_name("   "), "attachment");
        assert_eq!(safe_file_name("a\u{0}b.pdf"), "ab.pdf");
        assert_eq!(safe_file_name("invoice\u{202E}fdp.zip"), "invoicefdp.zip");
    }
}
