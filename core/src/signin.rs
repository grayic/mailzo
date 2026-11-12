//! Browser sign-in for mail: Microsoft and Google refuse or discourage passwords.
//!
//! Everything here happens on the device. The person signs in on the provider's own
//! page in their real browser, and this app never sees their password. What it keeps
//! is a long-lived refresh token, in the same place as passwords, and short-lived
//! access tokens in memory. Neither passes through our server in usable form: the
//! refresh token only travels inside the encrypted vault.

use crate::crypto;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::google::{cancelled, wait_for_redirect};
use crate::identity::unix_now;
use crate::imap::ImapConnection;
use crate::model::{Account, AccountPrefs, Security, SmtpSecurity};
use crate::secrets::account_secret;
use data_encoding::{BASE64URL_NOPAD, BASE64_NOPAD};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::net::TcpListener;

const WAIT_FOR_PERSON: Duration = Duration::from_secs(5 * 60);
/// An access token this close to running out counts as already gone.
const EARLY_RENEWAL_SECONDS: i64 = 60;

pub const PASSWORD: &str = "password";

/// Fixed servers, for stand-ins. Real providers are worked out from the address.
#[derive(Clone)]
pub struct Servers {
    pub imap: (String, u16, Security),
    pub smtp: (String, u16, SmtpSecurity),
}

/// How to sign in with one provider.
#[derive(Clone)]
pub struct MailSignIn {
    /// `microsoft` or `google`, matching the provider list.
    pub provider: String,
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    /// Google's desktop apps have one, and Google says it isn't treated as a secret.
    pub client_secret: Option<String>,
    pub scope: String,
    /// Permission to ask for on the sign-in page as well, for a different service. A
    /// token only ever covers one service, so this stays out of the token requests.
    pub extra_consent: Option<String>,
    pub extra: Vec<(String, String)>,
    pub servers: Option<Servers>,
}

impl MailSignIn {
    pub fn microsoft(client_id: &str) -> MailSignIn {
        MailSignIn {
            provider: "microsoft".into(),
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            client_id: client_id.into(),
            client_secret: None,
            scope: "openid email offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send".into(),
            extra_consent: Some(
                "https://graph.microsoft.com/MailboxSettings.ReadWrite https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Mail.Send"
                    .into(),
            ),
            extra: vec![("prompt".into(), "select_account".into())],
            // A token to someone's whole mailbox only ever goes to the provider's own servers.
            servers: Some(Servers {
                imap: ("outlook.office365.com".into(), 993, Security::Tls),
                smtp: ("smtp.office365.com".into(), 587, SmtpSecurity::StartTls),
            }),
        }
    }

    pub fn google(client_id: &str, client_secret: &str) -> MailSignIn {
        MailSignIn {
            provider: "google".into(),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            client_id: client_id.into(),
            client_secret: Some(client_secret.into()),
            scope: "openid email https://mail.google.com/ https://www.googleapis.com/auth/gmail.settings.basic https://www.googleapis.com/auth/calendar.events".into(),
            extra_consent: None,
            // Without these, Google gives no lasting access the second time round.
            extra: vec![
                ("access_type".into(), "offline".into()),
                ("prompt".into(), "consent".into()),
            ],
            servers: Some(Servers {
                imap: ("imap.gmail.com".into(), 993, Security::Tls),
                smtp: ("smtp.gmail.com".into(), 465, SmtpSecurity::Tls),
            }),
        }
    }
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    id_token: Option<String>,
}

/// The address inside an ID token. It came straight from the provider over a secure
/// connection, so the signature adds nothing here.
fn signed_in_address(id_token: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Claims {
        email: Option<String>,
        preferred_username: Option<String>,
    }
    let payload = id_token.split('.').nth(1)?;
    let bytes = BASE64URL_NOPAD
        .decode(payload.trim_end_matches('=').as_bytes())
        .or_else(|_| BASE64_NOPAD.decode(payload.trim_end_matches('=').as_bytes()))
        .ok()?;
    let claims: Claims = serde_json::from_slice(&bytes).ok()?;
    claims
        .email
        .or(claims.preferred_username)
        .filter(|a| a.contains('@'))
}

fn signin_failed(what: &str) -> CoreError {
    CoreError::Invalid(format!("{what} Please try again."))
}

impl Engine {
    pub(crate) fn sign_in_for_provider(&self, provider: &str) -> Result<MailSignIn> {
        self.sign_in_for(provider).cloned()
    }

