//! The address book: people from mail, people the person keeps, and vCard files in and out.

use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

const ME: &str = "me@grayic.test";
const PASS: &str = "secret-pass";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    _smtp: FakeSmtp,
    account: i64,
    downloads: PathBuf,
    _dir: tempfile::TempDir,
}

fn mail(from: &str, to: &str, id: &str, date: &str) -> String {
    format!(
        "From: {from}\nTo: {to}\nSubject: Hello\nDate: {date}\nMessage-ID: <{id}>\n\
         Content-Type: text/plain; charset=utf-8\n\nHi\n"
    )
}

async fn rig() -> Rig {
    let imap = FakeServer::start(ME, PASS).await;
    imap.add_folder("INBOX", &[], 1);
    imap.add_folder("Sent", &["\\Sent"], 1);
    imap.add_folder("Archive", &["\\Archive"], 1);
    imap.add_folder("Trash", &["\\Trash"], 1);
    imap.add_folder("Spam", &["\\Junk"], 1);
    let smtp = FakeSmtp::start(ME, PASS).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("downloads");
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_folders(dir.path().join("data"), downloads.clone())
    .with_sent_copy_wait(0);
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
        _smtp: smtp,
        account: added["id"].as_i64().unwrap(),
        downloads,
        _dir: dir,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        match self.engine.call(cmd, args).await {
            Ok(v) => v,
            Err(e) => panic!("{cmd} failed: {e}"),
        }
    }

    async fn fails(&self, cmd: &str, args: Value) -> String {
        self.engine.call(cmd, args).await.unwrap_err().to_string()
    }

    async fn sync(&self) {
        let stats = self
            .call("sync_account", json!({ "account_id": self.account }))
            .await;
        assert!(stats["errors"].as_array().unwrap().is_empty(), "{stats}");
    }

    async fn people(&self, query: &str) -> Vec<Value> {
        self.call("people_list", json!({ "query": query }))
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    async fn contacts(&self, query: &str) -> Vec<Value> {
        self.call(
            "contacts",
            json!({ "account_id": self.account, "query": query }),
        )
        .await
        .as_array()
        .unwrap()
        .clone()
    }

    /// Ana writes twice and is written to once; Bob writes once; a robot and spam besides.
    async fn with_mail(&self) {
        let early = "Mon, 02 Nov 2026 09:00:00 +0000";
        let late = "Wed, 04 Nov 2026 09:00:00 +0000";
        self.imap.add_message(
            "INBOX",
            &mail("Ana Silva <Ana@X.test>", ME, "1@x", early),
            true,
        );
        self.imap.add_message_dated(
            "INBOX",
            &mail("Ana Silva <ana@x.test>", ME, "2@x", late),
            true,
            "04-Nov-2026 09:00:00 +0000",
        );
        self.imap.add_message(
            "Sent",
            &mail(ME, "Ana <ana@x.test>, Bob <bob@y.test>", "3@x", early),
            true,
        );
        self.imap
            .add_message("INBOX", &mail("no-reply@shop.test", ME, "4@x", early), true);
        self.imap.add_message(
            "Spam",
            &mail("Spammer <spam@bad.test>", ME, "5@x", early),
            true,
        );
        self.sync().await;
    }
}

impl Rig {
    /// Each sender writes this many emails.
    async fn writes(&self, list: &[(&str, usize)]) {
        let mut n = 0;
        for (from, times) in list {
            for _ in 0..*times {
                n += 1;
                self.imap.add_message(
                    "INBOX",
                    &mail(
                        from,
                        ME,
                        &format!("w{n}@x"),
                        "Mon, 02 Nov 2026 09:00:00 +0000",
                    ),
                    true,
                );
            }
        }
        self.sync().await;
    }

    fn resolve(&self, name: &str) -> mailcore::people::Resolved {
        self.engine
            .resolve_names(&[name.to_string()])
            .unwrap()
            .remove(0)
    }
}

#[tokio::test]
async fn a_name_becomes_their_addresses() {
    let r = rig().await;
    // Written to at both addresses, so both are hers.
    r.imap.add_message(
        "Sent",
        &mail(
            ME,
            "Priya Shah <priya.shah@work.test>, Priya Shah <priya@a.test>",
            "s1@x",
            "Mon, 02 Nov 2026 10:00:00 +0000",
        ),
        true,
    );
    r.writes(&[
        ("Priya Shah <priya@a.test>", 2),
        ("Priya Shah <priya.shah@work.test>", 1),
        ("Juliana Ruiz <juliana@c.test>", 3),
    ])
    .await;
    let mut got = r.resolve("Priya").addresses;
    got.sort();
    assert_eq!(got, ["priya.shah@work.test", "priya@a.test"]);
    assert!(
        r.resolve("Ana").addresses.is_empty(),
        "a name must start a word: Ana isn't Juliana"
    );
}

