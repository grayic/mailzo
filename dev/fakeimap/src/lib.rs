//! A tiny IMAP server for tests and the demo mailbox.
//!
//! It speaks just enough IMAP4rev1 for the mail engine: LOGIN, LIST, SELECT,
//! EXAMINE, FETCH, UID FETCH, UID STORE, UID COPY, UID MOVE, EXPUNGE, CREATE and
//! APPEND. Changes are refused in a folder opened read-only, as on real servers,
//! so a test fails if sync ever tries to change mail. It copies the behaviours of real servers that
//! cause bugs, such as `UID FETCH n:*` returning the newest message when `n` is
//! past the end of the folder.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub mod sample;

pub const DEMO_USER: &str = "demo@inbox.test";
pub const DEMO_PASS: &str = "demo-password";

#[derive(Clone, Debug)]
pub struct FakeMessage {
    pub uid: u32,
    pub flags: BTreeSet<String>,
    pub internal_date: String,
    pub raw: Vec<u8>,
}

impl FakeMessage {
    fn header_bytes(&self) -> &[u8] {
        match self.raw.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(i) => &self.raw[..i + 4],
            None => &self.raw,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FakeFolder {
    pub name: String,
    pub attrs: Vec<String>,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub messages: Vec<FakeMessage>,
    /// Goes up with every change, for servers that report it.
    pub modseq: u64,
    /// When each message last changed, by UID.
    pub changed: std::collections::HashMap<u32, u64>,
}

#[derive(Default)]
pub struct ServerState {
    pub user: String,
    pub pass: String,
    pub folders: Vec<FakeFolder>,
    /// Counts fetches that would have marked mail as read on a real server.
    pub non_peek_fetches: usize,
    /// Counts full message downloads, so tests can prove the cache is used.
    pub body_fetches: usize,
    pub logins: usize,
    /// Old servers have no MOVE or narrow expunge, so the fallback gets tested too.
    pub no_move: bool,
    /// How many plain EXPUNGE commands ran, which remove all deleted mail.
    pub broad_expunges: usize,
    /// Access tokens that work, and whose they are.
    pub oauth_tokens: Vec<(String, String)>,
    /// Reports a change counter, and can answer "what changed since".
    pub change_counter: bool,
    /// How many flag rows have been sent, so tests can see the counter is used.
    pub flag_rows: usize,
    /// Answers every search with nothing, as a server having a bad moment can.
    pub empty_search: bool,
    /// An older server that can't say when mail arrives.
    pub no_idle: bool,
    /// Turns every sign-in down with this, as a busy server does for a while.
    pub login_refusal: Option<String>,
    /// A slow server: read and star changes are made and answered only after this long.
    pub store_wait: Option<std::time::Duration>,
}

/// Handle used by tests and the demo to change the mailbox while it is served.
#[derive(Clone)]
pub struct FakeServer {
    pub addr: SocketAddr,
    pub state: Arc<Mutex<ServerState>>,
    accept_task: Arc<tokio::task::AbortHandle>,
}

impl FakeServer {
    pub async fn start(user: &str, pass: &str) -> FakeServer {
        Self::start_on("127.0.0.1:0", user, pass).await
    }

    pub async fn start_on(bind: &str, user: &str, pass: &str) -> FakeServer {
        let listener = TcpListener::bind(bind).await.expect("bind fake imap");
        let addr = listener.local_addr().expect("local addr");
        let state = Arc::new(Mutex::new(ServerState {
            user: user.into(),
            pass: pass.into(),
            ..Default::default()
        }));
        let shared = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let state = shared.clone();
                tokio::spawn(async move {
                    let _ = serve(sock, state).await;
                });
            }
        });
        FakeServer {
            addr,
            state,
            accept_task: Arc::new(task.abort_handle()),
        }
    }

    /// Stops accepting connections, as if the network had gone away.
    pub async fn go_offline(&self) {
        self.accept_task.abort();
        tokio::task::yield_now().await;
    }

    pub fn add_folder(&self, name: &str, attrs: &[&str], uid_validity: u32) {
        self.state.lock().unwrap().folders.push(FakeFolder {
            name: name.into(),
            attrs: attrs.iter().map(|s| s.to_string()).collect(),
            uid_validity,
            uid_next: 1,
            modseq: 1,
            changed: Default::default(),
            messages: vec![],
        });
    }

    /// Adds a message and returns its UID. Line endings are normalised to CRLF.
    pub fn add_message(&self, folder: &str, raw: &str, seen: bool) -> u32 {
        self.add_message_dated(folder, raw, seen, "01-Nov-2026 10:00:00 +0000")
    }

    pub fn add_message_dated(&self, folder: &str, raw: &str, seen: bool, date: &str) -> u32 {
        let mut guard = self.state.lock().unwrap();
        let f = find_mut(&mut guard.folders, folder);
        let uid = f.uid_next;
        f.uid_next += 1;
        let mut flags = BTreeSet::new();
        if seen {
            flags.insert("\\Seen".to_string());
        }
        f.messages.push(FakeMessage {
            uid,
            flags,
            internal_date: date.to_string(),
            raw: raw.replace("\r\n", "\n").replace('\n', "\r\n").into_bytes(),
        });
        f.modseq += 1;
        f.changed.insert(uid, f.modseq);
        uid
    }

    pub fn delete_message(&self, folder: &str, uid: u32) {
        let mut guard = self.state.lock().unwrap();
        let f = find_mut(&mut guard.folders, folder);
        f.messages.retain(|m| m.uid != uid);
        f.modseq += 1;
    }

    pub fn set_flag(&self, folder: &str, uid: u32, flag: &str, on: bool) {
        let mut guard = self.state.lock().unwrap();
        let f = find_mut(&mut guard.folders, folder);
        if let Some(m) = f.messages.iter_mut().find(|m| m.uid == uid) {
            if on {
                m.flags.insert(flag.to_string());
            } else {
                m.flags.remove(flag);
            }
            f.modseq += 1;
            f.changed.insert(uid, f.modseq);
        }
    }

    /// Simulates the server renumbering a folder: every UID changes.
    pub fn reset_uid_validity(&self, folder: &str, new_validity: u32) {
        let mut guard = self.state.lock().unwrap();
        let f = find_mut(&mut guard.folders, folder);
        f.uid_validity = new_validity;
        let mut next = 100;
        for m in &mut f.messages {
            m.uid = next;
            next += 1;
        }
        f.uid_next = next;
    }

    pub fn non_peek_fetches(&self) -> usize {
        self.state.lock().unwrap().non_peek_fetches
    }

    pub fn body_fetches(&self) -> usize {
        self.state.lock().unwrap().body_fetches
    }

    pub fn folder_messages(&self, folder: &str) -> Vec<FakeMessage> {
        let mut guard = self.state.lock().unwrap();
        find_mut(&mut guard.folders, folder).messages.clone()
    }

    pub fn has_flag(&self, folder: &str, uid: u32, flag: &str) -> bool {
        let mut guard = self.state.lock().unwrap();
        find_mut(&mut guard.folders, folder)
            .messages
            .iter()
            .any(|m| m.uid == uid && m.flags.contains(flag))
    }

    pub fn is_seen(&self, folder: &str, uid: u32) -> bool {
        let mut guard = self.state.lock().unwrap();
        find_mut(&mut guard.folders, folder)
            .messages
            .iter()
            .any(|m| m.uid == uid && m.flags.contains("\\Seen"))
    }
}

