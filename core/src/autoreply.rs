//! Out-of-office replies. Email itself has no standard way to ask for these, and the
//! provider has to send them because this device may be off. So each provider has its
//! own method, and for the rest the app says where to set it on their website.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::unix_now;
use crate::model::Account;
use crate::secrets::account_secret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const GMAIL_API: &str =
    "https://gmail.googleapis.com/gmail/v1/users/me/settings/vacation";
pub(crate) const GRAPH_API: &str = "https://graph.microsoft.com/v1.0/me/mailboxSettings";
const GRAPH_SCOPE: &str = "https://graph.microsoft.com/MailboxSettings.ReadWrite";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoReply {
    pub on: bool,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub message: String,
    /// When it starts and stops, if not right away and until turned off.
    #[serde(default)]
    pub starts: Option<i64>,
    #[serde(default)]
    pub ends: Option<i64>,
    /// Only people in their contacts, or inside their organisation, get a reply.
    #[serde(default)]
    pub contacts_only: bool,
}

fn unsupported(account: &Account) -> Value {
    let (name, link) = provider_page(&account.imap_host);
    json!({ "supported": false, "provider": name, "link": link })
}

/// Where a provider keeps this on its own website.
fn provider_page(host: &str) -> (&'static str, Option<&'static str>) {
    let host = host.to_ascii_lowercase();
    let host_is = |d: &str| crate::calendar::host_is(&host, d);
    if host_is("migadu.com") {
        ("Migadu", Some("https://admin.migadu.com/"))
    } else if host_is("icloud.com") || host_is("me.com") {
        ("iCloud", Some("https://www.icloud.com/mail/"))
    } else if host_is("yahoo.com") {
        ("Yahoo", Some("https://mail.yahoo.com/"))
    } else if host.contains(".zoho.") {
        ("Zoho", Some("https://mail.zoho.com/"))
    } else if host_is("fastmail.com") {
        ("Fastmail", Some("https://app.fastmail.com/settings/"))
    } else {
        ("your email provider", None)
    }
}

fn iso(unix: i64) -> String {
    mail_parser::DateTime::from_timestamp(unix)
        .to_rfc3339()
        .trim_end_matches('Z')
        .to_string()
}

fn from_iso(text: &str) -> Option<i64> {
    let cleaned = if text.ends_with('Z') || text.contains('+') {
        text.to_string()
    } else {
        // Microsoft leaves off the zone and sends extra decimals. It's asked for in UTC.
        format!("{}Z", text.split('.').next().unwrap_or(text))
    };
    mail_parser::DateTime::parse_rfc3339(&cleaned).map(|d| d.to_timestamp())
}

impl Engine {
    /// Sends a finished message through Microsoft's own mail service.
    pub(crate) async fn graph_send(&self, account: &Account, raw: &[u8]) -> Result<()> {
        let token = self
            .graph_token_for(account, "https://graph.microsoft.com/Mail.Send")
            .await?;
        let url = self.graph_api.replace("/mailboxSettings", "/sendMail");
        let reply = self
            .ai_http
            .post(url)
            .bearer_auth(token)
            .header("content-type", "text/plain")
            .body(data_encoding::BASE64.encode(raw))
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        if !reply.status().is_success() {
            return Err(CoreError::Invalid(format!(
                "Microsoft didn't accept that (error {}).",
                reply.status().as_u16()
            )));
        }
        Ok(())
    }

    async fn graph_token(&self, account: &Account) -> Result<String> {
        self.graph_token_for(account, GRAPH_SCOPE).await
    }

    async fn graph_token_for(&self, account: &Account, scope: &str) -> Result<String> {
        let refresh = self
            .secrets
            .get(&account_secret(account.id))?
            .ok_or_else(|| CoreError::NeedsSignIn(account.email.clone()))?;
        let config = self.sign_in_for_provider(&account.auth)?;
        let reply = self.tokens_for_scope(&config, &refresh, scope).await?;
        let status = reply.status().as_u16();
        if !reply.status().is_success() {
            return Err(if status == 400 || status == 401 {
                CoreError::NeedsSignIn(account.email.clone())
            } else {
                CoreError::Network(format!("the sign-in service answered {status}"))
            });
        }
        #[derive(Deserialize)]
        struct Reply {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
        }
        let fresh: Reply = reply
            .json()
            .await
            .map_err(|_| CoreError::Network("the sign-in service gave a bad answer".into()))?;
        if let Some(next) = fresh.refresh_token.filter(|n| *n != refresh) {
            self.secrets.set(&account_secret(account.id), &next)?;
        }
        Ok(fresh.access_token)
    }

