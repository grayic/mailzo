//! Pictures of the people and companies who send mail.
//!
//! The sender is never contacted, so this can't tell them their mail was received
//! or opened. Pictures come from two public services, and each is asked once a
//! month at most, with the answer kept on this device:
//! - Gravatar, for people's photos. It's sent a scrambled form of the address.
//! - DuckDuckGo's icon service, for company logos. It's sent the company's domain.
//!
//! The person can turn this off, and then nothing is sent.

use crate::engine::Engine;
use crate::error::Result;
use data_encoding::{BASE64, HEXLOWER};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const PERSON_SOURCE: &str = "https://gravatar.com/avatar";
pub const LOGO_SOURCE: &str = "https://icons.duckduckgo.com/ip3";

const KEEP_FOUND_SECONDS: i64 = 30 * 86_400;
const KEEP_MISSING_SECONDS: i64 = 7 * 86_400;
const MAX_IMAGE_BYTES: usize = 256 * 1024;
/// Anything smaller looks blurred at the size pictures are shown.
const MIN_IMAGE_PIXELS: u32 = 32;
const TIMEOUT: Duration = Duration::from_secs(8);

/// Everyone at these uses the same provider, so its logo would say nothing about
/// who wrote to you.
pub(crate) const SHARED_PROVIDERS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "ymail.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "aol.com",
    "proton.me",
    "protonmail.com",
    "gmx.com",
    "gmx.net",
    "zoho.com",
    "fastmail.com",
    "hey.com",
    "yandex.com",
    "mail.com",
    "mail.ru",
    "qq.com",
    "163.com",
    "migadu.com",
];

/// Country endings made of two parts, where the company is the third from the end.
const TWO_PART_ENDINGS: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "com.au", "net.au", "co.nz", "co.jp", "co.in", "com.br", "com.bd",
    "net.bd", "org.bd", "com.sg", "com.my", "co.za", "com.mx", "com.tr", "com.cn",
];

/// A sender's address, tidied, or None if it isn't safe to use in a request.
fn clean_address(address: &str) -> Option<(String, String)> {
    let address = address.trim().to_ascii_lowercase();
    let (name, domain) = address.split_once('@')?;
    let ok_name = !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || c.is_control());
    (ok_name && clean_domain(domain)).then(|| (address.clone(), domain.to_string()))
}

fn clean_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && domain.len() <= 253
        // Letters in the ending rules out addresses such as 192.168.1.1.
        && labels.last().is_some_and(|l| l.chars().all(|c| c.is_ascii_alphabetic()))
        && labels.iter().all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// `email.openai.com` is `openai.com`, and `mail.shop.co.uk` is `shop.co.uk`.