fn find_mut<'a>(folders: &'a mut [FakeFolder], name: &str) -> &'a mut FakeFolder {
    folders
        .iter_mut()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no fake folder {name}"))
}

async fn serve(sock: TcpStream, state: Arc<Mutex<ServerState>>) -> std::io::Result<()> {
    let (read, mut write) = sock.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"* OK fakeimap ready\r\n").await?;

    let mut logged_in = false;
    let mut selected: Option<(String, bool)> = None;

    while let Some(line) = lines.next_line().await? {
        let Some((tag, rest)) = line.split_once(' ') else {
            continue;
        };
        let upper = rest.to_ascii_uppercase();
        if upper.starts_with("UID STORE ") {
            let wait = state.lock().unwrap().store_wait;
            if let Some(wait) = wait {
                tokio::time::sleep(wait).await;
            }
        }
        let refusal = state.lock().unwrap().login_refusal.clone();
        let out: Vec<u8> = if upper.starts_with("CAPABILITY") {
            let (no_move, no_idle) = {
                let s = state.lock().unwrap();
                (s.no_move, s.no_idle)
            };
            let extra = if no_move { "" } else { " UIDPLUS MOVE" };
            let idle = if no_idle { "" } else { " IDLE" };
            format!("* CAPABILITY IMAP4rev1 SPECIAL-USE{extra}{idle}\r\n{tag} OK done\r\n")
                .into_bytes()
        } else if upper.starts_with("IDLE") {
            write.write_all(b"+ idling\r\n").await?;
            let count = |s: &ServerState, folder: &Option<(String, bool)>| {
                folder.as_ref().and_then(|(name, _)| {
                    s.folders
                        .iter()
                        .find(|f| &f.name == name)
                        .map(|f| f.messages.len())
                })
            };
            let before = count(&state.lock().unwrap(), &selected);
            let mut told = false;
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        // DONE, or the other side going away.
                        line?;
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                        let now = count(&state.lock().unwrap(), &selected);
                        if !told && now != before {
                            told = true;
                            let n = now.unwrap_or(0);
                            write.write_all(format!("* {n} EXISTS\r\n").as_bytes()).await?;
                        }
                    }
                }
            }
            format!("{tag} OK idle done\r\n").into_bytes()
        } else if upper.starts_with("NOOP") {
            format!("{tag} OK done\r\n").into_bytes()
        } else if upper.starts_with("LOGOUT") {
            write
                .write_all(format!("* BYE bye\r\n{tag} OK done\r\n").as_bytes())
                .await?;
            return Ok(());
        } else if upper.starts_with("LOGIN ") {
            let args = parse_strings(&rest[6..]);
            let mut s = state.lock().unwrap();
            if args.len() == 2 && args[0] == s.user && args[1] == s.pass {
                logged_in = true;
                s.logins += 1;
                format!("{tag} OK logged in\r\n").into_bytes()
            } else {
                format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n").into_bytes()
            }
        } else if let (true, Some(why)) = (
            upper == "AUTHENTICATE XOAUTH2" || upper.starts_with("LOGIN "),
            refusal,
        ) {
            if upper == "AUTHENTICATE XOAUTH2" {
                write.write_all(b"+ \r\n").await?;
                lines.next_line().await?;
            }
            format!("{tag} NO {why}\r\n").into_bytes()
        } else if upper == "AUTHENTICATE XOAUTH2" {
            write.write_all(b"+ \r\n").await?;
            let encoded = lines.next_line().await?.unwrap_or_default();
            let decoded = data_encoding::BASE64
                .decode(encoded.trim().as_bytes())
                .unwrap_or_default();
            let text = String::from_utf8_lossy(&decoded).to_string();
            let mut parts = text.split('\x01');
            let user = parts
                .next()
                .unwrap_or("")
                .strip_prefix("user=")
                .unwrap_or("")
                .to_string();
            let token = parts
                .next()
                .unwrap_or("")
                .strip_prefix("auth=Bearer ")
                .unwrap_or("")
                .to_string();
            let good = state
                .lock()
                .unwrap()
                .oauth_tokens
                .iter()
                .any(|(t, u)| *t == token && *u == user);
            if good {
                logged_in = true;
                state.lock().unwrap().logins += 1;
                format!("{tag} OK authenticated\r\n").into_bytes()
            } else {
                // As Gmail does: a challenge saying why, then a refusal.
                write.write_all(b"+ eyJzdGF0dXMiOiI0MDEifQ==\r\n").await?;
                lines.next_line().await?;
                format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n").into_bytes()
            }
        } else if !logged_in {
            format!("{tag} BAD log in first\r\n").into_bytes()
        } else if upper.starts_with("LIST ") {
            let s = state.lock().unwrap();
            let mut out = String::new();
            for f in &s.folders {
                out.push_str(&format!(
                    "* LIST ({}) \"/\" \"{}\"\r\n",
                    f.attrs.join(" "),
                    f.name
                ));
            }
            out.push_str(&format!("{tag} OK done\r\n"));
            out.into_bytes()
        } else if upper.starts_with("SELECT ") || upper.starts_with("EXAMINE ") {
            let read_only = upper.starts_with("EXAMINE ");
            let arg = rest.split_once(' ').map(|x| x.1).unwrap_or("");
            let name = parse_strings(arg).into_iter().next().unwrap_or_default();
            let s = state.lock().unwrap();
            match s
                .folders
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&name))
            {
                Some(f) => {
                    selected = Some((f.name.clone(), read_only));
                    let mode = if read_only { "READ-ONLY" } else { "READ-WRITE" };
                    let counter = if s.change_counter {
                        format!("* OK [HIGHESTMODSEQ {}] ok\r\n", f.modseq)
                    } else {
                        String::new()
                    };
                    format!(
                        "* FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)\r\n\
                         * {} EXISTS\r\n* 0 RECENT\r\n\
                         * OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {}] ok\r\n{counter}\
                         {tag} OK [{mode}] done\r\n",
                        f.messages.len(),
                        f.uid_validity,
                        f.uid_next
                    )
                    .into_bytes()
                }
                None => format!("{tag} NO no such folder\r\n").into_bytes(),
            }
        } else if upper.starts_with("FETCH ") || upper.starts_with("UID FETCH ") {
            let by_uid = upper.starts_with("UID ");
            let args = if by_uid { &rest[10..] } else { &rest[6..] };
            match &selected {
                Some((name, read_only)) => {
                    let mut s = state.lock().unwrap();
                    fetch(&mut s, name, *read_only, by_uid, args, tag)
                }
                None => format!("{tag} BAD nothing selected\r\n").into_bytes(),
            }
        } else if upper.starts_with("UID SEARCH ") {
            match &selected {
                Some((name, _)) => {
                    let s = state.lock().unwrap();
                    let folder = s.folders.iter().find(|f| &f.name == name);
                    let terms = search_terms(&rest[11..]);
                    let mut position = 0;
                    if terms
                        .first()
                        .is_some_and(|t| t.eq_ignore_ascii_case("CHARSET"))
                    {
                        position = 2;
                    }
                    let mut wanted: Vec<SearchTerm> = Vec::new();
                    while position < terms.len() {
                        wanted.push(read_term(&terms, &mut position));
                    }
                    let uids: Vec<String> = folder
                        .map(|f| f.messages.as_slice())
                        .unwrap_or_default()
                        .iter()
                        .filter(|m| !s.empty_search && wanted.iter().all(|t| t.matches(m)))
                        .map(|m| m.uid.to_string())
                        .collect();
                    format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" ")).into_bytes()
                }
                None => format!("{tag} BAD nothing selected\r\n").into_bytes(),
            }
        } else if upper.starts_with("APPEND ") {
            // The message arrives as {size}, sent once the server invites it.
            let size = rest
                .rsplit_once('{')
                .and_then(|(_, n)| n.trim_end_matches('}').parse::<usize>().ok());
            let Some(size) = size else {
                write
                    .write_all(format!("{tag} BAD need a size\r\n").as_bytes())
                    .await?;
                continue;
            };
            let folder = parse_strings(&rest[7..])
                .into_iter()
                .next()
                .unwrap_or_default();
            write.write_all(b"+ go ahead\r\n").await?;
            let mut data = vec![0u8; size];
            lines.get_mut().read_exact(&mut data).await?;
            // The line ending after the message.
            lines.next_line().await?;
            let mut s = state.lock().unwrap();
            match s.folders.iter_mut().find(|f| f.name == folder) {
                Some(f) => {
                    let uid = f.uid_next;
                    f.uid_next += 1;
                    let mut flags = BTreeSet::new();
                    if upper.contains("\\SEEN") {
                        flags.insert("\\Seen".to_string());
                    }
                    if upper.contains("\\DRAFT") {
                        flags.insert("\\Draft".to_string());
                    }
                    f.messages.push(FakeMessage {
                        uid,
                        flags,
                        internal_date: "01-Nov-2036 10:00:00 +0000".into(),
                        raw: data,
                    });
                    format!("{tag} OK appended\r\n").into_bytes()
                }
                None => format!("{tag} NO no such folder\r\n").into_bytes(),
            }
        } else if upper.starts_with("DELETE ") {
            let name = parse_strings(&rest[7..])
                .into_iter()
                .next()
                .unwrap_or_default();
            let mut s = state.lock().unwrap();
            let before = s.folders.len();
            s.folders.retain(|f| f.name != name);
            if s.folders.len() < before {
                format!("{tag} OK deleted\r\n").into_bytes()
            } else {
                format!("{tag} NO no such folder\r\n").into_bytes()
            }
        } else if upper.starts_with("CREATE ") {
            let name = parse_strings(&rest[7..])
                .into_iter()
                .next()
                .unwrap_or_default();
            let mut s = state.lock().unwrap();
            s.folders.push(FakeFolder {
                name,
                attrs: vec![],
                uid_validity: 1,
                uid_next: 1,
                modseq: 1,
                changed: Default::default(),
                messages: vec![],
            });
            format!("{tag} OK created\r\n").into_bytes()
        } else if upper.starts_with("UID STORE ")
            || upper.starts_with("UID COPY ")
            || upper.starts_with("UID MOVE ")
            || upper.starts_with("UID EXPUNGE ")
            || upper == "EXPUNGE"
        {
            match &selected {
                None => format!("{tag} BAD nothing selected\r\n").into_bytes(),
                Some((_, true)) => {
                    format!("{tag} NO [READ-ONLY] folder is read-only\r\n").into_bytes()
                }
                Some((name, false)) => {
                    let mut s = state.lock().unwrap();
                    change(&mut s, name, rest, &upper, tag)
                }
            }
        } else {
            format!("{tag} BAD unsupported\r\n").into_bytes()
        };
        write.write_all(&out).await?;
    }
    Ok(())
}

