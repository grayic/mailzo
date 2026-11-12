//! Runs the whole engine against the fake IMAP server.

use fakeimap::FakeServer;
use mailcore::{CoreError, Engine, MemoryStore, Store, SyncOptions};
use serde_json::{json, Value};
use std::sync::Arc;

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";

fn raw(id: &str, refs: Option<&str>, subject: &str) -> String {
    let refs = refs
        .map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n"))
        .unwrap_or_default();
    format!(
        "From: Ana Diaz <ana@test.test>\nTo: Me <me@test.test>\nSubject: {subject}\n\
         Date: Sun, 01 Nov 2026 10:00:00 +0000\nMessage-ID: <{id}>\n{refs}\
         Content-Type: text/plain; charset=utf-8\n\nBody of {id}\n"
    )
}

/// A recent date, so messages fall inside the offline-reading window.
fn today() -> String {
    "01-Nov-2036 10:00:00 +0000".to_string()
}

struct Rig {
    server: FakeServer,
    engine: Engine,
    secrets: Arc<MemoryStore>,
    account_id: i64,
}

async fn rig_with(options: SyncOptions) -> Rig {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    let secrets = Arc::new(MemoryStore::default());
    let engine = Engine::with_options(Store::open_in_memory().unwrap(), secrets.clone(), options);
    let account = engine
        .call("add_account", add_args(&server, PASS))
        .await
        .unwrap();
    Rig {
        account_id: account["id"].as_i64().unwrap(),
        server,
        engine,
        secrets,
    }
}

async fn rig() -> Rig {
    rig_with(SyncOptions::default()).await
}

fn add_args(server: &FakeServer, password: &str) -> Value {
    json!({
        "email": USER, "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
        "security": "plain", "password": password,
    })
}

impl Rig {
    fn add(&self, folder: &str, id: &str, refs: Option<&str>, seen: bool) -> u32 {
        self.server
            .add_message_dated(folder, &raw(id, refs, "Plan"), seen, &today())
    }

    async fn sync(&self) -> Value {
        self.engine
            .call("sync_account", json!({ "account_id": self.account_id }))
            .await
            .unwrap()
    }

    async fn folder_id(&self, role: &str) -> i64 {
        let list = self
            .engine
            .call("list_folders", json!({ "account_id": self.account_id }))
            .await
            .unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == role)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    }

    async fn threads(&self, role: &str) -> Vec<Value> {
        let id = self.folder_id(role).await;
        self.engine
            .call("list_threads", json!({ "folder_id": id }))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    async fn thread(&self, thread_id: &Value) -> Vec<Value> {
        self.engine
            .call(
                "get_thread",
                json!({ "account_id": self.account_id, "thread_id": thread_id }),
            )
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
}

#[tokio::test]
async fn first_sync_downloads_mail_and_never_marks_it_read() {
    let r = rig().await;
    let uid = r.add("INBOX", "a@t", None, false);
    let stats = r.sync().await;
    assert_eq!(stats["new_messages"], 1);
    assert_eq!(stats["bodies_fetched"], 1);
    assert_eq!(stats["errors"].as_array().unwrap().len(), 0);

    let threads = r.threads("inbox").await;
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["unread_count"], 1);
    assert_eq!(threads[0]["snippet"], "Body of a@t");
    assert_eq!(threads[0]["participants"][0], "Ana Diaz");

    assert_eq!(r.server.non_peek_fetches(), 0);
    assert!(!r.server.is_seen("INBOX", uid));
}

#[tokio::test]
async fn syncing_again_adds_no_duplicates_when_nothing_is_new() {
    // Real servers answer `UID FETCH n:*` with the newest message even when it
    // is older than n, so a careless sync would re-add it every time.
    let r = rig().await;
    r.add("INBOX", "a@t", None, false);
    r.sync().await;
    let again = r.sync().await;
    assert_eq!(again["new_messages"], 0);
    assert_eq!(again["removed_messages"], 0);
    assert_eq!(r.threads("inbox").await[0]["message_count"], 1);
}

