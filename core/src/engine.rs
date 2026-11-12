//! The one interface to the mail engine.
//!
//! Every screen talks to `Engine::call(command, arguments)` using JSON. The Mac
//! app passes those calls in directly, and a web version would pass the same
//! calls over HTTP, so both hosts share all of this code.

use crate::actions::{BulkThread, ConnectionUpdate, SettingsUpdate};
use crate::alerts::Alert;
use crate::avatars::{LOGO_SOURCE, PERSON_SOURCE};
use crate::cloud::CloudClient;
use crate::compose::DraftUpdate;
use crate::error::{CoreError, Result};
use crate::imap::{check_security, ImapConnection, MailWait};
use crate::model::*;
use crate::secrets::{account_secret, SecretStore};
use crate::store::Store;
use crate::sync::{self, SyncOptions};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, Semaphore};

/// Opens a web address in the person's real browser.
pub type UrlOpener = Arc<dyn Fn(&str) + Send + Sync>;
/// Opens a saved file, or shows it in Finder when the flag is false.
pub type FileOpener = Arc<dyn Fn(&str, bool) + Send + Sync>;
/// Shows a notification.
pub type AlertSender = Arc<dyn Fn(&Alert) + Send + Sync>;
/// Told which inbox has new mail as soon as it's saved, so every window can show it.
pub type NewMailListener = Arc<dyn Fn(i64) + Send + Sync>;

pub struct Engine {
    pub(crate) store: Store,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) options: SyncOptions,
    syncing: Mutex<HashSet<i64>>,
    inbox_gates: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) cloud: Option<CloudClient>,
    pub(crate) device_name: String,
    pub(crate) number_tries: Mutex<HashMap<i64, u32>>,
    pub(crate) url_opener: Option<UrlOpener>,
    /// On a server: links from emails may only reach public addresses.
    pub(crate) public_links_only: bool,
    /// Mail servers, calendars and the servers guessed for an address are only reached at
    /// public addresses. Off on a person's own computer, where a server they typed may be
    /// on their home network or this computer (such as a mail bridge).
    pub(crate) public_servers_only: bool,
    pub(crate) google_auth_url: String,
    pub(crate) google_cancel: Notify,
    /// The address just signed in to with Google, Google's word for it, and the random
    /// value that sign-in asked Google to put in that word.
    pub(crate) mail_proof: Mutex<Option<crate::google::MailProof>>,
    /// When the latest code was asked for, so older codes in an inbox are ignored.
    pub(crate) code_asked_at: Mutex<Option<i64>>,
    pub(crate) data_dir: PathBuf,
    pub(crate) ai_app_places: crate::connections::Places,
    pub(crate) plan_programs: Option<PathBuf>,
    pub(crate) plan_turns: tokio::sync::Semaphore,
    pub(crate) mcp_sends: Mutex<Vec<crate::mcp::McpSend>>,
    pub(crate) mcp_send_changed: Notify,
    pub(crate) downloads_dir: PathBuf,
    pub(crate) file_opener: Option<FileOpener>,
    pub(crate) saved_files: Mutex<HashSet<String>>,
    pub(crate) sending: Mutex<HashSet<i64>>,
    /// Inboxes being watched for new mail, and how to stop each.
    mail_watches: Mutex<HashMap<i64, MailWatch>>,
    pub(crate) directory_url: String,
    pub(crate) record_lookup: Option<crate::providers::RecordLookup>,
    pub(crate) mail_signins: Vec<crate::signin::MailSignIn>,
    pub(crate) mail_tokens: tokio::sync::Mutex<HashMap<i64, (String, i64)>>,
    pub(crate) ai_url: String,
    /// AI a paid plan includes, plugged in by the service that provides it. Never set here.
    pub(crate) included_ai: Option<Arc<dyn crate::allowance::IncludedAi>>,
    /// Dollars of included AI set aside by requests still waiting for their answer.
    pub(crate) included_held: Mutex<f64>,
    /// The AI that comes with Mailzo Cloud, when it was plugged in by hand.
    pub(crate) paid_ai: Option<Arc<dyn crate::paid::PaidAi>>,
    /// Mailzo Cloud's own, used while the person is signed in.
    pub(crate) cloud_paid: Option<Arc<crate::paid::CloudPaidAi>>,
    /// Follow-ups being sent right now, so two overlapping checks can't both send one.
    pub(crate) follow_ups_sending: Mutex<HashSet<i64>>,
    pub(crate) ai_http: reqwest::Client,
    pub(crate) image_http: reqwest::Client,
    pub(crate) images_may_be_local: bool,
    pub(crate) sent_copy_wait_ms: u64,
    /// The newest email the screen was last told about, so mail saved by something other
    /// than a refresh (a sent copy, another app sending through this one) still shows.
    pub(crate) newest_told: std::sync::atomic::AtomicI64,
    pub(crate) avatar_sources: (String, String),
    pub(crate) avatar_http: reqwest::Client,
    pub(crate) avatar_turns: Semaphore,
    pub(crate) alert_sender: Option<AlertSender>,
    new_mail_listener: Option<NewMailListener>,
    pub(crate) team: crate::team::TeamRuntime,
    pub(crate) embedder: Option<crate::meaning::Embedder>,
    /// The Mac's own AI, used when nothing else is connected.
    pub(crate) on_device: Option<crate::on_device::OnDevice>,
    pub(crate) on_device_turns: tokio::sync::Semaphore,
    pub(crate) working_out_meanings: std::sync::atomic::AtomicBool,
    /// Locked behind Touch ID or the Mac's password: nothing that shows mail answers.
    /// Only the app itself unlocks it, after the Mac's own check (`set_locked`); no
    /// command can.
    locked: std::sync::atomic::AtomicBool,
    pub(crate) gmail_api: String,
    pub(crate) graph_api: String,
    pub(crate) caldav_base: Option<String>,
    pub(crate) google_calendar: String,
    pub(crate) microsoft_calendar: String,
}

/// One wait on an inbox's mail server, shared by every page asking about that inbox.
struct MailWatch {
    stop: Arc<Notify>,
    /// Tells everyone else waiting how it ended: (supported, changed).
    told: tokio::sync::broadcast::Sender<(bool, bool)>,
}

/// Takes a finished or abandoned wait off the list, which tells anyone still listening.
struct LeaveWatch<'a> {
    watches: &'a Mutex<HashMap<i64, MailWatch>>,
    account_id: i64,
    stop: Arc<Notify>,
}

impl Drop for LeaveWatch<'_> {
    fn drop(&mut self) {
        let mut watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
        if watches
            .get(&self.account_id)
            .is_some_and(|w| Arc::ptr_eq(&w.stop, &self.stop))
        {
            watches.remove(&self.account_id);
        }
    }
}

/// How much of a shared inbox the team's views look through.
/// Older emails read for the facts their markup states after each refresh.
const FACTS_PER_REFRESH: i64 = 300;

const TEAM_VIEW_REACH: i64 = 500;

#[derive(Deserialize)]
struct LockArg {
    locked: bool,
}

/// What the lock screen and the app's start need while it's locked. None of it shows mail.
const ALLOWED_WHILE_LOCKED: &[&str] = &["auth_status", "app_settings", "list_accounts"];

pub(crate) fn locked_error() -> CoreError {
    CoreError::Cloud {
        code: "locked".into(),
        message: "Mailzo is locked. Unlock it with Touch ID or your Mac's password.".into(),
    }
}

#[derive(Deserialize)]
struct SendArg {
    id: i64,
    #[serde(default)]
    force: bool,
    /// Confirmed: send unencrypted though it quotes an encrypted email.
    #[serde(default)]
    allow_plain: bool,
}

#[derive(Deserialize)]
struct RangeArg {
    from: i64,
    to: i64,
}

#[derive(Deserialize)]
struct EventArg {
    event: crate::calendar::EventInput,
}

#[derive(Deserialize)]
struct InviteArg {
    message_id: i64,
    status: String,
}

#[derive(Deserialize)]
struct SentenceArg {
    sentence: String,
}

#[derive(Deserialize)]
struct RuleArg {
    rule: crate::rules::Rule,
}

#[derive(Deserialize)]
struct AddressDecision {
    address: String,
    allow: bool,
}

#[derive(Deserialize)]
struct SenderList {
    folder_id: i64,
}

#[derive(Deserialize)]
struct MailWith {
    /// One inbox, or none for all of them.
    account_id: Option<i64>,
    addresses: Vec<String>,
}

#[derive(Deserialize)]
struct FromList {
    folder_id: i64,
    address: String,
    #[serde(default)]
    screened: bool,
}

#[derive(Deserialize)]
struct TemplateArg {
    #[serde(default)]
    id: Option<String>,
    name: String,
    body: String,
}

#[derive(Deserialize)]
struct TrackersArg {
    message_id: i64,
    companies: Vec<String>,
}

#[derive(Deserialize)]
struct TagArg {
    #[serde(default)]
    id: Option<i64>,
    name: String,
    colour: String,
}

#[derive(Deserialize)]
struct ThreadOnly {
    account_id: i64,
    thread_id: String,
}

#[derive(Deserialize)]
struct StyleArg {
    account_id: i64,
    text: String,
}

#[derive(Deserialize)]
struct SequenceArg {
    sequence: crate::sequences::Sequence,
}

#[derive(Deserialize)]
struct SequenceStartArg {
    message_id: i64,
    /// A saved set of follow-ups, or `steps` set up just for this conversation.
    #[serde(default)]
    sequence_id: Option<i64>,
    #[serde(default)]
    steps: Option<Vec<crate::sequences::Step>>,
}

#[derive(Deserialize)]
struct RunsArg {
    #[serde(default)]
    account_id: Option<i64>,
    #[serde(default)]
    thread_id: Option<String>,
}

#[derive(Deserialize)]
struct IdArg {
    id: i64,
}

#[derive(Deserialize)]
struct TagIdArg {
    tag_id: i64,
}

#[derive(Deserialize)]
struct ThreadTagArg {
    account_id: i64,
    thread_id: String,
    tag_id: i64,
    on: bool,
    #[serde(default)]
    amount: Option<f64>,
}

#[derive(Deserialize)]
struct TextArg {
    text: String,
}

