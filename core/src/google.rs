//! Sign in with Google. The person approves in their real browser, Google sends
//! them back to a short-lived address on this computer, and the account server
//! finishes the exchange so Google's secret never ships in the app.
//!
//! This proves who the person is. It gives no access to their Gmail.

use crate::crypto;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::{LinkKeys, VerifyReply};
use data_encoding::BASE64URL_NOPAD;
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const WAIT_FOR_PERSON: Duration = Duration::from_secs(5 * 60);
const WAIT_FOR_REQUEST: Duration = Duration::from_secs(3);

/// What's left in the browser once signing in ends, in the app's own design.
fn finished_page(outcome: &str, title: &str) -> String {
    let logo = include_str!("../../app/ui/app-icon.svg");
    include_str!("finished_page.html")
        .replace("{{tokens}}", include_str!("../../app/ui/tokens.css"))
        .replace(
            "{{favicon}}",
            &data_encoding::BASE64.encode(logo.as_bytes()),
        )
        .replace("{{logo}}", logo)
        .replace("{{outcome}}", outcome)
        .replace("{{title}}", title)
}

#[derive(Deserialize)]
struct Options {
    google_client_id: Option<String>,
}

#[derive(Deserialize)]
struct GoogleReply {
    #[serde(flatten)]
    session: VerifyReply,
    email: String,
}

pub(crate) fn cancelled() -> CoreError {
    CoreError::Cloud {
        code: "cancelled".into(),
        message: "Sign-in was cancelled.".into(),
    }
}

/// Google's word for whose address was just signed in to, kept for one sign-in.
pub struct MailProof {
    pub email: String,
    pub id_token: String,
    /// The random value the sign-in asked Google to put in the word.
    pub nonce: String,
}

impl Engine {
    /// Which sign-in methods the account server offers right now.
    pub(crate) async fn auth_options(&self) -> Result<Value> {
        let google = self.google_client_id().await.unwrap_or(None).is_some();
        Ok(json!({ "google": google && self.url_opener.is_some() }))
    }

    async fn google_client_id(&self) -> Result<Option<String>> {
        let options: Options = self
            .cloud()?
            .send(Method::GET, "/auth/options", None, None)
            .await?;
        Ok(options.google_client_id)
    }

    pub(crate) async fn google_sign_in(&self) -> Result<Value> {
        // Only one attempt at a time. A newer click replaces an older one.
        self.google_cancel.notify_waiters();
        let client_id = self
            .google_client_id()
            .await?
            .ok_or_else(|| CoreError::Cloud {
                code: "google_off".into(),
                message: "Sign in with Google isn't available yet. Use your email instead.".into(),
            })?;
        let opener = self
            .url_opener
            .as_ref()
            .ok_or_else(|| CoreError::Invalid("This app can't open a browser.".into()))?;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| CoreError::Network(e.to_string()))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let verifier = crypto::random_hex(32);
        let challenge = BASE64URL_NOPAD.encode(&Sha256::digest(verifier.as_bytes()));
        let state = crypto::random_hex(16);
        let nonce = crypto::random_hex(16);
        let url = Url::parse_with_params(
            &self.google_auth_url,
            &[
                ("client_id", client_id.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("response_type", "code"),
                ("scope", "openid email"),
                ("state", state.as_str()),
                ("nonce", nonce.as_str()),
                ("code_challenge", challenge.as_str()),
                ("code_challenge_method", "S256"),
                ("prompt", "select_account"),
            ],
        )
        .map_err(|_| CoreError::Invalid("Could not prepare the Google sign-in.".into()))?;

        // Set up before opening the page, so a very quick cancel isn't missed.
        let cancel = self.google_cancel.notified();
        opener(url.as_str());
        let code = tokio::select! {
            _ = cancel => return Err(cancelled()),
            waited = tokio::time::timeout(WAIT_FOR_PERSON, wait_for_redirect(&listener, &state, "/callback")) => {
                waited.map_err(|_| CoreError::Cloud {
                    code: "google_timeout".into(),
                    message: "Google sign-in took too long. Please try again.".into(),
                })??
            }
        };
        drop(listener);

        let keys = LinkKeys::new();
        let reply: GoogleReply = self
            .cloud()?
            .send(
                Method::POST,
                "/auth/google",
                None,
                Some(&json!({
                    "code": code, "code_verifier": verifier, "redirect_uri": redirect_uri,
                    "nonce": nonce, "device_name": self.device_name,
                    "commitment": keys.commitment(),
                    "returns": self.return_proofs()?,
                })),
            )
            .await?;
        self.begin_session(&reply.email, &reply.session, &keys)
            .await
    }

