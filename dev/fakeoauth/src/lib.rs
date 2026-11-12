//! A stand-in for Microsoft's and Google's sign-in, checking the same things they
//! check, and skipping the page where a person clicks "Allow".

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use data_encoding::BASE64URL_NOPAD;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const CLIENT_ID: &str = "fake-mail-client";

/// Called with each access token handed out, so a fake mail server can accept it.
pub type OnToken = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Default)]
struct Inner {
    email: String,
    counter: usize,
    codes: HashMap<String, (String, String)>,
    refresh_tokens: Vec<String>,
    refused: bool,
    no_refresh: bool,
    expires_in: i64,
    rotates: bool,
    renewals: usize,
    last_authorize: HashMap<String, String>,
    secret: Option<String>,
}

#[derive(Clone)]
pub struct FakeOAuth {
    inner: Arc<Mutex<Inner>>,
    pub url: String,
}

#[derive(Clone)]
struct App {
    inner: Arc<Mutex<Inner>>,
    on_token: OnToken,
}

async fn authorize(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut s = app.inner.lock().unwrap();
    s.last_authorize = q.clone();
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    let redirect = get("redirect_uri");
    let good = get("client_id") == CLIENT_ID
        && get("response_type") == "code"
        && get("code_challenge_method") == "S256"
        && !get("code_challenge").is_empty()
        && !get("state").is_empty()
        && (redirect.starts_with("http://localhost:") || redirect.starts_with("http://127.0.0.1:"));
    if !good {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if s.refused {
        return Redirect::to(&format!(
            "{redirect}/?error=access_denied&state={}",
            get("state")
        ))
        .into_response();
    }
    s.counter += 1;
    let code = format!("code-{}", s.counter);
    s.codes
        .insert(code.clone(), (get("code_challenge"), redirect.clone()));
    Redirect::to(&format!("{redirect}/?code={code}&state={}", get("state"))).into_response()
}

async fn token(
    State(app): State<App>,
    Form(f): Form<HashMap<String, String>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut s = app.inner.lock().unwrap();
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let refuse = || {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
    };
    if get("client_id") != CLIENT_ID {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_client" })),
        );
    }
    if let Some(secret) = &s.secret {
        if &get("client_secret") != secret {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid_client" })),
            );
        }
    }
    s.counter += 1;
    let n = s.counter;
    let access = format!("access-{n}");
    let email = s.email.clone();
    let expires_in = s.expires_in;
    let reply = match get("grant_type").as_str() {
        "authorization_code" => {
            let Some((challenge, redirect)) = s.codes.remove(&get("code")) else {
                return refuse();
            };
            let proof = BASE64URL_NOPAD.encode(&Sha256::digest(get("code_verifier").as_bytes()));
            if proof != challenge || redirect != get("redirect_uri") {
                return refuse();
            }
            let claims = BASE64URL_NOPAD.encode(json!({ "email": email }).to_string().as_bytes());
            let mut reply = json!({
                "access_token": access,
                "expires_in": expires_in,
                "id_token": format!("x.{claims}.y"),
            });
            if !s.no_refresh {
                let refresh = format!("refresh-{n}");
                s.refresh_tokens.push(refresh.clone());
                reply["refresh_token"] = json!(refresh);
            }
            reply
        }
        "refresh_token" => {
            if !s.refresh_tokens.contains(&get("refresh_token")) {
                return refuse();
            }
            s.renewals += 1;
            let mut reply = json!({ "access_token": access, "expires_in": expires_in });
            if s.rotates {
                let refresh = format!("refresh-{n}");
                s.refresh_tokens.push(refresh.clone());
                reply["refresh_token"] = json!(refresh);
            }
            reply
        }
        _ => return refuse(),
    };
    (app.on_token)(&access, &email);
    (StatusCode::OK, Json(reply))
}

impl FakeOAuth {
    pub async fn start_on(addr: &str, email: &str, on_token: OnToken) -> FakeOAuth {
        let inner = Arc::new(Mutex::new(Inner {
            email: email.into(),
            expires_in: 3600,
            ..Default::default()
        }));
        let app = App {
            inner: inner.clone(),
            on_token,
        };
        let router = Router::new()
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(app);
        let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        FakeOAuth { inner, url }
    }

    pub fn set_email(&self, email: &str) {
        self.inner.lock().unwrap().email = email.into();
    }
    pub fn refuse_at_consent(&self, refuse: bool) {
        self.inner.lock().unwrap().refused = refuse;
    }
    pub fn give_no_refresh(&self, none: bool) {
        self.inner.lock().unwrap().no_refresh = none;
    }
    pub fn expire_in(&self, seconds: i64) {
        self.inner.lock().unwrap().expires_in = seconds;
    }
    pub fn rotate_refresh(&self, rotates: bool) {
        self.inner.lock().unwrap().rotates = rotates;
    }
    /// As if the person removed the app's access in their account.
    pub fn revoke_all(&self) {
        self.inner.lock().unwrap().refresh_tokens.clear();
    }
    pub fn require_secret(&self, secret: &str) {
        self.inner.lock().unwrap().secret = Some(secret.into());
    }
    pub fn renewals(&self) -> usize {
        self.inner.lock().unwrap().renewals
    }
    pub fn last_authorize(&self) -> HashMap<String, String> {
        self.inner.lock().unwrap().last_authorize.clone()
    }
}