#[derive(Deserialize)]
struct BriefArg {
    #[serde(default)]
    saved_only: bool,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
struct PolishArg {
    text: String,
    how: String,
    #[serde(default)]
    account_id: Option<i64>,
}

#[derive(Deserialize)]
struct WriteReplyArg {
    message_id: i64,
    #[serde(default)]
    instruction: String,
}

#[derive(Deserialize)]
struct SummarizeArg {
    account_id: i64,
    thread_id: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    saved_only: bool,
}

#[derive(Deserialize)]
struct TranslateArg {
    message_id: i64,
    language: String,
}

#[derive(Deserialize)]
struct AutoReplyArg {
    account_id: i64,
    reply: crate::autoreply::AutoReply,
}

#[derive(Deserialize)]
struct ThreadOnOff {
    account_id: i64,
    thread_id: String,
    on: bool,
}

#[derive(Deserialize)]
struct ThreadWhen {
    account_id: i64,
    thread_id: String,
    #[serde(default)]
    at: Option<i64>,
}

#[derive(Deserialize)]
struct KindArg {
    kind: String,
}

#[derive(Deserialize)]
struct ScheduleArg {
    id: i64,
    send_at: i64,
}

#[derive(Deserialize)]
struct RemindArg {
    id: i64,
    #[serde(default)]
    days: Option<i64>,
}

#[derive(Deserialize)]
struct DueArg {
    #[serde(default)]
    quiet_now: bool,
    /// What time to treat it as, for tests.
    #[serde(default)]
    at: Option<i64>,
}

#[derive(Deserialize)]
struct TeamNameArg {
    name: String,
}

#[derive(Deserialize)]
struct TeamRoleArg {
    role: String,
}

#[derive(Deserialize)]
struct TeamCodeArg {
    code: String,
}

#[derive(Deserialize)]
struct TeamMemberRole {
    id: i64,
    role: String,
}

#[derive(Deserialize)]
struct TeamPoll {
    #[serde(default)]
    wait: bool,
}

#[derive(Deserialize)]
struct TeamThreadArg {
    account_id: i64,
    thread_id: String,
}

#[derive(Deserialize)]
struct TeamAssign {
    account_id: i64,
    thread_id: String,
    expected: Option<i64>,
    assignee: Option<i64>,
}

#[derive(Deserialize)]
struct TeamStatusArg {
    account_id: i64,
    thread_id: String,
    status: String,
}

#[derive(Deserialize)]
struct TeamCommentArg {
    account_id: i64,
    thread_id: String,
    text: String,
    #[serde(default)]
    mentions: Vec<i64>,
}

#[derive(Deserialize)]
struct TeamIdArg {
    id: String,
}

#[derive(Deserialize)]
struct TeamPresenceArg {
    account_id: i64,
    thread_id: String,
    kind: String,
}

#[derive(Deserialize)]
struct TeamTemplateArg {
    #[serde(default)]
    id: Option<String>,
    name: String,
    body: String,
    #[serde(default)]
    expected_version: i64,
}

#[derive(Deserialize)]
struct TeamTemplateDelete {
    id: String,
    expected_version: i64,
}

/// Deliberately has no Debug, so the password cannot end up in a log.
#[derive(Deserialize)]
struct AddAccount {
    email: String,
    #[serde(default)]
    display_name: String,
    imap_host: String,
    imap_port: u16,
    #[serde(default = "default_security")]
    security: Security,
    #[serde(default)]
    username: String,
    password: String,
    #[serde(default)]
    smtp_host: String,
    #[serde(default)]
    smtp_port: u16,
    #[serde(default = "default_smtp_security")]
    smtp_security: SmtpSecurity,
}

fn default_smtp_security() -> SmtpSecurity {
    SmtpSecurity::Tls
}

fn default_security() -> Security {
    Security::Tls
}

#[derive(Deserialize)]
struct AccountArg {
    account_id: i64,
}

#[derive(Deserialize)]
struct InboxOrder {
    account_ids: Vec<i64>,
}

#[derive(Deserialize)]
struct ListThreads {
    #[serde(default)]
    unread_only: bool,
    #[serde(default)]
    tab: Option<String>,
    folder_id: i64,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

#[derive(Deserialize)]
struct ListAll {
    #[serde(default)]
    unread_only: bool,
    #[serde(default)]
    tab: Option<String>,
    role: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Deserialize)]
struct GetThread {
    account_id: i64,
    thread_id: String,
    #[serde(default)]
    folder_id: Option<i64>,
}

#[derive(Deserialize)]
struct ThreadSeen {
    account_id: i64,
    thread_id: String,
    seen: bool,
}

#[derive(Deserialize)]
struct ThreadFlagged {
    account_id: i64,
    thread_id: String,
    flagged: bool,
}

#[derive(Deserialize)]
struct MoveThread {
    account_id: i64,
    thread_id: String,
    folder_id: i64,
    to: String,
}

#[derive(Deserialize)]
struct DeleteForever {
    account_id: i64,
    thread_id: String,
    folder_id: i64,
}

#[derive(Deserialize)]
struct BulkArg {
    threads: Vec<BulkThread>,
    action: String,
    #[serde(default)]
    to: Option<String>,
    /// For read and unread: "here" or "servers", one step at a time.
    #[serde(default)]
    step: Option<String>,
}

#[derive(Deserialize)]
struct SaveAttachment {
    message_id: i64,
    part: u32,
}

#[derive(Deserialize)]
struct SavedFile {
    path: String,
    #[serde(default)]
    open: bool,
}

#[derive(Deserialize)]
struct NewDraft {
    account_id: i64,
    mode: String,
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    date_text: Option<String>,
}

#[derive(Deserialize)]
struct DraftArg {
    id: i64,
}

#[derive(Deserialize)]
struct SaveDraft {
    id: i64,
    account_id: i64,
    #[serde(default)]
    from_address: String,
    #[serde(default)]
    to: Vec<Addr>,
    #[serde(default)]
    cc: Vec<Addr>,
    #[serde(default)]
    bcc: Vec<Addr>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    html: String,
    #[serde(default)]
    text: String,
    /// Left out means unchanged.
    #[serde(default)]
    encrypt: Option<bool>,
    #[serde(default)]
    sign: Option<bool>,
    #[serde(default)]
    allow_plain: bool,
}

#[derive(Deserialize)]
struct PgpAccountArg {
    account_id: i64,
}

#[derive(Deserialize)]
struct PgpImportSecretArg {
    account_id: i64,
    armored: String,
    #[serde(default)]
    passphrase: Option<String>,
}

#[derive(Deserialize)]
struct PgpArmoredArg {
    armored: String,
}

#[derive(Deserialize)]
struct PgpAddressArg {
    address: String,
}

#[derive(Deserialize)]
struct PgpCanEncryptArg {
    account_id: i64,
    #[serde(default)]
    to: Vec<String>,
}

#[derive(Deserialize)]
struct AddAttachment {
    id: i64,
    name: String,
    #[serde(default)]
    mime: String,
    data: String,
}

#[derive(Deserialize)]
struct RemoveAttachment {
    id: i64,
    attachment_id: String,
}

#[derive(Deserialize)]
struct ContactsArg {
    account_id: i64,
    #[serde(default)]
    query: String,
}

#[derive(Deserialize)]
struct PeopleListArg {
    #[serde(default)]
    query: String,
    #[serde(default = "default_people_limit")]
    limit: i64,
}

fn default_people_limit() -> i64 {
    200
}

#[derive(Deserialize)]
struct PersonAddressArg {
    address: String,
}

#[derive(Deserialize)]
struct PersonIdArg {
    id: i64,
}

#[derive(Deserialize)]
struct VcardTextArg {
    text: String,
}

#[derive(Deserialize)]
struct SenderImages {
    address: String,
    allowed: bool,
}

#[derive(Deserialize)]
struct AvatarArg {
    address: String,
}

#[derive(Deserialize)]
struct MailSignInArg {
    email: String,
    provider: String,
}

#[derive(Deserialize)]
struct FindProviderArg {
    email: String,
}

#[derive(Deserialize)]
struct SpamMarkArg {
    threads: Vec<BulkThread>,
    #[serde(default)]
    block: bool,
}

#[derive(Deserialize)]
struct SpamClearArg {
    threads: Vec<BulkThread>,
}

#[derive(Deserialize)]
struct UnlistArg {
    address: String,
    kind: String,
}

#[derive(Deserialize)]
struct SortSenderArg {
    address: String,
    category: String,
}

#[derive(Deserialize)]
struct TabCountsArg {
    #[serde(default)]
    folder_id: Option<i64>,
    #[serde(default)]
    unread: bool,
}

#[derive(Deserialize)]
struct ForgetLeftArg {
    account_id: i64,
    address: String,
}

#[derive(Deserialize)]
struct UnsubscribeArg {
    message_id: i64,
    /// Many at once: a page that would need opening is handed back instead of opened.
    #[serde(default)]
    quietly: bool,
}

#[derive(Deserialize)]
struct SearchArg {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    filters: Option<crate::search::SearchFilters>,
    #[serde(default)]
    extra_ids: Vec<i64>,
    /// Typed text looks in this inbox only. Filters say so themselves.
    #[serde(default)]
    account: Option<i64>,
    /// How many conversations at most, for a list that has scrolled past the first lot.
    #[serde(default)]
    most: Option<i64>,
}

impl SearchArg {
    fn filters(&self) -> crate::search::SearchFilters {
        match (&self.filters, &self.text) {
            (Some(f), _) => f.clone(),
            (None, Some(text)) => crate::search::SearchFilters {
                account: self.account,
                ..crate::search::parse_plain(text)
            },
            (None, None) => Default::default(),
        }
    }
}

#[derive(Deserialize)]
struct UnderstandArg {
    text: String,
    today: String,
    utc_offset_minutes: i64,
}

#[derive(Deserialize)]
struct NewFolderArg {
    account_id: i64,
    name: String,
}

/// Servers may drop a quiet connection after about 30 minutes, and home routers much sooner.
/// Enough that what just arrived opens at once. The longer refresh brings the rest.
const BODIES_WITH_NEW_MAIL: usize = 5;
const MAIL_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(9 * 60);

#[derive(Deserialize)]
struct AskSourcesArg {
    question: String,
    filters: Option<crate::search::SearchFilters>,
    /// One inbox to look in, or all of them.
    #[serde(default)]
    scope: Option<i64>,
    /// Find mail without asking AI what to look for first.
    #[serde(default)]
    quick: bool,
    /// The questions asked before in this chat, oldest first, for a follow-up.
    #[serde(default)]
    previous: Vec<String>,
    /// What was made of the last of them, which a follow-up starts from.
    #[serde(default)]
    previous_plan: Option<Value>,
    /// Asked from an email: its conversation.
    #[serde(default)]
    about: Option<AboutArg>,
}

#[derive(Deserialize)]
struct AboutArg {
    account_id: i64,
    thread_id: String,
}

#[derive(Deserialize)]
struct ChatArg {
    question: String,
    #[serde(default)]
    history: Vec<crate::assist::ChatTurn>,
    ids: Vec<i64>,
    /// Those of `ids` found only as maybe related.
    #[serde(default)]
    related: Vec<i64>,
    #[serde(default)]
    language: Option<String>,
    /// What Ask found the question to be about.
    #[serde(default)]
    plan: crate::assist::ChatPlan,
}

#[derive(Deserialize)]
struct AskArg {
    question: String,
    ids: Vec<i64>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
struct AiKeyArg {
    key: String,
    service: Option<String>,
}

#[derive(Deserialize)]
struct DeleteFolderArg {
    account_id: i64,
    folder_id: i64,
}

#[derive(Deserialize)]
struct ThreadPriorityArg {
    account_id: i64,
    thread_id: String,
    group: Option<String>,
}

#[derive(Deserialize)]
struct SenderPriorityArg {
    address: String,
    group: String,
}

#[derive(Deserialize)]
struct McpSendDecision {
    id: String,
    send: bool,
}

#[derive(Deserialize)]
struct AiAppArg {
    id: String,
}

#[derive(Deserialize)]
struct AiModelArg {
    model: String,
}

#[derive(Deserialize)]
struct SettingArg {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct PrefsArg {
    account_id: i64,
    prefs: AccountPrefs,
}

/// Deliberately has no Debug, because it can hold a password.
#[derive(Deserialize)]
struct ConnectionArg {
    account_id: i64,
    imap_host: String,
    imap_port: u16,
    #[serde(default = "default_security")]
    security: Security,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
struct SyncArg {
    account_id: i64,
    /// Just the inbox, because the server has said something changed there.
    #[serde(default)]
    inbox_only: bool,
    /// True during the person's quiet hours, which only the screens' clock knows.
    #[serde(default)]
    quiet_now: bool,
}

#[derive(Deserialize)]
struct UpdateSettings {
    account_id: i64,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    smtp_host: String,
    #[serde(default)]
    smtp_port: u16,
    #[serde(default = "default_smtp_security")]
    smtp_security: SmtpSecurity,
}

#[derive(Deserialize)]
struct MessageArg {
    message_id: i64,
}

#[derive(Deserialize)]
struct CodeArg {
    code: String,
}

#[derive(Deserialize)]
struct EmailArg {
    email: String,
}

#[derive(Deserialize)]
struct VerifyArg {
    email: String,
    code: String,
}

#[derive(Deserialize)]
struct RecoveryArg {
    recovery_key: String,
}

#[derive(Deserialize)]
struct DeviceArg {
    id: i64,
}

#[derive(Deserialize)]
struct ApproveArg {
    id: i64,
    number: String,
}

/// Marks an account as syncing, and clears the mark however the sync ends.
struct SyncGuard<'a>(&'a Mutex<HashSet<i64>>, i64);

impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.1);
    }
}

