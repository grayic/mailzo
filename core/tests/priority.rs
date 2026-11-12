//! Sorting mail into what needs the person, and everything that hangs off that.

use fakeai::FakeAi;
use fakeimap::FakeServer;
use mailcore::model::Addr;
use mailcore::priority::{decide, Facts, GOOD_TO_KNOW, NEEDS_YOU, THE_REST};
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[path = "support/fake_paid.rs"]
mod fake_paid;
use fake_paid::FakePaid;

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";
const NEW_MAIL: &str = "01-Nov-2036 10:00:00 +0000";

fn base<'a>(mine: &'a BTreeSet<String>, to: &'a [Addr]) -> Facts<'a> {
    Facts {
        from: "ana@people.test",
        to,
        subject: "Plans",
        text: "Hello",
        category: "general",
        risk: 0,
        is_contact: false,
        answers_me: false,
        chases: false,
        auth: "none",
        mine,
        chosen: None,
    }
}

fn to_me() -> (BTreeSet<String>, Vec<Addr>) {
    (
        BTreeSet::from([USER.to_string()]),
        vec![Addr {
            name: String::new(),
            address: USER.to_string(),
        }],
    )
}

#[test]
fn people_you_know_need_you_and_robots_do_not() {
    let (mine, to) = to_me();
    let f = |change: &dyn Fn(&mut Facts)| {
        let mut facts = base(&mine, &to);
        change(&mut facts);
        decide(&facts)
    };
    assert_eq!(f(&|x| x.is_contact = true).group, NEEDS_YOU);
    assert_eq!(f(&|x| x.answers_me = true).group, NEEDS_YOU);
    assert_eq!(
        f(&|x| {
            x.is_contact = true;
            x.text = "Thanks!";
        })
        .group,
        GOOD_TO_KNOW
    );
    assert_eq!(
        f(&|x| {
            x.is_contact = true;
            x.to = &[];
        })
        .group,
        GOOD_TO_KNOW
    );
    assert_eq!(f(&|x| x.category = "newsletters").group, THE_REST);
    assert_eq!(
        f(&|x| {
            x.from = "no-reply@shop.test";
            x.is_contact = true;
        })
        .group,
        GOOD_TO_KNOW
    );
    assert_eq!(
        f(&|x| {
            x.from = "support@host.test";
            x.is_contact = true;
        })
        .group,
        GOOD_TO_KNOW
    );
    assert_eq!(
        f(&|x| {
            x.subject = "Automatic reply: Plans";
            x.is_contact = true;
        })
        .group,
        GOOD_TO_KNOW
    );
}

#[test]
fn a_stranger_has_to_ask_something_and_not_be_selling() {
    let (mine, to) = to_me();
    let f = |text: &'static str, from: &'static str| {
        let mut facts = base(&mine, &to);
        facts.text = text;
        facts.from = from;
        decide(&facts)
    };
    let asks = f("Could you send me the contract?", "ana@people.test");
    assert_eq!(asks.group, NEEDS_YOU);
    assert!(asks.unsure, "worth a second opinion");
    assert_eq!(f("Nice to see you.", "ana@people.test").group, GOOD_TO_KNOW);
    assert_eq!(
        f("Can I book a call to grow your sales?", "ana@people.test").group,
        GOOD_TO_KNOW
    );
    assert_eq!(
        f("Can you read this? Unsubscribe here.", "ana@people.test").group,
        GOOD_TO_KNOW
    );
    assert_eq!(
        f("Can you read this?", "hello@company.test").group,
        GOOD_TO_KNOW
    );
    assert_eq!(
        f("Can you read this?", "ana@t.company.test").group,
        GOOD_TO_KNOW
    );
    // Only the new words count, not the question being answered.
    assert_eq!(
        f(
            "Sure.\n\nOn Monday Bo wrote:\n> Can you help?",
            "ana@people.test"
        )
        .group,
        GOOD_TO_KNOW
    );
}

