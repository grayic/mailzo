//! Finding mail, through the real engine.

use fakeai::{FakeAi, GOOD_KEY};
use fakeimap::FakeServer;
use mailcore::alerts::Alert;
use mailcore::sync::SyncOptions;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const ME: &str = "me@grayic.test";
const PASS: &str = "secret-pass";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    ai: FakeAi,
    account: i64,
    alerts: Arc<Mutex<Vec<String>>>,
    /// A second look at the same database, as anyone with the file would see it.
    db: Store,
    _dir: TempDir,
}

/// A folder that removes itself when the test ends.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mail(from: &str, id: &str, subject: &str, body: &str) -> String {
    format!(
        "From: {from}\nTo: {ME}\nSubject: {subject}\nMessage-ID: <{id}>\nContent-Type: text/plain; charset=utf-8\n\n{body}\n"
    )
}

/// Only the newest two inbox emails download at first, as with a big real inbox.
async fn rig() -> Rig {
    let imap = FakeServer::start(ME, PASS).await;
    imap.add_folder("INBOX", &[], 1);
    imap.add_folder("Sent", &["\\Sent"], 1);
    imap.add_folder("Archive", &["\\Archive"], 1);
    imap.add_folder("Trash", &["\\Trash"], 1);
    imap.add_folder("Junk", &["\\Junk"], 1);
    let ai = FakeAi::start().await;
    let alerts: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = alerts.clone();
    let options = SyncOptions {
        backfill_seconds: 0,
        inbox_window: 2,
        ..SyncOptions::default()
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "inbox-search-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mail.db");
    let engine = Engine::with_options(
        Store::open(&file).unwrap(),
        Arc::new(MemoryStore::default()),
        options,
    )
    .with_ai_url(&ai.url)
    .with_alert_sender(Arc::new(move |a: &Alert| {
        log.lock().unwrap().push(a.title.clone())
    }));
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
    Rig {
        engine,
        imap,
        ai,
        account: added["id"].as_i64().unwrap(),
        alerts,
        db: Store::open(&file).unwrap(),
        _dir: TempDir(dir),
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }
    async fn sync(&self) {
        self.call("sync_account", json!({ "account_id": self.account }))
            .await;
    }
    async fn find(&self, text: &str) -> Vec<String> {
        subjects(&self.call("search_mail", json!({ "text": text })).await)
    }
    fn add(&self, folder: &str, from: &str, id: &str, subject: &str, body: &str) {
        self.imap
            .add_message(folder, &mail(from, id, subject, body), true);
    }
}

