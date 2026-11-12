//! Spam protection and smart sorting, through the real engine.

use fakeai::{FakeAi, GOOD_KEY};
use fakeimap::FakeServer;
use mailcore::alerts::Alert;
use mailcore::spam::{
    check_mail, read_signals, signature_verdict, spam_probability, Mail, SPAM_AT,
};
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[path = "support/fake_paid.rs"]
mod fake_paid;
use fake_paid::FakePaid;

const ME: &str = "me@grayic.test";
const PASS: &str = "secret-pass";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    ai: FakeAi,
    account: i64,
    alerts: Arc<Mutex<Vec<String>>>,
    opened: Arc<Mutex<Vec<String>>>,
}

fn mail(from: &str, id: &str, subject: &str, extra: &str, body: &str) -> String {
    format!(
        "From: {from}\nTo: {ME}\nSubject: {subject}\nMessage-ID: <{id}>\n{extra}Content-Type: text/plain\n\n{body}\n"
    )
}

/// An email that's plainly a scam: the address imitates PayPal.
fn scam(id: &str) -> String {
    mail(
        "PayPal <service@paypa1.com>",
        id,
        "Your account has been suspended",
        "",
        "Click here to verify your account.",
    )
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
    let imap = FakeServer::start(ME, PASS).await;
    imap.add_folder("INBOX", &[], 1);
    imap.add_folder("Sent", &["\\Sent"], 1);
    imap.add_folder("Archive", &["\\Archive"], 1);
    imap.add_folder("Junk", &["\\Junk"], 1);
    imap.add_folder("Trash", &["\\Trash"], 1);
    let ai = FakeAi::start().await;
    let alerts: Arc<Mutex<Vec<String>>> = Arc::default();
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let (log, open_log) = (alerts.clone(), opened.clone());
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_sent_copy_wait(0)
    .with_ai_url(&ai.url)
    .with_url_opener(Arc::new(move |u: &str| {
        open_log.lock().unwrap().push(u.to_string())
    }))
    .with_alert_sender(Arc::new(move |a: &Alert| {
        log.lock().unwrap().push(a.title.clone())
    }));
    let engine = match paid {
        Some(paid) => engine.with_paid_ai(paid),
        None => engine,
    };
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
    let r = Rig {
        engine,
        imap,
        ai,
        account: added["id"].as_i64().unwrap(),
        alerts,
        opened,
    };
    // A first look, so later mail counts as new.
    r.sync().await;
    r
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }
    async fn sync(&self) -> Value {
        self.call("sync_account", json!({ "account_id": self.account }))
            .await
    }
    async fn folder(&self, role: &str) -> Value {
        let folders = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        folders
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == role)
            .unwrap()
            .clone()
    }
    async fn subjects(&self, role: &str, tab: Option<&str>) -> Vec<String> {
        let f = self.folder(role).await;
        let list = self
            .call("list_threads", json!({ "folder_id": f["id"], "tab": tab }))
            .await;
        let mut names: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["subject"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }
    async fn row(&self, role: &str, subject: &str) -> Value {
        let f = self.folder(role).await;
        let list = self
            .call("list_threads", json!({ "folder_id": f["id"] }))
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["subject"] == subject)
            .unwrap_or_else(|| panic!("{subject} not in {role}: {list}"))
            .clone()
    }
    async fn threads(&self, role: &str, subject: &str) -> Value {
        let t = self.row(role, subject).await;
        json!([{ "account_id": t["account_id"], "thread_id": t["thread_id"], "folder_id": t["folder_id"] }])
    }
    async fn setting(&self, key: &str, value: &str) {
        self.call("set_setting", json!({ "key": key, "value": value }))
            .await;
    }
    async fn message(&self, role: &str, subject: &str) -> Value {
        let t = self.row(role, subject).await;
        let list = self
            .call(
                "get_thread",
                json!({ "account_id": t["account_id"], "thread_id": t["thread_id"], "folder_id": t["folder_id"] }),
            )
            .await;
        list[0].clone()
    }
}

fn plain<'a>(from_name: &'a str, from_address: &'a str) -> Mail<'a> {
    Mail {
        from_name,
        from_address,
        reply_to: &[],
        subject: "Hello",
        text: "A normal note",
        html: "",
        auth: "none",
        provider_spam: false,
    }
}

fn is_scam(mail: &Mail) -> bool {
    check_mail(mail, &BTreeSet::new(), None)
        .1
        .iter()
        .any(|r| r.scam)
}

#[test]
fn only_the_providers_own_note_about_the_sender_counts() {
    let header = |notes: &str| format!("{notes}From: a@x.test\nSubject: Hi\n\n");
    let auth = |notes: &str| read_signals(header(notes).as_bytes(), "a@x.test").auth;
    assert_eq!(auth(""), "none");
    assert_eq!(
        auth("Authentication-Results: mx.test; dmarc=pass; dkim=pass; spf=pass\n"),
        "pass"
    );
    assert_eq!(
        auth("Authentication-Results: mx.test; dmarc=fail\n"),
        "fail"
    );
    assert_eq!(
        auth("Authentication-Results: mx.test; dkim=fail; spf=pass\n"),
        "weak"
    );
    assert_eq!(
        auth("Authentication-Results: mx.test;\n dmarc=fail (p=reject);\n dkim=none\n"),
        "fail",
        "folded"
    );
    // A sender can write their own note lower down. The top one is the provider's.
    assert_eq!(
        auth("Authentication-Results: mx.test; dmarc=fail\nAuthentication-Results: fake; dmarc=pass\n"),
        "fail"
    );
}

