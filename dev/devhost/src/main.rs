//! Development only. Serves the real engine over HTTP on this computer, using a
//! demo mailbox and throwaway storage. Nothing here ships in the Mac app.
//!
//! On its own it starts already signed in, so the screens open straight to the inbox.
//! To try signing in, devices and teams, run Mailzo's account server for development too
//! (it lives in the private dakly-cloud project) and start this with
//! `DAKLY_ACCOUNT_URL=http://127.0.0.1:4791`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{any, post};
use axum::{Json, Router};
use fakegoogle::FakeGoogle;
use fakeimap::{sample, FakeServer, DEMO_PASS, DEMO_USER};
use fakesmtp::FakeSmtp;
use mailcore::{Engine, MemoryStore, Store};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

/// `INBOX_DEVHOST_SHIFT=100` moves every port up by 100, so a second copy can run beside
/// one that's already going.
fn port(base: u16) -> u16 {
    base + std::env::var("INBOX_DEVHOST_SHIFT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn shifted(base: u16) -> String {
    format!("127.0.0.1:{}", port(base))
}

static HTTP_ADDR: LazyLock<String> = LazyLock::new(|| shifted(4790));
static IMAP_ADDR: LazyLock<String> = LazyLock::new(|| shifted(1143));
static GOOGLE_ADDR: LazyLock<String> = LazyLock::new(|| shifted(4792));
static SMTP_ADDR: LazyLock<String> = LazyLock::new(|| shifted(1025));
static AI_ADDR: LazyLock<String> = LazyLock::new(|| shifted(4793));
static OAUTH_ADDR: LazyLock<String> = LazyLock::new(|| shifted(4794));
/// A small blue square, standing in for one company's logo.
const DEMO_LOGO_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAMElEQVR4nO3OIQEAAAgDMFKRg/6KJhDjZmJ+1bOXVAICAgICAgICAgICAgICAunAAwBolIiJsR/NAAAAAElFTkSuQmCC";

struct App {
    oauth: fakeoauth::FakeOAuth,
    engine: Engine,
    /// A second person, so team features can be tried with someone to work with. Only
    /// with an account server to work through.
    teammate: Option<Engine>,
    /// Where the account server for development keeps the emails it would have sent.
    account_dev: Option<String>,
    sign_in_mails: AtomicUsize,
    server: FakeServer,
    new_mail: AtomicUsize,
    pixel_hits: AtomicUsize,
    outgoing: FakeSmtp,
}

/// Everything the demo's outgoing server has received, newest last.
async fn outbox(State(app): State<Arc<App>>) -> Json<Value> {
    let list: Vec<Value> = app
        .outgoing
        .received()
        .iter()
        .map(|m| json!({ "from": m.from, "to": m.to, "data": m.data }))
        .collect();
    Json(json!(list))
}

/// The demo's stand-in for the logo service. Only one company has a logo.
async fn demo_logo(
    axum::extract::Path(file): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if file == "northwind.test.ico" {
        let bytes = data_encoding::BASE64
            .decode(DEMO_LOGO_BASE64.as_bytes())
            .unwrap_or_default();
        return bytes.into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// The demo's stand-in for the public settings directory.
async fn demo_directory(
    axum::extract::Path(domain): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if domain != "listed.test" {
        return StatusCode::NOT_FOUND.into_response();
    }
    r#"<clientConfig><emailProvider><displayName>Listed Mail</displayName>
      <incomingServer type="imap"><hostname>imap.listed.test</hostname><port>993</port>
        <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
        <authentication>password-cleartext</authentication></incomingServer>
      <outgoingServer type="smtp"><hostname>smtp.listed.test</hostname><port>465</port>
        <socketType>SSL</socketType><username>%EMAILADDRESS%</username>
        <authentication>password-cleartext</authentication></outgoingServer>
    </emailProvider></clientConfig>"#
        .into_response()
}

#[derive(Deserialize)]
struct CodeQuery {
    email: String,
}

/// The newest sign-in code the account server for development "sent" to this address.
async fn last_code(State(app): State<Arc<App>>, Query(q): Query<CodeQuery>) -> Json<Value> {
    let Some(base) = &app.account_dev else {
        return Json(json!({ "code": null }));
    };
    let url = format!("{base}/dev/last-code?email={}", q.email);
    let code = tokio::task::spawn_blocking(move || {
        std::process::Command::new("curl")
            .args(["-s", "--max-time", "5", &url])
            .output()
            .ok()
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .map(|v| v["code"].clone())
            .unwrap_or(Value::Null)
    })
    .await
    .unwrap_or(Value::Null);
    Json(json!({ "code": code }))
}

#[derive(Deserialize)]
struct Delivery {
    to: String,
    subject: String,
    text: String,
}

/// The account server for development hands its emails here, and ones to the demo address
/// land in the demo inbox, as a real sign-in code would.
async fn deliver(State(app): State<Arc<App>>, Json(mail): Json<Delivery>) -> StatusCode {
    if mail.to != DEMO_USER {
        return StatusCode::NO_CONTENT;
    }
    let n = app.sign_in_mails.fetch_add(1, Ordering::SeqCst);
    app.server.add_message_dated(
        "INBOX",
        &format!(
            "From: Mailzo <no-reply@example.com>\nTo: {}\nSubject: {}\n\
             Date: Mon, 01 Nov 2036 10:00:00 +0000\nMessage-ID: <account-{n}@inbox.test>\n\
             Content-Type: text/plain; charset=utf-8\n\n{}\n",
            mail.to,
            mail.subject.replace(['\r', '\n'], " "),
            mail.text
        ),
        false,
        "01-Nov-2036 10:00:00 +0000",
    );
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct Call {
    cmd: String,
    #[serde(default)]
    args: Value,
}

async fn call(
    State(app): State<Arc<App>>,
    Json(req): Json<Call>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match app.engine.call(&req.cmd, req.args).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::to_value(&e).unwrap_or(json!({}))),
        )),
    }
}

