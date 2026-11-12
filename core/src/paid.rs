//! The plug-in point for the AI that comes with Mailzo Cloud.
//!
//! Four things are Cloud's to do: the daily brief, answering a question about the person's
//! mail, sorting new mail, and a second opinion on mail that might be spam. How they're
//! worked out lives with Cloud, not here. This app only gathers the material on the device
//! (who wrote, the subject, a line or an excerpt, and never an encrypted email's words),
//! hands it over, and applies the answer.
//!
//! With nothing plugged in, those four simply aren't available: asking for one says it comes
//! with Mailzo Cloud, and the automatic ones quietly do nothing. The engine plugs in
//! `CloudPaidAi` by itself once it has an account server and the person is signed in.

use crate::ai::ai_error;
use crate::cloud::CloudClient;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What every call to the plug-in hands back.
pub type PaidFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Said when one of the four is asked for without Mailzo Cloud.
pub const PAID_ONLY: &str = "This comes with Mailzo Cloud.";

/// How long an answer about whether Cloud is active is trusted.
const STATUS_KEPT: Duration = Duration::from_secs(5 * 60);
/// Answers take an AI a while to write, longer than the account server's usual wait.
const PAID_WAIT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PaidStatus {
    pub active: bool,
}

/// One conversation waiting on the person, as the brief sees it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BriefItem {
    pub from: String,
    pub subject: String,
    /// Its one line, or the start of it. Empty for an encrypted conversation.
    pub line: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BriefRequest {
    pub language: String,
    pub items: Vec<BriefItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BriefAnswer {
    pub text: String,
}

/// One email a question may be answered from. Never an encrypted one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskSource {
    pub id: i64,
    pub from: String,
    pub subject: String,
    /// Seconds since 1970.
    pub date: i64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskRequest {
    pub language: String,
    pub question: String,
    pub sources: Vec<AskSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskItem {
    /// One of the sources' ids.
    pub id: i64,
    pub title: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub headline: String,
    #[serde(default)]
    pub items: Vec<AskItem>,
}

/// One new email to sort. `text` is empty for an encrypted one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SortItem {
    pub index: usize,
    pub from: String,
    pub subject: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SortRequest {
    pub language: String,
    pub items: Vec<SortItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SortResult {
    pub index: usize,
    /// general, newsletters or promotions.
    #[serde(default)]
    pub tab: String,
    /// receipt, needs_reply or none.
    #[serde(default)]
    pub label: String,
    /// needs_you or good_to_know.
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub line: String,
    /// `YYYY-MM-DD`, or empty.
    #[serde(default)]
    pub due: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SortAnswer {
    pub items: Vec<SortResult>,
}

/// An email the spam filter is unsure about. `text` is empty for an encrypted one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpamQuestion {
    pub from: String,
    pub subject: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpamRequest {
    pub language: String,
    #[serde(flatten)]
    pub question: SpamQuestion,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpamOpinion {
    pub spam: bool,
    #[serde(default)]
    pub reason: String,
}

/// The AI that comes with Mailzo Cloud. Plugged in with `Engine::with_paid_ai`, or found by
/// itself once the person is signed in. An error's code and message reach the person as
/// they are, so they should be plain words.
pub trait PaidAi: Send + Sync {
    /// Whether this person has Cloud right now.
    fn status(&self) -> PaidFuture<'_, PaidStatus>;
    fn brief(&self, request: BriefRequest) -> PaidFuture<'_, BriefAnswer>;
    fn ask(&self, request: AskRequest) -> PaidFuture<'_, AskAnswer>;
    fn sort(&self, request: SortRequest) -> PaidFuture<'_, SortAnswer>;
    fn spam(&self, request: SpamRequest) -> PaidFuture<'_, SpamOpinion>;
}

/// Where the session token comes from, asked afresh each time so signing out takes effect.
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Mailzo Cloud's AI, over the same account server and session that sign-in uses.
pub struct CloudPaidAi {
    client: CloudClient,
    token: TokenSource,
    /// The last answer about being active, for which session, and when.
    status: Mutex<Option<(String, Instant, PaidStatus)>>,
}

impl CloudPaidAi {
    pub fn new(server: &str, token: TokenSource) -> Result<CloudPaidAi> {
        Ok(CloudPaidAi {
            client: CloudClient::with_timeout(server, PAID_WAIT)?,
            token,
            status: Mutex::new(None),
        })
    }

    fn token(&self) -> Result<String> {
        (self.token)().ok_or_else(|| ai_error("paid_only", PAID_ONLY))
    }

    async fn post<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let token = self.token()?;
        let body = serde_json::to_value(body).map_err(|e| CoreError::Invalid(e.to_string()))?;
        let result = self
            .client
            .send(Method::POST, path, Some(&token), Some(&body))
            .await
            .map_err(as_ai_error);
        // Cloud said it isn't active, so what's kept about it is out of date.
        if let Err(CoreError::Ai { code, .. }) = &result {
            if code == "paid_only" {
                *self.status.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
        result
    }
}

/// The account server's refusals reach the person the same way AI problems do.
fn as_ai_error(e: CoreError) -> CoreError {
    match e {
        CoreError::Cloud { code, message } => CoreError::Ai { code, message },
        other => other,
    }
}

impl PaidAi for CloudPaidAi {
    fn status(&self) -> PaidFuture<'_, PaidStatus> {
        Box::pin(async move {
            let Some(token) = (self.token)() else {
                return Ok(PaidStatus { active: false });
            };
            {
                let kept = self.status.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((for_token, at, status)) = kept.as_ref() {
                    if *for_token == token && at.elapsed() < STATUS_KEPT {
                        return Ok(status.clone());
                    }
                }
            }
            let status: PaidStatus = self
                .client
                .send(Method::GET, "/paid/status", Some(&token), None)
                .await
                .map_err(as_ai_error)?;
            *self.status.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((token, Instant::now(), status.clone()));
            Ok(status)
        })
    }

    fn brief(&self, request: BriefRequest) -> PaidFuture<'_, BriefAnswer> {
        Box::pin(async move { self.post("/paid/brief", &request).await })
    }

    fn ask(&self, request: AskRequest) -> PaidFuture<'_, AskAnswer> {
        Box::pin(async move { self.post("/paid/ask", &request).await })
    }

    fn sort(&self, request: SortRequest) -> PaidFuture<'_, SortAnswer> {
        Box::pin(async move { self.post("/paid/sort", &request).await })
    }

    fn spam(&self, request: SpamRequest) -> PaidFuture<'_, SpamOpinion> {
        Box::pin(async move { self.post("/paid/spam", &request).await })
    }
}

impl Engine {
    /// Plugs in the AI that comes with Mailzo Cloud. Without it, the engine uses Cloud's own
    /// once it has an account server and the person is signed in.
    pub fn with_paid_ai(mut self, paid: Arc<dyn PaidAi>) -> Self {
        self.paid_ai = Some(paid);
        self
    }

    /// The plug-in, if there is one: one given to the engine, or Cloud's while signed in.
    pub(crate) fn paid_ai(&self) -> Option<Arc<dyn PaidAi>> {
        if let Some(paid) = &self.paid_ai {
            return Some(paid.clone());
        }
        let signed_in = self
            .secrets
            .get(crate::identity::SESSION)
            .ok()
            .flatten()
            .is_some();
        self.cloud_paid
            .as_ref()
            .filter(|_| signed_in)
            .map(|c| c.clone() as Arc<dyn PaidAi>)
    }

    /// For something the person asked for: the plug-in, or why it isn't there.
    pub(crate) async fn paid_or_refuse(&self) -> Result<Arc<dyn PaidAi>> {
        let paid = self
            .paid_ai()
            .ok_or_else(|| ai_error("paid_only", PAID_ONLY))?;
        if !paid.status().await?.active {
            return Err(ai_error("paid_only", PAID_ONLY));
        }
        Ok(paid)
    }

    /// For what happens by itself: the plug-in only if it's there and active, and quietly
    /// nothing otherwise.
    pub(crate) async fn paid_if_active(&self) -> Option<Arc<dyn PaidAi>> {
        let paid = self.paid_ai()?;
        match paid.status().await {
            Ok(s) if s.active => Some(paid),
            _ => None,
        }
    }

    /// Whether the four Cloud features can be used, so the screens can show the right thing.
    pub(crate) async fn paid_status(&self) -> Result<Value> {
        Ok(serde_json::json!({ "paid": self.paid_if_active().await.is_some() }))
    }
}
