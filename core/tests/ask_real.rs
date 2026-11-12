//! Only runs by hand, pointed at a copy of real mail, to measure how well Ask AI finds things.
//! The questions live in a file on the Mac it runs on and are never committed:
//!
//!   INBOX_REAL_COPY=/path/to/copy-of-mail.db ASK_QUESTIONS=/path/to/questions.json \
//!     cargo test -p mailcore --test ask_real -- --ignored --nocapture
//!
//! questions.json is a list of `{ "question": "...", "expect_subject": "..." }`: a few words of
//! the subject of the email that answers it. Run it on a copy: it adds the app's newest tables.
//! It prints counts, and with INBOX_REAL_SHOW also each question and what was found.

use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

#[tokio::test]
#[ignore]
async fn measure_ask_on_a_copy_of_real_mail() {
    let (Ok(copy), Ok(questions)) = (
        std::env::var("INBOX_REAL_COPY"),
        std::env::var("ASK_QUESTIONS"),
    ) else {
        return;
    };
    let show = std::env::var("INBOX_REAL_SHOW").is_ok();
    let asked: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(questions).unwrap()).unwrap();
    let engine = Engine::new(
        Store::open(std::path::Path::new(&copy)).unwrap(),
        Arc::new(MemoryStore::default()),
    );
    let (mut first, mut top5, mut lists, mut facts) = (0, 0, 0, 0);
    let mut slowest = 0u128;
    for q in &asked {
        let question = q["question"].as_str().unwrap_or("");
        let want = q["expect_subject"].as_str().unwrap_or("").to_lowercase();
        let started = Instant::now();
        let found = engine
            .call(
                "ai_ask_sources",
                json!({ "question": question, "quick": true }),
            )
            .await
            .unwrap_or_else(|e| json!({ "sources": [], "kind": format!("error: {e}") }));
        let took = started.elapsed().as_millis();
        slowest = slowest.max(took);
        let subjects: Vec<String> = found["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| s["subject"].as_str().unwrap_or("").to_lowercase())
            .collect();
        let at = subjects
            .iter()
            .position(|s| !want.is_empty() && s.contains(&want));
        first += usize::from(at == Some(0));
        top5 += usize::from(at.is_some_and(|i| i < 5));
        lists += usize::from(found["kind"] == "list");
        facts += usize::from(!found["fact"].is_null());
        if show {
            println!(
                "{question}\n  kind {} · {took} ms · wanted '{want}' at {at:?}\n  found {:?}",
                found["kind"],
                &subjects[..subjects.len().min(5)]
            );
        }
    }
    println!(
        "{} questions: right email first {first}, in the top 5 {top5}; lists {lists}, facts {facts}; slowest {slowest} ms",
        asked.len()
    );
}