#[test]
fn headers_say_what_was_mass_mailed() {
    let sort = |extra: &str, from: &str| {
        read_signals(format!("From: {from}\n{extra}\n").as_bytes(), from).category
    };
    assert_eq!(sort("", "maya@x.test"), "general");
    assert_eq!(sort("", "no-reply@x.test"), "general", "codes and notices");
    assert_eq!(sort("Feedback-ID: 1:2:3\n", "billing@host.test"), "general");
    assert_eq!(
        sort("Auto-Submitted: auto-generated\n", "system@x.test"),
        "general"
    );
    assert_eq!(
        sort("List-Unsubscribe: <https://x.test/u>\n", "news@x.test"),
        "mass"
    );
    assert_eq!(
        sort("List-Unsubscribe: <https://x.test/u>\n", "no-reply@x.test"),
        "mass"
    );
    assert_eq!(sort("Precedence: bulk\n", "a@x.test"), "mass");
    assert_eq!(
        sort("List-Id: <repo.github.com>\n", "notifications@github.test"),
        "general"
    );

    let s = read_signals(
        b"From: a@x.test\nList-Unsubscribe: <mailto:u@x.test>, <https://x.test/u>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\nX-Spam-Flag: YES\n\n",
        "a@x.test",
    );
    assert_eq!(
        s.unsubscribe.as_deref(),
        Some("https://x.test/u"),
        "the web link first"
    );
    assert!(s.one_click);
    assert!(s.provider_spam);
}

#[test]
fn mass_mail_goes_where_a_person_would_put_it() {
    use mailcore::spam::sort_mass_mail as sort;
    // Words inside other words don't count.
    assert_eq!(sort("a@x.test", "Good design in 2026"), "promotions");
    assert_eq!(sort("news@x.test", "The ideal setup"), "newsletters");
    assert_eq!(
        sort("news@x.test", "Border radius done right"),
        "newsletters"
    );
    assert_eq!(sort("news@x.test", "Claude Code tips"), "newsletters");
    // Their real forms do, including plain endings and punctuation.
    assert_eq!(sort("a@x.test", "Your order has shipped"), "general");
    assert_eq!(sort("a@x.test", "Please confirm your email"), "general");
    assert_eq!(sort("a@x.test", "Your email is confirmed"), "general");
    assert_eq!(sort("a@x.test", "Your code is 1234"), "general");
    assert_eq!(sort("a@x.test", "Sign-in attempt"), "general");
    assert_eq!(sort("a@x.test", "20% off today"), "promotions");
    assert_eq!(sort("a@x.test", "Our best deals"), "promotions");
    // The real emails that landed in the wrong place.
    assert_eq!(
        sort(
            "a@m.relayfi.com",
            "Firstbase Inc. has invited you to Bank with Relay"
        ),
        "general"
    );
    assert_eq!(
        sort(
            "a@m.relayfi.com",
            "Firstbase Inc. is waiting for you to accept your invite"
        ),
        "general"
    );
    assert_eq!(sort("a@relayfi.com", "Verify your email"), "general");
    assert_eq!(
        sort("a@feedback.dnb.com", "Reminder: Make your voice heard"),
        "general"
    );
    assert_eq!(
        sort("a@feedback.dnb.com", "We'd love to hear from you"),
        "general"
    );
    assert_eq!(
        sort(
            "a@info.vercel.com",
            "Deploying on Vercel is as easy as picking a template"
        ),
        "promotions"
    );
    assert_eq!(
        sort(
            "a@info.vercel.com",
            "Ship your first Vercel deployment in minutes"
        ),
        "promotions"
    );
    assert_eq!(
        sort("a@email.openai.com", "A simpler way to put ChatGPT to work"),
        "promotions"
    );

    assert_eq!(
        sort("news@shop.test", "Our spring collection"),
        "newsletters"
    );
    assert_eq!(
        sort("a@paper.substack.com", "Why cities are changing"),
        "newsletters"
    );
    assert_eq!(sort("a@x.test", "Your weekly digest"), "newsletters");
    assert_eq!(sort("a@shop.test", "Your order has shipped"), "general");
    assert_eq!(
        sort("a@shop.test", "20% off everything today"),
        "promotions"
    );
    assert_eq!(
        sort("a@shop.test", "Use promo code SPRING"),
        "promotions",
        "not a sign-in code"
    );
}