impl Engine {
    pub fn new(store: Store, secrets: Arc<dyn SecretStore>) -> Self {
        Self::with_options(store, secrets, SyncOptions::default())
    }

    pub fn with_options(store: Store, secrets: Arc<dyn SecretStore>, options: SyncOptions) -> Self {
        // Anything that makes a secure connection without saying how uses this. Only the
        // first call in a run takes effect, which is all that's needed.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let scratch = std::env::temp_dir().join(format!("inbox-{}", crate::crypto::random_hex(8)));
        Self {
            store,
            secrets,
            options,
            syncing: Mutex::new(HashSet::new()),
            inbox_gates: Mutex::default(),
            cloud: None,
            device_name: "This device".into(),
            number_tries: Mutex::new(HashMap::new()),
            url_opener: None,
            public_links_only: true,
            public_servers_only: false,
            google_auth_url: crate::google::GOOGLE_AUTH_URL.into(),
            google_cancel: Notify::new(),
            mail_proof: Mutex::new(None),
            code_asked_at: Mutex::new(None),
            data_dir: scratch.clone(),
            ai_app_places: crate::connections::Places::of_this_mac(),
            plan_programs: None,
            plan_turns: tokio::sync::Semaphore::new(crate::plans::AT_ONCE),
            mcp_sends: Mutex::new(Vec::new()),
            mcp_send_changed: Notify::new(),
            downloads_dir: scratch.join("downloads"),
            file_opener: None,
            saved_files: Mutex::new(HashSet::new()),
            sending: Mutex::new(HashSet::new()),
            sent_copy_wait_ms: 1500,
            newest_told: std::sync::atomic::AtomicI64::new(0),
            avatar_sources: (PERSON_SOURCE.into(), LOGO_SOURCE.into()),
            // Sends nothing about this person: no cookies, no referrer, a plain name.
            avatar_http: crate::cloud::tls_config()
                .ok()
                .and_then(|tls| {
                    reqwest::Client::builder()
                        .tls_backend_preconfigured(tls)
                        .user_agent("Mail")
                        .redirect(reqwest::redirect::Policy::limited(2))
                        .build()
                        .ok()
                })
                .unwrap_or_default(),
            avatar_turns: Semaphore::new(4),
            mail_watches: Mutex::new(HashMap::new()),
            alert_sender: None,
            new_mail_listener: None,
            team: Default::default(),
            embedder: None,
            on_device: None,
            on_device_turns: tokio::sync::Semaphore::new(1),
            working_out_meanings: Default::default(),
            locked: std::sync::atomic::AtomicBool::new(false),
            gmail_api: crate::autoreply::GMAIL_API.into(),
            graph_api: crate::autoreply::GRAPH_API.into(),
            caldav_base: None,
            google_calendar: crate::calendar::GOOGLE_CALENDAR.into(),
            microsoft_calendar: crate::calendar::MICROSOFT_CALENDAR.into(),
            mail_signins: Vec::new(),
            mail_tokens: tokio::sync::Mutex::new(HashMap::new()),
            directory_url: crate::providers::DIRECTORY_URL.into(),
            record_lookup: None,
            ai_url: crate::ai::AI_URL.into(),
            included_ai: None,
            included_held: Mutex::new(0.0),
            paid_ai: None,
            cloud_paid: None,
            follow_ups_sending: Mutex::new(HashSet::new()),
            // No redirects, so the key only ever goes to the address it's meant for.
            image_http: crate::images::image_client(),
            images_may_be_local: false,
            ai_http: crate::cloud::tls_config()
                .ok()
                .and_then(|tls| {
                    reqwest::Client::builder()
                        .tls_backend_preconfigured(tls)
                        .timeout(crate::ai::AI_TIMEOUT)
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .ok()
                })
                .unwrap_or_default(),
        }
    }

    /// How this device works out what text means, for searching by meaning.
    pub fn with_embedder(mut self, embedder: crate::meaning::Embedder) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// The Mac's own AI, for when the person hasn't connected a plan or key.
    pub fn with_on_device(mut self, ai: crate::on_device::OnDevice) -> Self {
        self.on_device = Some(ai);
        self
    }

    /// Stand-ins for the providers' out-of-office services, for tests and the demo.
    pub fn with_auto_reply_urls(mut self, gmail: &str, graph: &str) -> Self {
        self.gmail_api = gmail.to_string();
        self.graph_api = graph.to_string();
        self
    }

    /// Stand-ins for the calendar services, for tests and the demo.
    pub fn with_calendar_urls(mut self, caldav: &str, google: &str, microsoft: &str) -> Self {
        self.caldav_base = Some(caldav.to_string());
        self.google_calendar = google.to_string();
        self.microsoft_calendar = microsoft.to_string();
        self
    }

    /// Lets people sign in with this provider through the browser.
    pub fn with_mail_sign_in(mut self, sign_in: crate::signin::MailSignIn) -> Self {
        self.mail_signins.push(sign_in);
        self
    }

    /// Stand-ins for the settings directory and mail records, for tests and the demo.
    pub fn with_provider_lookup(
        mut self,
        directory: &str,
        records: crate::providers::RecordLookup,
    ) -> Self {
        self.directory_url = directory.trim_end_matches('/').to_string();
        self.record_lookup = Some(records);
        self
    }

    /// Points AI at a stand-in, for tests and the demo.
    pub fn with_ai_url(mut self, url: &str) -> Self {
        self.ai_url = url.trim_end_matches('/').to_string();
        self
    }

    pub fn with_alert_sender(mut self, sender: AlertSender) -> Self {
        self.alert_sender = Some(sender);
        self
    }

    pub fn with_new_mail_listener(mut self, listener: NewMailListener) -> Self {
        self.new_mail_listener = Some(listener);
        self
    }

    /// Tests and the demo only: uses stand-ins for the picture services.
    pub fn with_avatar_sources(mut self, person: &str, logo: &str) -> Self {
        self.avatar_sources = (person.to_string(), logo.to_string());
        self
    }

    /// Where drafts' attachments are kept, and where saved attachments go.
    pub fn with_folders(mut self, data_dir: PathBuf, downloads_dir: PathBuf) -> Self {
        self.data_dir = data_dir;
        self.downloads_dir = downloads_dir;
        self
    }

    fn mcp_status(&self) -> Result<Value> {
        let command = mcp_command()?;
        Ok(json!({
            "apps": crate::connections::list(&self.ai_app_places, &command),
            "setup": crate::connections::setup_text(&command),
            "last_used": self.store.setting("mcp_last_used")?,
        }))
    }

    /// For an engine on a shared web host, which must never be talked into reaching inside
    /// its own network. An email's links (such as one-click unsubscribe) only ever reach
    /// public addresses, as on every engine. Here mail servers (incoming and outgoing),
    /// calendars and the servers guessed for an address do too: each name is looked up
    /// once, refused if any address it leads to is private, and connected to at exactly
    /// that address, still checking the certificate is the name's.
    pub fn with_public_links_only(mut self) -> Self {
        self.public_links_only = true;
        self.public_servers_only = true;
        self
    }

    /// Where this engine's own connections to servers may go.
    pub(crate) fn reach(&self) -> crate::net::Reach {
        if self.public_servers_only {
            crate::net::Reach::PublicOnly
        } else {
            crate::net::Reach::Anywhere
        }
    }

