//! Everything that speaks IMAP. Nothing outside this file and sync.rs knows the
//! protocol, so another mail source can be added later without touching the UI.

use crate::error::{CoreError, Result};
use crate::model::{Account, Security};
use crate::net::{connect_server, Reach};
use async_imap::types::{Fetch, Flag, Mailbox, Name, NameAttribute};
use async_imap::{Client, Session};
use futures_util::TryStreamExt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Signing in can be slow when the server or the connection is busy, such as while another
/// inbox downloads, so it gets longer than reaching the server does.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(45);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
/// How many emails one flag change names at most.
const STORE_BATCH: usize = 1_000;

/// Fetches headers without marking anything as read.
const HEADER_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])";
/// Fetches a whole message without marking it as read.
const BODY_ITEMS: &str = "(UID BODY.PEEK[])";
const FLAG_ITEMS: &str = "(UID FLAGS)";
/// How far back to look for a message just sent.
const RECENT_CHECK: u32 = 20;

pub trait ImapIo: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug> ImapIo for T {}

pub struct RemoteFolder {
    pub path: String,
    pub name: String,
    pub role: String,
}

pub struct RemoteHeader {
    pub is_draft: bool,
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub size: u32,
    pub internal_date: Option<i64>,
    pub raw_header: Vec<u8>,
}

/// How waiting on a folder ended.
#[derive(Debug, PartialEq)]
pub enum MailWait {
    /// The server said something changed.
    Changed,
    /// Nothing happened in the time allowed.
    Quiet,
    /// Something else took over, or the inbox is going away.
    Stopped,
    /// This server can't tell anyone when mail arrives.
    NotSupported,
}

pub struct ImapConnection {
    session: Session<Box<dyn ImapIo>>,
}

/// Unencrypted mail is only allowed to this computer, which is what the tests
/// and the demo mailbox use.
pub fn check_security(host: &str, security: Security) -> Result<()> {
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if security == Security::Plain && !local {
        return Err(CoreError::Invalid(
            "Unencrypted connections are only allowed to this computer.".into(),
        ));
    }
    Ok(())
}

