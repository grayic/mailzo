//! Features brought over from the CRM: the person's own tags, a conversation's files and
//! links, explaining an email, the writing style profile, drafts that check themselves,
//! follow-ups that stop on a reply, and the monthly AI allowance.

use fakeai::FakeAi;
use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";
const NEW_MAIL: &str = "01-Nov-2036 10:00:00 +0000";

struct Rig {
    server: FakeServer,
    outgoing: FakeSmtp,
    engine: Engine,
    ai: FakeAi,
    account: i64,
}

fn mail(id: &str, from: &str, subject: &str, body: &str) -> String {
    format!(
        "From: {from}\nTo: Me <{USER}>\nSubject: {subject}\nMessage-ID: <{id}>\n\
         Date: Sun, 01 Nov 2026 10:00:00 +0000\nContent-Type: text/plain; charset=utf-8\n\n{body}\n"
    )
}

async fn rig_with(build: impl FnOnce(Engine) -> Engine) -> Rig {
    rig_on(Arc::new(MemoryStore::default()), build).await
}

/// With passwords and keys kept where the test can reach them too.
async fn rig_on(secrets: Arc<MemoryStore>, build: impl FnOnce(Engine) -> Engine) -> Rig {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    server.add_folder("Archive", &["\\Archive"], 1);
    server.add_folder("Drafts", &["\\Drafts"], 1);
    server.add_folder("Junk", &["\\Junk"], 1);
    server.add_folder("Trash", &["\\Trash"], 1);
    let outgoing = FakeSmtp::start(USER, PASS).await;
    let ai = FakeAi::start().await;
    let engine = Engine::new(Store::open_in_memory().unwrap(), secrets)
        .with_sent_copy_wait(0)
        .with_ai_url(&ai.url);
    let engine = build(engine);
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
        engine,
        ai,
        account,
    }
}

async fn rig() -> Rig {
    rig_with(|e| e).await
}

use std::sync::Arc;

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
        self.call("sync_account", json!({ "account_id": self.account }))
            .await;
    }

    fn add(&self, id: &str, from: &str, subject: &str, body: &str) {
        self.server
            .add_message_dated("INBOX", &mail(id, from, subject, body), false, NEW_MAIL);
    }

    async fn folder(&self, role: &str) -> i64 {
        let list = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == role)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    }

    async fn inbox(&self) -> Vec<Value> {
        let id = self.folder("inbox").await;
        self.call("list_threads", json!({ "folder_id": id }))
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    async fn turn_on_ai(&self) {
        self.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
            .await;
    }
}

#[tokio::test]
async fn a_tag_with_an_amount_shows_on_the_conversation_and_lists_its_mail() {
    let r = rig().await;
    r.add(
        "inv@t",
        "Ana <ana@client.test>",
        "Invoice for March",
        "Here it is",
    );
    r.add("hi@t", "Bo <bo@x.test>", "Hello", "Hi");
    r.sync().await;
    let thread = r
        .inbox()
        .await
        .into_iter()
        .find(|t| t["subject"] == "Invoice for March")
        .unwrap();

    let paid = r
        .call("tag_save", json!({ "name": "Paid", "colour": "green" }))
        .await;
    let paid_id = paid["id"].as_i64().unwrap();
    assert!(r
        .fails("tag_save", json!({ "name": "Late", "colour": "#f00" }))
        .await
        .contains("colour"));

    let on = r
        .call(
            "thread_tag",
            json!({
                "account_id": r.account, "thread_id": thread["thread_id"],
                "tag_id": paid_id, "on": true, "amount": 450.0,
            }),
        )
        .await;
    assert_eq!(on, json!([{ "tag_id": paid_id, "amount": 450.0 }]));

    let list = r.inbox().await;
    let tagged = list
        .iter()
        .find(|t| t["subject"] == "Invoice for March")
        .unwrap();
    assert_eq!(
        tagged["tags"],
        json!([{ "tag_id": paid_id, "amount": 450.0 }])
    );
    let untagged = list.iter().find(|t| t["subject"] == "Hello").unwrap();
    assert_eq!(untagged["tags"], json!([]));

    let only = r.call("tagged_threads", json!({ "tag_id": paid_id })).await;
    let subjects: Vec<&str> = only
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, vec!["Invoice for March"]);
    assert_eq!(r.call("tags_list", json!({})).await[0]["count"], 1);

    // Deleting the tag leaves the mail where it was.
    r.call("tag_delete", json!({ "tag_id": paid_id })).await;
    assert_eq!(r.inbox().await.len(), 2);
    assert_eq!(r.inbox().await[0]["tags"], json!([]));
}

