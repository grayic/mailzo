//! An always-on server that works for this person while their devices are off: Mailzo
//! Cloud, or Mailzo Server on a machine of their own. Once paired it keeps the same
//! inboxes and choices as this device (both ways), brings snoozed mail and reminders back,
//! runs filters, and sends scheduled messages.
//!
//! Exactly one side sends each scheduled message. The device builds it in full, dated for
//! when it goes, and hands it over; from then on only the server may send it. Changing or
//! cancelling it here first takes it back from the server, which says whether it already
//! went. If the server can't be reached the message stays with the server, untouched, so it
//! can never go out from both.
//!
//! Pairing with a server of one's own takes its address and a short code shown on it. The
//! code is never sent: both sides prove they know it (SPAKE2), so a wrong server or one in
//! the middle learns nothing it could reuse, and both screens show the same six numbers for
//! the person to compare. Mailzo Cloud is paired with one click from a signed-in device:
//! the account server vouches for who the person is.
//!
//! After pairing every request is signed with a secret only the two hold, and so is every
//! answer, on top of the encrypted connection.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::unix_now;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};

pub const DEVICE_HEADER: &str = "x-mailzo-device";
pub const TIME_HEADER: &str = "x-mailzo-time";
pub const SIGN_HEADER: &str = "x-mailzo-sign";
pub const NONCE_HEADER: &str = "x-mailzo-nonce";
/// How far a signed request's time may be from the server's.
pub const CLOCK_SLACK: i64 = 300;
/// The most a handed-over message may weigh, attachments and all.
pub const MOST_MESSAGE_BYTES: usize = 30 * 1024 * 1024;

const URL: &str = "helper_url";
const KIND: &str = "helper_kind";
const NAME: &str = "helper_name";
const DEVICE: &str = "helper_device";
const SYNCED: &str = "helper_synced_at";
const PROBLEM: &str = "helper_problem";
pub(crate) const SEEN: &str = "helper_vault_seen";
/// On the server: the version of what it keeps for this person.
pub(crate) const SERVER_VERSION: &str = "helper_vault_version";
const SECRET: &str = "helper-secret";
/// How often the device brings itself and the server into step by itself.
const SYNC_EVERY: i64 = 60;

type HmacSha256 = Hmac<Sha256>;

fn mac_hex(key: &[u8], parts: &[&[u8]]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("any key length");
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            mac.update(b"\n");
        }
        mac.update(p);
    }
    data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())
}

fn body_hash(body: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(body))
}

/// The signature on a request to `path` at `time`, made unique by `nonce`. `secret` is hex.
pub fn request_sign(secret: &str, path: &str, time: i64, nonce: &str, body: &[u8]) -> String {
    mac_hex(
        secret.as_bytes(),
        &[
            b"mailzo-helper-request",
            path.as_bytes(),
            time.to_string().as_bytes(),
            nonce.as_bytes(),
            body_hash(body).as_bytes(),
        ],
    )
}

/// The signature on the answer to the request signed `request`.
pub fn reply_sign(secret: &str, request: &str, body: &[u8]) -> String {
    mac_hex(
        secret.as_bytes(),
        &[
            b"mailzo-helper-reply",
            request.as_bytes(),
            body_hash(body).as_bytes(),
        ],
    )
}