fn subjects(reply: &Value) -> Vec<String> {
    let mut list: Vec<String> = reply["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap().to_string())
        .collect();
    list.sort();
    list
}

#[tokio::test]
async fn finds_words_anywhere_in_saved_mail() {
    let r = rig().await;
    r.add(
        "INBOX",
        "Maya Chen <maya@northwind.test>",
        "1@x",
        "Q4 launch plan",
        "The budget is final",
    );
    r.add(
        "Archive",
        "Sam Okafor <sam@x.test>",
        "2@x",
        "Invoice 1042",
        "Payment is due",
    );
    r.add("Sent", ME, "3@x", "সভার সময়", "আগামীকাল দেখা হবে");
    r.sync().await;

    assert_eq!(r.find("launch").await, ["Q4 launch plan"], "the subject");
    assert_eq!(r.find("budget").await, ["Q4 launch plan"], "the body");
    assert_eq!(r.find("maya").await, ["Q4 launch plan"], "the sender");
    assert_eq!(
        r.find("northwind").await,
        ["Q4 launch plan"],
        "their address"
    );
    assert_eq!(
        r.find("invo").await,
        ["Invoice 1042"],
        "the start of a word"
    );
    assert_eq!(r.find("INVOICE").await, ["Invoice 1042"], "any capitals");
    assert_eq!(r.find("সভার").await, ["সভার সময়"], "Bangla");
    assert!(
        r.find("budget invoice").await.is_empty(),
        "every word must match"
    );
    assert_eq!(
        r.find("\"payment is due\"").await,
        ["Invoice 1042"],
        "an exact phrase"
    );
    assert!(
        r.find("\"due is payment\"").await.is_empty(),
        "in that order"
    );
    assert!(r.find("nothing-like-this").await.is_empty());
    assert!(r.find("").await.is_empty());
}

#[tokio::test]
async fn words_only_in_a_quoted_reply_dont_count() {
    let r = rig().await;
    r.add(
        "INBOX",
        "Ana <ana@x.test>",
        "1@x",
        "Thanks",
        "Thanks!\n\nOn Mon, Sam wrote:\n> the Wyoming invoice",
    );
    r.add(
        "INBOX",
        "Sam <sam@x.test>",
        "2@x",
        "Annual fee",
        "Your Wyoming invoice is paid.",
    );
    r.sync().await;
    assert_eq!(r.find("wyoming invoice").await, ["Annual fee"]);
}

#[tokio::test]
async fn word_forms_and_accents_still_match() {
    let r = rig().await;
    r.add(
        "INBOX",
        "a@x.test",
        "1@x",
        "Invoices for October",
        "Two of them",
    );
    r.add(
        "INBOX",
        "b@x.test",
        "2@x",
        "Café receipt",
        "Coffee and cake",
    );
    r.sync().await;
    assert_eq!(r.find("invoice").await, ["Invoices for October"]);
    assert_eq!(r.find("cafe").await, ["Café receipt"]);
}

#[tokio::test]
async fn the_exact_check_reads_words_the_way_the_index_does() {
    let r = rig().await;
    r.add(
        "Archive",
        "a@x.test",
        "1@x",
        "All Hands meeting notes",
        "Action items inside",
    );
    r.add(
        "Archive",
        "b@x.test",
        "2@x",
        "Fee",
        "Your Wyoming\ninvoice is paid.",
    );
    r.add(
        "Archive",
        "c@x.test",
        "3@x",
        "Draft",
        "The client\u{2019}s copy is attached.",
    );
    r.add(
        "Archive",
        "d@x.test",
        "4@x",
        "Th\u{1edd}i ti\u{1ebf}t",
        "H\u{e0} N\u{1ed9}i",
    );
    r.add("Archive", "e@x.test", "5@x", "Contract", "Smith signed it.");
    r.sync().await;
    assert_eq!(r.find("all-hands").await, ["All Hands meeting notes"]);
    assert_eq!(
        r.find("\"wyoming invoice\"").await,
        ["Fee"],
        "a phrase across a line break"
    );
    assert_eq!(r.find("client's").await, ["Draft"], "either apostrophe");
    assert_eq!(
        r.find("thoi").await,
        ["Th\u{1edd}i ti\u{1ebf}t"],
        "Vietnamese accents"
    );
    assert_eq!(
        r.find("Smith,").await,
        ["Contract"],
        "punctuation typed with a word"
    );
}

#[tokio::test]
async fn a_reply_written_between_quoted_lines_keeps_its_own_words() {
    let r = rig().await;
    r.add(
        "INBOX",
        "Ana <ana@x.test>",
        "1@x",
        "Re: Files",
        "> Can you send it?\nSure, attached.\n> And the budget?\nBudget is $5k",
    );
    r.sync().await;
    assert_eq!(r.find("budget").await, ["Files"]);
    assert!(r.find("send").await.is_empty(), "only in a quoted line");
}

#[tokio::test]
async fn a_sign_in_email_offers_its_code_and_an_order_email_doesnt() {
    let r = rig().await;
    r.add(
        "Archive",
        "Cloudflare <noreply@cloudflare.test>",
        "c@x",
        "Cloudflare Access login code for staging.orvio.app",
        "Your Cloudflare Access code\n\n147363\n\nThis code expires after 10 minutes.",
    );
    r.add(
        "Archive",
        "Dynadot Orders <orders@dynadot.test>",
        "d@x",
        "Transfer(s) Initiated (order 27298692)",
        "Hooray! The transfer of your domain has been initiated. ORDER NUMBER 27298692",
    );
    r.sync().await;
    let found = r.call("search_mail", json!({ "text": "cloudflare" })).await;
    assert_eq!(found["threads"][0]["code"], "147363", "{found}");
    let found = r.call("search_mail", json!({ "text": "dynadot" })).await;
    assert!(found["threads"][0]["code"].is_null(), "{found}");
}

#[tokio::test]
async fn languages_without_spaces_are_found() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "ご連絡", "請求書を送ります");
    r.sync().await;
    assert_eq!(r.find("請求書").await, ["ご連絡"]);
}

