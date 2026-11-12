//! The encryption behind linking devices. It uses standard building blocks:
//! XChaCha20-Poly1305 for the vault and libsodium-style sealed boxes for passing
//! the vault key to a new device.

use crate::error::{CoreError, Result};
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use crypto_box::{PublicKey, SecretKey};
use data_encoding::{BASE32_NOPAD, BASE64, HEXLOWER};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

const VAULT_LABEL: &[u8] = b"inbox-vault-v1";
const RECOVERY_LABEL: &[u8] = b"inbox-recovery-v1";
const COMMIT_LABEL: &[u8] = b"inbox-link-commit-v1";
const NUMBER_LABEL: &[u8] = b"inbox-link-number-v1";
const TEAM_THREAD_LABEL: &[u8] = b"inbox-team-thread-v1";
const LINK_NONCE_LABEL: &[u8] = b"inbox-link-nonce-v1";
/// Random half and proof half of a link nonce, in hex characters each.
const LINK_NONCE_HALF: usize = 32;

const NONCE_LEN: usize = 24;
const RECOVERY_BYTES: usize = 20;

fn bad_data() -> CoreError {
    CoreError::Invalid("Encrypted data could not be opened. It may be damaged.".into())
}

pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    OsRng.fill_bytes(&mut bytes);
    HEXLOWER.encode(&bytes)
}

fn key32(hex: &str) -> Result<[u8; 32]> {
    HEXLOWER
        .decode(hex.as_bytes())
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(bad_data)
}

fn seal_with(key: &[u8; 32], label: &[u8], plain: &[u8]) -> Result<String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut out = nonce.to_vec();
    out.extend(
        cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plain,
                    aad: label,
                },
            )
            .map_err(|_| bad_data())?,
    );
    Ok(BASE64.encode(&out))
}

fn open_with(key: &[u8; 32], label: &[u8], sealed: &str) -> Result<Vec<u8>> {
    let raw = BASE64.decode(sealed.as_bytes()).map_err(|_| bad_data())?;
    if raw.len() < NONCE_LEN {
        return Err(bad_data());
    }
    let (nonce, body) = raw.split_at(NONCE_LEN);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: body,
                aad: label,
            },
        )
        .map_err(|_| bad_data())
}

/// Encrypts with a 32-byte key given as hex, for one named purpose. Something sealed for
/// one purpose can't be opened as another.
pub fn seal_for(key_hex: &str, purpose: &str, plain: &[u8]) -> Result<String> {
    seal_with(&key32(key_hex)?, purpose.as_bytes(), plain)
}

pub fn open_for(key_hex: &str, purpose: &str, sealed: &str) -> Result<Vec<u8>> {
    open_with(&key32(key_hex)?, purpose.as_bytes(), sealed)
}

/// Encrypts the vault with the vault key, given as hex.
pub fn vault_seal(vault_key: &str, plain: &[u8]) -> Result<String> {
    seal_with(&key32(vault_key)?, VAULT_LABEL, plain)
}

pub fn vault_open(vault_key: &str, sealed: &str) -> Result<Vec<u8>> {
    open_with(&key32(vault_key)?, VAULT_LABEL, sealed)
}

/// A new device's keys, as hex: (secret, public).
pub fn device_keys() -> (String, String) {
    let secret = SecretKey::generate(&mut OsRng);
    let public = secret.public_key();
    (
        HEXLOWER.encode(&secret.to_bytes()),
        HEXLOWER.encode(public.as_bytes()),
    )
}

/// Encrypts a message that only the holder of the secret key can open.
pub fn seal_to(public_hex: &str, plain: &[u8]) -> Result<String> {
    let public = PublicKey::from(key32(public_hex)?);
    let sealed = public.seal(&mut OsRng, plain).map_err(|_| bad_data())?;
    Ok(BASE64.encode(&sealed))
}