#[test]
fn money_and_accounts_need_you_but_only_from_a_proven_sender() {
    let (mine, to) = to_me();
    let mut facts = base(&mine, &to);
    facts.from = "billing@company.test";
    facts.subject = "Your payment failed";
    assert_eq!(
        decide(&facts).group,
        GOOD_TO_KNOW,
        "scams use these words too"
    );
    facts.auth = "pass";
    assert_eq!(decide(&facts).group, NEEDS_YOU);
    facts.risk = 1;
    assert_eq!(
        decide(&facts).group,
        GOOD_TO_KNOW,
        "suspicious mail is never promoted"
    );
}

#[test]
fn the_person_has_the_last_word() {
    let (mine, to) = to_me();
    let mut facts = base(&mine, &to);
    facts.is_contact = true;
    facts.chosen = Some(GOOD_TO_KNOW);
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    facts.category = "promotions";
    facts.chosen = Some(NEEDS_YOU);
    assert_eq!(decide(&facts).group, NEEDS_YOU);
}

struct Rig {
    server: FakeServer,
    engine: Engine,
    ai: FakeAi,
    account: i64,
    alerts: Arc<Mutex<Vec<String>>>,
}

/// What went to their own AI, leaving out the quick replies written ahead for new mail,
/// which is theirs to use for that.
fn not_quick_replies(r: &Rig) -> usize {
    r.ai.requests()
        .iter()
        .filter(|q| !q.to_string().contains("short replies"))
        .count()
}

fn mail(id: &str, from: &str, subject: &str, body: &str) -> String {
    format!("From: {from}\r\nTo: {USER}\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\nDate: Mon, 1 Nov 2036 09:00:00 +0000\r\n\r\n{body}\r\n")
}

async fn rig() -> Rig {
    rig_on(None).await
}

/// With the AI that comes with Mailzo Cloud plugged in.
async fn rig_paid() -> (Rig, Arc<FakePaid>) {
    let paid = FakePaid::new();
    (rig_on(Some(paid.clone())).await, paid)
}

async fn rig_on(paid: Option<Arc<FakePaid>>) -> Rig {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    server.add_folder("Archive", &["\\Archive"], 1);
    server.add_folder("Junk", &["\\Junk"], 1);
    server.add_folder("Trash", &["\\Trash"], 1);
    let ai = FakeAi::start().await;
    let alerts: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = alerts.clone();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_ai_url(&ai.url)
    .with_alert_sender(Arc::new(move |a: &mailcore::alerts::Alert| {
        log.lock().unwrap().push(a.second_line().to_string());
    }));
    let engine = match paid {
        Some(paid) => engine.with_paid_ai(paid),
        None => engine,
    };
    let account = engine
        .call(
            "add_account",
            json!({ "email": USER, "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": PASS, "smtp_host": "127.0.0.1", "smtp_port": 1,
                    "smtp_security": "plain" }),
        )
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    Rig {
        server,
        engine,
        ai,
        account,
        alerts,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }
    fn add(&self, id: &str, from: &str, subject: &str, body: &str) {
        self.server
            .add_message_dated("INBOX", &mail(id, from, subject, body), false, NEW_MAIL);
    }
    async fn sync(&self) {
        self.call("sync_account", json!({ "account_id": self.account }))
            .await;
    }
    async fn inbox(&self) -> i64 {
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
    async fn tab(&self, tab: &str) -> Vec<Value> {
        let folder = self.inbox().await;
        self.call("list_threads", json!({ "folder_id": folder, "tab": tab }))
            .await
            .as_array()
            .unwrap()
            .clone()
    }
    async fn subjects(&self, tab: &str) -> Vec<String> {
        self.tab(tab)
            .await
            .iter()
            .map(|t| t["subject"].as_str().unwrap().to_string())
            .collect()
    }
    async fn counts(&self) -> Value {
        self.call("tab_counts", json!({})).await
    }
    async fn unread_counts(&self) -> Value {
        self.call("tab_counts", json!({ "unread": true })).await
    }
}

#[tokio::test]
async fn inbox_sorts_itself_and_reading_doesnt_clear_needs_you() {
    let r = rig().await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send the contract?",
    );
    r.add(
        "b@t",
        "Shop <no-reply@shop.test>",
        "Your receipt",
        "Thanks for buying.",
    );
    r.server.add_message_dated(
        "INBOX",
        "From: News <news@paper.test>\r\nTo: me@test.test\r\nSubject: Weekly\r\nMessage-ID: <c@t>\r\nList-Unsubscribe: <mailto:x@paper.test>\r\n\r\nNews.\r\n",
        false,
        NEW_MAIL,
    );
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("needs_you").await, ["Contract"]);
    assert_eq!(r.subjects("good_to_know").await, ["Your receipt"]);
    assert_eq!(r.subjects("rest").await, ["Weekly"]);
    let row = &r.tab("needs_you").await[0];
    assert_eq!(row["needs_you"], true);
    assert_eq!(row["why"], "Asks you something");
    assert_eq!(r.counts().await["needs_you"], 1);
    assert_eq!(r.unread_counts().await["needs_you"], 1);

    r.call(
        "set_thread_seen",
        json!({ "account_id": r.account, "thread_id": row["thread_id"], "seen": true }),
    )
    .await;
    assert_eq!(
        r.subjects("needs_you").await,
        ["Contract"],
        "reading isn't answering"
    );
    assert_eq!(r.counts().await["needs_you"], 1);
    assert_eq!(
        r.unread_counts().await["needs_you"],
        0,
        "with only unread showing, a read one isn't counted"
    );

    // One click says this one doesn't need them.
    let thread =
        json!({ "account_id": r.account, "thread_id": row["thread_id"], "group": "good_to_know" });
    r.call("thread_priority", thread).await;
    assert!(r.subjects("needs_you").await.is_empty());
    assert_eq!(r.counts().await["needs_you"], 0);
    assert!(r
        .subjects("good_to_know")
        .await
        .contains(&"Contract".to_string()));
    // And another brings it back.
    let thread =
        json!({ "account_id": r.account, "thread_id": row["thread_id"], "group": "needs_you" });
    r.call("thread_priority", thread).await;
    assert_eq!(r.subjects("needs_you").await, ["Contract"]);
    // Other marks coming and going don't lose the choice.
    let pin = json!({ "account_id": r.account, "thread_id": row["thread_id"], "on": true });
    r.call("thread_pin", pin.clone()).await;
    let mut off = pin;
    off["on"] = json!(false);
    r.call("thread_pin", off).await;
    assert_eq!(r.subjects("needs_you").await, ["Contract"]);
}