/// Midnight starting the Monday `weeks_back` weeks before this one, in UTC.
fn monday(weeks_back: i64) -> i64 {
    use chrono::Datelike;
    let today = chrono::Utc::now().date_naive();
    let this_monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    (this_monday - chrono::Duration::weeks(weeks_back))
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

#[tokio::test]
async fn a_sentence_is_understood_without_ai() {
    let r = rig().await;
    r.add(
        "INBOX",
        "Priya Shah <priya@a.test>",
        "1@x",
        "Budget",
        "Numbers attached",
    );
    r.sync().await;
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let asked = r.ai.requests().len();
    r.ai.fail_with(Some((500, "should not be asked")));
    let got = r
        .call(
            "search_understand",
            json!({ "text": "emails from Priya last week", "today": "2026-11-06", "utc_offset_minutes": 0 }),
        )
        .await;
    assert_eq!(got["used_ai"], false);
    assert_eq!(got["filters"]["from"], json!(["priya@a.test"]));
    assert_eq!(got["filters"]["after"], monday(1));
    assert_eq!(got["filters"]["before"], monday(0));
    assert_eq!(got["filters"]["words"], json!([]));
    assert_eq!(r.ai.requests().len(), asked, "the search box never asks AI");
}

#[tokio::test]
async fn ask_ai_asks_ai_only_when_unsure() {
    let r = rig().await;
    r.add(
        "INBOX",
        "Sam <sam@x.test>",
        "1@x",
        "Lunch",
        "Private sandwiches",
    );
    r.sync().await;
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let before = r.ai.requests().len();
    r.ai.answer(
        "a while back",
        json!({ "kind": "fact", "names": [], "date_phrase": "last month", "words": ["thing", "invented"] }),
    );
    let now = chrono::Utc::now();
    let (q, _, used_ai) = r
        .engine
        .understand("stuff about the thing from a while back", now, true)
        .await
        .unwrap();
    assert!(used_ai);
    let requests = r.ai.requests();
    assert_eq!(requests.len(), before + 1);
    let sent = requests.last().unwrap();
    assert_eq!(
        sent["messages"][0]["content"],
        "stuff about the thing from a while back"
    );
    assert!(
        !sent.to_string().contains("sandwiches"),
        "no email goes with the question"
    );
    assert_eq!(q.kind, mailcore::question::Kind::Fact);
    assert_eq!(
        q.words,
        ["thing"],
        "AI can't add words the question doesn't have"
    );
    let expected = mailcore::question::read_question("last month", now, true);
    assert_eq!(
        (q.after, q.before),
        (expected.after, expected.before),
        "code works out the dates"
    );
    // A question the rules read needs no AI at all.
    let (_, _, used_ai) = r
        .engine
        .understand("emails from Sam last week", now, true)
        .await
        .unwrap();
    assert!(!used_ai);
    assert_eq!(r.ai.requests().len(), before + 1);
}

#[tokio::test]
async fn odd_typing_never_breaks_search() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "Hello", "Hi there");
    r.sync().await;
    for text in [
        "\"",
        "a\"b",
        "*",
        "AND OR NOT",
        "(",
        "from:",
        "has:",
        "NEAR(",
        "'; DROP TABLE messages;--",
        "%_\\",
    ] {
        r.find(text).await;
    }
    assert_eq!(r.find("hello").await, ["Hello"]);
}

#[tokio::test]
async fn filters_narrow_what_is_found() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail("Maya <maya@x.test>", "1@x", "Plan A", "report"),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &mail("Sam <sam@x.test>", "2@x", "Plan B", "report"),
        true,
    );
    r.imap.add_message(
        "Sent",
        &format!(
            "From: {ME}\nTo: Ana <ana@x.test>\nSubject: Plan C\nMessage-ID: <3@x>\n\nreport\n"
        ),
        true,
    );
    r.sync().await;

    assert_eq!(r.find("report from:maya").await, ["Plan A"]);
    assert_eq!(r.find("from:\"sam@x.test\"").await, ["Plan B"]);
    assert_eq!(r.find("to:ana").await, ["Plan C"]);
    assert_eq!(r.find("report is:unread").await, ["Plan A"]);
    assert_eq!(r.find("report in:sent").await, ["Plan C"]);
    assert!(r.find("report is:starred").await.is_empty());
    assert!(r.find("report has:attachment").await.is_empty());
    assert_eq!(
        r.find("from:maya_").await,
        Vec::<String>::new(),
        "no wildcards"
    );

    let dated = r
        .call(
            "search_mail",
            json!({ "filters": { "words": ["report"], "before": 1000 } }),
        )
        .await;
    assert!(subjects(&dated).is_empty());
}

