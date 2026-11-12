//! Writing, sending and acting on mail, through a fake mail server that refuses
//! changes in read-only folders and a fake outgoing server that keeps what it gets.

use data_encoding::BASE64;
use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::parse::{parse_body, parse_headers};
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const ME: &str = "me@grayic.test";
const PASS: &str = "secret-pass";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    smtp: FakeSmtp,
    account: i64,
    downloads: PathBuf,
    data: PathBuf,
    opened: Arc<Mutex<Vec<(String, bool)>>>,
    _dir: tempfile::TempDir,
}

fn mail(from: &str, to: &str, id: &str, refs: &str, subject: &str, body: &str) -> String {
    let refs = if refs.is_empty() {
        String::new()
    } else {
        format!("In-Reply-To: <{refs}>\nReferences: <{refs}>\n")
    };
    format!(
        "From: {from}\nTo: {to}\nSubject: {subject}\nDate: Mon, 02 Nov 2026 09:00:00 +0000\nMessage-ID: <{id}>\n{refs}Content-Type: text/plain; charset=utf-8\n\n{body}\n"
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
    let data = dir.path().join("data");
    let downloads = dir.path().join("downloads");
    let opened: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let log = opened.clone();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_folders(data.clone(), downloads.clone())
    .with_sent_copy_wait(0)
    .with_file_opener(Arc::new(move |path: &str, open: bool| {
        log.lock().unwrap().push((path.to_string(), open));
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
        downloads,
        data,
        opened,
        _dir: dir,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }

    async fn code(&self, cmd: &str, args: Value) -> String {
        self.engine
            .call(cmd, args)
            .await
            .unwrap_err()
            .code()
            .to_string()
    }

    async fn sync(&self) {
        let stats = self
            .call("sync_account", json!({ "account_id": self.account }))
            .await;
        assert!(stats["errors"].as_array().unwrap().is_empty(), "{stats}");
    }

    async fn folder(&self, role: &str) -> i64 {
        let list = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == role)
            .unwrap_or_else(|| panic!("no {role} folder"))["id"]
            .as_i64()
            .unwrap()
    }

    async fn threads(&self, role: &str) -> Vec<Value> {
        let id = self.folder(role).await;
        self.call("list_threads", json!({ "folder_id": id }))
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    async fn thread(&self, id: &str, view: Option<i64>) -> Vec<Value> {
        self.call(
            "get_thread",
            json!({ "account_id": self.account, "thread_id": id, "folder_id": view }),
        )
        .await
        .as_array()
        .unwrap()
        .clone()
    }

    /// The newest message in the only conversation in the inbox.
    async fn inbox_message(&self) -> (String, i64) {
        let t = self.threads("inbox").await;
        let id = t[0]["thread_id"].as_str().unwrap().to_string();
        let msgs = self.thread(&id, None).await;
        (id, msgs.last().unwrap()["id"].as_i64().unwrap())
    }

    async fn new_draft(&self, mode: &str, message: Option<i64>) -> Value {
        self.call(
            "draft_new",
            json!({ "account_id": self.account, "mode": mode, "message_id": message, "date_text": "2 Nov 2026" }),
        )
        .await
    }

    /// Saves changes to a draft, leaving everything else as it was.
    async fn save(&self, draft: &Value, changes: Value) -> Value {
        let mut args = json!({
            "id": draft["id"], "account_id": draft["account_id"], "to": draft["to"],
            "cc": draft["cc"], "bcc": draft["bcc"], "subject": draft["subject"],
            "html": draft["html"], "text": draft["text"],
        });
        for (k, v) in changes.as_object().unwrap() {
            args[k] = v.clone();
        }
        self.call("draft_save", args).await
    }
}

fn to(addresses: &[&str]) -> Value {
    Value::Array(
        addresses
            .iter()
            .map(|a| json!({ "name": "", "address": a }))
            .collect(),
    )
}

fn addresses(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|a| a["address"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_new_message_reaches_everyone_and_bcc_stays_hidden() {
    let r = rig().await;
    let d = r.new_draft("new", None).await;
    let d = r
        .save(
            &d,
            json!({
                "to": [{"name": "Ana", "address": "ana@x.test"}], "cc": to(&["cat@x.test"]),
                "bcc": to(&["secret@x.test"]), "subject": "Hello héllo",
                "html": "<p><strong>Bold</strong> hi</p><script>bad()</script>", "text": "Bold hi",
            }),
        )
        .await;
    let sent = r.call("send_draft", json!({ "id": d["id"] })).await;
    assert_eq!(sent["sent"], true);
    assert!(sent["warnings"].as_array().unwrap().is_empty(), "{sent}");

    let got = r.smtp.received();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].from, ME);
    assert_eq!(got[0].to, ["ana@x.test", "cat@x.test", "secret@x.test"]);
    assert!(!got[0].data.contains("secret@x.test"), "Bcc must not show");
    assert!(!got[0].data.to_ascii_lowercase().contains("\nbcc:"));

    let h = parse_headers(got[0].data.as_bytes());
    assert_eq!(h.subject, "Hello héllo");
    assert_eq!(h.to[0].name, "Ana");
    assert_eq!(h.cc[0].address, "cat@x.test");
    assert!(h.in_reply_to.is_empty());
    let b = parse_body(got[0].data.as_bytes());
    assert!(b.html.as_deref().unwrap().contains("<strong>Bold</strong>"));
    assert!(!b.html.as_deref().unwrap().contains("script"));
    assert!(b.text.unwrap().contains("Bold hi"));

    // A read copy in Sent, which does show the Bcc to the sender.
    let copies = r.imap.folder_messages("Sent");
    assert_eq!(copies.len(), 1);
    assert!(copies[0].flags.contains("\\Seen"));
    let raw = String::from_utf8_lossy(&copies[0].raw);
    assert!(raw.contains("secret@x.test"));
    assert_eq!(parse_headers(&copies[0].raw).rfc_id, h.rfc_id);

    // Gone from drafts once sent.
    assert!(r
        .call("draft_list", json!({}))
        .await
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_provider_that_files_sent_mail_itself_gets_no_second_copy() {
    let r = rig().await;
    r.smtp.file_copy_in(r.imap.clone(), "Sent");
    let d = r.new_draft("new", None).await;
    let d = r
        .save(
            &d,
            json!({ "to": to(&["a@x.test"]), "subject": "Once", "text": "x" }),
        )
        .await;
    r.call("send_draft", json!({ "id": d["id"] })).await;
    assert_eq!(r.imap.folder_messages("Sent").len(), 1);
}

#[tokio::test]
async fn a_reply_threads_with_the_conversation_everywhere() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail("Ana <ana@x.test>", ME, "one@x", "", "Plan", "First"),
        true,
    );
    r.imap.add_message(
        "INBOX",
        &mail(
            "Ana <ana@x.test>",
            ME,
            "two@x",
            "one@x",
            "Re: Plan",
            "Second\nline",
        ),
        false,
    );
    r.sync().await;
    let (thread, last) = r.inbox_message().await;

    let d = r.new_draft("reply", Some(last)).await;
    assert_eq!(addresses(&d["to"]), ["ana@x.test"]);
    assert_eq!(d["subject"], "Re: Plan");
    assert!(d["quoted_html"].as_str().unwrap().contains("Second"));
    assert!(d["quoted_html"]
        .as_str()
        .unwrap()
        .contains("2 Nov 2026, Ana wrote"));
    assert!(d["quoted_text"].as_str().unwrap().contains("> line"));

    let d = r
        .save(&d, json!({ "html": "<p>Mine</p>", "text": "Mine" }))
        .await;
    // The regular look knows what's there before sending.
    r.call("due_tick", json!({ "quiet_now": true })).await;
    r.call("send_draft", json!({ "id": d["id"] })).await;
    // A send that told the screen nothing (another app's, say) still gets the lists redrawn.
    let tick = r.call("due_tick", json!({ "quiet_now": true })).await;
    assert_eq!(tick["changed"], true, "{tick}");
    let tick = r.call("due_tick", json!({ "quiet_now": true })).await;
    assert_eq!(tick["changed"], false, "only once: {tick}");

    let got = r.smtp.received();
    let h = parse_headers(got[0].data.as_bytes());
    assert_eq!(h.in_reply_to, ["two@x"]);
    assert_eq!(h.references, ["one@x", "two@x"]);
    let body = parse_body(got[0].data.as_bytes());
    assert!(body.html.as_deref().unwrap().contains("Mine"));
    assert!(body.html.as_deref().unwrap().contains("blockquote"));
    // The usual mark, so mail apps (this one too) can fold the earlier message.
    assert!(body
        .html
        .as_deref()
        .unwrap()
        .contains("class=\"gmail_quote\""));
    assert!(body.text.unwrap().contains("> Second"));

    // The original gets the replied mark, and stays unread on the server until read.
    assert!(r.imap.has_flag("INBOX", 2, "\\Answered"));

    // The reply shows in the conversation as soon as it's sent, before any refresh.
    let now = r.thread(&thread, None).await;
    assert_eq!(
        now.len(),
        3,
        "the sent reply is in the conversation straight away"
    );
    assert_eq!(now[2]["from"]["address"], ME);

    // After a sync, the reply sits in the same conversation, with the inbox's count.
    r.sync().await;
    let t = r.threads("inbox").await;
    assert_eq!(t.len(), 1);
    assert_eq!(t[0]["thread_id"], thread);
    assert_eq!(t[0]["message_count"], 3);
    assert!(t[0]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "me"));
    let msgs = r.thread(&thread, None).await;
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[2]["from"]["address"], ME);
}

#[tokio::test]
async fn reply_all_and_reply_to_pick_the_right_people() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &format!(
            "From: Ana <ana@x.test>\nReply-To: desk@x.test\nTo: {ME}, bob@x.test\nCc: cat@x.test\nSubject: Team\nMessage-ID: <t@x>\nContent-Type: text/plain\n\nHi\n"
        ),
        true,
    );
    r.sync().await;
    let (_, id) = r.inbox_message().await;
    let reply = r.new_draft("reply", Some(id)).await;
    assert_eq!(addresses(&reply["to"]), ["desk@x.test"]);
    assert!(addresses(&reply["cc"]).is_empty());
    let all = r.new_draft("reply_all", Some(id)).await;
    assert_eq!(addresses(&all["to"]), ["desk@x.test", "bob@x.test"]);
    assert_eq!(addresses(&all["cc"]), ["cat@x.test"]);
}

