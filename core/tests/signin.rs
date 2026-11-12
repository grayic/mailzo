//! Signing in through the browser, against stand-ins for the provider and mail servers.

use fakeimap::FakeServer;
use fakeoauth::FakeOAuth;
use fakesmtp::FakeSmtp;
use mailcore::model::{Security, SmtpSecurity};
use mailcore::providers::Method;
use mailcore::secrets::{account_secret, SecretStore};
use mailcore::signin::{MailSignIn, Servers};
use mailcore::{CoreError, Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::Arc;

const ME: &str = "me@outlook.test";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    smtp: FakeSmtp,
    oauth: FakeOAuth,
    secrets: Arc<MemoryStore>,
}

/// Follows the provider's page the way a browser would.
fn browser() -> mailcore::engine::UrlOpener {
    Arc::new(|url: &str| {
        let url = url.to_string();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("curl")
                .args(["-sL", "-o", "/dev/null", "--max-time", "10", &url])
                .status();
        });
    })
}

pub fn stand_in(oauth: &FakeOAuth, imap: &FakeServer, smtp: &FakeSmtp) -> MailSignIn {
    MailSignIn {
        provider: "microsoft".into(),
        auth_url: format!("{}/authorize", oauth.url),
        token_url: format!("{}/token", oauth.url),
        client_id: fakeoauth::CLIENT_ID.into(),
        client_secret: None,
        scope: "openid email offline_access mail".into(),
        extra_consent: None,
        extra: vec![],
        servers: Some(Servers {
            imap: ("127.0.0.1".into(), imap.addr.port(), Security::Plain),
            smtp: ("127.0.0.1".into(), smtp.addr.port(), SmtpSecurity::Plain),
        }),
    }
}

async fn rig() -> Rig {
    let imap = FakeServer::start("nobody", "no-password-works").await;
    imap.add_folder("INBOX", &[], 1);
    imap.add_folder("Sent", &["\\Sent"], 1);
    let smtp = FakeSmtp::start("nobody", "no-password-works").await;
    let (i, s) = (imap.clone(), smtp.clone());
    let oauth = FakeOAuth::start_on(
        "127.0.0.1:0",
        ME,
        Arc::new(move |token: &str, email: &str| {
            i.state
                .lock()
                .unwrap()
                .oauth_tokens
                .push((token.to_string(), email.to_string()));
            s.allow_token(token, email);
        }),
    )
    .await;
    let secrets = Arc::new(MemoryStore::default());
    let engine = Engine::new(Store::open_in_memory().unwrap(), secrets.clone())
        .with_sent_copy_wait(0)
        .with_url_opener(browser())
        .with_mail_sign_in(stand_in(&oauth, &imap, &smtp));
    Rig {
        engine,
        imap,
        smtp,
        oauth,
        secrets,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Result<Value, CoreError> {
        self.engine.call(cmd, args).await
    }
    async fn sign_in(&self) -> Result<Value, CoreError> {
        self.call(
            "mail_sign_in",
            json!({ "email": ME, "provider": "microsoft" }),
        )
        .await
    }
    async fn sync(&self, id: i64) -> Result<Value, CoreError> {
        self.call("sync_account", json!({ "account_id": id })).await
    }
}

#[tokio::test]
async fn signing_in_connects_with_no_password() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &format!("From: a@x.test\nTo: {ME}\nSubject: Hello\nMessage-ID: <1@x>\n\nHi\n"),
        false,
    );
    let account = r.sign_in().await.unwrap();
    let id = account["id"].as_i64().unwrap();
    assert_eq!(account["email"], ME);
    assert_eq!(account["auth"], "microsoft");

    let asked = r.oauth.last_authorize();
    assert_eq!(
        asked["login_hint"], ME,
        "the page starts on the right account"
    );
    assert_eq!(asked["code_challenge_method"], "S256");
    assert!(!asked.contains_key("client_secret"));

    // What's kept is the lasting token, and never an access token.
    let kept = r.secrets.get(&account_secret(id)).unwrap().unwrap();
    assert!(kept.starts_with("refresh-"), "{kept}");

    // Tokens never reach the screens.
    let shown = r
        .call("list_accounts", json!({}))
        .await
        .unwrap()
        .to_string();
    assert!(
        !shown.contains("refresh-") && !shown.contains("access-"),
        "{shown}"
    );

    let stats = r.sync(id).await.unwrap();
    assert_eq!(stats["errors"], json!([]));
    let folders = r
        .call("list_folders", json!({ "account_id": id }))
        .await
        .unwrap();
    assert_eq!(folders[0]["total"], 1, "{folders}");

    // Sending uses the token too.
    let draft = r
        .call(
            "draft_new",
            json!({ "account_id": id, "mode": "new", "message_id": null, "date_text": null }),
        )
        .await
        .unwrap();
    let draft_id = draft["id"].clone();
    r.call(
        "draft_save",
        json!({ "id": draft_id, "account_id": id, "from_address": "", "to": [{ "name": "", "address": "b@x.test" }],
                "cc": [], "bcc": [], "subject": "Sent with a token", "html": "<p>Hi</p>", "text": "Hi" }),
    )
    .await
    .unwrap();
    r.call("send_draft", json!({ "id": draft_id }))
        .await
        .unwrap();
    let got = r.smtp.received();
    assert_eq!(got.len(), 1);
    assert!(got[0].data.contains("Sent with a token"));
}

