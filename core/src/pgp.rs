//! Encrypted email with OpenPGP. Each inbox can have its own key, kept with the other
//! secrets and never in the database. Other people's public keys are kept in the
//! database, either imported by the person or learned from the Autocrypt header on
//! mail they send.
//!
//! Keys are made the way GnuPG, Thunderbird and Proton make them: an Ed25519 key that
//! signs, with a Curve25519 key under it for encryption, in the widely read v4 format.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::model::{Account, Body, PgpInfo};
use crate::parse;
use crate::store::Store;
use data_encoding::BASE64;
use mail_parser::MessageParser;
use pgp::composed::{
    ArmorOptions, DecryptionOptions, Deserializable, DetachedSignature, EncryptionCaps, KeyType,
    Message, MessageBuilder, SecretKeyParamsBuilder, SignedPublicKey, SignedPublicSubKey,
    SignedSecretKey, SubkeyParamsBuilder, TheRing, VerificationResult,
};
use pgp::crypto::{ecc_curve::ECCCurve, hash::HashAlgorithm, sym::SymmetricKeyAlgorithm};
use pgp::ser::Serialize as _;
use pgp::types::{CompressionAlgorithm, KeyDetails, Password, VerifyingKey};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::borrow::Cow;

/// The most an email's encrypted or signed part may unpack to.
const MOST_UNPACKED: u64 = 50 * 1024 * 1024;
const NOT_TO_YOU: &str = "This email is encrypted, but not to your key.";
const NO_OWN_KEY: &str = "You need your encryption key on this Mac to read this.";
const DAMAGED: &str = "This encrypted email couldn't be opened. It may be damaged.";
const NO_SENDER_KEY: &str =
    "This email is signed, but you don't have the sender's key to check the signature.";
const BAD_SIGNATURE: &str =
    "This email's signature doesn't match the sender's key, so it may have been changed on the way.";
const PARTLY_SIGNED: &str = "Only part of this email is signed. Text outside the signed part could have been added by anyone.";
const PARTLY_ENCRYPTED: &str = "Only part of this email is encrypted. Text outside the encrypted part could have been added by anyone.";
const KEY_CHANGED: &str = "This email is signed with a different key from the one their earlier email gave. Someone else may have sent it.";

/// Where an inbox's own private key is kept.
pub fn pgp_secret(account_id: i64) -> String {
    format!("pgp_secret:{account_id}")
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid(message.into())
}

// ---- keys ----

fn user_id(name: &str, email: &str) -> String {
    let name = name.replace(['<', '>', '\r', '\n'], " ");
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(email) {
        format!("<{email}>")
    } else {
        format!("{name} <{email}>")
    }
}

/// Makes a new key for this person, with no passphrase. The Keychain protects it.
pub fn generate_key(name: &str, email: &str) -> Result<SignedSecretKey> {
    let failed = |e: String| invalid(format!("The encryption key couldn't be made: {e}"));
    let mut encrypt = SubkeyParamsBuilder::default();
    encrypt
        .key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
        .can_encrypt(EncryptionCaps::All);
    let mut key = SecretKeyParamsBuilder::default();
    key.key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .primary_user_id(user_id(name, email))
        .preferred_symmetric_algorithms(
            vec![SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES128].into(),
        )
        .preferred_hash_algorithms(vec![HashAlgorithm::Sha256, HashAlgorithm::Sha512].into())
        .preferred_compression_algorithms(
            vec![CompressionAlgorithm::ZLIB, CompressionAlgorithm::ZIP].into(),
        )
        .subkeys(vec![encrypt.build().map_err(|e| failed(e.to_string()))?]);
    key.build()
        .map_err(|e| failed(e.to_string()))?
        .generate(rand::thread_rng())
        .map_err(|e| failed(e.to_string()))
}

pub fn armor_secret(key: &SignedSecretKey) -> Result<String> {
    key.to_armored_string(ArmorOptions::default())
        .map_err(|e| invalid(e.to_string()))
}

pub fn armor_public(key: &SignedPublicKey) -> Result<String> {
    key.to_armored_string(ArmorOptions::default())
        .map_err(|e| invalid(e.to_string()))
}

/// The key's fingerprint, as other apps show it.
pub fn fingerprint(key: &impl KeyDetails) -> String {
    format!("{:X}", key.fingerprint())
}

/// Reads a private key, unlocking it with the passphrase if it has one. What comes back
/// has no passphrase.
pub fn read_secret(armored: &str, passphrase: Option<&str>) -> Result<SignedSecretKey> {
    let (mut key, _) = SignedSecretKey::from_armor_single(armored.trim().as_bytes())
        .map_err(|_| {
            invalid("This doesn't look like a private key. Paste the whole key, including the BEGIN and END lines.")
        })?;
    let password: Password = passphrase.unwrap_or("").into();
    let locked = |_: pgp::errors::Error| {
        if passphrase.is_some_and(|p| !p.is_empty()) {
            invalid("That passphrase isn't right for this key.")
        } else {
            invalid("This key is protected by a passphrase. Enter it to import the key.")
        }
    };
    key.primary_key.remove_password(&password).map_err(locked)?;
    for sub in &mut key.secret_subkeys {
        sub.key.remove_password(&password).map_err(locked)?;
    }
    key.verify_bindings()
        .map_err(|_| invalid("This key is damaged, so it can't be used."))?;
    Ok(key)
}

/// Reads one or more public keys pasted together.
pub fn read_publics(armored: &str) -> Result<Vec<SignedPublicKey>> {
    let text = armored.trim();
    if text.contains("PRIVATE KEY BLOCK") {
        return Err(invalid(
            "This is a private key. To use it for one of your inboxes, import it in that inbox's settings.",
        ));
    }
    let mut out = Vec::new();
    // Several keys can be pasted one after another, each in its own block.
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN PGP PUBLIC KEY BLOCK-----") {
        let after = &rest[start..];
        let end_marker = "-----END PGP PUBLIC KEY BLOCK-----";
        let Some(end) = after.find(end_marker) else {
            break;
        };
        let block = &after[..end + end_marker.len()];
        if let Ok((keys, _)) = SignedPublicKey::from_armor_many(block.as_bytes()) {
            out.extend(
                keys.flatten()
                    .filter(|k: &SignedPublicKey| k.verify_bindings().is_ok()),
            );
        }
        rest = &after[end + end_marker.len()..];
    }
    Ok(out)
}

