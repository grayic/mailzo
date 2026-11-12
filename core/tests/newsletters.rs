//! The list of newsletters and leaving them, against the stand-in servers.

use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";

struct Rig {
    server: FakeServer,
    outgoing: FakeSmtp,
    engine: Arc<Engine>,
    account: i64,
    opened: Arc<Mutex<Vec<String>>>,
}

fn mail(id: &str, from: &str, day: u32, headers: &str) -> String {
    mail_on(
        id,
        from,
        &format!("{day:02} Nov 2026 10:00:00 +0000"),
        headers,
    )
}

fn mail_on(id: &str, from: &str, date: &str, headers: &str) -> String {
    format!(
        "From: {from}\nTo: Me <{USER}>\nSubject: Issue {id}\n\
         Date: {date}\nMessage-ID: <{id}@x.test>\n{headers}\
         Content-Type: text/plain; charset=utf-8\n\nBody of {id}\n"
    )
}

async fn rig() -> Rig {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    server.add_folder("Archive", &["\\Archive"], 1);
    server.add_folder("Junk", &["\\Junk"], 1);
    let outgoing = FakeSmtp::start(USER, PASS).await;
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = opened.clone();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_sent_copy_wait(0)
    .with_url_opener(Arc::new(move |url: &str| {
        log.lock().unwrap().push(url.to_string());
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
        opened,
    }
}

impl Rig {
    fn add(&self, folder: &str, id: &str, from: &str, day: u32, headers: &str) {
        self.server.add_message_dated(
            folder,
            &mail(id, from, day, headers),
            true,
            &format!("{day:02}-Nov-2026 10:00:00 +0000"),
        );
    }

    /// Mail that arrives a minute from now: after anything the test has done so far, and
    /// still new, whatever day the test runs on.
    fn add_soon(&self, folder: &str, id: &str, from: &str, headers: &str) {
        let at = chrono::Utc::now() + chrono::Duration::seconds(60);
        self.server.add_message_dated(
            folder,
            &mail_on(id, from, &at.to_rfc2822(), headers),
            true,
            &at.format("%d-%b-%Y %H:%M:%S +0000").to_string(),
        );
    }

    async fn sync(&self) {
        self.engine
            .call("sync_account", json!({ "account_id": self.account }))
            .await
            .unwrap();
    }

    async fn list(&self) -> Vec<Value> {
        let rows = self.engine.call("newsletters", json!({})).await.unwrap();
        rows.as_array().unwrap().clone()
    }

    async fn row(&self, address: &str) -> Value {
        self.list()
            .await
            .into_iter()
            .find(|r| r["address"] == address)
            .unwrap_or_else(|| panic!("{address} isn't listed"))
    }

    async fn leave(&self, address: &str) -> Result<Value, String> {
        let id = self.row(address).await["message_id"].clone();
        self.engine
            .call("unsubscribe", json!({ "message_id": id }))
            .await
            .map_err(|e| e.to_string())
    }

    async fn drafts(&self) -> usize {
        let list = self.engine.call("draft_list", json!({})).await.unwrap();
        list.as_array().unwrap().len()
    }
}

const MAILTO: &str =
    "List-Unsubscribe: <mailto:leave@list.test?subject=Stop%20please&cc=evil@x.test>\n";
const PAGE: &str = "List-Unsubscribe: <https://shop.test/out>\n";
const ONE_CLICK: &str =
    "List-Unsubscribe: <https://click.test/out>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\n";

#[tokio::test]
async fn senders_with_a_way_out_are_listed_once_with_how_a_click_works() {
    let r = rig().await;
    r.add("INBOX", "a1", "News <News@List.test>", 1, MAILTO);
    r.add("Archive", "a2", "News Weekly <news@list.test>", 2, MAILTO);
    r.add("INBOX", "b1", "Shop <deals@shop.test>", 3, PAGE);
    r.add("INBOX", "c1", "Click <hi@click.test>", 4, ONE_CLICK);
    r.add("INBOX", "p1", "A Person <friend@x.test>", 5, "");
    r.add("Junk", "j1", "Junk <junk@spam.test>", 6, PAGE);
    r.sync().await;

    let rows = r.list().await;
    let names: Vec<&str> = rows
        .iter()
        .map(|x| x["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["hi@click.test", "deals@shop.test", "news@list.test"],
        "newest first, one row each, no people and nothing from Junk"
    );
    assert_eq!(rows[0]["how"], "one_click");
    assert_eq!(rows[1]["how"], "page");
    let news = &rows[2];
    assert_eq!(news["how"], "mail");
    assert_eq!(news["threads"], 2, "counted across folders");
    assert_eq!(news["name"], "News Weekly", "as they call themselves now");
    assert_eq!(news["subject"], "Issue a2");
    assert!(news["left"].is_null());

    // Their mail, from wherever it's been filed.
    let mail = r
        .engine
        .call(
            "mail_with",
            json!({ "account_id": r.account, "addresses": ["news@list.test"] }),
        )
        .await
        .unwrap();
    let subjects: Vec<&str> = mail
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["Issue a2", "Issue a1"]);
}

#[tokio::test]
async fn senders_with_no_way_out_are_not_newsletters() {
    let r = rig().await;
    for day in 1..=3 {
        r.add(
            "INBOX",
            &format!("n{day}"),
            "Alerts <alerts@bank.test>",
            day,
            "",
        );
        r.add("INBOX", &format!("w{day}"), "Pal <pal@x.test>", day, "");
    }
    r.add("INBOX", "q1", "Quiet <quiet@x.test>", 1, "");
    r.add("INBOX", "q2", "Quiet <quiet@x.test>", 2, "");
    r.server.add_message_dated(
        "Sent",
        &format!(
            "From: Me <{USER}>\nTo: Pal <pal@x.test>\nSubject: Hello\n\
             Date: 04 Nov 2026 10:00:00 +0000\nMessage-ID: <hello@x.test>\n\
             Content-Type: text/plain; charset=utf-8\n\nHi\n"
        ),
        true,
        "04-Nov-2026 10:00:00 +0000",
    );
    r.sync().await;

    let rows = r.list().await;
    for address in ["alerts@bank.test", "quiet@x.test", "pal@x.test"] {
        assert!(
            !rows.iter().any(|x| x["address"] == address),
            "{address} has no unsubscribe link"
        );
    }
}

#[tokio::test]
async fn mail_with_someone_has_what_they_sent_and_what_was_sent_to_them() {
    let r = rig().await;
    r.add("INBOX", "f1", "Friend <friend@x.test>", 1, "");
    r.server.add_message_dated(
        "Sent",
        &format!(
            "From: Me <{USER}>\nTo: Someone <other@x.test>\nCc: Friend <Friend@X.test>\nSubject: Issue f2\n\
             Date: 02 Nov 2026 10:00:00 +0000\nMessage-ID: <f2@x.test>\n\
             Content-Type: text/plain; charset=utf-8\n\nBody\n"
        ),
        true,
        "02-Nov-2026 10:00:00 +0000",
    );
    r.add("Junk", "f3", "Friend <friend@x.test>", 3, "");
    r.add("INBOX", "o1", "Other <other@x.test>", 4, "");
    r.sync().await;

    // Every inbox, as Contacts asks.
    let mail = r
        .engine
        .call(
            "mail_with",
            json!({ "account_id": null, "addresses": ["friend@x.test"] }),
        )
        .await
        .unwrap();
    let subjects: Vec<&str> = mail
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap())
        .collect();
    assert_eq!(
        subjects,
        ["Issue f2", "Issue f1"],
        "sent to them too, newest first, not Junk"
    );
}

#[tokio::test]
async fn leaving_by_email_sends_only_what_the_list_may_ask_for_and_is_remembered() {
    let r = rig().await;
    r.add("INBOX", "a1", "News <news@list.test>", 1, MAILTO);
    r.sync().await;

    assert_eq!(r.leave("news@list.test").await.unwrap()["kind"], "sent");
    let sent = r.outgoing.received();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].to,
        ["leave@list.test"],
        "and nobody the link tried to copy in"
    );
    assert!(
        sent[0].data.contains("Subject: Stop please"),
        "{}",
        sent[0].data
    );
    assert_eq!(r.drafts().await, 0, "no draft left behind");

    let row = r.row("news@list.test").await;
    assert_eq!(row["left"]["how"], "sent");
    assert!(row["left"]["at"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn an_email_that_cant_go_changes_nothing() {
    let r = rig().await;
    r.add("INBOX", "a1", "News <news@list.test>", 1, MAILTO);
    r.add(
        "INBOX",
        "b1",
        "Many <many@list.test>",
        2,
        "List-Unsubscribe: <mailto:a@x.test%2Cb@x.test>\n",
    );
    r.sync().await;

    r.outgoing.refuse_with("550 no");
    assert!(r.leave("news@list.test").await.is_err());
    assert!(r.row("news@list.test").await["left"].is_null());
    assert_eq!(r.drafts().await, 0, "the failed send leaves no draft");
    r.outgoing.stop_refusing();

    assert!(
        r.leave("many@list.test").await.is_err(),
        "more than one address"
    );
    assert!(r.row("many@list.test").await["left"].is_null());
    assert!(r.outgoing.received().is_empty());
}

#[tokio::test]
async fn a_page_opens_and_says_so_and_one_click_falls_back_to_it_if_refused() {
    let r = rig().await;
    r.add("INBOX", "b1", "Shop <deals@shop.test>", 1, PAGE);
    r.add("INBOX", "c1", "Click <hi@click.test>", 2, ONE_CLICK);
    r.sync().await;

    assert_eq!(r.leave("deals@shop.test").await.unwrap()["kind"], "opened");
    assert_eq!(r.row("deals@shop.test").await["left"]["how"], "opened");

    // Nothing answers at this made-up address, so the one click can't have worked.
    assert_eq!(r.leave("hi@click.test").await.unwrap()["kind"], "opened");
    assert_eq!(
        *r.opened.lock().unwrap(),
        ["https://shop.test/out", "https://click.test/out"]
    );
}

#[tokio::test]
async fn someone_left_stays_known_after_their_mail_has_gone() {
    let r = rig().await;
    r.add("INBOX", "a1", "News <news@list.test>", 1, MAILTO);
    r.sync().await;
    r.leave("news@list.test").await.unwrap();

    r.server.delete_message("INBOX", 1);
    r.sync().await;
    let row = r.row("news@list.test").await;
    assert_eq!(row["left"]["how"], "sent");
    assert_eq!(row["threads"], 0);
    assert_eq!(row["name"], "News");
    assert!(row["message_id"].is_null());
}

async fn inbox_subjects(r: &Rig, role: &str) -> Vec<String> {
    let folders = r
        .engine
        .call("list_folders", json!({ "account_id": r.account }))
        .await
        .unwrap();
    let id = folders
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["role"] == role)
        .unwrap()["id"]
        .clone();
    let list = r
        .engine
        .call("list_threads", json!({ "folder_id": id }))
        .await
        .unwrap();
    list.as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn new_mail_from_a_sender_left_goes_to_archive_until_let_back_in() {
    let r = rig().await;
    r.add("INBOX", "a1", "News <news@list.test>", 1, MAILTO);
    r.sync().await;
    r.leave("news@list.test").await.unwrap();

    // They ignore it and write again.
    r.add_soon("INBOX", "a2", "News <news@list.test>", MAILTO);
    r.sync().await;
    r.sync().await;
    assert!(
        !inbox_subjects(&r, "inbox")
            .await
            .contains(&"Issue a2".to_string()),
        "kept out of the inbox"
    );
    assert!(inbox_subjects(&r, "archive")
        .await
        .contains(&"Issue a2".to_string()));

    // Let back in, their next email stays.
    r.engine
        .call(
            "newsletter_forget",
            json!({ "account_id": r.account, "address": "news@list.test" }),
        )
        .await
        .unwrap();
    assert!(r.row("news@list.test").await["left"].is_null());
    r.add_soon("INBOX", "a3", "News <news@list.test>", MAILTO);
    r.sync().await;
    r.sync().await;
    assert!(inbox_subjects(&r, "inbox")
        .await
        .contains(&"Issue a3".to_string()));
}