    pub fn with_local_images(mut self) -> Self {
        self.images_may_be_local = true;
        self
    }

    /// Where a test keeps stand-ins for the AI companies' programs.
    pub fn with_plan_programs(mut self, folder: PathBuf) -> Self {
        self.plan_programs = Some(folder);
        self
    }

    pub fn with_ai_app_places(mut self, places: crate::connections::Places) -> Self {
        self.ai_app_places = places;
        self
    }

    pub fn with_file_opener(mut self, opener: FileOpener) -> Self {
        self.file_opener = Some(opener);
        self
    }

    /// How long to give the provider to file sent mail itself before saving a copy.
    pub fn with_sent_copy_wait(mut self, millis: u64) -> Self {
        self.sent_copy_wait_ms = millis;
        self
    }

    /// Lets the engine send the person to a page in their browser.
    pub fn with_url_opener(mut self, opener: UrlOpener) -> Self {
        self.url_opener = Some(opener);
        self
    }

    /// Tests and the demo only: uses a stand-in for Google's sign-in page.
    pub fn with_google_auth_url(mut self, url: &str) -> Self {
        self.google_auth_url = url.to_string();
        self
    }

    /// Connects to the account server, which sign-in and linking need.
    pub fn with_cloud(mut self, server: &str, device_name: &str) -> Result<Self> {
        self.cloud = Some(CloudClient::new(server)?);
        let secrets = self.secrets.clone();
        let token: crate::paid::TokenSource =
            Arc::new(move || secrets.get(crate::identity::SESSION).ok().flatten());
        self.cloud_paid = Some(Arc::new(crate::paid::CloudPaidAi::new(server, token)?));
        self.device_name = device_name.to_string();
        Ok(self)
    }

    pub(crate) fn cloud(&self) -> Result<&CloudClient> {
        self.cloud
            .as_ref()
            .ok_or_else(|| CoreError::Invalid("No account server is set up.".into()))
    }

    /// The sound chosen for notifications on this device.
    pub fn alert_tone(&self) -> Result<String> {
        self.setting("alert_tone")
    }