fn read_public(armored: &str) -> Option<SignedPublicKey> {
    read_publics(armored).ok()?.into_iter().next()
}

/// The email address in a user id such as "Ana <ana@example.com>".
fn email_of(user_id: &str) -> Option<String> {
    let address = match (user_id.rfind('<'), user_id.rfind('>')) {
        (Some(a), Some(b)) if a < b => &user_id[a + 1..b],
        _ => user_id,
    };
    let address = address.trim().to_lowercase();
    (address.contains('@') && !address.contains(char::is_whitespace)).then_some(address)
}

/// Every address the key is for.
pub fn key_emails(details: &pgp::composed::SignedKeyDetails) -> Vec<String> {
    let mut out: Vec<String> = details
        .users
        .iter()
        .filter_map(|u| u.id.as_str().and_then(email_of))
        .collect();
    out.dedup();
    out
}

enum EncryptTo<'a> {
    Primary(&'a SignedPublicKey),
    Sub(&'a SignedPublicSubKey),
}

/// Whether a key part is still in use: not withdrawn by its owner, and not past the end
/// date its newest self-signature gives it.
fn still_valid(created: u32, signatures: &[pgp::packet::Signature]) -> bool {
    use pgp::packet::SignatureType;
    if signatures.iter().any(|s| {
        matches!(
            s.typ(),
            Some(SignatureType::SubkeyRevocation | SignatureType::KeyRevocation)
        )
    }) {
        return false;
    }
    let newest = signatures
        .iter()
        .filter(|s| {
            !matches!(
                s.typ(),
                Some(SignatureType::SubkeyRevocation | SignatureType::KeyRevocation)
            )
        })
        .max_by_key(|s| s.created().map(|t| t.as_secs()).unwrap_or(0));
    match newest.and_then(|s| s.key_expiration_time()) {
        Some(d) if d.as_secs() > 0 => {
            i64::from(created) + i64::from(d.as_secs()) > crate::identity::unix_now()
        }
        _ => true,
    }
}

/// The whole key: not withdrawn, and not out of date.
fn key_valid(cert: &SignedPublicKey) -> bool {
    if !cert.details.revocation_signatures.is_empty() {
        return false;
    }
    let self_signatures: Vec<pgp::packet::Signature> = cert
        .details
        .users
        .iter()
        .flat_map(|u| u.signatures.iter().cloned())
        .chain(cert.details.direct_signatures.iter().cloned())
        .collect();
    still_valid(cert.primary_key.created_at().as_secs(), &self_signatures)
}

/// The part of someone's key that mail is encrypted to. Withdrawn or out of date parts
/// are never used.
fn encryption_key(cert: &SignedPublicKey) -> Option<EncryptTo<'_>> {
    if !key_valid(cert) {
        return None;
    }
    let usable: Vec<&SignedPublicSubKey> = cert
        .public_subkeys
        .iter()
        .filter(|s| {
            s.key.algorithm().can_encrypt()
                && still_valid(s.key.created_at().as_secs(), &s.signatures)
        })
        .collect();
    let marked = usable.iter().find(|s| {
        s.signatures.iter().any(|sig| {
            let flags = sig.key_flags();
            flags.encrypt_comms() || flags.encrypt_storage()
        })
    });
    if let Some(sub) = marked.or(usable.first()) {
        return Some(EncryptTo::Sub(sub));
    }
    cert.primary_key
        .algorithm()
        .can_encrypt()
        .then_some(EncryptTo::Primary(cert))
}

/// The parts of someone's key that may have made a signature: the key isn't withdrawn or
/// out of date, and the part itself is in use and marked for signing. A withdrawn or
/// expired signing subkey, or one only meant for encryption, never checks out.
fn signing_parts(cert: &SignedPublicKey) -> SigningParts<'_> {
    let mut out = SigningParts {
        primary: None,
        subs: Vec::new(),
    };
    if !key_valid(cert) {
        return out;
    }
    let marks = |sig: &pgp::packet::Signature| {
        let f = sig.key_flags();
        let any = f.certify() || f.sign() || f.encrypt_comms() || f.encrypt_storage();
        (any, f.sign())
    };
    // The main key signs unless its own self-signatures mark it for other things only.
    let primary: Vec<(bool, bool)> = cert
        .details
        .users
        .iter()
        .flat_map(|u| u.signatures.iter())
        .chain(cert.details.direct_signatures.iter())
        .map(marks)
        .collect();
    let primary_signs =
        primary.iter().any(|(_, sign)| *sign) || primary.iter().all(|(any, _)| !*any);
    if primary_signs && cert.primary_key.algorithm().can_sign() {
        out.primary = Some(&cert.primary_key);
    }
    for sub in &cert.public_subkeys {
        if sub.key.algorithm().can_sign()
            && still_valid(sub.key.created_at().as_secs(), &sub.signatures)
            && sub.signatures.iter().any(|sig| sig.key_flags().sign())
        {
            out.subs.push(sub);
        }
    }
    out
}

struct SigningParts<'a> {
    primary: Option<&'a pgp::packet::PublicKey>,
    subs: Vec<&'a SignedPublicSubKey>,
}

impl SigningParts<'_> {
    fn all(&self) -> Vec<&dyn VerifyingKey> {
        let mut out: Vec<&dyn VerifyingKey> = Vec::new();
        if let Some(p) = self.primary {
            out.push(p);
        }
        for s in &self.subs {
            out.push(*s);
        }
        out
    }
}