#[tokio::test]
async fn the_bin_and_spam_stay_out_unless_asked_for() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "Kept report", "x");
    r.add("Trash", "a@x.test", "2@x", "Binned report", "x");
    r.add("Junk", "a@x.test", "3@x", "Spam report", "x");
    r.sync().await;

    let plain = r.call("search_mail", json!({ "text": "report" })).await;
    assert_eq!(subjects(&plain), ["Kept report"]);
    assert_eq!(plain["in_bin"], 2, "and it says how many more there are");
    assert_eq!(r.find("report in:bin").await, ["Binned report"]);
    let all = r
        .call(
            "search_mail",
            json!({ "filters": { "words": ["report"], "include_bin": true } }),
        )
        .await;
    assert_eq!(subjects(&all).len(), 3);
    assert_eq!(all["in_bin"], 0);
}

#[tokio::test]
async fn old_mail_is_found_on_the_server_and_nothing_changes_there() {
    let r = rig().await;
    r.engine
        .call(
            "set_setting",
            json!({ "key": "alert_which", "value": "people" }),
        )
        .await
        .unwrap();
    r.add(
        "INBOX",
        "Old <old@x.test>",
        "1@x",
        "Ancient contract",
        "signed long ago",
    );
    for n in 2..=4 {
        r.add(
            "INBOX",
            "New <new@x.test>",
            &format!("{n}@x"),
            &format!("Recent {n}"),
            "hello",
        );
    }
    r.imap.add_message(
        "INBOX",
        &mail("Old <old@x.test>", "u@x", "Unread old contract", "pending"),
        false,
    );
    r.sync().await;
    assert!(
        r.find("ancient").await.is_empty(),
        "too old to have been downloaded"
    );
    let fetches = r.imap.state.lock().unwrap().non_peek_fetches;

    let found = r
        .call("search_servers", json!({ "text": "contract" }))
        .await;
    let mut names = subjects(&found);
    names.retain(|n| n.contains("contract"));
    assert!(names.contains(&"Ancient contract".to_string()), "{found}");
    assert_eq!(found["unreached"], json!([]));
    assert!(!found["extra_ids"].as_array().unwrap().is_empty());
    assert_eq!(
        r.find("ancient").await,
        ["Ancient contract"],
        "now saved here"
    );
    assert_eq!(
        r.find("signed").await,
        ["Ancient contract"],
        "with its words"
    );
    assert_eq!(
        r.imap.state.lock().unwrap().non_peek_fetches,
        fetches,
        "nothing marked read"
    );

    // Finding old mail mustn't confuse the refresh.
    r.alerts.lock().unwrap().clear();
    r.imap.add_message(
        "INBOX",
        &mail("Fresh <fresh@x.test>", "9@x", "Brand new", "hi"),
        false,
    );
    r.sync().await;
    assert_eq!(r.find("brand").await, ["Brand new"]);
    assert_eq!(*r.alerts.lock().unwrap(), ["Fresh"]);
    r.sync().await;
    assert_eq!(
        r.find("ancient").await,
        ["Ancient contract"],
        "and it stays"
    );
}

#[tokio::test]
async fn searching_a_server_that_is_unreachable_keeps_saved_results() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "Saved report", "x");
    r.sync().await;
    r.imap.state.lock().unwrap().pass = "changed".into();
    let found = r.call("search_servers", json!({ "text": "report" })).await;
    assert_eq!(subjects(&found), ["Saved report"]);
    assert_eq!(found["unreached"], json!([ME]));
}