#[tokio::test]
async fn access_is_renewed_by_itself_and_a_replaced_token_is_kept() {
    let r = rig().await;
    r.oauth.expire_in(1);
    r.oauth.rotate_refresh(true);
    let account = r.sign_in().await.unwrap();
    let id = account["id"].as_i64().unwrap();
    let first = r.secrets.get(&account_secret(id)).unwrap().unwrap();
    assert_eq!(r.oauth.renewals(), 0);

    r.sync(id).await.unwrap();
    assert_eq!(r.oauth.renewals(), 1, "the token had run out");
    let second = r.secrets.get(&account_secret(id)).unwrap().unwrap();
    assert_ne!(first, second, "the newer lasting token is what's kept");

    r.oauth.expire_in(3600);
    r.sync(id).await.unwrap();
    assert_eq!(r.oauth.renewals(), 2);
    r.sync(id).await.unwrap();
    assert_eq!(
        r.oauth.renewals(),
        2,
        "a good token isn't renewed for nothing"
    );
}

#[tokio::test]
async fn when_access_is_removed_it_asks_to_sign_in_again_and_keeps_the_mail() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &format!("From: a@x.test\nTo: {ME}\nSubject: Hello\nMessage-ID: <1@x>\n\nHi\n"),
        false,
    );
    r.oauth.expire_in(1);
    let id = r.sign_in().await.unwrap()["id"].as_i64().unwrap();
    r.sync(id).await.unwrap();

    r.oauth.revoke_all();
    let failed = r.sync(id).await.unwrap_err();
    assert_eq!(failed.code(), "needs_signin");
    assert!(failed.to_string().contains(ME));

    let again = r.sign_in().await.unwrap();
    assert_eq!(again["id"], id, "the same inbox, not a second copy");
    let accounts = r.call("list_accounts", json!({})).await.unwrap();
    assert_eq!(accounts.as_array().unwrap().len(), 1);
    assert_eq!(
        r.call("list_folders", json!({ "account_id": id }))
            .await
            .unwrap()[0]["total"],
        1
    );
    r.sync(id).await.unwrap();
}

#[tokio::test]
async fn a_token_the_server_turns_down_is_renewed_at_once_with_no_sign_in_asked() {
    let r = rig().await;
    let id = r.sign_in().await.unwrap()["id"].as_i64().unwrap();
    r.imap.state.lock().unwrap().oauth_tokens.clear();
    // It asks the provider for a new one and tries again, so nobody is asked to sign in.
    r.sync(id).await.unwrap();
    assert_eq!(r.oauth.renewals(), 1);
}

#[tokio::test]
async fn a_busy_mail_server_is_not_a_sign_in_problem() {
    let r = rig().await;
    let id = r.sign_in().await.unwrap()["id"].as_i64().unwrap();
    // What Gmail says while it still counts the connections of an app that just quit.
    r.imap.state.lock().unwrap().login_refusal =
        Some("[ALERT] Too many simultaneous connections. (Failure)".into());
    let failed = r.sync(id).await.unwrap_err();
    assert_eq!(failed.code(), "network", "{failed}");
    r.imap.state.lock().unwrap().login_refusal = None;
    r.sync(id).await.unwrap();
}

#[tokio::test]
async fn stopping_at_the_provider_saves_nothing() {
    let r = rig().await;
    r.oauth.refuse_at_consent(true);
    let refused = r.sign_in().await.unwrap_err();
    assert_eq!(refused.code(), "cancelled");
    assert_eq!(r.call("list_accounts", json!({})).await.unwrap(), json!([]));

    r.oauth.refuse_at_consent(false);
    r.oauth.give_no_refresh(true);
    let none = r.sign_in().await.unwrap_err();
    assert!(none.to_string().contains("lasting access"), "{none}");
    assert_eq!(r.call("list_accounts", json!({})).await.unwrap(), json!([]));
}

#[tokio::test]
async fn picking_a_different_account_on_the_page_uses_that_account() {
    let r = rig().await;
    r.oauth.set_email("other@outlook.test");
    let account = r.sign_in().await.unwrap();
    assert_eq!(
        account["email"], "other@outlook.test",
        "mail never shows under the wrong address"
    );
    assert_eq!(account["username"], "other@outlook.test");
}

#[tokio::test]
async fn signing_in_needs_no_address_typed_first() {
    let r = rig().await;
    r.oauth.set_email("picked@outlook.test");
    let account = r
        .call(
            "mail_sign_in",
            json!({ "email": "", "provider": "microsoft" }),
        )
        .await
        .unwrap();
    assert_eq!(account["email"], "picked@outlook.test");
}

#[tokio::test]
async fn an_address_that_isnt_one_is_still_refused() {
    let r = rig().await;
    let wrong = r
        .call(
            "mail_sign_in",
            json!({ "email": "not an address", "provider": "microsoft" }),
        )
        .await;
    assert_eq!(wrong.unwrap_err().code(), "invalid");
}

