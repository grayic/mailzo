//! Each setting doing what it says, through the real engine.

use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::alerts::Alert;
use mailcore::parse::{parse_body, parse_headers};
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const ME: &str = "me@grayic.test";
const PASS: &str = "secret-pass";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    smtp: FakeSmtp,
    account: i64,
    alerts: Arc<Mutex<Vec<(String, String, bool)>>>,
    opens: Arc<Mutex<Vec<(i64, String)>>>,
}

fn mail(from: &str, to: &str, id: &str, refs: &str, subject: &str) -> String {
    let refs = if refs.is_empty() {
        String::new()
    } else {
        format!("In-Reply-To: <{refs}>\nReferences: <{refs}>\n")
    };
    format!(
        "From: {from}\nTo: {to}\nSubject: {subject}\nMessage-ID: <{id}>\n{refs}Content-Type: text/plain\n\nBody of {subject}\n"
    )
}

async fn rig() -> Rig {
    let imap = FakeServer::start(ME, PASS).await;
    imap.add_folder("INBOX", &[], 1);
    imap.add_folder("Sent", &["\\Sent"], 1);
    imap.add_folder("Archive", &["\\Archive"], 1);
    imap.add_folder("Old Stuff", &[], 1);
    imap.add_folder("Trash", &["\\Trash"], 1);
    let smtp = FakeSmtp::start(ME, PASS).await;
    let alerts: Arc<Mutex<Vec<(String, String, bool)>>> = Arc::default();
    let log = alerts.clone();
    let opens: Arc<Mutex<Vec<(i64, String)>>> = Arc::default();
    let opens_log = opens.clone();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_sent_copy_wait(0)
    .with_alert_sender(Arc::new(move |a: &Alert| {
        log.lock()
            .unwrap()
            .push((a.title.clone(), a.second_line().to_string(), a.sound));
        opens_log.lock().unwrap().extend(a.thread.clone());
    }));
    let added = engine
        .call(
            "add_account",
            json!({
                "email": ME, "imap_host": "127.0.0.1", "imap_port": imap.addr.port(),
                "security": "plain", "password": PASS,
                "smtp_host": "127.0.0.1", "smtp_port": smtp.addr.port(), "smtp_security": "plain",
            }),
        )
        .await
        .unwrap();
    Rig {
        engine,
        imap,
        smtp,
        account: added["id"].as_i64().unwrap(),
        alerts,
        opens,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }

    async fn set(&self, key: &str, value: &str) {
        self.call("set_setting", json!({ "key": key, "value": value }))
            .await;
    }

    async fn sync(&self) {
        self.call("sync_account", json!({ "account_id": self.account }))
            .await;
    }

    async fn account(&self) -> Value {
        self.call("list_accounts", json!({})).await[0].clone()
    }

    async fn prefs(&self, change: impl FnOnce(&mut Value)) {
        let mut prefs = self.account().await["prefs"].clone();
        change(&mut prefs);
        self.call(
            "update_account_prefs",
            json!({ "account_id": self.account, "prefs": prefs }),
        )
        .await;
    }

    async fn folder(&self, name: &str) -> i64 {
        let list = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == name)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    }

    async fn inbox(&self) -> Vec<Value> {
        let id = self.folder("INBOX").await;
        self.call("list_threads", json!({ "folder_id": id }))
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    async fn draft(&self, mode: &str, message: Option<i64>) -> Value {
        self.call(
            "draft_new",
            json!({ "account_id": self.account, "mode": mode, "message_id": message }),
        )
        .await
    }

    async fn send(&self, draft: &Value, changes: Value) -> String {
        let mut args = json!({
            "id": draft["id"], "account_id": draft["account_id"], "from_address": draft["from_address"],
            "to": draft["to"], "cc": draft["cc"], "bcc": draft["bcc"], "subject": draft["subject"],
            "html": draft["html"], "text": draft["text"],
        });
        for (k, v) in changes.as_object().unwrap() {
            args[k] = v.clone();
        }
        let saved = self.call("draft_save", args).await;
        self.call("send_draft", json!({ "id": saved["id"] })).await;
        self.smtp.received().last().unwrap().data.clone()
    }
}

fn to(address: &str) -> Value {
    json!([{ "name": "", "address": address }])
}