#[tokio::test]
async fn a_stranger_using_the_same_name_isnt_them() {
    let r = rig().await;
    r.imap.add_message(
        "Sent",
        &mail(
            ME,
            "Priya Shah <priya@a.test>",
            "s1@x",
            "Mon, 02 Nov 2026 10:00:00 +0000",
        ),
        true,
    );
    // Anyone can sign an email "Priya Shah"; writing more often doesn't make them her.
    r.writes(&[
        ("Priya Shah <priya@a.test>", 1),
        ("Priya Shah <priya@evil.test>", 4),
    ])
    .await;
    assert_eq!(r.resolve("Priya").addresses, ["priya@a.test"]);
}

#[tokio::test]
async fn two_people_with_one_name_picks_the_closer_and_says_so() {
    let r = rig().await;
    r.writes(&[
        ("Priya Shah <shah@a.test>", 3),
        ("Priya Rao <rao@b.test>", 2),
    ])
    .await;
    let got = r.resolve("Priya");
    assert_eq!(got.addresses, ["shah@a.test"]);
    assert_eq!(got.other_match.as_deref(), Some("Priya Rao"));
}

#[tokio::test]
async fn a_name_is_a_whole_name_not_the_start_of_one() {
    let r = rig().await;
    r.writes(&[
        ("Samantha Lee <samantha@a.test>", 3),
        ("Anand Rao <anand@b.test>", 2),
    ])
    .await;
    assert!(r.resolve("Sam").addresses.is_empty(), "Sam isn't Samantha");
    assert!(r.resolve("Ana").addresses.is_empty(), "Ana isn't Anand");
}

#[tokio::test]
async fn me_means_my_own_addresses() {
    let r = rig().await;
    assert_eq!(r.resolve("me").addresses, [ME]);
}

#[tokio::test]
async fn a_company_becomes_its_sender_domains() {
    let r = rig().await;
    r.writes(&[
        ("Amazon <no-reply@amazon.com>", 1),
        ("Amazon.co.uk <order-update@amazon.co.uk>", 1),
        ("Deals <deals@amazonia-deals.test>", 1),
        ("Sam <sam@x.test>", 1),
    ])
    .await;
    let got = r.resolve("Amazon");
    assert!(got.addresses.is_empty());
    let mut domains = got.domains;
    domains.sort();
    assert_eq!(domains, ["amazon.co.uk", "amazon.com"]);
}

#[tokio::test]
async fn a_name_nobody_has_stays_a_word() {
    let r = rig().await;
    r.writes(&[("Sam <sam@x.test>", 1)]).await;
    let got = r.resolve("Zed");
    assert!(got.addresses.is_empty() && got.domains.is_empty());
    assert_eq!(got.name, "Zed");
}

#[tokio::test]
async fn people_from_mail_show_unsaved_with_how_much_was_exchanged() {
    let r = rig().await;
    r.with_mail().await;
    let list = r.people("").await;
    let addresses: Vec<&str> = list
        .iter()
        .map(|p| p["emails"][0].as_str().unwrap())
        .collect();
    assert_eq!(addresses, ["ana@x.test", "bob@y.test"], "{list:?}");
    let ana = &list[0];
    assert_eq!(ana["saved"], false);
    assert_eq!(ana["id"], Value::Null);
    assert_eq!(ana["name"], "Ana Silva");
    assert_eq!(ana["count"], 3);
    assert!(ana["last_at"].as_i64().unwrap() > 0);
    assert_eq!(list[1]["count"], 1);

    let got = r
        .call("person_get", json!({ "address": "ANA@x.test" }))
        .await;
    assert_eq!(got["person"], Value::Null);
    assert_eq!(got["name_from_mail"], "Ana Silva");
    assert_eq!(got["count"], 3);
    assert!(got["first_at"].as_i64().unwrap() < got["last_at"].as_i64().unwrap());
}