#[tokio::test]
async fn a_reply_clears_it_and_a_new_question_brings_it_back() {
    let r = rig().await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("needs_you").await, ["Contract"]);

    r.server.add_message_dated(
        "Sent",
        "From: me@test.test\r\nTo: ana@people.test\r\nSubject: Re: Contract\r\nMessage-ID: <s@t>\r\nIn-Reply-To: <a@t>\r\nReferences: <a@t>\r\n\r\nHere.\r\n",
        true,
        "01-Nov-2036 11:00:00 +0000",
    );
    r.sync().await;
    assert!(r.subjects("needs_you").await.is_empty(), "answered");
    assert_eq!(r.counts().await["needs_you"], 0);

    r.server.add_message_dated(
        "INBOX",
        "From: Ana <ana@people.test>\r\nTo: me@test.test\r\nSubject: Re: Contract\r\nMessage-ID: <a2@t>\r\nIn-Reply-To: <s@t>\r\nReferences: <a@t> <s@t>\r\n\r\nGot it, but page 2 is missing.\r\n",
        false,
        "01-Nov-2036 12:00:00 +0000",
    );
    r.sync().await;
    r.sync().await;
    let row = r.tab("needs_you").await;
    assert_eq!(row.len(), 1);
    assert_eq!(row[0]["why"], "Replied to you");
}

