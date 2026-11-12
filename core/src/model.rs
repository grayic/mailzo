use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Encrypted from the first byte. The only option for real mail servers.
    Tls,
    /// No encryption. Refused for anything except this computer.
    Plain,
}

impl Security {
    pub fn as_str(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::Plain => "plain",
        }
    }

    pub fn parse(s: &str) -> Security {
        if s == "plain" {
            Security::Plain
        } else {
            Security::Tls
        }
    }
}

/// How the outgoing mail server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    /// Encrypted from the first byte, usually port 465.
    Tls,
    /// Starts plain and must switch to encrypted, usually port 587.
    StartTls,
    /// No encryption. Refused for anything except this computer.
    Plain,
}

impl SmtpSecurity {
    pub fn as_str(self) -> &'static str {
        match self {
            SmtpSecurity::Tls => "tls",
            SmtpSecurity::StartTls => "starttls",
            SmtpSecurity::Plain => "plain",
        }
    }

    pub fn parse(s: &str) -> SmtpSecurity {
        match s {
            "starttls" => SmtpSecurity::StartTls,
            "plain" => SmtpSecurity::Plain,
            _ => SmtpSecurity::Tls,
        }
    }
}

/// One saved signature.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Signature {
    pub id: String,
    pub name: String,
    pub html: String,
}

fn yes() -> bool {
    true
}

fn arrived() -> String {
    "arrived".into()
}

fn default_choice() -> String {
    "default".into()
}

/// An inbox's own choices. They travel with it to other devices. Every field has a
/// default, so older copies still read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountPrefs {
    /// A short name, such as "Work". Empty means the address is shown.
    #[serde(default)]
    pub label: String,
    /// The hue of this inbox's colour. None means one chosen from the address.
    #[serde(default)]
    pub color: Option<u16>,
    #[serde(default = "yes")]
    pub in_all_inbox: bool,
    #[serde(default)]
    pub signatures: Vec<Signature>,
    /// Which signature new messages and forwards start with. None means no signature.
    #[serde(default)]
    pub signature_new: Option<String>,
    #[serde(default)]
    pub signature_reply: Option<String>,
    /// Other addresses this inbox sends from.
    #[serde(default)]
    pub aliases: Vec<Addr>,
    /// `arrived` replies from the address the email came to, `main` always uses the
    /// inbox's own address.
    #[serde(default = "arrived")]
    pub reply_from: String,
    /// `default` follows the app's choice. Otherwise `all`, `people` or `none`.
    #[serde(default = "default_choice")]
    pub alerts: String,
    /// Which folder does which job, for providers that label them oddly.
    #[serde(default)]
    pub folder_roles: BTreeMap<String, String>,
    /// New messages start from this inbox, whichever one is selected.
    #[serde(default)]
    pub default_for_new: bool,
    /// Comes from the team's list, not this person's own.
    #[serde(default)]
    pub team_shared: bool,
    /// Which team it's shared with. A shared inbox belongs to one team.
    #[serde(default)]
    pub team_id: i64,
    /// Whether AI may read this inbox at all.
    #[serde(default = "yes")]
    pub ai_allowed: bool,
    /// The same for an inbox shared with a team. Each person chooses for their own AI,
    /// and it starts off, since the mail is the whole team's.
    #[serde(default)]
    pub shared_ai_allowed: bool,
    /// Folders AI stays out of.
    #[serde(default)]
    pub ai_skip_folders: Vec<String>,
    /// What the person tells AI about themselves and this inbox, for writing replies.
    #[serde(default)]
    pub about: String,
}

