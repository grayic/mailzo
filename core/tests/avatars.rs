//! Sender pictures, against a stand-in for the picture services.

use mailcore::{Engine, MemoryStore, Store};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Reply = (u16, Vec<u8>);

#[derive(Default, Clone)]
struct Pictures {
    replies: Arc<Mutex<HashMap<String, Reply>>>,
    asked: Arc<Mutex<Vec<String>>>,
}

fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    b.extend_from_slice(&64u32.to_be_bytes());
    b.extend_from_slice(&64u32.to_be_bytes());
    b
}

impl Pictures {
    async fn start() -> (Pictures, String) {
        let pictures = Pictures::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let shared = pictures.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let shared = shared.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
                    let key = path.split('?').next().unwrap_or("").to_string();
                    shared.asked.lock().unwrap().push(text.clone());
                    let (status, body) = shared
                        .replies
                        .lock()
                        .unwrap()
                        .get(&key)
                        .cloned()
                        .unwrap_or((404, vec![]));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                });
            }
        });
        (pictures, base)
    }

    fn set(&self, path: &str, status: u16, body: Vec<u8>) {
        self.replies
            .lock()
            .unwrap()
            .insert(path.into(), (status, body));
    }

    fn count(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

fn engine(base: &str) -> Engine {
    Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_avatar_sources(&format!("{base}/person"), &format!("{base}/logo"))
}

async fn avatar(e: &Engine, address: &str) -> Value {
    e.call("avatar", json!({ "address": address }))
        .await
        .unwrap()
}

// Where person requests go. The address itself must never appear after this.
const MAYA: &str = "/person/";

#[tokio::test]
async fn a_persons_photo_comes_first_using_a_scrambled_address() {
    let (p, base) = Pictures::start().await;
    p.set("/logo/northwind.test.ico", 200, png());
    let e = engine(&base);
    // No photo yet, so the company logo is used.
    let found = avatar(&e, "Maya@News.Northwind.test").await;
    assert_eq!(found["kind"], "logo");
    assert!(found["picture"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    let asked = p.asked.lock().unwrap().join("\n");
    assert!(asked.contains(MAYA), "{asked}");
    assert!(!asked.contains("maya"), "the address itself is never sent");
    assert!(!asked.to_lowercase().contains("cookie"));
    assert!(
        asked.contains("GET /logo/northwind.test.ico"),
        "only the company"
    );

    let (p2, base2) = Pictures::start().await;
    let e2 = engine(&base2);
    let path = {
        // Whatever hash it asks for gets a photo.
        let _ = avatar(&e2, "sam@fieldwork.test").await;
        let first = p2.asked.lock().unwrap()[0].clone();
        first
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap()
            .to_string()
    };
    p2.set(&path, 200, png());
    let e3 = engine(&base2);
    assert_eq!(avatar(&e3, "sam@fieldwork.test").await["kind"], "person");
}

#[tokio::test]
async fn answers_are_kept_so_services_are_asked_once() {
    let (p, base) = Pictures::start().await;
    p.set("/logo/northwind.test.ico", 200, png());
    let e = engine(&base);
    avatar(&e, "maya@northwind.test").await;
    let after_first = p.count();
    assert_eq!(after_first, 2, "one person request, one logo request");
    avatar(&e, "maya@northwind.test").await;
    assert_eq!(p.count(), after_first, "nothing asked again");
    // Someone else at the same company reuses the logo.
    avatar(&e, "lee@northwind.test").await;
    assert_eq!(p.count(), after_first + 1, "only the new person's photo");
}

#[tokio::test]
async fn shared_providers_get_no_logo() {
    let (p, base) = Pictures::start().await;
    p.set("/logo/gmail.com.ico", 200, png());
    let e = engine(&base);
    assert!(avatar(&e, "someone@gmail.com").await["picture"].is_null());
    assert!(!p.asked.lock().unwrap().join("\n").contains("/logo/"));
}

#[tokio::test]
async fn harmful_or_useless_replies_are_refused() {
    let (p, base) = Pictures::start().await;
    p.set(
        "/logo/svg.test.ico",
        200,
        b"<svg xmlns='x'><script>x()</script>........</svg>".to_vec(),
    );
    p.set(
        "/logo/html.test.ico",
        200,
        b"<html><script>x()</script></html>".to_vec(),
    );
    p.set("/logo/huge.test.ico", 200, vec![0x89; 300 * 1024]);
    let e = engine(&base);
    for address in ["a@svg.test", "a@html.test", "a@huge.test", "a@none.test"] {
        assert!(avatar(&e, address).await["picture"].is_null(), "{address}");
    }
}

#[tokio::test]
async fn unsafe_addresses_never_reach_the_network() {
    let (p, base) = Pictures::start().await;
    let e = engine(&base);
    for address in [
        "a@localhost",
        "a@192.168.1.1",
        "a@x.test/../../secret",
        "nonsense",
    ] {
        assert!(avatar(&e, address).await["picture"].is_null());
    }
    assert_eq!(p.count(), 0);
}

#[tokio::test]
async fn being_offline_is_asked_again_later() {
    let (p, base) = Pictures::start().await;
    p.set("/logo/flaky.test.ico", 500, vec![]);
    let e = engine(&base);
    assert!(avatar(&e, "a@flaky.test").await["picture"].is_null());
    p.set("/logo/flaky.test.ico", 200, png());
    assert_eq!(avatar(&e, "a@flaky.test").await["kind"], "logo");
}

#[tokio::test]
async fn turning_it_off_stops_all_requests_and_clears_what_was_kept() {
    let (p, base) = Pictures::start().await;
    p.set("/logo/northwind.test.ico", 200, png());
    let e = engine(&base);
    assert_eq!(
        e.call("app_settings", json!({})).await.unwrap()["sender_pictures"],
        "on"
    );
    avatar(&e, "maya@northwind.test").await;
    let before = p.count();
    e.call(
        "set_setting",
        json!({ "key": "sender_pictures", "value": "off" }),
    )
    .await
    .unwrap();
    assert!(avatar(&e, "maya@northwind.test").await["picture"].is_null());
    assert_eq!(p.count(), before);
    assert_eq!(
        e.call("app_settings", json!({})).await.unwrap()["sender_pictures"],
        "off"
    );
    // Back on, it has to ask again, because nothing was kept.
    e.call(
        "set_setting",
        json!({ "key": "sender_pictures", "value": "on" }),
    )
    .await
    .unwrap();
    assert_eq!(avatar(&e, "maya@northwind.test").await["kind"], "logo");
    assert!(p.count() > before);
}

async fn senders(e: &Engine) -> Value {
    e.call("image_senders", json!({})).await.unwrap()
}

#[tokio::test]
async fn showing_pictures_is_remembered_for_that_sender_only() {
    let (_, base) = Pictures::start().await;
    let e = engine(&base);
    assert!(senders(&e).await.as_array().unwrap().is_empty());
    e.call(
        "set_sender_images",
        json!({ "address": "News@Digest.test", "allowed": true }),
    )
    .await
    .unwrap();
    assert_eq!(senders(&e).await, json!(["news@digest.test"]));
    e.call(
        "set_sender_images",
        json!({ "address": "other@digest.test", "allowed": true }),
    )
    .await
    .unwrap();
    e.call(
        "set_sender_images",
        json!({ "address": "NEWS@digest.test", "allowed": false }),
    )
    .await
    .unwrap();
    assert_eq!(senders(&e).await, json!(["other@digest.test"]));
}
