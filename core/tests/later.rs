//! Snooze, reminders, pin, mute and scheduled sending, against the stand-in servers.

use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";
const LATER: i64 = 4_000_000_000;

struct Rig {
    server: FakeServer,
    outgoing: FakeSmtp,
    engine: Arc<Engine>,
    account: i64,
    alerts: Arc<Mutex<Vec<String>>>,
}

fn mail(id: &str, refs: Option<&str>, from: &str, subject: &str) -> String {
    let refs = refs
        .map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n"))
        .unwrap_or_default();
    format!(
        "From: {from}\nTo: Me <{USER}>\nSubject: {subject}\n\
         Date: Sun, 01 Nov 2026 10:00:00 +0000\nMessage-ID: <{id}>\n{refs}\
         Content-Type: text/plain; charset=utf-8\n\nBody of {id}\n"
    )
}

async fn rig() -> Rig {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    server.add_folder("Archive", &["\\Archive"], 1);
    let outgoing = FakeSmtp::start(USER, PASS).await;
    let alerts: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = alerts.clone();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_sent_copy_wait(0)
    .with_alert_sender(Arc::new(move |a: &mailcore::alerts::Alert| {
        log.lock().unwrap().push(format!("{}: {}", a.title, a.body));
    }));
    let account = engine
        .call(
            "add_account",
            json!({
                "email": USER, "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                "security": "plain", "password": PASS,
                "smtp_host": "127.0.0.1", "smtp_port": outgoing.addr.port(), "smtp_security": "plain",
            }),
        )
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    Rig {
        server,
        outgoing,
        engine: Arc::new(engine),
        account,
        alerts,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }

    async fn sync(&self) -> Value {
        self.call("sync_account", json!({ "account_id": self.account }))
            .await
    }

    async fn inbox_id(&self) -> i64 {
        let list = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == "inbox")
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    }

    async fn inbox(&self) -> Vec<Value> {
        let id = self.inbox_id().await;
        self.call("list_threads", json!({ "folder_id": id }))
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    async fn unread(&self) -> i64 {
        let list = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == "inbox")
            .unwrap()["unread"]
            .as_i64()
            .unwrap()
    }

    async fn thread_named(&self, subject: &str) -> Value {
        self.inbox()
            .await
            .into_iter()
            .find(|t| t["subject"] == subject)
            .unwrap()
    }

    async fn tick(&self, at: i64) -> Value {
        self.call("due_tick", json!({ "at": at })).await
    }

    fn add(&self, id: &str, refs: Option<&str>, subject: &str) {
        self.server.add_message_dated(
            "INBOX",
            &mail(id, refs, "Ana <ana@test.test>", subject),
            false,
            // Replies come after anything chosen here, and first emails before.
            if refs.is_some() || subject == "New" {
                "01-Nov-2036 10:00:00 +0000"
            } else {
                "01-Nov-2020 10:00:00 +0000"
            },
        );
    }
}

