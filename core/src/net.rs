//! Telling public internet addresses from private ones, for a server that must never be
//! talked into reaching inside its own network.

use crate::error::{CoreError, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::TcpStream;

/// Addresses inside a private network or this machine, or anything else that isn't an
/// ordinary public address: reserved, shared, test and broadcast ranges, and IPv6 forms that
/// carry an IPv4 address inside them (checked as that IPv4 address).
pub fn private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => private_v6(v6),
    }
}

fn private_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_documentation()
        // "This network", 0.0.0.0/8.
        || o[0] == 0
        // Shared by internet providers behind their own address translation, 100.64.0.0/10.
        || (o[0] == 100 && (64..128).contains(&o[1]))
        // Protocol assignments, 192.0.0.0/24.
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        // The old 6to4 relay, 192.88.99.0/24.
        || (o[0] == 192 && o[1] == 88 && o[2] == 99)
        // Network testing, 198.18.0.0/15.
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        // Reserved for the future, 240.0.0.0/4, which takes in 255.255.255.255.
        || o[0] >= 240
}

fn private_v6(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    let low_v4 = || Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
    // An IPv4 address written as IPv6 (::ffff:a.b.c.d) or the old ::a.b.c.d form.
    if s[..5].iter().all(|&x| x == 0) && (s[5] == 0xffff || s[5] == 0) {
        return v6.is_loopback() || v6.is_unspecified() || private_v4(low_v4());
    }
    // An IPv4 address translated for IPv6-only networks, 64:ff9b::/96.
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return private_v4(low_v4());
    }
    // 6to4, 2002::/16, carries the IPv4 address it stands for right after the prefix.
    if s[0] == 0x2002 {
        let embedded = Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
        return private_v4(embedded);
    }
    // Teredo, 2001::/32, carries the far end's IPv4 address with every bit flipped.
    if s[0] == 0x2001 && s[1] == 0 {
        let flipped = Ipv4Addr::from(!(u32::from(s[6]) << 16 | u32::from(s[7])));
        return private_v4(flipped);
    }
    v6.is_multicast()
        // Unique local, fc00::/7.
        || (s[0] & 0xfe00) == 0xfc00
        // Link-local, fe80::/10, and the old site-local, fec0::/10.
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        // Translation for a network's own use, 64:ff9b:1::/48.
        || s[..3] == [0x64, 0xff9b, 1]
        // Thrown away, 100::/64.
        || s[..4] == [0x100, 0, 0, 0]
        // Documentation, 2001:db8::/32.
        || (s[0] == 0x2001 && s[1] == 0xdb8)
}

/// The host and port a web address really connects to, read the way the web library
/// reads it. Anything that could be read two ways is refused: a backslash (which the
/// library takes as a slash, so `https://127.0.0.1\@evil.example/` goes to 127.0.0.1),
/// a name and password before the host, or anything other than https.
pub fn https_host(url: &str) -> Option<(String, u16)> {
    web_host(url, false)
}

/// As `https_host`, and plain http too when `http_too`.
pub fn web_host(url: &str, http_too: bool) -> Option<(String, u16)> {
    if url.contains('\\') {
        return None;
    }
    let parsed = reqwest::Url::parse(url).ok()?;
    let scheme_ok = parsed.scheme() == "https" || (http_too && parsed.scheme() == "http");
    if !scheme_ok || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    // An IPv6 address comes in brackets, which looking it up doesn't want.
    let host = parsed.host_str()?.trim_matches(['[', ']']).to_string();
    Some((host, parsed.port_or_known_default()?))
}

/// The one public address to use for a web address's host, or nothing if any of its
/// addresses is private. Using the address checked here stops a name changing in between.
pub async fn public_address(url: &str) -> Option<SocketAddr> {
    public_web_address(url, false).await
}

/// As `public_address`, and plain http too when `http_too`.
pub async fn public_web_address(url: &str, http_too: bool) -> Option<SocketAddr> {
    let (name, port) = web_host(url, http_too)?;
    let found: Vec<_> = tokio::net::lookup_host((name.as_str(), port))
        .await
        .ok()?
        .collect();
    all_public(&found).then(|| found[0])
}

/// A web client for one address that reaches its host only at the public address checked
/// here, follows no redirects itself (each hop needs checking again) and keeps the name for
/// the secure connection and its certificate. Nothing if any address the name leads to is
/// private.
pub async fn public_pinned_client(
    url: &str,
    http_too: bool,
    timeout: std::time::Duration,
) -> Option<reqwest::Client> {
    let (host, _) = web_host(url, http_too)?;
    let addr = public_web_address(url, http_too).await?;
    let tls = crate::cloud::tls_config().ok()?;
    let mut b = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    if host.parse::<IpAddr>().is_err() {
        b = b.resolve(&host, addr);
    }
    b.build().ok()
}