const WITH_FILE: &str = "From: Ana <ana@x.test>\nTo: me@grayic.test\nSubject: Report\nMessage-ID: <f@x>\nMIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: text/plain\n\nSee attached\n--b\nContent-Type: application/pdf\nContent-Disposition: attachment; filename=\"../../plan.pdf\"\nContent-Transfer-Encoding: base64\n\naGVsbG8=\n--b--\n";

#[tokio::test]
async fn forwarding_starts_fresh_and_brings_the_attachments() {
    let r = rig().await;
    r.imap.add_message("INBOX", WITH_FILE, true);
    r.sync().await;
    let (_, id) = r.inbox_message().await;
    let d = r.new_draft("forward", Some(id)).await;
    assert_eq!(d["subject"], "Fwd: Report");
    assert!(addresses(&d["to"]).is_empty());
    assert!(d["quoted_html"]
        .as_str()
        .unwrap()
        .contains("Forwarded message"));
    assert_eq!(d["attachments"][0]["name"], "plan.pdf");

    let d = r
        .save(
            &d,
            json!({ "to": to(&["b@x.test"]), "text": "FYI", "html": "<p>FYI</p>" }),
        )
        .await;
    r.call("send_draft", json!({ "id": d["id"] })).await;
    let got = r.smtp.received();
    let h = parse_headers(got[0].data.as_bytes());
    assert!(
        h.in_reply_to.is_empty(),
        "a forward starts a new conversation"
    );
    let b = parse_body(got[0].data.as_bytes());
    assert_eq!(b.attachments.len(), 1);
    assert_eq!(b.attachments[0].name, "plan.pdf");
    assert_eq!(b.attachments[0].size, 5);
    assert!(b.text.unwrap().contains("See attached"));
}