#[tokio::test]
async fn a_search_can_look_in_one_inbox() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "Budget at home", "x");
    r.sync().await;
    let work = FakeServer::start("work@grayic.test", PASS).await;
    work.add_folder("INBOX", &[], 1);
    work.add_message(
        "INBOX",
        &mail("b@x.test", "2@x", "Budget at work", "x"),
        true,
    );
    let added = r
        .call(
            "add_account",
            json!({
                "email": "work@grayic.test", "imap_host": "127.0.0.1",
                "imap_port": work.addr.port(), "security": "plain", "password": PASS,
            }),
        )
        .await;
    let other = added["id"].as_i64().unwrap();
    r.call("sync_account", json!({ "account_id": other })).await;

    assert_eq!(
        r.find("budget").await,
        ["Budget at home", "Budget at work"],
        "every inbox"
    );
    let only = |account: i64| json!({ "text": "budget", "account": account });
    assert_eq!(
        subjects(&r.call("search_mail", only(other)).await),
        ["Budget at work"]
    );
    assert_eq!(
        subjects(&r.call("search_mail", only(r.account)).await),
        ["Budget at home"]
    );
    let chosen = r
        .call(
            "search_mail",
            json!({ "filters": { "words": ["budget"], "account": other } }),
        )
        .await;
    assert_eq!(subjects(&chosen), ["Budget at work"]);
    assert_eq!(chosen["filters"]["account"], other, "kept for asking again");
    let nothing = r
        .call("search_mail", json!({ "filters": { "account": other } }))
        .await;
    assert!(
        subjects(&nothing).is_empty(),
        "an inbox alone isn't something to find"
    );

    // Only the chosen inbox's server is asked.
    r.imap.state.lock().unwrap().pass = "changed".into();
    let found = r.call("search_servers", only(other)).await;
    assert_eq!(subjects(&found), ["Budget at work"]);
    assert_eq!(found["unreached"], json!([]));
}

#[tokio::test]
async fn nothing_of_a_disconnected_inbox_stays_searchable() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "Private report", "secret words");
    r.sync().await;
    assert_eq!(r.find("secret").await.len(), 1);
    r.call("remove_account", json!({ "account_id": r.account }))
        .await;
    assert!(r.find("secret").await.is_empty());
    let left = r.db.search_index_size().unwrap();
    assert_eq!(left, 0, "the words themselves are gone too");
}

#[tokio::test]
async fn each_email_gets_its_own_row_when_conversations_are_off() {
    let r = rig().await;
    r.add("INBOX", "a@x.test", "1@x", "Report", "first");
    r.imap.add_message(
        "INBOX",
        "From: b@x.test\nSubject: Re: Report\nMessage-ID: <2@x>\nIn-Reply-To: <1@x>\nReferences: <1@x>\n\nreport second\n",
        true,
    );
    r.sync().await;
    assert_eq!(r.find("report").await.len(), 1);
    r.call(
        "set_setting",
        json!({ "key": "conversations", "value": "off" }),
    )
    .await;
    assert_eq!(r.find("report").await.len(), 2);
}

#[tokio::test]
async fn the_ai_key_is_tested_and_never_shown_or_stored_in_the_database() {
    let r = rig().await;
    assert_eq!(r.call("ai_status", json!({})).await["has_key"], false);

    let bad = r
        .engine
        .call("ai_set_key", json!({ "key": "wrong" }))
        .await
        .unwrap_err();
    assert_eq!(bad.code(), "ai_key");
    assert_eq!(
        r.call("ai_status", json!({})).await["has_key"],
        false,
        "not saved"
    );

    let status = r
        .call("ai_set_key", json!({ "key": format!("  {GOOD_KEY} ") }))
        .await;
    assert_eq!(status["has_key"], true);
    assert_eq!(status["ending"], "-key", "only the end, to recognise it");
    assert!(!status.to_string().contains(GOOD_KEY));
    assert!(!r
        .call("app_settings", json!({}))
        .await
        .to_string()
        .contains(GOOD_KEY));
    assert!(r.db.all_settings().unwrap().values().all(|v| v != GOOD_KEY));

    r.call("ai_remove_key", json!({})).await;
    assert_eq!(r.call("ai_status", json!({})).await["has_key"], false);
}

fn understand(text: &str) -> Value {
    json!({ "text": text, "today": "2026-11-02", "utc_offset_minutes": 360 })
}