#[tokio::test]
async fn snoozed_mail_leaves_and_comes_back_at_the_top_unread() {
    let r = rig().await;
    r.add("a@t", None, "Snooze me");
    r.add("b@t", None, "Stay");
    r.sync().await;
    assert_eq!(r.unread().await, 2);
    let t = r.thread_named("Snooze me").await;
    r.call(
        "set_thread_seen",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "seen": true }),
    )
    .await;
    r.call(
        "thread_snooze",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": LATER }),
    )
    .await;

    let names: Vec<_> = r
        .inbox()
        .await
        .iter()
        .map(|t| t["subject"].clone())
        .collect();
    assert_eq!(names, vec![json!("Stay")]);
    let later = r.call("list_later", json!({ "kind": "snoozed" })).await;
    assert_eq!(later[0]["subject"], "Snooze me");
    assert_eq!(later[0]["snooze_until"], LATER);

    // Too soon: nothing happens.
    assert_eq!(r.tick(LATER - 10).await["changed"], false);
    assert_eq!(r.inbox().await.len(), 1);

    // Due: back, unread, and it says so.
    assert_eq!(r.tick(LATER + 1).await["changed"], true);
    let list = r.inbox().await;
    assert_eq!(list[0]["subject"], "Snooze me", "it's at the top");
    assert_eq!(list[0]["unread_count"], 1);
    assert!(list[0]["returned_at"].is_number());
    assert!(r
        .alerts
        .lock()
        .unwrap()
        .iter()
        .any(|a| a.starts_with("Back from snooze")));
    // The server knows it's unread too, so other apps agree.
    let uid = r.server.folder_messages("INBOX")[0].uid;
    assert!(!r.server.is_seen("INBOX", uid));

    // Opening it settles it.
    r.call(
        "set_thread_seen",
        json!({ "account_id": r.account, "thread_id": list[0]["thread_id"], "seen": true }),
    )
    .await;
    assert!(r.thread_named("Snooze me").await["returned_at"].is_null());
    assert_eq!(
        r.call("list_later", json!({ "kind": "snoozed" }))
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn a_reply_brings_snoozed_mail_back_early() {
    let r = rig().await;
    r.add("a@t", None, "Plan");
    r.sync().await;
    let t = r.thread_named("Plan").await;
    r.call(
        "thread_snooze",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": LATER }),
    )
    .await;
    assert_eq!(r.inbox().await.len(), 0);
    // The stand-in's clock is in 2036, after the snooze was set.
    r.add("b@t", Some("a@t"), "Re: Plan");
    r.sync().await;
    assert_eq!(r.tick(1).await["changed"], true);
    assert_eq!(r.inbox().await.len(), 1);
}

#[tokio::test]
async fn unread_snoozed_mail_doesnt_count_as_unread() {
    let r = rig().await;
    r.add("a@t", None, "Plan");
    r.sync().await;
    let t = r.thread_named("Plan").await;
    r.call(
        "thread_snooze",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": LATER }),
    )
    .await;
    assert_eq!(r.unread().await, 0);
    let id = r.inbox_id().await;
    let tabs = r.call("tab_counts", json!({ "folder_id": id })).await;
    assert_eq!(tabs["general"], 0);
}

#[tokio::test]
async fn pinned_mail_stays_on_top() {
    let r = rig().await;
    r.add("old@t", None, "Old");
    r.sync().await;
    r.add("new@t", None, "New");
    r.sync().await;
    let t = r.thread_named("Old").await;
    r.call(
        "thread_pin",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "on": true }),
    )
    .await;
    let list = r.inbox().await;
    assert_eq!(list[0]["subject"], "Old");
    assert_eq!(list[0]["pinned"], true);
    assert_eq!(list[1]["pinned"], false);
    let all = r.call("list_all", json!({ "role": "inbox" })).await;
    assert_eq!(all[0]["subject"], "Old", "and in All Inbox");
    r.call(
        "thread_pin",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "on": false }),
    )
    .await;
    assert_eq!(r.inbox().await[1]["subject"], "Old");
}

#[tokio::test]
async fn muted_conversations_stay_quiet_even_when_replied_to() {
    let r = rig().await;
    r.add("a@t", None, "Noisy");
    r.sync().await;
    let t = r.thread_named("Noisy").await;
    r.call(
        "thread_mute",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "on": true }),
    )
    .await;
    assert_eq!(r.inbox().await.len(), 0);
    r.alerts.lock().unwrap().clear();
    r.add("b@t", Some("a@t"), "Re: Noisy");
    r.sync().await;
    r.tick(1).await;
    assert_eq!(
        r.inbox().await.len(),
        0,
        "a new reply doesn't bring it back"
    );
    assert_eq!(r.unread().await, 0);
    assert!(r.alerts.lock().unwrap().is_empty(), "and doesn't notify");
    assert_eq!(
        r.call("list_later", json!({ "kind": "muted" })).await[0]["message_count"],
        2
    );
    // Still there on the server, and comes back when unmuted.
    assert_eq!(r.server.folder_messages("INBOX").len(), 2);
    r.call(
        "thread_mute",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "on": false }),
    )
    .await;
    assert_eq!(r.inbox().await.len(), 1);
}

#[tokio::test]
async fn a_reminder_comes_up_only_if_nobody_replied() {
    let r = rig().await;
    r.add("a@t", None, "Will they reply");
    r.add("b@t", None, "They will reply");
    r.sync().await;
    for name in ["Will they reply", "They will reply"] {
        let t = r.thread_named(name).await;
        r.call(
            "set_thread_seen",
            json!({ "account_id": r.account, "thread_id": t["thread_id"], "seen": true }),
        )
        .await;
        r.call("move_thread", json!({ "account_id": r.account, "thread_id": t["thread_id"], "folder_id": t["folder_id"], "to": "archive" })).await;
    }
    r.sync().await;
    assert_eq!(r.inbox().await.len(), 0);
    let threads = r.call("list_all", json!({ "role": "archive" })).await;
    for t in threads.as_array().unwrap() {
        r.call(
            "thread_follow_up",
            json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": LATER }),
        )
        .await;
    }
    assert_eq!(
        r.call("list_later", json!({ "kind": "reminders" }))
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    r.add("reply@t", Some("b@t"), "Re: They will reply");
    r.sync().await;

    r.tick(LATER + 1).await;
    let list = r.inbox().await;
    let due: Vec<_> = list.iter().filter(|t| t["follow_due"] == true).collect();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0]["subject"], "Will they reply");
    assert!(r
        .alerts
        .lock()
        .unwrap()
        .iter()
        .any(|a| a.starts_with("No reply yet")));
    // It's in Archive, not the inbox, and still shows until dealt with.
    r.call("move_thread", json!({ "account_id": r.account, "thread_id": due[0]["thread_id"], "folder_id": due[0]["folder_id"], "to": "archive" })).await;
    assert!(r.inbox().await.iter().all(|t| t["follow_due"] == false));
}