#[tokio::test]
async fn doesnt_need_me_covers_what_was_there_and_a_new_reply_brings_it_back() {
    let r = rig().await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    r.sync().await;
    r.sync().await;
    let row = r.tab("needs_you").await[0].clone();
    let thread =
        json!({ "account_id": r.account, "thread_id": row["thread_id"], "group": "good_to_know" });
    r.call("thread_priority", thread).await;
    r.sync().await;
    assert!(r.subjects("needs_you").await.is_empty(), "the choice holds");

    r.server.add_message_dated(
        "Sent",
        "From: me@test.test\r\nTo: ana@people.test\r\nSubject: Re: Contract\r\nMessage-ID: <s@t>\r\nIn-Reply-To: <a@t>\r\nReferences: <a@t>\r\n\r\nWhich one?\r\n",
        true,
        "01-Nov-2036 11:00:00 +0000",
    );
    r.server.add_message_dated(
        "INBOX",
        "From: Ana <ana@people.test>\r\nTo: me@test.test\r\nSubject: Re: Contract\r\nMessage-ID: <a2@t>\r\nIn-Reply-To: <s@t>\r\nReferences: <a@t> <s@t>\r\n\r\nThe new one. Could you sign page 2 too?\r\n",
        false,
        "01-Nov-2036 12:00:00 +0000",
    );
    r.sync().await;
    r.sync().await;
    assert_eq!(
        r.subjects("needs_you").await,
        ["Contract"],
        "their newer reply needs them, whatever was chosen before"
    );
    assert_eq!(r.counts().await["needs_you"], 1);
}

#[tokio::test]
async fn a_sender_can_always_or_never_need_you() {
    let r = rig().await;
    r.add("a@t", "Boss <boss@work.test>", "Hello", "Nice weather.");
    r.sync().await;
    r.sync().await;
    assert!(r.subjects("needs_you").await.is_empty());
    let always = json!({ "address": "BOSS@work.test", "group": "needs_you" });
    r.call("sender_priority", always).await;
    assert_eq!(r.subjects("needs_you").await, ["Hello"]);
    assert_eq!(
        r.tab("needs_you").await[0]["why"],
        "You chose this for this sender"
    );
    // New mail from them follows too.
    r.add("b@t", "Boss <boss@work.test>", "Again", "More weather.");
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("needs_you").await.len(), 2);
    r.call(
        "sender_priority",
        json!({ "address": "boss@work.test", "group": "good_to_know" }),
    )
    .await;
    assert!(r.subjects("needs_you").await.is_empty());
    assert!(r
        .engine
        .call(
            "sender_priority",
            json!({ "address": "x@y", "group": "nope" })
        )
        .await
        .is_err());
}

#[tokio::test]
async fn alerts_for_all_new_mail_until_narrowed_down() {
    let r = rig().await;
    r.sync().await;
    r.add("b@t", "Bo <bo@people.test>", "Hello", "Nice weather.");
    r.sync().await;
    assert!(r.alerts.lock().unwrap().contains(&"Hello".to_string()));

    r.call(
        "set_setting",
        json!({ "key": "alert_which", "value": "important" }),
    )
    .await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    r.add("c@t", "Cy <cy@people.test>", "Hi", "Nice weather.");
    r.sync().await;
    let got = r.alerts.lock().unwrap().clone();
    assert!(got.contains(&"Contract".to_string()), "{got:?}");
    assert!(
        !got.contains(&"Hi".to_string()),
        "a person with nothing to ask doesn't interrupt"
    );
}

#[tokio::test]
async fn a_quick_look_at_the_inbox_finds_new_mail_and_tells_once() {
    let r = rig().await;
    r.sync().await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    let quick = json!({ "account_id": r.account, "inbox_only": true });
    let whole = json!({ "account_id": r.account });
    let (found, _) = tokio::join!(r.call("sync_account", quick), r.call("sync_account", whole),);
    assert_eq!(found["folders"], 1, "{found}");
    assert_eq!(r.subjects("needs_you").await, ["Contract"]);
    let alerts = r.alerts.lock().unwrap().clone();
    assert_eq!(alerts, ["Contract"], "both looks ran, one told");
}

