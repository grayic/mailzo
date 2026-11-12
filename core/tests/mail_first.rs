//! Mailzo works on one device with no Mailzo account: connect an inbox and the mail is there,
//! alerts included. The account only matters once the person wants other devices.

use fakeimap::FakeServer;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::json;
use std::sync::{Arc, Mutex};

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";
const NEW_MAIL: &str = "01-Nov-2036 10:00:00 +0000";

fn question(id: &str) -> String {
    format!(
        "From: Ana <ana@people.test>\r\nTo: {USER}\r\nSubject: Contract {id}\r\nMessage-ID: <{id}@t>\r\n\
         Date: Mon, 1 Nov 2036 09:00:00 +0000\r\n\r\nCould you send it?\r\n"
    )
}

/// New mail's alerts, on an engine set up like the Mac app (with an account server).
async fn alerts_with(store: Store) -> Vec<String> {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    let alerts: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = alerts.clone();
    let engine = Engine::new(store, Arc::new(MemoryStore::default()))
        .with_cloud("http://127.0.0.1:9", "Test Mac")
        .unwrap()
        .with_alert_sender(Arc::new(move |a: &mailcore::alerts::Alert| {
            log.lock().unwrap().push(a.second_line().to_string());
        }));
    let id = engine
        .call(
            "add_account",
            json!({ "email": USER, "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": PASS, "smtp_host": "127.0.0.1", "smtp_port": 1,
                    "smtp_security": "plain" }),
        )
        .await
        .unwrap()["id"]
        .clone();
    engine
        .call("sync_account", json!({ "account_id": id }))
        .await
        .unwrap();
    server.add_message_dated("INBOX", &question("a"), false, NEW_MAIL);
    engine
        .call("sync_account", json!({ "account_id": id }))
        .await
        .unwrap();
    let got = alerts.lock().unwrap().clone();
    got
}

#[tokio::test]
async fn with_no_account_the_mail_comes_in_and_alerts_show() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(alerts_with(store).await, ["Contract a"]);
}

#[tokio::test]
async fn halfway_through_signing_in_nothing_about_the_mail_shows() {
    let store = Store::open_in_memory().unwrap();
    store
        .save_identity(USER, 0, "Test Mac", "needs_setup")
        .unwrap();
    assert!(alerts_with(store).await.is_empty());
}

/// The menu bar and the main window hear of new mail the moment the banner shows, not once
/// the rest of the inbox has caught up, and they hear with banners turned off too.
#[tokio::test]
async fn the_windows_hear_of_new_mail_with_the_banner() {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    let heard: Arc<Mutex<Vec<(&'static str, std::time::Instant)>>> = Arc::default();
    let (banner, windows) = (heard.clone(), heard.clone());
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_alert_sender(Arc::new(move |_: &mailcore::alerts::Alert| {
        banner
            .lock()
            .unwrap()
            .push(("banner", std::time::Instant::now()));
    }))
    .with_new_mail_listener(Arc::new(move |_account: i64| {
        windows
            .lock()
            .unwrap()
            .push(("windows", std::time::Instant::now()));
    }));
    let id = engine
        .call(
            "add_account",
            json!({ "email": USER, "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": PASS, "smtp_host": "127.0.0.1", "smtp_port": 1,
                    "smtp_security": "plain" }),
        )
        .await
        .unwrap()["id"]
        .clone();
    let refresh = || engine.call("sync_account", json!({ "account_id": id }));
    refresh().await.unwrap();
    heard.lock().unwrap().clear();

    server.add_message_dated("INBOX", &question("a"), false, NEW_MAIL);
    refresh().await.unwrap();
    let got = heard.lock().unwrap().clone();
    let at = |who: &str| got.iter().find(|(w, _)| *w == who).map(|(_, t)| *t);
    let (windows, banner) = (
        at("windows").expect("windows told"),
        at("banner").expect("a banner"),
    );
    assert!(
        windows <= banner,
        "the windows hear no later than the banner: {got:?}"
    );

    // No banner, and still the windows hear.
    engine
        .call("set_setting", json!({ "key": "alerts", "value": "off" }))
        .await
        .unwrap();
    heard.lock().unwrap().clear();
    server.add_message_dated("INBOX", &question("b"), false, NEW_MAIL);
    refresh().await.unwrap();
    let got: Vec<&str> = heard.lock().unwrap().iter().map(|(w, _)| *w).collect();
    assert_eq!(got, ["windows"]);
}