#[tokio::test]
async fn a_conversation_lists_its_files_and_links_newest_first() {
    let r = rig().await;
    let with_file = format!(
        "From: Ana <ana@client.test>\nTo: {USER}\nSubject: Brief\nMessage-ID: <b1@t>\n\
         Date: Sun, 01 Nov 2026 09:00:00 +0000\nMIME-Version: 1.0\n\
         Content-Type: multipart/mixed; boundary=\"XX\"\n\n--XX\n\
         Content-Type: text/html; charset=utf-8\n\n\
         <p>The <a href=\"https://docs.test/brief\">project brief</a> and \
         <a href=\"https://news.test/u\">unsubscribe</a></p>\n--XX\n\
         Content-Type: application/pdf; name=\"quote.pdf\"\n\
         Content-Disposition: attachment; filename=\"quote.pdf\"\n\
         Content-Transfer-Encoding: base64\n\nJVBERi0xLjQK\n--XX--\n"
    );
    r.server
        .add_message_dated("INBOX", &with_file, false, NEW_MAIL);
    r.server.add_message_dated(
        "INBOX",
        &format!(
            "From: Ana <ana@client.test>\nTo: {USER}\nSubject: Re: Brief\nMessage-ID: <b2@t>\n\
             In-Reply-To: <b1@t>\nReferences: <b1@t>\nDate: Sun, 01 Nov 2026 10:00:00 +0000\n\
             Content-Type: text/plain; charset=utf-8\n\n\
             Also see https://figma.test/file/9. And again https://docs.test/brief\n"
        ),
        false,
        NEW_MAIL,
    );
    r.sync().await;
    let thread = r.inbox().await[0].clone();
    assert_eq!(thread["message_count"], 2);
    let shared = r
        .call(
            "thread_shared",
            json!({ "account_id": r.account, "thread_id": thread["thread_id"] }),
        )
        .await;
    let files = shared["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "quote.pdf");
    assert_eq!(files[0]["from"], "Ana");
    let links: Vec<(&str, &str)> = shared["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["url"].as_str().unwrap(), l["text"].as_str().unwrap()))
        .collect();
    // The brief counts once, credited to the email that shared it first.
    assert_eq!(
        links,
        vec![
            ("https://figma.test/file/9", ""),
            ("https://docs.test/brief", "project brief"),
        ]
    );
}

#[tokio::test]
async fn explaining_an_email_uses_the_quick_model_once_until_a_new_email_arrives() {
    let r = rig().await;
    r.add(
        "e1@t",
        "Legal <legal@corp.test>",
        "Notice of renewal",
        "Pursuant to clause 4.2 the agreement renews unless notice is served.",
    );
    r.sync().await;
    let thread = r.inbox().await[0].clone();
    let args = json!({ "account_id": r.account, "thread_id": thread["thread_id"] });
    assert!(r.fails("ai_explain", args.clone()).await.contains("key"));
    r.turn_on_ai().await;
    // Opening the email never asks the AI.
    let before = r.ai.requests().len();
    let mut saved = args.clone();
    saved["saved_only"] = json!(true);
    assert_eq!(r.call("ai_explain", saved.clone()).await, Value::Null);
    assert_eq!(r.ai.requests().len(), before);

    r.ai.answer(
        "clause 4.2",
        json!({
            "meaning": "Your contract renews by itself.",
            "wants": "Tell them if you want to stop it.",
            "tone": "formal",
            "watch_out": "Miss the notice and you pay another year.",
        }),
    );
    let explained = r.call("ai_explain", args.clone()).await;
    assert_eq!(explained["meaning"], "Your contract renews by itself.");
    let asked = r.ai.requests();
    assert_eq!(asked.len(), before + 1);
    assert_eq!(asked.last().unwrap()["model"], "claude-haiku-5-5");

    // Kept: asking again, or opening it again, is free.
    r.call("ai_explain", args.clone()).await;
    assert_eq!(r.call("ai_explain", saved).await["tone"], "formal");
    assert_eq!(r.ai.requests().len(), before + 1);

    // A reply in the conversation means it's explained afresh.
    r.server.add_message_dated(
        "INBOX",
        "From: Legal <legal@corp.test>\nTo: me@test.test\nSubject: Re: Notice of renewal\n\
         Message-ID: <e2@t>\nIn-Reply-To: <e1@t>\nReferences: <e1@t>\n\
         Date: Sun, 01 Nov 2026 11:00:00 +0000\nContent-Type: text/plain\n\nAlso clause 4.2 again.\n",
        false,
        "02-Nov-2036 10:00:00 +0000",
    );
    r.sync().await;
    r.call("ai_explain", args).await;
    assert_eq!(r.ai.requests().len(), before + 2);
}

fn sent(id: &str, to: &str, body: &str) -> String {
    format!(
        "From: Me <{USER}>\nTo: {to}\nSubject: Note\nMessage-ID: <{id}>\n\
         Date: Sun, 01 Nov 2026 10:00:00 +0000\nContent-Type: text/plain; charset=utf-8\n\n{body}\n"
    )
}

#[tokio::test]
async fn the_writing_style_is_made_once_used_for_drafts_and_can_be_rewritten_by_hand() {
    let r = rig().await;
    for (i, body) in [
        "Hey! Sounds great. Cheers, M",
        "Hey, yes please. Cheers, M",
        "Hey all, done. Cheers, M",
    ]
    .iter()
    .enumerate()
    {
        r.server
            .add_message("Sent", &sent(&format!("s{i}@t"), "x@y.test", body), true);
    }
    r.add(
        "ask@t",
        "Ana <ana@client.test>",
        "Lunch",
        "Lunch on Friday?",
    );
    r.sync().await;
    let account = json!({ "account_id": r.account });
    assert_eq!(r.call("style_get", account.clone()).await, Value::Null);
    r.turn_on_ai().await;

    r.ai.answer(
        "emails written by",
        json!({ "profile": "- Starts with Hey\n- Signs off Cheers, M" }),
    );
    r.ai.answer(
        "lunch on friday",
        json!({ "reply": "Hey! Friday works. Cheers, M" }),
    );
    let message = r.inbox().await[0]["thread_id"].clone();
    let thread = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": message }),
        )
        .await;
    let message_id = thread[0]["id"].as_i64().unwrap();

    let before = r.ai.requests().len();
    r.call("ai_write_reply", json!({ "message_id": message_id }))
        .await;
    let asked = r.ai.requests();
    // One quick look at their sent mail, then the draft written with what it found.
    assert_eq!(asked.len(), before + 2);
    assert_eq!(asked[before]["model"], "claude-haiku-5-5");
    let draft_instructions = asked[before + 1]["system"][0]["text"].as_str().unwrap();
    assert!(draft_instructions.contains("Signs off Cheers, M"));
    assert_eq!(
        asked[before + 1]["system"][0]["cache_control"]["type"],
        "ephemeral"
    );
    let profile = r.call("style_get", account.clone()).await;
    assert_eq!(profile["edited"], false);

    // The next draft reuses it.
    r.call("ai_write_reply", json!({ "message_id": message_id }))
        .await;
    assert_eq!(r.ai.requests().len(), before + 3);

    // Their own words win, and are what drafts use from then on.
    let mine = r
        .call(
            "style_save",
            json!({ "account_id": r.account, "text": "- Always say thanks first" }),
        )
        .await;
    assert_eq!(mine["edited"], true);
    r.call("ai_write_reply", json!({ "message_id": message_id }))
        .await;
    let last = r.ai.requests().last().unwrap().clone();
    assert!(last["system"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Always say thanks first"));

    // Making it again on request replaces theirs.
    let remade = r.call("style_make", account.clone()).await;
    assert_eq!(remade["edited"], false);
    // Clearing it goes back to learning from their mail.
    assert_eq!(
        r.call(
            "style_save",
            json!({ "account_id": r.account, "text": " " })
        )
        .await,
        Value::Null
    );
}

#[tokio::test]
async fn the_writing_style_only_reads_mail_ai_is_allowed_to_read() {
    let r = rig().await;
    for i in 0..3 {
        r.server.add_message(
            "Sent",
            &sent(&format!("s{i}@t"), "x@y.test", "Hey, done. Cheers, M"),
            true,
        );
    }
    r.sync().await;
    r.turn_on_ai().await;
    let account = json!({ "account_id": r.account });
    let accounts = r.call("list_accounts", json!({})).await;
    let mut prefs = accounts[0]["prefs"].clone();

    // Their Sent folder is kept away from AI.
    prefs["ai_skip_folders"] = json!(["Sent"]);
    r.call(
        "update_account_prefs",
        json!({ "account_id": r.account, "prefs": prefs }),
    )
    .await;
    let asked = r.ai.requests().len();
    assert!(r
        .fails("style_make", account.clone())
        .await
        .contains("isn't enough sent mail"));

    // And then the whole inbox.
    prefs["ai_skip_folders"] = json!([]);
    prefs["ai_allowed"] = json!(false);
    r.call(
        "update_account_prefs",
        json!({ "account_id": r.account, "prefs": prefs }),
    )
    .await;
    assert!(r
        .fails("style_make", account.clone())
        .await
        .contains("isn't allowed"));
    assert_eq!(r.ai.requests().len(), asked);
}

#[tokio::test]
async fn a_writing_style_made_from_mail_ai_may_no_longer_read_is_dropped() {
    let r = rig().await;
    for i in 0..3 {
        r.server.add_message(
            "Sent",
            &sent(&format!("s{i}@t"), "x@y.test", "Hey, done. Cheers, M"),
            true,
        );
    }
    r.add(
        "ask@t",
        "Ana <ana@client.test>",
        "Lunch",
        "Lunch on Friday?",
    );
    r.sync().await;
    r.turn_on_ai().await;
    let account = json!({ "account_id": r.account });
    r.ai.answer(
        "emails written by",
        json!({ "profile": "- Starts with Hey" }),
    );
    r.ai.answer("lunch on friday", json!({ "reply": "Friday works." }));
    r.call("style_make", account.clone()).await;
    let thread = r.inbox().await[0]["thread_id"].clone();
    let message_id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();

    // Sent mail is then kept from AI.
    let accounts = r.call("list_accounts", json!({})).await;
    let mut prefs = accounts[0]["prefs"].clone();
    prefs["ai_skip_folders"] = json!(["Sent"]);
    r.call(
        "update_account_prefs",
        json!({ "account_id": r.account, "prefs": prefs }),
    )
    .await;
    r.call("ai_write_reply", json!({ "message_id": message_id }))
        .await;
    let last = r.ai.requests().last().unwrap().clone();
    let instructions = last["system"][0]["text"].as_str().unwrap();
    assert!(!instructions.contains("Starts with Hey"));
    assert!(!instructions.contains("Cheers, M"));
    assert_eq!(r.call("style_get", account).await, Value::Null);
}

#[tokio::test]
async fn best_quality_drafts_are_checked_and_rewritten_at_most_once() {
    let r = rig().await;
    r.add(
        "q@t",
        "Ana <ana@client.test>",
        "Prices",
        "What does the pro plan cost?",
    );
    r.sync().await;
    r.turn_on_ai().await;
    let thread = r.inbox().await[0]["thread_id"].clone();
    let id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    // The fake answers the first question that matches, so the check and the rewrite are
    // told apart by words only they contain.
    r.ai.answer(
        "a reviewer found",
        json!({ "reply": "The pro plan is $20 a month." }),
    );
    r.ai.answer(
        "<draft>",
        json!({ "score": 3, "problem": "It doesn't give the price." }),
    );
    r.ai.answer("pro plan cost", json!({ "reply": "Thanks for asking!" }));

    // Off by default: one request, no check.
    let before = r.ai.requests().len();
    let plain = r.call("ai_write_reply", json!({ "message_id": id })).await;
    assert_eq!(plain["reply"], "Thanks for asking!");
    assert_eq!(r.ai.requests().len(), before + 1);

    r.call(
        "set_setting",
        json!({ "key": "ai_best_drafts", "value": "on" }),
    )
    .await;
    let before = r.ai.requests().len();
    let best = r.call("ai_write_reply", json!({ "message_id": id })).await;
    assert_eq!(best["reply"], "The pro plan is $20 a month.");
    assert_eq!(best["improved"], true);
    let asked = r.ai.requests();
    // Draft, quick check, one rewrite. Never a second check.
    assert_eq!(asked.len(), before + 3);
    assert_eq!(asked[before + 1]["model"], "claude-haiku-5-5");
}

#[tokio::test]
async fn a_good_best_quality_draft_is_kept_as_it_is() {
    let r = rig().await;
    r.add(
        "q@t",
        "Ana <ana@client.test>",
        "Prices",
        "What does the pro plan cost?",
    );
    r.sync().await;
    r.turn_on_ai().await;
    r.call(
        "set_setting",
        json!({ "key": "ai_best_drafts", "value": "on" }),
    )
    .await;
    let thread = r.inbox().await[0]["thread_id"].clone();
    let id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    r.ai.answer("<draft>", json!({ "score": 9, "problem": "" }));
    r.ai.answer("pro plan cost", json!({ "reply": "It's $20 a month." }));
    let before = r.ai.requests().len();
    let best = r.call("ai_write_reply", json!({ "message_id": id })).await;
    assert_eq!(best["reply"], "It's $20 a month.");
    assert_eq!(best["improved"], false);
    assert_eq!(r.ai.requests().len(), before + 2);
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn follow_ups_go_on_schedule_and_stop_when_they_reply() {
    let r = rig().await;
    r.server.add_message(
        "Sent",
        &format!(
            "From: Me <{USER}>\nTo: Ana Lima <ana@client.test>\nSubject: Proposal\n\
             Message-ID: <p1@t>\nDate: Sun, 01 Nov 2026 10:00:00 +0000\n\
             Content-Type: text/plain\n\nHere's the proposal.\n"
        ),
        true,
    );
    r.add("other@t", "Bo <bo@x.test>", "Unrelated", "Hi");
    r.sync().await;
    let sent_folder = r.folder("sent").await;
    let thread = r
        .call("list_threads", json!({ "folder_id": sent_folder }))
        .await[0]["thread_id"]
        .clone();
    let message_id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();

    let chase = r
        .call(
            "sequence_save",
            json!({ "sequence": {
                "name": "Chase a proposal",
                "steps": [
                    { "days": 2, "body": "Hi {first_name},\n\nAny thoughts on the proposal?" },
                    { "days": 3, "body": "Hi {first_name}, one last nudge." },
                ],
            }}),
        )
        .await;
    let seq_id = chase["id"].as_i64().unwrap();

    // Only after an email the person sent.
    let theirs = r.inbox().await[0]["thread_id"].clone();
    let their_id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": theirs }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    assert!(r
        .fails(
            "sequence_start",
            json!({ "message_id": their_id, "sequence_id": seq_id })
        )
        .await
        .contains("you sent"));

    let run = r
        .call(
            "sequence_start",
            json!({ "message_id": message_id, "sequence_id": seq_id }),
        )
        .await;
    assert_eq!(run["state"], "running");
    assert_eq!(run["steps"], 2);
    // One at a time per conversation.
    assert!(r
        .fails(
            "sequence_start",
            json!({ "message_id": message_id, "sequence_id": seq_id })
        )
        .await
        .contains("already"));

    // Nothing before it's due.
    let start = now();
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": start + 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);

    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": start + 2 * 86_400 + 60 }),
    )
    .await;
    let out = r.outgoing.received();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].to, vec!["ana@client.test"]);
    assert!(out[0].data.contains("Subject: Re: Proposal"));
    assert!(out[0].data.contains("Hi Ana,"));
    assert!(out[0].data.contains("In-Reply-To: <p1@t>"));
    let runs = r
        .call("sequence_runs", json!({ "account_id": r.account }))
        .await;
    assert_eq!(runs[0]["sent"], 1);
    assert_eq!(runs[0]["state"], "running");

    // Ana answers, so the last nudge never goes.
    r.server.add_message_dated(
        "INBOX",
        "From: Ana Lima <ana@client.test>\nTo: me@test.test\nSubject: Re: Proposal\n\
         Message-ID: <a1@t>\nIn-Reply-To: <p1@t>\nReferences: <p1@t>\n\
         Date: Sun, 01 Nov 2026 12:00:00 +0000\nContent-Type: text/plain\n\nLooks good!\n",
        false,
        NEW_MAIL,
    );
    r.sync().await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": start + 30 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 1);
    let runs = r
        .call(
            "sequence_runs",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await;
    assert_eq!(runs[0]["state"], "replied");
}

#[tokio::test]
async fn stopping_or_deleting_follow_ups_means_nothing_more_goes() {
    let r = rig().await;
    r.server.add_message(
        "Sent",
        &format!(
            "From: Me <{USER}>\nTo: Bo <bo@x.test>\nSubject: Quote\nMessage-ID: <q1@t>\n\
             Date: Sun, 01 Nov 2026 10:00:00 +0000\nContent-Type: text/plain\n\nQuote attached.\n"
        ),
        true,
    );
    r.sync().await;
    let sent_folder = r.folder("sent").await;
    let thread = r
        .call("list_threads", json!({ "folder_id": sent_folder }))
        .await[0]["thread_id"]
        .clone();
    let message_id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    let seq = r
        .call(
            "sequence_save",
            json!({ "sequence": { "name": "Nudge", "steps": [{ "days": 1, "body": "Any news?" }] } }),
        )
        .await;
    let run = r
        .call(
            "sequence_start",
            json!({ "message_id": message_id, "sequence_id": seq["id"] }),
        )
        .await;
    r.call("sequence_stop", json!({ "id": run["id"] })).await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 5 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);

    // Started again, then the follow-ups themselves are deleted.
    r.call(
        "sequence_start",
        json!({ "message_id": message_id, "sequence_id": seq["id"] }),
    )
    .await;
    r.call("sequence_delete", json!({ "id": seq["id"] })).await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 5 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);
    let runs = r
        .call("sequence_runs", json!({ "account_id": r.account }))
        .await;
    assert!(runs
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["state"] == "stopped"));
}