#[tokio::test]
async fn files_can_be_attached_removed_and_sent_with_images_inside() {
    let r = rig().await;
    let d = r.new_draft("new", None).await;
    let id = &d["id"];
    let added = r
        .call(
            "draft_add_attachment",
            json!({ "id": id, "name": "../sneaky/notes.txt", "mime": "text/plain", "data": BASE64.encode(b"file one") }),
        )
        .await;
    assert_eq!(added["attachments"][0]["name"], "notes.txt");
    let extra = r
        .call(
            "draft_add_attachment",
            json!({ "id": id, "name": "drop.bin", "data": BASE64.encode(b"drop me") }),
        )
        .await;
    assert_eq!(extra["attachments"][1]["mime"], "application/octet-stream");
    let gone = extra["attachments"][1]["id"].as_str().unwrap();
    let after = r
        .call(
            "draft_remove_attachment",
            json!({ "id": id, "attachment_id": gone }),
        )
        .await;
    assert_eq!(after["attachments"].as_array().unwrap().len(), 1);

    let image = BASE64.encode(b"\x89PNG-image");
    let d = r
        .save(
            &after,
            json!({
                "to": to(&["a@x.test"]), "subject": "Files", "text": "see",
                "html": format!("<p>see</p><img src=\"data:image/png;base64,{image}\">"),
            }),
        )
        .await;
    r.call("send_draft", json!({ "id": d["id"] })).await;

    let raw = r.smtp.received()[0].data.clone();
    assert!(
        !raw.contains("data:image/png"),
        "images travel as proper parts"
    );
    let b = parse_body(raw.as_bytes());
    assert!(
        b.html.as_deref().unwrap().contains("data:image/png"),
        "and show when read"
    );
    let files: Vec<&str> = b
        .attachments
        .iter()
        .filter(|a| !a.inline)
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(files, ["notes.txt"]);

    // Files of a sent draft don't stay behind.
    assert!(!r.data.join("drafts").join(d["id"].to_string()).exists());
}

#[tokio::test]
async fn sending_problems_keep_the_draft_and_explain_themselves() {
    let r = rig().await;
    let d = r.new_draft("new", None).await;
    assert_eq!(
        r.code("send_draft", json!({ "id": d["id"] })).await,
        "no_recipients"
    );

    let bad = r
        .save(&d, json!({ "to": to(&["a@x.test\r\nBcc: evil@x.test"]) }))
        .await;
    assert_eq!(
        r.code("send_draft", json!({ "id": bad["id"] })).await,
        "bad_address"
    );

    let d = r
        .save(
            &d,
            json!({ "to": to(&["a@x.test"]), "subject": "Evil\r\nBcc: evil@x.test" }),
        )
        .await;
    r.smtp.set_login(ME, "changed-password");
    assert_eq!(
        r.code("send_draft", json!({ "id": d["id"] })).await,
        "smtp_auth"
    );
    assert_eq!(
        r.call("draft_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    r.smtp.set_login(ME, PASS);
    r.smtp.refuse_with("550 not allowed");
    assert_eq!(
        r.code("send_draft", json!({ "id": d["id"] })).await,
        "smtp_refused"
    );
    assert!(r.smtp.received().is_empty());
    assert_eq!(
        r.call("draft_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        r.imap.folder_messages("Sent").is_empty(),
        "no false Sent copy"
    );
}

#[tokio::test]
async fn subjects_cannot_smuggle_in_extra_headers() {
    let r = rig().await;
    let d = r.new_draft("new", None).await;
    let d = r
        .save(
            &d,
            json!({ "to": to(&["a@x.test"]), "subject": "Hi\r\nBcc: evil@x.test" }),
        )
        .await;
    r.call("send_draft", json!({ "id": d["id"] })).await;
    let got = &r.smtp.received()[0];
    assert_eq!(got.to, ["a@x.test"]);
    assert!(parse_headers(got.data.as_bytes()).bcc.is_empty());
}

#[tokio::test]
async fn attachments_over_the_limit_are_refused_before_sending() {
    let r = rig().await;
    let d = r.new_draft("new", None).await;
    let big = vec![0u8; 26 * 1024 * 1024];
    let code = r
        .code(
            "draft_add_attachment",
            json!({ "id": d["id"], "name": "big.bin", "data": BASE64.encode(&big) }),
        )
        .await;
    assert_eq!(code, "too_big");
}

#[tokio::test]
async fn an_unencrypted_outgoing_server_is_refused_unless_on_this_computer() {
    let r = rig().await;
    let err = r
        .engine
        .call(
            "update_account_settings",
            json!({
                "account_id": r.account, "smtp_host": "mail.example.com", "smtp_port": 25,
                "smtp_security": "plain",
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
}

#[tokio::test]
async fn the_name_and_signature_are_used_when_writing() {
    let r = rig().await;
    r.call(
        "update_account_settings",
        json!({
            "account_id": r.account, "display_name": "Sam Lee", "signature": "Sam\n<Grayic>",
            "smtp_host": "127.0.0.1", "smtp_port": r.smtp.addr.port(), "smtp_security": "plain",
        }),
    )
    .await;
    let d = r.new_draft("new", None).await;
    let html = d["html"].as_str().unwrap();
    assert!(
        html.contains("<p>Sam</p>") && html.contains("&lt;Grayic&gt;"),
        "{html}"
    );
    let d = r
        .save(&d, json!({ "to": to(&["a@x.test"]), "text": "x" }))
        .await;
    r.call("send_draft", json!({ "id": d["id"] })).await;
    let h = parse_headers(r.smtp.received()[0].data.as_bytes());
    assert_eq!(h.from.unwrap().name, "Sam Lee");
}

#[tokio::test]
async fn a_finished_draft_can_be_discarded_with_its_files() {
    let r = rig().await;
    let d = r.new_draft("new", None).await;
    r.call(
        "draft_add_attachment",
        json!({ "id": d["id"], "name": "a.txt", "data": BASE64.encode(b"x") }),
    )
    .await;
    let dir = r.data.join("drafts").join(d["id"].to_string());
    assert!(dir.exists());
    r.call("draft_discard", json!({ "id": d["id"] })).await;
    assert!(!dir.exists());
}

// Reading state and moving mail.

#[tokio::test]
async fn read_and_star_reach_the_server_and_survive_a_sync() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), false);
    r.imap.add_message(
        "INBOX",
        &mail("a@x.test", ME, "2@x", "1@x", "Re: One", "y"),
        false,
    );
    r.sync().await;
    let (t, _) = r.inbox_message().await;
    let args = |extra: Value| {
        let mut base = json!({ "account_id": r.account, "thread_id": t });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    };

    r.call("set_thread_seen", args(json!({ "seen": true })))
        .await;
    assert!(r.imap.is_seen("INBOX", 1) && r.imap.is_seen("INBOX", 2));
    r.sync().await;
    assert_eq!(r.threads("inbox").await[0]["unread_count"], 0);

    r.call("set_thread_seen", args(json!({ "seen": false })))
        .await;
    assert!(!r.imap.is_seen("INBOX", 1));

    r.call("set_thread_flagged", args(json!({ "flagged": true })))
        .await;
    assert!(r.imap.has_flag("INBOX", 2, "\\Flagged"));
    assert!(!r.imap.has_flag("INBOX", 1, "\\Flagged"));
    r.sync().await;
    assert_eq!(r.threads("inbox").await[0]["flagged"], true);

    r.call("set_thread_flagged", args(json!({ "flagged": false })))
        .await;
    assert!(!r.imap.has_flag("INBOX", 2, "\\Flagged"));

    // Syncing itself still only ever reads.
    assert_eq!(r.imap.non_peek_fetches(), 0);
}

#[tokio::test]
async fn archive_and_delete_move_mail_and_it_stays_moved() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.imap
        .add_message("INBOX", &mail("b@x.test", ME, "2@x", "", "Two", "y"), true);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    let threads = r.threads("inbox").await;

    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": threads[0]["thread_id"], "folder_id": inbox, "to": "archive" }),
    )
    .await;
    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": threads[1]["thread_id"], "folder_id": inbox, "to": "trash" }),
    )
    .await;
    assert!(r.imap.folder_messages("INBOX").is_empty());
    assert_eq!(r.imap.folder_messages("Archive").len(), 1);
    assert_eq!(r.imap.folder_messages("Trash").len(), 1);
    assert!(r.threads("inbox").await.is_empty(), "gone right away");

    r.sync().await;
    assert!(r.threads("inbox").await.is_empty(), "and not brought back");
    assert_eq!(r.threads("archive").await.len(), 1);
    assert_eq!(r.threads("trash").await.len(), 1);
}

