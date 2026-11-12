//! Deleting the Mailzo account, against a stand-in for the account server. The device
//! carries on with no account, keeping its own inboxes and mail.

use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::{routing::any, Json, Router};
use mailcore::model::Security;
use mailcore::secrets::account_secret;
use mailcore::{Engine, MemoryStore, SecretStore, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(String, String, String, Value)>>>;

async fn record(
    State(seen): State<Seen>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Json<Value> {
    let session = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = body.map(|Json(b)| b).unwrap_or(Value::Null);
    seen.lock()
        .unwrap()
        .push((method.to_string(), uri.path().to_string(), session, body));
    Json(json!({ "sent": true, "deleted": true }))
}

#[tokio::test]
async fn deleting_the_account_keeps_this_devices_own_inboxes() {
    let seen: Seen = Arc::default();
    let app = Router::new().fallback(any(record)).with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let store = Store::open_in_memory().unwrap();
    store
        .save_identity("me@example.com", 7, "Test Mac", "ready")
        .unwrap();
    let own = store
        .insert_account(
            "me@example.com",
            "Me",
            "imap.example.com",
            993,
            Security::Tls,
            "me",
        )
        .unwrap();
    let mut shared = store
        .insert_account(
            "team@example.com",
            "Team",
            "imap.example.com",
            993,
            Security::Tls,
            "team",
        )
        .unwrap();
    shared.prefs.team_shared = true;
    shared.prefs.team_id = 3;
    store.save_account(&shared).unwrap();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set("session", "tok-1").unwrap();
    secrets.set(&account_secret(own.id), "pw-own").unwrap();
    secrets.set(&account_secret(shared.id), "pw-team").unwrap();
    let engine = Engine::new(store, secrets.clone())
        .with_cloud(&url, "Test Mac")
        .unwrap();

    // The code goes to the account's own address.
    engine
        .call("account_delete_start", json!({}))
        .await
        .unwrap();
    assert!(engine
        .call("account_delete", json!({ "code": "  " }))
        .await
        .is_err());
    engine
        .call("account_delete", json!({ "code": "123 456" }))
        .await
        .unwrap();

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].1, "/auth/start");
    assert_eq!(seen[0].3["email"], "me@example.com");
    let (method, path, session, body) = &seen[1];
    assert_eq!((method.as_str(), path.as_str()), ("DELETE", "/account"));
    assert_eq!(session, "Bearer tok-1");
    assert_eq!(body["code"], "123456");

    // No account any more, but this device's own inbox is still here and still works.
    let status = engine.call("auth_status", json!({})).await.unwrap();
    assert_eq!(status["state"], "signed_out");
    assert_eq!(secrets.get("session").unwrap(), None);
    let left = engine.call("list_accounts", json!({})).await.unwrap();
    let left: Vec<&str> = left
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["email"].as_str().unwrap())
        .collect();
    assert_eq!(left, ["me@example.com"]);
    assert_eq!(
        secrets.get(&account_secret(own.id)).unwrap().as_deref(),
        Some("pw-own")
    );
    // The team's shared inbox went with the team.
    assert_eq!(secrets.get(&account_secret(shared.id)).unwrap(), None);
}