#[test]
fn lookalike_addresses_and_borrowed_names_are_caught() {
    for fake in [
        "service@paypa1.com",
        "service@pаypal.com",
        "service@xn--pypal-4ve.com",
        "login@paypal.com.secure-check.test",
        "help@arnazon.com",
        "help@g00gle.com",
    ] {
        assert!(is_scam(&plain("", fake)), "{fake} should be caught");
    }
    for real in [
        "service@paypal.com",
        "service@mail.paypal.com",
        "orders@amazon.de",
        "orders@amazon.co.uk",
        "friend@example.test",
        "no-reply@accounts.google.com",
    ] {
        assert!(!is_scam(&plain("", real)), "{real} is genuine");
    }

    assert!(is_scam(&plain("PayPal", "x@evil.test")));
    assert!(is_scam(&plain("Apple Support Team", "x@evil.test")));
    assert!(
        is_scam(&plain("support@paypal.com", "x@evil.test")),
        "an address as a name"
    );
    assert!(!is_scam(&plain("PayPal", "service@paypal.com")));
    assert!(
        !is_scam(&plain("Sam Apple", "sam@orchard.test")),
        "just a surname"
    );
    assert!(!is_scam(&plain(
        "Google Ads tips from Sam",
        "sam@agency.test"
    )));

    // A domain made to look like someone they really deal with.
    let known: BTreeSet<String> = ["grayic.com".to_string()].into();
    assert!(check_mail(&plain("", "boss@grayıc.com"), &known, None)
        .1
        .iter()
        .any(|r| r.scam));
    assert!(!check_mail(&plain("", "boss@grayic.com"), &known, None)
        .1
        .iter()
        .any(|r| r.scam));
}

#[test]
fn a_link_pretending_to_go_somewhere_else_is_caught() {
    let html = r#"<p>Sign in at <a href="https://evil.test/login">www.paypal.com</a> now</p>"#;
    let mail = Mail {
        html,
        ..plain("", "x@shop.test")
    };
    assert!(is_scam(&mail));
    let honest = r#"<a href="https://www.paypal.com/x">paypal.com</a> <a href="https://t.test/1">Read more</a>"#;
    assert!(!is_scam(&Mail {
        html: honest,
        ..plain("", "x@shop.test")
    }));
    let tracked = r#"<a href="https://click.shop.test/1">shop.test</a>"#;
    assert!(!is_scam(&Mail {
        html: tracked,
        ..plain("", "x@shop.test")
    }));
}

#[test]
fn learning_leans_towards_keeping_mail() {
    assert!(spam_probability(&[(40, 0), (35, 1), (30, 0)], 50, 50) > 0.99);
    assert!(spam_probability(&[(0, 40), (1, 35), (0, 30)], 50, 50) < 0.01);
    assert!(
        spam_probability(&[], 50, 50) > 0.4,
        "nothing known says nothing"
    );
    // Seen equally in both: good mail counts double.
    assert!(spam_probability(&[(20, 20), (20, 20)], 50, 50) < 0.5);
}

#[tokio::test]
async fn an_email_with_no_signature_gives_no_verdict() {
    assert_eq!(
        signature_verdict(b"From: a@x.test\nSubject: Hi\n\nHello\n", "x.test").await,
        None
    );
}

#[tokio::test]
async fn the_trial_moves_nothing_and_shows_what_it_would_do() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail("Old <old@x.test>", "0@x", "Old", "", "Earlier"),
        true,
    );
    r.sync().await;
    r.imap.add_message("INBOX", &scam("1@x"), false);
    r.imap.add_message(
        "INBOX",
        &mail("Maya <maya@x.test>", "2@x", "Lunch", "", "Are you free?"),
        false,
    );
    r.sync().await;

    assert_eq!(
        r.subjects("inbox", None).await.len(),
        3,
        "still all in the inbox"
    );
    assert!(r.subjects("junk", None).await.is_empty());
    let row = r.row("inbox", "Your account has been suspended").await;
    assert_eq!(row["risk"], 2);
    assert_eq!(r.row("inbox", "Lunch").await["risk"], 0);

    let report = r.call("spam_report", json!({})).await;
    assert_eq!(report["mode"], "trial");
    assert_eq!(report["would_move"].as_array().unwrap().len(), 1);
    let reasons = report["would_move"][0]["reasons"].to_string();
    assert!(reasons.contains("paypa1.com"), "{reasons}");

    let shown = r.message("inbox", "Your account has been suspended").await;
    assert!(
        shown["warnings"].to_string().contains("made to look like"),
        "{shown}"
    );
    assert_eq!(*r.alerts.lock().unwrap(), ["Maya"], "spam never pops up");
}

#[tokio::test]
async fn once_turned_on_spam_goes_to_junk_with_its_reason() {
    let r = rig().await;
    r.imap.add_message("INBOX", &scam("1@x"), false);
    r.sync().await;
    r.setting("spam_mode", "move").await;
    r.sync().await;
    assert!(r.subjects("inbox", None).await.is_empty());
    r.sync().await;
    assert_eq!(
        r.subjects("junk", None).await,
        ["Your account has been suspended"]
    );
    let shown = r.message("junk", "Your account has been suspended").await;
    assert!(
        shown["warnings"].to_string().contains("made to look like"),
        "{shown}"
    );
    assert_eq!(
        r.imap
            .state
            .lock()
            .unwrap()
            .folders
            .iter()
            .find(|f| f.name == "Junk")
            .unwrap()
            .messages
            .len(),
        1,
        "moved on the server too, so the provider learns"
    );

    // Moved back by hand, it stays: the filter doesn't keep arguing.
    let t = r.threads("junk", "Your account has been suspended").await;
    r.call(
        "bulk_threads",
        json!({ "threads": t, "action": "move", "to": "inbox" }),
    )
    .await;
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("inbox", None).await.len(), 1);
}

