//! Checks the encrypted connection against real mail providers. Needs internet,
//! so it only runs on request:
//!
//!   cargo test -p mailcore --test real_servers -- --ignored
//!
//! It signs in with a made-up address, so each provider should refuse it.

use mailcore::{CoreError, Engine, MemoryStore, Store};
use serde_json::json;
use std::sync::Arc;

async fn try_sign_in(host: &str, port: u16) -> CoreError {
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    );
    engine
        .call(
            "add_account",
            json!({
                "email": "nobody@example.test", "imap_host": host, "imap_port": port,
                "security": "tls", "password": "not-a-real-password",
            }),
        )
        .await
        .unwrap_err()
}

#[tokio::test]
#[ignore]
async fn real_providers_complete_encryption_and_refuse_a_made_up_login() {
    for host in [
        "imap.gmail.com",
        "imap.mail.me.com",
        "imap.zoho.com",
        "outlook.office365.com",
    ] {
        let err = try_sign_in(host, 993).await;
        assert!(matches!(err, CoreError::Auth), "{host}: {err}");
    }
}

#[tokio::test]
#[ignore]
async fn servers_with_bad_certificates_are_refused() {
    for host in [
        "expired.badssl.com",
        "self-signed.badssl.com",
        "wrong.host.badssl.com",
    ] {
        let err = try_sign_in(host, 443).await;
        let text = err.to_string().to_lowercase();
        assert!(matches!(err, CoreError::Network(_)), "{host}: {text}");
        assert!(text.contains("certificate"), "{host}: {text}");
    }
}

/// The live account server, reached the way the Mac app reaches it. Uses an
/// address reserved for testing, so no person receives anything.
#[tokio::test]
#[ignore]
async fn the_live_account_server_answers_over_encryption() {
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_cloud("https://account.getmailzo.com", "Live test")
    .unwrap();
    let email = "inbox-live-test@example.com";
    engine
        .call("auth_start", json!({ "email": email }))
        .await
        .unwrap();
    let err = engine
        .call("auth_verify", json!({ "email": email, "code": "abc" }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_code");
}

fn made_up_account() -> mailcore::model::Account {
    mailcore::model::Account {
        id: 1,
        sync_id: String::new(),
        email: "nobody@example.test".into(),
        display_name: String::new(),
        imap_host: String::new(),
        imap_port: 993,
        security: mailcore::model::Security::Tls,
        username: "nobody@example.test".into(),
        smtp_host: String::new(),
        smtp_port: 0,
        smtp_security: mailcore::model::SmtpSecurity::Tls,
        signature: String::new(),
        settings_updated_at: 0,
        prefs: Default::default(),
        auth: "password".into(),
    }
}

/// Reaches real outgoing servers over encryption, and is refused for the made-up
/// login. It never gets as far as sending anything.
#[tokio::test]
#[ignore]
async fn real_outgoing_servers_connect_securely_and_refuse_a_made_up_login() {
    use mailcore::model::SmtpSecurity::{StartTls, Tls};
    for (host, port, security) in [
        ("smtp.gmail.com", 465, Tls),
        ("smtp.migadu.com", 465, Tls),
        ("smtp.mail.me.com", 587, StartTls),
        ("smtp.office365.com", 587, StartTls),
        ("smtp.zoho.com", 465, Tls),
    ] {
        let err = mailcore::smtp::check(
            &made_up_account(),
            "not-a-real-password",
            host,
            port,
            security,
            mailcore::net::Reach::Anywhere,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "smtp_auth", "{host}: {err}");
    }
}

#[tokio::test]
#[ignore]
async fn outgoing_servers_with_bad_certificates_are_refused() {
    use mailcore::model::SmtpSecurity::Tls;
    for host in [
        "expired.badssl.com",
        "self-signed.badssl.com",
        "wrong.host.badssl.com",
    ] {
        let err = mailcore::smtp::check(
            &made_up_account(),
            "not-a-real-password",
            host,
            443,
            Tls,
            mailcore::net::Reach::Anywhere,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "smtp_connect", "{host}: {err}");
    }
}

/// Every server on our own list really answers, securely. No sign-in is attempted.
#[tokio::test]
#[ignore]
async fn every_known_server_answers() {
    use mailcore::model::SmtpSecurity;
    use mailcore::net::Reach::Anywhere;
    use std::time::Duration;
    let mut silent = Vec::new();
    for (provider, host, port, security) in mailcore::providers::known_servers() {
        let limit = Duration::from_secs(10);
        let good = match (port, security) {
            (993, _) => mailcore::imap::answers(&host, port, limit, Anywhere).await,
            (_, SmtpSecurity::StartTls) => {
                tokio::time::timeout(limit, tokio::net::TcpStream::connect((host.as_str(), port)))
                    .await
                    .is_ok_and(|c| c.is_ok())
            }
            _ => mailcore::imap::tls_answers(&host, port, limit, Anywhere).await,
        };
        println!(
            "{provider:10} {host}:{port} {}",
            if good { "ok" } else { "NO ANSWER" }
        );
        if !good {
            silent.push(host);
        }
    }
    assert!(silent.is_empty(), "{silent:?}");
}

/// What real addresses resolve to, using the real internet.
#[tokio::test]
#[ignore]
async fn real_addresses_are_found() {
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    );
    for email in [
        "x@gmail.com",
        "x@outlook.com",
        "x@icloud.com",
        "x@yahoo.com",
        "x@aol.com",
        "x@zoho.com",
        "x@fastmail.com",
        "x@gmx.com",
        "x@gmx.net",
        "x@mail.com",
        "x@web.de",
        "x@yandex.com",
        "x@proton.me",
        "x@tuta.com",
        "x@hey.com",
        "x@example.com",
        "x@microsoft.com",
        "x@shopify.com",
        "x@comcast.net",
        "x@orange.fr",
        "x@mail.ru",
        "x@qq.com",
        "x@163.com",
        "x@att.net",
        "x@this-domain-should-not-exist-12345.com",
    ] {
        let f = engine.find_provider(email).await;
        println!(
            "{email:32} {:?} {} {}:{} smtp {}:{} via {}",
            f.method, f.name, f.imap_host, f.imap_port, f.smtp_host, f.smtp_port, f.source
        );
    }
}