#[tokio::test]
async fn a_quick_look_tells_once_and_still_catches_up_on_the_rest() {
    let r = rig().await;
    r.add(
        "old@t",
        "Ana <ana@people.test>",
        "Old question",
        "Could you look?",
    );
    r.sync().await;
    r.alerts.lock().unwrap().clear();
    let old = r.server.folder_messages("INBOX")[0].uid;
    r.server.set_flag("INBOX", old, "\\Seen", true);
    r.add(
        "new@t",
        "Bo <bo@people.test>",
        "New question",
        "Could you look too?",
    );
    let quick = json!({ "account_id": r.account, "inbox_only": true });
    r.call("sync_account", quick.clone()).await;
    assert_eq!(*r.alerts.lock().unwrap(), ["New question"]);
    assert_eq!(r.unread_counts().await["needs_you"], 1, "read elsewhere");
    r.call("sync_account", quick).await;
    assert_eq!(
        *r.alerts.lock().unwrap(),
        ["New question"],
        "told only once"
    );
    assert_eq!(r.subjects("needs_you").await.len(), 2);
}

#[tokio::test]
async fn ai_only_sees_what_the_level_allows_and_can_correct_the_sorting() {
    let (r, paid) = rig_paid().await;
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    let own = not_quick_replies(&r);
    paid.answer_sort(json!([
        { "index": 0, "tab": "general", "label": "none", "group": "good_to_know",
          "line": "Cold pitch about marketing", "due": "" },
    ]));
    r.sync().await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Question",
        "Could you look at my offer?",
    );
    r.add("b@t", "Bo <bo@people.test>", "Weather", "Nice weather.");
    r.add("c@t", "Shop <no-reply@shop.test>", "Receipt", "Thanks.");
    r.sync().await;
    assert!(paid.sent("sort").is_empty(), "the level is off");
    assert_eq!(r.subjects("needs_you").await, ["Question"]);

    r.call(
        "set_setting",
        json!({ "key": "ai_auto", "value": "important" }),
    )
    .await;
    r.sync().await;
    let sent = paid.sent("sort");
    assert_eq!(sent.len(), 1);
    let sent = sent[0].to_string();
    assert!(
        sent.contains("Question") && sent.contains("Weather"),
        "the unsure ones"
    );
    assert!(
        !sent.contains("Receipt"),
        "a robot's mail isn't worth asking about"
    );
    assert_eq!(
        not_quick_replies(&r),
        own,
        "no sorting went to their own key"
    );
}

#[tokio::test]
async fn ai_can_add_a_line_a_date_and_a_group() {
    let (r, paid) = rig_paid().await;
    r.call(
        "set_setting",
        json!({ "key": "ai_auto", "value": "everything" }),
    )
    .await;
    paid.answer_sort(json!([
        { "index": 0, "tab": "general", "label": "needs_reply", "group": "needs_you",
          "line": "Wants the signed lease by Friday", "due": "2036-11-07" },
    ]));
    r.sync().await;
    r.add("a@t", "Ana <ana@people.test>", "Lease", "Sign this.");
    r.sync().await;
    let row = &r.tab("needs_you").await[0];
    assert_eq!(row["why"], "AI thinks this needs you");
    assert_eq!(row["ai_line"], "Wants the signed lease by Friday");
    assert!(row["due_at"].as_i64().unwrap() > 2_000_000_000);
    // The email itself carries the date too, for the reader's Add to calendar card.
    let mail = r
        .call("get_thread", json!({ "account_id": r.account, "thread_id": row["thread_id"], "folder_id": row["folder_id"] }))
        .await;
    assert_eq!(mail[0]["due_at"], row["due_at"]);
    let brief = r.call("brief", json!({})).await;
    assert_eq!(brief["needs_you"], 1);
}

#[tokio::test]
async fn ai_sorting_needs_dakly_cloud_and_never_uses_their_own_key() {
    let r = rig().await;
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    let own = not_quick_replies(&r);
    r.call(
        "set_setting",
        json!({ "key": "ai_auto", "value": "everything" }),
    )
    .await;
    r.sync().await;
    r.add(
        "a@t",
        "Ana <ana@people.test>",
        "Question",
        "Could you look at my offer?",
    );
    r.sync().await;
    assert_eq!(not_quick_replies(&r), own);
    // The sorting that needs no AI still works.
    assert_eq!(r.subjects("needs_you").await, ["Question"]);
    let row = &r.tab("needs_you").await[0];
    assert_ne!(row["why"], "AI thinks this needs you");
}

