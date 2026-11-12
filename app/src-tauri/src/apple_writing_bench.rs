//! A bench for how well Apple Intelligence writes replies, on realistic conversations.
//! Uses this Mac's real model, so only run by hand:
//! `cargo test -p inbox-app --lib writing_bench -- --ignored --nocapture`

use crate::apple_ai::AppleAi;
use fakeimap::FakeServer;
use mailcore::{Engine, MemoryStore, Store};
use serde_json::json;
use std::sync::Arc;

const ME: &str = "farhan@grayic.test";
const PASS: &str = "bench-pass";

struct Mail<'a> {
    id: &'a str,
    from: &'a str,
    to: &'a str,
    subject: &'a str,
    reply_to: Option<&'a str>,
    date: &'a str,
    body: String,
}

fn raw(m: &Mail) -> String {
    let refs = m
        .reply_to
        .map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n"))
        .unwrap_or_default();
    format!(
        "From: {}\nTo: {}\nSubject: {}\nMessage-ID: <{}>\n{refs}Date: {}\nContent-Type: text/plain; charset=utf-8\n\n{}\n",
        m.from, m.to, m.subject, m.id, m.date, m.body
    )
}

/// How the person sounds: a few short replies they sent.
fn sent_examples() -> Vec<String> {
    [
        "Hi Sara,\n\nThanks, that works. I'll send the files by Monday.\n\nBest,\nFarhan",
        "Hey Tom, sounds good. Let's do Tuesday at 11.\n\nFarhan",
        "Hi there,\n\nSorry for the slow reply. Yes, we can add that to the plan. I'll update the invoice today.\n\nThanks,\nFarhan",
    ]
    .iter()
    .enumerate()
    .map(|(i, b)| {
        raw(&Mail {
            id: &format!("sent-{i}@bench"),
            from: &format!("Farhan Rizvi <{ME}>"),
            to: "someone@client.test",
            subject: "Re: plans",
            reply_to: None,
            date: "Mon, 26 Oct 2026 10:00:00 +0000",
            body: b.to_string(),
        })
    })
    .collect()
}

fn quoted(text: &str, who: &str) -> String {
    let lines: String = text.lines().map(|l| format!("> {l}\n")).collect();
    format!("\n\nOn Tue, {who} wrote:\n{lines}")
}

