//! Apple Intelligence: the Mac's own AI answers when nothing else is connected, and steps
//! aside for a plan or key, or when switched off.

use fakeai::FakeAi;
use mailcore::on_device::{OnDeviceAi, OnDeviceError, OnDeviceState};
use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct FakeApple {
    state: Mutex<OnDeviceState>,
    asked: AtomicUsize,
    longest: AtomicUsize,
    prompts: Mutex<Vec<String>>,
    failed_once: std::sync::atomic::AtomicBool,
    systems: Mutex<Vec<String>>,
}

impl OnDeviceAi for FakeApple {
    fn state(&self) -> OnDeviceState {
        *self.state.lock().unwrap()
    }

    fn ask(&self, system: &str, prompt: &str, schema: &str) -> Result<String, OnDeviceError> {
        self.systems.lock().unwrap().push(system.to_string());
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        // Apple's model sometimes just fails; once is enough to test a second try.
        if prompt.contains("flaky") && !self.failed_once.swap(true, Ordering::SeqCst) {
            return Err(OnDeviceError::Failed);
        }
        self.longest
            .fetch_max(prompt.chars().count(), Ordering::SeqCst);
        self.prompts.lock().unwrap().push(prompt.to_string());
        // Reading the email in short, for a reply.
        if schema.contains("they_ask") {
            return Ok(
                json!({ "they_said": "Ana asks for a price.", "they_ask": ["a price"] })
                    .to_string(),
            );
        }
        // Reading an Ask question into what to look for.
        if schema.contains("about_itself") {
            let (topics, answer) = if prompt.contains("how many") {
                (json!([{ "words": ["account"] }]), "count")
            } else if prompt.contains("emails about account") {
                (json!([{ "words": ["account"] }]), "list")
            } else if prompt.contains("log in") {
                (json!([{ "words": ["log", "login"] }]), "answer")
            } else if prompt.to_lowercase().contains("happened") {
                (json!([]), "summary")
            } else {
                (json!([{ "words": ["invoice"] }]), "list")
            };
            return Ok(json!({
                "names": [], "topics": topics, "after": "", "before": "", "answer": answer,
                "waiting_on_me": false, "upcoming": false, "latest": false, "about_itself": false
            })
            .to_string());
        }
        // Reading one email for an Ask question: a line about it.
        if schema.contains("\"note\"") {
            let note = if system.contains("catching up") {
                "A line about this email."
            } else if prompt.contains("can't log in") {
                "Ana can't log in since Monday."
            } else {
                ""
            };
            return Ok(json!({ "note": note }).to_string());
        }
        // Saying how many the app counted: right, or with a number of its own.
        if schema.contains("not_found") && prompt.contains("how many") {
            let answer = if prompt.contains("wrong") {
                "Three people wrote."
            } else {
                "2 people wrote about their accounts."
            };
            return Ok(json!({ "answer": answer, "not_found": false }).to_string());
        }
        // Answering from those lines.
        if schema.contains("not_found") {
            return Ok(json!({
                "answer": "Ana can't log in.", "points": [], "key_ids": [], "suggestions": [],
                // Quoted from its notes, which put the email in other words.
                "facts": [{ "text": "Ana can't log in", "source": 1 }], "not_found": false
            })
            .to_string());
        }
        // Writing it: the first try forgets the price, the second has it.
        if schema.contains("greeting") {
            let answer = if n <= 1 {
                "Thanks for asking."
            } else {
                "It's $1,800 \u{1F600} all in."
            };
            return Ok(
                json!({ "greeting": "Hi Ana,", "answer": answer, "closing": "Thanks!" })
                    .to_string(),
            );
        }
        Ok(json!({
            "words": ["invoice"], "similar": [], "from": [], "to": [],
            "after": null, "before": null, "has_attachment": false, "unread": false,
            "starred": false, "folder": null, "summary": "Invoices"
        })
        .to_string())
    }
}

async fn rig(state: OnDeviceState) -> (Engine, Arc<FakeApple>, FakeAi) {
    let apple = Arc::new(FakeApple {
        state: Mutex::new(state),
        asked: AtomicUsize::new(0),
        longest: AtomicUsize::new(0),
        prompts: Mutex::default(),
        failed_once: Default::default(),
        systems: Mutex::default(),
    });
    let ai = FakeAi::start().await;
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_ai_url(&ai.url)
    .with_on_device(apple.clone());
    engine
        .call("set_setting", json!({ "key": "ai_search", "value": "on" }))
        .await
        .unwrap();
    (engine, apple, ai)
}