/// Where the engine's own connections to mail and calendar servers may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Wherever the server's name leads, as on a person's own computer, where a server on
    /// their home network or this computer (such as a mail bridge) is theirs to use.
    Anywhere,
    /// Only public internet addresses, as on a shared web host, which must never be talked
    /// into reaching inside its own network.
    PublicOnly,
}

/// Whether a name led somewhere, and every address it led to is public.
pub fn all_public(found: &[SocketAddr]) -> bool {
    !found.is_empty() && found.iter().all(|a| !private_address(a.ip()))
}

/// Looks a server's name up once and gives the one address to connect to, or nothing if
/// the name didn't lead anywhere or any address it leads to is private. Connecting to
/// exactly this address stops the name changing between the check and the connection.
pub async fn public_server_address(host: &str, port: u16) -> Result<SocketAddr> {
    let name = host.trim_matches(['[', ']']);
    let found: Vec<SocketAddr> = tokio::net::lookup_host((name, port))
        .await
        .map_err(|_| CoreError::Network(format!("Could not find the server {host}.")))?
        .collect();
    if !all_public(&found) {
        return Err(not_public(host));
    }
    Ok(found[0])
}

fn not_public(host: &str) -> CoreError {
    CoreError::Invalid(format!(
        "{host} isn't on the public internet, so it can't be reached from here."
    ))
}

/// Opens a connection to a server. With `Reach::PublicOnly` the name is looked up once,
/// refused unless every address is public, and the connection goes to that checked address.
pub async fn connect_server(host: &str, port: u16, reach: Reach) -> Result<TcpStream> {
    let tcp = match reach {
        Reach::Anywhere => TcpStream::connect((host, port)).await,
        Reach::PublicOnly => TcpStream::connect(public_server_address(host, port).await?).await,
    };
    tcp.map_err(|e| CoreError::Network(e.to_string()))
}