/// The person's proposal to Ana, synced, with one follow-up two days later on the way.
/// Returns the conversation and the run.
async fn proposal_with_a_follow_up(r: &Rig) -> (Value, Value) {
    r.server.add_message(
        "Sent",
        &format!(
            "From: Me <{USER}>\nTo: Ana Lima <ana@client.test>\nSubject: Proposal\n\
             Message-ID: <p1@t>\nDate: Sun, 01 Nov 2026 10:00:00 +0000\n\
             Content-Type: text/plain\n\nHere's the proposal.\n"
        ),
        true,
    );
    r.sync().await;
    let sent_folder = r.folder("sent").await;
    let thread = r
        .call("list_threads", json!({ "folder_id": sent_folder }))
        .await[0]["thread_id"]
        .clone();
    let message_id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    let seq = r
        .call(
            "sequence_save",
            json!({ "sequence": { "name": "Nudge", "steps": [
                { "days": 2, "body": "Any thoughts?" },
                { "days": 2, "body": "One last nudge." },
            ] } }),
        )
        .await;
    let run = r
        .call(
            "sequence_start",
            json!({ "message_id": message_id, "sequence_id": seq["id"] }),
        )
        .await;
    (thread, run)
}

fn anas_reply(id: &str, references: &str) -> String {
    format!(
        "From: Ana Lima <ana@client.test>\nTo: {USER}\nSubject: Re: Proposal\n\
         Message-ID: <{id}>\nIn-Reply-To: <p1@t>\nReferences: {references}\n\
         Date: Sun, 01 Nov 2026 12:00:00 +0000\nContent-Type: text/plain\n\nLooks good!\n"
    )
}