#[tokio::test]
async fn the_brief_counts_who_hasnt_answered() {
    let r = rig().await;
    r.sync().await;
    let brief = r.call("brief", json!({})).await;
    assert_eq!(brief["needs_you"], 0);
    assert_eq!(brief["no_reply"], 0);
    let list = r.call("list_later", json!({ "kind": "no_reply" })).await;
    assert_eq!(list, json!([]));
    assert_eq!(
        r.call("list_later", json!({ "kind": "deadlines" })).await,
        json!([])
    );
}

#[tokio::test]
async fn replies_learn_how_the_person_writes() {
    let r = rig().await;
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    r.ai.answer("Note from the person", json!({ "reply": "Sure." }));
    r.server.add_message_dated(
        "Sent",
        "From: me@test.test\r\nTo: bo@x.test\r\nSubject: Hi\r\nMessage-ID: <s1@t>\r\n\r\nCheers mate, sorted that for you.\r\n\r\nOn Monday Bo wrote:\r\n> secret from Bo\r\n",
        true,
        "01-Oct-2036 10:00:00 +0000",
    );
    r.add("a@t", "Ana <ana@people.test>", "Plans", "Lunch?");
    r.sync().await;
    r.sync().await;
    let row = &r.tab("needs_you").await[0];
    let mail = r
        .call("get_thread", json!({ "account_id": r.account, "thread_id": row["thread_id"], "folder_id": row["folder_id"] }))
        .await;
    r.call(
        "ai_write_reply",
        json!({ "message_id": mail[0]["id"], "instruction": "" }),
    )
    .await;
    let sent = r.ai.requests().last().unwrap().to_string();
    assert!(sent.contains("Cheers mate"), "{sent}");
    assert!(!sent.contains("secret from Bo"), "only their own words");
}

#[tokio::test]
async fn deleting_a_folder_keeps_its_mail_in_archive() {
    let r = rig().await;
    r.server.add_folder("Projects", &[], 1);
    r.server.add_folder("Parent", &[], 1);
    r.server.add_folder("Parent/Child", &[], 1);
    r.server.add_message_dated(
        "Projects",
        &mail("p@t", "Ana <ana@people.test>", "Kept", "Keep me."),
        true,
        NEW_MAIL,
    );
    r.sync().await;
    let folders = r
        .call("list_folders", json!({ "account_id": r.account }))
        .await;
    let id_of = |path: &str| {
        folders
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap()["id"]
            .clone()
    };
    let call = |id: Value| json!({ "account_id": r.account, "folder_id": id });

    // Built-in folders, and ones holding other folders, stay.
    for path in ["INBOX", "Archive", "Trash", "Parent"] {
        assert!(
            r.engine
                .call("delete_folder", call(id_of(path)))
                .await
                .is_err(),
            "{path}"
        );
    }
    let done = r.call("delete_folder", call(id_of("Projects"))).await;
    assert_eq!(done["moved"], 1);
    assert_eq!(
        r.server.folder_messages("Archive").len(),
        1,
        "the email is safe"
    );
    let after = r
        .call("list_folders", json!({ "account_id": r.account }))
        .await;
    assert!(!after
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "Projects"));
    assert!(
        r.engine
            .call("delete_folder", call(id_of("Projects")))
            .await
            .is_err(),
        "already gone"
    );
}

#[tokio::test]
async fn email_pictures_come_through_the_app_but_only_pictures() {
    use axum::{http::header, routing::get, Router};
    let app = Router::new()
        .route(
            "/a.png",
            get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![1u8, 2, 3]) }),
        )
        .route(
            "/page",
            get(|| async { ([(header::CONTENT_TYPE, "text/html")], "<script>") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let new_engine = || {
        Engine::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryStore::default()),
        )
    };

    let engine = new_engine().with_local_images();
    let (kind, bytes) = engine
        .email_image(&format!("http://{addr}/a.png"))
        .await
        .unwrap();
    assert_eq!(kind, "image/png");
    assert_eq!(bytes, [1, 2, 3]);
    assert!(
        engine
            .email_image(&format!("http://{addr}/page"))
            .await
            .is_err(),
        "not a picture"
    );
    assert!(engine
        .email_image(&format!("http://{addr}/none.png"))
        .await
        .is_err());
    assert!(engine.email_image("file:///etc/passwd").await.is_err());

    let real = new_engine();
    assert!(
        real.email_image(&format!("http://{addr}/a.png"))
            .await
            .is_err(),
        "this machine is off limits"
    );
}