impl Default for AccountPrefs {
    fn default() -> Self {
        Self {
            label: String::new(),
            color: None,
            in_all_inbox: true,
            signatures: vec![],
            signature_new: None,
            signature_reply: None,
            aliases: vec![],
            reply_from: arrived(),
            alerts: default_choice(),
            folder_roles: BTreeMap::new(),
            default_for_new: false,
            team_shared: false,
            team_id: 0,
            ai_allowed: true,
            shared_ai_allowed: false,
            ai_skip_folders: vec![],
            about: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: i64,
    /// The same on every linked device, unlike `id`.
    pub sync_id: String,
    pub email: String,
    pub display_name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub security: Security,
    pub username: String,
    /// Empty until it has been set or found automatically.
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    pub signature: String,
    /// When the settings last changed, so the newest wins across devices.
    pub settings_updated_at: i64,
    pub prefs: AccountPrefs,
    /// `password`, or the provider signed in with through the browser. With a browser
    /// sign-in, what's kept as the secret is a refresh token.
    pub auth: String,
}

/// Who is signed in on this device, and how far along they are.
#[derive(Debug, Clone)]
pub struct Identity {
    pub email: String,
    pub device_id: i64,
    pub device_name: String,
    /// `needs_setup`, `linking` or `ready`.
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Folder {
    pub id: i64,
    pub account_id: i64,
    pub path: String,
    pub name: String,
    pub role: String,
    pub total: i64,
    pub unread: i64,
    pub last_synced_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Addr {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub account_id: i64,
    pub account_email: String,
    /// Who the conversation is with, for their picture.
    pub lead_address: String,
    /// The folder this listing found it in, which actions on it start from.
    pub folder_id: i64,
    pub subject: String,
    pub participants: Vec<String>,
    pub snippet: String,
    pub last_date: i64,
    pub message_count: i64,
    pub unread_count: i64,
    pub flagged: bool,
    pub has_attachments: bool,
    /// 0 is fine, 1 is worth a warning, 2 looks like spam.
    pub risk: i64,
    /// What AI noticed, such as `receipt`, if it's been asked.
    pub label: Option<String>,
    /// Who has it, if it's in an inbox shared with a team.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<ThreadTeam>,
    /// Waiting on this person.
    pub needs_you: bool,
    /// Why the app thinks so, in a few words.
    pub why: Option<String>,
    /// What it's about in a line, if AI has been asked.
    pub ai_line: Option<String>,
    /// A date the email asks for something by, if AI has been asked.
    pub due_at: Option<i64>,
    /// Kept at the top of the inbox.
    pub pinned: bool,
    /// Stays out of the inbox, and doesn't notify.
    pub muted: bool,
    /// Put aside until this time.
    pub snooze_until: Option<i64>,
    /// Came back from being put aside, and hasn't been looked at yet.
    pub returned_at: Option<i64>,
    /// A reminder came due with no reply.
    pub follow_due: bool,
    /// When to remind, if there's still no reply.
    pub follow_at: Option<i64>,
    /// In a search, what it's looking for is right there in the subject.
    #[serde(default)]
    pub best_match: bool,
    /// The person's own tags on it.
    #[serde(default)]
    pub tags: Vec<crate::tags::ThreadTag>,
    /// A sign-in or verification code it gives, to copy in one click.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadTeam {
    pub assignee: Option<i64>,
    pub assignee_email: Option<String>,
    pub mine: bool,
    pub done: bool,
    pub comments: i64,
    pub viewing: Vec<String>,
    pub replying: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Its position among the message's attachments. Missing on older saved mail.
    #[serde(default)]
    pub part: Option<u32>,
    /// Already shown inside the message, so it isn't listed separately.
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Body {
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    /// Set for encrypted or signed mail only.
    pub pgp: Option<PgpInfo>,
}

/// What an encrypted or signed email's protection amounts to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PgpInfo {
    pub encrypted: bool,
    pub signed: bool,
    /// The signature was checked against a known key and is good.
    pub verified: bool,
    /// Whose key made the signature.
    pub signer: Option<String>,
    /// What's wrong, in words to show the person.
    pub problem: Option<String>,
    /// Signed with a key that only came from the sender's earlier email (Autocrypt),
    /// not one the person added. The signature is good, but who holds the key isn't
    /// confirmed, so it isn't shown as verified.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signer_unconfirmed: bool,
    /// Signed with a different key from the one kept for the sender: a newer email
    /// offered it, and it hasn't been accepted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key_changed: bool,
    /// Only some of the email's text is inside the signed or encrypted part.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageDetail {
    pub id: i64,
    pub account_id: i64,
    pub thread_id: String,
    pub rfc_id: Option<String>,
    pub subject: String,
    pub from: Addr,
    pub reply_to: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
    pub folder_id: i64,
    pub body: Option<Body>,
    /// 0 is fine, 1 is worth a warning, 2 looks like spam.
    pub risk: i64,
    /// Why, in words to show the person.
    pub warnings: Vec<String>,
    pub can_unsubscribe: bool,
    /// The date it asks for something by, found in its words, if any.
    pub due_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStats {
    pub folders: usize,
    pub new_messages: usize,
    pub removed_messages: usize,
    pub bodies_fetched: usize,
    /// New mail checked and spam moved, so the screens know to look again.
    pub filtered: usize,
    pub already_running: bool,
    pub errors: Vec<String>,
    /// Older mail that came down in the background.
    pub older_saved: usize,
    /// There's more older mail still to come, so another refresh should follow soon.
    pub more_to_download: bool,
    /// Emails read, unread, starred or unstarred elsewhere.
    pub flags_changed: usize,
    /// New unread mail in the inbox, for notifications.
    #[serde(skip)]
    pub new_mail: Vec<NewMail>,
}

#[derive(Debug, Clone)]
pub struct NewMail {
    pub account_id: i64,
    pub thread_id: String,
    pub rfc_id: Option<String>,
    /// What the filter's opinion of this email is saved under.
    pub key: String,
    pub from_name: String,
    pub from_address: String,
    pub subject: String,
    /// The start of what it says, for a notification that shows it.
    pub preview: String,
    /// False for newsletters and automatic mail.
    pub is_person: bool,
    /// Sorted as waiting on this person, which is worked out after it arrives.
    pub needs_you: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// A message being written. Saved on this device until it is sent.
#[derive(Debug, Clone, Serialize)]
pub struct Draft {
    pub id: i64,
    pub account_id: i64,
    /// `new`, `reply`, `reply_all` or `forward`.
    pub mode: String,
    pub parent_message_id: Option<i64>,
    /// One of the inbox's other addresses. Empty means its main address.
    pub from_address: String,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub html: String,
    pub text: String,
    /// The earlier message, added below what the person writes.
    pub quoted_html: String,
    pub quoted_text: String,
    pub attachments: Vec<DraftAttachment>,
    pub updated_at: i64,
    /// Going out by itself at this time.
    pub send_at: Option<i64>,
    /// Why a scheduled send didn't go.
    pub send_problem: Option<String>,
    /// Remind the person if nobody has replied this many days after it goes.
    pub remind_days: Option<i64>,
    /// Saved by another AI app, so it should be read closely before sending.
    pub made_by_ai: bool,
    /// Goes out encrypted to everyone it's sent to, and signed.
    pub encrypt: bool,
    /// Goes out signed with this inbox's key.
    pub sign: bool,
    /// Replies to or forwards an encrypted email, so it holds words that were only ever
    /// sent encrypted. Never saved to the server's Drafts, and not sent unencrypted
    /// unless the person said so (`plain_ok`).
    pub from_encrypted: bool,
    /// The person confirmed sending this unencrypted anyway.
    pub plain_ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Contact {
    pub name: String,
    pub address: String,
}