#[tokio::test]
async fn a_late_reply_clears_a_reminder_that_came_up() {
    let r = rig().await;
    r.add("a@t", None, "Waiting");
    r.sync().await;
    let t = r.thread_named("Waiting").await;
    r.call(
        "thread_follow_up",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": LATER }),
    )
    .await;
    r.tick(LATER + 1).await;
    assert_eq!(r.thread_named("Waiting").await["follow_due"], true);
    r.add("b@t", Some("a@t"), "Re: Waiting");
    r.sync().await;
    r.tick(LATER + 2).await;
    assert_eq!(r.inbox().await[0]["follow_due"], false);
}

async fn draft(r: &Rig, to: &str) -> i64 {
    let d = r
        .call(
            "draft_new",
            json!({ "account_id": r.account, "mode": "new" }),
        )
        .await;
    let id = d["id"].as_i64().unwrap();
    r.call(
        "draft_save",
        json!({
            "id": id, "account_id": r.account, "to": [{ "name": "", "address": to }],
            "subject": "Later", "html": "<p>Hi</p>", "text": "Hi",
        }),
    )
    .await;
    id
}

#[tokio::test]
async fn scheduled_mail_goes_once_at_its_time() {
    let r = rig().await;
    r.sync().await;
    let id = draft(&r, "bob@test.test").await;
    r.call("schedule_draft", json!({ "id": id, "send_at": LATER }))
        .await;
    assert_eq!(
        r.call("draft_get", json!({ "id": id })).await["send_at"],
        LATER
    );
    r.tick(LATER - 5).await;
    assert_eq!(r.outgoing.received().len(), 0);
    assert_eq!(r.tick(LATER + 1).await["sent"], 1);
    assert_eq!(r.outgoing.received().len(), 1);
    assert_eq!(r.tick(LATER + 2).await["sent"], 0, "and only once");
    assert_eq!(r.outgoing.received().len(), 1);
    assert_eq!(
        r.call("draft_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn cancelling_a_scheduled_message_keeps_it_as_a_draft() {
    let r = rig().await;
    r.sync().await;
    let id = draft(&r, "bob@test.test").await;
    r.call("schedule_draft", json!({ "id": id, "send_at": LATER }))
        .await;
    let back = r.call("unschedule_draft", json!({ "id": id })).await;
    assert!(back["send_at"].is_null());
    assert_eq!(r.tick(LATER + 1).await["sent"], 0);
}

#[tokio::test]
async fn a_scheduled_message_that_cant_go_comes_back_saying_why() {
    let r = rig().await;
    r.sync().await;
    let id = draft(&r, "not an address").await;
    r.call("schedule_draft", json!({ "id": id, "send_at": LATER }))
        .await;
    let reply = r.tick(LATER + 1).await;
    assert_eq!(reply["failed"].as_array().unwrap().len(), 1);
    let d = r.call("draft_get", json!({ "id": id })).await;
    assert!(d["send_at"].is_null());
    assert!(d["send_problem"].is_string());
    assert!(r
        .alerts
        .lock()
        .unwrap()
        .iter()
        .any(|a| a.contains("didn't send")));
}

#[tokio::test]
async fn being_offline_keeps_a_scheduled_message_waiting() {
    let r = rig().await;
    r.sync().await;
    let id = draft(&r, "bob@test.test").await;
    r.call("schedule_draft", json!({ "id": id, "send_at": LATER }))
        .await;
    // Nothing listens here, as with no connection.
    r.call(
        "update_account_settings",
        json!({
            "account_id": r.account, "display_name": "Me", "smtp_host": "127.0.0.1",
            "smtp_port": 9, "smtp_security": "plain",
        }),
    )
    .await;
    let reply = r.tick(LATER + 1).await;
    assert_eq!(reply["sent"], 0);
    assert_eq!(reply["failed"].as_array().unwrap().len(), 0);
    assert_eq!(
        r.call("draft_get", json!({ "id": id })).await["send_at"],
        LATER
    );
}

#[tokio::test]
async fn a_time_already_gone_is_refused() {
    let r = rig().await;
    r.sync().await;
    let id = draft(&r, "bob@test.test").await;
    assert!(r
        .engine
        .call("schedule_draft", json!({ "id": id, "send_at": 5 }))
        .await
        .is_err());
    r.add("a@t", None, "Plan");
    r.sync().await;
    let t = r.thread_named("Plan").await;
    assert!(r
        .engine
        .call(
            "thread_snooze",
            json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": 5 })
        )
        .await
        .is_err());
}

#[tokio::test]
async fn asking_to_be_reminded_when_sending_sets_it_on_the_conversation() {
    let r = rig().await;
    r.sync().await;
    let id = draft(&r, "bob@test.test").await;
    r.call("draft_remind", json!({ "id": id, "days": 3 })).await;
    r.call("send_draft", json!({ "id": id })).await;
    r.sync().await;
    let list = r.call("list_later", json!({ "kind": "reminders" })).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["subject"], "Later");
}

#[tokio::test]
async fn merged_conversations_keep_what_was_chosen() {
    let r = rig().await;
    r.add("a@t", None, "One");
    r.add("c@t", Some("b@t"), "Two");
    r.sync().await;
    let t = r.thread_named("Two").await;
    r.call(
        "thread_pin",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "on": true }),
    )
    .await;
    // This names both, so the two become one conversation.
    let raw =
        "From: Ana <ana@test.test>\nTo: Me <me@test.test>\nSubject: Re: One\nMessage-ID: <b@t>\n\
               References: <a@t>\nContent-Type: text/plain\n\nBridge\n";
    r.server
        .add_message_dated("INBOX", raw, false, "01-Nov-2036 11:00:00 +0000");
    r.sync().await;
    let list = r.inbox().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["pinned"], true);
}