#[tokio::test]
async fn the_ai_brief_is_made_from_what_needs_you_and_kept_for_the_day() {
    let (r, paid) = rig_paid().await;
    r.sync().await;
    r.add(
        "brief1@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    r.sync().await;
    assert_eq!(
        r.call("ai_brief", json!({ "saved_only": true })).await,
        Value::Null
    );
    paid.answer_brief("Ana wants the contract from you.");
    let brief = r.call("ai_brief", json!({ "language": "Deutsch" })).await;
    assert_eq!(brief["text"], "Ana wants the contract from you.");
    assert_eq!(brief["count"], 1);
    let sent = paid.sent("brief");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["language"], "Deutsch");
    let items = sent[0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["subject"], "Contract");
    assert!(items[0]["from"].as_str().unwrap().contains("Ana"));
    assert!(
        !items[0]["line"].as_str().unwrap().contains('\r'),
        "only its one line, not the whole email"
    );

    // Kept: asking again, or showing the saved one, doesn't ask again.
    r.call("ai_brief", json!({ "language": "Deutsch" })).await;
    let saved = r
        .call(
            "ai_brief",
            json!({ "saved_only": true, "language": "Deutsch" }),
        )
        .await;
    assert_eq!(saved["text"], "Ana wants the contract from you.");
    assert_eq!(paid.sent("brief").len(), 1);
}