pub fn can_encrypt_to(cert: &SignedPublicKey) -> bool {
    encryption_key(cert).is_some()
}

/// Signs with the main key when it may sign, otherwise with a signing subkey.
fn with_signing_key<T>(
    key: &SignedSecretKey,
    work: impl FnOnce(&dyn pgp::types::SigningKey) -> Result<T>,
) -> Result<T> {
    let primary_signs = key
        .details
        .users
        .iter()
        .flat_map(|u| u.signatures.iter())
        .any(|s| s.key_flags().sign());
    let sub = key.secret_subkeys.iter().find(|s| {
        s.key.algorithm().can_sign() && s.signatures.iter().any(|sig| sig.key_flags().sign())
    });
    match sub {
        Some(sub) if !primary_signs => work(&sub.key),
        _ if key.primary_key.algorithm().can_sign() => work(&key.primary_key),
        _ => Err(invalid(
            "This inbox's encryption key can't sign email. Import a key that can.",
        )),
    }
}

// ---- sending ----

/// Encrypts a whole MIME part to these keys and signs it, giving the armored message.
pub fn encrypt_entity(
    entity: &[u8],
    recipients: &[SignedPublicKey],
    signer: &SignedSecretKey,
) -> Result<String> {
    let failed = |e: pgp::errors::Error| CoreError::Send {
        code: "pgp_failed".into(),
        message: format!("This message couldn't be encrypted: {e}"),
    };
    let mut rng = rand::thread_rng();
    with_signing_key(signer, |key| {
        let mut builder = MessageBuilder::from_bytes("", entity.to_vec())
            .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
        for cert in recipients {
            match encryption_key(cert) {
                Some(EncryptTo::Sub(sub)) => builder.encrypt_to_key(&mut rng, sub),
                Some(EncryptTo::Primary(p)) => builder.encrypt_to_key(&mut rng, p),
                None => continue,
            }
            .map_err(failed)?;
        }
        builder.sign(key, Password::empty(), HashAlgorithm::Sha256);
        builder
            .to_armored_string(&mut rng, ArmorOptions::default())
            .map_err(failed)
    })
}

/// A detached signature over exactly these bytes.
pub fn sign_detached(content: &[u8], signer: &SignedSecretKey) -> Result<String> {
    let failed = |e: pgp::errors::Error| CoreError::Send {
        code: "pgp_failed".into(),
        message: format!("This message couldn't be signed: {e}"),
    };
    with_signing_key(signer, |key| {
        let key = Box::new(key);
        DetachedSignature::sign_binary_data(
            rand::thread_rng(),
            &key,
            &Password::empty(),
            HashAlgorithm::Sha256,
            content,
        )
        .map_err(failed)?
        .to_armored_string(ArmorOptions::default())
        .map_err(failed)
    })
}

/// The Autocrypt header's value, already folded to fit mail's line length.
pub fn autocrypt_value(address: &str, key: &SignedSecretKey) -> Option<String> {
    let bytes = key.to_public_key().to_bytes().ok()?;
    let data = BASE64.encode(&bytes);
    let mut out = format!("addr={address}; keydata=");
    for chunk in data.as_bytes().chunks(76) {
        out.push_str("\r\n ");
        out.push_str(std::str::from_utf8(chunk).ok()?);
    }
    Some(out)
}

// ---- reading ----

/// What's needed to open and check mail for one inbox: its own key, and other
/// people's keys from the database.
pub struct Ring<'a> {
    pub own: Option<SignedSecretKey>,
    pub store: &'a Store,
}

/// How far a key that checked a signature can be trusted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trust {
    /// The person's own key, or one they added.
    Full,
    /// Only learned from the sender's earlier email.
    Unconfirmed,
    /// A different key a later email offered, not accepted.
    Changed,
}

impl Ring<'_> {
    /// The sender's saved key, how it was saved, and any different key a later email offered.
    fn sender_keys(&self, from: &str) -> Option<(SignedPublicKey, Trust, Option<SignedPublicKey>)> {
        let (armored, source, _) = self.store.pgp_key(from).ok()??;
        let key = read_public(&armored)?;
        let trust = if source == "imported" {
            Trust::Full
        } else {
            Trust::Unconfirmed
        };
        let offered = self
            .store
            .pgp_offered_key(from)
            .ok()
            .flatten()
            .and_then(|(armored, _)| read_public(&armored));
        Some((key, trust, offered))
    }

    /// Keys a signature on mail from this address may be checked against, with whose
    /// each is and how far it's trusted.
    fn verifiers(&self, from: &str) -> Vec<(String, SignedPublicKey, Trust)> {
        let mut out = Vec::new();
        if let Some((key, trust, offered)) = self.sender_keys(from) {
            out.push((from.to_string(), key, trust));
            if let Some(offered) = offered {
                out.push((from.to_string(), offered, Trust::Changed));
            }
        }
        if let Some(own) = &self.own {
            let public = own.to_public_key();
            let address = key_emails(&public.details)
                .into_iter()
                .next()
                .unwrap_or_default();
            out.push((address, public, Trust::Full));
        }
        out
    }

    fn knows(&self, from: &str) -> bool {
        self.store.pgp_key(from).ok().flatten().is_some()
            || self
                .own
                .as_ref()
                .is_some_and(|k| key_emails(&k.details).iter().any(|a| a == from))
    }

    /// What a signature (or none) comes to, for showing.
    fn info(
        &self,
        encrypted: bool,
        signed: bool,
        found: Option<(String, Trust)>,
        from: &str,
    ) -> PgpInfo {
        let trust = found.as_ref().map(|(_, t)| *t);
        let problem = match trust {
            Some(Trust::Changed) => Some(KEY_CHANGED.to_string()),
            Some(_) => None,
            None if !signed => None,
            None if self.knows(from) => Some(BAD_SIGNATURE.to_string()),
            None => Some(NO_SENDER_KEY.to_string()),
        };
        PgpInfo {
            encrypted,
            signed,
            verified: trust == Some(Trust::Full),
            signer: found.map(|(a, _)| a),
            problem,
            signer_unconfirmed: trust == Some(Trust::Unconfirmed),
            key_changed: trust == Some(Trust::Changed),
            partial: false,
        }
    }
}