enum SearchTerm {
    All,
    Text(String),
    Header(String, String),
    Since(i64),
    Before(i64),
    Unseen,
    Flagged,
    UidFrom(u32),
    Or(Box<SearchTerm>, Box<SearchTerm>),
    Unknown,
}

impl SearchTerm {
    fn matches(&self, m: &FakeMessage) -> bool {
        let raw = String::from_utf8_lossy(&m.raw).to_lowercase();
        match self {
            SearchTerm::All => true,
            SearchTerm::Text(t) => raw.contains(&t.to_lowercase()),
            SearchTerm::Header(name, t) => raw
                .lines()
                .any(|l| l.starts_with(&format!("{name}:")) && l.contains(&t.to_lowercase())),
            SearchTerm::Since(day) => day_number(&m.internal_date) >= *day,
            SearchTerm::Before(day) => day_number(&m.internal_date) < *day,
            SearchTerm::Unseen => !m.flags.contains("\\Seen"),
            SearchTerm::Flagged => m.flags.contains("\\Flagged"),
            SearchTerm::UidFrom(from) => m.uid >= *from,
            SearchTerm::Or(a, b) => a.matches(m) || b.matches(m),
            SearchTerm::Unknown => false,
        }
    }
}

/// A date such as `1-Nov-2026` as a number that sorts correctly.
fn day_number(date: &str) -> i64 {
    let mut parts = date.split_whitespace().next().unwrap_or("").split('-');
    let day: i64 = parts.next().and_then(|d| d.parse().ok()).unwrap_or(0);
    let month = parts.next().unwrap_or("");
    let year: i64 = parts.next().and_then(|y| y.parse().ok()).unwrap_or(0);
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let index = months.iter().position(|m| *m == month).unwrap_or(0) as i64;
    year * 500 + index * 40 + day
}