pub fn unseal(secret_hex: &str, sealed: &str) -> Result<Vec<u8>> {
    let secret = SecretKey::from(key32(secret_hex)?);
    let raw = BASE64.decode(sealed.as_bytes()).map_err(|_| bad_data())?;
    secret.unseal(&raw).map_err(|_| bad_data())
}

/// The new device's sealed promise of its public key, sent before it sees the
/// other device's random value.
pub fn commitment(public_hex: &str, salt_hex: &str) -> String {
    let mut h = Sha256::new();
    h.update(COMMIT_LABEL);
    h.update(public_hex.as_bytes());
    h.update(salt_hex.as_bytes());
    HEXLOWER.encode(&h.finalize())
}

/// The six-digit number both devices must agree on.
///
/// Each side fixes its part before seeing the other's, so someone in the middle
/// can't search for a fake key that produces the same number.
pub fn link_number(public_hex: &str, salt_hex: &str, nonce_hex: &str) -> String {
    let mut h = Sha256::new();
    h.update(NUMBER_LABEL);
    h.update(public_hex.as_bytes());
    h.update(salt_hex.as_bytes());
    h.update(nonce_hex.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

fn link_nonce_tag(vault_key: &str, device_id: i64, random_hex: &str) -> String {
    let hk = Hkdf::<Sha256>::new(Some(LINK_NONCE_LABEL), vault_key.as_bytes());
    let info = format!("{device_id}\0{random_hex}");
    let mut out = [0u8; LINK_NONCE_HALF / 2];
    // A 16-byte result is always within what this can produce.
    let _ = hk.expand(info.as_bytes(), &mut out);
    HEXLOWER.encode(&out)
}

/// A random value for linking a new device, made by one of the person's own
/// devices. Its second half proves that (it needs the vault key, which the
/// account server never has), so any of their devices can approve with it but a
/// value the server made up is refused. Same length and shape as before: 64 hex.
pub fn new_link_nonce(vault_key: &str, device_id: i64) -> String {
    let random = random_hex(LINK_NONCE_HALF / 2);
    let tag = link_nonce_tag(vault_key, device_id, &random);
    format!("{random}{tag}")
}

/// Whether a link nonce was made by one of the person's devices for this device.
pub fn link_nonce_is_ours(vault_key: &str, device_id: i64, nonce: &str) -> bool {
    if nonce.len() != LINK_NONCE_HALF * 2 || !nonce.is_ascii() {
        return false;
    }
    let (random, tag) = nonce.split_at(LINK_NONCE_HALF);
    let want = link_nonce_tag(vault_key, device_id, random);
    // Compared without stopping early, so timing says nothing about the tag.
    want.len() == tag.len()
        && want
            .bytes()
            .zip(tag.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// A new recovery key to show the person, in groups of four letters and digits.
pub fn new_recovery_key() -> String {
    let mut bytes = [0u8; RECOVERY_BYTES];
    OsRng.fill_bytes(&mut bytes);
    BASE32_NOPAD
        .encode(&bytes)
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Splits a recovery key into proof for the server, and a key the server never
/// sees: (proof, wrapping key).
pub fn recovery_parts(recovery_key: &str) -> Result<(String, [u8; 32])> {
    let cleaned: String = recovery_key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let wrong = || CoreError::Cloud {
        code: "wrong_recovery_key".into(),
        message: "That recovery key isn't right.".into(),
    };
    let bytes = BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .map_err(|_| wrong())?;
    if bytes.len() != RECOVERY_BYTES {
        return Err(wrong());
    }
    let kdf = Hkdf::<Sha256>::new(Some(RECOVERY_LABEL), &bytes);
    let mut proof = [0u8; 32];
    let mut wrap = [0u8; 32];
    kdf.expand(b"proof", &mut proof).map_err(|_| wrong())?;
    kdf.expand(b"wrap", &mut wrap).map_err(|_| wrong())?;
    Ok((HEXLOWER.encode(&proof), wrap))
}

pub fn recovery_seal(wrap: &[u8; 32], vault_key: &str) -> Result<String> {
    seal_with(wrap, RECOVERY_LABEL, vault_key.as_bytes())
}

pub fn recovery_open(wrap: &[u8; 32], sealed: &str) -> Result<String> {
    String::from_utf8(open_with(wrap, RECOVERY_LABEL, sealed)?).map_err(|_| bad_data())
}

/// The number the account server knows a shared conversation by. Everyone in the
/// team works out the same one, and the server can't turn it back into anything.
pub fn team_thread_key(team_key: &str, inbox: &str, thread_id: &str) -> String {
    let info = format!("{}\0{thread_id}", inbox.to_lowercase());
    let hk = Hkdf::<Sha256>::new(Some(TEAM_THREAD_LABEL), team_key.as_bytes());
    let mut out = [0u8; 32];
    // A 32-byte result is always within what this can produce.
    let _ = hk.expand(info.as_bytes(), &mut out);
    HEXLOWER.encode(&out)
}

/// The hash the server stores to check a recovery key's proof.
pub fn proof_hash(proof: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(proof.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_opens_only_with_its_key() {
        let key = random_hex(32);
        let sealed = vault_seal(&key, b"secret").unwrap();
        assert_eq!(vault_open(&key, &sealed).unwrap(), b"secret");
        assert!(vault_open(&random_hex(32), &sealed).is_err());
        assert!(!sealed.contains("secret"));
    }

    #[test]
    fn tampering_with_the_vault_is_detected() {
        let key = random_hex(32);
        let sealed = vault_seal(&key, b"secret").unwrap();
        let mut raw = BASE64.decode(sealed.as_bytes()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(vault_open(&key, &BASE64.encode(&raw)).is_err());
    }

    #[test]
    fn sealed_messages_open_only_for_the_right_device() {
        let (secret, public) = device_keys();
        let (other_secret, _) = device_keys();
        let sealed = seal_to(&public, b"vault key").unwrap();
        assert_eq!(unseal(&secret, &sealed).unwrap(), b"vault key");
        assert!(unseal(&other_secret, &sealed).is_err());
    }

    #[test]
    fn the_number_changes_with_every_input() {
        let n = link_number("a", "b", "c");
        assert_eq!(n.len(), 6);
        assert_eq!(n, link_number("a", "b", "c"));
        assert_ne!(n, link_number("x", "b", "c"));
        assert_ne!(n, link_number("a", "b", "x"));
    }

    #[test]
    fn link_nonces_prove_they_came_from_the_persons_devices() {
        let vault = random_hex(32);
        let nonce = new_link_nonce(&vault, 7);
        assert_eq!(nonce.len(), 64);
        assert!(link_nonce_is_ours(&vault, 7, &nonce));
        // Another device, another vault, or a server-made value: all refused.
        assert!(!link_nonce_is_ours(&vault, 8, &nonce));
        assert!(!link_nonce_is_ours(&random_hex(32), 7, &nonce));
        assert!(!link_nonce_is_ours(&vault, 7, &random_hex(32)));
        assert!(!link_nonce_is_ours(&vault, 7, ""));
    }

    #[test]
    fn recovery_key_works_however_it_is_typed() {
        let key = new_recovery_key();
        let (proof, wrap) = recovery_parts(&key).unwrap();
        let sloppy = key.replace('-', " ").to_lowercase();
        let (proof2, wrap2) = recovery_parts(&sloppy).unwrap();
        assert_eq!(proof, proof2);
        assert_eq!(wrap, wrap2);
        let sealed = recovery_seal(&wrap, "vault").unwrap();
        assert_eq!(recovery_open(&wrap2, &sealed).unwrap(), "vault");
        assert!(recovery_parts("not a key").is_err());
    }

    #[test]
    fn recovery_proof_cannot_open_the_vault_key() {
        let (proof, wrap) = recovery_parts(&new_recovery_key()).unwrap();
        assert_ne!(proof, HEXLOWER.encode(&wrap));
    }
}