#[tokio::test]
async fn signing_in_can_be_called_off() {
    let imap = FakeServer::start("n", "n").await;
    let smtp = FakeSmtp::start("n", "n").await;
    let oauth = FakeOAuth::start_on("127.0.0.1:0", ME, Arc::new(|_: &str, _: &str| {})).await;
    // This browser never comes back.
    let engine = Arc::new(
        Engine::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryStore::default()),
        )
        .with_url_opener(Arc::new(|_: &str| {}))
        .with_mail_sign_in(stand_in(&oauth, &imap, &smtp)),
    );
    let waiting = engine.clone();
    let task = tokio::spawn(async move {
        waiting
            .call(
                "mail_sign_in",
                json!({ "email": ME, "provider": "microsoft" }),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    engine.call("mail_sign_in_cancel", json!({})).await.unwrap();
    assert_eq!(task.await.unwrap().unwrap_err().code(), "cancelled");
}

#[tokio::test]
async fn the_connect_screen_offers_the_browser_only_when_it_can() {
    let plain = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    );
    let outlook = plain.find_provider("me@hotmail.com").await;
    assert_eq!(outlook.method, Method::BrowserSoon);
    assert_eq!(outlook.browser, None);

    let google = MailSignIn::google("id", "secret");
    let ready = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_url_opener(browser())
    .with_mail_sign_in(MailSignIn::microsoft("id"))
    .with_mail_sign_in(google);
    let outlook = ready.find_provider("me@hotmail.com").await;
    assert_eq!(outlook.method, Method::Browser);
    assert_eq!(outlook.browser.as_deref(), Some("microsoft"));
    assert_eq!(outlook.note, None);
    assert_eq!(
        outlook.smtp_host, "smtp-mail.outlook.com",
        "personal accounts' own server"
    );

    let gmail = ready.find_provider("me@gmail.com").await;
    assert_eq!(gmail.method, Method::AppPassword, "still an option");
    assert_eq!(gmail.browser.as_deref(), Some("google"));

    assert_eq!(ready.find_provider("me@icloud.com").await.browser, None);
}

#[test]
fn real_providers_ask_for_the_right_things() {
    let m = MailSignIn::microsoft("id");
    assert!(
        m.scope.contains("IMAP.AccessAsUser.All")
            && m.scope.contains("SMTP.Send")
            && m.scope.contains("offline_access")
    );
    assert_eq!(m.client_secret, None, "Microsoft needs no secret");
    assert!(m.auth_url.starts_with("https://") && m.token_url.starts_with("https://"));
    let g = MailSignIn::google("id", "s");
    assert!(g.scope.contains("https://mail.google.com/"));
    assert!(g
        .extra
        .iter()
        .any(|(k, v)| k == "access_type" && v == "offline"));
}

#[tokio::test]
async fn a_google_sign_in_proves_itself_with_the_nonce_it_asked_for() {
    use axum::{routing::post, Json, Router};
    use std::sync::Mutex;
    let imap = FakeServer::start("nobody", "no-password-works").await;
    imap.add_folder("INBOX", &[], 1);
    let smtp = FakeSmtp::start("nobody", "no-password-works").await;
    let (i, s) = (imap.clone(), smtp.clone());
    let oauth = FakeOAuth::start_on(
        "127.0.0.1:0",
        ME,
        Arc::new(move |token: &str, email: &str| {
            i.state
                .lock()
                .unwrap()
                .oauth_tokens
                .push((token.to_string(), email.to_string()));
            s.allow_token(token, email);
        }),
    )
    .await;

    // The account server, noting what it was sent.
    let proofs: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = proofs.clone();
    let cloud = Router::new().route(
        "/auth/google-proof",
        post(move |Json(body): Json<Value>| {
            seen.lock().unwrap().push(body);
            async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "no" })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cloud_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, cloud).await;
    });

    // Follows the page like a browser, keeping the address it was opened at.
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = opened.clone();
    let follow = browser();
    let mut google = stand_in(&oauth, &imap, &smtp);
    google.provider = "google".into();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_cloud(&cloud_url, "Test Mac")
    .unwrap()
    .with_url_opener(Arc::new(move |url: &str| {
        log.lock().unwrap().push(url.to_string());
        follow(url);
    }))
    .with_mail_sign_in(google);

    engine
        .call("mail_sign_in", json!({ "email": ME, "provider": "google" }))
        .await
        .unwrap();
    let page = reqwest::Url::parse(&opened.lock().unwrap()[0]).unwrap();
    let nonce = page
        .query_pairs()
        .find(|(k, _)| k == "nonce")
        .map(|(_, v)| v.to_string())
        .expect("the sign-in asks Google for a nonce");
    assert_eq!(nonce.len(), 32);

    let reply = engine
        .call("auth_with_mail_sign_in", json!({ "email": ME }))
        .await
        .unwrap();
    assert_eq!(reply["signed_in"], false);
    let sent = proofs.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["nonce"], nonce);
    assert!(sent[0]["id_token"].as_str().is_some());
}