#[tokio::test]
async fn a_saved_person_comes_first_and_their_name_wins() {
    let r = rig().await;
    r.with_mail().await;
    let saved = r
        .call(
            "person_save",
            json!({ "name": "  Bobby Tables ", "emails": [" BOB@y.test", "bob@y.test", "b2@y.test"],
                    "phones": ["+1 555 0100", ""], "company": "Acme", "notes": "Met in Lisbon" }),
        )
        .await;
    assert_eq!(saved["name"], "Bobby Tables");
    assert_eq!(saved["emails"], json!(["bob@y.test", "b2@y.test"]));
    assert_eq!(saved["phones"], json!(["+1 555 0100"]));
    assert!(saved["uid"].as_str().unwrap().len() >= 32);

    let list = r.people("").await;
    assert_eq!(list[0]["saved"], true);
    assert_eq!(list[0]["name"], "Bobby Tables");
    assert_eq!(list[0]["company"], "Acme");
    assert_eq!(list[0]["count"], 1);
    // Bob isn't listed a second time from mail.
    assert_eq!(list.len(), 2);
    assert_eq!(list[1]["emails"][0], "ana@x.test");

    let found = r.contacts("bob").await;
    assert_eq!(found[0]["name"], "Bobby Tables");
    assert_eq!(found[0]["address"], "bob@y.test");
    assert_eq!(
        found
            .iter()
            .filter(|c| c["address"] == "bob@y.test")
            .count(),
        1
    );
    // Saved ones are found by company too, and come before people from mail.
    assert_eq!(r.contacts("acme").await[0]["address"], "bob@y.test");
    assert_eq!(r.contacts("").await[0]["name"], "Bobby Tables");

    let got = r
        .call("person_get", json!({ "address": "b2@y.test" }))
        .await;
    assert_eq!(got["person"]["name"], "Bobby Tables");
    assert_eq!(got["person"]["notes"], "Met in Lisbon");
    assert_eq!(got["count"], 1);

    // Editing keeps the same person.
    let id = saved["id"].as_i64().unwrap();
    let edited = r
        .call(
            "person_save",
            json!({ "id": id, "name": "Bob Tables", "emails": ["bob@y.test"], "phones": [],
                    "company": "", "notes": "" }),
        )
        .await;
    assert_eq!(edited["id"], id);
    assert_eq!(edited["uid"], saved["uid"]);
    assert_eq!(r.people("").await[0]["name"], "Bob Tables");
}

#[tokio::test]
async fn an_address_saved_for_someone_else_is_refused() {
    let r = rig().await;
    r.call(
        "person_save",
        json!({ "name": "Ana Silva", "emails": ["ana@x.test"] }),
    )
    .await;
    let e = r
        .fails(
            "person_save",
            json!({ "name": "Other", "emails": ["Ana@x.test"] }),
        )
        .await;
    assert!(
        e.contains("ana@x.test is already saved for Ana Silva"),
        "{e}"
    );
    let e = r
        .fails("person_save", json!({ "name": " ", "emails": [] }))
        .await;
    assert!(e.contains("Add a name or an email address"), "{e}");
    let e = r
        .fails(
            "person_save",
            json!({ "name": "X", "emails": ["not-an-address"] }),
        )
        .await;
    assert!(e.contains("isn't an email address"), "{e}");
    // A name alone is fine.
    let only_name = r.call("person_save", json!({ "name": "Plumber" })).await;
    assert_eq!(only_name["emails"], json!([]));
}

#[tokio::test]
async fn removing_a_person_takes_them_off_the_list() {
    let r = rig().await;
    r.with_mail().await;
    let saved = r
        .call(
            "person_save",
            json!({ "name": "Ana S", "emails": ["ana@x.test"] }),
        )
        .await;
    let id = saved["id"].as_i64().unwrap();
    assert_eq!(
        r.call("person_delete", json!({ "id": id })).await,
        json!({ "deleted": true })
    );
    let list = r.people("").await;
    assert!(list.iter().all(|p| p["saved"] == false));
    // Still someone from mail.
    assert_eq!(list[0]["emails"][0], "ana@x.test");
    assert_eq!(list[0]["name"], "Ana Silva");
    let got = r
        .call("person_get", json!({ "address": "ana@x.test" }))
        .await;
    assert_eq!(got["person"], Value::Null);
    assert!(r
        .fails("person_delete", json!({ "id": id }))
        .await
        .contains("isn't in your address book"));
    // The address is free to save again.
    r.call(
        "person_save",
        json!({ "name": "Ana again", "emails": ["ana@x.test"] }),
    )
    .await;
}

