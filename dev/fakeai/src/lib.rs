//! A stand-in for the AI company's messages service, for tests and the demo. It
//! checks the key, keeps what it was sent, and gives the reply it was told to.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub const GOOD_KEY: &str = "test-ai-key";

#[derive(Default)]
struct Inner {
    /// Text to look for in the question, and the filters to answer with.
    answers: Mutex<Vec<(String, Value)>>,
    requests: Mutex<Vec<Value>>,
    /// Makes the next replies fail with this status.
    fail_with: Mutex<Option<(u16, String)>>,
    /// What `/models` lists, for services that say what they have.
    models: Mutex<Vec<String>>,
    /// Asking for these models fails as if they'd been retired.
    retired: Mutex<Vec<String>>,
    /// Refuses to be asked for JSON, like a few real models do.
    no_json_mode: Mutex<bool>,
    /// Wraps its answers in a code block, like some real models do.
    fenced: Mutex<bool>,
    /// Tokens each reply says it used: in, out, and instructions read back from the cache.
    usage: Mutex<(i64, i64, i64)>,
}

#[derive(Clone)]
pub struct FakeAi {
    inner: Arc<Inner>,
    pub url: String,
}

/// A full set of filters with nothing chosen, for answers to build on.
pub fn blank_answer() -> Value {
    json!({
        "words": [], "similar": [], "from": [], "to": [], "after": null, "before": null,
        "has_attachment": false, "unread": false, "starred": false, "folder": null, "summary": "",
    })
}

async fn messages(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    inner.requests.lock().unwrap().push(body.clone());
    if let Some((status, message)) = inner.fail_with.lock().unwrap().clone() {
        return (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({ "type": "error", "error": { "message": message } })),
        );
    }
    if headers.get("x-api-key").and_then(|k| k.to_str().ok()) != Some(GOOD_KEY) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "type": "error", "error": { "message": "invalid x-api-key" } })),
        );
    }
    let question = body["messages"][0]["content"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let answer = inner
        .answers
        .lock()
        .unwrap()
        .iter()
        .find(|(needle, _)| question.contains(needle.as_str()))
        .map(|(_, answer)| answer.clone())
        .unwrap_or_else(blank_answer);
    let (input, output, cache_read) = *inner.usage.lock().unwrap();
    (
        StatusCode::OK,
        Json(json!({
            "stop_reason": "end_turn",
            "content": [{ "type": "text", "text": answer.to_string() }],
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": cache_read,
                "cache_creation_input_tokens": 0,
            },
        })),
    )
}

fn has_good_bearer(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|k| k.to_str().ok())
        == Some(&format!("Bearer {GOOD_KEY}"))
}

fn unauthorized() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": { "message": "Incorrect API key provided" } })),
    )
}

async fn models(State(inner): State<Arc<Inner>>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if !has_good_bearer(&headers) {
        return unauthorized();
    }
    let data: Vec<Value> = inner
        .models
        .lock()
        .unwrap()
        .iter()
        .map(|id| json!({ "id": id }))
        .collect();
    (StatusCode::OK, Json(json!({ "data": data })))
}

/// The shape most services other than Anthropic's use.
async fn chat(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    inner.requests.lock().unwrap().push(body.clone());
    if let Some((status, message)) = inner.fail_with.lock().unwrap().clone() {
        return (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({ "error": { "message": message } })),
        );
    }
    if !has_good_bearer(&headers) {
        return unauthorized();
    }
    let model = body["model"].as_str().unwrap_or("");
    if inner.retired.lock().unwrap().iter().any(|m| m == model) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": { "message": format!("The model {model} does not exist") } })),
        );
    }
    if *inner.no_json_mode.lock().unwrap() && !body["response_format"].is_null() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": { "message": "response_format is not supported by this model" } }),
            ),
        );
    }
    let question = body["messages"][1]["content"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let answer = inner
        .answers
        .lock()
        .unwrap()
        .iter()
        .find(|(needle, _)| question.contains(needle.as_str()))
        .map(|(_, answer)| answer.clone())
        .unwrap_or_else(blank_answer)
        .to_string();
    let text = if *inner.fenced.lock().unwrap() {
        format!("```json\n{answer}\n```")
    } else {
        answer
    };
    (
        StatusCode::OK,
        Json(json!({
            "choices": [{ "finish_reason": "stop", "message": { "content": text } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5 },
        })),
    )
}

impl FakeAi {
    pub async fn start() -> FakeAi {
        FakeAi::start_on("127.0.0.1:0").await
    }

    pub async fn start_on(addr: &str) -> FakeAi {
        let inner = Arc::new(Inner::default());
        let router = Router::new()
            .route("/v1/messages", post(messages))
            .route("/chat/completions", post(chat))
            .route("/models", get(models))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        FakeAi { inner, url }
    }

    pub fn answer(&self, needle: &str, answer: Value) {
        self.inner
            .answers
            .lock()
            .unwrap()
            .push((needle.to_lowercase(), answer));
    }

    /// Like `answer`, but checked before every answer given so far.
    pub fn answer_first(&self, needle: &str, answer: Value) {
        self.inner
            .answers
            .lock()
            .unwrap()
            .insert(0, (needle.to_lowercase(), answer));
    }

    pub fn fail_with(&self, failure: Option<(u16, &str)>) {
        *self.inner.fail_with.lock().unwrap() = failure.map(|(s, m)| (s, m.to_string()));
    }

    pub fn set_models(&self, models: &[&str]) {
        *self.inner.models.lock().unwrap() = models.iter().map(|m| m.to_string()).collect();
    }

    pub fn retire(&self, model: &str) {
        self.inner.retired.lock().unwrap().push(model.to_string());
    }

    pub fn refuse_json_mode(&self, refuse: bool) {
        *self.inner.no_json_mode.lock().unwrap() = refuse;
    }

    /// What each reply says it used, so costs can be checked.
    pub fn set_usage(&self, input: i64, output: i64, cache_read: i64) {
        *self.inner.usage.lock().unwrap() = (input, output, cache_read);
    }

    pub fn fence_answers(&self, fenced: bool) {
        *self.inner.fenced.lock().unwrap() = fenced;
    }

    pub fn requests(&self) -> Vec<Value> {
        self.inner.requests.lock().unwrap().clone()
    }
}
