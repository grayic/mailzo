//! The AI that comes with Mailzo Cloud, as the app reaches it: the paths, what's sent, and
//! the account server's refusals reaching the person as they are.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use mailcore::paid::*;
use mailcore::{Engine, MemoryStore, SecretStore, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Every request the stand-in got: method, path, the session it carried, and its body.
type Seen = Arc<Mutex<Vec<(String, String, String, Value)>>>;

async fn note(seen: Seen, req: Request) -> Value {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    seen.lock()
        .unwrap()
        .push((method, path, auth, body.clone()));
    body
}

async fn stand_in() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let (a, b, c, d, e) = (
        seen.clone(),
        seen.clone(),
        seen.clone(),
        seen.clone(),
        seen.clone(),
    );
    let router = Router::new()
        .route(
            "/paid/status",
            get(move |req: Request| async move {
                note(a, req).await;
                Json(json!({ "active": true }))
            }),
        )
        .route(
            "/paid/brief",
            post(move |req: Request| async move {
                note(b, req).await;
                Json(json!({ "text": "Ana wants the contract." }))
            }),
        )
        .route(
            "/paid/ask",
            post(move |req: Request| async move {
                let body = note(c, req).await;
                let id = body["sources"][0]["id"].clone();
                Json(json!({ "headline": "At noon.", "items": [{ "id": id, "title": "Flight", "value": "12:00" }] }))
            }),
        )
        .route(
            "/paid/sort",
            post(move |req: Request| async move {
                note(d, req).await;
                Json(json!({ "items": [{ "index": 0, "tab": "general", "label": "receipt",
                    "group": "good_to_know", "line": "A receipt", "due": "" }] }))
            }),
        )
        .route(
            "/paid/spam",
            post(move |req: Request| async move {
                note(e, req).await;
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({ "code": "ai_resting", "message": "Cloud AI is resting. Try again in a minute." })),
                )
                    .into_response()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (url, seen)
}

fn with_token(token: &str) -> TokenSource {
    let token = token.to_string();
    Arc::new(move || Some(token.clone()))
}

#[tokio::test]
async fn each_one_goes_to_its_own_path_with_the_session_and_the_material() {
    let (url, seen) = stand_in().await;
    let cloud = CloudPaidAi::new(&url, with_token("tok-1")).unwrap();

    assert!(cloud.status().await.unwrap().active);
    let brief = cloud
        .brief(BriefRequest {
            language: "Deutsch".into(),
            items: vec![BriefItem {
                from: "Ana".into(),
                subject: "Contract".into(),
                line: "Wants it signed".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(brief.text, "Ana wants the contract.");
    let answer = cloud
        .ask(AskRequest {
            language: "English".into(),
            question: "when is my flight".into(),
            sources: vec![AskSource {
                id: 42,
                from: "Air <air@x.test>".into(),
                subject: "Booking".into(),
                date: 1_700_000_000,
                text: "Leaves at noon".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(answer.headline, "At noon.");
    assert_eq!(
        answer.items,
        vec![AskItem {
            id: 42,
            title: "Flight".into(),
            value: "12:00".into()
        }]
    );
    let sorted = cloud
        .sort(SortRequest {
            language: "English".into(),
            items: vec![SortItem {
                index: 0,
                from: "shop@x.test".into(),
                subject: "Receipt".into(),
                text: String::new(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(sorted.items[0].label, "receipt");
    assert_eq!(sorted.items[0].group, "good_to_know");

    // The account server's own words come through, as an AI problem.
    let refused = cloud
        .spam(SpamRequest {
            language: "English".into(),
            question: SpamQuestion {
                from: "odd@x.test".into(),
                subject: "URGENT".into(),
                text: "Act now".into(),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "ai_resting");
    assert_eq!(
        refused.to_string(),
        "Cloud AI is resting. Try again in a minute."
    );

    let seen = seen.lock().unwrap().clone();
    let paths: Vec<(String, String)> = seen
        .iter()
        .map(|(m, p, _, _)| (m.clone(), p.clone()))
        .collect();
    assert_eq!(
        paths,
        [
            ("GET", "/paid/status"),
            ("POST", "/paid/brief"),
            ("POST", "/paid/ask"),
            ("POST", "/paid/sort"),
            ("POST", "/paid/spam"),
        ]
        .map(|(m, p)| (m.to_string(), p.to_string()))
    );
    assert!(seen.iter().all(|(_, _, auth, _)| auth == "Bearer tok-1"));
    assert_eq!(
        seen[1].3,
        json!({ "language": "Deutsch", "items": [{ "from": "Ana", "subject": "Contract", "line": "Wants it signed" }] })
    );
    assert_eq!(
        seen[2].3,
        json!({ "language": "English", "question": "when is my flight", "sources": [{
            "id": 42, "from": "Air <air@x.test>", "subject": "Booking", "date": 1_700_000_000, "text": "Leaves at noon",
        }] })
    );
    assert_eq!(
        seen[3].3,
        json!({ "language": "English", "items": [{ "index": 0, "from": "shop@x.test", "subject": "Receipt", "text": "" }] })
    );
    assert_eq!(
        seen[4].3,
        json!({ "language": "English", "from": "odd@x.test", "subject": "URGENT", "text": "Act now" })
    );
}

#[tokio::test]
async fn whether_cloud_is_active_is_kept_for_a_while_and_signed_out_asks_nothing() {
    let (url, seen) = stand_in().await;
    let cloud = CloudPaidAi::new(&url, with_token("tok-1")).unwrap();
    for _ in 0..3 {
        assert!(cloud.status().await.unwrap().active);
    }
    assert_eq!(seen.lock().unwrap().len(), 1, "asked once");

    let signed_out = CloudPaidAi::new(&url, Arc::new(|| None)).unwrap();
    assert!(!signed_out.status().await.unwrap().active);
    let refused = signed_out.brief(BriefRequest::default()).await.unwrap_err();
    assert_eq!(refused.code(), "paid_only");
    assert_eq!(seen.lock().unwrap().len(), 1, "nothing was sent signed out");
}

#[tokio::test]
async fn an_unreachable_server_is_said_plainly() {
    let cloud = CloudPaidAi::new("http://127.0.0.1:9", with_token("tok")).unwrap();
    let e = cloud.status().await.unwrap_err();
    assert_eq!(e.code(), "offline");
}

#[tokio::test]
async fn the_engine_uses_cloud_by_itself_once_signed_in() {
    let (url, seen) = stand_in().await;
    let secrets = Arc::new(MemoryStore::default());
    let engine = Engine::new(Store::open_in_memory().unwrap(), secrets.clone())
        .with_cloud(&url, "Test Mac")
        .unwrap();
    assert_eq!(
        engine.call("paid_status", json!({})).await.unwrap()["paid"],
        false,
        "signed out"
    );
    assert!(seen.lock().unwrap().is_empty());
    let refused = engine.call("ai_brief", json!({})).await;
    // With nothing waiting there's nothing to brief on, which needs no Cloud.
    assert_eq!(refused.unwrap()["count"], 0);

    secrets.set("session", "tok-9").unwrap();
    assert_eq!(
        engine.call("paid_status", json!({})).await.unwrap()["paid"],
        true
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, "/paid/status");
    assert_eq!(seen[0].2, "Bearer tok-9");
}

#[tokio::test]
async fn without_cloud_the_four_say_so() {
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    );
    assert_eq!(
        engine.call("paid_status", json!({})).await.unwrap()["paid"],
        false
    );
    let e = engine
        .call("ai_ask", json!({ "question": "anything", "ids": [1] }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "paid_only");
    assert_eq!(e.to_string(), PAID_ONLY);
}