/// The address a library that connects by itself should be given for a server: the name
/// itself, or with `Reach::PublicOnly` the one checked public address it leads to.
pub async fn server_to_dial(host: &str, port: u16, reach: Reach) -> Result<String> {
    match reach {
        Reach::Anywhere => Ok(host.to_string()),
        Reach::PublicOnly => Ok(public_server_address(host, port).await?.ip().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private(ip: &str) -> bool {
        private_address(ip.parse().unwrap())
    }

    #[test]
    fn private_and_local_addresses_are_spotted() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.5",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(private(ip), "{ip}");
        }
        for ip in ["142.250.80.109", "2a00:1450:4001::6d"] {
            assert!(!private(ip), "{ip}");
        }
    }

    #[test]
    fn shared_and_reserved_ipv4_ranges_are_spotted() {
        // Shared by internet providers, 100.64.0.0/10, and just either side of it.
        assert!(private("100.64.0.0"));
        assert!(private("100.127.255.255"));
        assert!(!private("100.63.255.255"));
        assert!(!private("100.128.0.0"));
        // Network testing, 198.18.0.0/15.
        assert!(private("198.18.0.1"));
        assert!(private("198.19.255.255"));
        assert!(!private("198.17.255.255"));
        assert!(!private("198.20.0.0"));
        // Protocol assignments, 192.0.0.0/24, but not the public 192.0.1.x.
        assert!(private("192.0.0.8"));
        assert!(!private("192.0.1.1"));
        // Documentation.
        assert!(private("192.0.2.1"));
        assert!(private("198.51.100.1"));
        assert!(private("203.0.113.1"));
        // The old 6to4 relay.
        assert!(private("192.88.99.1"));
        // Reserved, 240.0.0.0/4, and the broadcast address.
        assert!(private("240.0.0.1"));
        assert!(private("250.1.2.3"));
        assert!(private("255.255.255.255"));
        // Multicast, 224.0.0.0/4.
        assert!(private("224.0.0.1"));
        // This network, 0.0.0.0/8.
        assert!(private("0.1.2.3"));
    }

    #[test]
    fn ipv6_ranges_are_spotted() {
        // Unique local, fc00::/7, both halves.
        assert!(private("fc00::1"));
        assert!(private("fdff:ffff::1"));
        assert!(!private("fe00::1"));
        // Link-local, fe80::/10, and old site-local, fec0::/10.
        assert!(private("fe80::1"));
        assert!(private("febf::1"));
        assert!(private("fec0::1"));
        // Multicast.
        assert!(private("ff02::1"));
        assert!(private("ff05::2"));
        // Documentation and thrown-away.
        assert!(private("2001:db8::1"));
        assert!(private("100::1"));
        assert!(private("64:ff9b:1::1"));
        assert!(private("::"));
        assert!(private("::1"));
    }

    #[test]
    fn ipv4_inside_ipv6_is_checked_as_ipv4() {
        // IPv4-mapped.
        assert!(private("::ffff:10.0.0.1"));
        assert!(private("::ffff:169.254.169.254"));
        assert!(!private("::ffff:142.250.80.109"));
        // The old IPv4-compatible form.
        assert!(private("::127.0.0.1"));
        assert!(private("::10.0.0.1"));
        assert!(!private("::142.250.80.109"));
        // NAT64.
        assert!(private("64:ff9b::127.0.0.1"));
        assert!(private("64:ff9b::a9fe:a9fe"));
        assert!(!private("64:ff9b::142.250.80.109"));
        // 6to4: 2002:AABB:CCDD:: stands for AA.BB.CC.DD.
        assert!(private("2002:7f00:1::"));
        assert!(private("2002:c0a8:0101::1"));
        assert!(private("2002:a9fe:a9fe::"));
        assert!(!private("2002:8efa:506d::1"));
        // Teredo carries the address with every bit flipped: 127.0.0.1 is 80ff:fffe.
        assert!(private("2001:0:4136:e378:8000:63bf:80ff:fffe"));
        assert!(!private("2001:0:4136:e378:8000:63bf:7105:af92"));
    }

    #[tokio::test]
    async fn a_link_to_this_machine_has_no_public_address() {
        assert_eq!(public_address("https://127.0.0.1/u").await, None);
        assert_eq!(public_address("https://localhost:8443/u").await, None);
        assert_eq!(public_address("http://example.com/u").await, None);
        // Read by the web library as a path, so this one would really go to 127.0.0.1.
        assert_eq!(
            public_address("https://127.0.0.1\\@evil.example/").await,
            None
        );
        assert_eq!(public_address("https://me@example.com/u").await, None);
        // A name that isn't spelled "localhost" but leads there, found by looking it up.
        assert_eq!(public_web_address("http://localhost./u", true).await, None);
        // An IPv4 address written as IPv6.
        assert_eq!(
            public_web_address("http://[::ffff:127.0.0.1]/u", true).await,
            None
        );
        assert_eq!(
            public_web_address("http://[::ffff:7f00:1]:8080/u", true).await,
            None
        );
    }

    #[test]
    fn a_server_counts_as_public_only_if_every_address_is() {
        let at = |ip: &str| SocketAddr::new(ip.parse().unwrap(), 993);
        assert!(all_public(&[at("142.250.80.109")]));
        assert!(all_public(&[
            at("142.250.80.109"),
            at("2a00:1450:4001::6d")
        ]));
        // Nothing found isn't somewhere safe.
        assert!(!all_public(&[]));
        // One private address among public ones is enough to refuse.
        assert!(!all_public(&[at("142.250.80.109"), at("10.0.0.1")]));
        assert!(!all_public(&[at("::ffff:127.0.0.1")]));
    }

    #[tokio::test]
    async fn servers_on_this_machine_are_refused_when_only_public_ones_may_be_reached() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        for host in ["localhost", "localhost.", "127.0.0.1", "[::ffff:127.0.0.1]"] {
            let refused = connect_server(host, port, Reach::PublicOnly).await;
            assert!(refused.is_err(), "{host}");
            assert!(server_to_dial(host, port, Reach::PublicOnly).await.is_err());
        }
        // A name that doesn't lead anywhere is refused too.
        assert!(public_server_address("no-such-host.invalid", 993)
            .await
            .is_err());
        // Anywhere keeps connecting by name, as before.
        assert!(connect_server("127.0.0.1", port, Reach::Anywhere)
            .await
            .is_ok());
        assert_eq!(
            server_to_dial("localhost", port, Reach::Anywhere)
                .await
                .unwrap(),
            "localhost"
        );
    }

    #[test]
    fn the_host_checked_is_the_host_connected_to() {
        assert_eq!(
            https_host("https://example.com/u?x=1"),
            Some(("example.com".into(), 443))
        );
        assert_eq!(
            https_host("https://Example.com:8443/u"),
            Some(("example.com".into(), 8443))
        );
        assert_eq!(
            https_host("https://[::1]:9000/u"),
            Some(("::1".into(), 9000))
        );
        assert_eq!(https_host("https://127.0.0.1\\@evil.example/"), None);
        assert_eq!(https_host("https://evil.example@127.0.0.1/"), None);
        assert_eq!(https_host("https://a:b@example.com/"), None);
        assert_eq!(https_host("http://example.com/"), None);
    }
}