#[tokio::test]
async fn people_youve_written_to_are_never_spam() {
    let r = rig().await;
    r.imap.add_message(
        "Sent",
        &format!(
            "From: {ME}\nTo: Friend <friend@paypa1.com>\nSubject: Hi\nMessage-ID: <s@x>\n\nHi\n"
        ),
        true,
    );
    r.sync().await;
    r.setting("spam_mode", "move").await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "Friend <friend@paypa1.com>",
            "1@x",
            "WINNER WINNER WINNER!!!",
            "",
            "you have won the lottery",
        ),
        false,
    );
    r.sync().await;
    r.sync().await;
    let row = r.row("inbox", "WINNER WINNER WINNER!!!").await;
    assert_eq!(row["risk"], 1, "still warned about the address, but kept");
    assert!(r.subjects("junk", None).await.is_empty());
    assert_eq!(r.subjects("inbox", Some("general")).await.len(), 1);
}

#[tokio::test]
async fn not_spam_brings_it_back_and_trusts_the_sender() {
    let r = rig().await;
    r.setting("spam_mode", "move").await;
    r.imap.add_message("INBOX", &scam("1@x"), false);
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("junk", None).await.len(), 1);

    let t = r.threads("junk", "Your account has been suspended").await;
    r.call("spam_clear", json!({ "threads": t })).await;
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("inbox", None).await.len(), 1);
    assert!(r.subjects("junk", None).await.is_empty());
    let shown = r.message("inbox", "Your account has been suspended").await;
    assert_eq!(shown["warnings"], json!([]));
    assert_eq!(
        r.call("spam_lists", json!({})).await["trusted"],
        json!(["service@paypa1.com"])
    );

    r.imap.add_message("INBOX", &scam("2@x"), false);
    r.sync().await;
    r.sync().await;
    assert_eq!(
        r.subjects("junk", None).await.len(),
        0,
        "their next email stays too"
    );
}

#[tokio::test]
async fn blocking_works_even_during_the_trial_and_can_be_undone() {
    let r = rig().await;
    r.setting("alert_which", "people").await;
    r.imap.add_message(
        "INBOX",
        &mail("Old <old@x.test>", "0@x", "Old", "", "Earlier"),
        true,
    );
    r.sync().await;
    r.imap.add_message(
        "INBOX",
        &mail("Pest <pest@x.test>", "1@x", "First", "", "Hello"),
        false,
    );
    r.sync().await;
    let t = r.threads("inbox", "First").await;
    r.call("spam_mark", json!({ "threads": t, "block": true }))
        .await;
    r.sync().await;
    assert_eq!(
        r.call("spam_lists", json!({})).await["blocked"],
        json!(["pest@x.test"])
    );
    assert_eq!(r.subjects("junk", None).await, ["First"]);

    r.imap.add_message(
        "INBOX",
        &mail("Pest <pest@x.test>", "2@x", "Second", "", "Hello again"),
        false,
    );
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("inbox", None).await, ["Old"]);
    assert_eq!(r.subjects("junk", None).await.len(), 2);
    assert_eq!(
        r.alerts
            .lock()
            .unwrap()
            .iter()
            .filter(|a| *a == "Pest")
            .count(),
        1,
        "the first email, before the block, and no more"
    );

    r.call(
        "spam_unlist",
        json!({ "address": "pest@x.test", "kind": "blocked" }),
    )
    .await;
    r.imap.add_message(
        "INBOX",
        &mail("Pest <pest@x.test>", "3@x", "Third", "", "Hello once more"),
        false,
    );
    r.sync().await;
    r.sync().await;
    assert_eq!(r.subjects("inbox", None).await, ["Old", "Third"]);
}

#[tokio::test]
async fn a_problem_moving_spam_never_costs_the_refresh() {
    let r = rig().await;
    r.setting("spam_mode", "move").await;
    r.imap.add_message("INBOX", &scam("1@x"), false);
    r.imap.add_message(
        "INBOX",
        &mail("Maya <maya@x.test>", "2@x", "Lunch", "", "Hi"),
        false,
    );
    r.imap
        .state
        .lock()
        .unwrap()
        .folders
        .retain(|f| f.name != "Junk");
    let stats = r.sync().await;
    assert_eq!(
        stats["errors"],
        json!([]),
        "with no Junk folder, one is made"
    );
    assert_eq!(r.subjects("inbox", None).await, ["Lunch"]);
}