#[tokio::test]
async fn moved_mail_is_in_its_new_folder_without_waiting_for_a_refresh() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.imap
        .add_message("INBOX", &mail("b@x.test", ME, "2@x", "", "Two", "y"), true);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    let threads = r.threads("inbox").await;

    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": threads[0]["thread_id"], "folder_id": inbox, "to": "trash" }),
    )
    .await;
    let binned = r.threads("trash").await;
    assert_eq!(binned.len(), 1, "one at a time");
    assert_eq!(binned[0]["subject"], threads[0]["subject"]);

    r.call(
        "bulk_threads",
        json!({
            "threads": [{ "account_id": r.account, "thread_id": threads[1]["thread_id"], "folder_id": inbox }],
            "action": "move",
            "to": "archive",
        }),
    )
    .await;
    let filed = r.threads("archive").await;
    assert_eq!(filed.len(), 1, "several at a time");
    assert_eq!(filed[0]["subject"], threads[1]["subject"]);
}

#[tokio::test]
async fn reading_all_marks_them_read_here_and_on_the_server() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), false);
    r.imap
        .add_message("INBOX", &mail("b@x.test", ME, "2@x", "", "Two", "y"), false);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    let threads: Vec<Value> = r
        .threads("inbox")
        .await
        .iter()
        .map(
            |t| json!({ "account_id": r.account, "thread_id": t["thread_id"], "folder_id": inbox }),
        )
        .collect();
    let done = r
        .call(
            "bulk_threads",
            json!({ "threads": threads, "action": "seen" }),
        )
        .await;
    assert_eq!(done["done"], 2, "{done}");
    assert!(r
        .threads("inbox")
        .await
        .iter()
        .all(|t| t["unread_count"] == 0));
    assert!(r.imap.has_flag("INBOX", 1, "\\Seen") && r.imap.has_flag("INBOX", 2, "\\Seen"));
}

/// Two unread emails in the inbox, and how to name both conversations to `bulk_threads`.
async fn two_unread(r: &Rig) -> Vec<Value> {
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), false);
    r.imap
        .add_message("INBOX", &mail("b@x.test", ME, "2@x", "", "Two", "y"), false);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    r.threads("inbox")
        .await
        .iter()
        .map(
            |t| json!({ "account_id": r.account, "thread_id": t["thread_id"], "folder_id": inbox }),
        )
        .collect()
}

async fn unread_here(r: &Rig) -> usize {
    r.threads("inbox")
        .await
        .iter()
        .filter(|t| t["unread_count"] != 0)
        .count()
}

#[tokio::test]
async fn a_refresh_before_the_server_hears_doesnt_undo_reading_all() {
    let r = rig().await;
    let threads = two_unread(&r).await;
    // A slow server: a refresh reads its old marks while it's still being told.
    r.imap.state.lock().unwrap().store_wait = Some(std::time::Duration::from_millis(600));
    let (done, during) = tokio::join!(
        r.call(
            "bulk_threads",
            json!({ "threads": threads, "action": "seen" })
        ),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            r.sync().await;
            unread_here(&r).await
        }
    );
    assert_eq!(done["done"], 2, "{done}");
    assert_eq!(during, 0, "they don't come back as unread");
    assert!(r.imap.has_flag("INBOX", 1, "\\Seen") && r.imap.has_flag("INBOX", 2, "\\Seen"));
    r.sync().await;
    assert_eq!(unread_here(&r).await, 0);
}