struct Opened {
    data: Vec<u8>,
    encrypted: bool,
    signed: bool,
    signer: Option<(String, Trust)>,
}

/// Decrypts and checks one OpenPGP message, armored or not.
fn open_message(
    data: &[u8],
    ring: &Ring<'_>,
    from: &str,
) -> std::result::Result<Opened, &'static str> {
    let parsed = if data.trim_ascii_start().starts_with(b"-----BEGIN") {
        Message::from_armor(data).map(|(m, _)| m)
    } else {
        Message::from_bytes(data)
    };
    let message = parsed.map_err(|_| DAMAGED)?;
    let encrypted = message.is_encrypted();
    let mut message = if encrypted {
        let Some(own) = ring.own.as_ref() else {
            return Err(NO_OWN_KEY);
        };
        let password = Password::empty();
        let keys = TheRing {
            secret_keys: vec![own],
            key_passwords: vec![&password],
            decrypt_options: DecryptionOptions::new().enable_gnupg_aead(),
            ..Default::default()
        };
        match message.decrypt_the_ring(keys, true) {
            Ok((m, _)) => m,
            Err(pgp::errors::Error::MissingKey) => return Err(NOT_TO_YOU),
            Err(_) => return Err(DAMAGED),
        }
    } else {
        message
    };
    for _ in 0..3 {
        if !message.is_compressed() {
            break;
        }
        message = message.decompress().map_err(|_| DAMAGED)?;
    }
    let signed = message.is_signed();
    // A few kilobytes can unpack to gigabytes, so stop well before that.
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(&mut message, MOST_UNPACKED + 1),
        &mut data,
    )
    .map_err(|_| DAMAGED)?;
    if data.len() as u64 > MOST_UNPACKED {
        return Err(DAMAGED);
    }
    let mut signer = None;
    if signed {
        let verifiers = ring.verifiers(from);
        let mut owners: Vec<(&str, Trust)> = Vec::new();
        let mut keys: Vec<&dyn VerifyingKey> = Vec::new();
        let parts: Vec<_> = verifiers
            .iter()
            .map(|(address, cert, trust)| (address, *trust, signing_parts(cert)))
            .collect();
        for (address, trust, part) in &parts {
            for key in part.all() {
                keys.push(key);
                owners.push((address, *trust));
            }
        }
        if let Ok(results) = message.verify_nested(&keys) {
            signer = results
                .iter()
                .position(|r| matches!(r, VerificationResult::Valid(_)))
                .map(|i| (owners[i].0.to_string(), owners[i].1));
        }
    }
    Ok(Opened {
        data,
        encrypted,
        signed,
        signer,
    })
}

/// Which of these keys made this detached signature over the content.
fn check_detached(
    signature: &[u8],
    content: &[u8],
    verifiers: &[(String, SignedPublicKey, Trust)],
) -> Option<(String, Trust)> {
    let (sig, _) = DetachedSignature::from_armor_single(signature).ok()?;
    for (address, cert, trust) in verifiers {
        let parts = signing_parts(cert);
        if parts
            .primary
            .is_some_and(|p| sig.verify(p, content).is_ok())
            || parts
                .subs
                .iter()
                .any(|sub| sig.verify(*sub, content).is_ok())
        {
            return Some((address.clone(), *trust));
        }
    }
    None
}

/// A message's header and body, split at the first empty line.
fn split_head(raw: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0;
    while i < raw.len() {
        let Some(end) = raw[i..].iter().position(|b| *b == b'\n').map(|p| i + p) else {
            break;
        };
        let line = &raw[i..end];
        if line.strip_suffix(b"\r").unwrap_or(line).is_empty() {
            return (&raw[..i], &raw[end + 1..]);
        }
        i = end + 1;
    }
    (raw, &[])
}

/// The parts of a multipart body exactly as they are. The line break before each
/// boundary belongs to the boundary, so it isn't part of the content.
fn multipart_parts<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i < body.len() {
        let end = body[i..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(body.len(), |p| i + p);
        let line = &body[i..end];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(rest) = line.strip_prefix(delimiter.as_bytes()) {
            let closing = rest.starts_with(b"--");
            let rest = if closing { &rest[2..] } else { rest };
            if rest.iter().all(|b| *b == b' ' || *b == b'\t') {
                if let Some(s) = start {
                    let mut stop = i;
                    if stop > s && body[stop - 1] == b'\n' {
                        stop -= 1;
                        if stop > s && body[stop - 1] == b'\r' {
                            stop -= 1;
                        }
                    }
                    parts.push(&body[s..stop]);
                }
                if closing {
                    return parts;
                }
                start = Some((end + 1).min(body.len()));
            }
        }
        i = end + 1;
    }
    parts
}

/// Line breaks as signatures expect them: CR LF.
fn crlf(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 40);
    let mut previous = 0u8;
    for &b in data {
        if b == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        previous = b;
    }
    out
}

/// The armored block of this kind in a part, decoding the part first if it was sent encoded.
fn armor_block(part: &[u8], kind: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let find = |data: &[u8]| -> Option<Vec<u8>> {
        let text = String::from_utf8_lossy(data);
        let a = text.find(&begin)?;
        let b = text[a..].find(&end)? + a + end.len();
        Some(format!("{}\n", &text[a..b]).into_bytes())
    };
    find(part).or_else(|| {
        let m = MessageParser::default().parse(part)?;
        let decoded = m.root_part().decoded();
        find(&decoded)
    })
}

enum Readable {
    /// The MIME part inside, to show as the email.
    Inner(Vec<u8>, PgpInfo),
    /// Encrypted, and it can't be opened here.
    Locked(PgpInfo),
}

