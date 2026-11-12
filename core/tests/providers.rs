//! Working out how to connect an address, without touching the real internet.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use mailcore::providers::{parse_settings, Found, Method};
use mailcore::{Engine, MemoryStore, Store};
use std::sync::Arc;

const SETTINGS: &str = r#"<clientConfig version="1.1"><emailProvider id="x">
  <displayName>Example Mail</displayName>
  <incomingServer type="pop3"><hostname>pop.example.net</hostname><port>995</port><socketType>SSL</socketType>
    <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication></incomingServer>
  <incomingServer type="imap"><hostname>imap.example.net</hostname><port>993</port><socketType>SSL</socketType>
    <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication></incomingServer>
  <outgoingServer type="smtp"><hostname>smtp.example.net</hostname><port>587</port><socketType>STARTTLS</socketType>
    <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication></outgoingServer>
</emailProvider></clientConfig>"#;

async fn engine() -> Engine {
    async fn directory(Path(domain): Path<String>) -> Result<String, StatusCode> {
        match domain.as_str() {
            "listed.test" | "bigmailhost.test" => Ok(SETTINGS.to_string()),
            _ => Err(StatusCode::NOT_FOUND),
        }
    }
    let router = Router::new().route("/{domain}", get(directory));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_provider_lookup(
        &url,
        Arc::new(|domain: &str| match domain {
            "company.test" => vec!["aspmx.l.google.com.".to_string()],
            "acme.test" => vec!["acme-test.mail.protection.outlook.com.".to_string()],
            "zohoed.test" => vec!["mx.zoho.eu.".to_string()],
            "migadued.test" => vec!["aspmx1.migadu.com.".to_string()],
            "hosted.test" => vec!["mx1.bigmailhost.test.".to_string()],
            _ => vec![],
        }),
    )
}

async fn find(email: &str) -> Found {
    engine().await.find_provider(email).await
}

#[tokio::test]
async fn well_known_addresses_need_no_lookup() {
    let gmail = find("me@gmail.com").await;
    assert_eq!(gmail.method, Method::AppPassword);
    assert_eq!(gmail.imap_host, "imap.gmail.com");
    assert_eq!(gmail.smtp_host, "smtp.gmail.com");
    assert_eq!(gmail.source, "list");
    assert!(gmail.link.unwrap().starts_with("https://"));
    assert!(!gmail.steps.is_empty());

    let icloud = find("Me@ICLOUD.com").await;
    assert_eq!(icloud.method, Method::AppPassword);
    assert_eq!(
        icloud.usernames,
        ["Me", "Me@ICLOUD.com"],
        "Apple uses the short name first"
    );

    let outlook = find("me@hotmail.com").await;
    assert_eq!(
        outlook.method,
        Method::BrowserSoon,
        "Microsoft refuses passwords"
    );

    let proton = find("me@proton.me").await;
    assert_eq!(proton.method, Method::Bridge);
    assert_eq!(
        (proton.imap_host.as_str(), proton.imap_port),
        ("127.0.0.1", 1143)
    );

    assert_eq!(find("me@tuta.com").await.method, Method::Unsupported);
    assert_eq!(find("me@hey.com").await.method, Method::Unsupported);
}

#[tokio::test]
async fn own_domains_are_recognised_by_their_mail_records() {
    let workspace = find("me@company.test").await;
    assert_eq!(workspace.provider, "google");
    assert_eq!(workspace.name, "Google Workspace");
    assert_eq!(workspace.source, "mail_records");

    let m365 = find("me@acme.test").await;
    assert_eq!(m365.name, "Microsoft 365");
    assert_eq!(m365.method, Method::BrowserSoon);

    let zoho = find("me@zohoed.test").await;
    assert_eq!(
        zoho.imap_host, "imappro.zoho.eu",
        "companies use Zoho's other servers"
    );
    assert_eq!(zoho.smtp_host, "smtppro.zoho.eu");

    assert_eq!(find("me@migadued.test").await.imap_host, "imap.migadu.com");
}

#[tokio::test]
async fn the_public_directory_fills_in_the_rest() {
    let found = find("sam@listed.test").await;
    assert_eq!(found.method, Method::Password);
    assert_eq!(found.source, "directory");
    assert_eq!(found.name, "Example Mail");
    assert_eq!(found.imap_host, "imap.example.net");
    assert_eq!(found.smtp_host, "smtp.example.net");
    assert_eq!(found.smtp_port, 587);
    assert_eq!(found.usernames, ["sam", "sam@listed.test"]);

    // Looked up by the company that handles its mail.
    assert_eq!(find("sam@hosted.test").await.imap_host, "imap.example.net");
}

#[tokio::test]
async fn nothing_found_asks_and_bad_input_is_harmless() {
    for email in [
        "me@nowhere.test",
        "not an email",
        "me@",
        "me@../../etc",
        "me@a/b.test",
        "me@localhost",
    ] {
        let found = find(email).await;
        assert_eq!(found.method, Method::Manual, "{email}");
        assert_eq!(found.imap_host, "", "{email}");
    }
}

#[test]
fn settings_that_are_unsafe_are_refused() {
    let with = |host: &str, socket: &str| {
        format!(
            r#"<incomingServer type="imap"><hostname>{host}</hostname><port>993</port><socketType>{socket}</socketType><username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication></incomingServer>"#
        )
    };
    assert!(parse_settings(&with("imap.ok.test", "SSL"), "a@b.test").is_some());
    assert!(
        parse_settings(&with("imap.ok.test", "plain"), "a@b.test").is_none(),
        "no encryption"
    );
    assert!(
        parse_settings(&with("localhost", "SSL"), "a@b.test").is_none(),
        "this computer"
    );
    assert!(
        parse_settings(&with("192.168.1.1", "SSL"), "a@b.test").is_none(),
        "their network"
    );
    assert!(parse_settings("not xml at all", "a@b.test").is_none());

    let oauth_only = with("imap.ok.test", "SSL").replace("password-cleartext", "OAuth2");
    assert_eq!(
        parse_settings(&oauth_only, "a@b.test").unwrap().method,
        Method::BrowserSoon
    );
}