async fn run_state(r: &Rig, run: &Value) -> Value {
    let runs = r
        .call("sequence_runs", json!({ "account_id": r.account }))
        .await;
    runs.as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == run["id"])
        .unwrap()
        .clone()
}

#[tokio::test]
async fn a_reply_on_the_server_not_yet_brought_down_stops_the_follow_up() {
    let r = rig().await;
    let (_, run) = proposal_with_a_follow_up(&r).await;
    // Ana answered while the app was closed, and this device hasn't fetched it yet.
    r.server
        .add_message_dated("INBOX", &anas_reply("a1@t", "<p1@t>"), false, NEW_MAIL);
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 3 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);
    assert_eq!(run_state(&r, &run).await["state"], "replied");
}

#[tokio::test]
async fn a_follow_up_waits_when_the_mail_server_cant_be_checked() {
    let r = rig().await;
    let (_, run) = proposal_with_a_follow_up(&r).await;
    r.server.go_offline().await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 3 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);
    let state = run_state(&r, &run).await;
    assert_eq!(state["state"], "running");
    assert_eq!(state["sent"], 0);
    // Long past due, it says why it's waiting.
    assert!(state["problem"]
        .as_str()
        .unwrap()
        .contains("mail server can be checked"));
}

#[tokio::test]
async fn a_reply_that_came_just_before_the_follow_ups_started_still_counts() {
    let r = rig().await;
    // Ana answered two hours after the proposal went, and it's brought down only after
    // the follow-ups were started.
    let (_, run) = proposal_with_a_follow_up(&r).await;
    r.server.add_message_dated(
        "INBOX",
        &anas_reply("a1@t", "<p1@t>"),
        false,
        "01-Nov-2026 12:00:00 +0000",
    );
    r.sync().await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 3 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);
    assert_eq!(run_state(&r, &run).await["state"], "replied");
}