#[tokio::test]
async fn the_search_box_reads_a_sentence_by_code_and_sends_nothing() {
    let r = rig().await;
    r.add(
        "INBOX",
        "Maya <maya@x.test>",
        "1@x",
        "Your itinerary",
        "Boarding at gate 4, private details",
    );
    r.add("INBOX", "Sam <sam@x.test>", "2@x", "Lunch", "Sandwiches");
    r.sync().await;

    let plain = r
        .call("search_understand", understand("flight details from Maya"))
        .await;
    assert_eq!(plain["used_ai"], false);
    assert_eq!(plain["filters"]["from"], json!(["maya@x.test"]));
    assert_eq!(plain["filters"]["words"], json!(["flight", "details"]));

    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let asked = r.ai.requests().len();
    let reply = r
        .call(
            "search_understand",
            understand("flight details from Maya last month"),
        )
        .await;
    assert_eq!(reply["used_ai"], false);
    // Midnight where the person is, six hours ahead of UTC.
    let zone = chrono::FixedOffset::east_opt(6 * 3600).unwrap();
    let month = mailcore::question::read_question(
        "last month",
        chrono::Utc::now().with_timezone(&zone),
        true,
    );
    assert_eq!(reply["filters"]["after"], json!(month.after));
    assert_eq!(reply["filters"]["before"], json!(month.before));
    assert_eq!(
        r.ai.requests().len(),
        asked,
        "with a key, still nothing goes to AI"
    );
    // Only what has every word is found: no similar words are added.
    let mut filters = reply["filters"].clone();
    filters["after"] = Value::Null;
    filters["before"] = Value::Null;
    let found = r.call("search_mail", json!({ "filters": filters })).await;
    assert!(subjects(&found).is_empty(), "no email has 'flight'");
}

#[tokio::test]
async fn ai_problems_are_reported_and_not_asking_makes_no_calls() {
    let r = rig().await;
    r.call("ai_set_key", json!({ "key": GOOD_KEY })).await;
    let now = chrono::Utc::now();
    r.ai.fail_with(Some((400, "Your credit balance is too low")));
    let e = r
        .engine
        .understand("the big report", now, true)
        .await
        .unwrap_err();
    assert_eq!(e.code(), "ai_credit");
    r.ai.fail_with(Some((429, "slow down")));
    let e = r
        .engine
        .understand("the big report", now, true)
        .await
        .unwrap_err();
    assert_eq!(e.code(), "ai_busy");
    r.ai.fail_with(None);
    let asked = r.ai.requests().len();
    let (q, _, used_ai) = r
        .engine
        .understand("the big report", now, false)
        .await
        .unwrap();
    assert!(!used_ai);
    assert_eq!(q.words, ["big", "report"]);
    assert_eq!(r.ai.requests().len(), asked);
}

#[tokio::test]
async fn an_email_delivered_several_times_counts_once() {
    let r = rig().await;
    // The same email reaching three addresses that all forward to this inbox.
    for _ in 0..3 {
        r.imap.add_message(
            "INBOX",
            &mail("a@x.test", "same@x", "Sent to all of you", "x"),
            false,
        );
    }
    // A conversation with two unread replies is one unread conversation.
    r.imap
        .add_message("INBOX", &mail("b@x.test", "1@x", "Lunch", "x"), false);
    r.imap.add_message(
        "INBOX",
        "From: c@x.test\nSubject: Re: Lunch\nMessage-ID: <2@x>\nIn-Reply-To: <1@x>\nReferences: <1@x>\n\nyes\n",
        false,
    );
    let options = json!({ "account_id": r.account });
    r.call("sync_account", options.clone()).await;
    // The rig keeps only the newest two at first, so ask for the rest.
    r.call("search_servers", json!({ "text": "from:x.test" }))
        .await;

    let unread = |folders: &Value| {
        folders
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == "inbox")
            .unwrap()["unread"]
            .as_i64()
            .unwrap()
    };
    let rows = |list: &Value| list.as_array().unwrap().len();
    let folders = r.call("list_folders", options.clone()).await;
    assert_eq!(unread(&folders), 2, "two conversations");
    assert_eq!(
        rows(&r.call("list_all", json!({ "role": "inbox" })).await),
        2
    );

    r.call(
        "set_setting",
        json!({ "key": "conversations", "value": "off" }),
    )
    .await;
    let folders = r.call("list_folders", options).await;
    assert_eq!(unread(&folders), 3, "three emails");
    assert_eq!(
        rows(&r.call("list_all", json!({ "role": "inbox" })).await),
        3
    );
    assert_eq!(r.find("from:x.test").await.len(), 3, "and once in search");
}

async fn on_service(r: &Rig, key: &str) -> Value {
    r.engine
        .call("ai_set_key", json!({ "key": key, "service": "openai" }))
        .await
        .unwrap_or_else(|e| json!({ "failed": e.to_string() }))
}