fn tls_connector() -> Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| CoreError::Network(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

async fn timed<T>(limit: Duration, what: &str, fut: impl Future<Output = T>) -> Result<T> {
    tokio::time::timeout(limit, fut)
        .await
        .map_err(|_| CoreError::Network(format!("{what} timed out")))
}

/// A refusal that says to come back later, rather than that the sign-in is wrong.
fn for_now(why: &str) -> bool {
    let why = why.to_ascii_lowercase();
    [
        "too many",
        "try again",
        "temporar",
        "unavailable",
        "[inuse]",
        "exceeded",
        "[limit]",
        "throttl",
        "busy",
    ]
    .iter()
    .any(|w| why.contains(w))
}

fn imap_err(e: async_imap::error::Error) -> CoreError {
    use async_imap::error::Error as E;
    match e {
        E::Io(e) => CoreError::Network(e.to_string()),
        E::ConnectionLost => CoreError::Network("the connection was lost".into()),
        other => CoreError::Imap(other.to_string()),
    }
}

/// Signing in with an access token instead of a password.
struct XOAuth2 {
    user: String,
    token: String,
    sent: bool,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> String {
        // A second round means it was refused, and the server wants an empty answer
        // before it says so.
        if self.sent {
            return String::new();
        }
        self.sent = true;
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}

/// Whether a mail server really is there: a secure connection with a valid certificate,
/// and a greeting. No sign-in, and nothing about the person is sent.
/// With `Reach::PublicOnly`, a server at a private address doesn't count as there.
pub async fn answers(host: &str, port: u16, limit: Duration, reach: Reach) -> bool {
    let attempt = async {
        let name = ServerName::try_from(host.to_string()).ok()?;
        let tcp = connect_server(host, port, reach).await.ok()?;
        let tls = tls_connector().ok()?.connect(name, tcp).await.ok()?;
        let stream: Box<dyn ImapIo> = Box::new(tls);
        let mut client = Client::new(stream);
        client.read_response().await.ok()?
    };
    matches!(tokio::time::timeout(limit, attempt).await, Ok(Some(_)))
}

/// A secure connection with a valid certificate, for servers that aren't IMAP.
pub async fn tls_answers(host: &str, port: u16, limit: Duration, reach: Reach) -> bool {
    let attempt = async {
        let name = ServerName::try_from(host.to_string()).ok()?;
        let tcp = connect_server(host, port, reach).await.ok()?;
        tls_connector().ok()?.connect(name, tcp).await.ok()
    };
    matches!(tokio::time::timeout(limit, attempt).await, Ok(Some(_)))
}

impl ImapConnection {
    /// With `Reach::PublicOnly`, the server's name is looked up once, refused if it leads
    /// anywhere private, and connected to at exactly that address. The certificate is
    /// still checked against the name.
    pub async fn connect(
        account: &Account,
        password: &str,
        reach: Reach,
    ) -> Result<ImapConnection> {
        let host = account.imap_host.as_str();
        check_security(host, account.security)?;

        let tcp = timed(
            CONNECT_TIMEOUT,
            "Connecting",
            connect_server(host, account.imap_port, reach),
        )
        .await??;

        let stream: Box<dyn ImapIo> = match account.security {
            Security::Plain => Box::new(tcp),
            Security::Tls => {
                let name = ServerName::try_from(host.to_string())
                    .map_err(|_| CoreError::Invalid("That server name is not valid.".into()))?;
                let tls = timed(
                    CONNECT_TIMEOUT,
                    "Securing the connection",
                    tls_connector()?.connect(name, tcp),
                )
                .await?
                .map_err(|e| CoreError::Network(e.to_string()))?;
                Box::new(tls)
            }
        };

        let mut client = Client::new(stream);
        timed(CONNECT_TIMEOUT, "Greeting", client.read_response())
            .await?
            .map_err(|e| CoreError::Network(e.to_string()))?
            .ok_or_else(|| CoreError::Network("the server closed the connection".into()))?;

        // With a browser sign-in, what's passed in here is an access token. A server that's
        // busy for now ("too many simultaneous connections" after a restart) is passing, not
        // a reason to ask anyone to sign in again.
        let refused = |e: async_imap::error::Error| match e {
            async_imap::error::Error::No(why) | async_imap::error::Error::Bad(why)
                if for_now(&why) =>
            {
                CoreError::Network(format!("the mail server is busy for now: {why}"))
            }
            async_imap::error::Error::No(why) | async_imap::error::Error::Bad(why) => {
                eprintln!(
                    "{}: the mail server turned the sign-in down: {why}",
                    account.email
                );
                if account.auth == crate::signin::PASSWORD {
                    CoreError::Auth
                } else {
                    CoreError::NeedsSignIn(account.email.clone())
                }
            }
            other => imap_err(other),
        };
        let session = if account.auth == crate::signin::PASSWORD {
            timed(
                SIGN_IN_TIMEOUT,
                "Signing in",
                client.login(&account.username, password),
            )
            .await?
            .map_err(|(e, _)| refused(e))?
        } else {
            let token = XOAuth2 {
                user: account.username.clone(),
                token: password.to_string(),
                sent: false,
            };
            timed(
                SIGN_IN_TIMEOUT,
                "Signing in",
                client.authenticate("XOAUTH2", token),
            )
            .await?
            .map_err(|(e, _)| refused(e))?
        };
        Ok(ImapConnection { session })
    }

    /// Sits on a folder until the server says something changed. Uses IMAP IDLE, which every
    /// server speaking the newer IMAP version has to support even if it doesn't say so.
    pub async fn wait_for_change(
        mut self,
        path: &str,
        known_next: Option<u32>,
        limit: Duration,
        stop: Arc<tokio::sync::Notify>,
    ) -> Result<MailWait> {
        let caps = timed(
            COMMAND_TIMEOUT,
            "Checking the server",
            self.session.capabilities(),
        )
        .await?
        .map_err(imap_err)?;
        if !caps.has_str("IDLE") && !caps.has_str("IMAP4rev2") {
            self.logout().await;
            return Ok(MailWait::NotSupported);
        }
        let mailbox = self.open_folder(path).await?;
        // Nobody announces what landed while no one was watching.
        if known_next.is_some() && mailbox.uid_next != known_next {
            self.logout().await;
            return Ok(MailWait::Changed);
        }
        let mut idle = self.session.idle();
        timed(COMMAND_TIMEOUT, "Watching for mail", idle.init())
            .await?
            .map_err(imap_err)?;
        let (waiting, interrupt) = idle.wait_with_timeout(limit);
        use async_imap::extensions::idle::IdleResponse;
        let outcome = tokio::select! {
            ended = waiting => match ended.map_err(imap_err)? {
                IdleResponse::NewData(_) => MailWait::Changed,
                IdleResponse::Timeout | IdleResponse::ManualInterrupt => MailWait::Quiet,
            },
            _ = stop.notified() => {
                drop(interrupt);
                return Ok(MailWait::Stopped);
            }
        };
        // Leaving politely is nice, but what the server said is already known.
        if let Ok(Ok(mut session)) = tokio::time::timeout(COMMAND_TIMEOUT, idle.done()).await {
            let _ = tokio::time::timeout(COMMAND_TIMEOUT, session.logout()).await;
        }
        Ok(outcome)
    }

    pub async fn logout(mut self) {
        let _ = timed(CONNECT_TIMEOUT, "Signing out", self.session.logout()).await;
    }

    pub async fn folders(&mut self) -> Result<Vec<RemoteFolder>> {
        let session = &mut self.session;
        let names: Vec<Name> = timed(COMMAND_TIMEOUT, "Listing folders", async move {
            session
                .list(Some(""), Some("*"))
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        // A real Archive folder wins over a view of everything, which some servers offer too.
        let has_archive = names.iter().any(|n| {
            n.attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::Archive))
        });
        Ok(one_folder_per_role(
            names
                .iter()
                .filter(|n| {
                    !has_archive
                        || !n
                            .attributes()
                            .iter()
                            .any(|a| matches!(a, NameAttribute::All))
                })
                .filter_map(classify_folder)
                .collect(),
        ))
    }

    /// Opens a folder read-only, so nothing here can change the user's mail.
    pub async fn open_folder(&mut self, path: &str) -> Result<Mailbox> {
        // These notices queue up unread, and a full queue would stall the
        // connection. Sync re-checks the folder anyway, so they can be dropped.
        while self.session.unsolicited_responses.try_recv().is_ok() {}
        timed(
            COMMAND_TIMEOUT,
            "Opening a folder",
            self.session.examine(path),
        )
        .await?
        .map_err(imap_err)
    }

    /// Opens a folder for changing. Only the mail actions use this, never sync.
    pub async fn select_folder(&mut self, path: &str) -> Result<Mailbox> {
        while self.session.unsolicited_responses.try_recv().is_ok() {}
        timed(
            COMMAND_TIMEOUT,
            "Opening a folder",
            self.session.select(path),
        )
        .await?
        .map_err(imap_err)
    }

    /// Changes flags on messages in the folder opened with `select_folder`.
    pub async fn store_flags(&mut self, uids: &[u32], query: &str) -> Result<()> {
        // In batches, written as ranges: thousands of numbers in one line is more than
        // some servers take.
        let mut sorted = uids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        for batch in sorted.chunks(STORE_BATCH) {
            let set = uid_ranges(batch);
            let session = &mut self.session;
            let query = query.to_string();
            timed(COMMAND_TIMEOUT, "Updating mail", async move {
                session
                    .uid_store(set, query)
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?
            .map_err(imap_err)?;
        }
        Ok(())
    }

    /// Moves messages out of the folder opened with `select_folder`.
    pub async fn move_uids(&mut self, uids: &[u32], target: &str) -> Result<()> {
        let set = uid_ranges(uids);
        let caps = timed(
            COMMAND_TIMEOUT,
            "Checking the server",
            self.session.capabilities(),
        )
        .await?
        .map_err(imap_err)?;
        if caps.has_str("MOVE") {
            timed(
                COMMAND_TIMEOUT,
                "Moving mail",
                self.session.uid_mv(&set, target),
            )
            .await?
            .map_err(imap_err)?;
            return Ok(());
        }
        timed(
            COMMAND_TIMEOUT,
            "Moving mail",
            self.session.uid_copy(&set, target),
        )
        .await?
        .map_err(imap_err)?;
        self.delete_uids(uids).await
    }

    /// Permanently removes messages from the folder opened with `select_folder`.
    /// Uses the narrow command where it exists, so other deleted mail is untouched.
    pub async fn delete_uids(&mut self, uids: &[u32]) -> Result<()> {
        self.store_flags(uids, "+FLAGS.SILENT (\\Deleted)").await?;
        let caps = timed(
            COMMAND_TIMEOUT,
            "Checking the server",
            self.session.capabilities(),
        )
        .await?
        .map_err(imap_err)?;
        let set = uid_ranges(uids);
        let narrow = caps.has_str("UIDPLUS");
        let session = &mut self.session;
        timed(COMMAND_TIMEOUT, "Removing mail", async move {
            if narrow {
                session
                    .uid_expunge(set)
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
                    .map(|_| ())
            } else {
                session
                    .expunge()
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
                    .map(|_| ())
            }
        })
        .await?
        .map_err(imap_err)?;
        Ok(())
    }

    /// Creates a folder if needed, and returns its path.
    pub async fn ensure_folder(&mut self, name: &str) -> Result<String> {
        let existing = self.folders().await?;
        if let Some(f) = existing.iter().find(|f| f.path.eq_ignore_ascii_case(name)) {
            return Ok(f.path.clone());
        }
        timed(
            COMMAND_TIMEOUT,
            "Creating a folder",
            self.session.create(name),
        )
        .await?
        .map_err(imap_err)?;
        Ok(name.to_string())
    }

    /// Makes a new folder of the person's, alongside their others. Some servers keep every
    /// folder under the inbox, and refuse or hide one made anywhere else.
    pub async fn create_folder(&mut self, name: &str) -> Result<()> {
        let existing = self.folders().await?;
        let path = ["INBOX.", "INBOX/"]
            .iter()
            .find(|prefix| existing.iter().any(|f| f.path.starts_with(**prefix)))
            .map_or(name.to_string(), |prefix| format!("{prefix}{name}"));
        timed(
            COMMAND_TIMEOUT,
            "Creating a folder",
            self.session.create(&path),
        )
        .await?
        .map_err(imap_err)?;
        // Some mail apps and servers only show folders that are subscribed to.
        let _ = timed(
            COMMAND_TIMEOUT,
            "Creating a folder",
            self.session.subscribe(&path),
        )
        .await;
        Ok(())
    }

    /// The folder has to be empty, and not the open one.
    pub async fn delete_folder(&mut self, path: &str) -> Result<()> {
        // Some servers won't delete the folder that's open.
        self.select_folder("INBOX").await?;
        let _ = timed(
            COMMAND_TIMEOUT,
            "Deleting a folder",
            self.session.unsubscribe(path),
        )
        .await;
        timed(
            COMMAND_TIMEOUT,
            "Deleting a folder",
            self.session.delete(path),
        )
        .await?
        .map_err(imap_err)?;
        Ok(())
    }

    /// Gmail's "All Mail", which is hidden from the folder list.
    pub async fn all_mail_folder(&mut self) -> Result<Option<String>> {
        let session = &mut self.session;
        let names: Vec<Name> = timed(COMMAND_TIMEOUT, "Listing folders", async move {
            session
                .list(Some(""), Some("*"))
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(names
            .iter()
            .find(|n| {
                n.attributes()
                    .iter()
                    .any(|a| matches!(a, NameAttribute::All))
            })
            .map(|n| n.name().to_string()))
    }

    /// Whether a message with this ID is among the newest in a folder.
    pub async fn folder_has_message(&mut self, path: &str, rfc_id: &str) -> Result<bool> {
        let exists = self.open_folder(path).await?.exists;
        if exists == 0 {
            return Ok(false);
        }
        let start = exists.saturating_sub(RECENT_CHECK - 1).max(1);
        let headers = self.headers_from_position(start).await?;
        Ok(headers
            .iter()
            .any(|h| crate::parse::parse_headers(&h.raw_header).rfc_id.as_deref() == Some(rfc_id)))
    }

    /// Adds a finished message to a folder, marked as read.
    pub async fn append(&mut self, path: &str, raw: &[u8]) -> Result<()> {
        timed(
            COMMAND_TIMEOUT,
            "Saving a copy",
            self.session.append(path, Some("(\\Seen)"), None, raw),
        )
        .await?
        .map_err(imap_err)
    }

    /// Adds a draft to the Drafts folder.
    pub async fn append_draft(&mut self, path: &str, raw: &[u8]) -> Result<()> {
        timed(
            COMMAND_TIMEOUT,
            "Saving a draft",
            self.session
                .append(path, Some("(\\Seen \\Draft)"), None, raw),
        )
        .await?
        .map_err(imap_err)
    }

    /// Headers by position, for the first sync of a folder.
    pub async fn headers_from_position(&mut self, start: u32) -> Result<Vec<RemoteHeader>> {
        let session = &mut self.session;
        let set = format!("{start}:*");
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Downloading headers", async move {
            session
                .fetch(set, HEADER_ITEMS)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(items.iter().filter_map(to_header).collect())
    }

    /// The numbers of the messages in the open folder that match a search.
    pub async fn search_uids(&mut self, query: &str) -> Result<Vec<u32>> {
        let session = &mut self.session;
        let query = query.to_string();
        let found = timed(COMMAND_TIMEOUT, "Searching", async move {
            session.uid_search(query).await
        })
        .await?
        .map_err(imap_err)?;
        Ok(found.into_iter().collect())
    }

    pub async fn headers_for_uids(&mut self, uids: &[u32]) -> Result<Vec<RemoteHeader>> {
        let session = &mut self.session;
        let set = uid_ranges(uids);
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Downloading headers", async move {
            session
                .uid_fetch(set, HEADER_ITEMS)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(items.iter().filter_map(to_header).collect())
    }

    /// Headers of messages newer than `last_uid`.
    pub async fn headers_after_uid(&mut self, last_uid: u32) -> Result<Vec<RemoteHeader>> {
        let session = &mut self.session;
        let set = format!("{}:*", last_uid + 1);
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Downloading headers", async move {
            session
                .uid_fetch(set, HEADER_ITEMS)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        // `n:*` always returns the newest message, even when it is older than n.
        Ok(items
            .iter()
            .filter_map(to_header)
            .filter(|h| h.uid > last_uid)
            .collect())
    }

    /// Current read and flagged state for a range of UIDs.
    pub async fn flags_in_range(&mut self, low: u32, high: u32) -> Result<Vec<(u32, bool, bool)>> {
        let session = &mut self.session;
        let set = format!("{low}:{high}");
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Checking for changes", async move {
            session
                .uid_fetch(set, FLAG_ITEMS)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(items
            .iter()
            .filter_map(|f| {
                let (seen, flagged) = flags_of(f);
                f.uid.map(|uid| (uid, seen, flagged))
            })
            .collect())
    }

    /// What's changed since the server's change counter last read this. Only servers
    /// that report a counter are asked this way.
    pub async fn flags_changed_since(
        &mut self,
        high: u32,
        since: u64,
    ) -> Result<Vec<(u32, bool, bool)>> {
        let session = &mut self.session;
        let set = format!("1:{high}");
        let query = format!("(UID FLAGS) (CHANGEDSINCE {since})");
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Checking for changes", async move {
            session
                .uid_fetch(set, query)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(items
            .iter()
            .filter_map(|f| {
                let (seen, flagged) = flags_of(f);
                f.uid.map(|uid| (uid, seen, flagged))
            })
            .collect())
    }

    /// Read and star marks for these messages.
    pub async fn flags_for_uids(&mut self, uids: &[u32]) -> Result<Vec<(u32, bool, bool)>> {
        let session = &mut self.session;
        let set = uid_ranges(uids);
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Checking for changes", async move {
            session
                .uid_fetch(set, FLAG_ITEMS)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(items
            .iter()
            .filter_map(|f| {
                let (seen, flagged) = flags_of(f);
                f.uid.map(|uid| (uid, seen, flagged))
            })
            .collect())
    }

    /// Whole messages by UID, as (uid, raw bytes).
    pub async fn bodies(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<u8>)>> {
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let session = &mut self.session;
        let items: Vec<Fetch> = timed(COMMAND_TIMEOUT, "Downloading mail", async move {
            session
                .uid_fetch(set, BODY_ITEMS)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .map_err(imap_err)?;
        Ok(items
            .iter()
            .filter_map(|f| Some((f.uid?, f.body()?.to_vec())))
            .collect())
    }
}

/// `1,2,3,7` as `1:3,7`, which keeps long lists short.
pub(crate) fn uid_ranges(uids: &[u32]) -> String {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut run: Option<(u32, u32)> = None;
    for u in sorted {
        run = match run {
            Some((a, b)) if u == b + 1 => Some((a, u)),
            Some((a, b)) => {
                parts.push(if a == b {
                    a.to_string()
                } else {
                    format!("{a}:{b}")
                });
                Some((u, u))
            }
            None => Some((u, u)),
        };
    }
    if let Some((a, b)) = run {
        parts.push(if a == b {
            a.to_string()
        } else {
            format!("{a}:{b}")
        });
    }
    parts.join(",")
}

fn flags_of(f: &Fetch) -> (bool, bool) {
    let (mut seen, mut flagged) = (false, false);
    for flag in f.flags() {
        match flag {
            Flag::Seen => seen = true,
            Flag::Flagged => flagged = true,
            _ => {}
        }
    }
    (seen, flagged)
}

fn to_header(f: &Fetch) -> Option<RemoteHeader> {
    let (seen, flagged) = flags_of(f);
    Some(RemoteHeader {
        is_draft: f.flags().any(|x| matches!(x, Flag::Draft)),
        uid: f.uid?,
        seen,
        flagged,
        size: f.size.unwrap_or(0),
        internal_date: f.internal_date().map(|d| d.timestamp()),
        raw_header: f.header()?.to_vec(),
    })
}

/// Folder names other than plain English letters come from the server coded (RFC 3501's
/// "modified UTF-7"): Gmail's "✔" label arrives as "&JxQ-". This turns them back into text.
/// Anything that isn't valid code is left as it came.
pub(crate) fn folder_name_from_server(raw: &str) -> String {
    fn sixes(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b',' => 63,
            _ => return None,
        } as u32)
    }
    let decode = |coded: &str| -> Option<String> {
        let (mut bits, mut count, mut units) = (0u32, 0u32, Vec::new());
        for c in coded.bytes() {
            bits = (bits << 6) | sixes(c)?;
            count += 6;
            if count >= 16 {
                count -= 16;
                units.push(((bits >> count) & 0xFFFF) as u16);
            }
        }
        String::from_utf16(&units).ok()
    };
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let coded = &after[..end];
        if coded.is_empty() {
            out.push('&');
        } else {
            match decode(coded) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[start..start + end + 2]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Keeps one folder for each job (Sent, Drafts, Trash…). One the server marks for the job
/// wins over one whose name only suggests it, such as a Gmail label called "Sent" beside
/// Gmail's own Sent Mail; otherwise the first wins. The rest are ordinary folders.
fn one_folder_per_role(folders: Vec<(RemoteFolder, bool)>) -> Vec<RemoteFolder> {
    use std::collections::HashSet;
    let marked: HashSet<String> = folders
        .iter()
        .filter(|(_, by_server)| *by_server)
        .map(|(f, _)| f.role.clone())
        .collect();
    let mut taken = HashSet::new();
    folders
        .into_iter()
        .map(|(mut f, by_server)| {
            let special = f.role != "folder" && f.role != "inbox";
            if special
                && ((!by_server && marked.contains(&f.role)) || !taken.insert(f.role.clone()))
            {
                f.role = "folder".to_string();
            }
            f
        })
        .collect()
}

fn classify_folder(n: &Name) -> Option<(RemoteFolder, bool)> {
    let path = n.name().to_string();
    let mut role = None;
    for attr in n.attributes() {
        match attr {
            NameAttribute::NoSelect => return None,
            // Gmail's view of starred mail, which lives in other folders anyway.
            NameAttribute::Flagged => return None,
            // Gmail's All Mail. What's been archived is only here, so it's kept, and it
            // does the Archive folder's job.
            NameAttribute::All => role = Some("archive"),
            NameAttribute::Sent => role = Some("sent"),
            NameAttribute::Drafts => role = Some("drafts"),
            NameAttribute::Trash => role = Some("trash"),
            NameAttribute::Junk => role = Some("junk"),
            NameAttribute::Archive => role = Some("archive"),
            _ => {}
        }
    }
    let name = match n.delimiter() {
        Some(d) if !d.is_empty() => path.rsplit(d).next().unwrap_or(&path).to_string(),
        _ => path.clone(),
    };
    // Only the name shown is decoded. The server still needs the path as it sent it.
    let name = folder_name_from_server(&name);
    let by_server = role.is_some();
    let role = role.unwrap_or_else(|| role_from_name(&path)).to_string();
    let name = if role == "inbox" {
        "Inbox".to_string()
    } else {
        name
    };
    Some((RemoteFolder { path, name, role }, by_server))
}

/// For older servers that do not label their special folders.
pub fn role_from_name(path: &str) -> &'static str {
    match path.to_ascii_lowercase().as_str() {
        "inbox" => "inbox",
        "sent" | "sent items" | "sent mail" | "sent messages" => "sent",
        "drafts" | "draft" => "drafts",
        "trash" | "deleted" | "deleted items" | "deleted messages" | "bin" => "trash",
        "junk" | "spam" | "junk email" | "bulk mail" => "junk",
        "archive" | "archives" => "archive",
        _ => "folder",
    }
}

#[cfg(test)]
mod one_per_role_tests {
    use super::{one_folder_per_role, RemoteFolder};

    fn f(path: &str, role: &str) -> RemoteFolder {
        RemoteFolder {
            path: path.into(),
            name: path.into(),
            role: role.into(),
        }
    }

    #[test]
    fn a_label_named_like_sent_doesnt_make_a_second_sent() {
        let roles: Vec<(String, String)> = one_folder_per_role(vec![
            (f("INBOX", "inbox"), false),
            (f("Sent", "sent"), false),
            (f("[Gmail]/Sent Mail", "sent"), true),
            (f("Trash", "trash"), false),
            (f("Deleted Items", "trash"), false),
        ])
        .into_iter()
        .map(|x| (x.path, x.role))
        .collect();
        let role_of = |p: &str| roles.iter().find(|(path, _)| path == p).unwrap().1.clone();
        assert_eq!(role_of("[Gmail]/Sent Mail"), "sent");
        assert_eq!(role_of("Sent"), "folder");
        // Neither marked by the server: the first keeps the job.
        assert_eq!(role_of("Trash"), "trash");
        assert_eq!(role_of("Deleted Items"), "folder");
        assert_eq!(role_of("INBOX"), "inbox");
    }
}

#[cfg(test)]
mod folder_name_tests {
    use super::folder_name_from_server as named;

    #[test]
    fn coded_folder_names_read_as_text() {
        assert_eq!(named("&JxQ-"), "✔");
        assert_eq!(named("&JxQnFA-"), "✔✔");
        assert_eq!(named("R&AOk-sum&AOk-"), "Résumé");
        assert_eq!(named("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(named("Clients"), "Clients");
        // Not valid code: left alone rather than lost.
        assert_eq!(named("A&B"), "A&B");
        assert_eq!(named("&!!-"), "&!!-");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_connections_only_reach_this_computer() {
        assert!(check_security("127.0.0.1", Security::Plain).is_ok());
        assert!(check_security("localhost", Security::Plain).is_ok());
        assert!(check_security("mail.example.com", Security::Plain).is_err());
        assert!(check_security("mail.example.com", Security::Tls).is_ok());
    }

    #[test]
    fn long_lists_of_numbers_shrink_to_ranges() {
        assert_eq!(uid_ranges(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
        assert_eq!(uid_ranges(&[5]), "5");
        assert_eq!(uid_ranges(&[]), "");
    }

    #[test]
    fn recognises_common_folder_names() {
        assert_eq!(role_from_name("INBOX"), "inbox");
        assert_eq!(role_from_name("Sent Items"), "sent");
        assert_eq!(role_from_name("Deleted Items"), "trash");
        assert_eq!(role_from_name("Clients"), "folder");
    }
}