/// Splits a search into words and quoted strings.
fn search_terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => word.extend(chars.next()),
                    '"' => break,
                    c => word.push(c),
                }
            }
            out.push(word);
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
            out.push(word);
        }
    }
    out
}

fn read_term(terms: &[String], position: &mut usize) -> SearchTerm {
    let word = terms.get(*position).cloned().unwrap_or_default();
    *position += 1;
    let mut value = || {
        let v = terms.get(*position).cloned().unwrap_or_default();
        *position += 1;
        v
    };
    match word.to_ascii_uppercase().as_str() {
        "ALL" => SearchTerm::All,
        "HEADER" => {
            let name = value().to_lowercase();
            SearchTerm::Header(name, value())
        }
        "TEXT" => SearchTerm::Text(value()),
        "FROM" => SearchTerm::Header("from".into(), value()),
        "TO" => SearchTerm::Header("to".into(), value()),
        "SINCE" => SearchTerm::Since(day_number(&value())),
        "BEFORE" => SearchTerm::Before(day_number(&value())),
        "UNSEEN" => SearchTerm::Unseen,
        "FLAGGED" => SearchTerm::Flagged,
        // Only ever asked for as `n:*`.
        "UID" => SearchTerm::UidFrom(
            value()
                .split(':')
                .next()
                .and_then(|from| from.parse().ok())
                .unwrap_or(u32::MAX),
        ),
        "OR" => {
            let a = read_term(terms, position);
            let b = read_term(terms, position);
            SearchTerm::Or(Box::new(a), Box::new(b))
        }
        _ => SearchTerm::Unknown,
    }
}