/// Compares two signatures without giving away where they differ.
pub fn same_sign(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A pairing code as typed: letters and digits only, in capitals.
pub fn clean_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A new pairing code: eight letters and digits that can't be mistaken for each other,
/// shown as two groups of four.
pub fn new_code() -> String {
    use rand::Rng;
    const LETTERS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let code: String = (0..8)
        .map(|_| LETTERS[rng.gen_range(0..LETTERS.len())] as char)
        .collect();
    format!("{}-{}", &code[..4], &code[4..])
}

/// Which pairing a code belongs to, without giving the rest of it away.
pub fn code_hint(code: &str) -> String {
    clean_code(code).chars().take(2).collect()
}

const MAC_ID: &[u8] = b"mailzo-device";
const SERVER_ID: &[u8] = b"mailzo-server";

/// The server's half of a pairing: its message, and the key both will share if the
/// device knew the same code.
pub fn pair_server(code: &str, device_message: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    let (state, mine) = Spake2::<Ed25519Group>::start_b(
        &Password::new(clean_code(code).as_bytes()),
        &Identity::new(MAC_ID),
        &Identity::new(SERVER_ID),
    );
    let key = state
        .finish(device_message)
        .map_err(|_| CoreError::Invalid("That pairing message couldn't be read.".into()))?;
    Ok((mine, key))
}

/// What proves to the device that the server knew the code.
pub fn server_proof(key: &[u8]) -> String {
    mac_hex(key, &[b"mailzo-pair-server"])
}

/// What proves to the server that the device knew the code.
pub fn device_proof(key: &[u8]) -> String {
    mac_hex(key, &[b"mailzo-pair-device"])
}

/// What the device sends while it waits for the person to say yes on the server.
pub fn waiting_proof(key: &[u8]) -> String {
    mac_hex(key, &[b"mailzo-pair-waiting"])
}

/// The six numbers both screens show, as "123 456".
pub fn pair_numbers(key: &[u8]) -> String {
    let mac = mac_hex(key, &[b"mailzo-pair-numbers"]);
    let n = u64::from_str_radix(&mac[..12], 16).unwrap_or(0) % 1_000_000;
    let n = format!("{n:06}");
    format!("{} {}", &n[..3], &n[3..])
}

/// The secret the device and the server keep after pairing, as hex.
pub fn pair_secret(key: &[u8]) -> String {
    let mut out = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(None, key)
        .expand(b"mailzo-helper-device-secret", &mut out)
        .expect("32 bytes");
    data_encoding::HEXLOWER.encode(&out)
}

/// A pairing started on this device, waiting for the person.
pub(crate) struct Pending {
    url: String,
    pair_id: String,
    key: Vec<u8>,
    numbers: String,
    name: String,
}

/// What the engine keeps about the server between calls.
#[derive(Default)]
pub(crate) struct HelperState {
    pub(crate) pending: std::sync::Mutex<Option<Pending>>,
    /// One sync at a time, here, and on the server one change to what it keeps at a time.
    pub(crate) busy: tokio::sync::Mutex<()>,
}

/// The paired server, as this device knows it.
#[derive(Clone)]
pub(crate) struct Link {
    url: String,
    device: String,
    secret: String,
}

/// A scheduled message, built in full, handed to the server.
#[derive(Serialize, Deserialize)]
pub struct SendJob {
    pub job_id: String,
    /// The inbox it goes from, by the name every device knows it by.
    pub inbox: String,
    pub send_at: i64,
    pub from: Option<String>,
    pub to: Vec<String>,
    /// As it goes out, base64.
    pub raw: String,
    /// As it's kept in Sent, base64.
    pub copy: String,
    pub rfc_id: String,
    /// Which conversation to bring back if nobody replies in `remind_days`.
    pub follow_thread: Option<String>,
    pub remind_days: Option<i64>,
}

#[derive(Deserialize)]
struct JobState {
    state: String,
    #[serde(default)]
    problem: Option<String>,
}

fn offline() -> CoreError {
    CoreError::Cloud {
        code: "helper_offline".into(),
        message: "Your always-on server couldn't be reached.".into(),
    }
}

fn clean_url(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let parsed = reqwest::Url::parse(&url)
        .ok()
        .filter(|u| u.username().is_empty() && u.password().is_none() && !url.contains('\\'));
    let local = parsed.as_ref().is_some_and(|u| {
        u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    });
    match parsed {
        Some(u) if u.scheme() == "https" || local => Ok(url),
        _ => Err(CoreError::Invalid(
            "That doesn't look like a server address. It should start with https://.".into(),
        )),
    }
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .tls_backend_preconfigured(crate::cloud::tls_config()?)
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CoreError::Network(e.without_url().to_string()))
}

/// The server's plain-English answer when it says no.
fn refused(status: reqwest::StatusCode, body: &[u8]) -> CoreError {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    CoreError::Cloud {
        code: v["code"].as_str().unwrap_or("helper_refused").to_string(),
        message: v["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Your always-on server said no ({status}).")),
    }
}

/// An unsigned request, only for pairing.
async fn post_plain(url: &str, path: &str, body: Value) -> Result<Value> {
    let r = http()?
        .post(format!("{url}{path}"))
        .json(&body)
        .send()
        .await
        .map_err(|_| offline())?;
    let status = r.status();
    let bytes = r.bytes().await.map_err(|_| offline())?;
    if !status.is_success() {
        return Err(refused(status, &bytes));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| CoreError::Invalid("Your server's answer couldn't be read.".into()))
}

impl Link {
    pub(crate) fn secret(&self) -> &str {
        &self.secret
    }
}

impl Engine {
    pub(crate) fn helper_link(&self) -> Result<Option<Link>> {
        let (Some(url), Some(device)) = (self.store.setting(URL)?, self.store.setting(DEVICE)?)
        else {
            return Ok(None);
        };
        let Some(secret) = self.secrets.get(SECRET)? else {
            return Ok(None);
        };
        Ok(Some(Link {
            url,
            device,
            secret,
        }))
    }