#[derive(Deserialize)]
struct AttachmentQuery {
    message_id: i64,
    part: u32,
}

/// The same download the Cloud host offers, so the web screens can be tried here.
async fn attachment(
    State(app): State<Arc<App>>,
    Query(q): Query<AttachmentQuery>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;
    let (name, _, bytes) = app
        .engine
        .attachment(q.message_id, q.part)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
struct MessageQuery {
    message_id: i64,
}

async fn original(
    State(app): State<Arc<App>>,
    Query(q): Query<MessageQuery>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;
    let (name, bytes) = app
        .engine
        .original(q.message_id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn teammate_call(
    State(app): State<Arc<App>>,
    Json(req): Json<Call>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(teammate) = &app.teammate else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "code": "no_account_server",
                "message": "Teams need the account server for development. See the top of dev/devhost/src/main.rs.",
            })),
        ));
    };
    match teammate.call(&req.cmd, req.args).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::to_value(&e).unwrap_or(json!({}))),
        )),
    }
}

/// As if the person removed the app's access in their account.
async fn revoke_oauth(State(app): State<Arc<App>>) -> StatusCode {
    app.oauth.revoke_all();
    app.server.state.lock().unwrap().oauth_tokens.clear();
    StatusCode::NO_CONTENT
}

/// A slow mail server: read and unread changes are answered only after this many seconds. 0 is quick again.
async fn slow_store(
    State(app): State<Arc<App>>,
    axum::extract::Path(secs): axum::extract::Path<u64>,
) -> StatusCode {
    app.server.state.lock().unwrap().store_wait =
        (secs > 0).then(|| std::time::Duration::from_secs(secs));
    StatusCode::NO_CONTENT
}

/// Drops a new message into the demo inbox.
async fn new_mail(State(app): State<Arc<App>>) -> Json<Value> {
    let n = app.new_mail.fetch_add(1, Ordering::SeqCst) + 1;
    let uid = app.server.add_message_dated(
        "INBOX",
        &format!(
            "From: Priya Nair <priya@harbor.test>\nTo: Demo User <demo@inbox.test>\n\
             Subject: New mail number {n}\nDate: Mon, 02 Nov 2026 09:00:00 +0000\n\
             Message-ID: <new-{n}@harbor.test>\nContent-Type: text/plain; charset=utf-8\n\n\
             This message arrived after the first sync.\n"
        ),
        false,
        "02-Nov-2026 09:00:00 +0000",
    );
    Json(json!({ "uid": uid }))
}

