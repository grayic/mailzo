//! A stand-in for the AI that comes with Mailzo Cloud: canned answers, and a note of
//! everything it was sent. Shared by the tests that use it.
#![allow(dead_code)]

use mailcore::paid::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct FakePaid {
    pub active: AtomicBool,
    sent: Mutex<Vec<(String, Value)>>,
    brief: Mutex<BriefAnswer>,
    ask: Mutex<AskAnswer>,
    sort: Mutex<SortAnswer>,
    spam: Mutex<SpamOpinion>,
}

impl FakePaid {
    pub fn new() -> Arc<FakePaid> {
        let paid = FakePaid::default();
        paid.active.store(true, Ordering::SeqCst);
        Arc::new(paid)
    }

    pub fn answer_brief(&self, text: &str) {
        *self.brief.lock().unwrap() = BriefAnswer { text: text.into() };
    }

    pub fn answer_ask(&self, answer: Value) {
        *self.ask.lock().unwrap() = serde_json::from_value(answer).unwrap();
    }

    pub fn answer_sort(&self, items: Value) {
        *self.sort.lock().unwrap() = serde_json::from_value(json!({ "items": items })).unwrap();
    }

    pub fn answer_spam(&self, spam: bool, reason: &str) {
        *self.spam.lock().unwrap() = SpamOpinion {
            spam,
            reason: reason.into(),
        };
    }

    /// What was sent for one of brief, ask, sort or spam, oldest first.
    pub fn sent(&self, kind: &str) -> Vec<Value> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// Everything it was ever sent, as one piece of text.
    pub fn everything_sent(&self) -> String {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, v)| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn note(&self, kind: &str, request: &impl serde::Serialize) {
        self.sent
            .lock()
            .unwrap()
            .push((kind.into(), serde_json::to_value(request).unwrap()));
    }
}

impl PaidAi for FakePaid {
    fn status(&self) -> PaidFuture<'_, PaidStatus> {
        let active = self.active.load(Ordering::SeqCst);
        Box::pin(async move { Ok(PaidStatus { active }) })
    }

    fn brief(&self, request: BriefRequest) -> PaidFuture<'_, BriefAnswer> {
        self.note("brief", &request);
        let answer = self.brief.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }

    fn ask(&self, request: AskRequest) -> PaidFuture<'_, AskAnswer> {
        self.note("ask", &request);
        let answer = self.ask.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }

    fn sort(&self, request: SortRequest) -> PaidFuture<'_, SortAnswer> {
        self.note("sort", &request);
        let answer = self.sort.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }

    fn spam(&self, request: SpamRequest) -> PaidFuture<'_, SpamOpinion> {
        self.note("spam", &request);
        let answer = self.spam.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }
}