    /// Keeps Google's word for whose address this is, for one sign-in, in memory only,
    /// with the random value the sign-in asked Google to put in it.
    pub fn note_mail_proof(&self, email: &str, id_token: &str, nonce: &str) {
        *self.mail_proof.lock().unwrap_or_else(|e| e.into_inner()) = Some(MailProof {
            email: email.to_string(),
            id_token: id_token.to_string(),
            nonce: nonce.to_string(),
        });
    }

    /// Signing in to Gmail already proved whose address this is, so no code needs emailing.
    /// Says if that didn't work, which needs no telling off, as the emailed code still does.
    pub(crate) async fn sign_in_with_mail_proof(&self, email: &str) -> Result<Value> {
        let proof = self
            .mail_proof
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let proof = match proof {
            Some(p) if p.email.eq_ignore_ascii_case(email.trim()) => p,
            _ => return Ok(json!({ "signed_in": false })),
        };
        let keys = LinkKeys::new();
        let reply: Result<GoogleReply> = self
            .cloud()?
            .send(
                Method::POST,
                "/auth/google-proof",
                None,
                Some(&json!({
                    // The nonce lets the server check Google's word was made for this
                    // very sign-in, not one taken from somewhere else.
                    "id_token": proof.id_token, "nonce": proof.nonce,
                    "device_name": self.device_name,
                    "commitment": keys.commitment(),
                    "returns": self.return_proofs()?,
                })),
            )
            .await;
        let Ok(reply) = reply else {
            return Ok(json!({ "signed_in": false }));
        };
        let mut session = self
            .begin_session(&reply.email, &reply.session, &keys)
            .await?;
        session["signed_in"] = json!(true);
        Ok(session)
    }
}

/// Waits for the browser to come back. Anything without the right random value
/// is turned away, so another program on this computer can't sneak a code in.
pub(crate) async fn wait_for_redirect(
    listener: &TcpListener,
    state: &str,
    path: &str,
) -> Result<String> {
    loop {
        let (mut sock, _) = listener
            .accept()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let Some(target) = read_request_target(&mut sock).await else {
            continue;
        };
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
            respond(&mut sock, "404 Not Found", "").await;
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        if url.path() != path || param("state").as_deref() != Some(state) {
            respond(&mut sock, "404 Not Found", "").await;
            continue;
        }
        if let Some(code) = param("code") {
            respond(&mut sock, "200 OK", &finished_page("done", "All done")).await;
            return Ok(code);
        }
        respond(
            &mut sock,
            "200 OK",
            &finished_page("stopped", "Sign-in stopped"),
        )
        .await;
        return Err(cancelled());
    }
}

/// The address in the request's first line, for GET requests that arrive promptly.
async fn read_request_target(sock: &mut TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 8192];
    let mut used = 0;
    loop {
        let read = tokio::time::timeout(WAIT_FOR_REQUEST, sock.read(&mut buf[used..]))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return None;
        }
        used += read;
        if let Some(end) = buf[..used].windows(2).position(|w| w == b"\r\n") {
            let line = std::str::from_utf8(&buf[..end]).ok()?;
            let mut parts = line.split_whitespace();
            return match (parts.next(), parts.next()) {
                (Some("GET"), Some(target)) if target.starts_with('/') => Some(target.to_string()),
                _ => None,
            };
        }
        if used == buf.len() {
            return None;
        }
    }
}

async fn respond(sock: &mut TcpStream, status: &str, body: &str) {
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(reply.as_bytes()).await;
    let _ = sock.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finished_page_is_filled_in() {
        let page = finished_page("stopped", "Sign-in stopped");
        assert!(page.contains("<h1>Sign-in stopped</h1>"));
        assert!(page.contains("data-outcome=\"stopped\""));
        assert!(page.contains("--accent:"), "uses the app's own design");
        assert!(!page.contains("{{"), "nothing left to fill in");
    }
}
