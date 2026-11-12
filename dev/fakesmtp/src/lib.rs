//! A tiny outgoing mail server. It checks the login and keeps what it receives,
//! so tests can read exactly what would have gone out.

use data_encoding::BASE64;
use fakeimap::FakeServer;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug)]
pub struct Received {
    pub from: String,
    pub to: Vec<String>,
    pub data: String,
}

#[derive(Default)]
struct Config {
    user: String,
    pass: String,
    /// Access tokens that work, and whose they are.
    oauth_tokens: Vec<(String, String)>,
    /// Refuse every message after accepting it for delivery.
    refuse_with: Option<String>,
    /// Files a copy in this folder, as Gmail and others do by themselves.
    file_copy_in: Option<(FakeServer, String)>,
    /// Delivers to this inbox when addressed to one of these people.
    deliver_to: Vec<(String, FakeServer)>,
}

#[derive(Clone)]
pub struct FakeSmtp {
    pub addr: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
    config: Arc<Mutex<Config>>,
}

impl FakeSmtp {
    pub fn allow_token(&self, token: &str, user: &str) {
        self.config
            .lock()
            .unwrap()
            .oauth_tokens
            .push((token.to_string(), user.to_string()));
    }

    pub async fn start(user: &str, pass: &str) -> FakeSmtp {
        Self::start_on("127.0.0.1:0", user, pass).await
    }

    pub async fn start_on(bind: &str, user: &str, pass: &str) -> FakeSmtp {
        let listener = TcpListener::bind(bind).await.expect("bind fake smtp");
        let addr = listener.local_addr().expect("addr");
        let server = FakeSmtp {
            addr,
            received: Arc::default(),
            config: Arc::new(Mutex::new(Config {
                user: user.into(),
                pass: pass.into(),
                oauth_tokens: Vec::new(),
                ..Default::default()
            })),
        };
        let shared = server.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let shared = shared.clone();
                tokio::spawn(async move {
                    let _ = serve(sock, shared).await;
                });
            }
        });
        server
    }

    pub fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    pub fn refuse_with(&self, reply: &str) {
        self.config.lock().unwrap().refuse_with = Some(reply.into());
    }

    pub fn stop_refusing(&self) {
        self.config.lock().unwrap().refuse_with = None;
    }

    pub fn set_login(&self, user: &str, pass: &str) {
        let mut c = self.config.lock().unwrap();
        c.user = user.into();
        c.pass = pass.into();
    }

    pub fn file_copy_in(&self, server: FakeServer, folder: &str) {
        self.config.lock().unwrap().file_copy_in = Some((server, folder.into()));
    }

    pub fn deliver_to(&self, address: &str, server: FakeServer) {
        self.config
            .lock()
            .unwrap()
            .deliver_to
            .push((address.to_ascii_lowercase(), server));
    }
}

fn address_in(line: &str) -> String {
    line.split_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map(|(a, _)| a.to_string())
        .unwrap_or_default()
}

async fn serve(sock: TcpStream, server: FakeSmtp) -> std::io::Result<()> {
    let (read, mut write) = sock.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"220 fakesmtp ready\r\n").await?;
    let mut authed = false;
    let mut from = String::new();
    let mut to = Vec::new();

    while let Some(line) = lines.next_line().await? {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            write
                .write_all(b"250-fakesmtp\r\n250-8BITMIME\r\n250 AUTH PLAIN LOGIN XOAUTH2\r\n")
                .await?;
        } else if upper.starts_with("AUTH PLAIN") {
            let encoded = line.split_whitespace().nth(2).unwrap_or("");
            let decoded = BASE64.decode(encoded.as_bytes()).unwrap_or_default();
            let parts: Vec<&[u8]> = decoded.split(|b| *b == 0).collect();
            let ok = {
                let c = server.config.lock().unwrap();
                parts.len() == 3 && parts[1] == c.user.as_bytes() && parts[2] == c.pass.as_bytes()
            };
            if ok {
                authed = true;
                write.write_all(b"235 ok\r\n").await?;
            } else {
                write.write_all(b"535 bad login\r\n").await?;
            }
        } else if upper.starts_with("AUTH XOAUTH2") {
            let encoded = line.split_whitespace().nth(2).unwrap_or("");
            let decoded = BASE64.decode(encoded.as_bytes()).unwrap_or_default();
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
            let ok = server
                .config
                .lock()
                .unwrap()
                .oauth_tokens
                .iter()
                .any(|(t, u)| *t == token && *u == user);
            if ok {
                authed = true;
                write.write_all(b"235 ok\r\n").await?;
            } else {
                write.write_all(b"535 bad token\r\n").await?;
            }
        } else if upper.starts_with("MAIL FROM") {
            if !authed {
                write.write_all(b"530 log in first\r\n").await?;
                continue;
            }
            from = address_in(&line);
            to.clear();
            write.write_all(b"250 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO") {
            to.push(address_in(&line));
            write.write_all(b"250 ok\r\n").await?;
        } else if upper == "DATA" {
            write.write_all(b"354 go\r\n").await?;
            let mut data = String::new();
            while let Some(l) = lines.next_line().await? {
                if l == "." {
                    break;
                }
                data.push_str(l.strip_prefix('.').unwrap_or(&l));
                data.push_str("\r\n");
            }
            let refused = server.config.lock().unwrap().refuse_with.clone();
            if let Some(reply) = refused {
                write.write_all(format!("{reply}\r\n").as_bytes()).await?;
                continue;
            }
            server.received.lock().unwrap().push(Received {
                from: from.clone(),
                to: to.clone(),
                data: data.clone(),
            });
            {
                let c = server.config.lock().unwrap();
                if let Some((imap, folder)) = &c.file_copy_in {
                    imap.add_message(folder, &data, true);
                }
                for (who, imap) in &c.deliver_to {
                    if to.iter().any(|t| t.eq_ignore_ascii_case(who)) {
                        imap.add_message("INBOX", &data, false);
                    }
                }
            }
            write.write_all(b"250 queued\r\n").await?;
        } else if upper == "NOOP" || upper == "RSET" {
            write.write_all(b"250 ok\r\n").await?;
        } else if upper == "QUIT" {
            write.write_all(b"221 bye\r\n").await?;
            return Ok(());
        } else {
            write.write_all(b"502 unsupported\r\n").await?;
        }
    }
    Ok(())
}