#[tokio::test]
async fn settings_have_sensible_defaults_and_refuse_nonsense() {
    let r = rig().await;
    let all = r.call("app_settings", json!({})).await;
    assert_eq!(all["undo_send"], "10");
    assert_eq!(all["theme"], "system");
    assert_eq!(all["alert_which"], "all");
    r.set("undo_send", "30").await;
    assert_eq!(r.call("app_settings", json!({})).await["undo_send"], "30");
    for (key, value) in [("undo_send", "7"), ("theme", "pink"), ("made_up", "on")] {
        let bad = r
            .engine
            .call("set_setting", json!({ "key": key, "value": value }))
            .await;
        assert_eq!(bad.unwrap_err().code(), "invalid", "{key}");
    }
}

#[tokio::test]
async fn a_copied_sign_in_code_sends_its_email_to_the_bin_unless_chosen_otherwise() {
    let r = rig().await;
    assert_eq!(
        r.call("app_settings", json!({})).await["after_code_copy"],
        "trash"
    );
    for choice in ["archive", "read", "keep", "trash"] {
        r.set("after_code_copy", choice).await;
        assert_eq!(
            r.call("app_settings", json!({})).await["after_code_copy"],
            choice
        );
    }
}

#[tokio::test]
async fn new_mail_shows_in_the_menu_bar_unless_turned_off() {
    let r = rig().await;
    assert_eq!(
        r.call("app_settings", json!({})).await["alert_menu_bar"],
        "on"
    );
    r.set("alert_menu_bar", "off").await;
    assert_eq!(
        r.call("app_settings", json!({})).await["alert_menu_bar"],
        "off"
    );
}

#[tokio::test]
async fn the_app_starts_in_the_dock_and_menu_bar_and_not_at_login_until_asked() {
    let r = rig().await;
    let all = r.call("app_settings", json!({})).await;
    assert_eq!(all["show_in_menu_bar"], "on");
    assert_eq!(all["show_in_dock"], "on");
    assert_eq!(all["open_at_login"], "off");
    r.set("open_at_login", "on").await;
    assert_eq!(
        r.call("app_settings", json!({})).await["open_at_login"],
        "on"
    );
}

#[tokio::test]
async fn the_menu_bar_and_dock_cant_both_be_turned_off() {
    let r = rig().await;
    let turn_off = |key: &'static str| {
        r.engine
            .call("set_setting", json!({ "key": key, "value": "off" }))
    };
    // With only the menu bar on, it has to stay.
    r.set("show_in_dock", "off").await;
    assert_eq!(
        turn_off("show_in_menu_bar").await.unwrap_err().code(),
        "invalid"
    );
    // With both on, either can go, but then the other has to stay.
    r.set("show_in_dock", "on").await;
    r.set("show_in_menu_bar", "off").await;
    assert_eq!(
        turn_off("show_in_dock").await.unwrap_err().code(),
        "invalid"
    );
    let all = r.call("app_settings", json!({})).await;
    assert_eq!(all["show_in_menu_bar"], "off");
    assert_eq!(all["show_in_dock"], "on");
}

#[tokio::test]
async fn conversations_can_be_switched_off() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "Plan"), false);
    r.imap.add_message(
        "INBOX",
        &mail("a@x.test", ME, "2@x", "1@x", "Re: Plan"),
        false,
    );
    r.imap.add_message(
        "Sent",
        &mail(ME, "a@x.test", "3@x", "2@x", "Re: Plan"),
        true,
    );
    r.sync().await;
    assert_eq!(r.inbox().await.len(), 1);

    r.set("conversations", "off").await;
    let rows = r.inbox().await;
    assert_eq!(rows.len(), 2, "each email has its own row");
    assert_eq!(rows[0]["message_count"], 1);
    let id = rows[0]["thread_id"].as_str().unwrap();
    let opened = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": id, "folder_id": rows[0]["folder_id"] }),
        )
        .await;
    assert_eq!(opened.as_array().unwrap().len(), 1, "opens just that email");

    // Actions on a row touch only that email.
    r.call(
        "set_thread_seen",
        json!({ "account_id": r.account, "thread_id": id, "seen": true }),
    )
    .await;
    let seen = r
        .imap
        .folder_messages("INBOX")
        .iter()
        .filter(|m| m.flags.contains("\\Seen"))
        .count();
    assert_eq!(seen, 1);
    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": id, "folder_id": rows[0]["folder_id"], "to": "archive" }),
    )
    .await;
    assert_eq!(r.imap.folder_messages("INBOX").len(), 1);
    assert_eq!(r.imap.folder_messages("Archive").len(), 1);
}

