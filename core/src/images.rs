//! Fetches the pictures in emails on behalf of the email frame. Some senders' servers tell browsers
//! not to show their pictures on other sites, which leaves holes in their own emails. Fetched here,
//! it's a plain download. No cookies go with it, and only pictures come back.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use std::net::{IpAddr, SocketAddr};

const MOST_BYTES: usize = 20 * 1024 * 1024;

/// A home router or this Mac isn't something an email gets to reach. This only reads the
/// name; the addresses it leads to are checked too, when connecting.
fn is_private_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(crate::net::private_address)
}

/// The most redirects followed for one picture.
const MOST_HOPS: usize = 5;

fn refused() -> CoreError {
    CoreError::Invalid("That picture can't be shown.".into())
}

/// A client that connects this host only to this one address, follows no redirects
/// itself, and sends no cookies.
fn pinned_client(host: &str, addr: SocketAddr) -> Option<reqwest::Client> {
    let tls = crate::cloud::tls_config().ok()?;
    let mut b = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(reqwest::redirect::Policy::none())
        // A proxy would look the name up again itself, past the check.
        .no_proxy()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(USER_AGENT);
    if host.parse::<IpAddr>().is_err() {
        b = b.resolve(host, addr);
    }
    b.build().ok()
}

/// Fetches a picture, connecting at every hop (the first and each redirect) only to an
/// address `allowed` approves, and to exactly the address that was checked, so a name
/// can't point somewhere else between the check and the connection.
async fn fetch_pinned(
    start: reqwest::Url,
    allowed: &(dyn Fn(SocketAddr) -> bool + Sync),
) -> Result<reqwest::Response> {
    let mut url = start;
    for _ in 0..=MOST_HOPS {
        let (host, port) = crate::net::web_host(url.as_str(), true).ok_or_else(refused)?;
        // Whatever the name says, it's where it leads that counts: "localhost.", a
        // ".local" printer, or a name the sender's own DNS points inside.
        let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| refused())?
            .collect();
        if found.is_empty() || !found.iter().all(|a| allowed(*a)) {
            return Err(refused());
        }
        let client = pinned_client(&host, found[0]).ok_or_else(refused)?;
        let reply = client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        if !reply.status().is_redirection() {
            return Ok(reply);
        }
        let next = reply
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|l| url.join(l).ok())
            .ok_or_else(refused)?;
        url = next;
    }
    Err(refused())
}

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";

/// The address carried in a request from the email frame.
pub fn image_address(path: &str) -> String {
    let bytes = path.trim_start_matches('/').as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (other, _) => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

impl Engine {
    pub async fn email_image(&self, address: &str) -> Result<(String, Vec<u8>)> {
        let url = reqwest::Url::parse(address).map_err(|_| refused())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(refused());
        }
        let reply = if self.images_may_be_local {
            // A test's stand-in picture server, on this machine.
            self.image_http
                .get(url)
                .send()
                .await
                .map_err(|e| CoreError::Network(e.to_string()))?
        } else {
            fetch_pinned(url, &|a| !crate::net::private_address(a.ip())).await?
        };
        read_picture(reply).await
    }
}

async fn read_picture(mut reply: reqwest::Response) -> Result<(String, Vec<u8>)> {
    let kind = reply
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !reply.status().is_success() || !kind.starts_with("image/") {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    while let Some(part) = reply
        .chunk()
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?
    {
        bytes.extend_from_slice(&part);
        if bytes.len() > MOST_BYTES {
            return Err(refused());
        }
    }
    Ok((kind, bytes))
}

/// Redirects count too, or an outside address could send this to a private one.
pub(crate) fn image_client() -> reqwest::Client {
    crate::cloud::tls_config()
        .ok()
        .and_then(|tls| {
            reqwest::Client::builder()
                .tls_backend_preconfigured(tls)
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    let private = is_private_host(attempt.url().host_str().unwrap_or("localhost"));
                    if attempt.previous().len() >= 5 || private {
                        attempt.stop()
                    } else {
                        attempt.follow()
                    }
                }))
                .timeout(std::time::Duration::from_secs(20))
                .user_agent(USER_AGENT)
                .build()
                .ok()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_places_are_refused() {
        for host in [
            "localhost",
            "127.0.0.1",
            "192.168.1.1",
            "10.0.0.2",
            "printer.local",
            "[::1]",
            "169.254.1.1",
            "[fe80::1]",
            "[::ffff:127.0.0.1]",
            "localhost.",
        ] {
            assert!(is_private_host(host), "{host}");
        }
        for host in ["claude.ai", "8.8.8.8", "example.com"] {
            assert!(!is_private_host(host), "{host}");
        }
    }

    /// A one-request web server on this machine that always gives this reply.
    async fn serve(reply: &'static str) -> SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        addr
    }

    fn url(addr: SocketAddr, path: &str) -> reqwest::Url {
        reqwest::Url::parse(&format!("http://{addr}{path}")).unwrap()
    }

    #[tokio::test]
    async fn private_addresses_are_refused_however_written() {
        let public_only = |a: SocketAddr| !crate::net::private_address(a.ip());
        for address in [
            "http://127.0.0.1/x.png",
            "http://[::ffff:127.0.0.1]/x.png",
            "http://[::ffff:a00:1]/x.png",
            "http://localhost./x.png",
            "http://10.0.0.1/x.png",
            "https://user@example.com/x.png",
            "ftp://example.com/x.png",
        ] {
            let url = reqwest::Url::parse(address).unwrap();
            assert!(fetch_pinned(url, &public_only).await.is_err(), "{address}");
        }
    }

    #[tokio::test]
    async fn every_redirect_is_checked_again() {
        let picture =
            serve("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 3\r\n\r\nPNG")
                .await;
        // Allowed to reach this one server only, as if it were the public one.
        let only = move |a: SocketAddr| a == picture;
        let reply = fetch_pinned(url(picture, "/x.png"), &only).await.unwrap();
        assert_eq!(read_picture(reply).await.unwrap().1, b"PNG");

        let to_private = serve(
            "HTTP/1.1 302 Found\r\nLocation: http://10.0.0.1/x.png\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let only = move |a: SocketAddr| a == to_private;
        assert!(fetch_pinned(url(to_private, "/x.png"), &only)
            .await
            .is_err());

        // A redirect to a name that leads to this machine is refused too.
        let to_local: &'static str = Box::leak(
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://localhost.:{}/x.png\r\nContent-Length: 0\r\n\r\n",
                picture.port()
            )
            .into_boxed_str(),
        );
        let hop = serve(to_local).await;
        let only = move |a: SocketAddr| a == hop;
        assert!(fetch_pinned(url(hop, "/x.png"), &only).await.is_err());
    }

    #[test]
    fn the_address_comes_back_whole() {
        assert_eq!(
            image_address("/https%3A%2F%2Fclaude.ai%2Fa%20b.png%3Fx%3D1"),
            "https://claude.ai/a b.png?x=1"
        );
        assert_eq!(image_address("/%zz"), "%zz");
    }
}