/// The commands that change mail in the selected folder.
fn change(s: &mut ServerState, folder: &str, rest: &str, upper: &str, tag: &str) -> Vec<u8> {
    if upper == "EXPUNGE" {
        s.broad_expunges += 1;
        find_mut(&mut s.folders, folder)
            .messages
            .retain(|m| !m.flags.contains("\\Deleted"));
        return format!("{tag} OK done\r\n").into_bytes();
    }
    let no_move = s.no_move;
    let mut words = rest.splitn(4, ' ');
    let _uid = words.next();
    let verb = words.next().unwrap_or("").to_ascii_uppercase();
    let set = words.next().unwrap_or("").to_string();
    let args = words.next().unwrap_or("").to_string();
    let f = find_mut(&mut s.folders, folder);
    let max = f.messages.iter().map(|m| m.uid).max().unwrap_or(0);
    let wanted = parse_set(&set, max);
    match verb.as_str() {
        "STORE" => {
            let adding = args.starts_with('+');
            let inside = args
                .split_once('(')
                .and_then(|(_, r)| r.split_once(')'))
                .map(|(a, _)| a.to_string())
                .unwrap_or_default();
            let silent = args.to_ascii_uppercase().contains(".SILENT");
            let mut out = String::new();
            let mut bumped: Vec<u32> = Vec::new();
            for (i, m) in f.messages.iter_mut().enumerate() {
                if !wanted(m.uid) {
                    continue;
                }
                for flag in inside.split_whitespace() {
                    if adding {
                        m.flags.insert(flag.to_string());
                    } else {
                        m.flags.remove(flag);
                    }
                }
                bumped.push(m.uid);
                if !silent {
                    let flags: Vec<&str> = m.flags.iter().map(String::as_str).collect();
                    out.push_str(&format!(
                        "* {} FETCH (UID {} FLAGS ({}))\r\n",
                        i + 1,
                        m.uid,
                        flags.join(" ")
                    ));
                }
            }
            for uid in bumped {
                f.modseq += 1;
                let now = f.modseq;
                f.changed.insert(uid, now);
            }
            out.push_str(&format!("{tag} OK done\r\n"));
            out.into_bytes()
        }
        "EXPUNGE" => {
            f.messages
                .retain(|m| !(wanted(m.uid) && m.flags.contains("\\Deleted")));
            f.modseq += 1;
            format!("{tag} OK done\r\n").into_bytes()
        }
        "COPY" | "MOVE" => {
            if verb == "MOVE" && no_move {
                return format!("{tag} BAD unsupported\r\n").into_bytes();
            }
            let target = parse_strings(&args).into_iter().next().unwrap_or_default();
            let f = find_mut(&mut s.folders, folder);
            let picked: Vec<FakeMessage> = f
                .messages
                .iter()
                .filter(|m| wanted(m.uid))
                .cloned()
                .collect();
            if verb == "MOVE" {
                f.messages.retain(|m| !wanted(m.uid));
            }
            let Some(t) = s.folders.iter_mut().find(|f| f.name == target) else {
                return format!("{tag} NO no such folder\r\n").into_bytes();
            };
            for mut m in picked {
                m.uid = t.uid_next;
                t.uid_next += 1;
                t.messages.push(m);
            }
            format!("{tag} OK done\r\n").into_bytes()
        }
        _ => format!("{tag} BAD unsupported\r\n").into_bytes(),
    }
}