#[tokio::test]
async fn another_ai_service_works_with_its_own_key() {
    let r = rig().await;
    r.ai.set_models(&[
        "gpt-5.5-luna",
        "gpt-5.6-luna",
        "gpt-6.1-sol",
        "whisper-1",
        "text-embedding-3",
    ]);
    let refused = on_service(&r, "not-the-key").await;
    assert!(
        refused["failed"]
            .as_str()
            .unwrap()
            .contains("wasn't accepted"),
        "{refused}"
    );
    let status = r.engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["has_key"], false, "a refused key isn't kept");
    assert_eq!(
        status["service"], "anthropic",
        "and the service doesn't change"
    );

    let status = on_service(&r, GOOD_KEY).await;
    assert_eq!(status["has_key"], true);
    assert_eq!(status["service"], "openai");
    assert_eq!(
        status["models"],
        json!(["gpt-6.1-sol", "gpt-5.6-luna", "gpt-5.5-luna"]),
        "only models that write"
    );

    r.ai.answer(
        "flights",
        json!({ "kind": "list", "names": [], "date_phrase": "", "words": ["flights"] }),
    );
    let (_, _, used_ai) = r
        .engine
        .understand("flights", chrono::Utc::now(), true)
        .await
        .unwrap();
    assert!(used_ai);
    let last = r.ai.requests().pop().unwrap();
    assert_eq!(
        last["model"], "gpt-5.6-luna",
        "the quick model of the newest kind"
    );
    assert!(last["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("JSON schema"));
    assert_eq!(last["max_completion_tokens"], 4000);
}

#[tokio::test]
async fn a_chosen_model_is_used_for_writing_only_if_the_service_has_it() {
    let r = rig().await;
    r.ai.set_models(&["gpt-5.6-luna", "gpt-6.1-sol"]);
    on_service(&r, GOOD_KEY).await;
    assert!(r
        .engine
        .call("ai_choose_model", json!({ "model": "made-up" }))
        .await
        .is_err());
    let status = r
        .engine
        .call("ai_choose_model", json!({ "model": "gpt-6.1-sol" }))
        .await
        .unwrap();
    assert_eq!(status["model"], "gpt-6.1-sol");
}

#[tokio::test]
async fn a_retired_model_is_replaced_by_itself() {
    let r = rig().await;
    r.ai.set_models(&["gpt-5.6-luna"]);
    on_service(&r, GOOD_KEY).await;
    r.ai.retire("gpt-5.6-luna");
    r.ai.set_models(&["gpt-6-luna"]);
    r.ai.answer(
        "flights",
        json!({ "kind": "list", "names": [], "date_phrase": "", "words": ["flights"] }),
    );
    let (_, _, used_ai) = r
        .engine
        .understand("flights", chrono::Utc::now(), true)
        .await
        .unwrap();
    assert!(used_ai);
    assert_eq!(r.ai.requests().pop().unwrap()["model"], "gpt-6-luna");
}

#[tokio::test]
async fn models_that_refuse_json_mode_or_wrap_answers_still_work() {
    let r = rig().await;
    r.ai.set_models(&["gpt-5.6-luna"]);
    r.ai.refuse_json_mode(true);
    r.ai.fence_answers(true);
    let status = on_service(&r, GOOD_KEY).await;
    assert_eq!(status["has_key"], true, "{status}");
}

#[tokio::test]
async fn removing_the_key_forgets_the_models() {
    let r = rig().await;
    r.ai.set_models(&["gpt-5.6-luna"]);
    on_service(&r, GOOD_KEY).await;
    let status = r.engine.call("ai_remove_key", json!({})).await.unwrap();
    assert_eq!(status["has_key"], false);
    assert_eq!(status["models"], json!([]));
}

#[tokio::test]
async fn search_shows_more_when_the_list_asks_for_more() {
    let r = rig().await;
    r.sync().await;
    for i in 0..320 {
        r.add(
            "INBOX",
            "Billing <billing@x.test>",
            &format!("{i}@bill"),
            &format!("Invoice {i}"),
            "Thanks",
        );
    }
    r.sync().await;
    let found = |reply: Value| reply["threads"].as_array().unwrap().len();
    let first = r
        .call("search_mail", json!({ "text": "from:billing" }))
        .await;
    assert_eq!(found(first), 300, "a page at first");
    let more = r
        .call(
            "search_mail",
            json!({ "text": "from:billing", "most": 600 }),
        )
        .await;
    assert_eq!(found(more), 320, "and the rest when asked");
}