#[tokio::test]
async fn it_learns_from_junk_and_from_mail_it_passed() {
    let r = rig().await;
    r.imap.add_message(
        "Sent",
        &format!("From: {ME}\nTo: Maya <maya@x.test>\nSubject: Hi\nMessage-ID: <s@x>\n\nHi\n"),
        true,
    );
    r.sync().await;
    for n in 0..25 {
        r.imap.add_message(
            "Junk",
            &mail(
                "Cheap <deals@spammy.test>",
                &format!("j{n}@x"),
                "cheap replica watches",
                "",
                "cheap replica watches discount deal bargain",
            ),
            true,
        );
        r.imap.add_message(
            "INBOX",
            &mail(
                "Maya <maya@x.test>",
                &format!("g{n}@x"),
                "project meeting notes",
                "",
                "project meeting agenda tomorrow thanks",
            ),
            true,
        );
    }
    r.sync().await;
    r.sync().await;
    let report = r.call("spam_report", json!({})).await;
    assert_eq!(report["examples"]["spam"], 25);
    assert_eq!(report["examples"]["good"], 25);

    r.imap.add_message(
        "INBOX",
        &mail(
            "New <a@other.test>",
            "n1@x",
            "cheap replica discount",
            "",
            "cheap replica watches deal bargain",
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "New <b@other.test>",
            "n2@x",
            "meeting notes",
            "",
            "project agenda for the meeting tomorrow, thanks",
        ),
        false,
    );
    r.sync().await;
    assert_eq!(
        r.row("inbox", "cheap replica discount").await["risk"],
        0,
        "reading like Junk isn't enough by itself"
    );
    assert_eq!(r.row("inbox", "meeting notes").await["risk"], 0);

    // Correcting it counts: the example moves across, not added twice.
    let t = r.threads("inbox", "cheap replica discount").await;
    r.call("spam_clear", json!({ "threads": t })).await;
    let report = r.call("spam_report", json!({})).await;
    assert_eq!(report["examples"]["good"], 26);
    assert_eq!(report["examples"]["spam"], 25);
    assert_eq!(report["would_move"], json!([]));
}

#[tokio::test]
async fn mail_sorts_into_tabs_and_the_numbers_add_up() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail("Maya <maya@x.test>", "1@x", "Lunch", "", "Hi"),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Shop <news@shop.test>",
            "2@x",
            "Sale",
            "List-Unsubscribe: <https://shop.test/u>\n",
            "Sale",
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Bank <no-reply@bank.test>",
            "3@x",
            "Your code",
            "",
            "123456",
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Bank <no-reply@bank.test>",
            "4@x",
            "Your other code",
            "",
            "654321",
        ),
        true,
    );
    r.sync().await;

    assert_eq!(
        r.subjects("inbox", Some("general")).await,
        ["Lunch", "Your code", "Your other code"],
        "codes are about the person's own dealings"
    );
    assert_eq!(r.subjects("inbox", Some("newsletters")).await, ["Sale"]);
    assert!(r.subjects("inbox", Some("promotions")).await.is_empty());
    assert_eq!(
        r.subjects("inbox", None).await.len(),
        4,
        "no tab means everything"
    );

    let counts = r.call("tab_counts", json!({})).await;
    assert_eq!(
        counts,
        json!({ "general": 2, "newsletters": 1, "promotions": 0, "needs_you": 0, "good_to_know": 2, "rest": 1 })
    );
    assert_eq!(
        r.folder("inbox").await["unread"],
        3,
        "the same total as the sidebar"
    );

    // Moving a sender.
    r.call(
        "sort_sender",
        json!({ "address": "News@Shop.test", "category": "general" }),
    )
    .await;
    assert_eq!(
        r.subjects("inbox", Some("general")).await,
        ["Lunch", "Sale", "Your code", "Your other code"]
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Shop <news@shop.test>",
            "5@x",
            "Sale two",
            "List-Unsubscribe: <https://shop.test/u>\n",
            "Sale",
        ),
        false,
    );
    r.sync().await;
    assert!(
        r.subjects("inbox", Some("general"))
            .await
            .contains(&"Sale two".to_string()),
        "and their next email"
    );

    // A newsletter you've replied to is a conversation with a person now.
    r.imap.add_message(
        "Sent",
        &format!("From: {ME}\nTo: x@bank.test\nSubject: Re: Your code\nMessage-ID: <s@x>\nIn-Reply-To: <3@x>\nReferences: <3@x>\n\nThanks\n"),
        true,
    );
    r.sync().await;
    assert!(r
        .subjects("inbox", Some("general"))
        .await
        .contains(&"Your code".to_string()));
    assert!(!r
        .subjects("inbox", Some("promotions"))
        .await
        .contains(&"Your code".to_string()));
}

#[tokio::test]
async fn a_newsletter_that_returns_to_the_inbox_is_still_a_newsletter() {
    let r = rig().await;
    let digest = mail(
        "Digest <news@digest.test>",
        "1@digest.test",
        "This week in design",
        "List-Unsubscribe: <https://digest.test/stop>\n",
        "Five ideas",
    );
    r.imap.add_message("INBOX", &digest, false);
    r.sync().await;
    // A second arrival of the same email, as when it comes back from Archive.
    r.imap.add_message("INBOX", &digest, false);
    r.sync().await;
    assert_eq!(
        r.subjects("inbox", Some("newsletters")).await,
        ["This week in design"]
    );
    assert!(r.subjects("inbox", Some("promotions")).await.is_empty());
}