#[tokio::test]
async fn all_inboxes_read_page_by_page_give_the_same_list_as_all_at_once() {
    let r = rig().await;
    for i in 0..7 {
        r.server.add_message_dated(
            "INBOX",
            &mail(
                &format!("m{i}@t"),
                "Bo <bo@x.test>",
                &format!("Note {i}"),
                "Hi",
            ),
            false,
            &format!("0{}-Nov-2036 10:00:00 +0000", i + 1),
        );
    }
    r.sync().await;
    let names = |list: Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|t| t["thread_id"].as_str().unwrap().to_string())
            .collect()
    };
    let whole = names(
        r.call("list_all", json!({ "role": "inbox", "limit": 50 }))
            .await,
    );
    assert_eq!(whole.len(), 7);
    let mut paged = Vec::new();
    for offset in [0, 3, 6] {
        paged.extend(names(
            r.call(
                "list_all",
                json!({ "role": "inbox", "limit": 3, "offset": offset }),
            )
            .await,
        ));
    }
    assert_eq!(paged, whole);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_checks_at_once_send_a_follow_up_only_once() {
    let r = rig().await;
    let (_, run) = proposal_with_a_follow_up(&r).await;
    let at = now() + 3 * 86_400;
    let tick = || {
        r.engine
            .call("due_tick", json!({ "quiet_now": true, "at": at }))
    };
    let (a, b) = tokio::join!(tick(), tick());
    a.unwrap();
    b.unwrap();
    assert_eq!(r.outgoing.received().len(), 1);
    let state = run_state(&r, &run).await;
    assert_eq!(state["sent"], 1);
    assert_eq!(state["state"], "running");
    // And again later, still once each.
    r.call("due_tick", json!({ "quiet_now": true, "at": at }))
        .await;
    assert_eq!(r.outgoing.received().len(), 1);
}