/// A customer writes to the demo inbox, for team tests.
async fn team_mail(State(app): State<Arc<App>>) -> Json<Value> {
    let uid = app.server.add_message_dated(
        "INBOX",
        "From: Customer <customer@harbor.test>\nTo: Demo User <demo@inbox.test>\n\
         Subject: Team question\nDate: Thu, 01 Jan 2037 09:00:00 +0000\n\
         Message-ID: <team-question@harbor.test>\nContent-Type: text/plain; charset=utf-8\n\n\
         Can someone help me?\n",
        false,
        "01-Jan-2037 09:00:00 +0000",
    );
    Json(json!({ "uid": uid }))
}

/// Someone who has never written before.
async fn stranger_mail(State(app): State<Arc<App>>) -> StatusCode {
    app.server.add_message_dated(
        "INBOX",
        "From: New Person <newperson@elsewhere.test>\nTo: Demo User <demo@inbox.test>\n\
         Subject: A first hello\nDate: Mon, 01 Nov 2036 09:00:00 +0000\n\
         Message-ID: <stranger-1@elsewhere.test>\nContent-Type: text/plain; charset=utf-8\n\n\
         Hello there.\n",
        false,
        "01-Nov-2036 09:00:00 +0000",
    );
    StatusCode::NO_CONTENT
}

/// One sender for each way of leaving a list, for the Newsletters view.
async fn newsletter_mail(State(app): State<Arc<App>>) -> StatusCode {
    const ONE_CLICK: &str = "List-Unsubscribe-Post: List-Unsubscribe=One-Click\n";
    let senders = [
        (
            "Daily Brief",
            "news@dailybrief.test",
            "https://dailybrief.test/out",
            ONE_CLICK,
            3,
        ),
        (
            "Market Wrap",
            "wrap@marketwrap.test",
            "https://marketwrap.test/out",
            ONE_CLICK,
            2,
        ),
        (
            "Sunday Reads",
            "editor@sundayreads.test",
            "https://sundayreads.test/out",
            ONE_CLICK,
            1,
        ),
        (
            "Field Notes",
            "hello@fieldnotes.test",
            "mailto:leave@fieldnotes.test?subject=Leave",
            "",
            2,
        ),
        (
            "Shop Deals",
            "deals@shopdeals.test",
            "https://shopdeals.test/out",
            "",
            1,
        ),
    ];
    for (name, address, link, extra, issues) in senders {
        for issue in 1..=issues {
            let id = format!("{issue}-{address}");
            let day = format!("{:02}", issue + 1);
            // Not the inbox, where newer rows would push other tests' mail out of sight.
            app.server.add_message_dated(
                "Archive",
                &format!(
                    "From: {name} <{address}>\nTo: Demo User <demo@inbox.test>\n\
                     Subject: {name}, issue {issue}\nDate: {day} Nov 2026 09:00:00 +0000\n\
                     Message-ID: <{id}>\nList-Unsubscribe: <{link}>\n{extra}\
                     Content-Type: text/plain; charset=utf-8\n\nThis issue is number {issue}.\n"
                ),
                true,
                &format!("{day}-Nov-2026 09:00:00 +0000"),
            );
        }
    }
    StatusCode::NO_CONTENT
}

/// A sign-in code email, filed in Archive so it doesn't push the inbox samples out of sight,
/// and the newest there, so it's at the top of that list where the test can see it.
async fn code_mail(State(app): State<Arc<App>>) -> StatusCode {
    app.server.add_message_dated(
        "Archive",
        "From: Acme <login@acme.test>\nTo: Demo User <demo@inbox.test>\n\
         Subject: Your Acme sign-in code\nDate: 08 Nov 2026 09:00:00 +0000\n\
         Message-ID: <code-1@acme.test>\nContent-Type: text/plain; charset=utf-8\n\n\
         Your verification code is 482913. It expires in 10 minutes.\n",
        true,
        "08-Nov-2026 09:00:00 +0000",
    );
    StatusCode::NO_CONTENT
}