#[tokio::test]
async fn new_mail_arrives_on_the_next_sync() {
    let r = rig().await;
    r.add("INBOX", "a@t", None, false);
    r.sync().await;
    r.add("INBOX", "b@t", Some("a@t"), false);
    r.add("INBOX", "c@t", None, false);
    let stats = r.sync().await;
    assert_eq!(stats["new_messages"], 2);
    assert_eq!(r.threads("inbox").await.len(), 2);
}

#[tokio::test]
async fn read_state_and_deletions_on_the_server_reach_the_mac() {
    let r = rig().await;
    let a = r.add("INBOX", "a@t", None, false);
    let b = r.add("INBOX", "b@t", None, false);
    r.sync().await;

    r.server.set_flag("INBOX", a, "\\Seen", true);
    r.server.set_flag("INBOX", a, "\\Flagged", true);
    r.server.delete_message("INBOX", b);
    let stats = r.sync().await;
    assert_eq!(stats["removed_messages"], 1);

    let threads = r.threads("inbox").await;
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["unread_count"], 0);
    assert_eq!(threads[0]["flagged"], true);
}

#[tokio::test]
async fn emptying_a_folder_on_the_server_empties_it_here() {
    let r = rig().await;
    let a = r.add("INBOX", "a@t", None, false);
    r.sync().await;
    r.server.delete_message("INBOX", a);
    let stats = r.sync().await;
    assert_eq!(stats["removed_messages"], 1);
    assert!(r.threads("inbox").await.is_empty());
}

#[tokio::test]
async fn a_renumbered_folder_is_downloaded_again_cleanly() {
    let r = rig().await;
    r.add("INBOX", "a@t", None, false);
    r.add("INBOX", "b@t", None, false);
    r.sync().await;

    r.server.reset_uid_validity("INBOX", 2);
    let stats = r.sync().await;
    assert_eq!(stats["removed_messages"], 2);
    assert_eq!(stats["new_messages"], 2);
    assert_eq!(r.threads("inbox").await.len(), 2);

    // The new numbers must be the ones in use from now on.
    let again = r.sync().await;
    assert_eq!(again["new_messages"], 0);
    assert_eq!(again["removed_messages"], 0);
}

#[tokio::test]
async fn first_sync_keeps_only_the_newest_mail_in_the_window() {
    let r = rig_with(SyncOptions {
        backfill_seconds: 0,
        inbox_window: 3,
        ..Default::default()
    })
    .await;
    for i in 0..10 {
        r.add("INBOX", &format!("m{i}@t"), None, false);
    }
    assert_eq!(r.sync().await["new_messages"], 3);
    // Mail older than the window must not count as deleted on later syncs.
    assert_eq!(r.sync().await["removed_messages"], 0);
    r.add("INBOX", "new@t", None, false);
    assert_eq!(r.sync().await["new_messages"], 1);
    assert_eq!(r.threads("inbox").await.len(), 4);
}

#[tokio::test]
async fn saved_mail_stays_readable_with_the_network_gone() {
    let r = rig().await;
    r.add("INBOX", "a@t", None, false);
    r.sync().await;
    r.server.go_offline().await;

    let threads = r.threads("inbox").await;
    let messages = r.thread(&threads[0]["thread_id"]).await;
    assert_eq!(
        messages[0]["body"]["text"].as_str().unwrap().trim(),
        "Body of a@t"
    );

    // The cached body is also returned without touching the network.
    let body = r
        .engine
        .call("fetch_body", json!({ "message_id": messages[0]["id"] }))
        .await
        .unwrap();
    assert_eq!(body["text"].as_str().unwrap().trim(), "Body of a@t");

    // A sync fails clearly and leaves the saved mail alone.
    let err = r
        .engine
        .call("sync_account", json!({ "account_id": r.account_id }))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Network(_)));
    assert_eq!(r.threads("inbox").await.len(), 1);
}