#[tokio::test]
async fn reading_all_shows_here_first_then_tells_the_servers() {
    let r = rig().await;
    let threads = two_unread(&r).await;
    // Here first, so every window can show it straight away.
    r.call(
        "bulk_threads",
        json!({ "threads": threads, "action": "seen", "step": "here" }),
    )
    .await;
    assert_eq!(unread_here(&r).await, 0);
    assert!(
        !r.imap.has_flag("INBOX", 1, "\\Seen"),
        "the server isn't told yet"
    );
    // A refresh now reads the server's old marks, and leaves these alone.
    r.sync().await;
    assert_eq!(unread_here(&r).await, 0);
    let done = r
        .call(
            "bulk_threads",
            json!({ "threads": threads, "action": "seen", "step": "servers" }),
        )
        .await;
    assert_eq!(done["done"], 2, "{done}");
    assert!(r.imap.has_flag("INBOX", 1, "\\Seen") && r.imap.has_flag("INBOX", 2, "\\Seen"));
    r.sync().await;
    assert_eq!(unread_here(&r).await, 0);
}

/// A refresh that brought nothing says so, and the windows leave everything as it is. One
/// that only saw mail read elsewhere says that.
#[tokio::test]
async fn a_refresh_says_when_nothing_changed() {
    let r = rig().await;
    two_unread(&r).await;
    let refresh = || r.call("sync_account", json!({ "account_id": r.account }));
    let quiet = refresh().await;
    for what in [
        "new_messages",
        "removed_messages",
        "flags_changed",
        "filtered",
        "older_saved",
    ] {
        assert_eq!(quiet[what], 0, "{what}: {quiet}");
    }
    r.imap.set_flag("INBOX", 1, "\\Seen", true);
    let read_elsewhere = refresh().await;
    assert_eq!(read_elsewhere["flags_changed"], 1, "{read_elsewhere}");
}

#[tokio::test]
async fn a_move_only_touches_the_folder_it_names() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    let archive = r.folder("archive").await;
    let thread = r.threads("inbox").await[0]["thread_id"].clone();
    let move_from = |folder: i64, to: &'static str| {
        r.call(
            "bulk_threads",
            json!({
                "threads": [{ "account_id": r.account, "thread_id": thread, "folder_id": folder }],
                "action": "move",
                "to": to,
            }),
        )
    };
    move_from(inbox, "archive").await;
    // Nothing's in the inbox now, so this doesn't go looking for it in other folders.
    move_from(inbox, "trash").await;
    assert_eq!(r.threads("archive").await.len(), 1, "left alone");
    assert_eq!(r.threads("trash").await.len(), 0);
    // Where it really is, it moves.
    move_from(archive, "trash").await;
    assert_eq!(r.threads("archive").await.len(), 0);
    assert_eq!(r.threads("trash").await.len(), 1, "from where it really is");
}

#[tokio::test]
async fn servers_without_move_still_work_and_touch_only_the_chosen_mail() {
    let r = rig().await;
    r.imap.state.lock().unwrap().no_move = true;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    let other = r
        .imap
        .add_message("INBOX", &mail("b@x.test", ME, "2@x", "", "Two", "y"), true);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    let threads = r.threads("inbox").await;
    let one = threads.iter().find(|t| t["subject"] == "One").unwrap();
    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": one["thread_id"], "folder_id": inbox, "to": "archive" }),
    )
    .await;
    assert_eq!(r.imap.folder_messages("Archive").len(), 1);
    let left = r.imap.folder_messages("INBOX");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].uid, other);
}

#[tokio::test]
async fn archive_uses_all_mail_on_gmail_and_creates_a_folder_elsewhere() {
    let r = rig().await;
    {
        let mut s = r.imap.state.lock().unwrap();
        s.folders.retain(|f| f.name != "Archive");
    }
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.sync().await;
    let inbox = r.folder("inbox").await;
    let (t, _) = r.inbox_message().await;
    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": t, "folder_id": inbox, "to": "archive" }),
    )
    .await;
    assert_eq!(r.imap.folder_messages("Archive").len(), 1);

    // Gmail's All Mail takes over once it exists, and stays hidden from the list.
    r.imap.add_folder("[Gmail]/All Mail", &["\\All"], 9);
    {
        let mut s = r.imap.state.lock().unwrap();
        s.folders.retain(|f| f.name != "Archive");
    }
    r.imap
        .add_message("INBOX", &mail("b@x.test", ME, "2@x", "", "Two", "y"), true);
    r.sync().await;
    let (t, _) = r.inbox_message().await;
    r.call(
        "move_thread",
        json!({ "account_id": r.account, "thread_id": t, "folder_id": inbox, "to": "archive" }),
    )
    .await;
    assert_eq!(r.imap.folder_messages("[Gmail]/All Mail").len(), 1);
}

#[tokio::test]
async fn deleting_for_good_only_works_from_the_bin() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.imap
        .add_message("Trash", &mail("b@x.test", ME, "2@x", "", "Two", "y"), true);
    let keep = r.imap.add_message(
        "Trash",
        &mail("c@x.test", ME, "3@x", "", "Three", "z"),
        true,
    );
    r.sync().await;
    let (t, _) = r.inbox_message().await;
    let inbox = r.folder("inbox").await;
    assert_eq!(
        r.code(
            "delete_forever",
            json!({ "account_id": r.account, "thread_id": t, "folder_id": inbox })
        )
        .await,
        "invalid"
    );
    assert_eq!(r.imap.folder_messages("INBOX").len(), 1);

    let trash = r.folder("trash").await;
    let list = r.threads("trash").await;
    let two = list.iter().find(|t| t["subject"] == "Two").unwrap();
    r.call(
        "delete_forever",
        json!({ "account_id": r.account, "thread_id": two["thread_id"], "folder_id": trash }),
    )
    .await;
    let left = r.imap.folder_messages("Trash");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].uid, keep);
}

// Conversations.

