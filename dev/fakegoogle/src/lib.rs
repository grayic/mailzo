//! A stand-in for Google's sign-in, for tests and the demo. It checks the same
//! things Google checks, and skips the page where a person clicks "Allow".

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub const CLIENT_ID: &str = "fake-client.apps.test";
pub const CLIENT_SECRET: &str = "fake-client-secret";

/// What the next sign-in will report. Tests change this to try bad replies.
#[derive(Clone)]
pub struct Person {
    pub email: String,
    pub email_verified: bool,
    pub audience: String,
    pub issuer: String,
    pub nonce_override: Option<String>,
    pub refuse_at_consent: bool,
}

impl Person {
    pub fn new(email: &str) -> Person {
        Person {
            email: email.into(),
            email_verified: true,
            audience: CLIENT_ID.into(),
            issuer: "https://accounts.google.com".into(),
            nonce_override: None,
            refuse_at_consent: false,
        }
    }
}

struct Grant {
    challenge: String,
    redirect_uri: String,
    nonce: String,
}

struct Inner {
    person: Mutex<Person>,
    grants: Mutex<HashMap<String, Grant>>,
    next: Mutex<u32>,
    issued: Mutex<HashSet<String>>,
}

#[derive(Clone)]
pub struct FakeGoogle {
    pub auth_url: String,
    pub token_url: String,
    pub tokeninfo_url: String,
    inner: Arc<Inner>,
}

impl FakeGoogle {
    pub async fn start(email: &str) -> FakeGoogle {
        Self::start_on("127.0.0.1:0", email).await
    }

    pub async fn start_on(bind: &str, email: &str) -> FakeGoogle {
        let inner = Arc::new(Inner {
            person: Mutex::new(Person::new(email)),
            grants: Mutex::new(HashMap::new()),
            next: Mutex::new(0),
            issued: Mutex::new(HashSet::new()),
        });
        let router = Router::new()
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/tokeninfo", post(tokeninfo))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind(bind).await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        FakeGoogle {
            auth_url: format!("http://{addr}/authorize"),
            token_url: format!("http://{addr}/token"),
            tokeninfo_url: format!("http://{addr}/tokeninfo"),
            inner,
        }
    }

    /// What Google hands an app that signs in by itself.
    pub fn id_token(&self) -> String {
        mint(&self.inner, "")
    }

    pub fn change_person(&self, change: impl FnOnce(&mut Person)) {
        change(&mut self.inner.person.lock().unwrap());
    }
}

#[derive(Deserialize)]
struct AuthQuery {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: String,
    nonce: String,
    code_challenge: String,
    code_challenge_method: String,
}

async fn authorize(State(g): State<Arc<Inner>>, Query(q): Query<AuthQuery>) -> Response {
    let sep = |kind: &str| format!("{}?state={}&{kind}", q.redirect_uri, q.state);
    if q.client_id != CLIENT_ID || q.response_type != "code" || q.code_challenge_method != "S256" {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if g.person.lock().unwrap().refuse_at_consent {
        return Redirect::to(&sep("error=access_denied")).into_response();
    }
    let code = {
        let mut n = g.next.lock().unwrap();
        *n += 1;
        format!("code-{n}")
    };
    g.grants.lock().unwrap().insert(
        code.clone(),
        Grant {
            challenge: q.code_challenge.clone(),
            redirect_uri: q.redirect_uri.clone(),
            nonce: q.nonce.clone(),
        },
    );
    Redirect::to(&sep(&format!("code={code}"))).into_response()
}

#[derive(Deserialize)]
struct TokenForm {
    code: String,
    code_verifier: String,
    redirect_uri: String,
    client_id: String,
    client_secret: String,
}

async fn token(State(g): State<Arc<Inner>>, Form(f): Form<TokenForm>) -> Response {
    let refuse = || {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response()
    };
    // A code works once, as with the real Google.
    let Some(grant) = g.grants.lock().unwrap().remove(&f.code) else {
        return refuse();
    };
    let challenge = BASE64URL_NOPAD.encode(&Sha256::digest(f.code_verifier.as_bytes()));
    if f.client_id != CLIENT_ID
        || f.client_secret != CLIENT_SECRET
        || challenge != grant.challenge
        || f.redirect_uri != grant.redirect_uri
    {
        return refuse();
    }
    let nonce = g
        .person
        .lock()
        .unwrap()
        .nonce_override
        .clone()
        .unwrap_or(grant.nonce);
    let id_token = mint(&g, &nonce);
    Json(json!({ "id_token": id_token })).into_response()
}

fn mint(g: &Inner, nonce: &str) -> String {
    let person = g.person.lock().unwrap().clone();
    let claims = json!({
        "iss": person.issuer, "aud": person.audience, "email": person.email,
        "email_verified": person.email_verified, "exp": 4_102_444_800i64,
        "nonce": nonce,
    });
    let part = |v: serde_json::Value| BASE64URL_NOPAD.encode(v.to_string().as_bytes());
    let signature = HEXLOWER.encode(b"not-checked");
    let id_token = format!(
        "{}.{}.{signature}",
        part(json!({"alg": "none"})),
        part(claims)
    );
    g.issued.lock().unwrap().insert(id_token.clone());
    id_token
}

#[derive(Deserialize)]
struct TokenInfoForm {
    id_token: String,
}

/// Google's own check of a token it signed. Numbers and yes-or-no answers come back as words.
async fn tokeninfo(State(g): State<Arc<Inner>>, Form(f): Form<TokenInfoForm>) -> Response {
    if !g.issued.lock().unwrap().contains(&f.id_token) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_token" })),
        )
            .into_response();
    }
    let payload = f.id_token.split('.').nth(1).unwrap_or_default();
    let bytes = BASE64URL_NOPAD
        .decode(payload.as_bytes())
        .unwrap_or_default();
    let mut claims: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    claims["email_verified"] = json!(claims["email_verified"].to_string());
    claims["exp"] = json!(claims["exp"].to_string());
    Json(claims).into_response()
}