#[tokio::test]
async fn older_mail_brought_down_later_is_not_taken_for_a_reply() {
    let r = rig().await;
    let (_, run) = proposal_with_a_follow_up(&r).await;
    // An old email from Ana in the same conversation, saved here only now, as when older
    // mail is brought down or mail is moved between folders.
    r.server.add_message_dated(
        "Archive",
        &anas_reply("old@t", "<p1@t>"),
        true,
        "01-Oct-2026 10:00:00 +0000",
    );
    r.sync().await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 3 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 1);
    assert_eq!(run_state(&r, &run).await["sent"], 1);
}

#[tokio::test]
async fn a_reply_filed_as_spam_still_stops_the_follow_ups() {
    let r = rig().await;
    let (_, run) = proposal_with_a_follow_up(&r).await;
    r.server
        .add_message_dated("Junk", &anas_reply("a1@t", "<p1@t>"), false, NEW_MAIL);
    r.sync().await;
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 3 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);
    assert_eq!(run_state(&r, &run).await["state"], "replied");
}

#[tokio::test]
async fn follow_ups_move_with_their_conversation_when_two_are_joined() {
    let r = rig().await;
    // An earlier email from Bo, in a conversation of its own for now.
    r.add("a0@t", "Bo <bo@x.test>", "Intro", "Meet Ana.");
    let (thread, run) = proposal_with_a_follow_up(&r).await;
    // Ana's reply refers to both, so they become one conversation, named after Bo's.
    r.server.add_message_dated(
        "INBOX",
        &anas_reply("a1@t", "<a0@t> <p1@t>"),
        false,
        NEW_MAIL,
    );
    r.sync().await;
    let inbox = r.inbox().await;
    assert_eq!(inbox.len(), 1);
    let joined = inbox[0]["thread_id"].clone();
    assert_ne!(joined, thread);
    let runs = r
        .call(
            "sequence_runs",
            json!({ "account_id": r.account, "thread_id": joined }),
        )
        .await;
    assert_eq!(runs[0]["id"], run["id"]);
    // Followed there, the reply stops it.
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": now() + 3 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 0);
    assert_eq!(run_state(&r, &run).await["state"], "replied");
}

/// Stands in for a paid plan's AI: moves writing to the quick model past 50 cents, and
/// stops at a dollar. The real rules live with the service that plugs it in.
struct TestPlan;

impl mailcore::allowance::IncludedAi for TestPlan {
    fn key(&self) -> String {
        fakeai::GOOD_KEY.into()
    }

    fn allow(&self, spent: mailcore::allowance::Spent, writing: bool) -> mailcore::Result<bool> {
        if spent.month >= 1.0 {
            return Err(mailcore::CoreError::Ai {
                code: "ai_resting".into(),
                message: "Resting.".into(),
            });
        }
        Ok(writing && spent.month < 0.5)
    }
}