    /// A token for another of the provider's services, using the same lasting access.
    pub(crate) async fn tokens_for_scope(
        &self,
        s: &MailSignIn,
        refresh: &str,
        scope: &str,
    ) -> std::result::Result<reqwest::Response, CoreError> {
        self.ask_for_tokens(
            s,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("scope", scope),
            ],
        )
        .await
    }

    fn sign_in_for(&self, provider: &str) -> Result<&MailSignIn> {
        self.mail_signins
            .iter()
            .find(|s| s.provider == provider)
            .ok_or_else(|| {
                CoreError::Invalid("Signing in with this provider isn't set up yet.".into())
            })
    }

    pub(crate) fn can_sign_in_with(&self, provider: &str) -> bool {
        self.url_opener.is_some() && self.mail_signins.iter().any(|s| s.provider == provider)
    }

    async fn ask_for_tokens_with(
        &self,
        s: &MailSignIn,
        scope: Option<&str>,
        form: &[(&str, &str)],
    ) -> std::result::Result<reqwest::Response, CoreError> {
        let mut with_scope: Vec<(&str, &str)> = form.to_vec();
        if let Some(scope) = scope {
            with_scope.push(("scope", scope));
        }
        self.ask_for_tokens(s, &with_scope).await
    }

    async fn ask_for_tokens(
        &self,
        s: &MailSignIn,
        form: &[(&str, &str)],
    ) -> std::result::Result<reqwest::Response, CoreError> {
        let mut form: Vec<(&str, &str)> = form.to_vec();
        form.push(("client_id", &s.client_id));
        if let Some(secret) = &s.client_secret {
            form.push(("client_secret", secret));
        }
        self.ai_http
            .post(&s.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))
    }

    /// Opens the provider's page, waits for the person, connects, and saves the inbox.
    /// Signing in again on an inbox already here just renews its access.
    pub(crate) async fn mail_sign_in(&self, email: &str, provider: &str) -> Result<Account> {
        let email = email.trim();
        // Left empty, the person picks their account on the provider's own page.
        if !email.is_empty() && !email.contains('@') {
            return Err(CoreError::Invalid("Enter a valid email address.".into()));
        }
        // Only one attempt at a time. A newer click replaces an older one.
        self.google_cancel.notify_waiters();
        let config = self.sign_in_for(provider)?.clone();
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
        let redirect_uri = format!("http://localhost:{port}");
        let verifier = crypto::random_hex(32);
        let challenge = BASE64URL_NOPAD.encode(&Sha256::digest(verifier.as_bytes()));
        let state = crypto::random_hex(16);
        // Google puts this in its word for whose address it is, so the account server
        // can tell that word was made for this very sign-in.
        let nonce = crypto::random_hex(16);
        let mut params: Vec<(String, String)> = vec![
            ("client_id".into(), config.client_id.clone()),
            ("redirect_uri".into(), redirect_uri.clone()),
            ("response_type".into(), "code".into()),
            (
                "scope".into(),
                match &config.extra_consent {
                    Some(more) => format!("{} {more}", config.scope),
                    None => config.scope.clone(),
                },
            ),
            ("state".into(), state.clone()),
            ("code_challenge".into(), challenge),
            ("code_challenge_method".into(), "S256".into()),
        ];
        if !email.is_empty() {
            params.push(("login_hint".into(), email.to_string()));
        }
        if provider == "google" {
            params.push(("nonce".into(), nonce.clone()));
        }
        params.extend(config.extra.iter().cloned());
        let url = Url::parse_with_params(&config.auth_url, &params)
            .map_err(|_| signin_failed("Could not prepare the sign-in."))?;

        // Set up before opening the page, so a very quick cancel isn't missed.
        let cancel = self.google_cancel.notified();
        opener(url.as_str());
        let code = tokio::select! {
            _ = cancel => return Err(cancelled()),
            waited = tokio::time::timeout(WAIT_FOR_PERSON, wait_for_redirect(&listener, &state, "/")) => {
                waited.map_err(|_| CoreError::Cloud {
                    code: "signin_timeout".into(),
                    message: "Signing in took too long. Please try again.".into(),
                })??
            }
        };
        drop(listener);

        // Microsoft has to be told which service this token is for, as permission was asked
        // for more than one. Google doesn't want to be asked.
        let token_scope = config.scope.clone();
        let reply = self
            .ask_for_tokens_with(
                &config,
                config
                    .extra_consent
                    .is_some()
                    .then_some(token_scope.as_str()),
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("redirect_uri", &redirect_uri),
                    ("code_verifier", &verifier),
                ],
            )
            .await?;
        if !reply.status().is_success() {
            return Err(signin_failed("The sign-in didn't finish."));
        }
        let tokens: TokenReply = reply
            .json()
            .await
            .map_err(|_| signin_failed("The sign-in didn't finish."))?;
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or_else(|| signin_failed("The provider didn't give lasting access."))?;

        // They may have picked a different account on the provider's page. Mail
        // showing up under the wrong address would be far worse than a surprise.
        let signed_in = tokens.id_token.as_deref().and_then(signed_in_address);
        let address = signed_in
            .clone()
            .or_else(|| (!email.is_empty()).then(|| email.to_string()))
            .ok_or_else(|| signin_failed("The provider didn't say which account that was."))?;
        let found = self.find_provider(&address).await;
        let (imap, smtp) = match &config.servers {
            Some(fixed) => (fixed.imap.clone(), fixed.smtp.clone()),
            None => (
                (found.imap_host.clone(), found.imap_port, Security::Tls),
                (
                    found.smtp_host.clone(),
                    found.smtp_port,
                    found.smtp_security,
                ),
            ),
        };
        let probe = Account {
            id: 0,
            sync_id: String::new(),
            email: address.clone(),
            display_name: address.clone(),
            imap_host: imap.0.clone(),
            imap_port: imap.1,
            security: imap.2,
            username: address.clone(),
            smtp_host: smtp.0.clone(),
            smtp_port: smtp.1,
            smtp_security: smtp.2,
            signature: String::new(),
            settings_updated_at: 0,
            prefs: AccountPrefs::default(),
            auth: provider.to_string(),
        };
        ImapConnection::connect(&probe, &tokens.access_token, self.reach())
            .await?
            .logout()
            .await;

        let existing = self
            .store
            .list_accounts()?
            .into_iter()
            .find(|a| a.email.eq_ignore_ascii_case(&address));
        let was_here = existing.is_some();
        let account = match existing {
            Some(a) => a,
            None => self
                .store
                .insert_account(&address, &address, &imap.0, imap.1, imap.2, &address)?,
        };
        let mut next = account;
        next.auth = provider.to_string();
        next.imap_host = imap.0;
        next.imap_port = imap.1;
        next.security = imap.2;
        next.username = address;
        next.smtp_host = smtp.0;
        next.smtp_port = smtp.1;
        next.smtp_security = smtp.2;
        // Newest wins, so other devices pick up this access too.
        next.settings_updated_at = unix_now();
        self.store.save_account(&next)?;
        if let Err(e) = self.secrets.set(&account_secret(next.id), &refresh) {
            // Only one made just now. An inbox already here keeps its mail and drafts.
            if !was_here {
                self.store.delete_account(next.id)?;
            }
            return Err(e);
        }
        self.remember_token(next.id, &tokens.access_token, tokens.expires_in)
            .await;
        if let (true, None, Some(id_token)) = (
            provider == "google",
            self.store.identity()?,
            &tokens.id_token,
        ) {
            self.note_mail_proof(&next.email, id_token, &nonce);
        }
        // Telling other devices can wait if this one is offline.
        let _ = self.vault_sync().await;
        self.store.account(next.id)
    }

    async fn remember_token(&self, account_id: i64, token: &str, expires_in: Option<i64>) {
        let expires = unix_now() + expires_in.unwrap_or(3600);
        self.mail_tokens
            .lock()
            .await
            .insert(account_id, (token.to_string(), expires));
    }

    pub(crate) async fn forget_token(&self, account_id: i64) {
        self.mail_tokens.lock().await.remove(&account_id);
    }

    /// A working access token, renewing it if it's about to run out.
    pub(crate) async fn access_token(&self, account: &Account, refresh: &str) -> Result<String> {
        // Held across the renewal, so two refreshes don't both renew at once.
        let mut tokens = self.mail_tokens.lock().await;
        if let Some((token, expires)) = tokens.get(&account.id) {
            if *expires > unix_now() + EARLY_RENEWAL_SECONDS {
                return Ok(token.clone());
            }
        }
        let config = self.sign_in_for(&account.auth).map_err(|_| {
            eprintln!(
                "{}: no {} sign-in in this build",
                account.email, account.auth
            );
            CoreError::NeedsSignIn(account.email.clone())
        })?;
        let reply = self
            .ask_for_tokens_with(
                config,
                config
                    .extra_consent
                    .is_some()
                    .then_some(config.scope.as_str()),
                &[("grant_type", "refresh_token"), ("refresh_token", refresh)],
            )
            .await?;
        let status = reply.status().as_u16();
        if status == 400 || status == 401 {
            // Revoked, expired or replaced. Only the person can fix that. Google's reason (never
            // the token) goes in the log, so "sign in again" can be told apart from a setup fault.
            let why = reply.text().await.unwrap_or_default();
            let why: serde_json::Value = serde_json::from_str(&why).unwrap_or_default();
            eprintln!(
                "{}: sign-in renewal refused ({status}): {} {}",
                account.email,
                why["error"].as_str().unwrap_or("?"),
                why["error_description"].as_str().unwrap_or("")
            );
            return Err(CoreError::NeedsSignIn(account.email.clone()));
        }
        if !reply.status().is_success() {
            return Err(CoreError::Network(format!(
                "the sign-in service answered {status}"
            )));
        }
        let fresh: TokenReply = reply
            .json()
            .await
            .map_err(|_| CoreError::Network("the sign-in service gave a bad answer".into()))?;
        if let Some(next) = &fresh.refresh_token {
            if next != refresh {
                self.secrets.set(&account_secret(account.id), next)?;
            }
        }
        let expires = unix_now() + fresh.expires_in.unwrap_or(3600);
        tokens.insert(account.id, (fresh.access_token.clone(), expires));
        Ok(fresh.access_token)
    }

    /// Which sign-in providers are set up in this build, for the screens.
    pub(crate) fn sign_in_providers(&self) -> BTreeMap<String, bool> {
        self.mail_signins
            .iter()
            .map(|s| (s.provider.clone(), self.can_sign_in_with(&s.provider)))
            .collect()
    }
}
