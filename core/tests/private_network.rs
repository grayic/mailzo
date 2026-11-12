//! On a shared web host the engine only reaches servers at public addresses. A mail server
//! or calendar whose name leads to this machine (here "localhost") is refused before
//! anything is sent to it, so it never even sees a connection.

use mailcore::model::{Security, SmtpSecurity};
use mailcore::net::Reach;
use mailcore::secrets::account_secret;
use mailcore::{Engine, MemoryStore, SecretStore, Store};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

const ME: &str = "me@example.com";
const PASS: &str = "secret-pass";

/// A server on this machine that notes whether anyone connected.
async fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

async fn nobody_connected(listener: &TcpListener) -> bool {
    tokio::time::timeout(Duration::from_millis(200), listener.accept())
        .await
        .is_err()
}

/// A web host's engine, with an inbox already saved whose servers are on this machine.
fn web_host_engine(imap_port: u16, smtp_port: u16, caldav: &str) -> (Engine, i64) {
    let store = Store::open_in_memory().unwrap();
    let account = store
        .insert_account(ME, ME, "localhost", imap_port, Security::Plain, ME)
        .unwrap();
    store
        .set_smtp(account.id, "localhost", smtp_port, SmtpSecurity::Plain)
        .unwrap();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(&account_secret(account.id), PASS).unwrap();
    let engine = Engine::new(store, secrets)
        .with_calendar_urls(caldav, "http://127.0.0.1:9/g", "http://127.0.0.1:9/m")
        .with_public_links_only();
    (engine, account.id)
}

#[tokio::test]
async fn an_incoming_server_on_this_machine_is_refused_on_a_web_host() {
    let (imap, port) = listener().await;
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_public_links_only();
    let refused = engine
        .call(
            "add_account",
            json!({
                "email": ME, "imap_host": "localhost", "imap_port": port,
                "security": "plain", "password": PASS,
            }),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("public internet"), "{refused}");
    assert!(nobody_connected(&imap).await);

    // An inbox saved earlier doesn't get through either when it syncs.
    let (engine, account) = web_host_engine(port, 9, "http://127.0.0.1:9/dav");
    let synced = engine
        .call("sync_account", json!({ "account_id": account }))
        .await;
    let problem = match synced {
        Err(e) => e.to_string(),
        Ok(stats) => stats["errors"].to_string(),
    };
    assert!(problem.contains("public internet"), "{problem}");
    assert!(nobody_connected(&imap).await);
}

#[tokio::test]
async fn an_outgoing_server_on_this_machine_is_refused_on_a_web_host() {
    let (smtp, port) = listener().await;
    let (engine, account) = web_host_engine(9, port, "http://127.0.0.1:9/dav");
    let refused = engine
        .call("check_smtp", json!({ "account_id": account }))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("public internet"), "{refused}");
    assert!(nobody_connected(&smtp).await);
}

#[tokio::test]
async fn on_a_mac_a_server_on_this_machine_is_still_reached() {
    // A mail bridge on the person's own computer keeps working.
    let (imap, port) = listener().await;
    let account = mailcore::model::Account {
        imap_host: "localhost".into(),
        imap_port: port,
        security: Security::Plain,
        ..test_account()
    };
    let attempt = tokio::spawn(async move {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            mailcore::imap::ImapConnection::connect(&account, PASS, mailcore::net::Reach::Anywhere),
        )
        .await;
    });
    assert!(!nobody_connected(&imap).await);
    attempt.abort();
}

fn test_account() -> mailcore::model::Account {
    let store = Store::open_in_memory().unwrap();
    store
        .insert_account(ME, ME, "localhost", 9, Security::Plain, ME)
        .unwrap()
}

#[tokio::test]
async fn a_calendar_on_this_machine_is_refused_on_a_web_host() {
    let (dav, port) = listener().await;
    let start = format!("http://localhost:{port}/dav/start");
    let (engine, account) = web_host_engine(9, 9, &start);
    // The calendar is never found, and never hears a thing.
    let refused = engine
        .call("calendar_connect", json!({ "account_id": account }))
        .await;
    assert!(refused.is_err());
    assert!(nobody_connected(&dav).await);

    // The same calendar is tried on a Mac.
    let store = Store::open_in_memory().unwrap();
    let mac_account = store
        .insert_account(ME, ME, "localhost", 9, Security::Plain, ME)
        .unwrap();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(&account_secret(mac_account.id), PASS).unwrap();
    let mac = Engine::new(store, secrets).with_calendar_urls(
        &start,
        "http://127.0.0.1:9/g",
        "http://127.0.0.1:9/m",
    );
    let attempt = tokio::spawn(async move {
        let _ = mac
            .call("calendar_connect", json!({ "account_id": mac_account.id }))
            .await;
    });
    assert!(!nobody_connected(&dav).await);
    attempt.abort();
}

#[tokio::test]
async fn a_guessed_mail_server_on_this_machine_isnt_probed_on_a_web_host() {
    let (server, port) = listener().await;
    let limit = Duration::from_millis(500);
    let probed = mailcore::imap::answers("localhost", port, limit, Reach::PublicOnly).await;
    assert!(!probed);
    assert!(nobody_connected(&server).await);

    // On a Mac the probe does connect (it fails only for want of a real server).
    let attempt = tokio::spawn(mailcore::imap::answers(
        "localhost",
        port,
        limit,
        Reach::Anywhere,
    ));
    assert!(!nobody_connected(&server).await);
    attempt.abort();
}