    pub(crate) async fn auto_reply_get(&self, account_id: i64) -> Result<Value> {
        let account = self.store.account(account_id)?;
        match account.auth.as_str() {
            "google" => {
                let token = self.password(account_id).await?;
                let value = self
                    .reply_json(
                        self.ai_http.get(&self.gmail_api).bearer_auth(token),
                        &account,
                    )
                    .await?;
                let at = |k: &str| {
                    value[k]
                        .as_str()
                        .and_then(|t| t.parse::<i64>().ok())
                        .or_else(|| value[k].as_i64())
                        .map(|ms| ms / 1000)
                };
                Ok(json!({
                    "supported": true,
                    "reply": AutoReply {
                        on: value["enableAutoReply"].as_bool().unwrap_or(false),
                        subject: value["responseSubject"].as_str().unwrap_or("").to_string(),
                        message: value["responseBodyPlainText"].as_str().unwrap_or("").to_string(),
                        starts: at("startTime"),
                        ends: at("endTime"),
                        contacts_only: value["restrictToContacts"].as_bool().unwrap_or(false),
                    },
                }))
            }
            "microsoft" => {
                let token = self.graph_token(&account).await?;
                let url = format!("{}/automaticRepliesSetting", self.graph_api);
                let value = self
                    .reply_json(self.ai_http.get(url).bearer_auth(token), &account)
                    .await?;
                let status = value["status"].as_str().unwrap_or("disabled");
                let at = |k: &str| value[k]["dateTime"].as_str().and_then(from_iso);
                Ok(json!({
                    "supported": true,
                    "reply": AutoReply {
                        on: status != "disabled",
                        subject: String::new(),
                        message: value["externalReplyMessage"].as_str().unwrap_or("").to_string(),
                        starts: if status == "scheduled" { at("scheduledStartDateTime") } else { None },
                        ends: if status == "scheduled" { at("scheduledEndDateTime") } else { None },
                        contacts_only: value["externalAudience"] == "contactsOnly",
                    },
                }))
            }
            _ => Ok(unsupported(&account)),
        }
    }

    pub(crate) async fn auto_reply_set(&self, account_id: i64, reply: AutoReply) -> Result<Value> {
        let account = self.store.account(account_id)?;
        if reply.on && reply.message.trim().is_empty() {
            return Err(CoreError::Invalid(
                "Write the message people should get.".into(),
            ));
        }
        if let (Some(a), Some(b)) = (reply.starts, reply.ends) {
            if b <= a {
                return Err(CoreError::Invalid("It has to end after it starts.".into()));
            }
        }
        if reply.on && reply.ends.is_some_and(|e| e <= unix_now()) {
            return Err(CoreError::Invalid(
                "That end date has already passed.".into(),
            ));
        }
        match account.auth.as_str() {
            "google" => {
                let token = self.password(account_id).await?;
                let mut body = json!({
                    "enableAutoReply": reply.on,
                    "responseSubject": reply.subject,
                    "responseBodyPlainText": reply.message,
                    "restrictToContacts": reply.contacts_only,
                    "restrictToDomain": false,
                });
                if let Some(s) = reply.starts {
                    body["startTime"] = json!((s * 1000).to_string());
                }
                if let Some(e) = reply.ends {
                    body["endTime"] = json!((e * 1000).to_string());
                }
                self.reply_json(
                    self.ai_http
                        .put(&self.gmail_api)
                        .bearer_auth(token)
                        .json(&body),
                    &account,
                )
                .await?;
            }
            "microsoft" => {
                let token = self.graph_token(&account).await?;
                let status = match (reply.on, reply.starts.is_some() || reply.ends.is_some()) {
                    (false, _) => "disabled",
                    (true, true) => "scheduled",
                    (true, false) => "alwaysEnabled",
                };
                let mut setting = json!({
                    "status": status,
                    "externalAudience": if reply.contacts_only { "contactsOnly" } else { "all" },
                    "internalReplyMessage": reply.message,
                    "externalReplyMessage": reply.message,
                });
                if status == "scheduled" {
                    let start = reply.starts.unwrap_or_else(unix_now);
                    let end = reply.ends.unwrap_or(start + 30 * 86_400);
                    setting["scheduledStartDateTime"] =
                        json!({ "dateTime": iso(start), "timeZone": "UTC" });
                    setting["scheduledEndDateTime"] =
                        json!({ "dateTime": iso(end), "timeZone": "UTC" });
                }
                let body = json!({ "automaticRepliesSetting": setting });
                self.reply_json(
                    self.ai_http
                        .patch(&self.graph_api)
                        .bearer_auth(token)
                        .json(&body),
                    &account,
                )
                .await?;
            }
            _ => {
                return Err(CoreError::Invalid(
                    "Out-of-office replies for this inbox are set on your provider's website."
                        .into(),
                ))
            }
        }
        Ok(json!({ "saved": true }))
    }

    async fn reply_json(
        &self,
        request: reqwest::RequestBuilder,
        account: &Account,
    ) -> Result<Value> {
        let reply = request
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let status = reply.status().as_u16();
        // No permission means this sign-in came before the feature did.
        if status == 401 || status == 403 {
            return Err(CoreError::NeedsSignIn(account.email.clone()));
        }
        if !reply.status().is_success() {
            return Err(CoreError::Invalid(format!(
                "Your provider didn't accept that (error {status})."
            )));
        }
        Ok(reply.json().await.unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsofts_dates_read_back_as_written() {
        assert_eq!(from_iso(&iso(1_700_000_000)), Some(1_700_000_000));
        assert_eq!(from_iso("2023-11-14T22:13:20.0000000"), Some(1_700_000_000));
    }
}