/// A question the rules can't read, so AI is asked; whether it was.
async fn understand(engine: &Engine, text: &str) -> Value {
    let (_, _, used_ai) = engine
        .understand(text, chrono::Utc::now(), true)
        .await
        .unwrap();
    json!({ "used_ai": used_ai })
}

#[tokio::test]
async fn apple_intelligence_answers_when_nothing_else_is_connected() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    let status = engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["using_on_device"], true);
    assert_eq!(status["has_key"], true);
    assert_eq!(status["on_device"], "ready");
    assert_eq!(understand(&engine, "my invoices").await["used_ai"], true);
    assert_eq!(apple.asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn what_goes_to_it_is_cut_to_fit() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    understand(&engine, &"invoice ".repeat(5_000)).await;
    assert!(apple.longest.load(Ordering::SeqCst) <= 9_000);
}

#[tokio::test]
async fn a_key_comes_first() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    engine
        .call(
            "ai_set_key",
            json!({ "key": "test-ai-key", "service": "anthropic" }),
        )
        .await
        .unwrap();
    let status = engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["using_on_device"], false);
    understand(&engine, "my invoices").await;
    assert_eq!(apple.asked.load(Ordering::SeqCst), 0);

    // Picked in Ask AI, Apple Intelligence answers Ask AI only; the rest keeps the key.
    engine
        .call(
            "set_setting",
            json!({ "key": "ask_ai_apple", "value": "on" }),
        )
        .await
        .unwrap();
    let status = engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["ask_on_device"], true);
    assert_eq!(status["using_on_device"], false);
    understand(&engine, "my invoices").await;
    assert_eq!(apple.asked.load(Ordering::SeqCst), 0);
    engine
        .call(
            "set_setting",
            json!({ "key": "ask_ai_apple", "value": "off" }),
        )
        .await
        .unwrap();
    let status = engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["ask_on_device"], false);
}

#[tokio::test]
async fn switched_off_or_not_ready_it_stays_quiet() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    engine
        .call(
            "set_setting",
            json!({ "key": "ai_on_device", "value": "off" }),
        )
        .await
        .unwrap();
    assert_eq!(understand(&engine, "my invoices").await["used_ai"], false);

    engine
        .call(
            "set_setting",
            json!({ "key": "ai_on_device", "value": "on" }),
        )
        .await
        .unwrap();
    *apple.state.lock().unwrap() = OnDeviceState::Downloading;
    let status = engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["has_key"], false);
    assert_eq!(status["on_device"], "downloading");
    assert_eq!(understand(&engine, "my invoices").await["used_ai"], false);
    assert_eq!(apple.asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_reply_reads_the_email_first_and_keeps_the_notes_numbers() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    let server = fakeimap::FakeServer::start("me@test.test", "pw").await;
    server.add_folder("INBOX", &[], 1);
    server.add_message(
        "INBOX",
        "From: Ana <ana@person.test>\nTo: me@test.test\nSubject: Price?\nMessage-ID: <p1@t>\n\
         Date: Sun, 01 Nov 2026 10:00:00 +0000\nContent-Type: text/plain\n\nHow much is it?\n\n\
         On Mon, Me wrote:\n> an old quoted line\n",
        false,
    );
    let account = engine
        .call(
            "add_account",
            json!({ "email": "me@test.test", "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": "pw",
                    "smtp_host": "127.0.0.1", "smtp_port": 1, "smtp_security": "plain" }),
        )
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    engine
        .call("sync_account", json!({ "account_id": account }))
        .await
        .unwrap();
    let folders = engine
        .call("list_folders", json!({ "account_id": account }))
        .await
        .unwrap();
    let inbox = folders
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["role"] == "inbox")
        .unwrap()["id"]
        .clone();
    let threads = engine
        .call("list_threads", json!({ "folder_id": inbox }))
        .await
        .unwrap();
    let thread = threads[0]["thread_id"].clone();
    let got = engine
        .call(
            "get_thread",
            json!({ "account_id": account, "thread_id": thread }),
        )
        .await
        .unwrap();
    let msgs = if got.is_array() {
        got
    } else {
        got["messages"].clone()
    };
    let id = msgs[0]["id"].clone();
    let mut prefs = engine.call("list_accounts", json!({})).await.unwrap()[0]["prefs"].clone();
    prefs["about"] = json!("I sell handmade tables.");
    engine
        .call(
            "update_account_prefs",
            json!({ "account_id": account, "prefs": prefs }),
        )
        .await
        .unwrap();
    let out = engine
        .call(
            "ai_write_reply",
            json!({ "message_id": id, "instruction": "$1,800 all in" }),
        )
        .await
        .unwrap();
    // Read in short, a first try without the price, then one with it; no emoji left.
    assert_eq!(apple.asked.load(Ordering::SeqCst), 3);
    assert_eq!(out["reply"], "Hi Ana,\n\nIt's $1,800 all in.\n\nThanks!");
    let prompts = apple.prompts.lock().unwrap();
    assert!(!prompts.iter().any(|p| p.contains("old quoted line")));
    assert!(prompts[1].contains("- a price") && prompts[1].contains("$1,800 all in"));
    assert!(prompts[1].contains("I sell handmade tables."));
}

