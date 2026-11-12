//! New mail showing the moment the server says so.

use fakeimap::FakeServer;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const ME: &str = "me@grayic.test";
const PASS: &str = "secret";

struct Rig {
    engine: Arc<Engine>,
    imap: FakeServer,
    account: i64,
    _dir: TempDir,
}

async fn rig_with(no_idle: bool) -> Rig {
    let imap = FakeServer::start(ME, PASS).await;
    imap.add_folder("INBOX", &[], 1);
    imap.state.lock().unwrap().no_idle = no_idle;
    let dir = TempDir::new().unwrap();
    let engine = Engine::new(
        Store::open(&dir.path().join("mail.db")).unwrap(),
        Arc::new(MemoryStore::default()),
    );
    let added = engine
        .call(
            "add_account",
            json!({
                "email": ME, "imap_host": "127.0.0.1", "imap_port": imap.addr.port(),
                "security": "plain", "password": PASS,
            }),
        )
        .await
        .unwrap();
    let account = added["id"].as_i64().unwrap();
    engine
        .call("sync_account", json!({ "account_id": account }))
        .await
        .unwrap();
    Rig {
        engine: Arc::new(engine),
        imap,
        account,
        _dir: dir,
    }
}

fn start_waiting(r: &Rig) -> tokio::task::JoinHandle<Result<Value, mailcore::CoreError>> {
    let engine = r.engine.clone();
    let account = r.account;
    tokio::spawn(async move {
        engine
            .call("mail_wait", json!({ "account_id": account }))
            .await
    })
}

const MAIL: &str = "From: Ana <ana@test.test>\nSubject: Hi\nMessage-ID: <a@t>\n\nHello\n";

#[tokio::test]
async fn mail_that_landed_while_nobody_watched_is_noticed_at_once() {
    let r = rig_with(false).await;
    r.imap.add_message("INBOX", MAIL, false);
    let answer = tokio::time::timeout(Duration::from_secs(5), start_waiting(&r))
        .await
        .expect("it doesn't sit waiting for mail that's already there")
        .unwrap()
        .unwrap();
    assert_eq!(answer["changed"], true, "{answer}");
}

#[tokio::test]
async fn it_answers_as_soon_as_mail_arrives_and_not_before() {
    let r = rig_with(false).await;
    let waiting = start_waiting(&r);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!waiting.is_finished(), "nothing has happened yet");
    r.imap.add_message("INBOX", MAIL, false);
    let reply = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("it should answer quickly")
        .unwrap()
        .unwrap();
    assert_eq!(reply, json!({ "supported": true, "changed": true }));
}

#[tokio::test]
async fn a_server_that_cannot_tell_us_says_so_at_once() {
    let r = rig_with(true).await;
    let reply = tokio::time::timeout(Duration::from_secs(5), start_waiting(&r))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reply["supported"], false);
}

#[tokio::test]
async fn a_newer_watch_takes_over_and_the_older_page_hears_the_same_news() {
    // Two pages on one inbox, such as two browser tabs, or the app after waking from sleep.
    let r = rig_with(false).await;
    let first = start_waiting(&r);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = start_waiting(&r);
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Neither gives up and asks again, which would knock the other off.
    assert!(!first.is_finished(), "the older page keeps waiting");
    assert!(!second.is_finished(), "the newer one keeps watching");
    r.imap.add_message("INBOX", MAIL, false);
    for waiting in [second, first] {
        let reply = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("both hear about it quickly")
            .unwrap()
            .unwrap();
        assert_eq!(reply["changed"], true);
    }
}

#[tokio::test]
async fn stopping_and_removing_an_inbox_let_go() {
    let r = rig_with(false).await;
    let waiting = start_waiting(&r);
    tokio::time::sleep(Duration::from_millis(300)).await;
    r.engine
        .call("mail_wait_stop", json!({ "account_id": r.account }))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("stopping should end it")
        .unwrap()
        .unwrap();

    let waiting = start_waiting(&r);
    tokio::time::sleep(Duration::from_millis(300)).await;
    r.engine
        .call("remove_account", json!({ "account_id": r.account }))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("removing the inbox should end it")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_refused_password_is_reported_rather_than_waited_on() {
    let r = rig_with(false).await;
    r.imap.state.lock().unwrap().pass = "changed".into();
    let reply = tokio::time::timeout(Duration::from_secs(5), start_waiting(&r))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.is_err());
}

async fn folder_named(r: &Rig, name: &str) -> Value {
    let list = r
        .engine
        .call("list_folders", json!({ "account_id": r.account }))
        .await
        .unwrap();
    list.as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_folder_can_be_added_to_one_inbox() {
    let r = rig_with(false).await;
    let made = r
        .engine
        .call(
            "create_folder",
            json!({ "account_id": r.account, "name": "  Clients " }),
        )
        .await
        .unwrap();
    assert_eq!(made["name"], "Clients");
    assert_eq!(made["role"], "folder");
    assert_eq!(folder_named(&r, "Clients").await["path"], "Clients");
    assert!(
        r.imap
            .state
            .lock()
            .unwrap()
            .folders
            .iter()
            .any(|f| f.name == "Clients"),
        "on the server too"
    );
}

#[tokio::test]
async fn folders_go_where_this_server_keeps_them() {
    let r = rig_with(false).await;
    r.imap.add_folder("INBOX.Sent", &["\\Sent"], 1);
    r.engine
        .call(
            "create_folder",
            json!({ "account_id": r.account, "name": "Clients" }),
        )
        .await
        .unwrap();
    assert!(
        r.imap
            .state
            .lock()
            .unwrap()
            .folders
            .iter()
            .any(|f| f.name == "INBOX.Clients"),
        "beside the others under the inbox"
    );
}

#[tokio::test]
async fn bad_folder_names_are_refused_before_reaching_the_server() {
    let r = rig_with(false).await;
    r.imap.add_folder("Clients", &[], 1);
    r.engine
        .call("sync_account", json!({ "account_id": r.account }))
        .await
        .unwrap();
    let logins = r.imap.state.lock().unwrap().logins;
    for bad in ["", "   ", "a/b", "a.b", "Inbox", "spam", "clients", "\"x"] {
        let out = r
            .engine
            .call(
                "create_folder",
                json!({ "account_id": r.account, "name": bad }),
            )
            .await;
        assert!(out.is_err(), "{bad:?} should be refused");
    }
    assert_eq!(r.imap.state.lock().unwrap().logins, logins);
}