#[tokio::test]
async fn opening_a_message_downloads_it_once() {
    let r = rig_with(SyncOptions {
        backfill_seconds: 0,
        inbox_bodies: 0,
        ..Default::default()
    })
    .await;
    r.add("INBOX", "a@t", None, false);
    r.sync().await;
    assert_eq!(r.server.body_fetches(), 0);

    let threads = r.threads("inbox").await;
    let messages = r.thread(&threads[0]["thread_id"]).await;
    assert!(messages[0]["body"].is_null());

    let args = json!({ "message_id": messages[0]["id"] });
    let body = r.engine.call("fetch_body", args.clone()).await.unwrap();
    assert_eq!(body["text"].as_str().unwrap().trim(), "Body of a@t");
    assert_eq!(r.server.body_fetches(), 1);

    r.engine.call("fetch_body", args).await.unwrap();
    assert_eq!(r.server.body_fetches(), 1);
    assert_eq!(r.server.non_peek_fetches(), 0);
}

#[tokio::test]
async fn an_unsaved_message_reports_offline_without_a_network() {
    let r = rig_with(SyncOptions {
        backfill_seconds: 0,
        inbox_bodies: 0,
        ..Default::default()
    })
    .await;
    r.add("INBOX", "a@t", None, false);
    r.sync().await;
    let threads = r.threads("inbox").await;
    let messages = r.thread(&threads[0]["thread_id"]).await;
    r.server.go_offline().await;
    let err = r
        .engine
        .call("fetch_body", json!({ "message_id": messages[0]["id"] }))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Offline));
}

#[tokio::test]
async fn a_thread_spans_inbox_and_sent() {
    let r = rig().await;
    r.add("INBOX", "a@t", None, true);
    r.add("Sent", "b@t", Some("a@t"), true);
    r.add("INBOX", "c@t", Some("b@t"), false);
    r.sync().await;

    let threads = r.threads("inbox").await;
    assert_eq!(threads.len(), 1);
    assert_eq!(r.thread(&threads[0]["thread_id"]).await.len(), 3);
}

#[tokio::test]
async fn a_message_in_two_folders_shows_once() {
    let r = rig().await;
    r.add("INBOX", "a@t", None, true);
    r.add("Sent", "a@t", None, true);
    r.sync().await;
    let threads = r.threads("inbox").await;
    assert_eq!(r.thread(&threads[0]["thread_id"]).await.len(), 1);
}

#[tokio::test]
async fn a_wrong_password_saves_nothing() {
    let r = rig().await;
    let other = FakeServer::start("x@test.test", PASS).await;
    let err = r
        .engine
        .call(
            "add_account",
            json!({
                "email": "x@test.test", "imap_host": "localhost", "imap_port": other.addr.port(),
                "security": "plain", "password": "wrong",
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Auth));
    let accounts = r.engine.call("list_accounts", json!({})).await.unwrap();
    assert_eq!(accounts.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unencrypted_mail_to_another_computer_is_refused() {
    let r = rig().await;
    let err = r
        .engine
        .call(
            "add_account",
            json!({
                "email": "x@test.test", "imap_host": "mail.example.test", "imap_port": 143,
                "security": "plain", "password": "p",
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)));
}

#[tokio::test]
async fn the_same_account_cannot_be_added_twice() {
    let r = rig().await;
    let err = r
        .engine
        .call("add_account", add_args(&r.server, PASS))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)));
}

#[tokio::test]
async fn passwords_never_appear_in_replies_or_the_database_listing() {
    let r = rig().await;
    let text = r
        .engine
        .call("list_accounts", json!({}))
        .await
        .unwrap()
        .to_string();
    assert!(!text.contains(PASS));
    // The sign-in kind is called "password", but there's no field holding one.
    assert!(!text.contains("\"password\":"));
    assert!(!text.contains("\"secret\":"));
}