#[tokio::test]
async fn the_chosen_font_goes_into_sent_mail() {
    let r = rig().await;
    r.set("font", "georgia").await;
    r.set("font_size", "18").await;
    let d = r.draft("new", None).await;
    let raw = r
        .send(
            &d,
            json!({ "to": to("a@x.test"), "html": "<p>Hi</p>", "text": "Hi" }),
        )
        .await;
    let html = parse_body(raw.as_bytes()).html.unwrap();
    assert!(
        html.contains("Georgia") && html.contains("font-size:18px"),
        "{html}"
    );
}

#[tokio::test]
async fn replies_can_leave_out_the_earlier_message_but_forwards_cannot() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "Plan"), true);
    r.sync().await;
    let t = r.inbox().await;
    let msgs = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": t[0]["thread_id"] }),
        )
        .await;
    let id = msgs[0]["id"].as_i64();
    assert!(r.draft("reply", id).await["quoted_html"]
        .as_str()
        .unwrap()
        .contains("Body of Plan"));
    r.set("quote", "off").await;
    let reply = r.draft("reply", id).await;
    assert_eq!(reply["quoted_html"], "");
    assert!(r.draft("forward", id).await["quoted_html"]
        .as_str()
        .unwrap()
        .contains("Body of Plan"));
    let raw = r
        .send(&reply, json!({ "html": "<p>Hi</p>", "text": "Hi" }))
        .await;
    assert!(
        !parse_headers(raw.as_bytes()).in_reply_to.is_empty(),
        "it still threads"
    );
}

#[tokio::test]
async fn new_messages_and_replies_can_start_with_different_signatures() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "Plan"), true);
    r.sync().await;
    r.prefs(|p| {
        p["signatures"] = json!([
            { "id": "full", "name": "Full", "html": "<p><strong>Sam</strong></p><p>Grayic</p><script>bad()</script>" },
            { "id": "short", "name": "Short", "html": "<p>F</p>" },
        ]);
        p["signature_new"] = json!("full");
        p["signature_reply"] = json!("short");
    })
    .await;
    let new = r.draft("new", None).await;
    let html = new["html"].as_str().unwrap();
    assert!(
        html.contains("<strong>Sam</strong>") && !html.contains("script"),
        "{html}"
    );

    let t = r.inbox().await;
    let msgs = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": t[0]["thread_id"] }),
        )
        .await;
    let reply = r.draft("reply", msgs[0]["id"].as_i64()).await;
    assert!(reply["html"].as_str().unwrap().contains("<p>F</p>"));

    r.prefs(|p| p["signature_reply"] = Value::Null).await;
    let bare = r.draft("reply", msgs[0]["id"].as_i64()).await;
    assert_eq!(bare["html"], "", "no signature is a real choice");
}

#[tokio::test]
async fn older_plain_signatures_keep_working() {
    let r = rig().await;
    r.call(
        "update_account_settings",
        json!({ "account_id": r.account, "display_name": "Me", "signature": "Plain sign-off" }),
    )
    .await;
    assert!(r.draft("new", None).await["html"]
        .as_str()
        .unwrap()
        .contains("Plain sign-off"));
}

#[tokio::test]
async fn other_addresses_send_and_replies_come_from_where_mail_arrived() {
    let r = rig().await;
    r.prefs(|p| p["aliases"] = json!([{ "name": "Grayic Help", "address": "help@grayic.test" }]))
        .await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "a@x.test",
            "help@grayic.test, b@x.test",
            "1@x",
            "",
            "Question",
        ),
        true,
    );
    r.sync().await;
    let t = r.inbox().await;
    let msgs = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": t[0]["thread_id"] }),
        )
        .await;
    let id = msgs[0]["id"].as_i64();

    let reply = r.draft("reply_all", id).await;
    assert_eq!(reply["from_address"], "help@grayic.test");
    let people: Vec<&str> = reply["to"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        people,
        ["a@x.test", "b@x.test"],
        "my own alias isn't a recipient"
    );
    let raw = r
        .send(&reply, json!({ "html": "<p>x</p>", "text": "x" }))
        .await;
    let from = parse_headers(raw.as_bytes()).from.unwrap();
    assert_eq!(from.address, "help@grayic.test");
    assert_eq!(from.name, "Grayic Help");
    assert_eq!(r.smtp.received().last().unwrap().from, "help@grayic.test");

    r.prefs(|p| p["reply_from"] = json!("main")).await;
    assert_eq!(r.draft("reply", id).await["from_address"], "");

    // An address this inbox doesn't have can't be used.
    let d = r.draft("new", None).await;
    let raw = r
        .send(
            &d,
            json!({ "to": to("a@x.test"), "from_address": "ceo@bank.test", "text": "x" }),
        )
        .await;
    assert_eq!(parse_headers(raw.as_bytes()).from.unwrap().address, ME);
}

