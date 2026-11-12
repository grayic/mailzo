//! Talks to the account server. Everything sensitive is encrypted before it
//! reaches this file.

use crate::error::{CoreError, Result};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

fn unreachable() -> CoreError {
    CoreError::Cloud {
        code: "offline".into(),
        message: "Could not reach the account server. Check your internet connection.".into(),
    }
}

/// Trusts the public certificate list, with no system libraries needed.
pub(crate) fn tls_config() -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| CoreError::Network(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

pub struct CloudClient {
    http: reqwest::Client,
    base: String,
}

/// Plain http is only for a server on this computer, as in tests and the demo.
fn check_url(url: &str) -> Result<()> {
    // Read the way the web library reads it, so "http://localhost.evil.example" or
    // "http://127.0.0.1@evil.example" can't pass for this computer.
    let parsed = reqwest::Url::parse(url)
        .ok()
        .filter(|u| !url.contains('\\') && u.username().is_empty() && u.password().is_none());
    let local = parsed.as_ref().is_some_and(|u| {
        u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    });
    let secure = parsed.as_ref().is_some_and(|u| u.scheme() == "https");
    if secure || local {
        Ok(())
    } else {
        Err(CoreError::Invalid(
            "The account server must use an encrypted connection.".into(),
        ))
    }
}

impl CloudClient {
    pub fn new(base: &str) -> Result<CloudClient> {
        Self::with_timeout(base, Duration::from_secs(20))
    }

    /// The same, waiting this long for an answer.
    pub fn with_timeout(base: &str, wait: Duration) -> Result<CloudClient> {
        check_url(base)?;
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls_config()?)
            .timeout(wait)
            .build()
            .map_err(|e| CoreError::Network(e.without_url().to_string()))?;
        Ok(CloudClient {
            http,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    pub async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<T> {
        self.send_for_team(None, method, path, token, body).await
    }

    /// The same, about one of the person's teams.
    pub async fn send_for_team<T: DeserializeOwned>(
        &self,
        team: Option<i64>,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<T> {
        let mut req = self.http.request(method, format!("{}{path}", self.base));
        if let Some(team) = team {
            req = req.header("x-team-id", team);
        }
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req.send().await.map_err(|_| unreachable())?;
        let ok = res.status().is_success();
        let value: Value = res.json().await.map_err(|_| unreachable())?;
        if !ok {
            return Err(CoreError::Cloud {
                code: value["code"].as_str().unwrap_or("server").to_string(),
                message: value["message"]
                    .as_str()
                    .unwrap_or("The account server reported a problem.")
                    .to_string(),
            });
        }
        serde_json::from_value(value).map_err(|_| unreachable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_only_for_this_computer_exactly() {
        for ok in [
            "https://api.getmailzo.com",
            "http://127.0.0.1:8080",
            "http://localhost:3000/x",
            "http://[::1]:9000",
        ] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://127.0.0.1@evil.example",
            "http://localhost:80@evil.example",
            "http://[::1].evil.example",
            "http://example.com",
            "ftp://localhost",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }
}
