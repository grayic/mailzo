//! Sends mail. Everything that speaks to the outgoing server lives here.

use crate::error::{CoreError, Result};
use crate::model::{Account, SmtpSecurity};
use crate::net::Reach;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::time::Duration;

const SMTP_TIMEOUT: Duration = Duration::from_secs(60);
/// The demo mailbox's outgoing server.
const LOCAL_PORT: u16 = 1025;

/// The outgoing server to try when one hasn't been set, best guess first.
pub fn candidates(imap_host: &str) -> Vec<(String, u16, SmtpSecurity)> {
    use SmtpSecurity::{Plain, StartTls, Tls};
    if matches!(imap_host, "localhost" | "127.0.0.1" | "::1") {
        return vec![(imap_host.to_string(), LOCAL_PORT, Plain)];
    }
    let known: Option<(&str, u16, SmtpSecurity)> = match imap_host {
        "imap.gmail.com" => Some(("smtp.gmail.com", 465, Tls)),
        "outlook.office365.com" | "imap-mail.outlook.com" => {
            Some(("smtp.office365.com", 587, StartTls))
        }
        "imap.mail.me.com" => Some(("smtp.mail.me.com", 587, StartTls)),
        "imap.mail.yahoo.com" => Some(("smtp.mail.yahoo.com", 465, Tls)),
        "imap.migadu.com" => Some(("smtp.migadu.com", 465, Tls)),
        "imap.fastmail.com" => Some(("smtp.fastmail.com", 465, Tls)),
        "imap.zoho.com" => Some(("smtp.zoho.com", 465, Tls)),
        _ => None,
    };
    if let Some((host, port, security)) = known {
        return vec![(host.to_string(), port, security)];
    }
    let mut hosts = Vec::new();
    if let Some(rest) = imap_host.strip_prefix("imap.") {
        hosts.push(format!("smtp.{rest}"));
        hosts.push(format!("mail.{rest}"));
    }
    if let Some(rest) = imap_host.strip_prefix("mail.") {
        hosts.push(format!("smtp.{rest}"));
    }
    hosts.push(imap_host.to_string());
    let mut out = Vec::new();
    for host in hosts {
        out.push((host.clone(), 465, Tls));
        out.push((host, 587, StartTls));
    }
    out
}

fn is_local(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn send_error(code: &str, message: impl Into<String>) -> CoreError {
    CoreError::Send {
        code: code.into(),
        message: message.into(),
    }
}

/// Turns the library's error into plain words. Nothing here includes a password.
fn explain(host: &str, e: lettre::transport::smtp::Error) -> CoreError {
    if let Some(code) = e.status() {
        let number = code.to_string();
        if matches!(number.as_str(), "530" | "534" | "535" | "538") {
            return send_error(
                "smtp_auth",
                "The outgoing mail server rejected this email address or password.",
            );
        }
        if e.is_permanent() {
            return send_error(
                "smtp_refused",
                format!("The mail server refused this message: {e}"),
            );
        }
        return send_error(
            "smtp_busy",
            format!("The mail server can't take this message right now. Try again shortly. ({e})"),
        );
    }
    send_error(
        "smtp_connect",
        format!(
            "Could not reach the outgoing mail server {host}. Check your internet connection and the outgoing server settings."
        ),
    )
}

/// With `Reach::PublicOnly`, the server's name is looked up once, refused if it leads
/// anywhere private, and the connection goes to exactly that address. The secure
/// connection still names the server and checks its certificate is that name's.
async fn transport(
    account: &Account,
    password: &str,
    host: &str,
    port: u16,
    security: SmtpSecurity,
    reach: Reach,
) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    if security == SmtpSecurity::Plain && !is_local(host) {
        return Err(CoreError::Invalid(
            "Unencrypted connections are only allowed to this computer.".into(),
        ));
    }
    let dial = tokio::time::timeout(SMTP_TIMEOUT, crate::net::server_to_dial(host, port, reach))
        .await
        .map_err(|_| send_error("smtp_connect", format!("Looking up {host} timed out.")))?
        .map_err(|e| send_error("smtp_connect", e.to_string()))?;
    let tls_for = |host: &str| {
        TlsParameters::builder(host.to_string())
            .build_rustls()
            .map_err(|e| CoreError::Network(e.to_string()))
    };
    let tls = match security {
        SmtpSecurity::Tls => Tls::Wrapper(tls_for(host)?),
        SmtpSecurity::StartTls => Tls::Required(tls_for(host)?),
        SmtpSecurity::Plain => Tls::None,
    };
    // Introduces itself by the mail domain, so the computer's name isn't revealed.
    let domain = account.email.rsplit('@').next().unwrap_or("localhost");
    Ok(
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(dial)
            .port(port)
            .tls(tls)
            .hello_name(ClientId::Domain(domain.to_string()))
            .credentials(Credentials::new(
                account.username.clone(),
                password.to_string(),
            ))
            .authentication(if account.auth == crate::signin::PASSWORD {
                vec![Mechanism::Plain, Mechanism::Login]
            } else {
                vec![Mechanism::Xoauth2]
            })
            .timeout(Some(SMTP_TIMEOUT))
            .build(),
    )
}

/// Connects and signs in, without sending anything.
pub async fn check(
    account: &Account,
    password: &str,
    host: &str,
    port: u16,
    security: SmtpSecurity,
    reach: Reach,
) -> Result<()> {
    transport(account, password, host, port, security, reach)
        .await?
        .test_connection()
        .await
        .map(|_| ())
        .map_err(|e| match explain(host, e) {
            // Nothing is being sent yet, so a flat refusal here is about the login.
            // iCloud, for one, answers a wrong address this way.
            refused if refused.code() == "smtp_refused" => send_error(
                "smtp_auth",
                "The outgoing mail server rejected this email address or password.",
            ),
            other => other,
        })
}

/// Finds a working outgoing server. A wrong password stops the search, because
/// trying more servers wouldn't help and would send the password to each.
pub async fn discover(
    account: &Account,
    password: &str,
    reach: Reach,
) -> Result<(String, u16, SmtpSecurity)> {
    let mut last = None;
    for (host, port, security) in candidates(&account.imap_host) {
        match check(account, password, &host, port, security, reach).await {
            Ok(()) => return Ok((host, port, security)),
            Err(e) if e.code() == "smtp_auth" => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        send_error(
            "smtp_connect",
            "Could not find this inbox's outgoing mail server. Add it in the inbox settings.",
        )
    }))
}

pub async fn send(
    account: &Account,
    password: &str,
    host: &str,
    port: u16,
    security: SmtpSecurity,
    message: Message,
    reach: Reach,
) -> Result<()> {
    transport(account, password, host, port, security, reach)
        .await?
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| explain(host, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_providers_are_recognised() {
        assert_eq!(
            candidates("imap.migadu.com"),
            [("smtp.migadu.com".to_string(), 465, SmtpSecurity::Tls)]
        );
        assert_eq!(
            candidates("outlook.office365.com")[0].1,
            587,
            "Microsoft uses port 587"
        );
    }

    #[test]
    fn unknown_providers_get_sensible_guesses() {
        let list = candidates("imap.example.com");
        assert_eq!(list[0].0, "smtp.example.com");
        assert_eq!(list[1].1, 587);
        assert!(list.iter().any(|c| c.0 == "imap.example.com"));
        assert!(list.iter().all(|c| c.2 != SmtpSecurity::Plain));
    }
}