#[tokio::test]
async fn a_folder_can_be_given_a_job() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One"), true);
    r.sync().await;
    r.prefs(|p| p["folder_roles"] = json!({ "Old Stuff": "archive" }))
        .await;
    r.sync().await;
    let list = r
        .call("list_folders", json!({ "account_id": r.account }))
        .await;
    let role = |name: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == name)
            .unwrap()["role"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(role("Old Stuff"), "archive");
    assert_eq!(role("Archive"), "folder", "the job moved");

    let t = r.inbox().await;
    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": t[0]["thread_id"], "folder_id": t[0]["folder_id"], "to": "archive" }),
    )
    .await;
    assert_eq!(r.imap.folder_messages("Old Stuff").len(), 1);
    assert!(r.imap.folder_messages("Archive").is_empty());

    let bad = r
        .engine
        .call(
            "update_account_prefs",
            json!({ "account_id": r.account, "prefs": { "folder_roles": { "Old Stuff": "made-up" } } }),
        )
        .await;
    assert_eq!(bad.unwrap_err().code(), "invalid");
}

async fn all_inbox_count(r: &Rig) -> usize {
    r.call("list_all", json!({ "role": "inbox" }))
        .await
        .as_array()
        .unwrap()
        .len()
}

#[tokio::test]
async fn an_inbox_can_be_left_out_of_all_inbox() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One"), true);
    r.sync().await;
    assert_eq!(all_inbox_count(&r).await, 1);
    r.prefs(|p| p["in_all_inbox"] = json!(false)).await;
    assert_eq!(all_inbox_count(&r).await, 0);
    assert_eq!(r.inbox().await.len(), 1, "its own inbox still works");
}

#[tokio::test]
async fn updating_a_password_tests_it_first() {
    let r = rig().await;
    r.imap.state.lock().unwrap().pass = "new-pass".into();
    // The old password no longer works, as after changing it with the provider.
    assert!(r
        .engine
        .call("sync_account", json!({ "account_id": r.account }))
        .await
        .is_err());

    let args = |password: &str| {
        json!({
            "account_id": r.account, "imap_host": "127.0.0.1", "imap_port": r.imap.addr.port(),
            "security": "plain", "password": password,
        })
    };
    let wrong = r.engine.call("update_connection", args("typo")).await;
    assert_eq!(wrong.unwrap_err().code(), "auth");
    r.call("update_connection", args("new-pass")).await;
    r.sync().await;

    // Changing the server without retyping the password keeps the saved one.
    let bad_host = r
        .engine
        .call(
            "update_connection",
            json!({ "account_id": r.account, "imap_host": "127.0.0.1", "imap_port": 1, "security": "plain" }),
        )
        .await;
    assert!(bad_host.is_err());
    assert_eq!(
        r.account().await["imap_port"],
        r.imap.addr.port(),
        "nothing changed"
    );
    r.call(
        "update_connection",
        json!({ "account_id": r.account, "imap_host": "127.0.0.1", "imap_port": r.imap.addr.port(), "security": "plain" }),
    )
    .await;
    r.sync().await;

    // Plain connections still only reach this computer.
    let remote = r
        .engine
        .call(
            "update_connection",
            json!({ "account_id": r.account, "imap_host": "mail.example.com", "imap_port": 143, "security": "plain" }),
        )
        .await;
    assert_eq!(remote.unwrap_err().code(), "invalid");
}

#[tokio::test]
async fn a_notification_opens_the_email_it_is_about() {
    let r = rig().await;
    r.set("alert_which", "all").await;
    r.sync().await;
    r.imap.add_message(
        "INBOX",
        &mail("Ana <ana@x.test>", ME, "1@x", "", "Lunch"),
        false,
    );
    r.sync().await;
    let (account_id, thread_id) = r.opens.lock().unwrap().pop().unwrap();
    assert_eq!(account_id, r.account);
    let opened = r
        .call(
            "thread_summary",
            json!({ "account_id": account_id, "thread_id": thread_id }),
        )
        .await;
    assert_eq!(opened["subject"], "Lunch");

    let gone = r
        .call(
            "thread_summary",
            json!({ "account_id": account_id, "thread_id": "gone" }),
        )
        .await;
    assert!(gone.is_null(), "the window still opens, on nothing new");
}