#[tokio::test]
async fn unsubscribing_opens_their_page_and_never_for_suspect_mail() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "Shop <a@shop.test>",
            "1@x",
            "Web",
            "List-Unsubscribe: <https://shop.test/u?x=1>\n",
            "x",
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail("Maya <maya@x.test>", "3@x", "Plain", "", "x"),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "PayPal <x@paypa1.com>",
            "4@x",
            "Suspect",
            "List-Unsubscribe: <https://evil.test/confirm>
",
            "x",
        ),
        false,
    );
    r.sync().await;

    let web = r.message("inbox", "Web").await;
    assert_eq!(web["can_unsubscribe"], true);
    assert_eq!(r.message("inbox", "Plain").await["can_unsubscribe"], false);
    let reply = r
        .call("unsubscribe", json!({ "message_id": web["id"] }))
        .await;
    assert_eq!(reply["kind"], "opened");
    assert_eq!(*r.opened.lock().unwrap(), ["https://shop.test/u?x=1"]);

    let suspect = r.message("inbox", "Suspect").await;
    let refused = r
        .engine
        .call("unsubscribe", json!({ "message_id": suspect["id"] }))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("suspicious"));
    assert_eq!(
        r.opened.lock().unwrap().len(),
        1,
        "nothing was opened for it"
    );
}

#[tokio::test]
async fn ai_stays_off_until_asked_and_only_sees_an_excerpt() {
    let r = rig().await;
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let asked = r.ai.requests().len();
    let unsure = "Replies please: all capitals here too";
    r.imap.add_message(
        "INBOX",
        &mail(
            "Odd <odd@x.test>",
            "1@x",
            "URGENT ACTION REQUIRED NOW",
            "Reply-To: other@elsewhere.test\n",
            unsure,
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Maya <maya@x.test>",
            "2@x",
            "Lunch",
            "",
            "Are you free for lunch?",
        ),
        false,
    );
    r.sync().await;
    assert_eq!(r.ai.requests().len(), asked, "nothing sent by default");
    assert_eq!(
        r.row("inbox", "URGENT ACTION REQUIRED NOW").await["risk"],
        0
    );
}

#[tokio::test]
async fn an_ai_second_opinion_only_goes_to_the_unsure_ones() {
    let (r, paid) = rig_paid().await;
    // Their own key is never what this uses.
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let own = r.ai.requests().len();
    r.setting("ai_spam", "on").await;
    paid.answer_spam(true, "It pressures you to act.");
    let long_tail = "x".repeat(2000);
    r.imap.add_message(
        "INBOX",
        &mail(
            "Odd <odd@x.test>",
            "1@x",
            "URGENT ACTION REQUIRED NOW",
            "Reply-To: other@elsewhere.test\n",
            &format!("Hello there. {long_tail} SECRET-END"),
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Maya <maya@x.test>",
            "2@x",
            "Lunch",
            "",
            "Are you free for lunch?",
        ),
        false,
    );
    r.imap.add_message("INBOX", &scam("3@x"), false);
    r.sync().await;

    let sent = paid.sent("spam");
    assert_eq!(sent.len(), 1, "only the unsure one: {sent:?}");
    assert_eq!(sent[0]["from"], "odd@x.test");
    assert_eq!(sent[0]["subject"], "URGENT ACTION REQUIRED NOW");
    assert!(sent[0]["language"].is_string());
    let text = sent[0]["text"].as_str().unwrap();
    assert!(text.starts_with("Hello there."));
    assert!(!text.contains("SECRET-END"), "just the start of it");
    assert_eq!(r.ai.requests().len(), own, "nothing went to their own key");
    let row = r.row("inbox", "URGENT ACTION REQUIRED NOW").await;
    assert_eq!(row["risk"], 2);
    let shown = r.message("inbox", "URGENT ACTION REQUIRED NOW").await;
    assert!(shown["warnings"].to_string().contains("It pressures you"));
    assert_eq!(r.row("inbox", "Lunch").await["risk"], 0);
}

#[tokio::test]
async fn without_dakly_cloud_the_second_opinion_and_ai_sorting_quietly_do_nothing() {
    let r = rig().await;
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let own = r.ai.requests().len();
    r.setting("ai_spam", "on").await;
    r.setting("ai_sort", "on").await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "Odd <odd@x.test>",
            "1@x",
            "URGENT ACTION REQUIRED NOW",
            "Reply-To: other@elsewhere.test\n",
            &format!("Hello there. {} SECRET-END", "x".repeat(2000)),
        ),
        false,
    );
    r.sync().await;
    assert_eq!(
        r.ai.requests().len(),
        own,
        "their own key isn't used instead"
    );
    let problem = r.call("spam_report", json!({})).await;
    assert!(
        problem["ai_problem"].is_null(),
        "and it isn't a problem: {problem}"
    );

    // Plugged in but not active is the same.
    let (r, paid) = rig_paid().await;
    paid.active
        .store(false, std::sync::atomic::Ordering::SeqCst);
    r.setting("ai_spam", "on").await;
    r.setting("ai_sort", "on").await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "Odd <odd@x.test>",
            "1@x",
            "URGENT ACTION REQUIRED NOW",
            "Reply-To: other@elsewhere.test\n",
            &format!("Hello there. {} SECRET-END", "x".repeat(2000)),
        ),
        false,
    );
    r.sync().await;
    assert_eq!(paid.everything_sent(), "");
}

