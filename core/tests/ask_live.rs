//! Only runs by hand, on a copy of real mail, to see how Ask AI does on real questions: what
//! it read, what it found, how long each step took, and what the AI said. Nothing is synced or
//! sent, and every question keeps to the one inbox named.
//!
//!   INBOX_LIVE_COPY=/path/copy-of/mail.db INBOX_LIVE_HELPER=/path/inbox-keyhelper \
//!   INBOX_LIVE_INBOX=someone@example.com INBOX_LIVE_QUESTIONS=/path/questions.txt \
//!   INBOX_LIVE_REPORT=/path/report.jsonl INBOX_LIVE_DATA=/path/scratch \
//!   INBOX_AI_LOG=/path/ai.jsonl \
//!     cargo test -p mailcore --test ask_live -- --ignored --nocapture
//!
//! One question a line. "> question" goes on in the same chat as the line before;
//! "search: words :: question" runs that search, then asks beside it, which like the app's
//! Ask pane is a question of its own in the same inbox.

use mailcore::{Engine, HelperStore, SecretStore, Store};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
#[ignore]
async fn ask_real_questions_on_a_copy_of_real_mail() {
    let (Some(copy), Some(helper), Some(inbox), Some(questions), Some(report), Some(data)) = (
        env("INBOX_LIVE_COPY"),
        env("INBOX_LIVE_HELPER"),
        env("INBOX_LIVE_INBOX"),
        env("INBOX_LIVE_QUESTIONS"),
        env("INBOX_LIVE_REPORT"),
        env("INBOX_LIVE_DATA"),
    ) else {
        return;
    };
    let secrets: Arc<dyn SecretStore> = Arc::new(HelperStore::new(&helper, "com.grayic.inbox"));
    let opened = Store::open_protected(std::path::Path::new(&copy), secrets.as_ref()).unwrap();
    let engine =
        Engine::new(opened.store, secrets).with_folders(data.clone().into(), data.clone().into());
    let accounts = engine.call("list_accounts", json!({})).await.unwrap();
    let account = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["email"].as_str() == Some(inbox.as_str()))
        .expect("that inbox isn't here");
    let scope = account["id"].as_i64().unwrap();
    let mut out = std::fs::File::create(&report).unwrap();
    let mut history: Vec<Value> = Vec::new();
    let mut previous: Vec<String> = Vec::new();

    for line in std::fs::read_to_string(&questions).unwrap().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (follow_up, rest) = match line.strip_prefix('>') {
            Some(rest) => (true, rest.trim()),
            None => (false, line),
        };
        if !follow_up {
            history.clear();
            previous.clear();
        }
        let (search, question) = match rest.strip_prefix("search:") {
            Some(s) => {
                let (words, q) = s.split_once("::").expect("search: words :: question");
                (Some(words.trim().to_string()), q.trim().to_string())
            }
            None => (None, rest.to_string()),
        };
        let read = mailcore::question::read_question(&question, chrono::Local::now(), true);
        let mut record = json!({
            "question": question,
            "follow_up": follow_up,
            "read": {
                "kind": format!("{:?}", read.kind), "sure": read.sure, "words": read.words,
                "names": read.names, "after": read.after, "before": read.before,
            },
        });
        // Beside search results: the search first, its results kept to this inbox.
        if let Some(words) = &search {
            let t = Instant::now();
            let found = engine
                .call("search_mail", json!({ "text": words }))
                .await
                .unwrap();
            let mine: Vec<&Value> = found["threads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| t["account_id"].as_i64() == Some(scope))
                .collect();
            record["search"] = json!({
                "words": words, "ms": t.elapsed().as_millis() as u64, "results_here": mine.len(),
                "top": mine.iter().take(6).map(|t| format!("{} — {} ({})", t["participants"][0].as_str().unwrap_or(""), t["subject"].as_str().unwrap_or(""), t["last_date"])).collect::<Vec<_>>(),
            });
        }
        let t = Instant::now();
        let looked = engine
            .call(
                "ai_ask_sources",
                json!({
                    "question": question, "quick": true, "scope": scope,
                    "previous": previous,
                }),
            )
            .await;
        let find_ms = t.elapsed().as_millis() as u64;
        let looked = match looked {
            Ok(v) => v,
            Err(e) => {
                record["find"] = json!({ "ms": find_ms, "error": e.to_string() });
                writeln!(out, "{record}").unwrap();
                continue;
            }
        };
        let sources = looked["sources"].as_array().cloned().unwrap_or_default();
        record["find"] = json!({
            "ms": find_ms,
            "kind": looked["kind"], "total": looked["total"], "found": sources.len(),
            "related": sources.iter().filter(|s| s["related"] == true).count(),
            "top": sources.iter().take(6).map(|s| format!("{} — {}", s["from"].as_str().unwrap_or(""), s["subject"].as_str().unwrap_or(""))).collect::<Vec<_>>(),
            "fact": looked["fact"],
        });
        let kind = looked["kind"].as_str().unwrap_or("");
        let needs_ai = !(kind == "list" || (kind == "fact" && !looked["fact"].is_null()));
        if needs_ai && !sources.is_empty() {
            let ids: Vec<Value> = sources.iter().map(|s| s["id"].clone()).collect();
            let related: Vec<Value> = sources
                .iter()
                .filter(|s| s["related"] == true)
                .map(|s| s["id"].clone())
                .collect();
            let t = Instant::now();
            let reply = engine
                .call(
                    "ai_chat",
                    json!({ "question": question, "history": history, "ids": ids, "related": related, "language": "English" }),
                )
                .await;
            let chat_ms = t.elapsed().as_millis() as u64;
            match reply {
                Ok(r) => {
                    record["answer"] = json!({
                        "ms": chat_ms, "text": r["answer"], "points": r["points"].as_array().map_or(0, Vec::len),
                        "not_found": r["not_found"], "removed": r["removed"], "partial": r["partial"],
                        "cited": r["sources"].as_array().map_or(0, Vec::len),
                    });
                    history.push(json!({ "role": "user", "text": question }));
                    history.push(json!({ "role": "assistant", "text": r["answer"] }));
                }
                Err(e) => record["answer"] = json!({ "ms": chat_ms, "error": e.to_string() }),
            }
        } else {
            history.push(json!({ "role": "user", "text": question }));
        }
        previous.push(question.clone());
        writeln!(out, "{record}").unwrap();
        out.flush().unwrap();
    }
}