#[tokio::test]
async fn new_mail_sends_the_right_alerts() {
    let r = rig().await;
    r.set("alert_which", "people").await;
    r.imap.add_message(
        "INBOX",
        &mail("old@x.test", ME, "0@x", "", "Old mail"),
        false,
    );
    r.sync().await;
    assert!(
        r.alerts.lock().unwrap().is_empty(),
        "the first look finds nothing new"
    );

    r.imap.add_message(
        "INBOX",
        &mail("Ana <ana@x.test>", ME, "1@x", "", "Lunch"),
        false,
    );
    r.imap.add_message(
        "INBOX",
        "From: News <news@x.test>\nList-Unsubscribe: <mailto:u@x.test>\nSubject: Sale\nMessage-ID: <2@x>\n\nBuy\n",
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail("seen@x.test", ME, "3@x", "", "Already read"),
        true,
    );
    r.sync().await;
    assert_eq!(
        *r.alerts.lock().unwrap(),
        [("Ana".to_string(), "Lunch".to_string(), true)],
        "people only"
    );

    r.alerts.lock().unwrap().clear();
    r.imap.add_message(
        "INBOX",
        "From: Ana <ana@x.test>\nSubject: Moved back\nDate: Mon, 01 Jan 2024 09:00:00 +0000\nMessage-ID: <old@x>\n\nOld\n",
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(ME, "ana@x.test", "mine@x", "", "To myself"),
        false,
    );
    r.sync().await;
    assert!(
        r.alerts.lock().unwrap().is_empty(),
        "old mail moved back, and your own mail, aren't news"
    );

    r.imap
        .add_message("INBOX", &mail("Bo <bo@x.test>", ME, "4@x", "", "Hi"), false);
    r.call(
        "sync_account",
        json!({ "account_id": r.account, "quiet_now": true }),
    )
    .await;
    assert!(r.alerts.lock().unwrap().is_empty(), "quiet hours");

    r.imap
        .add_message("INBOX", &mail("Cy <cy@x.test>", ME, "5@x", "", "Hi"), false);
    r.prefs(|p| p["alerts"] = json!("none")).await;
    r.sync().await;
    assert!(
        r.alerts.lock().unwrap().is_empty(),
        "this inbox is silenced"
    );

    r.prefs(|p| p["alerts"] = json!("default")).await;
    r.set("alert_details", "off").await;
    r.set("alert_sound", "off").await;
    r.imap.add_message(
        "INBOX",
        &mail("Di <di@x.test>", ME, "6@x", "", "Secret"),
        false,
    );
    r.sync().await;
    assert_eq!(
        *r.alerts.lock().unwrap(),
        [("New email".to_string(), ME.to_string(), false)]
    );
}

#[tokio::test]
async fn saved_mail_can_be_cleared_and_downloaded_again() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One"), true);
    r.sync().await;
    assert_eq!(r.inbox().await.len(), 1);
    r.call("clear_saved_mail", json!({})).await;
    assert!(r.inbox().await.is_empty());
    assert_eq!(
        r.imap.folder_messages("INBOX").len(),
        1,
        "the server is untouched"
    );
    r.sync().await;
    assert_eq!(r.inbox().await.len(), 1);
    assert_eq!(
        r.call("list_accounts", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn held_back_pictures_are_counted_once_per_email() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One"), true);
    r.sync().await;
    let t = r.inbox().await;
    let msgs = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": t[0]["thread_id"] }),
        )
        .await;
    assert_eq!(r.call("privacy_stats", json!({})).await["images_held"], 0);
    for _ in 0..2 {
        r.call("note_images_held", json!({ "message_id": msgs[0]["id"] }))
            .await;
    }
    assert_eq!(r.call("privacy_stats", json!({})).await["images_held"], 1);
}

#[tokio::test]
async fn the_layout_can_be_like_gmail_and_stays_on_this_device() {
    let r = rig().await;
    assert_eq!(r.call("app_settings", json!({})).await["layout"], "side");
    r.set("layout", "full").await;
    assert_eq!(r.call("app_settings", json!({})).await["layout"], "full");
    let bad = r
        .engine
        .call(
            "set_setting",
            json!({ "key": "layout", "value": "sideways" }),
        )
        .await;
    assert_eq!(bad.unwrap_err().code(), "invalid");
}