#[tokio::test]
async fn a_reply_that_lost_its_ids_still_joins_but_lookalikes_do_not() {
    let r = rig().await;
    r.imap.add_message_dated(
        "INBOX",
        &mail("Ana <ana@x.test>", ME, "1@x", "", "Budget", "x"),
        true,
        "01-Nov-2026 10:00:00 +0000",
    );
    r.sync().await;
    // A reply from a mail app that dropped References, days later.
    r.imap.add_message_dated(
        "Sent",
        &mail(ME, "ana@x.test", "2@x", "", "RE: Budget", "y"),
        true,
        "03-Nov-2026 10:00:00 +0000",
    );
    // Same subject, but from a stranger.
    r.imap.add_message_dated(
        "INBOX",
        &mail("spam@x.test", ME, "3@x", "", "Re: Budget", "z"),
        true,
        "03-Nov-2026 11:00:00 +0000",
    );
    // Same people and subject, but not marked as a reply.
    r.imap.add_message_dated(
        "INBOX",
        &mail("ana@x.test", ME, "4@x", "", "Budget", "z"),
        true,
        "03-Nov-2026 12:00:00 +0000",
    );
    // A reply, far too late to belong.
    r.imap.add_message_dated(
        "INBOX",
        &mail("ana@x.test", ME, "5@x", "", "Re: Budget", "z"),
        true,
        "15-Mar-2027 12:00:00 +0000",
    );
    r.sync().await;
    let list = r.threads("inbox").await;
    assert_eq!(list.len(), 4, "{list:?}");
    let joined = list.iter().find(|t| t["message_count"] == 2).unwrap();
    let msgs = r.thread(joined["thread_id"].as_str().unwrap(), None).await;
    assert_eq!(msgs[1]["rfc_id"], "2@x");
}

#[tokio::test]
async fn replies_to_answers_this_device_never_saw_still_join_one_conversation() {
    let r = rig().await;
    // Nazim answers twice, each time to a reply sent from another app that didn't keep
    // it in Sent, so neither email's IDs lead to anything here.
    r.imap.add_message_dated(
        "INBOX",
        &mail(
            "Nazim <nazim@x.test>",
            ME,
            "n1@x",
            "gone-1@me",
            "Re: Project",
            "a",
        ),
        true,
        "02-Nov-2026 10:00:00 +0000",
    );
    r.imap.add_message_dated(
        "INBOX",
        &mail(
            "Nazim <nazim@x.test>",
            ME,
            "n2@x",
            "gone-2@me",
            "Re: Project",
            "b",
        ),
        true,
        "03-Nov-2026 10:00:00 +0000",
    );
    // Someone else on the same subject stays apart.
    r.imap.add_message_dated(
        "INBOX",
        &mail(
            "Ola <ola@x.test>",
            ME,
            "o1@x",
            "gone-3@me",
            "Re: Project",
            "c",
        ),
        true,
        "03-Nov-2026 11:00:00 +0000",
    );
    r.sync().await;
    let list = r.threads("inbox").await;
    assert_eq!(list.len(), 2, "{list:?}");
    assert!(list.iter().any(|t| t["message_count"] == 2), "{list:?}");
}

#[tokio::test]
async fn binned_copies_stay_out_of_the_conversation_unless_viewing_the_bin() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.imap.add_message(
        "Trash",
        &mail("a@x.test", ME, "2@x", "1@x", "Re: One", "y"),
        false,
    );
    r.sync().await;
    let list = r.threads("inbox").await;
    assert_eq!(list[0]["message_count"], 1);
    assert_eq!(list[0]["unread_count"], 0);
    let id = list[0]["thread_id"].as_str().unwrap();
    assert_eq!(r.thread(id, None).await.len(), 1);
    let trash = r.folder("trash").await;
    assert_eq!(r.thread(id, Some(trash)).await.len(), 2);
}

// An email's original.

#[tokio::test]
async fn the_original_of_an_email_is_shown_and_saved_exactly_as_it_came() {
    let r = rig().await;
    let raw = format!(
        "Authentication-Results: mx.test; dkim=pass header.i=@x.test; spf=softfail smtp.mailfrom=x.test;\r\n dmarc=fail\r\nAuthentication-Results: fake.test; spf=pass; dmarc=pass\r\n{}",
        mail("a@x.test", ME, "1@x", "", "Plans/2026", "hello")
    );
    r.imap.add_message("INBOX", &raw, false);
    r.sync().await;
    let (_, id) = r.inbox_message().await;

    let source = r.call("message_source", json!({ "message_id": id })).await;
    assert!(source["text"]
        .as_str()
        .unwrap()
        .contains("Subject: Plans/2026"));
    assert_eq!(source["cut_short"], false);
    assert!(source["message_id"].as_str().unwrap().contains("1@x"));
    assert_eq!(source["dkim"], "pass");
    assert_eq!(
        source["spf"], "softfail",
        "only the provider's own note counts"
    );
    assert_eq!(source["dmarc"], "fail");

    let saved = r.call("save_original", json!({ "message_id": id })).await;
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(path, r.downloads.join("Plans-2026.eml"));
    let file = std::fs::read_to_string(&path).unwrap();
    assert_eq!(file.replace("\r\n", "\n"), raw.replace("\r\n", "\n"));
    assert!(!r.imap.is_seen("INBOX", 1), "looking doesn't mark it read");
}

// Attachments arriving.

#[tokio::test]
async fn saved_attachments_land_safely_and_only_those_can_be_opened() {
    let r = rig().await;
    r.imap.add_message("INBOX", WITH_FILE, false);
    r.sync().await;
    let (_, id) = r.inbox_message().await;
    let saved = r
        .call("save_attachment", json!({ "message_id": id, "part": 0 }))
        .await;
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(path, r.downloads.join("plan.pdf"), "stays in Downloads");
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert_eq!(saved["can_open"], true);
    assert!(!r.imap.is_seen("INBOX", 1), "saving doesn't mark it read");

    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(out.status.success(), "macOS must treat it as a download");
    }

    let again = r
        .call("save_attachment", json!({ "message_id": id, "part": 0 }))
        .await;
    assert_eq!(again["name"], "plan (1).pdf", "never overwrites");

    r.call(
        "use_saved_file",
        json!({ "path": saved["path"], "open": true }),
    )
    .await;
    assert_eq!(
        r.opened.lock().unwrap()[0],
        (saved["path"].as_str().unwrap().to_string(), true)
    );
    let refused = r
        .code(
            "use_saved_file",
            json!({ "path": "/etc/hosts", "open": true }),
        )
        .await;
    assert_eq!(refused, "invalid");
    assert_eq!(r.opened.lock().unwrap().len(), 1);
    assert_eq!(
        r.code("save_attachment", json!({ "message_id": id, "part": 5 }))
            .await,
        "not_found"
    );
}