#[tokio::test]
async fn a_reminder_to_reply_ends_once_you_reply() {
    let r = rig().await;
    r.add("a@t", None, "Can you answer");
    r.sync().await;
    let t = r.thread_named("Can you answer").await;
    r.call(
        "thread_follow_up",
        json!({ "account_id": r.account, "thread_id": t["thread_id"], "at": LATER }),
    )
    .await;
    // The person answers it themselves before the reminder is due.
    r.server.add_message_dated(
        "Sent",
        &mail(
            "mine@t",
            Some("a@t"),
            &format!("Me <{USER}>"),
            "Re: Can you answer",
        ),
        true,
        "01-Nov-2036 10:00:00 +0000",
    );
    r.sync().await;
    r.tick(LATER + 1).await;
    assert!(r.inbox().await.iter().all(|x| x["follow_due"] == false));
    assert!(r
        .alerts
        .lock()
        .unwrap()
        .iter()
        .all(|a| !a.starts_with("No reply yet")));
}

#[tokio::test]
async fn saying_it_doesnt_need_me_takes_it_out_of_needs_you_and_its_count() {
    let r = rig().await;
    r.add("a@t", None, "Back from snooze");
    r.add("b@t", None, "Reminder came up");
    r.sync().await;
    let inbox = r.inbox_id().await;
    let snoozed = r.thread_named("Back from snooze").await;
    r.call(
        "thread_snooze",
        json!({ "account_id": r.account, "thread_id": snoozed["thread_id"], "at": LATER }),
    )
    .await;
    let reminded = r.thread_named("Reminder came up").await;
    r.call(
        "thread_follow_up",
        json!({ "account_id": r.account, "thread_id": reminded["thread_id"], "at": LATER }),
    )
    .await;
    r.tick(LATER + 1).await;

    let needs = || async {
        let list = r
            .call(
                "list_threads",
                json!({ "folder_id": inbox, "tab": "needs_you" }),
            )
            .await;
        let count = r.call("tab_counts", json!({ "folder_id": inbox })).await["needs_you"]
            .as_i64()
            .unwrap();
        (list.as_array().unwrap().len() as i64, count)
    };
    assert_eq!(needs().await, (2, 2));

    for t in [&snoozed, &reminded] {
        r.call(
            "thread_priority",
            json!({ "account_id": r.account, "thread_id": t["thread_id"], "group": "good_to_know" }),
        )
        .await;
    }
    assert_eq!(
        needs().await,
        (0, 0),
        "both leave the list and the number together"
    );
}