#[tokio::test]
async fn included_ai_is_a_plug_in_that_decides_each_request_and_counts_its_cost() {
    let r = rig_with(|e| e.with_included_ai(Arc::new(TestPlan))).await;
    r.add(
        "q@t",
        "Ana <ana@client.test>",
        "Prices",
        "What does the pro plan cost?",
    );
    without_quick_replies_ahead(&r).await;
    let thread = r.inbox().await[0]["thread_id"].clone();
    let id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    r.ai.answer("pro plan cost", json!({ "reply": "It's $20." }));
    // Each reply costs 30 cents on the balanced model: 100k in at $2/M, 10k out at $10/M.
    r.ai.set_usage(100_000, 10_000, 0);
    let status = r.call("ai_status", json!({})).await;
    assert_eq!(status["has_key"], true);
    assert_eq!(status["included"], true);
    assert_eq!(r.call("ai_spend", json!({})).await["dollars"], Value::Null);

    let write = json!({ "message_id": id });
    r.call("ai_write_reply", write.clone()).await;
    r.call("ai_write_reply", write.clone()).await;
    let spent = r.engine.included_ai_spent().unwrap();
    assert!((spent.month - 0.6).abs() < 1e-9);
    // The plug-in said quick model only from here.
    r.call("ai_write_reply", write.clone()).await;
    let models: Vec<String> =
        r.ai.requests()
            .iter()
            .map(|q| q["model"].as_str().unwrap().to_string())
            .collect();
    assert_eq!(
        models,
        vec!["claude-sonnet-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]
    );
    // Its message stops the request before anything is asked of the AI.
    r.ai.set_usage(10_000_000, 0, 0);
    r.call("ai_write_reply", write.clone()).await;
    let asked = r.ai.requests().len();
    assert_eq!(r.fails("ai_write_reply", write.clone()).await, "Resting.");
    assert_eq!(r.ai.requests().len(), asked);

    // The person's own key comes first and isn't counted as included.
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    let before = r.engine.included_ai_spent().unwrap();
    r.call("ai_write_reply", write).await;
    assert_eq!(r.engine.included_ai_spent().unwrap(), before);
    assert_eq!(r.call("ai_status", json!({})).await["included"], false);
}

/// A refresh without quick replies written ahead, so only what a test asks for reaches the AI.
async fn without_quick_replies_ahead(r: &Rig) {
    let set = |on: &str| json!({ "key": "ai_replies", "value": on });
    r.call("set_setting", set("off")).await;
    r.sync().await;
    r.call("set_setting", set("on")).await;
}

async fn a_question_from_ana(r: &Rig) -> Value {
    r.add(
        "q@t",
        "Ana <ana@client.test>",
        "Prices",
        "What does the pro plan cost?",
    );
    without_quick_replies_ahead(r).await;
    let thread = r.inbox().await[0]["thread_id"].clone();
    let id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    r.ai.answer("pro plan cost", json!({ "reply": "It's $20." }));
    json!({ "message_id": id })
}

/// Included AI where the person adds their own key while a request is on its way.
struct KeyAddedMidway(Arc<MemoryStore>);

impl mailcore::allowance::IncludedAi for KeyAddedMidway {
    fn key(&self) -> String {
        fakeai::GOOD_KEY.into()
    }

    fn allow(&self, _: mailcore::allowance::Spent, writing: bool) -> mailcore::Result<bool> {
        use mailcore::secrets::SecretStore;
        self.0.set("ai-key", "sk-their-own").unwrap();
        Ok(writing)
    }
}

#[tokio::test]
async fn a_request_sent_on_included_ai_is_counted_there_even_if_a_key_is_added_meanwhile() {
    let secrets = Arc::new(MemoryStore::default());
    let plan = KeyAddedMidway(secrets.clone());
    let r = rig_on(secrets, |e| e.with_included_ai(Arc::new(plan))).await;
    let write = a_question_from_ana(&r).await;
    r.ai.set_usage(100_000, 10_000, 0);
    r.call("ai_write_reply", write).await;
    let spent = r.engine.included_ai_spent().unwrap();
    assert!((spent.month - 0.3).abs() < 1e-9, "{spent:?}");
}

/// Included AI with a limit of one cent, to see what requests side by side are told.
struct OneCent;

impl mailcore::allowance::IncludedAi for OneCent {
    fn key(&self) -> String {
        fakeai::GOOD_KEY.into()
    }

    fn allow(&self, spent: mailcore::allowance::Spent, writing: bool) -> mailcore::Result<bool> {
        if spent.month >= 0.01 {
            return Err(mailcore::CoreError::Ai {
                code: "ai_resting".into(),
                message: "Resting.".into(),
            });
        }
        Ok(writing)
    }
}

#[tokio::test]
async fn included_requests_side_by_side_count_what_each_other_may_cost() {
    let r = rig_with(|e| e.with_included_ai(Arc::new(OneCent))).await;
    let write = a_question_from_ana(&r).await;
    // Free in the end, but nobody knows that until the answers are back.
    r.ai.set_usage(0, 0, 0);
    let ask = || r.engine.call("ai_write_reply", write.clone());
    let (a, b) = tokio::join!(ask(), ask());
    let refused = [&a, &b].iter().filter(|x| x.is_err()).count();
    assert_eq!(refused, 1, "{a:?} {b:?}");
    // What was set aside is given back once the answers are in.
    r.call("ai_write_reply", write.clone()).await;
}

#[tokio::test]
async fn reused_instructions_are_counted_at_their_lower_price() {
    let r = rig_with(|e| e.with_included_ai(Arc::new(TestPlan))).await;
    r.add(
        "e1@t",
        "Legal <legal@corp.test>",
        "Renewal",
        "Clause 4.2 renews it.",
    );
    r.sync().await;
    let thread = r.inbox().await[0]["thread_id"].clone();
    r.ai.answer(
        "clause 4.2",
        json!({ "meaning": "It renews.", "wants": "", "tone": "formal", "watch_out": "" }),
    );
    // Quick model: 1M read back from the cache costs $0.01, not $0.10.
    r.ai.set_usage(0, 0, 1_000_000);
    r.call(
        "ai_explain",
        json!({ "account_id": r.account, "thread_id": thread }),
    )
    .await;
    let spent = r.engine.included_ai_spent().unwrap();
    assert!((spent.month - 0.01).abs() < 1e-9);
}

#[tokio::test]
async fn a_draft_can_be_polished_or_written_from_notes_and_only_its_words_go_to_ai() {
    let r = rig().await;
    let ask =
        |how: &str| json!({ "text": "hi bob, can u send the report by friday thx", "how": how });
    assert!(r.fails("ai_polish", ask("fix")).await.contains("key"));
    r.turn_on_ai().await;
    r.ai.answer(
        "can u send the report",
        json!({ "text": "Hi Bob, could you send the report by Friday? Thanks." }),
    );
    let fixed = r.call("ai_polish", ask("fix")).await;
    assert_eq!(
        fixed["text"],
        "Hi Bob, could you send the report by Friday? Thanks."
    );
    let sent = r.ai.requests().last().unwrap().to_string();
    assert!(sent.contains("Fix spelling"), "{sent}");

    r.ai.answer(
        "dinner on saturday",
        json!({ "text": "Would you like to have dinner on Saturday?" }),
    );
    let written = r
        .call("ai_polish", json!({ "text": "ask ana about dinner on saturday", "how": "write", "account_id": r.account }))
        .await;
    assert_eq!(
        written["text"],
        "Would you like to have dinner on Saturday?"
    );

    assert!(r
        .fails("ai_polish", ask("shout"))
        .await
        .contains("Bad request"));
    assert!(r
        .fails("ai_polish", json!({ "text": "  ", "how": "fix" }))
        .await
        .contains("Write something"));
}

#[tokio::test]
async fn follow_ups_set_up_in_the_conversation_keep_their_own_steps() {
    let r = rig().await;
    r.server.add_message(
        "Sent",
        &format!(
            "From: Me <{USER}>\nTo: Ana Lima <ana@client.test>\nSubject: Proposal\n\
             Message-ID: <p1@t>\nDate: Sun, 01 Nov 2026 10:00:00 +0000\n\
             Content-Type: text/plain\n\nHere's the proposal.\n"
        ),
        true,
    );
    r.sync().await;
    let sent_folder = r.folder("sent").await;
    let thread = r
        .call("list_threads", json!({ "folder_id": sent_folder }))
        .await[0]["thread_id"]
        .clone();
    let message_id = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread }),
        )
        .await[0]["id"]
        .as_i64()
        .unwrap();
    let start = now();

    // A date before the step it follows, or in the past, isn't allowed.
    assert!(r
        .fails(
            "sequence_start",
            json!({ "message_id": message_id, "steps": [
                { "days": 0, "at": start - 60, "body": "Too late" },
            ] })
        )
        .await
        .contains("after the one before"));
    // An empty message is only allowed when the AI writes it.
    assert!(r
        .fails(
            "sequence_start",
            json!({ "message_id": message_id, "steps": [{ "days": 1, "body": " " }] })
        )
        .await
        .contains("Write what"));

    let run = r
        .call(
            "sequence_start",
            json!({ "message_id": message_id, "steps": [
                { "days": 1, "body": "Hi {first_name}, any news?" },
                { "days": 0, "at": start + 5 * 86_400, "body": "", "ai": true },
            ] }),
        )
        .await;
    assert_eq!(run["state"], "running");
    assert_eq!(run["steps"], 2);
    assert_eq!(run["sequence_name"], "Follow-ups");
    // Nothing was saved to the list of follow-ups.
    assert_eq!(r.call("sequences_list", json!({})).await, json!([]));

    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": start + 86_400 + 60 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 1);
    assert!(r.outgoing.received()[0].data.contains("Hi Ana, any news?"));

    // The second waits for its date, not a number of days after the first.
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": start + 4 * 86_400 }),
    )
    .await;
    assert_eq!(r.outgoing.received().len(), 1);
    r.call(
        "due_tick",
        json!({ "quiet_now": true, "at": start + 5 * 86_400 + 60 }),
    )
    .await;
    let out = r.outgoing.received();
    assert_eq!(out.len(), 2);
    // With no AI set up, a polite nudge goes instead.
    assert!(out[1].data.contains("Just following up on my email below"));
    assert_eq!(run_state(&r, &run).await["state"], "finished");
}