fn locked(problem: &str) -> Readable {
    Readable::Locked(PgpInfo {
        encrypted: true,
        problem: Some(problem.to_string()),
        ..Default::default()
    })
}

/// Opens PGP/MIME mail (RFC 3156). None means it isn't PGP/MIME.
fn open_mime(raw: &[u8], ring: &Ring<'_>, from: &str, depth: u8) -> Option<Readable> {
    let headers = MessageParser::default().parse_headers(raw)?;
    let ct = headers.content_type()?;
    if !ct.ctype().eq_ignore_ascii_case("multipart") {
        return None;
    }
    let subtype = ct.subtype()?.to_ascii_lowercase();
    let protocol = ct.attribute("protocol").unwrap_or("").to_ascii_lowercase();
    let boundary = ct.attribute("boundary")?;
    let (_, body) = split_head(raw);
    let parts = multipart_parts(body, boundary);
    match (subtype.as_str(), protocol.as_str()) {
        ("encrypted", "application/pgp-encrypted") => {
            let Some(armored) = parts.get(1).and_then(|p| armor_block(p, "PGP MESSAGE")) else {
                return Some(locked(DAMAGED));
            };
            let opened = match open_message(&armored, ring, from) {
                Ok(o) => o,
                Err(problem) => return Some(locked(problem)),
            };
            let mut info = ring.info(true, opened.signed, opened.signer, from);
            let inner = opened.data;
            // Some apps sign first, as a part of its own, and then encrypt that.
            if depth == 0 {
                if let Some(Readable::Inner(deeper, signed)) = open_mime(&inner, ring, from, 1) {
                    if signed.signed && info.signer.is_none() {
                        info = PgpInfo {
                            encrypted: true,
                            ..signed
                        };
                    }
                    return Some(Readable::Inner(deeper, info));
                }
            }
            Some(Readable::Inner(inner, info))
        }
        ("signed", "application/pgp-signature") => {
            let content = *parts.first()?;
            let signer = parts
                .get(1)
                .and_then(|p| armor_block(p, "PGP SIGNATURE"))
                .and_then(|sig| check_detached(&sig, &crlf(content), &ring.verifiers(from)));
            Some(Readable::Inner(
                content.to_vec(),
                ring.info(false, true, signer, from),
            ))
        }
        _ => None,
    }
}

/// Mail that has an OpenPGP message pasted into its text, as some apps send it.
fn open_inline(mut body: Body, ring: &Ring<'_>, from: &str) -> Body {
    let Some(text) = body.text.clone() else {
        return body;
    };
    let begin = "-----BEGIN PGP MESSAGE-----";
    let end = "-----END PGP MESSAGE-----";
    let Some(a) = text.find(begin) else {
        return body;
    };
    let Some(b) = text[a..].find(end).map(|b| a + b + end.len()) else {
        return body;
    };
    match open_message(format!("{}\n", &text[a..b]).as_bytes(), ring, from) {
        Ok(opened) => {
            let plain = String::from_utf8_lossy(&opened.data).replace("\r\n", "\n");
            let (before, after) = (text[..a].trim(), text[b..].trim());
            let mut info = ring.info(opened.encrypted, opened.signed, opened.signer, from);
            if before.is_empty() && after.is_empty() {
                body.text = Some(plain);
            } else {
                // Text around the block isn't covered by its signature or encryption, so
                // it's kept apart and the email isn't shown as signed as a whole.
                info.partial = true;
                info.verified = false;
                let what = if opened.signed { "signed" } else { "encrypted" };
                info.problem = Some(if opened.signed {
                    PARTLY_SIGNED.to_string()
                } else {
                    PARTLY_ENCRYPTED.to_string()
                });
                let outside: Vec<&str> = [before, after]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect();
                body.text = Some(format!(
                    "{}\n\n----- Not {what}: added outside the {what} part -----\n\n{}",
                    plain.trim_end(),
                    outside.join("\n\n")
                ));
            }
            // Any HTML would only show the scrambled text.
            body.html = None;
            body.pgp = Some(info);
        }
        Err(problem) => {
            body.pgp = Some(PgpInfo {
                encrypted: true,
                problem: Some(problem.to_string()),
                ..Default::default()
            });
        }
    }
    body
}

fn sender(raw: &[u8]) -> String {
    parse::parse_headers(raw)
        .from
        .map(|a| a.address.to_lowercase())
        .unwrap_or_default()
}

/// The email to show, decrypted and checked where it's encrypted or signed.
pub fn read_body(raw: &[u8], ring: &Ring<'_>) -> Body {
    let from = sender(raw);
    match open_mime(raw, ring, &from, 0) {
        Some(Readable::Inner(inner, info)) => {
            let mut body = parse::parse_body(&inner);
            body.pgp = Some(info);
            body
        }
        Some(Readable::Locked(info)) => Body {
            text: None,
            html: None,
            attachments: vec![],
            pgp: Some(info),
        },
        None => open_inline(parse::parse_body(raw), ring, &from),
    }
}

/// The message as its attachments are counted: the part inside, for PGP/MIME mail.
pub fn readable_raw<'r>(raw: &'r [u8], ring: &Ring<'_>) -> Cow<'r, [u8]> {
    match open_mime(raw, ring, &sender(raw), 0) {
        Some(Readable::Inner(inner, _)) => Cow::Owned(inner),
        _ => Cow::Borrowed(raw),
    }
}

// ---- Autocrypt ----

/// Every value of one header, with folded lines joined.
fn header_values(raw_header: &[u8], name: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(raw_header);
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with([' ', '\t']) {
            if let Some(c) = current.as_mut() {
                c.push(' ');
                c.push_str(line.trim());
            }
            continue;
        }
        if let Some(done) = current.take() {
            out.push(done);
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case(name) {
                current = Some(value.trim().to_string());
            }
        }
    }
    out.extend(current);
    out
}