fn fetch(
    s: &mut ServerState,
    folder: &str,
    read_only: bool,
    by_uid: bool,
    args: &str,
    tag: &str,
) -> Vec<u8> {
    let (set, items) = args.split_once(' ').unwrap_or((args, ""));
    let items = items.to_ascii_uppercase();
    let wants_full = items.contains("BODY[]") || items.contains("BODY.PEEK[]");
    let wants_header = items.contains("[HEADER]");
    let is_peek = items.contains(".PEEK[");
    let body_requested = wants_full || wants_header;
    let since: Option<u64> = items
        .split_once("CHANGEDSINCE ")
        .and_then(|(_, r)| r.split(')').next())
        .and_then(|n| n.trim().parse().ok());
    let mut flag_rows = 0usize;

    let mut out = Vec::new();
    let mut touched = 0usize;
    let mut full = 0usize;
    let f = find_mut(&mut s.folders, folder);
    let max = if by_uid {
        f.messages.iter().map(|m| m.uid).max().unwrap_or(0)
    } else {
        f.messages.len() as u32
    };
    let wanted = parse_set(set, max);
    let changed = f.changed.clone();
    for (i, m) in f.messages.iter_mut().enumerate() {
        let seq = i as u32 + 1;
        if !wanted(if by_uid { m.uid } else { seq }) {
            continue;
        }
        if since.is_some_and(|n| changed.get(&m.uid).copied().unwrap_or(0) <= n) {
            continue;
        }
        if !body_requested {
            flag_rows += 1;
        }
        if body_requested && !is_peek {
            touched += 1;
            if !read_only {
                m.flags.insert("\\Seen".into());
            }
        }
        let mut parts = vec![format!("UID {}", m.uid)];
        if items.contains("FLAGS") {
            let flags: Vec<&str> = m.flags.iter().map(String::as_str).collect();
            parts.push(format!("FLAGS ({})", flags.join(" ")));
        }
        if items.contains("INTERNALDATE") {
            parts.push(format!("INTERNALDATE \"{}\"", m.internal_date));
        }
        if items.contains("RFC822.SIZE") {
            parts.push(format!("RFC822.SIZE {}", m.raw.len()));
        }
        out.extend_from_slice(format!("* {seq} FETCH ({}", parts.join(" ")).as_bytes());
        if wants_header {
            let header = m.header_bytes().to_vec();
            out.extend_from_slice(format!(" BODY[HEADER] {{{}}}\r\n", header.len()).as_bytes());
            out.extend_from_slice(&header);
        }
        if wants_full {
            full += 1;
            out.extend_from_slice(format!(" BODY[] {{{}}}\r\n", m.raw.len()).as_bytes());
            out.extend_from_slice(&m.raw);
        }
        out.extend_from_slice(b")\r\n");
    }
    s.flag_rows += flag_rows;
    s.non_peek_fetches += touched;
    s.body_fetches += full;
    out.extend_from_slice(format!("{tag} OK done\r\n").as_bytes());
    out
}

/// Parses an IMAP number set such as `1:5,9,12:*`.
///
/// As on real servers, `*` means the largest number, and a range is unordered,
/// so `9:*` matches the newest message even when the newest number is below 9.
fn parse_set(set: &str, max: u32) -> impl Fn(u32) -> bool {
    let num = move |t: &str| -> u32 {
        if t == "*" {
            max
        } else {
            t.parse().unwrap_or(0)
        }
    };
    let ranges: Vec<(u32, u32)> = set
        .split(',')
        .map(|p| match p.split_once(':') {
            Some((a, b)) => {
                let (a, b) = (num(a), num(b));
                (a.min(b), a.max(b))
            }
            None => (num(p), num(p)),
        })
        .collect();
    move |n| ranges.iter().any(|(a, b)| n >= *a && n <= *b)
}

/// Splits a run of IMAP strings, quoted or bare.
fn parse_strings(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = input.trim().chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut cur = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => cur.extend(chars.next()),
                    '"' => break,
                    c => cur.push(c),
                }
            }
            out.push(cur);
        } else {
            let mut cur = String::new();
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                cur.push(c);
                chars.next();
            }
            out.push(cur);
        }
    }
    out
}