pub(crate) fn company_domain(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').collect();
    let keep = if labels.len() >= 3
        && TWO_PART_ENDINGS.contains(&labels[labels.len() - 2..].join(".").as_str())
    {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

fn big_enough(width: u32, height: u32) -> bool {
    width.min(height) >= MIN_IMAGE_PIXELS
}

/// Decides from the bytes themselves, never from what the server says it sent. Only
/// plain picture formats pass, so nothing here can carry a script.
fn checked_image(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_IMAGE_BYTES || bytes.len() < 12 {
        return None;
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 24 {
            return None;
        }
        let side = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if !big_enough(side(16), side(20)) {
            return None;
        }
        "image/png"
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        // An icon file holds several sizes. It needs one that's large enough.
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let large = (0..count).any(|i| {
            let at = 6 + i * 16;
            let side = |b: Option<&u8>| match b {
                Some(0) => 256,
                Some(n) => u32::from(*n),
                None => 0,
            };
            big_enough(side(bytes.get(at)), side(bytes.get(at + 1)))
        });
        if !large {
            return None;
        }
        "image/x-icon"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    Some(format!("data:{mime};base64,{}", BASE64.encode(bytes)))
}

impl Engine {
    /// A picture for this sender, or null to show their initials.
    pub(crate) async fn avatar(&self, address: &str) -> Result<Value> {
        let nothing = json!({ "picture": null });
        if self.setting(crate::settings::SENDER_PICTURES)? != "on" {
            return Ok(nothing);
        }
        let Some((address, domain)) = clean_address(address) else {
            return Ok(nothing);
        };
        let hash = HEXLOWER.encode(&Sha256::digest(address.as_bytes()));
        let person_url = format!("{}/{hash}?s=128&d=404&r=g", self.avatar_sources.0);
        if let Some(picture) = self
            .lookup(&format!("person:{address}"), &person_url)
            .await?
        {
            return Ok(json!({ "picture": picture, "kind": "person" }));
        }
        if SHARED_PROVIDERS.contains(&domain.as_str()) {
            return Ok(nothing);
        }
        let company = company_domain(&domain);
        let logo_url = format!("{}/{company}.ico", self.avatar_sources.1);
        if let Some(picture) = self.lookup(&format!("logo:{company}"), &logo_url).await? {
            return Ok(json!({ "picture": picture, "kind": "logo" }));
        }
        Ok(nothing)
    }

    async fn lookup(&self, key: &str, url: &str) -> Result<Option<String>> {
        if let Some((data, fetched_at)) = self.store.avatar(key)? {
            let keep = if data.is_some() {
                KEEP_FOUND_SECONDS
            } else {
                KEEP_MISSING_SECONDS
            };
            if crate::identity::unix_now() - fetched_at < keep {
                return Ok(data);
            }
        }
        // A few at a time, so a long list doesn't send a burst of requests.
        let Ok(_turn) = self.avatar_turns.acquire().await else {
            return Ok(None);
        };
        // It may have been fetched for another row while this one waited.
        if let Some((data, fetched_at)) = self.store.avatar(key)? {
            if crate::identity::unix_now() - fetched_at < KEEP_MISSING_SECONDS {
                return Ok(data);
            }
        }
        match self.download_image(url).await {
            Some(found) => {
                self.store.save_avatar(key, found.as_deref())?;
                Ok(found)
            }
            // Offline or a server problem says nothing, so it's asked again later.
            None => Ok(None),
        }
    }

    /// Some(None) means the service answered that there's no picture, and None
    /// that it couldn't be asked.
    async fn download_image(&self, url: &str) -> Option<Option<String>> {
        let mut reply = tokio::time::timeout(TIMEOUT, self.avatar_http.get(url).send())
            .await
            .ok()?
            .ok()?;
        let status = reply.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Some(None);
        }
        if !status.is_success() {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = tokio::time::timeout(TIMEOUT, reply.chunk())
            .await
            .ok()?
            .ok()?
        {
            bytes.extend_from_slice(&chunk);
            // Too big is an answer: this service has nothing usable.
            if bytes.len() > MAX_IMAGE_BYTES {
                return Some(None);
            }
        }
        Some(checked_image(&bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_web_domains_are_used_in_requests() {
        assert!(clean_address("A@Openai.com").is_some());
        for bad in [
            "a@localhost",
            "a@192.168.1.1",
            "a@evil.com/../../x",
            "a@evil.com?x=1",
            "a@[::1]",
            "no-at-sign",
            "@openai.com",
            "a b@openai.com",
            "a@-bad.com",
        ] {
            assert!(clean_address(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_company_is_found_from_a_mail_subdomain() {
        assert_eq!(company_domain("email.openai.com"), "openai.com");
        assert_eq!(company_domain("openai.com"), "openai.com");
        assert_eq!(company_domain("mail.shop.co.uk"), "shop.co.uk");
        assert_eq!(company_domain("news.bank.com.bd"), "bank.com.bd");
    }

    #[test]
    fn only_plain_pictures_of_a_decent_size_pass() {
        let png = |w: u32, h: u32| {
            let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            b.extend_from_slice(&w.to_be_bytes());
            b.extend_from_slice(&h.to_be_bytes());
            b
        };
        assert!(checked_image(&png(64, 64))
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(checked_image(&png(16, 16)).is_none(), "too small");
        assert!(checked_image(
            b"<svg xmlns='http://www.w3.org/2000/svg'><script>x()</script></svg>"
        )
        .is_none());
        assert!(checked_image(b"<html><script>x()</script></html>").is_none());
        assert!(checked_image(&vec![0xffu8; MAX_IMAGE_BYTES + 1]).is_none());
        // An icon file: one 16 pixel size is not enough, and 48 is.
        let icon = |side: u8| {
            let mut b = vec![0, 0, 1, 0, 1, 0, side, side];
            b.extend_from_slice(&[0; 14]);
            b
        };
        assert!(checked_image(&icon(16)).is_none());
        assert!(checked_image(&icon(48))
            .unwrap()
            .starts_with("data:image/x-icon"));
    }
}