#[tokio::test]
async fn ask_reads_the_question_and_each_email_on_its_own() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    engine
        .call("set_setting", json!({ "key": "ai_chat", "value": "on" }))
        .await
        .unwrap();
    let server = fakeimap::FakeServer::start("me@test.test", "pw").await;
    server.add_folder("INBOX", &[], 1);
    for (id, from, subject, body) in [
        (
            "l1@t",
            "Ana <ana@person.test>",
            "Login",
            "I can't log in to my account since Monday.",
        ),
        (
            "l2@t",
            "Bo <bo@person.test>",
            "Account",
            "Thanks, my account works fine now.",
        ),
    ] {
        server.add_message(
            "INBOX",
            &format!(
                "From: {from}\nTo: me@test.test\nSubject: {subject}\nMessage-ID: <{id}>\n\
                 Date: Sun, 01 Nov 2026 10:00:00 +0000\nContent-Type: text/plain\n\n{body}\n"
            ),
            false,
        );
    }
    let account = engine
        .call(
            "add_account",
            json!({ "email": "me@test.test", "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": "pw",
                    "smtp_host": "127.0.0.1", "smtp_port": 1, "smtp_security": "plain" }),
        )
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    engine
        .call("sync_account", json!({ "account_id": account }))
        .await
        .unwrap();

    // Apple Intelligence reads the question too: what to look for, and to answer it.
    let looked = engine
        .call(
            "ai_ask_sources",
            json!({ "question": "who can't log in to their account?", "quick": true }),
        )
        .await
        .unwrap();
    assert_eq!(looked["kind"], "fact", "{looked}");
    assert!(apple
        .prompts
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.contains("Plan the search for: who can't log in")));

    // A question it answers: each email read on its own, then the answer from those lines.
    let both = engine
        .call(
            "ai_ask_sources",
            json!({ "question": "emails about account", "quick": true }),
        )
        .await
        .unwrap();
    let ids: Vec<i64> = both["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["id"].as_i64())
        .collect();
    assert_eq!(ids.len(), 2, "{both}");
    let before = apple.prompts.lock().unwrap().len();
    let reply = engine
        .call(
            "ai_chat",
            json!({ "question": "what's wrong with the accounts?", "ids": ids }),
        )
        .await
        .unwrap();
    assert_eq!(reply["answer"], "Ana can't log in.", "{reply}");
    let prompts = apple.prompts.lock().unwrap()[before..].to_vec();
    assert_eq!(
        prompts.len(),
        3,
        "one per email, then the answer: {prompts:?}"
    );
    assert!(prompts[..2]
        .iter()
        .all(|p| p.matches("<email ").count() == 1));
    assert!(prompts[2].contains("Ana can't log in since Monday."));
    assert!(
        !prompts[2].contains("works fine now"),
        "an email with nothing on it is left out"
    );
    drop(prompts);

    // A summary of the day reads every email for a line, none left out as unhelpful.
    let before = apple.systems.lock().unwrap().len();
    engine
        .call(
            "ai_chat",
            json!({ "question": "Summarise what happened in my inbox today", "ids": ids, "plan": { "kind": "catch_up" } }),
        )
        .await
        .unwrap();
    let systems = apple.systems.lock().unwrap()[before..].to_vec();
    assert!(systems[0].contains("who wrote"), "{}", systems[0]);
    assert!(!systems[0].contains("leave `note` empty"));
}

#[tokio::test]
async fn a_summary_lists_a_line_for_every_email() {
    let (engine, _apple, _ai) = rig(OnDeviceState::Ready).await;
    let (ids, _server) = two_emails(&engine).await;
    let reply = engine
        .call(
            "ai_chat",
            json!({ "question": "Summarise what happened in my inbox today", "ids": ids, "plan": { "kind": "catch_up" } }),
        )
        .await
        .unwrap();
    // The small model's answer named one; the lines come from every email's note.
    assert_eq!(reply["points"].as_array().unwrap().len(), 2, "{reply}");
}