#[tokio::test]
async fn ai_sorting_labels_mail_when_turned_on_but_doesnt_outrank_you() {
    let (r, paid) = rig_paid().await;
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let own = r.ai.requests().len();
    r.setting("ai_sort", "on").await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "Shop <orders@shop.test>",
            "1@x",
            "Your order",
            "",
            "Thanks for your purchase",
        ),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail("Fixed <fixed@x.test>", "2@x", "Pinned", "", "Hello"),
        false,
    );
    r.call(
        "sort_sender",
        json!({ "address": "fixed@x.test", "category": "general" }),
    )
    .await;
    paid.answer_sort(json!([
        { "index": 0, "tab": "notifications", "label": "none" },
        { "index": 1, "tab": "notifications", "label": "receipt" },
    ]));
    r.sync().await;
    let sent = paid.sent("sort");
    assert_eq!(sent.len(), 1);
    let items = sent[0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{sent:?}");
    assert!(items.iter().any(|i| i["subject"] == "Your order"
        && i["from"] == "orders@shop.test"
        && i["text"]
            .as_str()
            .unwrap()
            .contains("Thanks for your purchase")));
    assert_eq!(r.ai.requests().len(), own, "nothing went to their own key");
    let order = r.row("inbox", "Your order").await;
    let pinned = r.row("inbox", "Pinned").await;
    // Newest first in the question, so which is which depends on order: check both ways.
    assert!(
        order["label"] == "receipt" || pinned["label"] == "receipt",
        "{order} {pinned}"
    );
    assert!(
        r.subjects("inbox", Some("general"))
            .await
            .contains(&"Pinned".to_string()),
        "your choice stands"
    );
}

/// How real mail would sort. Reads a copy of a mail database, and changes nothing:
/// REAL_DB=/path cargo test -p mailcore --test spam real_mail -- --ignored --nocapture
#[test]
#[ignore]
fn real_mail_sorts_sensibly() {
    let path = std::env::var("REAL_DB").expect("set REAL_DB");
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut st = db
        .prepare(
            "SELECT m.from_addr, m.subject, m.unsubscribe IS NOT NULL OR m.category = 'newsletters' FROM messages m
             JOIN folders f ON f.id = m.folder_id WHERE f.role = 'inbox' ORDER BY m.from_addr, m.date DESC",
        )
        .unwrap();
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (from, subject, mass) = row.unwrap();
        let tab = if mass {
            mailcore::spam::sort_mass_mail(&from, &subject)
        } else {
            "general"
        };
        println!(
            "{tab:12} {mass:5} {from:42} {}",
            subject.chars().take(58).collect::<String>()
        );
    }
}

fn with_words_only<'a>(from: &'a str) -> Mail<'a> {
    Mail {
        from_name: "A Company",
        from_address: from,
        reply_to: &[],
        subject: "Your weekly update",
        text: "Here is what happened this week.",
        html: "",
        auth: "none",
        provider_spam: false,
    }
}

#[test]
fn how_an_email_reads_is_never_enough_to_call_it_spam() {
    let mail = with_words_only("news@company.test");
    let (score, _) = check_mail(&mail, &BTreeSet::new(), Some(0.999));
    assert!(score < SPAM_AT, "wording alone scored {score}");
}

#[test]
fn wording_and_a_failed_sender_check_together_are_spam() {
    let mail = Mail {
        auth: "fail",
        ..with_words_only("news@company.test")
    };
    let (score, _) = check_mail(&mail, &BTreeSet::new(), Some(0.999));
    assert!(score >= SPAM_AT, "scored {score}");
}

#[test]
fn friendly_wording_does_not_excuse_a_forged_sender() {
    let mail = Mail {
        auth: "fail",
        ..with_words_only("news@company.test")
    };
    let (friendly, reasons) = check_mail(&mail, &BTreeSet::new(), Some(0.001));
    let (plain, _) = check_mail(&mail, &BTreeSet::new(), None);
    assert_eq!(friendly, plain);
    assert!(reasons.iter().any(|r| r.scam));
}

#[test]
fn a_senders_own_social_link_is_not_a_trick_but_a_fake_login_is() {
    let social = Mail {
        html: r#"<a href="https://click.company.test/r/1">youtube.com</a>"#,
        ..with_words_only("news@company.test")
    };
    assert!(check_mail(&social, &BTreeSet::new(), None).1.is_empty());
    let login = Mail {
        html: r#"<a href="https://company.test/login">paypal.com</a>"#,
        ..with_words_only("news@company.test")
    };
    assert!(check_mail(&login, &BTreeSet::new(), None)
        .1
        .iter()
        .any(|r| r.scam));
}