/// A key someone's mail app offers in its Autocrypt header. Only taken when it's for the
/// address the email is from.
fn autocrypt_offer(raw_header: &[u8], from: &str) -> Option<(SignedPublicKey, bool)> {
    let values = header_values(raw_header, "autocrypt");
    // More than one means it can't be trusted to say which.
    let [value] = values.as_slice() else {
        return None;
    };
    let mut address = None;
    let mut keydata = None;
    let mut mutual = false;
    for attribute in value.split(';') {
        let Some((key, val)) = attribute.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        match key.as_str() {
            "addr" => address = Some(val.to_lowercase()),
            "keydata" => keydata = Some(val.to_string()),
            "prefer-encrypt" => mutual = val.eq_ignore_ascii_case("mutual"),
            // Anything else it doesn't understand means it should be ignored.
            other if !other.starts_with('_') => return None,
            _ => {}
        }
    }
    let address = address?;
    if address != from {
        return None;
    }
    let data: String = keydata?.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = BASE64.decode(data.as_bytes()).ok()?;
    let key = SignedPublicKey::from_bytes(&bytes[..]).ok()?;
    key.verify_bindings().ok()?;
    if !key_emails(&key.details).contains(&address) || !can_encrypt_to(&key) {
        return None;
    }
    Some((key, mutual))
}

/// Learns the sender's key from an incoming email's header, if it offers one.
pub(crate) fn learn_from_header(store: &Store, raw_header: &[u8]) -> Result<()> {
    if !raw_header
        .windows(10)
        .any(|w| w.eq_ignore_ascii_case(b"autocrypt:"))
    {
        return Ok(());
    }
    let headers = parse::parse_headers(raw_header);
    let Some(from) = headers.from.map(|a| a.address.to_lowercase()) else {
        return Ok(());
    };
    if store.is_own_address(&from)? {
        return Ok(());
    }
    // Anyone can write any From address and Autocrypt header, so only mail the person's
    // own provider confirmed really comes from that address's domain counts.
    if !crate::spam::sender_confirmed(raw_header, &from) {
        return Ok(());
    }
    let Some((key, mutual)) = autocrypt_offer(raw_header, &from) else {
        return Ok(());
    };
    // A date in the future would keep its key in place over anything that comes later.
    let now = crate::identity::unix_now();
    let seen_at = headers.date.unwrap_or(now).min(now);
    let print = fingerprint(&key);
    if let Some((_, source, saved_at)) = store.pgp_key(&from)? {
        // A key the person chose stays. An older email doesn't change anything.
        if source == "imported" || saved_at > seen_at {
            return Ok(());
        }
        let kept = store.pgp_fingerprint(&from)?;
        if kept.as_deref() != Some(print.as_str()) {
            // Never put in place of the saved key on an email's word: kept beside it,
            // and the change is shown, until the person accepts it.
            return store.pgp_offer_key(&from, &armor_public(&key)?, &print);
        }
    }
    store.pgp_save_key(
        &from,
        &armor_public(&key)?,
        &print,
        "autocrypt",
        mutual,
        seen_at,
    )
}

// ---- storage ----