#[tokio::test]
async fn the_brief_and_questions_come_with_dakly_cloud_and_never_use_their_own_key() {
    let r = rig().await;
    r.sync().await;
    r.add(
        "brief1@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    r.sync().await;
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    let own = not_quick_replies(&r);
    let refused = r.engine.call("ai_brief", json!({})).await.unwrap_err();
    assert_eq!(refused.code(), "paid_only");
    assert_eq!(refused.to_string(), "This comes with Mailzo Cloud.");
    let found = r
        .call(
            "ai_ask_sources",
            json!({ "question": "contract", "filters": { "words": ["contract"] } }),
        )
        .await;
    let ids: Vec<Value> = found["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].clone())
        .collect();
    assert_eq!(ids.len(), 1, "finding the emails stays free: {found}");
    let refused = r
        .engine
        .call("ai_ask", json!({ "question": "what contract", "ids": ids }))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "paid_only");
    assert_eq!(r.ai.requests().len(), own, "nothing went to their own key");
    assert_eq!(r.call("paid_status", json!({})).await["paid"], false);

    // Plugged in but not active says the same.
    let (r, paid) = rig_paid().await;
    assert_eq!(r.call("paid_status", json!({})).await["paid"], true);
    paid.active
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(r.call("paid_status", json!({})).await["paid"], false);
    r.sync().await;
    r.add(
        "brief2@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send it?",
    );
    r.sync().await;
    let refused = r.engine.call("ai_brief", json!({})).await.unwrap_err();
    assert_eq!(refused.code(), "paid_only");
    assert_eq!(paid.everything_sent(), "");
}

#[test]
fn bank_alerts_and_receipts_never_ask_you_something() {
    let (mine, to) = to_me();
    let mut facts = base(&mine, &to);
    facts.from = "ucb.retail@ucb.com.bd";
    facts.subject = "Transaction Alert for UCB Credit Card";
    facts.text = "Dear Customer, Greetings from UCB Cards! Thank you for using your card. \
                  For any query, please call us. Need help?";
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    // A real question from a person still needs them.
    facts.from = "ana@people.test";
    facts.subject = "Can you look at this?";
    facts.text = "Could you send it by Friday?";
    assert_eq!(decide(&facts).group, NEEDS_YOU);
}

#[test]
fn cold_sales_emails_and_transfer_alerts_are_only_good_to_know() {
    let (mine, to) = to_me();
    let mut facts = base(&mine, &to);
    for (subject, text) in [
        (
            "Horizon Shine Exterior Cleaning question",
            "Hey there — came across Horizon Shine Exterior Cleaning and wondered, are you taking new jobs?",
        ),
        (
            "idea for Anchor Down Renovations",
            "Andres, any interest? There's no fee for signing up, and it brings jobs to you.",
        ),
        (
            "compare it with a recent bid",
            "<p style=\"margin: 0 0 14px 0;\">bring a job you have already quoted?</p>",
        ),
        (
            "Maler Müller Horgenzell: Ist das Thema Bewertungen gerade relevant?",
            "Guten Morgen, wie sammeln Sie aktuell Bewertungen?",
        ),
    ] {
        facts.subject = subject;
        facts.text = text;
        assert_eq!(decide(&facts).group, GOOD_TO_KNOW, "{subject}");
    }
    // A stranger writing again without an answer is a follow-up chain.
    facts.subject = "Checking in";
    facts.text = "Did you get a chance to look?";
    facts.chases = true;
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    facts.chases = false;
    assert_eq!(decide(&facts).group, NEEDS_YOU);
    // A bank alert whose subject says nothing about money.
    facts.from = "citytouch@thecitybank.com";
    facts.subject = "You made a Wallet Transfer";
    facts.text =
        "Transaction Alert\nYou have transferred BDT 5,000.00 to 017... Questions? Call us.";
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    // Someone you've written to still needs you, whatever words they use.
    facts.from = "ana@people.test";
    facts.is_contact = true;
    facts.subject = "Quick question about your website";
    facts.text = "Could you send the logo?";
    assert_eq!(decide(&facts).group, NEEDS_YOU);
}

#[tokio::test]
async fn a_real_cold_email_never_reaches_needs_you() {
    let r = rig().await;
    r.add(
        "zoe@t",
        "Zoe Hartley <zoe@hartleyoutreach.com>",
        "Horizon Shine Exterior Cleaning question",
        "Hey there \u{2014} came across Horizon Shine Exterior Cleaning on Google Maps.\r\n\r\nQuick q \u{2014} how often does someone stop a service to answer the phone?\r\n\r\nLet me know if you want to check it out.\r\n",
    );
    r.sync().await;
    r.sync().await;
    assert!(r.subjects("needs_you").await.is_empty());
    assert_eq!(
        r.subjects("good_to_know").await,
        ["Horizon Shine Exterior Cleaning question"]
    );
}

#[tokio::test]
async fn sorting_new_mail_isnt_counted_as_a_change_on_every_refresh() {
    let r = rig().await;
    r.add(
        "q@t",
        "Ana <ana@people.test>",
        "Contract",
        "Could you send the contract?",
    );
    r.sync().await;
    let again = r
        .call("sync_account", json!({ "account_id": r.account }))
        .await;
    assert_eq!(again["filtered"], 0, "{again}");
    assert_eq!(r.subjects("needs_you").await, ["Contract"]);
}

#[test]
fn codes_links_mailings_and_seo_pitches_dont_need_you() {
    let (mine, to) = to_me();
    let mut facts = base(&mine, &to);
    facts.auth = "pass";
    facts.from = "agent@wyomingagents.com";
    facts.subject = "Your Verification Code - Expires in 15 Minutes";
    facts.text = "Use the code below to continue. Questions?";
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    facts.from = "support@anthropic.test";
    facts.subject = "Your secure link to Claude is here";
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    // A real deadline from a proven sender still needs them.
    facts.subject = "Your domains will expire in 24 hours";
    assert_eq!(decide(&facts).group, NEEDS_YOU);
    // A sales pitch about search rankings.
    facts.auth = "none";
    facts.from = "caitlin@people.test";
    facts.subject = "Above All Roofing Professionals x Caitlin";
    facts.text =
        "Hi there, I can rank Above All Roofing Professionals above its rivals. Interested?";
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
    // Their own newsletter, from an address they've written to.
    facts.is_contact = true;
    facts.from = "farhan@framerbite.test";
    facts.subject = "Fresh this week: 4 Framer templates";
    facts.text = "Hi Zahirul, what do you think? Unsubscribe here.";
    assert_eq!(decide(&facts).group, GOOD_TO_KNOW);
}