    /// A signed request to the paired server, whose answer must be signed too.
    pub(crate) async fn helper_post<T: serde::de::DeserializeOwned>(
        &self,
        link: &Link,
        call: &str,
        body: Value,
    ) -> Result<T> {
        let path = format!("/helper/call/{call}");
        let body = serde_json::to_vec(&body).map_err(|e| CoreError::Invalid(e.to_string()))?;
        let time = unix_now();
        let nonce = crate::crypto::random_hex(16);
        let sign = request_sign(&link.secret, &path, time, &nonce, &body);
        let r = http()?
            .post(format!("{}{path}", link.url))
            .header(DEVICE_HEADER, &link.device)
            .header(TIME_HEADER, time.to_string())
            .header(NONCE_HEADER, &nonce)
            .header(SIGN_HEADER, &sign)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| offline())?;
        let status = r.status();
        let their_sign = r
            .headers()
            .get(SIGN_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = r.bytes().await.map_err(|_| offline())?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CoreError::Cloud {
                code: "helper_forgot".into(),
                message: "Your always-on server no longer knows this device. Connect it again."
                    .into(),
            });
        }
        if !same_sign(&their_sign, &reply_sign(&link.secret, &sign, &bytes)) {
            // Not from the server this device paired with, whatever it says.
            return Err(CoreError::Cloud {
                code: "helper_unsigned".into(),
                message: "An answer claiming to be from your always-on server wasn't from it, so it was ignored.".into(),
            });
        }
        if !status.is_success() {
            return Err(refused(status, &bytes));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| CoreError::Invalid("Your server's answer couldn't be read.".into()))
    }

    fn device_name(&self) -> String {
        self.store
            .identity()
            .ok()
            .flatten()
            .map(|i| i.device_name)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Mac".into())
    }

    fn keep_link(
        &self,
        url: &str,
        kind: &str,
        name: &str,
        device: &str,
        secret: &str,
    ) -> Result<()> {
        self.secrets.set(SECRET, secret)?;
        self.store.set_setting(URL, url)?;
        self.store.set_setting(KIND, kind)?;
        self.store.set_setting(NAME, name)?;
        self.store.set_setting(DEVICE, device)?;
        self.store.set_setting(SEEN, "0")?;
        self.store.delete_setting(PROBLEM)?;
        Ok(())
    }

    pub(crate) fn helper_status(&self) -> Result<Value> {
        let pending = self
            .helper
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|p| json!({ "numbers": p.numbers, "name": p.name }));
        let connected = self.helper_link()?.is_some();
        let handed = if connected {
            self.store.helper_jobs()?.len()
        } else {
            0
        };
        Ok(json!({
            "connected": connected,
            "kind": self.store.setting(KIND)?,
            "name": self.store.setting(NAME)?,
            "url": self.store.setting(URL)?,
            "synced_at": self.store.setting(SYNCED)?.and_then(|t| t.parse::<i64>().ok()),
            "problem": self.store.setting(PROBLEM)?,
            "scheduled_on_server": handed,
            "pending": pending,
        }))
    }

    /// Step one of pairing with a server of one's own: proves both know the code, and
    /// returns the six numbers to compare with the server's screen.
    pub(crate) async fn helper_pair_start(&self, url: &str, code: &str) -> Result<Value> {
        use spake2::{Ed25519Group, Identity, Password, Spake2};
        if self.helper_link()?.is_some() {
            return Err(CoreError::Invalid(
                "This device is already connected to an always-on server. Disconnect it first."
                    .into(),
            ));
        }
        let url = clean_url(url)?;
        let clean = clean_code(code);
        if clean.len() != 8 {
            return Err(CoreError::Invalid(
                "The code is eight letters and numbers, as shown on your server.".into(),
            ));
        }
        let (state, mine) = Spake2::<Ed25519Group>::start_a(
            &Password::new(clean.as_bytes()),
            &Identity::new(MAC_ID),
            &Identity::new(SERVER_ID),
        );
        let reply = post_plain(
            &url,
            "/helper/pair/start",
            json!({ "hint": code_hint(&clean), "message": data_encoding::BASE64.encode(&mine) }),
        )
        .await?;
        let theirs = reply["message"]
            .as_str()
            .and_then(|m| data_encoding::BASE64.decode(m.as_bytes()).ok())
            .ok_or_else(|| CoreError::Invalid("Your server's answer couldn't be read.".into()))?;
        let wrong = || {
            CoreError::Invalid(
                "That code didn't work. Make a new one on your server and try again.".into(),
            )
        };
        let key = state.finish(&theirs).map_err(|_| wrong())?;
        if !same_sign(reply["proof"].as_str().unwrap_or(""), &server_proof(&key)) {
            return Err(wrong());
        }
        let numbers = pair_numbers(&key);
        let name = reply["name"].as_str().unwrap_or("your server").to_string();
        *self
            .helper
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            url,
            pair_id: reply["pair_id"].as_str().unwrap_or("").to_string(),
            key,
            numbers: numbers.clone(),
            name: name.clone(),
        });
        Ok(json!({ "numbers": numbers, "name": name }))
    }

    /// Step two: the person saw the same numbers here. The server's screen asks them too.
    pub(crate) async fn helper_pair_confirm(&self) -> Result<Value> {
        let (url, pair_id, proof) = {
            let pending = self
                .helper
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let p = pending
                .as_ref()
                .ok_or_else(|| CoreError::Invalid("Start pairing again.".into()))?;
            (p.url.clone(), p.pair_id.clone(), device_proof(&p.key))
        };
        post_plain(
            &url,
            "/helper/pair/finish",
            json!({ "pair_id": pair_id, "proof": proof, "device_name": self.device_name() }),
        )
        .await?;
        Ok(json!({ "state": "waiting" }))
    }

    /// Step three, asked every few seconds: whether the person said yes on the server too.
    pub(crate) async fn helper_pair_wait(&self) -> Result<Value> {
        let (url, pair_id, proof, key, name) = {
            let pending = self
                .helper
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let p = pending
                .as_ref()
                .ok_or_else(|| CoreError::Invalid("Start pairing again.".into()))?;
            (
                p.url.clone(),
                p.pair_id.clone(),
                waiting_proof(&p.key),
                p.key.clone(),
                p.name.clone(),
            )
        };
        let reply = post_plain(
            &url,
            "/helper/pair/wait",
            json!({ "pair_id": pair_id, "proof": proof }),
        )
        .await?;
        match reply["state"].as_str() {
            Some("approved") => {
                let device = reply["device_id"].as_str().unwrap_or("").to_string();
                self.keep_link(&url, "own", &name, &device, &pair_secret(&key))?;
                *self
                    .helper
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                let _ = self.helper_tick(true).await;
                Ok(json!({ "state": "approved" }))
            }
            Some("refused") => {
                *self
                    .helper
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                Ok(json!({ "state": "refused" }))
            }
            _ => Ok(json!({ "state": "waiting" })),
        }
    }

    pub(crate) fn helper_pair_cancel(&self) -> Value {
        *self
            .helper
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        json!({ "cancelled": true })
    }

    /// Mailzo Cloud, in one click: the account server vouches for who this is.
    pub(crate) async fn helper_cloud_on(&self) -> Result<Value> {
        if self.helper_link()?.is_some() {
            return Err(CoreError::Invalid(
                "This device is already connected to an always-on server. Disconnect it first."
                    .into(),
            ));
        }
        if !self.is_ready()? {
            return Err(CoreError::Cloud {
                code: "sign_in_first".into(),
                message: "Sign in to your Mailzo account first.".into(),
            });
        }
        let ticket: Value = self
            .cloud_send(reqwest::Method::POST, "/helper/ticket", Some(json!({})))
            .await?;
        let url = clean_url(ticket["url"].as_str().unwrap_or(""))?;
        let reply = post_plain(
            &url,
            "/helper/pair/cloud",
            json!({ "ticket": ticket["ticket"], "device_name": self.device_name() }),
        )
        .await?;
        let (Some(device), Some(secret)) = (reply["device_id"].as_str(), reply["secret"].as_str())
        else {
            return Err(CoreError::Invalid(
                "Mailzo Cloud's answer couldn't be read.".into(),
            ));
        };
        self.keep_link(&url, "cloud", "Mailzo Cloud", device, secret)?;
        let _ = self.helper_tick(true).await;
        self.helper_status()
    }

    /// Stops using the server. Scheduled messages it holds are taken back first, so they
    /// still go. `anyway` disconnects even if the server can't be reached: what it holds is
    /// then left unscheduled here with a note, never sent from here by itself.
    pub(crate) async fn helper_disconnect(&self, anyway: bool) -> Result<Value> {
        let Some(link) = self.helper_link()? else {
            return Ok(json!({ "disconnected": true }));
        };
        let _busy = self.helper.busy.lock().await;
        let mut unsure = 0;
        for (draft_id, job) in self.store.helper_jobs()? {
            match self.take_back_job(&link, draft_id, &job).await {
                Ok(()) => {}
                Err(_) if anyway => {
                    self.store.set_send_problem(
                        draft_id,
                        "This was handed to your always-on server, which couldn't be reached when you disconnected it. Check Sent before sending it again.",
                    )?;
                    self.store.set_helper_job(draft_id, None)?;
                    unsure += 1;
                }
                Err(e) => return Err(e),
            }
        }
        let _: Result<Value> = self.helper_post(&link, "forget", json!({})).await;
        for key in [URL, KIND, NAME, DEVICE, SYNCED, PROBLEM, SEEN] {
            self.store.delete_setting(key)?;
        }
        self.secrets.delete(SECRET)?;
        Ok(json!({ "disconnected": true, "unsure": unsure }))
    }

    /// Before a scheduled message is changed, cancelled or sent from here: takes it back
    /// from the server if the server holds it.
    pub(crate) async fn helper_take_back(&self, draft_id: i64) -> Result<()> {
        let Some(job) = self.store.helper_job(draft_id)? else {
            return Ok(());
        };
        let Some(link) = self.helper_link()? else {
            self.store.set_helper_job(draft_id, None)?;
            return Ok(());
        };
        let _busy = self.helper.busy.lock().await;
        self.take_back_job(&link, draft_id, &job).await
    }

    async fn take_back_job(&self, link: &Link, draft_id: i64, job: &str) -> Result<()> {
        let reply: JobState = self
            .helper_post(link, "send/cancel", json!({ "job_id": job }))
            .await
            .map_err(|e| match e {
                CoreError::Cloud { code, .. } if code == "helper_offline" => CoreError::Cloud {
                    code: "helper_offline".into(),
                    message: "Your always-on server is in charge of sending this, and it couldn't be reached, so it can't be changed right now. Try again in a moment.".into(),
                },
                e => e,
            })?;
        match reply.state.as_str() {
            "sent" => {
                self.finish_handed(draft_id).await;
                Err(CoreError::Send {
                    code: "already_sent".into(),
                    message: "This has already been sent, by your always-on server.".into(),
                })
            }
            "sending" => Err(CoreError::Send {
                code: "already_sending".into(),
                message: "Your always-on server is sending this right now.".into(),
            }),
            // Cancelled, or it never got there: it's this device's again.
            _ => self.store.set_helper_job(draft_id, None),
        }
    }

    /// A message the server sent: cleared away here as if it had gone from here.
    async fn finish_handed(&self, draft_id: i64) {
        let Ok(draft) = self.store.draft(draft_id) else {
            return;
        };
        let _ = self.store.mark_draft_sent(draft_id);
        if self.store.delete_draft(draft_id).is_ok() {
            self.remove_draft_files(draft_id);
            let _ = self.store.clear_draft_team_base(draft_id);
        }
        if let Some(parent) = draft.parent_message_id.filter(|_| draft.mode != "forward") {
            if let Ok(Some(thread)) = self.store.message_thread(parent) {
                let _ = self.store.clear_returned(draft.account_id, &thread);
            }
            if draft.mode == "reply" || draft.mode == "reply_all" {
                if let (Ok(account), Ok(password)) = (
                    self.store.account(draft.account_id),
                    self.password(draft.account_id).await,
                ) {
                    let _ = self.mark_answered(&account, &password, parent).await;
                }
            }
        }
    }

    /// Brings this device and the server into step: what each keeps, the scheduled
    /// messages the server now holds, and what became of them. At most once a minute
    /// unless `now`.
    pub(crate) async fn helper_tick(&self, now: bool) -> Result<Value> {
        let Some(link) = self.helper_link()? else {
            return Ok(json!({ "connected": false }));
        };
        let last: i64 = self
            .store
            .setting(SYNCED)?
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if !now && unix_now() - last < SYNC_EVERY {
            return Ok(json!({ "connected": true, "skipped": true }));
        }
        let Ok(_busy) = self.helper.busy.try_lock() else {
            return Ok(json!({ "connected": true, "skipped": true }));
        };
        match self.helper_round(&link).await {
            Ok(changed) => {
                self.store.set_setting(SYNCED, &unix_now().to_string())?;
                self.store.delete_setting(PROBLEM)?;
                Ok(json!({ "connected": true, "changed": changed }))
            }
            Err(e) => {
                self.store.set_setting(PROBLEM, &e.to_string())?;
                Err(e)
            }
        }
    }

    async fn helper_round(&self, link: &Link) -> Result<bool> {
        let mut changed = self.helper_vault_sync(link).await?;
        // What became of the messages already handed over.
        let jobs = self.store.helper_jobs()?;
        let mut not_there = Vec::new();
        if !jobs.is_empty() {
            let ids: Vec<&str> = jobs.iter().map(|(_, j)| j.as_str()).collect();
            let states: std::collections::HashMap<String, JobState> = self
                .helper_post(link, "send/status", json!({ "jobs": ids }))
                .await?;
            for (draft_id, job) in &jobs {
                match states.get(job).map(|s| (s.state.as_str(), s)) {
                    Some(("sent", _)) => {
                        self.finish_handed(*draft_id).await;
                        changed = true;
                    }
                    Some(("problem", s)) => {
                        let text = s
                            .problem
                            .clone()
                            .unwrap_or_else(|| "Your always-on server couldn't send this.".into());
                        self.store.set_send_problem(*draft_id, &text)?;
                        self.store.set_helper_job(*draft_id, None)?;
                        changed = true;
                    }
                    Some(("waiting" | "sending", _)) => {}
                    // Never arrived (the answer was lost), so it's handed over again under
                    // the same name, which the server only ever takes once.
                    _ => not_there.push((*draft_id, job.clone())),
                }
            }
        }
        // Scheduled messages still here go to the server, which sends them from now on.
        for draft in self.store.drafts()? {
            let Some(send_at) = draft.send_at else {
                continue;
            };
            let job = match self.store.helper_job(draft.id)? {
                Some(job) if not_there.iter().any(|(id, _)| *id == draft.id) => job,
                Some(_) => continue,
                None => {
                    if !self.may_hand_over(&draft)? {
                        continue;
                    }
                    let job = crate::crypto::random_hex(16);
                    // Named before it's sent, so from this moment this device never sends
                    // it by itself, even if the answer is lost or the app stops. Not while
                    // it's being sent from here, which checks the same under the same lock.
                    let running = self.sending.lock().unwrap_or_else(|e| e.into_inner());
                    if running.contains(&draft.id) {
                        continue;
                    }
                    self.store.set_helper_job(draft.id, Some(&job))?;
                    drop(running);
                    job
                }
            };
            match self.hand_over(link, &draft, send_at, &job).await {
                Ok(()) => changed = true,
                Err(CoreError::Send { message, .. }) | Err(CoreError::Invalid(message)) => {
                    // It can't be built, so it couldn't go from here either.
                    self.store.set_send_problem(draft.id, &message)?;
                    self.store.set_helper_job(draft.id, None)?;
                    changed = true;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(changed)
    }

    /// Team inboxes check with the team at the moment of sending, which only this device
    /// can do, so those stay here.
    fn may_hand_over(&self, draft: &crate::model::Draft) -> Result<bool> {
        Ok(!self.store.account(draft.account_id)?.prefs.team_shared)
    }

    async fn hand_over(
        &self,
        link: &Link,
        draft: &crate::model::Draft,
        send_at: i64,
        job: &str,
    ) -> Result<()> {
        let date = SystemTime::UNIX_EPOCH + Duration::from_secs(send_at.max(0) as u64);
        let prepared = self.prepare_message(draft, date).await?;
        let envelope = prepared.message.envelope().clone();
        let raw = prepared.message.formatted();
        if raw.len() + prepared.copy.len() > MOST_MESSAGE_BYTES {
            return Err(CoreError::Send {
                code: "too_big_for_server".into(),
                message: "This is too big to hand to your always-on server, so it wasn't scheduled. Make the attachments smaller, or send it yourself.".into(),
            });
        }
        let follow_thread = match draft.parent_message_id.filter(|_| draft.mode != "forward") {
            Some(parent) => self.store.message_thread(parent)?,
            None => None,
        }
        .unwrap_or_else(|| prepared.rfc_id.clone());
        let send = SendJob {
            job_id: job.to_string(),
            inbox: prepared.account.sync_id.clone(),
            send_at,
            from: envelope.from().map(|a| a.to_string()),
            to: envelope.to().iter().map(|a| a.to_string()).collect(),
            raw: data_encoding::BASE64.encode(&raw),
            copy: data_encoding::BASE64.encode(&prepared.copy),
            rfc_id: prepared.rfc_id,
            follow_thread: Some(follow_thread),
            remind_days: draft.remind_days.filter(|d| *d > 0),
        };
        let _: JobState = self
            .helper_post(link, "send", serde_json::to_value(send).unwrap_or_default())
            .await?;
        Ok(())
    }

    // ---- on the server ----

    /// One signed request from a paired device, on the server. `secret` is that device's.
    pub async fn helper_serve(&self, secret: &str, call: &str, body: Value) -> Result<Value> {
        match call {
            "vault/get" => self.helper_vault_get(secret),
            "vault/put" => {
                let _busy = self.helper.busy.lock().await;
                self.helper_vault_put(
                    secret,
                    body["expected_version"].as_i64().unwrap_or(-1),
                    body["blob"].as_str().unwrap_or(""),
                )
            }
            "send" => {
                let job: SendJob = serde_json::from_value(body)
                    .map_err(|_| CoreError::Invalid("That message couldn't be read.".into()))?;
                self.store.helper_send_put(&job)?;
                Ok(self.store.helper_send_state(&job.job_id)?)
            }
            "send/cancel" => {
                let job = body["job_id"].as_str().unwrap_or("");
                self.store.helper_send_cancel(job)?;
                Ok(self.store.helper_send_state(job)?)
            }
            "send/status" => {
                let mut out = serde_json::Map::new();
                for job in body["jobs"].as_array().into_iter().flatten() {
                    if let Some(job) = job.as_str() {
                        out.insert(job.to_string(), self.store.helper_send_state(job)?);
                    }
                }
                Ok(Value::Object(out))
            }
            "forget" => Ok(json!({ "ok": true })),
            _ => Err(CoreError::NotFound("No such request.".into())),
        }
    }

    /// On the server: sends what's due among the messages devices handed over. Each is
    /// claimed before it's tried, and never tried twice.
    pub(crate) async fn helper_sends_due(&self, now: i64) -> usize {
        let Ok(due) = self.store.helper_sends_due(now) else {
            return 0;
        };
        let mut sent = 0;
        for job_id in due {
            if !self.store.helper_send_claim(&job_id).unwrap_or(false) {
                continue;
            }
            match self.helper_send_one(&job_id).await {
                Ok(true) => sent += 1,
                Ok(false) => {
                    let _ = self.store.helper_send_unclaim(&job_id);
                }
                Err(e) => {
                    let unsure = matches!(&e, CoreError::Send { code, .. } if matches!(code.as_str(), "smtp_connect" | "smtp_busy"))
                        || matches!(&e, CoreError::Network(_) | CoreError::Offline);
                    let text = if unsure {
                        format!("{e} It may or may not have gone, so check Sent before sending it again.")
                    } else {
                        e.to_string()
                    };
                    let _ = self.store.helper_send_done(&job_id, Some(&text));
                }
            }
        }
        sent
    }

    /// Ok(false): the outgoing server couldn't be reached before anything was sent, so it's
    /// tried again next time.
    async fn helper_send_one(&self, job_id: &str) -> Result<bool> {
        let job = self.store.helper_send(job_id)?;
        let account = self
            .store
            .list_accounts()?
            .into_iter()
            .find(|a| a.sync_id == job.inbox)
            .ok_or_else(|| {
                CoreError::Invalid(
                    "The inbox this goes from isn't on your always-on server any more.".into(),
                )
            })?;
        let password = self.password(account.id).await?;
        if let Err(e) = self.check_smtp(account.id).await {
            let waiting = matches!(&e, CoreError::Network(_) | CoreError::Offline)
                || matches!(&e, CoreError::Send { code, .. } if matches!(code.as_str(), "smtp_connect" | "smtp_busy"));
            if waiting {
                return Ok(false);
            }
        }
        let bad = || CoreError::Invalid("That message couldn't be read.".into());
        let raw = data_encoding::BASE64
            .decode(job.raw.as_bytes())
            .map_err(|_| bad())?;
        let parse = |a: &String| a.parse::<lettre::Address>().map_err(|_| bad());
        let envelope = lettre::address::Envelope::new(
            job.from.as_ref().map(parse).transpose()?,
            job.to.iter().map(parse).collect::<Result<_>>()?,
        )
        .map_err(|_| bad())?;
        let (host, port, security) = self.smtp_settings(&account, &password).await?;
        crate::smtp::send_raw(
            &account,
            &password,
            &host,
            port,
            security,
            &envelope,
            &raw,
            self.reach(),
        )
        .await?;
        // From here it has gone. Nothing below may report failure.
        let _ = self.store.helper_send_done(job_id, None);
        if let (Some(days), Some(thread)) = (job.remind_days, &job.follow_thread) {
            let at = unix_now() + days * 86_400;
            if self
                .store
                .set_follow_up(account.id, thread, Some(at))
                .is_ok()
            {
                let _ = self.marks_changed().await;
            }
        }
        if let Ok(copy) = data_encoding::BASE64.decode(job.copy.as_bytes()) {
            let _ = self
                .save_sent_copy(&account, &password, &job.rfc_id, &copy)
                .await;
        }
        Ok(true)
    }
}

impl crate::store::Store {
    /// Scheduled messages this device handed to the server: (draft, its name there).
    pub(crate) fn helper_jobs(&self) -> Result<Vec<(i64, String)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, helper_job FROM drafts WHERE helper_job IS NOT NULL AND sent_at IS NULL",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn helper_job(&self, draft_id: i64) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .db()
            .query_row(
                "SELECT helper_job FROM drafts WHERE id = ?1",
                [draft_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub(crate) fn set_helper_job(&self, draft_id: i64, job: Option<&str>) -> Result<()> {
        self.db().execute(
            "UPDATE drafts SET helper_job = ?2 WHERE id = ?1",
            rusqlite::params![draft_id, job],
        )?;
        Ok(())
    }

    // On the server.

    /// Keeps a handed-over message. The same name is only ever taken once, so handing it
    /// over again after a lost answer changes nothing.
    pub(crate) fn helper_send_put(&self, job: &SendJob) -> Result<()> {
        if job.job_id.len() < 16 || job.raw.len() > MOST_MESSAGE_BYTES * 2 {
            return Err(CoreError::Invalid("That message couldn't be kept.".into()));
        }
        let envelope = json!({ "from": job.from, "to": job.to }).to_string();
        self.db().execute(
            "INSERT OR IGNORE INTO helper_sends
               (job_id, inbox, send_at, envelope_json, raw, copy, rfc_id, follow_thread,
                remind_days, made_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                job.job_id,
                job.inbox,
                job.send_at,
                envelope,
                job.raw,
                job.copy,
                job.rfc_id,
                job.follow_thread,
                job.remind_days,
                unix_now()
            ],
        )?;
        // Old records of what happened only need to last until every device has heard.
        self.db().execute(
            "DELETE FROM helper_sends WHERE done_at IS NOT NULL AND done_at < ?1",
            [unix_now() - 30 * 86_400],
        )?;
        Ok(())
    }

    pub(crate) fn helper_send_state(&self, job_id: &str) -> Result<Value> {
        use rusqlite::OptionalExtension;
        let row: Option<(String, Option<String>)> = self
            .db()
            .query_row(
                "SELECT state, problem FROM helper_sends WHERE job_id = ?1",
                [job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((state, problem)) => json!({ "state": state, "problem": problem }),
            None => json!({ "state": "unknown" }),
        })
    }

    /// Only a message still waiting can be cancelled; one being sent or sent stays so.
    pub(crate) fn helper_send_cancel(&self, job_id: &str) -> Result<()> {
        self.db().execute(
            "UPDATE helper_sends SET state = 'cancelled', raw = NULL, copy = NULL,
               done_at = ?2 WHERE job_id = ?1 AND state IN ('waiting', 'problem')",
            rusqlite::params![job_id, unix_now()],
        )?;
        Ok(())
    }

    pub(crate) fn helper_sends_due(&self, now: i64) -> Result<Vec<String>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT job_id FROM helper_sends WHERE state = 'waiting' AND send_at <= ?1
             ORDER BY send_at",
        )?;
        let rows = st.query_map([now], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Takes a due message for sending. False if something else already took it.
    pub(crate) fn helper_send_claim(&self, job_id: &str) -> Result<bool> {
        Ok(self.db().execute(
            "UPDATE helper_sends SET state = 'sending' WHERE job_id = ?1 AND state = 'waiting'",
            [job_id],
        )? == 1)
    }

    /// Nothing was sent, so it waits for the next try.
    pub(crate) fn helper_send_unclaim(&self, job_id: &str) -> Result<()> {
        self.db().execute(
            "UPDATE helper_sends SET state = 'waiting' WHERE job_id = ?1 AND state = 'sending'",
            [job_id],
        )?;
        Ok(())
    }

    /// Sent (no problem), or tried and failed (never tried again by itself).
    pub(crate) fn helper_send_done(&self, job_id: &str, problem: Option<&str>) -> Result<()> {
        self.db().execute(
            "UPDATE helper_sends SET state = ?2, problem = ?3, raw = NULL, copy = NULL,
               done_at = ?4 WHERE job_id = ?1",
            rusqlite::params![
                job_id,
                if problem.is_some() { "problem" } else { "sent" },
                problem,
                unix_now()
            ],
        )?;
        Ok(())
    }

    pub(crate) fn helper_send(&self, job_id: &str) -> Result<SendJob> {
        Ok(self.db().query_row(
            "SELECT inbox, send_at, envelope_json, raw, copy, rfc_id, follow_thread, remind_days
             FROM helper_sends WHERE job_id = ?1",
            [job_id],
            |r| {
                let envelope: Value =
                    serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default();
                Ok(SendJob {
                    job_id: job_id.to_string(),
                    inbox: r.get(0)?,
                    send_at: r.get(1)?,
                    from: envelope["from"].as_str().map(str::to_string),
                    to: envelope["to"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|a| a.as_str().map(str::to_string))
                        .collect(),
                    raw: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    copy: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    rfc_id: r.get(5)?,
                    follow_thread: r.get(6)?,
                    remind_days: r.get(7)?,
                })
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_agree_only_when_they_know_the_same_code() {
        use spake2::{Ed25519Group, Identity, Password, Spake2};
        let code = new_code();
        let device = |typed: &str| {
            Spake2::<Ed25519Group>::start_a(
                &Password::new(clean_code(typed).as_bytes()),
                &Identity::new(MAC_ID),
                &Identity::new(SERVER_ID),
            )
        };
        let (state, msg) = device(&code.to_lowercase().replace('-', " "));
        let (theirs, server_key) = pair_server(&code, &msg).unwrap();
        let key = state.finish(&theirs).unwrap();
        assert_eq!(key, server_key);
        assert_eq!(pair_numbers(&key), pair_numbers(&server_key));
        assert_eq!(pair_numbers(&key).len(), 7);
        assert!(same_sign(&server_proof(&key), &server_proof(&server_key)));
        assert_eq!(pair_secret(&key).len(), 64);

        let (state, msg) = device("WRNG-CODE");
        let (theirs, server_key) = pair_server(&code, &msg).unwrap();
        let key = state.finish(&theirs).unwrap();
        assert_ne!(key, server_key, "a wrong code shares nothing");
        assert!(!same_sign(&server_proof(&key), &server_proof(&server_key)));
    }

    #[test]
    fn a_signature_covers_the_path_the_time_and_every_byte() {
        let secret = crate::crypto::random_hex(32);
        let s = request_sign(&secret, "/helper/call/send", 100, "n", b"{}");
        assert!(same_sign(
            &s,
            &request_sign(&secret, "/helper/call/send", 100, "n", b"{}")
        ));
        assert!(!same_sign(
            &s,
            &request_sign(&secret, "/helper/call/send", 101, "n", b"{}")
        ));
        assert!(!same_sign(
            &s,
            &request_sign(&secret, "/helper/call/forget", 100, "n", b"{}")
        ));
        assert!(!same_sign(
            &s,
            &request_sign(&secret, "/helper/call/send", 100, "n", b"{ }")
        ));
        let other = crate::crypto::random_hex(32);
        assert!(!same_sign(
            &s,
            &request_sign(&other, "/helper/call/send", 100, "n", b"{}")
        ));
        assert!(!same_sign(
            &reply_sign(&secret, &s, b"a"),
            &reply_sign(&secret, &s, b"b")
        ));
    }

    #[test]
    fn codes_read_however_they_are_typed_and_server_addresses_need_https() {
        assert_eq!(clean_code(" abcd-efgh "), "ABCDEFGH");
        assert_eq!(code_hint("abcd-efgh"), "AB");
        assert_eq!(
            clean_url("mail.example.com/").unwrap(),
            "https://mail.example.com"
        );
        assert!(clean_url("http://mail.example.com").is_err());
        assert!(clean_url("http://127.0.0.1:8080").is_ok());
        assert!(clean_url("https://user@mail.example.com").is_err());
    }

    fn job(id: &str) -> SendJob {
        SendJob {
            job_id: id.into(),
            inbox: "inbox-1".into(),
            send_at: 50,
            from: Some("a@x.test".into()),
            to: vec!["b@x.test".into()],
            raw: "cmF3".into(),
            copy: "Y29weQ==".into(),
            rfc_id: "r@x.test".into(),
            follow_thread: None,
            remind_days: None,
        }
    }

    #[test]
    fn a_handed_over_message_is_taken_once_sent_once_and_cancelled_only_while_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(&dir.path().join("m.db")).unwrap();
        let id = "0123456789abcdef";
        store.helper_send_put(&job(id)).unwrap();
        let mut again = job(id);
        again.send_at = 999;
        store.helper_send_put(&again).unwrap();
        assert_eq!(
            store.helper_send(id).unwrap().send_at,
            50,
            "the first copy stands"
        );
        assert!(store.helper_sends_due(49).unwrap().is_empty());
        assert_eq!(store.helper_sends_due(50).unwrap(), vec![id.to_string()]);
        assert!(store.helper_send_claim(id).unwrap());
        assert!(!store.helper_send_claim(id).unwrap(), "claimed only once");
        store.helper_send_cancel(id).unwrap();
        assert_eq!(store.helper_send_state(id).unwrap()["state"], "sending");
        store.helper_send_done(id, None).unwrap();
        store.helper_send_cancel(id).unwrap();
        assert_eq!(store.helper_send_state(id).unwrap()["state"], "sent");
        assert_eq!(store.helper_send_state("nope").unwrap()["state"], "unknown");

        let other = "fedcba9876543210";
        store.helper_send_put(&job(other)).unwrap();
        store.helper_send_cancel(other).unwrap();
        assert_eq!(
            store.helper_send_state(other).unwrap()["state"],
            "cancelled"
        );
        assert!(
            !store.helper_send_claim(other).unwrap(),
            "a cancelled one never goes"
        );
    }
}