#[tokio::test]
async fn removing_an_account_deletes_its_password_and_mail() {
    let r = rig().await;
    r.add("INBOX", "a@t", None, false);
    r.sync().await;
    r.engine
        .call("remove_account", json!({ "account_id": r.account_id }))
        .await
        .unwrap();
    use mailcore::SecretStore;
    assert!(r
        .secrets
        .get(&format!("account-{}", r.account_id))
        .unwrap()
        .is_none());
    let accounts = r.engine.call("list_accounts", json!({})).await.unwrap();
    assert!(accounts.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_second_sync_of_the_same_account_is_skipped_while_one_runs() {
    let r = rig().await;
    for i in 0..50 {
        r.add("INBOX", &format!("m{i}@t"), None, false);
    }
    let args = json!({ "account_id": r.account_id });
    let (a, b) = tokio::join!(
        r.engine.call("sync_account", args.clone()),
        r.engine.call("sync_account", args.clone()),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a["already_running"], b["already_running"]);
    // The mark clears afterwards, so later syncs still run.
    assert_eq!(r.sync().await["already_running"], false);
}

#[tokio::test]
async fn unknown_commands_and_bad_arguments_fail_cleanly() {
    let r = rig().await;
    assert!(r.engine.call("nope", json!({})).await.is_err());
    let err = r
        .engine
        .call("add_account", json!({ "password": "topsecret" }))
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("topsecret"));
}

#[tokio::test]
async fn the_whole_mailbox_comes_down_in_the_background() {
    let r = rig_with(SyncOptions {
        inbox_window: 3,
        inbox_bodies: 2,
        ..Default::default()
    })
    .await;
    // Keeping all time, which a person can choose; the default is the past year.
    r.engine
        .call("set_setting", json!({ "key": "keep_mail", "value": "all" }))
        .await
        .unwrap();
    for i in 0..12 {
        r.server.add_message_dated(
            "INBOX",
            &raw(&format!("m{i}@t"), None, &format!("Mail {i}")),
            true,
            "01-Jan-2015 10:00:00 +0000",
        );
    }
    let stats = r.sync().await;
    assert_eq!(stats["new_messages"], 3, "only the newest count as new");
    assert_eq!(stats["older_saved"], 9, "the rest came down as older mail");
    assert_eq!(stats["more_to_download"], false);
    let folders = r
        .engine
        .call("list_folders", json!({ "account_id": r.account_id }))
        .await
        .unwrap();
    assert_eq!(folders[0]["total"], 12);
    // Every body too, however old, so it reads offline.
    assert_eq!(r.server.body_fetches(), 12);
    // Nothing more to do next time.
    let again = r.sync().await;
    assert_eq!(again["older_saved"], 0);
    assert_eq!(r.server.body_fetches(), 12);
}

#[tokio::test]
async fn pausing_the_download_stops_older_mail_but_not_new() {
    let r = rig_with(SyncOptions {
        inbox_window: 3,
        inbox_bodies: 2,
        ..Default::default()
    })
    .await;
    r.engine
        .call(
            "set_setting",
            json!({ "key": "download_paused", "value": "on" }),
        )
        .await
        .unwrap();
    for i in 0..12 {
        r.server.add_message_dated(
            "INBOX",
            &raw(&format!("m{i}@t"), None, &format!("Mail {i}")),
            true,
            "01-Jan-2015 10:00:00 +0000",
        );
    }
    let stats = r.sync().await;
    assert_eq!(stats["new_messages"], 3, "the newest still come in");
    assert_eq!(stats["older_saved"], 0, "nothing older while paused");
    // Resumed, the rest follows.
    r.engine
        .call(
            "set_setting",
            json!({ "key": "download_paused", "value": "off" }),
        )
        .await
        .unwrap();
    let again = r.sync().await;
    assert_eq!(again["older_saved"], 9);
}

#[tokio::test]
async fn keeping_less_mail_leaves_older_bodies_on_the_server() {
    let r = rig().await;
    r.server.add_message_dated(
        "INBOX",
        &raw("old@t", None, "Old"),
        true,
        "01-Jan-2015 10:00:00 +0000",
    );
    r.server
        .add_message_dated("INBOX", &raw("new@t", None, "New"), true, &today());
    r.engine
        .call(
            "set_setting",
            json!({ "key": "keep_mail", "value": "year" }),
        )
        .await
        .unwrap();
    r.sync().await;
    assert_eq!(r.server.body_fetches(), 1);
    // The header is here all the same, so it can be found and opened.
    let folders = r
        .engine
        .call("list_folders", json!({ "account_id": r.account_id }))
        .await
        .unwrap();
    assert_eq!(folders[0]["total"], 2);
}

#[tokio::test]
async fn the_same_email_under_two_labels_downloads_once() {
    let r = rig().await;
    r.server.add_folder("Work", &[], 1);
    r.server
        .add_message_dated("INBOX", &raw("twin@t", None, "Twin"), true, &today());
    r.server
        .add_message_dated("Work", &raw("twin@t", None, "Twin"), true, &today());
    r.sync().await;
    assert_eq!(r.server.body_fetches(), 1);
}

#[tokio::test]
async fn a_change_counter_means_only_changes_are_fetched() {
    let r = rig().await;
    r.server.state.lock().unwrap().change_counter = true;
    let mut uids = Vec::new();
    for i in 0..5 {
        uids.push(r.server.add_message_dated(
            "INBOX",
            &raw(&format!("c{i}@t"), None, "C"),
            false,
            &today(),
        ));
    }
    r.sync().await;
    let rows = |r: &Rig| r.server.state.lock().unwrap().flag_rows;
    let base = rows(&r);

    // Nothing changed: not one row is asked for.
    r.sync().await;
    assert_eq!(rows(&r), base);

    // One changed: only that one comes back, and it's applied.
    r.server.set_flag("INBOX", uids[1], "\\Seen", true);
    r.sync().await;
    assert_eq!(rows(&r) - base, 1);
    let list = r
        .engine
        .call(
            "list_threads",
            json!({ "folder_id": r.folder_id("inbox").await }),
        )
        .await
        .unwrap();
    let unread = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["unread_count"] == 1)
        .count();
    assert_eq!(unread, 4);

    // Deleted on the server: it goes here too, without reading every row.
    r.server.delete_message("INBOX", uids[0]);
    let stats = r.sync().await;
    assert_eq!(stats["removed_messages"], 1);
    assert_eq!(rows(&r) - base, 1);
}