#[tokio::test]
async fn a_pdf_can_be_looked_at_without_saving_it() {
    let r = rig().await;
    r.imap.add_message("INBOX", WITH_FILE, false);
    r.sync().await;
    let (_, id) = r.inbox_message().await;
    let shown = r
        .call("attachment_preview", json!({ "message_id": id, "part": 0 }))
        .await;
    assert_eq!(shown["kind"], "pdf");
    assert_eq!(shown["mime"], "application/pdf");
    assert_eq!(shown["data"], "aGVsbG8=");
    assert_eq!(
        std::fs::read_dir(&r.downloads)
            .map(|d| d.count())
            .unwrap_or(0),
        0,
        "nothing is saved"
    );
}

#[tokio::test]
async fn program_attachments_can_only_be_shown_in_finder() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &WITH_FILE.replace("../../plan.pdf", "Invoice.pdf.app"),
        false,
    );
    r.sync().await;
    let (_, id) = r.inbox_message().await;
    let saved = r
        .call("save_attachment", json!({ "message_id": id, "part": 0 }))
        .await;
    assert_eq!(saved["can_open"], false);
    assert_eq!(
        r.code(
            "use_saved_file",
            json!({ "path": saved["path"], "open": true })
        )
        .await,
        "invalid"
    );
    r.call(
        "use_saved_file",
        json!({ "path": saved["path"], "open": false }),
    )
    .await;
    assert_eq!(r.opened.lock().unwrap().len(), 1);
    assert!(!r.opened.lock().unwrap()[0].1);
}

#[tokio::test]
async fn contacts_come_from_mail_people_have_exchanged() {
    let r = rig().await;
    r.imap.add_message(
        "INBOX",
        &mail("Ana Silva <ana@x.test>", ME, "1@x", "", "Hi", "x"),
        true,
    );
    r.imap.add_message(
        "INBOX",
        &mail("no-reply@x.test", ME, "2@x", "", "Robot", "x"),
        true,
    );
    r.imap.add_message(
        "Sent",
        &mail(ME, "Bob <bob@y.test>", "3@x", "", "Hello", "x"),
        true,
    );
    r.sync().await;
    let find = |q: &'static str| {
        let account = r.account;
        let engine = &r.engine;
        async move {
            engine
                .call("contacts", json!({ "account_id": account, "query": q }))
                .await
                .unwrap()
        }
    };
    assert_eq!(find("silva").await[0]["address"], "ana@x.test");
    assert_eq!(find("bob").await[0]["name"], "Bob");
    assert!(find("no-reply").await.as_array().unwrap().is_empty());
}

// All Inbox.