    /// Locks or unlocks. Unlocking is for the app to do once the Mac has checked it's its
    /// owner; it isn't reachable through `call`.
    pub fn set_locked(&self, on: bool) {
        self.locked.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Every command from the screens (and the development door) comes through here.
    /// While locked, only what the lock screen needs answers.
    pub async fn call(&self, command: &str, args: Value) -> Result<Value> {
        match command {
            "app_lock_state" => return Ok(json!({ "locked": self.is_locked() })),
            "app_lock_set" => {
                let a: LockArg = from_json(args)?;
                if !a.locked {
                    return Err(CoreError::Invalid(
                        "Only Touch ID or your Mac's password can unlock Mailzo.".into(),
                    ));
                }
                self.set_locked(true);
                return Ok(json!({ "locked": true }));
            }
            _ => {}
        }
        if self.is_locked() && !ALLOWED_WHILE_LOCKED.contains(&command) {
            return Err(locked_error());
        }
        self.run_command(command, args).await
    }

    async fn run_command(&self, command: &str, args: Value) -> Result<Value> {
        match command {
            "auth_status" => self.auth_status(),
            "auth_start" => {
                let a: EmailArg = from_json(args)?;
                self.auth_start(&a.email).await
            }
            "auth_with_mail_sign_in" => {
                let a: EmailArg = from_json(args)?;
                self.sign_in_with_mail_proof(&a.email).await
            }
            "auth_verify" => {
                let a: VerifyArg = from_json(args)?;
                self.auth_verify(&a.email, &a.code).await
            }
            "auth_options" => self.auth_options().await,
            "google_sign_in" => self.google_sign_in().await,
            "google_cancel" => {
                self.google_cancel.notify_waiters();
                Ok(json!({ "cancelled": true }))
            }
            "find_signin_code" => {
                let a: EmailArg = from_json(args)?;
                self.find_signin_code(&a.email).await
            }
            "finish_setup" => self.finish_setup().await,
            "link_status" => self.link_status().await,
            "link_with_recovery" => {
                let a: RecoveryArg = from_json(args)?;
                self.link_with_recovery(&a.recovery_key).await
            }
            "pending_devices" => self.pending_devices().await,
            "approve_device" => {
                let a: ApproveArg = from_json(args)?;
                self.approve_device(a.id, &a.number).await
            }
            "reject_device" => {
                let a: DeviceArg = from_json(args)?;
                self.reject_device(a.id).await
            }
            "list_devices" => self.list_devices().await,
            "remove_device" => {
                let a: DeviceArg = from_json(args)?;
                self.remove_device(a.id).await
            }
            "new_recovery_key" => self.new_recovery_key().await,
            "sign_out" => self.sign_out().await,
            "erase_device" => self.erase_device().await,
            "start_fresh" => self.start_fresh().await,
            "account_delete_start" => self.account_delete_start().await,
            "account_delete" => {
                let a: CodeArg = from_json(args)?;
                self.account_delete(&a.code).await
            }
            "vault_sync" => self.vault_sync().await,
            "add_account" => to_json(self.add_account(from_json(args)?).await?),
            "list_accounts" => to_json(self.store.list_accounts()?),
            "reorder_accounts" => {
                let a: InboxOrder = from_json(args)?;
                let order: Vec<String> = a.account_ids.iter().map(i64::to_string).collect();
                self.store.set_setting("inbox_order", &order.join(","))?;
                to_json(self.store.list_accounts()?)
            }
            "remove_account" => {
                let a: AccountArg = from_json(args)?;
                if self.store.account(a.account_id)?.prefs.team_shared {
                    return Err(CoreError::Invalid(
                        "This inbox belongs to your team. An admin can stop sharing it.".into(),
                    ));
                }
                self.remove_account(a.account_id)?;
                // Telling other devices can wait if this one is offline.
                let _ = self.vault_sync().await;
                Ok(json!({ "removed": true }))
            }
            "sync_account" => {
                let a: SyncArg = from_json(args)?;
                to_json(if a.inbox_only {
                    self.sync_inbox(a.account_id, a.quiet_now).await?
                } else {
                    self.sync_account(a.account_id, a.quiet_now).await?
                })
            }
            "list_folders" => {
                let a: AccountArg = from_json(args)?;
                let grouped = self.setting("conversations")? == "on";
                to_json(self.store.list_folders_counted(a.account_id, grouped)?)
            }
            "list_threads" => {
                let a: ListThreads = from_json(args)?;
                let limit = a.limit.clamp(1, 5000);
                let grouped = self.setting("conversations")? == "on";
                let view = a.tab.as_deref().filter(|t| t.starts_with("team_"));
                let (tab, reach, skip) = match view {
                    Some(_) => (None, TEAM_VIEW_REACH, 0),
                    None => (a.tab.as_deref(), limit, a.offset.max(0)),
                };
                let mut list = self.store.list_threads_filtered(
                    a.folder_id,
                    reach,
                    skip,
                    grouped,
                    tab,
                    a.unread_only,
                )?;
                self.add_team_info(&mut list)?;
                if let Some(view) = view {
                    self.keep_team_view(&mut list, view);
                    list = list
                        .into_iter()
                        .skip(a.offset.max(0) as usize)
                        .take(limit as usize)
                        .collect();
                }
                to_json(list)
            }
            "list_all" => {
                let a: ListAll = from_json(args)?;
                let grouped = self.setting("conversations")? == "on";
                let mut list = self.store.list_all_filtered(
                    &a.role,
                    a.limit.clamp(1, 5000),
                    a.offset.max(0),
                    grouped,
                    a.tab.as_deref(),
                    a.unread_only,
                )?;
                self.add_team_info(&mut list)?;
                to_json(list)
            }
            "thread_pin" => {
                let a: ThreadOnOff = from_json(args)?;
                self.thread_pin(a.account_id, &a.thread_id, a.on).await
            }
            "thread_mute" => {
                let a: ThreadOnOff = from_json(args)?;
                self.thread_mute(a.account_id, &a.thread_id, a.on).await
            }
            "thread_snooze" => {
                let a: ThreadWhen = from_json(args)?;
                self.thread_snooze(a.account_id, &a.thread_id, a.at).await
            }
            "thread_follow_up" => {
                let a: ThreadWhen = from_json(args)?;
                self.thread_follow_up(a.account_id, &a.thread_id, a.at)
                    .await
            }
            "later_counts" => to_json(self.store.later_counts()?),
            "mcp_status" => self.mcp_status(),
            "agent_activity" => self.agent_activity(),
            "agent_activity_clear" => self.clear_agent_activity(),
            "mcp_connect" => {
                let a: AiAppArg = from_json(args)?;
                crate::connections::connect(&self.ai_app_places, &a.id, &mcp_command()?)?;
                self.set_app_setting("mcp_on", "on").await?;
                self.mcp_status()
            }
            "mcp_send_next" => self.mcp_send_next().await,
            "mcp_send_decide" => {
                let a: McpSendDecision = from_json(args)?;
                self.mcp_send_decide(&a.id, a.send).await
            }
            "mcp_disconnect" => {
                let a: AiAppArg = from_json(args)?;
                crate::connections::disconnect(&self.ai_app_places, &a.id, &mcp_command()?)?;
                self.mcp_status()
            }
            "list_later" => {
                let a: KindArg = from_json(args)?;
                to_json(self.store.list_marked(&a.kind)?)
            }
            "schedule_draft" => {
                let a: ScheduleArg = from_json(args)?;
                self.schedule_draft(a.id, a.send_at)
            }
            "unschedule_draft" => {
                let a: DraftArg = from_json(args)?;
                let draft = self.store.draft(a.id)?;
                self.store.set_send_at(a.id, None, draft.remind_days)?;
                to_json(self.store.draft(a.id)?)
            }
            "draft_remind" => {
                let a: RemindArg = from_json(args)?;
                let draft = self.store.draft(a.id)?;
                self.store.set_send_at(
                    a.id,
                    draft.send_at,
                    a.days.filter(|d| (1..=365).contains(d)),
                )?;
                to_json(self.store.draft(a.id)?)
            }
            "due_tick" => {
                let a: DueArg = from_json(args)?;
                self.due_tick(a.quiet_now, a.at).await
            }
            "calendar_list" => self.calendar_list(),
            "calendar_connect" => {
                let a: AccountArg = from_json(args)?;
                self.calendar_connect(a.account_id).await
            }
            "calendar_sync" => {
                self.calendar_sync(true).await?;
                self.calendar_list()
            }
            "calendar_events" => {
                let a: RangeArg = from_json(args)?;
                self.calendar_events(a.from, a.to)
            }
            "calendar_save" => {
                let a: EventArg = from_json(args)?;
                self.calendar_save(a.event).await
            }
            "calendar_delete" => {
                let a: DeviceArg = from_json(args)?;
                self.calendar_delete(a.id).await
            }
            "invite_for_message" => {
                let a: MessageArg = from_json(args)?;
                self.invite_for_message(a.message_id).await
            }
            "invite_respond" => {
                let a: InviteArg = from_json(args)?;
                self.invite_respond(a.message_id, &a.status).await
            }
            "invite_cancelled" => {
                let a: MessageArg = from_json(args)?;
                self.invite_cancelled(a.message_id).await
            }
            "rules_list" => to_json(self.rules()?),
            "rule_understand" => {
                let a: SentenceArg = from_json(args)?;
                self.rule_understand(&a.sentence).await
            }
            "rule_preview" => {
                let a: RuleArg = from_json(args)?;
                self.rule_preview(&a.rule)
            }
            "rule_try_ai" => {
                let a: RuleArg = from_json(args)?;
                self.rule_try_ai(&a.rule).await
            }
            "rule_save" => {
                let a: RuleArg = from_json(args)?;
                self.rule_save(a.rule).await
            }
            "rule_delete" => {
                let a: TeamIdArg = from_json(args)?;
                self.rule_delete(&a.id).await
            }
            "rule_apply_now" => {
                let a: TeamIdArg = from_json(args)?;
                self.rule_apply_now(&a.id).await
            }
            "screener_list" => to_json(self.store.screener_rows()?),
            "screener_decide" => {
                let a: AddressDecision = from_json(args)?;
                self.screener_decide(&a.address, a.allow).await
            }
            "list_senders" => {
                let a: SenderList = from_json(args)?;
                to_json(self.store.senders_in(a.folder_id)?)
            }
            "newsletters" => to_json(self.newsletters()?),
            "mail_with" => {
                let a: MailWith = from_json(args)?;
                to_json(self.store.mail_with(a.account_id, &a.addresses)?)
            }
            "list_from" => {
                let a: FromList = from_json(args)?;
                to_json(
                    self.store
                        .threads_from(a.folder_id, &a.address, a.screened)?,
                )
            }
            "thread_shared" => {
                let a: ThreadOnly = from_json(args)?;
                to_json(self.thread_shared(a.account_id, &a.thread_id).await?)
            }
            "style_get" => {
                let a: AccountArg = from_json(args)?;
                self.style_get(a.account_id)
            }
            "style_save" => {
                let a: StyleArg = from_json(args)?;
                self.style_save(a.account_id, &a.text)
            }
            "style_make" => {
                let a: AccountArg = from_json(args)?;
                self.style_make(a.account_id).await
            }
            "sequences_list" => to_json(self.store.list_sequences()?),
            "sequence_save" => {
                let a: SequenceArg = from_json(args)?;
                self.sequence_save(a.sequence)
            }
            "sequence_delete" => {
                let a: IdArg = from_json(args)?;
                self.store.delete_sequence(a.id)?;
                Ok(json!({ "deleted": true }))
            }
            "sequence_start" => {
                let a: SequenceStartArg = from_json(args)?;
                self.sequence_start(a.message_id, a.sequence_id, a.steps)
            }
            "sequence_stop" => {
                let a: IdArg = from_json(args)?;
                self.sequence_stop(a.id)
            }
            "sequence_runs" => {
                let a: RunsArg = from_json(args)?;
                let thread = a
                    .thread_id
                    .map(|t| self.store.real_thread(&t))
                    .transpose()?;
                to_json(self.store.runs(a.account_id, thread.as_deref())?)
            }
            "tags_list" => to_json(self.store.list_tags()?),
            "tag_save" => {
                let a: TagArg = from_json(args)?;
                self.tag_save(a.id, &a.name, &a.colour)
            }
            "tag_delete" => {
                let a: TagIdArg = from_json(args)?;
                self.store.delete_tag(a.tag_id)?;
                Ok(json!({ "deleted": true }))
            }
            "thread_tag" => {
                let a: ThreadTagArg = from_json(args)?;
                self.thread_tag(a.account_id, &a.thread_id, a.tag_id, a.on, a.amount)
            }
            "tagged_threads" => {
                let a: TagIdArg = from_json(args)?;
                to_json(self.tagged_threads(a.tag_id)?)
            }
            "my_templates" => self.my_templates(),
            "save_my_template" => {
                let a: TemplateArg = from_json(args)?;
                self.save_my_template(a.id.as_deref(), &a.name, &a.body)
                    .await
            }
            "delete_my_template" => {
                let a: TeamIdArg = from_json(args)?;
                self.delete_my_template(&a.id).await
            }
            "note_trackers" => {
                let a: TrackersArg = from_json(args)?;
                self.store.note_trackers(a.message_id, &a.companies)?;
                Ok(json!({ "noted": true }))
            }
            "search_meaning" => {
                let a: TextArg = from_json(args)?;
                self.search_meaning(&a.text, None)
            }
            "ai_summarize" => {
                let a: SummarizeArg = from_json(args)?;
                self.ai_summarize(
                    a.account_id,
                    &a.thread_id,
                    a.language.as_deref(),
                    a.saved_only,
                )
                .await
            }
            "ai_explain" => {
                let a: SummarizeArg = from_json(args)?;
                self.ai_explain(
                    a.account_id,
                    &a.thread_id,
                    a.language.as_deref(),
                    a.saved_only,
                )
                .await
            }
            "ai_write_reply" => {
                let a: WriteReplyArg = from_json(args)?;
                self.ai_write_reply(a.message_id, &a.instruction).await
            }
            "ai_short_replies" => {
                let a: WriteReplyArg = from_json(args)?;
                self.ai_short_replies(a.message_id).await
            }
            "ai_brief" => {
                let a: BriefArg = from_json(args)?;
                self.ai_brief(a.saved_only, a.language.as_deref()).await
            }
            "ai_polish" => {
                let a: PolishArg = from_json(args)?;
                self.ai_polish(&a.text, &a.how, a.account_id).await
            }
            "ai_translate" => {
                let a: TranslateArg = from_json(args)?;
                self.ai_translate(a.message_id, &a.language).await
            }
            "create_folder" => {
                let a: NewFolderArg = from_json(args)?;
                self.create_folder(a.account_id, &a.name).await
            }
            "delete_folder" => {
                let a: DeleteFolderArg = from_json(args)?;
                self.delete_folder(a.account_id, a.folder_id).await
            }
            "mail_wait" => {
                let a: AccountArg = from_json(args)?;
                self.mail_wait(a.account_id).await
            }
            "mail_wait_stop" => {
                let a: AccountArg = from_json(args)?;
                self.stop_mail_watch(a.account_id);
                Ok(json!({ "stopped": true }))
            }
            "ai_ask_sources" => {
                let a: AskSourcesArg = from_json(args)?;
                self.ai_ask_sources(
                    &a.question,
                    a.filters,
                    a.scope,
                    a.quick,
                    &a.previous,
                    a.previous_plan.as_ref(),
                    a.about
                        .as_ref()
                        .map(|t| (t.account_id, t.thread_id.as_str())),
                )
                .await
            }
            "ai_chat" => {
                let a: ChatArg = from_json(args)?;
                self.ai_chat(
                    &a.question,
                    &a.history,
                    &a.ids,
                    &a.related,
                    a.language.as_deref(),
                    &a.plan,
                )
                .await
            }
            "ai_ask" => {
                let a: AskArg = from_json(args)?;
                self.ai_ask(&a.question, &a.ids, a.language.as_deref())
                    .await
            }
            "paid_status" => self.paid_status().await,
            "ai_spend" => self.ai_spend(),
            "auto_reply_get" => {
                let a: AccountArg = from_json(args)?;
                self.auto_reply_get(a.account_id).await
            }
            "auto_reply_set" => {
                let a: AutoReplyArg = from_json(args)?;
                self.auto_reply_set(a.account_id, a.reply).await
            }
            "draft_from_message" => {
                let a: MessageArg = from_json(args)?;
                self.draft_from_message(a.message_id).await
            }
            // Tests only: skips the pause after typing. Nothing is sent by this.
            "draft_age_for_tests" if cfg!(debug_assertions) => {
                self.store.db().execute(
                    "UPDATE drafts SET updated_at = updated_at - 60, server_saved_at = MIN(server_saved_at, updated_at - 61)",
                    [],
                )?;
                Ok(json!({ "aged": true }))
            }
            "team_status" => self.team_status(asked_team(&args)),
            "teams_status" => self.teams_status(),
            "team_refresh" => self.team_refresh().await,
            "teams_refresh" => {
                self.team_refresh().await?;
                self.teams_status()
            }
            "team_create" => {
                let a: TeamNameArg = from_json(args)?;
                self.team_create(&a.name).await
            }
            "team_invite" => {
                let team = asked_team(&args);
                let a: TeamRoleArg = from_json(args)?;
                self.team_invite(team, &a.role).await
            }
            "team_invites" => self.team_invites(asked_team(&args)).await,
            "team_cancel_invite" => {
                let team = asked_team(&args);
                let a: DeviceArg = from_json(args)?;
                self.team_cancel_invite(team, a.id).await
            }
            "team_join" => {
                let a: TeamCodeArg = from_json(args)?;
                self.team_join(&a.code).await
            }
            "team_leave" => self.team_leave(asked_team(&args)).await,
            "team_delete" => self.team_delete(asked_team(&args)).await,
            "team_remove_member" => {
                let team = asked_team(&args);
                let a: DeviceArg = from_json(args)?;
                self.team_remove_member(team, a.id).await
            }
            "team_set_role" => {
                let team = asked_team(&args);
                let a: TeamMemberRole = from_json(args)?;
                self.team_set_role(team, a.id, &a.role).await
            }
            "team_share_inbox" => {
                let team = asked_team(&args);
                let a: AccountArg = from_json(args)?;
                self.team_share_inbox(team, a.account_id).await
            }
            "team_unshare_inbox" => {
                let a: AccountArg = from_json(args)?;
                self.team_unshare_inbox(a.account_id).await
            }
            "team_poll" => {
                let a: TeamPoll = from_json(args)?;
                self.team_poll(a.wait).await
            }
            "team_thread" => {
                let a: TeamThreadArg = from_json(args)?;
                self.team_thread_view(a.account_id, &a.thread_id)
            }
            "team_assign" => {
                let a: TeamAssign = from_json(args)?;
                self.team_assign(a.account_id, &a.thread_id, a.expected, a.assignee)
                    .await
            }
            "team_set_status" => {
                let a: TeamStatusArg = from_json(args)?;
                self.team_set_status(a.account_id, &a.thread_id, &a.status)
                    .await
            }
            "team_comment" => {
                let a: TeamCommentArg = from_json(args)?;
                self.team_comment(a.account_id, &a.thread_id, &a.text, a.mentions)
                    .await
            }
            "team_delete_comment" => {
                let a: TeamIdArg = from_json(args)?;
                self.team_delete_comment(&a.id).await
            }
            "team_presence" => {
                let a: TeamPresenceArg = from_json(args)?;
                self.team_presence(a.account_id, &a.thread_id, &a.kind)
                    .await
            }
            "team_templates" => self.team_templates(asked_team(&args)),
            "team_save_template" => {
                let team = asked_team(&args);
                let a: TeamTemplateArg = from_json(args)?;
                self.team_save_template(team, a.id.as_deref(), &a.name, &a.body, a.expected_version)
                    .await
            }
            "team_delete_template" => {
                let team = asked_team(&args);
                let a: TeamTemplateDelete = from_json(args)?;
                self.team_delete_template(team, &a.id, a.expected_version)
                    .await
            }
            "get_thread" => {
                let a: GetThread = from_json(args)?;
                to_json(
                    self.store
                        .get_thread(a.account_id, &a.thread_id, a.folder_id)?,
                )
            }
            "thread_summary" => {
                let a: GetThread = from_json(args)?;
                to_json(self.store.summary_of(a.account_id, &a.thread_id)?)
            }
            "set_thread_seen" => {
                let a: ThreadSeen = from_json(args)?;
                self.set_thread_seen(a.account_id, &a.thread_id, a.seen)
                    .await
            }
            "set_thread_flagged" => {
                let a: ThreadFlagged = from_json(args)?;
                self.set_thread_flagged(a.account_id, &a.thread_id, a.flagged)
                    .await
            }
            "move_thread" => {
                let a: MoveThread = from_json(args)?;
                self.move_thread(a.account_id, &a.thread_id, a.folder_id, &a.to)
                    .await
            }
            "delete_forever" => {
                let a: DeleteForever = from_json(args)?;
                self.delete_forever(a.account_id, &a.thread_id, a.folder_id)
                    .await
            }
            "bulk_threads" => {
                let a: BulkArg = from_json(args)?;
                match a.step.as_deref() {
                    Some(step) => self.bulk_seen_step(a.threads, &a.action, step).await,
                    None => {
                        self.bulk_threads(a.threads, &a.action, a.to.as_deref())
                            .await
                    }
                }
            }
            "save_attachment" => {
                let a: SaveAttachment = from_json(args)?;
                self.save_attachment(a.message_id, a.part).await
            }
            "attachment_preview" => {
                let a: SaveAttachment = from_json(args)?;
                self.attachment_preview(a.message_id, a.part).await
            }
            "use_saved_file" => {
                let a: SavedFile = from_json(args)?;
                self.use_saved_file(&a.path, a.open)
            }
            "draft_new" => {
                let a: NewDraft = from_json(args)?;
                let draft = self
                    .draft_new(a.account_id, &a.mode, a.message_id, a.date_text.as_deref())
                    .await?;
                self.store.clear_draft_team_base(draft.id)?;
                let account = self.store.account(a.account_id)?;
                if let (true, Some(parent)) = (account.prefs.team_shared, draft.parent_message_id) {
                    if let Some(thread_id) = self.store.message_thread(parent)? {
                        self.note_team_draft(draft.id, &account, &thread_id)?;
                    }
                }
                to_json(draft)
            }
            "draft_get" => {
                let a: DraftArg = from_json(args)?;
                to_json(self.store.draft(a.id)?)
            }
            "draft_list" => to_json(self.store.drafts()?),
            "draft_save" => {
                let a: SaveDraft = from_json(args)?;
                to_json(self.draft_save(DraftUpdate {
                    id: a.id,
                    account_id: a.account_id,
                    from_address: a.from_address,
                    to: a.to,
                    cc: a.cc,
                    bcc: a.bcc,
                    subject: a.subject,
                    html: a.html,
                    text: a.text,
                    encrypt: a.encrypt,
                    sign: a.sign,
                    allow_plain: a.allow_plain,
                })?)
            }
            "pgp_status" => {
                let a: PgpAccountArg = from_json(args)?;
                self.pgp_status(a.account_id)
            }
            "pgp_generate" => {
                let a: PgpAccountArg = from_json(args)?;
                self.pgp_generate(a.account_id)
            }
            "pgp_import_secret" => {
                let a: PgpImportSecretArg = from_json(args)?;
                self.pgp_import_secret(a.account_id, &a.armored, a.passphrase.as_deref())
            }
            "pgp_remove" => {
                let a: PgpAccountArg = from_json(args)?;
                self.pgp_remove(a.account_id)
            }
            "pgp_keys_list" => Ok(Value::Array(self.store.pgp_keys_list()?)),
            "pgp_key_import" => {
                let a: PgpArmoredArg = from_json(args)?;
                self.pgp_key_import(&a.armored)
            }
            "pgp_key_delete" => {
                let a: PgpAddressArg = from_json(args)?;
                self.pgp_key_delete(&a.address)
            }
            "pgp_key_accept_change" => {
                let a: PgpAddressArg = from_json(args)?;
                self.pgp_key_accept_change(&a.address)
            }
            "pgp_can_encrypt" => {
                let a: PgpCanEncryptArg = from_json(args)?;
                self.pgp_can_encrypt(a.account_id, &a.to)
            }
            "draft_discard" => {
                let a: DraftArg = from_json(args)?;
                self.draft_discard(a.id)?;
                Ok(json!({ "discarded": true }))
            }
            "draft_add_attachment" => {
                let a: AddAttachment = from_json(args)?;
                to_json(self.draft_add_attachment(a.id, &a.name, &a.mime, &a.data)?)
            }
            "draft_remove_attachment" => {
                let a: RemoveAttachment = from_json(args)?;
                to_json(self.draft_remove_attachment(a.id, &a.attachment_id)?)
            }
            "send_draft" => {
                let a: SendArg = from_json(args)?;
                if a.allow_plain {
                    self.store.allow_plain_draft(a.id)?;
                }
                self.send_draft(a.id, a.force).await
            }
            "contacts" => {
                let a: ContactsArg = from_json(args)?;
                to_json(self.store.contacts(a.account_id, &a.query, 8)?)
            }
            "people_list" => {
                let a: PeopleListArg = from_json(args)?;
                self.people_list(&a.query, a.limit)
            }
            "person_get" => {
                let a: PersonAddressArg = from_json(args)?;
                self.person_get(&a.address)
            }
            "person_save" => self.person_save(from_json(args)?).await,
            "person_delete" => {
                let a: PersonIdArg = from_json(args)?;
                self.person_delete(a.id).await
            }
            "people_import_vcard" => {
                let a: VcardTextArg = from_json(args)?;
                self.people_import_vcard(&a.text).await
            }
            "people_export_vcard" => self.people_export_vcard(),
            "people_export_file" => self.people_export_file(),
            "avatar" => {
                let a: AvatarArg = from_json(args)?;
                self.avatar(&a.address).await
            }
            "app_settings" => self.app_settings(),
            "spam_mark" => {
                let a: SpamMarkArg = from_json(args)?;
                self.spam_mark(a.threads, a.block).await
            }
            "spam_clear" => {
                let a: SpamClearArg = from_json(args)?;
                self.spam_clear(a.threads).await
            }
            "spam_lists" => self.spam_lists(),
            "spam_unlist" => {
                let a: UnlistArg = from_json(args)?;
                self.spam_unlist(&a.address, &a.kind).await
            }
            "spam_report" => self.spam_report(),
            "sort_sender" => {
                let a: SortSenderArg = from_json(args)?;
                self.sort_sender(&a.address, &a.category).await
            }
            "tab_counts" => {
                let a: TabCountsArg = from_json(args)?;
                let grouped = self.setting("conversations")? == "on";
                let mut total: BTreeMap<String, i64> = BTreeMap::new();
                for (folder, prefs) in self.store.inbox_folder_ids()? {
                    let included: crate::model::AccountPrefs =
                        serde_json::from_str(&prefs).unwrap_or_default();
                    let wanted = match a.folder_id {
                        Some(id) => id == folder,
                        None => included.in_all_inbox,
                    };
                    if wanted {
                        for (tab, n) in self.store.tab_counts(folder, grouped)? {
                            *total.entry(tab).or_default() += n;
                        }
                        *total.entry("needs_you".into()).or_default() +=
                            self.store.needs_you_count(folder, a.unread)?;
                    }
                }
                to_json(total)
            }
            "thread_priority" => {
                let a: ThreadPriorityArg = from_json(args)?;
                self.store
                    .set_thread_priority(a.account_id, &a.thread_id, a.group.as_deref())?;
                Ok(json!({ "ok": true }))
            }
            "sender_priority" => {
                let a: SenderPriorityArg = from_json(args)?;
                self.set_sender_priority(&a.address, &a.group)?;
                Ok(json!({ "ok": true }))
            }
            "brief" => self.brief(),
            "unsubscribe" => {
                let a: UnsubscribeArg = from_json(args)?;
                self.unsubscribe(a.message_id, a.quietly).await
            }
            "newsletter_forget" => {
                let a: ForgetLeftArg = from_json(args)?;
                self.forget_left(a.account_id, &a.address).await?;
                Ok(json!({ "ok": true }))
            }
            "mail_sign_in" => {
                let a: MailSignInArg = from_json(args)?;
                to_json(self.mail_sign_in(&a.email, &a.provider).await?)
            }
            "mail_sign_in_cancel" => {
                self.google_cancel.notify_waiters();
                Ok(json!({ "cancelled": true }))
            }
            "sign_in_providers" => to_json(self.sign_in_providers()),
            "find_provider" => {
                let a: FindProviderArg = from_json(args)?;
                self.find_provider_reply(&a.email).await
            }
            "search_mail" => {
                let a: SearchArg = from_json(args)?;
                self.search_mail(&a.filters(), &a.extra_ids, a.most)
            }
            "search_servers" => {
                let a: SearchArg = from_json(args)?;
                self.search_servers(&a.filters()).await
            }
            "search_understand" => {
                let a: UnderstandArg = from_json(args)?;
                self.search_understand(&a.text, &a.today, a.utc_offset_minutes)
                    .await
            }
            "ai_status" => self.ai_status(),
            "ai_set_key" => {
                let a: AiKeyArg = from_json(args)?;
                self.ai_set_key(&a.key, a.service.as_deref()).await
            }
            "ai_choose_model" => {
                let a: AiModelArg = from_json(args)?;
                self.ai_choose_model(&a.model).await
            }
            "ai_remove_key" => self.ai_remove_key().await,
            "ai_plans" => to_json(self.ai_plans().await?),
            "ai_plan_sign_in" => {
                let a: AiAppArg = from_json(args)?;
                self.ai_plan_sign_in(&a.id)
            }
            "ai_plan_use" => {
                let a: AiAppArg = from_json(args)?;
                self.ai_plan_use(&a.id).await
            }
            "set_setting" => {
                let a: SettingArg = from_json(args)?;
                self.set_app_setting(&a.key, &a.value).await
            }
            "update_account_prefs" => {
                let a: PrefsArg = from_json(args)?;
                self.update_account_prefs(a.account_id, a.prefs).await
            }
            "update_connection" => {
                let a: ConnectionArg = from_json(args)?;
                let account_id = a.account_id;
                let account = self.store.account(account_id)?;
                let shared = account.prefs.team_shared;
                let team = self.account_team(&account)?;
                let member = match team {
                    Some(team) => self
                        .store
                        .team_info(team)?
                        .is_some_and(|i| i.role == "member"),
                    None => false,
                };
                if member {
                    return Err(CoreError::Invalid(
                        "This inbox belongs to your team, so an admin needs to change this.".into(),
                    ));
                }
                let reply = self
                    .update_connection(ConnectionUpdate {
                        account_id,
                        imap_host: a.imap_host,
                        imap_port: a.imap_port,
                        security: a.security,
                        username: a.username,
                        password: a.password,
                    })
                    .await?;
                if shared {
                    self.team_publish_inbox(account_id).await?;
                }
                Ok(reply)
            }
            "alert_test" => {
                if let Some(sender) = &self.alert_sender {
                    sender(&crate::alerts::Alert {
                        subtitle: String::new(),
                        title: "Mailzo".to_string(),
                        body: "Notifications are working.".to_string(),
                        sound: self.setting("alert_sound")? == "on",
                        thread: None,
                    });
                }
                Ok(json!({ "sent": true }))
            }
            "note_images_held" => {
                let a: MessageArg = from_json(args)?;
                self.store.note_images_held(a.message_id)?;
                Ok(json!({ "noted": true }))
            }
            "privacy_stats" => self.privacy_stats(),
            "mail_health" => {
                let keep = self.keep_since()?;
                Ok(json!({
                    "inboxes": self.store.mail_health(self.options.body_max_bytes, keep)?,
                    "bytes": self.store.database_size()?,
                }))
            }
            "clear_saved_mail" => {
                self.store.clear_saved_mail()?;
                Ok(json!({ "cleared": true }))
            }
            "image_senders" => to_json(self.store.image_senders()?),
            "set_sender_images" => {
                let a: SenderImages = from_json(args)?;
                self.set_sender_images(&a.address, a.allowed).await
            }
            "check_smtp" => {
                let a: AccountArg = from_json(args)?;
                self.check_smtp(a.account_id).await
            }
            "update_account_settings" => {
                let a: UpdateSettings = from_json(args)?;
                let reply = self.update_account_settings(SettingsUpdate {
                    account_id: a.account_id,
                    display_name: a.display_name,
                    signature: a.signature,
                    smtp_host: a.smtp_host,
                    smtp_port: a.smtp_port,
                    smtp_security: a.smtp_security,
                })?;
                // Telling other devices can wait if this one is offline.
                let _ = self.vault_sync().await;
                Ok(reply)
            }
            "fetch_body" => {
                let a: MessageArg = from_json(args)?;
                to_json(self.fetch_body(a.message_id).await?)
            }
            "message_source" => {
                let a: MessageArg = from_json(args)?;
                self.message_source(a.message_id).await
            }
            "save_original" => {
                let a: MessageArg = from_json(args)?;
                self.save_original(a.message_id).await
            }
            other => Err(CoreError::Invalid(format!("Unknown command: {other}"))),
        }
    }

    async fn add_account(&self, a: AddAccount) -> Result<Account> {
        let email = a.email.trim().to_string();
        let host = a.imap_host.trim().to_ascii_lowercase();
        if !email.contains('@') {
            return Err(CoreError::Invalid("Enter a valid email address.".into()));
        }
        if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CoreError::Invalid("Enter a valid server name.".into()));
        }
        if a.imap_port == 0 {
            return Err(CoreError::Invalid("Enter a valid server port.".into()));
        }
        if a.password.is_empty() {
            return Err(CoreError::Invalid("Enter a password.".into()));
        }
        check_security(&host, a.security)?;
        if self.store.account_exists(&email, &host)? {
            return Err(CoreError::Invalid(
                "This account has already been added.".into(),
            ));
        }