#[tokio::test]
async fn a_failure_is_remembered_and_cleared_by_the_next_good_refresh() {
    let r = rig().await;
    r.sync().await;
    r.server.go_offline().await;
    assert!(r
        .engine
        .call("sync_account", json!({ "account_id": r.account_id }))
        .await
        .is_err());
    let health = r.engine.call("mail_health", json!({})).await.unwrap();
    assert!(health["inboxes"][0]["problem"].is_string());
    assert!(health["inboxes"][0]["last_ok"].is_number());
    assert!(health["bytes"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn a_broken_answer_from_the_server_deletes_nothing() {
    let r = rig().await;
    r.server
        .add_message_dated("INBOX", &raw("keep@t", None, "Keep"), false, &today());
    r.sync().await;
    r.server.state.lock().unwrap().empty_search = true;
    r.server
        .state
        .lock()
        .unwrap()
        .folders
        .iter_mut()
        .find(|f| f.name == "INBOX")
        .unwrap()
        .uid_next += 1;
    r.sync().await;
    let list = r
        .engine
        .call(
            "list_threads",
            json!({ "folder_id": r.folder_id("inbox").await }),
        )
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_list_shows_the_newest_email_in_a_conversation() {
    let r = rig().await;
    r.server.add_message_dated(
        "INBOX",
        &raw("first@t", None, "First"),
        false,
        "01-Nov-2036 10:00:00 +0000",
    );
    r.server.add_message_dated(
        "INBOX",
        &raw("second@t", Some("first@t"), "Second").replace("Sun, 01 Nov 2026", "Sat, 01 Nov 2036"),
        false,
        "02-Nov-2036 10:00:00 +0000",
    );
    r.sync().await;
    let list = r
        .engine
        .call(
            "list_threads",
            json!({ "folder_id": r.folder_id("inbox").await }),
        )
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["subject"], "Second");
}