#[tokio::test]
async fn all_inbox_mixes_every_account_newest_first_and_acts_on_the_right_one() {
    let r = rig().await;
    let second = FakeServer::start("two@grayic.test", PASS).await;
    second.add_folder("INBOX", &[], 1);
    second.add_folder("Archive", &["\\Archive"], 1);
    let two = r
        .call(
            "add_account",
            json!({
                "email": "two@grayic.test", "imap_host": "127.0.0.1", "imap_port": second.addr.port(),
                "security": "plain", "password": PASS,
            }),
        )
        .await;
    let two = two["id"].as_i64().unwrap();

    r.imap.add_message_dated(
        "INBOX",
        &mail("a@x.test", ME, "old@x", "", "Older", "x"),
        false,
        "01-Nov-2026 10:00:00 +0000",
    );
    second.add_message_dated(
        "INBOX",
        &mail("b@x.test", "two@grayic.test", "new@x", "", "Newer", "y"),
        false,
        "02-Nov-2026 10:00:00 +0000",
    );
    // The same email sent to both inboxes.
    let both = mail("c@x.test", "everyone@x.test", "both@x", "", "To both", "z");
    r.imap
        .add_message_dated("INBOX", &both, true, "03-Nov-2026 10:00:00 +0000");
    second.add_message_dated("INBOX", &both, true, "03-Nov-2026 10:00:00 +0000");
    // Mail outside the inbox stays out.
    r.imap.add_message_dated(
        "Archive",
        &mail("d@x.test", ME, "arch@x", "", "Archived", "w"),
        true,
        "04-Nov-2026 10:00:00 +0000",
    );
    r.sync().await;
    r.call("sync_account", json!({ "account_id": two })).await;

    let list = r.call("list_all", json!({ "role": "inbox" })).await;
    let list = list.as_array().unwrap();
    let subjects: Vec<&str> = list
        .iter()
        .map(|t| t["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["To both", "To both", "Newer", "Older"]);
    assert_eq!(list[0]["account_email"], ME);
    assert_eq!(list[1]["account_email"], "two@grayic.test");
    assert_eq!(list[2]["account_id"], two);
    assert_eq!(list[3]["account_email"], ME);
    assert_ne!(list[0]["folder_id"], list[1]["folder_id"]);

    // Other folders work the same way.
    let archived = r.call("list_all", json!({ "role": "archive" })).await;
    assert_eq!(archived[0]["subject"], "Archived");
    assert_eq!(archived.as_array().unwrap().len(), 1);

    // Paging works across inboxes.
    let page = r
        .call(
            "list_all",
            json!({ "role": "inbox", "limit": 2, "offset": 2 }),
        )
        .await;
    assert_eq!(page[0]["subject"], "Newer");
    assert_eq!(page.as_array().unwrap().len(), 2);

    // Same conversation name in both inboxes, but only the chosen copy changes.
    let copy = &list[1];
    r.call(
        "move_thread",
        json!({ "account_id": copy["account_id"], "thread_id": copy["thread_id"],
                "folder_id": copy["folder_id"], "to": "archive" }),
    )
    .await;
    assert_eq!(second.folder_messages("Archive").len(), 1);
    assert_eq!(
        r.imap.folder_messages("INBOX").len(),
        2,
        "the other inbox is untouched"
    );

    let newer = &list[2];
    r.call(
        "set_thread_seen",
        json!({ "account_id": newer["account_id"], "thread_id": newer["thread_id"], "seen": true }),
    )
    .await;
    assert!(second.is_seen("INBOX", 1));
    assert!(!r.imap.is_seen("INBOX", 1));

    // A disconnected inbox leaves the list.
    r.call("remove_account", json!({ "account_id": two })).await;
    let after = r.call("list_all", json!({ "role": "inbox" })).await;
    assert!(after
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["account_email"] == ME));
}

// Acting on many conversations at once.

impl Rig {
    async fn bulk(&self, threads: &[Value], action: &str, to: Option<&str>) -> Value {
        let list: Vec<Value> = threads
            .iter()
            .map(|t| json!({ "account_id": t["account_id"], "thread_id": t["thread_id"], "folder_id": t["folder_id"] }))
            .collect();
        self.call(
            "bulk_threads",
            json!({ "threads": list, "action": action, "to": to }),
        )
        .await
    }
}

#[tokio::test]
async fn many_conversations_can_be_acted_on_with_one_connection() {
    let r = rig().await;
    for n in 1..=5 {
        r.imap.add_message(
            "INBOX",
            &mail(
                "a@x.test",
                ME,
                &format!("{n}@x"),
                "",
                &format!("Mail {n}"),
                "x",
            ),
            false,
        );
    }
    r.sync().await;
    let list = r.threads("inbox").await;
    let chosen = &list[..3];
    let keep = list[3]["subject"].clone();

    let before = r.imap.state.lock().unwrap().logins;
    let reply = r.bulk(chosen, "seen", None).await;
    assert_eq!(reply["done"], 3);
    assert_eq!(
        r.imap.state.lock().unwrap().logins,
        before + 1,
        "one sign-in"
    );
    let seen = r
        .imap
        .folder_messages("INBOX")
        .iter()
        .filter(|m| m.flags.contains("\\Seen"))
        .count();
    assert_eq!(seen, 3);

    r.bulk(chosen, "unseen", None).await;
    assert!(r
        .imap
        .folder_messages("INBOX")
        .iter()
        .all(|m| !m.flags.contains("\\Seen")));

    r.bulk(chosen, "star", None).await;
    let starred = r
        .imap
        .folder_messages("INBOX")
        .iter()
        .filter(|m| m.flags.contains("\\Flagged"))
        .count();
    assert_eq!(starred, 3);
    r.bulk(chosen, "unstar", None).await;
    assert!(r
        .imap
        .folder_messages("INBOX")
        .iter()
        .all(|m| !m.flags.contains("\\Flagged")));

    let before = r.imap.state.lock().unwrap().logins;
    r.bulk(chosen, "move", Some("archive")).await;
    assert_eq!(
        r.imap.state.lock().unwrap().logins,
        before + 1,
        "one sign-in"
    );
    assert_eq!(r.imap.folder_messages("Archive").len(), 3);
    assert_eq!(r.imap.folder_messages("INBOX").len(), 2);
    let left = r.threads("inbox").await;
    assert_eq!(left.len(), 2, "gone right away");
    assert!(left.iter().any(|t| t["subject"] == keep));
    r.sync().await;
    assert_eq!(r.threads("inbox").await.len(), 2, "and not brought back");
    assert_eq!(r.threads("archive").await.len(), 3);
}

#[tokio::test]
async fn deleting_many_for_good_is_refused_outside_the_bin() {
    let r = rig().await;
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "One", "x"), true);
    r.imap
        .add_message("Trash", &mail("a@x.test", ME, "2@x", "", "Two", "x"), true);
    r.imap.add_message(
        "Trash",
        &mail("a@x.test", ME, "3@x", "", "Three", "x"),
        true,
    );
    r.sync().await;
    let inbox = r.threads("inbox").await;
    let threads = [
        json!({ "account_id": inbox[0]["account_id"], "thread_id": inbox[0]["thread_id"], "folder_id": inbox[0]["folder_id"] }),
    ];
    assert_eq!(
        r.code(
            "bulk_threads",
            json!({ "threads": threads, "action": "delete_forever" })
        )
        .await,
        "invalid"
    );
    assert_eq!(r.imap.folder_messages("INBOX").len(), 1);

    let trash = r.threads("trash").await;
    r.bulk(&trash, "delete_forever", None).await;
    assert!(r.imap.folder_messages("Trash").is_empty());
}

#[tokio::test]
async fn a_selection_across_inboxes_acts_on_each_and_reports_a_problem_with_one() {
    let r = rig().await;
    let second = FakeServer::start("two@grayic.test", PASS).await;
    second.add_folder("INBOX", &[], 1);
    second.add_folder("Archive", &["\\Archive"], 1);
    let two = r
        .call(
            "add_account",
            json!({
                "email": "two@grayic.test", "imap_host": "127.0.0.1", "imap_port": second.addr.port(),
                "security": "plain", "password": PASS,
            }),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    r.imap
        .add_message("INBOX", &mail("a@x.test", ME, "1@x", "", "Mine", "x"), true);
    second.add_message(
        "INBOX",
        &mail("b@x.test", "two@grayic.test", "2@x", "", "Theirs", "y"),
        true,
    );
    r.sync().await;
    r.call("sync_account", json!({ "account_id": two })).await;
    let all = r.call("list_all", json!({ "role": "inbox" })).await;
    let all = all.as_array().unwrap().clone();
    assert_eq!(all.len(), 2);

    // The second inbox's password stops working, and the first still gets done.
    second.state.lock().unwrap().pass = "changed".into();
    let reply = r.bulk(&all, "move", Some("archive")).await;
    assert_eq!(reply["done"], 1);
    assert!(
        reply["problem"].as_str().unwrap().contains("rejected"),
        "{reply}"
    );
    assert_eq!(r.imap.folder_messages("Archive").len(), 1);
    assert_eq!(second.folder_messages("INBOX").len(), 1);

    second.state.lock().unwrap().pass = PASS.into();
    let left = r.call("list_all", json!({ "role": "inbox" })).await;
    let reply = r
        .bulk(left.as_array().unwrap(), "move", Some("archive"))
        .await;
    assert_eq!(reply["done"], 1);
    assert_eq!(second.folder_messages("Archive").len(), 1);
}