/// The demo newsletter points at this URL. The reader must never load it.
async fn tracking_pixel(State(app): State<Arc<App>>) -> StatusCode {
    app.pixel_hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn pixel_hits(State(app): State<Arc<App>>) -> Json<Value> {
    Json(json!({ "hits": app.pixel_hits.load(Ordering::SeqCst) }))
}

#[tokio::main]
async fn main() {
    let server = FakeServer::start_on(&IMAP_ADDR, DEMO_USER, DEMO_PASS).await;
    sample::seed(&server);

    let outgoing = FakeSmtp::start_on(&SMTP_ADDR, DEMO_USER, DEMO_PASS).await;
    outgoing.deliver_to(DEMO_USER, server.clone());

    // Started here either way, so an account server for development can check Google
    // sign-ins against it.
    let google = FakeGoogle::start_on(&GOOGLE_ADDR, DEMO_USER).await;
    let account_url = std::env::var("DAKLY_ACCOUNT_URL").ok();
    let account_dev = account_url.as_ref().map(|_| {
        std::env::var("DAKLY_ACCOUNT_DEV_URL").unwrap_or_else(|_| "http://127.0.0.1:4795".into())
    });

    // The demo's stand-in for Microsoft's and Google's sign-in pages.
    let (imap_tokens, smtp_tokens) = (server.clone(), outgoing.clone());
    let oauth = fakeoauth::FakeOAuth::start_on(
        &OAUTH_ADDR,
        "signed@inbox.test",
        Arc::new(move |token: &str, email: &str| {
            imap_tokens
                .state
                .lock()
                .unwrap()
                .oauth_tokens
                .push((token.to_string(), email.to_string()));
            smtp_tokens.allow_token(token, email);
        }),
    )
    .await;
    let mail_sign_in = |provider: &str| mailcore::signin::MailSignIn {
        provider: provider.into(),
        auth_url: format!("{}/authorize", oauth.url),
        token_url: format!("{}/token", oauth.url),
        client_id: fakeoauth::CLIENT_ID.into(),
        client_secret: None,
        scope: "mail".into(),
        extra_consent: None,
        extra: vec![],
        servers: Some(mailcore::signin::Servers {
            imap: (
                "127.0.0.1".into(),
                port(1143),
                mailcore::model::Security::Plain,
            ),
            smtp: (
                "127.0.0.1".into(),
                port(1025),
                mailcore::model::SmtpSecurity::Plain,
            ),
        }),
    };

    // The demo's stand-in for the AI company, with its key in fakeai::GOOD_KEY.
    let ai = fakeai::FakeAi::start_on(&AI_ADDR).await;
    // Short replies to tap under an email, whichever email it is.
    ai.answer(
        "short replies to:",
        json!({ "replies": ["Looks good, I'll review it by Friday.", "Could we move the press date a week?", "Let me check and get back to you."] }),
    );
    // AI filters: the first email of each batch fits the first filter.
    ai.answer(
        "filters:\n1.",
        json!({ "emails": [{ "email": 0, "filters": [1] }] }),
    );
    let mut launch = fakeai::blank_answer();
    launch["similar"] = json!(["launch", "rollout"]);
    launch["from"] = json!(["Maya"]);
    launch["summary"] = json!("Launch emails from Maya");
    ai.answer("what maya said", launch);
    // The Ask chat: a question about the launch plan, answered from the emails found.
    ai.answer(
        "Question: what needs my reply",
        json!({
            "answer": "Maya is waiting on your thoughts about the Q4 launch plan, and Sam's invoice 1042 is due in 14 days.",
            "points": [
                { "text": "Maya asked you to review the press date and the launch timeline.", "ids": [3, 1, 2, 4, 5] },
                { "text": "Invoice 1042 from Sam Okafor is on their portal, due in 14 days.", "ids": [] },
            ],
            "key_ids": [3],
            "suggestions": ["When is the launch?", "How much is invoice 1042?"],
        }),
    );
    // Today's brief, from what needs the person.
    ai.answer(
        "\nAbout: ",
        json!({ "text": "Maya wants your thoughts on the Q4 launch plan, and Bo needs the report by Friday." }),
    );
    // Reading new mail by itself: an email that asks for something by a date.
    ai.answer(
        "Subject: Report due",
        json!({ "items": [{ "index": 0, "tab": "general", "label": "needs_reply", "group": "needs_you",
                            "line": "Bo needs the report by Friday", "due": "2026-11-13" }] }),
    );
    // Polishing a draft, and writing a new email from notes.
    ai.answer(
        "can u send",
        json!({ "text": "Hi Bob,\n\nCould you send the report by Friday?\n\nThanks" }),
    );
    ai.answer(
        "dinner",
        json!({ "text": "Hi Ana,\n\nWould you like to have dinner on Saturday?\n\nBest" }),
    );
    ai.answer(
        "review the timeline",
        json!({
            "summary": "Maya asks for a review of the Q4 launch plan.",
            "points": ["Review by Friday"],
            "needs_reply": true,
            "reply": "Thanks Maya, I'll review it by Friday.",
            "text": "Hola, aquí está el plan.",
        }),
    );
    ai.answer(
        "agreed, moved it",
        json!({ "text": "Hola, aquí está el plan." }),
    );

    let store = Store::open_in_memory().expect("database");
    if account_url.is_none() {
        // No account server: start signed in, as the demo person.
        store
            .save_identity(DEMO_USER, 0, "Browser test", "ready")
            .expect("identity");
    }
    let mut engine = Engine::new(store, Arc::new(MemoryStore::default()));
    if let Some(url) = &account_url {
        engine = engine
            .with_cloud(url, "Browser test")
            .expect("account server address");
    }
    let engine = engine
        .with_google_auth_url(&google.auth_url)
        .with_ai_url(&ai.url)
        .with_mail_sign_in(mail_sign_in("microsoft"))
        .with_mail_sign_in(mail_sign_in("google"))
        .with_provider_lookup(
            &format!("http://{}/directory", *HTTP_ADDR),
            Arc::new(|_: &str| vec![]),
        )
        .with_avatar_sources(
            &format!("http://{}/avatars/person", *HTTP_ADDR),
            &format!("http://{}/avatars/logo", *HTTP_ADDR),
        )
        .with_url_opener(Arc::new(|url: &str| {
            let url = url.to_string();
            std::thread::spawn(move || {
                let _ = std::process::Command::new("curl")
                    .args(["-sL", "-o", "/dev/null", "--max-time", "10", &url])
                    .status();
            });
        }));
    let teammate = account_url.as_ref().map(|url| {
        Engine::new(
            Store::open_in_memory().expect("database"),
            Arc::new(MemoryStore::default()),
        )
        .with_cloud(url, "Teammate's computer")
        .expect("account server address")
    });
    let app = Arc::new(App {
        oauth,
        engine,
        teammate,
        account_dev,
        sign_in_mails: AtomicUsize::new(0),
        server,
        new_mail: AtomicUsize::new(0),
        pixel_hits: AtomicUsize::new(0),
        outgoing,
    });

    let router = Router::new()
        .route("/call", post(call))
        .route("/attachment", axum::routing::get(attachment))
        .route("/original", axum::routing::get(original))
        .route("/dev/teammate/call", post(teammate_call))
        .route("/dev/new-mail", post(new_mail))
        .route("/dev/team-mail", post(team_mail))
        .route("/dev/revoke-oauth", post(revoke_oauth))
        .route("/dev/slow-store/{secs}", post(slow_store))
        .route("/dev/stranger-mail", post(stranger_mail))
        .route("/dev/newsletter-mail", post(newsletter_mail))
        .route("/dev/code-mail", post(code_mail))
        .route("/dev/pixel-hits", axum::routing::get(pixel_hits))
        .route("/dev/last-code", axum::routing::get(last_code))
        .route("/dev/deliver", post(deliver))
        .route("/dev/outbox", axum::routing::get(outbox))
        .route("/avatars/logo/{file}", axum::routing::get(demo_logo))
        .route("/directory/{domain}", axum::routing::get(demo_directory))
        .route("/tracking-pixel.gif", any(tracking_pixel))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(HTTP_ADDR.as_str())
        .await
        .expect("bind");
    println!(
        "devhost on http://{}, demo IMAP on {}",
        *HTTP_ADDR, *IMAP_ADDR
    );
    axum::serve(listener, router).await.expect("serve");
}