#[test]
fn a_checked_senders_social_link_through_its_mail_service_is_not_a_trick() {
    // Customer.io, Mailchimp and the like send every link through their own address.
    let tracked = |shown: &str, auth| Mail {
        html: Box::leak(
            format!(r#"<a href="https://e.customeriomail.com/e/c/abc">{shown}</a>"#)
                .into_boxed_str(),
        ),
        auth,
        ..with_words_only("rayj@tryatria.test")
    };
    let warned = |m: &Mail| {
        check_mail(m, &BTreeSet::new(), None)
            .1
            .iter()
            .any(|r| r.scam)
    };
    assert!(!warned(&tracked("linkedin.com", "pass")));
    // Not when the sender can't be confirmed, and never for a login-style brand.
    assert!(warned(&tracked("linkedin.com", "none")));
    assert!(warned(&tracked("paypal.com", "pass")));
    // A social link to anywhere else is still a trick.
    let elsewhere = Mail {
        html: r#"<a href="https://evil.test/r/1">linkedin.com</a>"#,
        auth: "pass",
        ..with_words_only("rayj@tryatria.test")
    };
    assert!(warned(&elsewhere));
}

/// The same sender and subject each time, so only the body and the arrival can tell two apart.
fn invoice(body: &str) -> String {
    format!(
        "From: Shop <billing@shop.test>\nTo: {ME}\nSubject: Your invoice\nMessage-ID: <reused@x>\nContent-Type: multipart/alternative; boundary=b\n\n--b\nContent-Type: text/plain\n\n{body}\n--b\nContent-Type: text/html\n\n<p>{body}</p>\n--b--\n"
    )
}

const HONEST_INVOICE: &str = "Thanks for your order.";
const LYING_INVOICE: &str = r#"Verify your account or your account will be suspended: <a href="https://evil.test/login">www.paypal.com</a>"#;

#[tokio::test]
async fn reusing_a_judged_message_id_earns_no_free_pass() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail(
            "Maya <maya@x.test>",
            "reused@x",
            "Lunch",
            "",
            "Are you free?",
        ),
        false,
    );
    r.sync().await;
    assert_eq!(r.row("inbox", "Lunch").await["risk"], 0);
    r.setting("spam_mode", "move").await;

    r.imap.add_message("INBOX", &scam("reused@x"), false);
    r.sync().await;
    r.sync().await;

    assert_eq!(
        r.row("junk", "Your account has been suspended").await["risk"],
        2
    );
    assert_eq!(
        r.subjects("inbox", None).await,
        ["Lunch"],
        "the earlier email isn't dragged along"
    );
    assert_eq!(
        *r.alerts.lock().unwrap(),
        ["Maya"],
        "the scam never pops up"
    );
}

#[tokio::test]
async fn copying_the_sender_and_subject_too_earns_no_free_pass() {
    let r = rig().await;
    r.imap.add_message("INBOX", &invoice(HONEST_INVOICE), false);
    r.sync().await;
    assert_eq!(r.row("inbox", "Your invoice").await["risk"], 0);

    // Everything a sender can choose is the same, but the server says when it arrived.
    r.imap.add_message_dated(
        "INBOX",
        &invoice(LYING_INVOICE),
        false,
        "01-Nov-2026 10:05:00 +0000",
    );
    r.sync().await;

    assert_eq!(r.row("inbox", "Your invoice").await["risk"], 2);
    assert_eq!(
        *r.alerts.lock().unwrap(),
        ["Shop"],
        "the scam never pops up"
    );
}

#[tokio::test]
async fn the_same_email_in_two_folders_shares_one_verdict() {
    let r = rig().await;
    r.imap.add_message("Archive", &scam("1@x"), false);
    r.imap.add_message("INBOX", &scam("1@x"), false);
    r.sync().await;

    let subject = "Your account has been suspended";
    assert_eq!(r.row("inbox", subject).await["risk"], 2);
    assert_eq!(
        r.row("archive", subject).await["risk"],
        2,
        "only the inbox copy gets checked, and this one is the same email"
    );
}

#[tokio::test]
async fn emails_that_cant_be_told_apart_share_the_worse_verdict() {
    let r = rig().await;
    // Same headers, same second. Whichever is checked last mustn't clear the other.
    for (id, subject, bodies) in [
        ("a@x", "First honest", [HONEST_INVOICE, LYING_INVOICE]),
        ("b@x", "First lying", [LYING_INVOICE, HONEST_INVOICE]),
    ] {
        for body in bodies {
            let raw = invoice(body)
                .replace("reused@x", id)
                .replace("Your invoice", subject);
            r.imap.add_message("INBOX", &raw, false);
        }
    }
    r.sync().await;

    assert_eq!(r.row("inbox", "First honest").await["risk"], 2);
    assert_eq!(r.row("inbox", "First lying").await["risk"], 2);
}

#[tokio::test]
async fn an_email_delivered_twice_is_still_one_email_to_act_on() {
    let r = rig().await;
    r.setting("conversations", "off").await;
    let lunch = mail(
        "Maya <maya@x.test>",
        "twice@x",
        "Lunch",
        "",
        "Are you free?",
    );
    r.imap.add_message("INBOX", &lunch, false);
    r.imap
        .add_message_dated("INBOX", &lunch, false, "01-Nov-2026 10:05:00 +0000");
    r.sync().await;
    assert_eq!(r.folder("inbox").await["unread"], 1);

    let t = r.threads("inbox", "Lunch").await;
    r.call("bulk_threads", json!({ "threads": t, "action": "seen" }))
        .await;
    assert_eq!(r.folder("inbox").await["unread"], 0, "both deliveries read");
}