impl Store {
    /// Someone's saved key: armored, where it came from, and when it was last seen.
    pub fn pgp_key(&self, address: &str) -> Result<Option<(String, String, i64)>> {
        Ok(self
            .db()
            .query_row(
                "SELECT armored, source, seen_at FROM pgp_keys WHERE address = ?1",
                [address.to_lowercase()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    pub fn pgp_save_key(
        &self,
        address: &str,
        armored: &str,
        fingerprint: &str,
        source: &str,
        prefer_encrypt: bool,
        seen_at: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO pgp_keys (address, armored, fingerprint, source, prefer_encrypt, seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                address.to_lowercase(),
                armored,
                fingerprint,
                source,
                prefer_encrypt,
                seen_at
            ],
        )?;
        Ok(())
    }

    fn pgp_fingerprint(&self, address: &str) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "SELECT fingerprint FROM pgp_keys WHERE address = ?1",
                [address.to_lowercase()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// A different key a later email offered for this address: armored, and its fingerprint.
    pub fn pgp_offered_key(&self, address: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .db()
            .query_row(
                "SELECT offered_armored, offered_fingerprint FROM pgp_keys
                  WHERE address = ?1 AND offered_armored IS NOT NULL",
                [address.to_lowercase()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    fn pgp_offer_key(&self, address: &str, armored: &str, fingerprint: &str) -> Result<()> {
        self.db().execute(
            "UPDATE pgp_keys SET offered_armored = ?2, offered_fingerprint = ?3 WHERE address = ?1",
            params![address.to_lowercase(), armored, fingerprint],
        )?;
        Ok(())
    }

    /// Someone's keys. A key a later email offered in place of the saved one shows as
    /// "changed", with the kept key's fingerprint as "old_fingerprint".
    pub fn pgp_keys_list(&self) -> Result<Vec<Value>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT address, fingerprint, source, offered_fingerprint FROM pgp_keys ORDER BY address",
        )?;
        let rows = st.query_map([], |r| {
            let offered: Option<String> = r.get(3)?;
            let fingerprint: String = r.get(1)?;
            let mut row = json!({
                "address": r.get::<_, String>(0)?,
                "fingerprint": fingerprint,
                "source": r.get::<_, String>(2)?,
                "changed": offered.is_some(),
            });
            if let Some(offered) = offered {
                row["old_fingerprint"] = json!(fingerprint);
                row["new_fingerprint"] = json!(offered);
            }
            Ok(row)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn pgp_delete_key(&self, address: &str) -> Result<bool> {
        Ok(self.db().execute(
            "DELETE FROM pgp_keys WHERE address = ?1",
            [address.trim().to_lowercase()],
        )? > 0)
    }

    /// Forgets saved encrypted or signed bodies, so they're read again with the keys as they
    /// are now. None means every inbox's.
    pub fn pgp_forget_bodies(&self, account_id: Option<i64>) -> Result<()> {
        self.db().execute(
            "DELETE FROM bodies WHERE pgp_json IS NOT NULL
               AND (?1 IS NULL OR message_id IN (SELECT id FROM messages WHERE account_id = ?1))",
            [account_id],
        )?;
        Ok(())
    }

    fn is_own_address(&self, address: &str) -> Result<bool> {
        let address = address.to_lowercase();
        Ok(self
            .list_accounts()?
            .iter()
            .any(|a| a.email.eq_ignore_ascii_case(&address)))
    }
}

// ---- commands ----

/// Every address this inbox sends from.
fn own_addresses(account: &Account) -> Vec<String> {
    let mut out = vec![account.email.to_lowercase()];
    out.extend(
        account
            .prefs
            .aliases
            .iter()
            .map(|a| a.address.to_lowercase()),
    );
    out
}

impl Engine {
    /// This inbox's own key, if it has one.
    pub(crate) fn pgp_own_key(&self, account_id: i64) -> Result<Option<SignedSecretKey>> {
        let Some(armored) = self.secrets.get(&pgp_secret(account_id))? else {
            return Ok(None);
        };
        Ok(SignedSecretKey::from_armor_single(armored.as_bytes())
            .ok()
            .map(|(k, _)| k))
    }

    /// The keys for reading this inbox's mail. A Keychain problem means reading without
    /// its own key, not failing.
    pub(crate) fn pgp_ring(&self, account_id: i64) -> Ring<'_> {
        Ring {
            own: self.pgp_own_key(account_id).ok().flatten(),
            store: &self.store,
        }
    }

    /// The whole message from the server, opened if it's encrypted, so its attachments
    /// are the ones the person sees. Nothing opened is written back to the server.
    pub(crate) async fn readable_message(&self, message_id: i64) -> Result<Vec<u8>> {
        let raw = self.raw_message(message_id).await?;
        let account_id = self.store.message_location(message_id)?.account_id;
        Ok(readable_raw(&raw, &self.pgp_ring(account_id)).into_owned())
    }

    pub(crate) fn pgp_status(&self, account_id: i64) -> Result<Value> {
        self.store.account(account_id)?;
        Ok(match self.pgp_own_key(account_id)? {
            Some(key) => json!({
                "has_key": true,
                "fingerprint": fingerprint(&key.primary_key),
                "created_at": i64::from(key.primary_key.created_at().as_secs()),
                "public_key": armor_public(&key.to_public_key())?,
            }),
            None => json!({
                "has_key": false, "fingerprint": null, "created_at": null, "public_key": null,
            }),
        })
    }

    fn pgp_refuse_second_key(&self, account_id: i64) -> Result<()> {
        if self.secrets.get(&pgp_secret(account_id))?.is_some() {
            return Err(invalid(
                "This inbox already has an encryption key. Remove it first to use a different one.",
            ));
        }
        Ok(())
    }

    fn pgp_keep(&self, account_id: i64, key: &SignedSecretKey) -> Result<Value> {
        self.secrets
            .set(&pgp_secret(account_id), &armor_secret(key)?)?;
        self.store.pgp_forget_bodies(Some(account_id))?;
        self.pgp_status(account_id)
    }

    pub(crate) fn pgp_generate(&self, account_id: i64) -> Result<Value> {
        let account = self.store.account(account_id)?;
        self.pgp_refuse_second_key(account_id)?;
        let key = generate_key(&account.display_name, &account.email.to_lowercase())?;
        self.pgp_keep(account_id, &key)
    }

    pub(crate) fn pgp_import_secret(
        &self,
        account_id: i64,
        armored: &str,
        passphrase: Option<&str>,
    ) -> Result<Value> {
        let account = self.store.account(account_id)?;
        self.pgp_refuse_second_key(account_id)?;
        let key = read_secret(armored, passphrase)?;
        if !can_encrypt_to(&key.to_public_key()) {
            return Err(invalid(
                "This key can't be used for encryption. Choose a key that has an encryption subkey.",
            ));
        }
        let emails = key_emails(&key.details);
        let own = own_addresses(&account);
        if !emails.iter().any(|e| own.contains(e)) {
            let found = if emails.is_empty() {
                "no email address".to_string()
            } else {
                emails.join(", ")
            };
            return Err(invalid(format!(
                "This key is for {found}, not {}. Import the key made for this inbox.",
                account.email
            )));
        }
        self.pgp_keep(account_id, &key)
    }

    pub(crate) fn pgp_remove(&self, account_id: i64) -> Result<Value> {
        self.store.account(account_id)?;
        self.secrets.delete(&pgp_secret(account_id))?;
        self.store.pgp_forget_bodies(Some(account_id))?;
        Ok(json!({ "removed": true }))
    }

    pub(crate) fn pgp_key_import(&self, armored: &str) -> Result<Value> {
        let keys = read_publics(armored)?;
        let now = crate::identity::unix_now();
        let mut out = Vec::new();
        for key in &keys {
            if !can_encrypt_to(key) {
                continue;
            }
            let armored = armor_public(key)?;
            let print = fingerprint(key);
            for address in key_emails(&key.details) {
                self.store
                    .pgp_save_key(&address, &armored, &print, "imported", false, now)?;
                out.push(json!({ "address": address, "fingerprint": print }));
            }
        }
        if out.is_empty() {
            return Err(invalid(
                "No public keys with an email address were found in what you pasted. Paste the whole key, including the BEGIN and END lines.",
            ));
        }
        // Signatures from these people can be checked now.
        self.store.pgp_forget_bodies(None)?;
        Ok(Value::Array(out))
    }

    /// The person accepts the different key a later email offered: it's used from now on.
    pub(crate) fn pgp_key_accept_change(&self, address: &str) -> Result<Value> {
        let address = address.trim().to_lowercase();
        let Some((armored, print)) = self.store.pgp_offered_key(&address)? else {
            return Err(CoreError::NotFound(
                "There's no new key waiting for that address.".into(),
            ));
        };
        self.store.pgp_save_key(
            &address,
            &armored,
            &print,
            "autocrypt",
            false,
            crate::identity::unix_now(),
        )?;
        self.store.pgp_forget_bodies(None)?;
        Ok(json!({ "address": address, "fingerprint": print }))
    }

    pub(crate) fn pgp_key_delete(&self, address: &str) -> Result<Value> {
        if !self.store.pgp_delete_key(address)? {
            return Err(CoreError::NotFound(
                "There's no saved key for that address.".into(),
            ));
        }
        Ok(json!({ "deleted": true }))
    }

    /// The keys to encrypt to for these people, and who has none.
    pub(crate) fn pgp_recipient_keys(
        &self,
        account: &Account,
        own: Option<&SignedSecretKey>,
        to: &[String],
    ) -> Result<(Vec<SignedPublicKey>, Vec<String>)> {
        let mine = own_addresses(account);
        let mut keys = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for address in to {
            let address = address.trim().to_lowercase();
            if address.is_empty() || missing.contains(&address) {
                continue;
            }
            if mine.contains(&address) {
                match own {
                    Some(k) => keys.push(k.to_public_key()),
                    None => missing.push(address),
                }
                continue;
            }
            let found = self
                .store
                .pgp_key(&address)?
                .and_then(|(armored, _, _)| read_public(&armored))
                .filter(can_encrypt_to);
            match found {
                Some(k) => keys.push(k),
                None => missing.push(address),
            }
        }
        Ok((keys, missing))
    }

    pub(crate) fn pgp_can_encrypt(&self, account_id: i64, to: &[String]) -> Result<Value> {
        let account = self.store.account(account_id)?;
        let own = self.pgp_own_key(account_id)?;
        let (_, missing) = self.pgp_recipient_keys(&account, own.as_ref(), to)?;
        Ok(json!({ "own_key": own.is_some(), "missing": missing }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key whose main key only certifies, with an Ed25519 subkey marked as
    /// `sub_signs` and one for encryption.
    fn key_with_subkey(sub_signs: bool) -> SignedSecretKey {
        let mut encrypt = SubkeyParamsBuilder::default();
        encrypt
            .key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
            .can_encrypt(EncryptionCaps::All);
        let mut other = SubkeyParamsBuilder::default();
        other
            .key_type(KeyType::Ed25519Legacy)
            .can_sign(sub_signs)
            .can_authenticate(!sub_signs);
        let mut key = SecretKeyParamsBuilder::default();
        key.key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(false)
            .primary_user_id(user_id("Sam", "sam@x.test"))
            .subkeys(vec![other.build().unwrap(), encrypt.build().unwrap()]);
        key.build().unwrap().generate(rand::thread_rng()).unwrap()
    }

    fn signed_by_first_subkey(key: &SignedSecretKey, content: &[u8]) -> Vec<u8> {
        let sub = &key.secret_subkeys[0].key;
        DetachedSignature::sign_binary_data(
            rand::thread_rng(),
            sub,
            &Password::empty(),
            HashAlgorithm::Sha256,
            content,
        )
        .unwrap()
        .to_armored_string(ArmorOptions::default())
        .unwrap()
        .into_bytes()
    }

    #[test]
    fn only_parts_marked_for_signing_can_check_a_signature() {
        let content = b"Pay invoice 42";
        let signer = key_with_subkey(true);
        let ring = [(
            "sam@x.test".to_string(),
            signer.to_public_key(),
            Trust::Full,
        )];
        let sig = signed_by_first_subkey(&signer, content);
        assert_eq!(
            check_detached(&sig, content, &ring),
            Some(("sam@x.test".to_string(), Trust::Full))
        );
        // The same kind of subkey, but only for signing in to servers: not accepted.
        let other = key_with_subkey(false);
        let ring = [("sam@x.test".to_string(), other.to_public_key(), Trust::Full)];
        let sig = signed_by_first_subkey(&other, content);
        assert_eq!(check_detached(&sig, content, &ring), None);
        // Nor the main key, which these keys mark for certifying only.
        let main = &other.primary_key;
        let sig = DetachedSignature::sign_binary_data(
            rand::thread_rng(),
            main,
            &Password::empty(),
            HashAlgorithm::Sha256,
            &content[..],
        )
        .unwrap()
        .to_armored_string(ArmorOptions::default())
        .unwrap();
        assert_eq!(check_detached(sig.as_bytes(), content, &ring), None);
    }

    #[test]
    fn parts_keep_their_exact_bytes() {
        let body =
            b"preamble\r\n--b\r\nContent-Type: text/plain\r\n\r\nHi\r\n\r\n--b\r\nsig\r\n--b--\r\n";
        let parts = multipart_parts(body, "b");
        assert_eq!(
            parts,
            [&b"Content-Type: text/plain\r\n\r\nHi\r\n"[..], &b"sig"[..]]
        );
    }

    #[test]
    fn autocrypt_headers_are_unfolded() {
        let raw = b"From: a@x.test\r\nAutocrypt: addr=a@x.test;\r\n keydata=AAAA\r\n BBBB\r\nSubject: hi\r\n";
        assert_eq!(
            header_values(raw, "autocrypt"),
            ["addr=a@x.test; keydata=AAAA BBBB"]
        );
    }

    #[test]
    fn user_ids_give_their_address() {
        assert_eq!(
            email_of("Ana B <Ana@X.test>").as_deref(),
            Some("ana@x.test")
        );
        assert_eq!(email_of("ana@x.test").as_deref(), Some("ana@x.test"));
        assert_eq!(email_of("Ana"), None);
    }

    #[test]
    fn line_breaks_become_crlf_once() {
        assert_eq!(crlf(b"a\nb\r\nc"), b"a\r\nb\r\nc");
    }
}