#[tokio::test]
async fn how_many_is_counted_by_the_app() {
    let (engine, apple, _ai) = rig(OnDeviceState::Ready).await;
    let (ids, _server) = two_emails(&engine).await;
    let looked = engine
        .call(
            "ai_ask_sources",
            json!({ "question": "how many emails about account?", "quick": true }),
        )
        .await
        .unwrap();
    assert_eq!(looked["kind"], "count", "{looked}");
    assert_eq!(looked["total"], 2);
    // A follow-up starts from the plan before it, which a small model changes far more
    // reliably than it works out the whole question again.
    assert_eq!(looked["plan"]["answer"], "count", "{looked}");
    let before = apple.prompts.lock().unwrap().len();
    engine
        .call(
            "ai_ask_sources",
            json!({ "question": "and in September?", "quick": true,
                    "previous": ["how many emails about account?"],
                    "previous_plan": looked["plan"] }),
        )
        .await
        .unwrap();
    let asked = apple.prompts.lock().unwrap()[before].clone();
    assert!(asked.contains(r#""answer":"count""#), "{asked}");
    assert!(
        asked.contains("Plan the search for: and in September?"),
        "{asked}"
    );
    // One call says it, with the number the app counted; no email is read for a note.
    let before = apple.prompts.lock().unwrap().len();
    let reply = engine
        .call(
            "ai_chat",
            json!({ "question": "how many emails about account?", "ids": ids,
                    "plan": { "kind": "count", "total": 2 } }),
        )
        .await
        .unwrap();
    let prompts = apple.prompts.lock().unwrap()[before..].to_vec();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("Exactly 2"), "{}", prompts[0]);
    assert_eq!(
        reply["answer"], "2 people wrote about their accounts.",
        "{reply}"
    );
    // A sentence with some other number isn't shown; the count is.
    let reply = engine
        .call(
            "ai_chat",
            json!({ "question": "how many wrong emails about account?", "ids": ids,
                    "plan": { "kind": "count", "total": 2 } }),
        )
        .await
        .unwrap();
    assert_eq!(reply["answer"], "", "{reply}");
    assert_eq!(reply["count"], 2);
}

#[tokio::test]
async fn a_failed_try_is_tried_again() {
    let (engine, _apple, _ai) = rig(OnDeviceState::Ready).await;
    // Once failed, then answered: the person sees the answer, not an error.
    let (q, _, used_ai) = engine
        .understand("flaky question about my invoices", chrono::Utc::now(), true)
        .await
        .unwrap();
    assert!(used_ai);
    assert_eq!(q.words, ["invoice"]);
}

/// Ana can't log in, Bo is fine: two emails in a new inbox, and their ids.
async fn two_emails(engine: &Engine) -> (Vec<i64>, fakeimap::FakeServer) {
    engine
        .call("set_setting", json!({ "key": "ai_chat", "value": "on" }))
        .await
        .unwrap();
    let server = fakeimap::FakeServer::start("me@test.test", "pw").await;
    server.add_folder("INBOX", &[], 1);
    for (id, from, subject, body) in [
        (
            "l1@t",
            "Ana <ana@person.test>",
            "Login",
            "I can't log in to my account since Monday.",
        ),
        (
            "l2@t",
            "Bo <bo@person.test>",
            "Account",
            "Thanks, my account works fine now.",
        ),
    ] {
        server.add_message(
            "INBOX",
            &format!(
                "From: {from}\nTo: me@test.test\nSubject: {subject}\nMessage-ID: <{id}>\n\
                 Date: Sun, 01 Nov 2026 10:00:00 +0000\nContent-Type: text/plain\n\n{body}\n"
            ),
            false,
        );
    }
    let account = engine
        .call(
            "add_account",
            json!({ "email": "me@test.test", "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": "pw",
                    "smtp_host": "127.0.0.1", "smtp_port": 1, "smtp_security": "plain" }),
        )
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    engine
        .call("sync_account", json!({ "account_id": account }))
        .await
        .unwrap();
    let both = engine
        .call(
            "ai_ask_sources",
            json!({ "question": "emails about account", "quick": true }),
        )
        .await
        .unwrap();
    let ids = both["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["id"].as_i64())
        .collect();
    (ids, server)
}