/// (case name, emails oldest first, note from the person)
fn cases() -> Vec<(&'static str, Vec<Mail<'static>>, &'static str)> {
    let long_brief = "Hi Farhan,\n\nWe're a small bakery chain with four shops in Dhaka and we want a new website. \
We need an online menu, a page for each shop with opening hours and a map, a catering order form, and a simple blog. \
Our current site is on WordPress and is very slow. We'd like the new one to be fast on phones because most of our \
customers order from their phones. We also want to collect emails for a monthly newsletter. Our brand colours are \
cream and dark brown, and we have a logo. We have photos of most products but may need help with a few.\n\n"
        .repeat(4);
    let reply1 = "Thanks for the details! Before I quote, how many products are on the menu, and do you need online payment for catering?";
    let answer1 = "About 60 products. No online payment, they can pay on delivery. Also, could you share a rough price and how long it would take? We'd love to launch before Eid.";
    vec![
        (
            "long thread, question at the end (price + timeline)",
            vec![
                Mail { id: "b1@bench", from: "Nadia Karim <nadia@bakery.test>", to: ME, subject: "New website for our bakery", reply_to: None, date: "Mon, 02 Nov 2026 09:00:00 +0000", body: long_brief.clone() },
                Mail { id: "b2@bench", from: "Farhan Rizvi <farhan@grayic.test>", to: "nadia@bakery.test", subject: "Re: New website for our bakery", reply_to: Some("b1@bench"), date: "Mon, 02 Nov 2026 12:00:00 +0000", body: format!("{reply1}{}", quoted(&long_brief, "Nadia")) },
                Mail { id: "b3@bench", from: "Nadia Karim <nadia@bakery.test>", to: ME, subject: "Re: New website for our bakery", reply_to: Some("b2@bench"), date: "Tue, 03 Nov 2026 08:00:00 +0000", body: format!("{answer1}{}", quoted(&format!("{reply1}{}", quoted(&long_brief, "Nadia")), "Farhan")) },
            ],
            "$1,800, about 3 weeks, so before Eid works",
        ),
        (
            "colleague asks for a review, no note",
            vec![Mail { id: "r1@bench", from: "Faridul <faridul@grayic.test>", to: ME, subject: "Can you review the sync change?", reply_to: None, date: "Wed, 04 Nov 2026 10:00:00 +0000", body: "Hey,\n\nI pushed the new sync change to my branch. Could you look at it before Friday? It touches the IMAP idle code so I'd like a second pair of eyes.\n\nThanks!\nFaridul".into() }],
            "",
        ),
        (
            "unhappy customer, note says offer half refund",
            vec![Mail { id: "c1@bench", from: "James Lee <james@shop.test>", to: ME, subject: "Template not what I expected", reply_to: None, date: "Wed, 04 Nov 2026 15:00:00 +0000", body: "Hello,\n\nI bought your Framer template last week but the blog pages don't match the preview at all, and two of the sections are missing on mobile. I'm pretty disappointed. I'd like a full refund please.\n\nJames".into() }],
            "sorry, we fixed the mobile sections in today's update; offer a 50% refund or a free setup call",
        ),
        (
            "meeting move, note yes",
            vec![Mail { id: "m1@bench", from: "Priya Das <priya@agency.test>", to: ME, subject: "Moving our call", reply_to: None, date: "Thu, 05 Nov 2026 07:00:00 +0000", body: "Hi Farhan,\n\nSomething came up on Wednesday. Could we move our call to Thursday at 3pm instead? Same link.\n\nPriya".into() }],
            "yes",
        ),
        (
            "vendor email, question buried at the end, no note",
            vec![Mail { id: "v1@bench", from: "Hosting Support <support@host.test>", to: ME, subject: "Your server upgrade", reply_to: None, date: "Thu, 05 Nov 2026 09:00:00 +0000", body: format!("Dear customer,\n\n{}We'll move your server to the new plan this weekend. To pick the right time, please reply and tell us which window suits you best: Saturday 2-4am or Sunday 3-5am.\n\nRegards,\nSupport Team", "Our new data centre has faster disks, more memory and better network links. Every plan gets free backups and a firewall. Prices stay the same for the first year. ".repeat(12)) }],
            "",
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn writing_bench() {
    let server = FakeServer::start(ME, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    for m in sent_examples() {
        server.add_message("Sent", &m, true);
    }
    let all = cases();
    for (_, mails, _) in &all {
        for m in mails {
            let folder = if m.from.contains(ME) { "Sent" } else { "INBOX" };
            server.add_message(folder, &raw(m), true);
        }
    }
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_on_device(Arc::new(AppleAi));
    let account = engine
        .call(
            "add_account",
            json!({ "email": ME, "imap_host": "127.0.0.1", "imap_port": server.addr.port(),
                    "security": "plain", "password": PASS,
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
    let status = engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(
        status["using_on_device"], true,
        "Apple Intelligence isn't answering here"
    );
    let style = engine
        .call("style_get", json!({ "account_id": account }))
        .await;
    eprintln!("STYLE: {style:?}");
    for (name, mails, note) in &all {
        let last = mails.last().unwrap();
        let id = message_id_of(&engine, account, last.id).await;
        let started = std::time::Instant::now();
        let out = engine
            .call(
                "ai_write_reply",
                json!({ "message_id": id, "instruction": note }),
            )
            .await;
        eprintln!("\nCASE {name} (note: {note:?}) [{:.1?}]", started.elapsed());
        match out {
            Ok(v) => eprintln!("{}", v["reply"].as_str().unwrap_or("?")),
            Err(e) => eprintln!("ERROR: {e}"),
        }
    }
    let style = engine
        .call("style_get", json!({ "account_id": account }))
        .await;
    eprintln!("\nSTYLE AFTER: {style:?}");
}

/// The saved id of an email, found by its Message-ID.
async fn message_id_of(engine: &Engine, account: i64, message_id: &str) -> i64 {
    let folders = engine
        .call("list_folders", json!({ "account_id": account }))
        .await
        .unwrap();
    for f in folders.as_array().unwrap() {
        let threads = engine
            .call("list_threads", json!({ "folder_id": f["id"] }))
            .await
            .unwrap();
        for t in threads.as_array().unwrap() {
            let got = engine
                .call(
                    "get_thread",
                    json!({ "account_id": account, "thread_id": t["thread_id"] }),
                )
                .await
                .unwrap_or(json!([]));
            let msgs = if got.is_array() {
                got.clone()
            } else {
                got["messages"].clone()
            };
            for m in msgs.as_array().into_iter().flatten() {
                if m["rfc_id"].as_str().is_some_and(|x| x.contains(message_id)) {
                    return m["id"].as_i64().unwrap();
                }
            }
        }
    }
    panic!("no email {message_id}");
}