        let username = match a.username.trim() {
            "" => email.clone(),
            u => u.to_string(),
        };
        let display_name = match a.display_name.trim() {
            "" => email.clone(),
            n => n.to_string(),
        };

        // Prove the details work before saving anything.
        let probe = Account {
            id: 0,
            sync_id: String::new(),
            email: email.clone(),
            display_name: display_name.clone(),
            imap_host: host.clone(),
            imap_port: a.imap_port,
            security: a.security,
            username: username.clone(),
            smtp_host: String::new(),
            smtp_port: 0,
            smtp_security: SmtpSecurity::Tls,
            signature: String::new(),
            settings_updated_at: 0,
            prefs: AccountPrefs::default(),
            auth: crate::signin::PASSWORD.to_string(),
        };
        ImapConnection::connect(&probe, &a.password, self.reach())
            .await?
            .logout()
            .await;

        let account = self.store.insert_account(
            &email,
            &display_name,
            &host,
            a.imap_port,
            a.security,
            &username,
        )?;
        if let Err(e) = self.secrets.set(&account_secret(account.id), &a.password) {
            self.store.delete_account(account.id)?;
            return Err(e);
        }
        let smtp_host = a.smtp_host.trim().to_ascii_lowercase();
        if !smtp_host.is_empty() {
            self.store
                .set_smtp(account.id, &smtp_host, a.smtp_port, a.smtp_security)?;
        }
        let account = self.store.account(account.id)?;
        // Telling other devices can wait if this one is offline.
        let _ = self.vault_sync().await;
        Ok(account)
    }

    /// Adds a folder to one inbox, on its mail server, so it's there on every device.
    async fn create_folder(&self, account_id: i64, name: &str) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::Invalid("Give the folder a name.".into()));
        }
        if name.chars().count() > 60 {
            return Err(CoreError::Invalid("That name is too long.".into()));
        }
        // These split a folder into levels, or mean something else, on mail servers.
        if name
            .chars()
            .any(|c| c.is_control() || "/\\.*%\"".contains(c))
        {
            return Err(CoreError::Invalid(
                "A folder name can't include / \\ . * % or \".".into(),
            ));
        }
        let folders = self.store.list_folders(account_id)?;
        // "Inbox" and friends mean something, even when a folder's real name differs.
        let taken = folders.iter().any(|f| f.name.eq_ignore_ascii_case(name))
            || [
                "inbox", "sent", "drafts", "archive", "spam", "junk", "trash",
            ]
            .contains(&name.to_lowercase().as_str());
        if taken {
            return Err(CoreError::Invalid(format!(
                "You already have a folder called {name}."
            )));
        }
        let account = self.store.account(account_id)?;
        let password = self.password(account_id).await?;
        let mut conn = ImapConnection::connect(&account, &password, self.reach()).await?;
        conn.create_folder(name).await?;
        let list: Vec<(String, String, String)> =
            crate::sync::with_chosen_roles(conn.folders().await?, &account)
                .into_iter()
                .map(|f| (f.path, f.name, f.role))
                .collect();
        self.store.replace_folders(account_id, &list)?;
        conn.logout().await;
        let made = self
            .store
            .list_folders(account_id)?
            .into_iter()
            .find(|f| f.name == name || f.path.ends_with(name))
            .ok_or_else(|| {
                CoreError::Network("The mail server didn't keep the new folder.".into())
            })?;
        to_json(made)
    }

    /// Waits for the mail server to say the inbox changed, so new mail shows at once. The
    /// screens ask again straight after, and refresh every minute whether or not this works.
    async fn mail_wait(&self, account_id: i64) -> Result<Value> {
        let account = self.store.account(account_id)?;
        let Some(inbox) = self
            .store
            .list_folders(account_id)?
            .into_iter()
            .find(|f| f.role == "inbox")
        else {
            return Ok(json!({ "supported": false, "changed": false }));
        };
        // One connection per inbox, and a newer one takes over, as after waking from sleep
        // the old one may be dead. Whoever was waiting on the older one hears what the newer
        // one finds, so several pages on one inbox (two browser tabs) never take turns.
        let stop = Arc::new(Notify::new());
        let (told, _) = tokio::sync::broadcast::channel(1);
        let older = self
            .mail_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                account_id,
                MailWatch {
                    stop: stop.clone(),
                    told: told.clone(),
                },
            );
        if let Some(older) = older {
            older.stop.notify_one();
        }
        let _leave = LeaveWatch {
            watches: &self.mail_watches,
            account_id,
            stop: stop.clone(),
        };
        let outcome = async {
            let password = self.password(account_id).await?;
            let conn = ImapConnection::connect(&account, &password, self.reach()).await?;
            // Not while refreshing it keeps failing, or this would just keep logging in to say so.
            let (_, known_next, _) = self.store.folder_server_state(inbox.id)?;
            let known_next =
                known_next.filter(|_| !self.store.folder_has_problem(inbox.id).unwrap_or(true));
            conn.wait_for_change(&inbox.path, known_next, MAIL_WAIT_LIMIT, stop.clone())
                .await
        }
        .await;
        let newer = if matches!(outcome, Ok(MailWait::Stopped)) {
            self.mail_watches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&account_id)
                .filter(|w| !Arc::ptr_eq(&w.stop, &stop))
                .map(|w| w.told.subscribe())
        } else {
            None
        };
        let result = match (&outcome, newer) {
            (_, Some(mut newer)) => newer.recv().await.unwrap_or((true, false)),
            (Ok(o), None) => (*o != MailWait::NotSupported, *o == MailWait::Changed),
            (Err(_), None) => (true, false),
        };
        let _ = told.send(result);
        outcome?;
        let outcome = result;
        Ok(json!({ "supported": outcome.0, "changed": outcome.1 }))
    }

    fn stop_mail_watch(&self, account_id: i64) {
        let watch = self
            .mail_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&account_id);
        if let Some(watch) = watch {
            watch.stop.notify_one();
        }
    }

    fn remove_account(&self, account_id: i64) -> Result<()> {
        self.stop_mail_watch(account_id);
        let account = self.store.account(account_id)?;
        for draft in self.store.draft_ids_for_account(account_id)? {
            self.remove_draft_files(draft);
        }
        self.secrets.delete(&account_secret(account_id))?;
        if self
            .secrets
            .get(&crate::pgp::pgp_secret(account_id))?
            .is_some()
        {
            self.secrets.delete(&crate::pgp::pgp_secret(account_id))?;
        }
        if let Ok(mut tokens) = self.mail_tokens.try_lock() {
            tokens.remove(&account_id);
        }
        self.store.add_tombstone(&account.sync_id)?;
        self.store.forget_summaries(account_id)?;
        self.store.delete_account(account_id)?;
        // Its mail can be gigabytes, which the Mac should get back.
        self.store.give_room_back();
        Ok(())
    }

    /// What to connect with: their password, or a fresh access token if they signed
    /// in through the browser.
    pub(crate) async fn password(&self, account_id: i64) -> Result<String> {
        let secret = self
            .secrets
            .get(&account_secret(account_id))?
            .ok_or_else(|| CoreError::Secret("The password for this account is missing.".into()))?;
        let account = self.store.account(account_id)?;
        if account.auth == crate::signin::PASSWORD {
            return Ok(secret);
        }
        self.access_token(&account, &secret).await
    }

    async fn sync_account(&self, account_id: i64, quiet_now: bool) -> Result<SyncStats> {
        let account = self.store.account(account_id)?;
        {
            let mut running = self.syncing.lock().unwrap_or_else(|e| e.into_inner());
            if !running.insert(account_id) {
                return Ok(SyncStats {
                    already_running: true,
                    ..Default::default()
                });
            }
        }
        let _guard = SyncGuard(&self.syncing, account_id);
        self.store.note_sync_try(account_id)?;
        let attempt = || async {
            let password = self.password(account_id).await?;
            let mut options = self.options.clone();
            if options.keep_since == 0 {
                options.keep_since = self.keep_since()?;
            }
            // Paused: only what's new comes in, nothing older.
            if self.setting("download_paused")? == "on" {
                options.backfill_seconds = 0;
            }
            let ring = self.pgp_ring(account_id);
            sync::sync_account(
                &self.store,
                &account,
                &password,
                &options,
                &ring,
                self.reach(),
            )
            .await
        };
        let mut synced = attempt().await;
        // A token turned down that looked fine is renewed and tried once more, so nobody is
        // asked to sign in over a token that only had to be replaced.
        if matches!(synced, Err(CoreError::NeedsSignIn(_)))
            && account.auth != crate::signin::PASSWORD
        {
            self.forget_token(account_id).await;
            synced = attempt().await;
        }
        // The server turned down a token that looked fine: ask for a new one next time.
        if matches!(synced, Err(CoreError::NeedsSignIn(_))) {
            self.forget_token(account_id).await;
        }
        let problem = match &synced {
            Ok(stats) => stats.errors.first().cloned(),
            Err(e) => Some(e.to_string()),
        };
        self.store
            .note_sync_result(account_id, problem.as_deref())?;
        let mut stats = synced?;
        self.act_on_new_mail(&account, &mut stats, quiet_now)
            .await?;
        if let Err(e) = self.work_out_meanings() {
            stats.errors.push(e.to_string());
        }
        if let Err(e) = self.store.fill_some_facts(FACTS_PER_REFRESH) {
            stats.errors.push(e.to_string());
        }
        if let Err(e) = self.store.fill_some_ways_out(FACTS_PER_REFRESH) {
            stats.errors.push(e.to_string());
        }
        if let Err(e) = self.recheck_link_warnings() {
            stats.errors.push(e.to_string());
        }
        Ok(stats)
    }

    /// The server has said something changed in the inbox, so it's looked at straight away,
    /// whatever a longer refresh is busy with. The other folders wait for that refresh.
    async fn sync_inbox(&self, account_id: i64, quiet_now: bool) -> Result<SyncStats> {
        let account = self.store.account(account_id)?;
        let Some(inbox) = self
            .store
            .list_folders(account_id)?
            .into_iter()
            .find(|f| f.role == "inbox")
        else {
            return Ok(SyncStats::default());
        };
        let password = self.password(account_id).await?;
        let options = sync::SyncOptions {
            inbox_bodies: BODIES_WITH_NEW_MAIL,
            ..self.options.clone()
        };
        let mut conn = ImapConnection::connect(&account, &password, self.reach()).await?;
        let ring = self.pgp_ring(account_id);
        let mut news = sync::look_for_new_mail(
            &mut conn,
            &self.store,
            account_id,
            inbox.id,
            &inbox.path,
            "inbox",
            &options,
            &ring,
        )
        .await?;
        // The person hears first. What was read or deleted elsewhere can catch up after.
        if !news.new_mail.is_empty() {
            self.act_on_new_mail(&account, &mut news, quiet_now).await?;
        }
        let mut stats = sync::sync_inbox(
            conn,
            &self.store,
            &account,
            inbox.id,
            &inbox.path,
            &options,
            &ring,
        )
        .await?;
        self.act_on_new_mail(&account, &mut stats, quiet_now)
            .await?;
        stats.new_messages += news.new_messages;
        stats.bodies_fetched += news.bodies_fetched;
        stats.filtered += news.filtered;
        Ok(stats)
    }

    fn inbox_gate(&self, account_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.inbox_gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(account_id)
            .or_default()
            .clone()
    }

    /// Filters and rules get first say, then the person hears about whatever's left.
    async fn act_on_new_mail(
        &self,
        account: &Account,
        stats: &mut SyncStats,
        quiet_now: bool,
    ) -> Result<()> {
        {
            let gate = self.inbox_gate(account.id);
            let _turn = gate.lock().await;
            // A problem filtering never costs the refresh. Mail just stays where it is.
            // The person's own filters go first: new mail is kept out of sight until they've
            // looked at it, so they mustn't wait on the spam checks and sorting.
            match self.run_rules(account).await {
                Ok(changed) => stats.filtered = changed,
                Err(e) => stats.errors.push(e.to_string()),
            }
            match self.run_filters(account).await {
                Ok(changed) => stats.filtered += changed,
                Err(e) => stats.errors.push(e.to_string()),
            }
        }
        // The windows show it as the banner does, banner or not, rather than once the rest of
        // the inbox has caught up.
        if !stats.new_mail.is_empty() {
            if let Some(tell) = &self.new_mail_listener {
                tell(account.id);
            }
        }
        let alertable = self.keep_alertable(account.id, &stats.new_mail)?;
        self.send_alerts(account, &alertable, quiet_now)?;
        // Once the person has heard: quick replies for what needs them, so they're ready
        // when it's opened. Never costs the refresh.
        if let Err(e) = self.write_quick_replies_ahead(account).await {
            tracing::debug!("quick replies ahead: {e}");
        }
        Ok(())
    }

    /// How far back whole emails are kept on this device.
    pub(crate) fn keep_since(&self) -> Result<i64> {
        let days = match self.setting("keep_mail")?.as_str() {
            "year" => 365,
            "quarter" => 90,
            _ => return Ok(0),
        };
        Ok(crate::identity::unix_now() - days * 86_400)
    }

    /// Returns a message body from the Mac if we have it, or downloads it once.
    pub(crate) async fn fetch_body(&self, message_id: i64) -> Result<Body> {
        if let Some(body) = self.store.body(message_id)? {
            return Ok(body);
        }
        let raw = self.raw_message(message_id).await?;
        let account_id = self.store.message_location(message_id)?.account_id;
        let body = crate::pgp::read_body(&raw, &self.pgp_ring(account_id));
        self.store.save_body(message_id, &body)?;
        Ok(body)
    }

    /// The whole message from the server, without marking it as read.
    pub(crate) async fn raw_message(&self, message_id: i64) -> Result<Vec<u8>> {
        let loc = self.store.message_location(message_id)?;
        let account = self.store.account(loc.account_id)?;
        let password = self.password(loc.account_id).await?;

        let mut conn = ImapConnection::connect(&account, &password, self.reach())
            .await
            .map_err(|e| match e {
                CoreError::Network(_) => CoreError::Offline,
                other => other,
            })?;
        let mailbox = conn.open_folder(&loc.folder_path).await?;
        if mailbox.uid_validity != loc.uid_validity {
            conn.logout().await;
            return Err(CoreError::Invalid(
                "This folder changed on the server. Refresh and try again.".into(),
            ));
        }
        let fetched = conn.bodies(&[loc.uid]).await?;
        conn.logout().await;
        fetched
            .into_iter()
            .find(|(uid, _)| *uid == loc.uid)
            .map(|(_, raw)| raw)
            .ok_or_else(|| CoreError::NotFound("This message is no longer on the server.".into()))
    }
}

/// What another AI app runs to reach this one.
fn mcp_command() -> Result<String> {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| CoreError::Invalid(e.to_string()))
}

/// Which team a request is about, for those that say.
fn asked_team(args: &Value) -> Option<i64> {
    args.get("team_id").and_then(Value::as_i64)
}

fn from_json<T: DeserializeOwned>(args: Value) -> Result<T> {
    // The serde message can quote the input, which may hold a password.
    serde_json::from_value(args).map_err(|_| CoreError::Invalid("Bad request.".into()))
}

fn to_json<T: Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| CoreError::Invalid(e.to_string()))
}