#[tokio::test]
async fn vcards_come_in_and_go_back_out() {
    let r = rig().await;
    r.call(
        "person_save",
        json!({ "name": "Ana", "emails": ["ana@x.test"], "phones": ["111"] }),
    )
    .await;
    let file = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ana Silva\r\nEMAIL;TYPE=INTERNET:ana@x.test\r\n\
                EMAIL:ana.work@x.test\r\nTEL;TYPE=CELL:222\r\nEND:VCARD\r\n\
                BEGIN:VCARD\r\nVERSION:4.0\r\nUID:urn:uuid:carlos-1\r\nN:Mendes;Carlos;;;\r\n\
                EMAIL:carlos@z.test\r\nORG:Mendes\\, Lda;Sales\r\n\
                NOTE:A long note that goes on and on so that it has to be folded over \r\n more than one line\\, really.\r\n\
                END:VCARD\r\n\
                BEGIN:VCARD\r\nVERSION:3.0\r\nNOTE:nothing to go on\r\nEND:VCARD\r\n";
    let got = r.call("people_import_vcard", json!({ "text": file })).await;
    assert_eq!(got, json!({ "added": 1, "updated": 1, "skipped": 1 }));

    let ana = r
        .call("person_get", json!({ "address": "ana.work@x.test" }))
        .await;
    assert_eq!(ana["person"]["name"], "Ana Silva");
    assert_eq!(
        ana["person"]["emails"],
        json!(["ana@x.test", "ana.work@x.test"])
    );
    assert_eq!(ana["person"]["phones"], json!(["111", "222"]));
    let carlos = r
        .call("person_get", json!({ "address": "carlos@z.test" }))
        .await;
    assert_eq!(carlos["person"]["name"], "Carlos Mendes");
    assert_eq!(carlos["person"]["uid"], "carlos-1");
    assert_eq!(carlos["person"]["company"], "Mendes, Lda");
    assert_eq!(
        carlos["person"]["notes"],
        "A long note that goes on and on so that it has to be folded over more than one line, really."
    );

    // The same file again changes nothing.
    let again = r.call("people_import_vcard", json!({ "text": file })).await;
    assert_eq!(again, json!({ "added": 0, "updated": 0, "skipped": 3 }));

    let text = r.call("people_export_vcard", json!({})).await["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
    assert!(text.contains("ORG:Mendes\\, Lda\r\n"));
    assert!(!text.replace("\r\n", "").contains('\n'));
    assert!(text.split("\r\n").all(|l| l.len() <= 75));
    assert!(text.contains("\r\n "), "the long note is folded");
    assert_eq!(text.matches("BEGIN:VCARD").count(), 2);

    // Read into a fresh address book, it comes back the same.
    let other = rig().await;
    let got = other
        .call("people_import_vcard", json!({ "text": text }))
        .await;
    assert_eq!(got["added"], 2);
    let back = other
        .call("person_get", json!({ "address": "carlos@z.test" }))
        .await;
    for field in ["uid", "name", "emails", "phones", "company", "notes"] {
        assert_eq!(back["person"][field], carlos["person"][field], "{field}");
    }

    // And as a file in Downloads, never over one that's there.
    let saved = r.call("people_export_file", json!({})).await;
    assert_eq!(saved["name"], "Mailzo contacts.vcf");
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(path.parent().unwrap(), r.downloads);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let second = r.call("people_export_file", json!({})).await;
    assert_ne!(second["path"], saved["path"]);
    assert!(PathBuf::from(second["path"].as_str().unwrap()).starts_with(&r.downloads));
}

#[tokio::test]
async fn searching_matches_names_addresses_and_companies() {
    let r = rig().await;
    r.with_mail().await;
    r.call(
        "person_save",
        json!({ "name": "Carla Dias", "emails": ["carla@q.test"], "company": "Northwind" }),
    )
    .await;
    let names = |list: Vec<Value>| -> Vec<String> {
        list.iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(r.people("NORTH").await), ["Carla Dias"]);
    assert_eq!(names(r.people("carla@").await), ["Carla Dias"]);
    assert_eq!(names(r.people("silva").await), ["Ana Silva"]);
    assert_eq!(names(r.people("y.test").await), ["Bob"]);
    assert!(r.people("no-reply").await.is_empty());
    assert!(r.people("spam").await.is_empty());
    assert!(
        r.people("grayic").await.is_empty(),
        "your own address isn't a contact"
    );
    let one = r
        .call("people_list", json!({ "query": "", "limit": 1 }))
        .await;
    assert_eq!(one.as_array().unwrap().len(), 1);
    assert_eq!(one[0]["saved"], true);
}
