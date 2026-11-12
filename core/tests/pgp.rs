//! Encrypted email, end to end: keys, Autocrypt, sending encrypted and signed mail, and
//! reading what others send, through the fake mail servers. The other side of each
//! conversation uses the OpenPGP library directly, as another mail app would.

use fakeai::FakeAi;
use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mail_parser::MessageParser;
use mailcore::{Engine, MemoryStore, Store};
use pgp::composed::{
    ArmorOptions, Deserializable, DetachedSignature, Message, MessageBuilder, SignedPublicKey,
    SignedSecretKey,
};
use pgp::crypto::{hash::HashAlgorithm, sym::SymmetricKeyAlgorithm};
use pgp::types::{KeyDetails, Password};
use serde_json::{json, Value};
use std::sync::Arc;

#[path = "support/fake_paid.rs"]
mod fake_paid;
use fake_paid::FakePaid;

const ME: &str = "me@grayic.test";
const PASS: &str = "secret-pass";
const BOB: &str = "bob@x.test";

struct Rig {
    engine: Engine,
    imap: FakeServer,
    smtp: FakeSmtp,
    ai: FakeAi,
    /// The AI that comes with Mailzo Cloud.
    paid: Arc<FakePaid>,
    account: i64,
    _dir: tempfile::TempDir,
}

async fn rig() -> Rig {
    let imap = FakeServer::start(ME, PASS).await;
    imap.add_folder("INBOX", &[], 1);
    imap.add_folder("Sent", &["\\Sent"], 1);
    imap.add_folder("Trash", &["\\Trash"], 1);
    let smtp = FakeSmtp::start(ME, PASS).await;
    let ai = FakeAi::start().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_folders(dir.path().join("data"), dir.path().join("downloads"))
    .with_sent_copy_wait(0)
    .with_ai_url(&ai.url);
    let paid = FakePaid::new();
    let engine = engine.with_paid_ai(paid.clone());
    let added = engine
        .call(
            "add_account",
            json!({
                "email": ME, "imap_host": "127.0.0.1", "imap_port": imap.addr.port(),
                "security": "plain", "password": PASS,
                "smtp_host": "127.0.0.1", "smtp_port": smtp.addr.port(), "smtp_security": "plain",
            }),
        )
        .await
        .unwrap();
    Rig {
        engine,
        imap,
        smtp,
        ai,
        paid,
        account: added["id"].as_i64().unwrap(),
        _dir: dir,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }

    async fn fails(&self, cmd: &str, args: Value) -> (String, String) {
        let e = self.engine.call(cmd, args).await.unwrap_err();
        (e.code().to_string(), e.to_string())
    }

    async fn sync(&self) {
        let stats = self
            .call("sync_account", json!({ "account_id": self.account }))
            .await;
        assert!(stats["errors"].as_array().unwrap().is_empty(), "{stats}");
    }

    async fn generate(&self) -> Value {
        self.call("pgp_generate", json!({ "account_id": self.account }))
            .await
    }

    async fn my_public_key(&self) -> SignedPublicKey {
        let status = self
            .call("pgp_status", json!({ "account_id": self.account }))
            .await;
        SignedPublicKey::from_string(status["public_key"].as_str().unwrap())
            .unwrap()
            .0
    }

    /// The newest message with this subject in a folder, as the reader gets it.
    async fn message(&self, role: &str, subject: &str) -> Value {
        let folders = self
            .call("list_folders", json!({ "account_id": self.account }))
            .await;
        let folder = folders
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["role"] == role)
            .unwrap()["id"]
            .clone();
        let threads = self
            .call("list_threads", json!({ "folder_id": folder }))
            .await;
        let thread = threads
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["subject"] == subject)
            .unwrap_or_else(|| panic!("no {subject} in {role}: {threads}"))
            .clone();
        let messages = self
            .call(
                "get_thread",
                json!({ "account_id": self.account, "thread_id": thread["thread_id"], "folder_id": folder }),
            )
            .await;
        messages.as_array().unwrap().last().unwrap().clone()
    }

    /// Writes and sends a message, giving what came back, or the refusal.
    async fn send(&self, to: &[&str], subject: &str, text: &str, flags: Value) -> Value {
        let d = self
            .call(
                "draft_new",
                json!({ "account_id": self.account, "mode": "new" }),
            )
            .await;
        assert_eq!((&d["encrypt"], &d["sign"]), (&json!(false), &json!(false)));
        let mut args = json!({
            "id": d["id"], "account_id": self.account,
            "to": to.iter().map(|a| json!({ "name": "", "address": a })).collect::<Vec<_>>(),
            "subject": subject, "html": format!("<p>{text}</p>"), "text": text,
        });
        for (k, v) in flags.as_object().unwrap() {
            args[k] = v.clone();
        }
        let saved = self.call("draft_save", args).await;
        for (k, v) in flags.as_object().unwrap() {
            assert_eq!(&saved[k], v, "the draft keeps {k}");
        }
        match self
            .engine
            .call("send_draft", json!({ "id": d["id"] }))
            .await
        {
            Ok(v) => v,
            Err(e) => json!({ "code": e.code(), "message": e.to_string() }),
        }
    }
}

fn bob_key() -> SignedSecretKey {
    mailcore::pgp::generate_key("Bob", BOB).unwrap()
}

fn print(key: &SignedSecretKey) -> String {
    format!("{:X}", key.primary_key.fingerprint())
}

fn armored_public(key: &SignedSecretKey) -> String {
    key.to_public_key()
        .to_armored_string(ArmorOptions::default())
        .unwrap()
}

/// Mail from Bob offering his key, as his mail app would send it.
fn mail_with_autocrypt(key: &SignedSecretKey, id: &str, date: &str) -> String {
    let value = mailcore::pgp::autocrypt_value(BOB, key).unwrap();
    format!(
        "Authentication-Results: mx.grayic.test; dkim=pass header.d=x.test; spf=pass; dmarc=pass\r\nFrom: Bob <{BOB}>\r\nTo: {ME}\r\nSubject: Hello from Bob\r\nDate: {date}\r\nMessage-ID: <{id}>\r\nAutocrypt: {value}\r\nContent-Type: text/plain\r\n\r\nHi there\r\n"
    )
}

/// An encrypted, signed email in the PGP/MIME form (RFC 3156).
fn encrypted_mail(
    to_key: &SignedPublicKey,
    signer: &SignedSecretKey,
    subject: &str,
    inner: &str,
) -> String {
    let mut rng = rand::thread_rng();
    let mut b = MessageBuilder::from_bytes("", inner.as_bytes().to_vec())
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    b.encrypt_to_key(&mut rng, &to_key.public_subkeys[0])
        .unwrap();
    b.sign(
        &signer.primary_key,
        Password::empty(),
        HashAlgorithm::Sha256,
    );
    let armored = b
        .to_armored_string(&mut rng, ArmorOptions::default())
        .unwrap()
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    format!(
        "From: Bob <{BOB}>\r\nTo: {ME}\r\nSubject: {subject}\r\nDate: Tue, 03 Nov 2026 09:00:00 +0000\r\nMessage-ID: <{subject}@x.test>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"enc\"\r\n\r\n--enc\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n\r\n--enc\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\n\r\n{armored}\r\n--enc--\r\n"
    )
}

fn armored_block(data: &str, kind: &str) -> String {
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let a = data
        .find(&begin)
        .unwrap_or_else(|| panic!("no {kind} in {data}"));
    let b = data[a..].find(&end).unwrap() + a + end.len();
    data[a..b].to_string()
}

#[tokio::test]
async fn an_inbox_gets_a_key_once() {
    let r = rig().await;
    let none = r
        .call("pgp_status", json!({ "account_id": r.account }))
        .await;
    assert_eq!(
        none,
        json!({ "has_key": false, "fingerprint": null, "created_at": null, "public_key": null })
    );
    let made = r.generate().await;
    assert_eq!(made["has_key"], true);
    assert_eq!(made["fingerprint"].as_str().unwrap().len(), 40);
    assert!(made["created_at"].as_i64().unwrap() > 0);
    let public = r.my_public_key().await;
    assert_eq!(made["fingerprint"], format!("{:X}", public.fingerprint()));
    assert!(mailcore::pgp::key_emails(&public.details).contains(&ME.to_string()));
    // Ed25519 to sign, Curve25519 to encrypt, as GnuPG makes them.
    assert_eq!(
        public.primary_key.algorithm(),
        pgp::crypto::public_key::PublicKeyAlgorithm::EdDSALegacy
    );
    assert_eq!(
        public.public_subkeys[0].key.algorithm(),
        pgp::crypto::public_key::PublicKeyAlgorithm::ECDH
    );

    let (code, message) = r
        .fails("pgp_generate", json!({ "account_id": r.account }))
        .await;
    assert_eq!(code, "invalid");
    assert!(
        message.contains("already has an encryption key"),
        "{message}"
    );

    assert_eq!(
        r.call("pgp_remove", json!({ "account_id": r.account }))
            .await,
        json!({ "removed": true })
    );
    let after = r
        .call("pgp_status", json!({ "account_id": r.account }))
        .await;
    assert_eq!(after["has_key"], false);
}

#[tokio::test]
async fn outgoing_mail_offers_the_key_in_an_autocrypt_header() {
    let r = rig().await;
    let plain = r.send(&[BOB], "Before", "no key yet", json!({})).await;
    assert_eq!(plain["sent"], true, "{plain}");
    assert!(!r.smtp.received()[0].data.contains("Autocrypt:"));

    r.generate().await;
    let sent = r.send(&[BOB], "After", "with key", json!({})).await;
    assert_eq!(sent["sent"], true, "{sent}");
    let data = r.smtp.received()[1].data.clone();
    let header = &data[..data.find("\r\n\r\n").unwrap()];
    let start = header
        .find("Autocrypt: addr=me@grayic.test; keydata=")
        .unwrap();
    let value: String = header[start..]
        .split("\r\n")
        .enumerate()
        .take_while(|(i, l)| *i == 0 || l.starts_with(' '))
        .map(|(_, l)| l.trim().to_string())
        .collect();
    let keydata = value.split("keydata=").nth(1).unwrap();
    let bytes = data_encoding::BASE64.decode(keydata.as_bytes()).unwrap();
    let offered = SignedPublicKey::from_bytes(&bytes[..]).unwrap();
    assert_eq!(offered.fingerprint(), r.my_public_key().await.fingerprint());
    // Plain mail stays plain.
    assert!(data.contains("with key"));
}

#[tokio::test]
async fn keys_are_learned_from_autocrypt_and_mail_goes_out_encrypted() {
    let r = rig().await;
    r.generate().await;
    let bob = bob_key();
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&bob, "a1@x.test", "Mon, 02 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    assert_eq!(
        r.call("pgp_keys_list", json!({})).await,
        json!([{ "address": BOB, "fingerprint": print(&bob), "source": "autocrypt", "changed": false }])
    );

    let can = r
        .call(
            "pgp_can_encrypt",
            json!({ "account_id": r.account, "to": [BOB, "Dave@x.test", ME] }),
        )
        .await;
    assert_eq!(can, json!({ "own_key": true, "missing": ["dave@x.test"] }));

    let sent = r
        .send(
            &[BOB],
            "Plans",
            "The secret plans",
            json!({ "encrypt": true }),
        )
        .await;
    assert_eq!(sent["sent"], true, "{sent}");
    let data = r.smtp.received()[0].data.clone();
    assert!(!data.contains("secret plans"), "nothing readable goes out");
    let headers = MessageParser::default()
        .parse_headers(data.as_bytes())
        .unwrap();
    let ct = headers.content_type().unwrap();
    assert_eq!(ct.subtype(), Some("encrypted"));
    assert_eq!(ct.attribute("protocol"), Some("application/pgp-encrypted"));
    assert_eq!(headers.subject(), Some("Plans"));
    assert!(data.contains("Version: 1"));

    // Bob's own copy of the library opens it, and the signature is mine.
    let armored = armored_block(&data, "PGP MESSAGE");
    let (message, _) = Message::from_string(&armored).unwrap();
    let mut opened = message.decrypt(&Password::empty(), &bob).unwrap();
    let inner = opened.as_data_string().unwrap();
    assert!(inner.contains("The secret plans"), "{inner}");
    assert!(inner.contains("multipart/alternative"), "{inner}");
    let mine = r.my_public_key().await;
    opened.verify(&mine.primary_key).unwrap();

    // The copy in Sent is the encrypted one, and still reads here.
    let copies = r.imap.folder_messages("Sent");
    let copy = String::from_utf8_lossy(&copies[0].raw).to_string();
    assert!(copy.contains("-----BEGIN PGP MESSAGE-----"));
    assert!(!copy.contains("secret plans"));
    r.sync().await;
    let shown = r.message("sent", "Plans").await;
    let body = &shown["body"];
    assert!(body["text"].as_str().unwrap().contains("The secret plans"));
    assert_eq!(
        body["pgp"],
        json!({ "encrypted": true, "signed": true, "verified": true, "signer": ME, "problem": null })
    );
}

#[tokio::test]
async fn encrypted_signed_mail_to_us_reads_and_checks_out() {
    let r = rig().await;
    r.generate().await;
    let bob = bob_key();
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&bob, "a1@x.test", "Mon, 02 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    let mine = r.my_public_key().await;
    let inner = "Content-Type: multipart/mixed; boundary=\"in\"\r\n\r\n--in\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon. Bring the map.\r\n--in\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"map.txt\"\r\n\r\nX marks the spot\r\n--in--\r\n";
    r.imap.add_message(
        "INBOX",
        &encrypted_mail(&mine, &bob, "Secret", inner),
        false,
    );
    r.sync().await;
    let shown = r.message("inbox", "Secret").await;
    let body = &shown["body"];
    assert!(
        body["text"].as_str().unwrap().contains("Meet at noon"),
        "{body}"
    );
    // Bob's key only came from his earlier email, so the signature is good but not
    // shown as verified.
    assert_eq!(
        body["pgp"],
        json!({ "encrypted": true, "signed": true, "verified": false, "signer": BOB, "problem": null, "signer_unconfirmed": true })
    );
    // Its attachment is the one inside, and opens.
    assert_eq!(body["attachments"][0]["name"], "map.txt");
    let preview = r
        .call(
            "attachment_preview",
            json!({ "message_id": shown["id"], "part": body["attachments"][0]["part"] }),
        )
        .await;
    assert_eq!(preview["text"], "X marks the spot");
    // The list shows its words, not the scrambled text.
    let threads = r
        .call("list_threads", json!({ "folder_id": shown["folder_id"] }))
        .await;
    let secret = threads
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["subject"] == "Secret")
        .unwrap();
    let snippet = secret["snippet"].as_str().unwrap();
    assert!(snippet.contains("Meet at noon"), "{snippet}");
}

#[tokio::test]
async fn signed_mail_goes_out_and_comes_back_verified() {
    let r = rig().await;
    r.generate().await;
    let sent = r
        .send(&[BOB], "Signed", "Sign here", json!({ "sign": true }))
        .await;
    assert_eq!(sent["sent"], true, "{sent}");
    let data = r.smtp.received()[0].data.clone();
    assert!(data.contains("Sign here"));
    let headers = MessageParser::default()
        .parse_headers(data.as_bytes())
        .unwrap();
    let ct = headers.content_type().unwrap();
    assert_eq!(ct.subtype(), Some("signed"));
    assert_eq!(ct.attribute("protocol"), Some("application/pgp-signature"));
    assert_eq!(ct.attribute("micalg"), Some("pgp-sha256"));
    let boundary = ct.attribute("boundary").unwrap().to_string();

    // What a receiving app checks: the first part's exact bytes.
    let open = format!("--{boundary}\r\n");
    let start = data.find(&open).unwrap() + open.len();
    let end = data[start..]
        .find(&format!("\r\n--{boundary}\r\n"))
        .unwrap()
        + start;
    let signed = &data.as_bytes()[start..end];
    let sig = armored_block(&data, "PGP SIGNATURE");
    let (sig, _) = DetachedSignature::from_string(&sig).unwrap();
    let mine = r.my_public_key().await;
    sig.verify(&mine.primary_key, signed).unwrap();

    // Changing a word breaks it, and the reader says so.
    r.imap.add_message("INBOX", &data, false);
    let changed = data
        .replace("Sign here", "Sign there")
        .replace("Subject: Signed", "Subject: Changed")
        .replace("Message-ID: <", "Message-ID: <changed-");
    r.imap.add_message("INBOX", &changed, false);
    r.sync().await;
    let good = r.message("inbox", "Signed").await;
    assert_eq!(
        good["body"]["pgp"],
        json!({ "encrypted": false, "signed": true, "verified": true, "signer": ME, "problem": null })
    );
    assert!(good["body"]["text"].as_str().unwrap().contains("Sign here"));
    // The signature isn't listed as an attachment.
    assert!(good["body"]["attachments"].as_array().unwrap().is_empty());
    let bad = r.message("inbox", "Changed").await;
    assert_eq!(bad["body"]["pgp"]["verified"], false, "{bad}");
    assert!(bad["body"]["pgp"]["problem"]
        .as_str()
        .unwrap()
        .contains("doesn't match"));
}

#[tokio::test]
async fn mail_encrypted_to_someone_else_says_so() {
    let r = rig().await;
    r.generate().await;
    let carol = mailcore::pgp::generate_key("Carol", "carol@x.test").unwrap();
    let bob = bob_key();
    r.imap.add_message(
        "INBOX",
        &encrypted_mail(
            &carol.to_public_key(),
            &bob,
            "NotMine",
            "Content-Type: text/plain\r\n\r\nFor Carol\r\n",
        ),
        false,
    );
    r.sync().await;
    let shown = r.message("inbox", "NotMine").await;
    assert_eq!(
        shown["body"]["pgp"],
        json!({ "encrypted": true, "signed": false, "verified": false, "signer": null,
                "problem": "This email is encrypted, but not to your key." })
    );
    assert!(shown["body"]["text"].is_null());

    // Without a key of its own, the inbox says what's needed.
    r.call("pgp_remove", json!({ "account_id": r.account }))
        .await;
    r.sync().await;
    let shown = r.message("inbox", "NotMine").await;
    assert_eq!(
        shown["body"]["pgp"]["problem"],
        "You need your encryption key on this Mac to read this."
    );
    // Ordinary mail has nothing.
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&bob, "plain@x.test", "Mon, 02 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    assert!(r.message("inbox", "Hello from Bob").await["body"]["pgp"].is_null());
}

#[tokio::test]
async fn sending_encrypted_without_everyones_key_is_refused() {
    let r = rig().await;
    let none = r
        .send(&[BOB], "Nokey", "hi", json!({ "encrypt": true }))
        .await;
    assert_eq!(none["code"], "pgp_no_key", "{none}");

    r.generate().await;
    r.call(
        "pgp_key_import",
        json!({ "armored": armored_public(&bob_key()) }),
    )
    .await;
    let refused = r
        .send(
            &[BOB, "dave@x.test"],
            "Missing",
            "hi",
            json!({ "encrypt": true }),
        )
        .await;
    assert_eq!(refused["code"], "pgp_missing_keys", "{refused}");
    let message = refused["message"].as_str().unwrap();
    assert!(
        message.contains("dave@x.test") && !message.contains(BOB),
        "{message}"
    );
    assert!(r.smtp.received().is_empty(), "nothing was sent");
    // The drafts are still there to fix.
    assert_eq!(
        r.call("draft_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn imported_keys_are_kept_over_autocrypt_and_can_be_deleted() {
    let r = rig().await;
    let chosen = bob_key();
    let offered = bob_key();
    let imported = r
        .call(
            "pgp_key_import",
            json!({ "armored": armored_public(&chosen) }),
        )
        .await;
    assert_eq!(
        imported,
        json!([{ "address": BOB, "fingerprint": print(&chosen) }])
    );
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&offered, "b1@x.test", "Wed, 04 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    assert_eq!(
        r.call("pgp_keys_list", json!({})).await,
        json!([{ "address": BOB, "fingerprint": print(&chosen), "source": "imported", "changed": false }])
    );
    assert_eq!(
        r.call("pgp_key_delete", json!({ "address": "Bob@X.test" }))
            .await,
        json!({ "deleted": true })
    );
    assert!(r
        .call("pgp_keys_list", json!({}))
        .await
        .as_array()
        .unwrap()
        .is_empty());
    let (code, _) = r.fails("pgp_key_delete", json!({ "address": BOB })).await;
    assert_eq!(code, "not_found");
    let (code, message) = r
        .fails("pgp_key_import", json!({ "armored": "nonsense" }))
        .await;
    assert_eq!(code, "invalid", "{message}");
}

#[tokio::test]
async fn a_different_autocrypt_key_is_kept_aside_and_flagged() {
    let r = rig().await;
    r.generate().await;
    let first = bob_key();
    let second = bob_key();
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&first, "c1@x.test", "Mon, 02 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&second, "c2@x.test", "Tue, 03 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    // The first key stays in use; the change is shown, never made silently.
    assert_eq!(
        r.call("pgp_keys_list", json!({})).await,
        json!([{
            "address": BOB, "fingerprint": print(&first), "source": "autocrypt", "changed": true,
            "old_fingerprint": print(&first), "new_fingerprint": print(&second),
        }])
    );
    // Mail signed with the new key carries a warning.
    let mine = r.my_public_key().await;
    r.imap.add_message(
        "INBOX",
        &encrypted_mail(
            &mine,
            &second,
            "NewKey",
            "Content-Type: text/plain\r\n\r\nHello\r\n",
        ),
        false,
    );
    r.sync().await;
    let pgp = &r.message("inbox", "NewKey").await["body"]["pgp"];
    assert_eq!(pgp["verified"], false, "{pgp}");
    assert_eq!(pgp["key_changed"], true, "{pgp}");
    assert!(
        pgp["problem"].as_str().unwrap().contains("different key"),
        "{pgp}"
    );

    // A header for someone else's address is ignored.
    let forged = mail_with_autocrypt(&first, "c4@x.test", "Wed, 04 Nov 2026 09:00:00 +0000")
        .replace(&format!("From: Bob <{BOB}>"), "From: Eve <eve@x.test>");
    r.imap.add_message("INBOX", &forged, false);
    r.sync().await;
    assert_eq!(
        r.call("pgp_keys_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Accepting the change puts the new key in use.
    r.call("pgp_key_accept_change", json!({ "address": BOB }))
        .await;
    assert_eq!(
        r.call("pgp_keys_list", json!({})).await,
        json!([{ "address": BOB, "fingerprint": print(&second), "source": "autocrypt", "changed": false }])
    );
}

#[tokio::test]
async fn keys_are_only_learned_from_mail_the_provider_confirmed() {
    let r = rig().await;
    let bob = bob_key();
    let unchecked = mail_with_autocrypt(&bob, "d1@x.test", "Mon, 02 Nov 2026 09:00:00 +0000")
        .replace(
            "Authentication-Results: mx.grayic.test; dkim=pass header.d=x.test; spf=pass; dmarc=pass\r\n",
            "",
        );
    let failed = mail_with_autocrypt(&bob, "d2@x.test", "Mon, 02 Nov 2026 09:00:00 +0000")
        .replace("dmarc=pass", "dmarc=fail");
    // Signed by some other domain, and only SPF: neither says it's really from x.test.
    let elsewhere = mail_with_autocrypt(&bob, "d3@x.test", "Mon, 02 Nov 2026 09:00:00 +0000")
        .replace(
            "dkim=pass header.d=x.test; spf=pass; dmarc=pass",
            "dkim=pass header.d=evil.test; spf=pass",
        );
    let spam = mail_with_autocrypt(&bob, "d4@x.test", "Mon, 02 Nov 2026 09:00:00 +0000")
        .replace("Autocrypt:", "X-Spam-Flag: YES\r\nAutocrypt:");
    for m in [&unchecked, &failed, &elsewhere, &spam] {
        r.imap.add_message("INBOX", m, false);
    }
    r.sync().await;
    assert!(r
        .call("pgp_keys_list", json!({}))
        .await
        .as_array()
        .unwrap()
        .is_empty());
    // DKIM of their own domain is enough.
    let dkim = mail_with_autocrypt(&bob, "d5@x.test", "Mon, 02 Nov 2026 09:00:00 +0000").replace(
        "dkim=pass header.d=x.test; spf=pass; dmarc=pass",
        "dkim=pass header.d=x.test",
    );
    r.imap.add_message("INBOX", &dkim, false);
    r.sync().await;
    assert_eq!(
        r.call("pgp_keys_list", json!({})).await[0]["fingerprint"],
        json!(print(&bob))
    );
}

#[tokio::test]
async fn a_date_in_the_future_does_not_pin_a_key() {
    let r = rig().await;
    let first = bob_key();
    let second = bob_key();
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&first, "e1@x.test", "Fri, 01 Jan 2100 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&second, "e2@x.test", "Thu, 01 Jan 2099 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    // Both dates count as "now", so the first one's far-off date doesn't make the later
    // email look older: its different key is offered, not ignored.
    let list = r.call("pgp_keys_list", json!({})).await;
    assert_eq!(list[0]["changed"], true, "{list}");
    assert_eq!(list[0]["new_fingerprint"], json!(print(&second)));
}

#[tokio::test]
async fn an_added_key_gives_a_verified_signature() {
    let r = rig().await;
    r.generate().await;
    let bob = bob_key();
    r.call("pgp_key_import", json!({ "armored": armored_public(&bob) }))
        .await;
    let mine = r.my_public_key().await;
    r.imap.add_message(
        "INBOX",
        &encrypted_mail(
            &mine,
            &bob,
            "Chosen",
            "Content-Type: text/plain\r\n\r\nHi\r\n",
        ),
        false,
    );
    r.sync().await;
    assert_eq!(
        r.message("inbox", "Chosen").await["body"]["pgp"],
        json!({ "encrypted": true, "signed": true, "verified": true, "signer": BOB, "problem": null })
    );
}

#[tokio::test]
async fn a_passphrase_protected_key_is_imported_unlocked() {
    let r = rig().await;
    let mut key = mailcore::pgp::generate_key("Me", ME).unwrap();
    let password: Password = "correct horse".into();
    key.primary_key
        .set_password(rand::thread_rng(), &password)
        .unwrap();
    for sub in &mut key.secret_subkeys {
        sub.key.set_password(rand::thread_rng(), &password).unwrap();
    }
    let armored = key.to_armored_string(ArmorOptions::default()).unwrap();

    let (code, message) = r
        .fails(
            "pgp_import_secret",
            json!({ "account_id": r.account, "armored": armored }),
        )
        .await;
    assert_eq!(code, "invalid");
    assert!(message.contains("protected by a passphrase"), "{message}");
    let (_, message) = r
        .fails(
            "pgp_import_secret",
            json!({ "account_id": r.account, "armored": armored, "passphrase": "wrong" }),
        )
        .await;
    assert!(message.contains("passphrase isn't right"), "{message}");

    // A key for another address doesn't fit this inbox.
    let other = bob_key();
    let (_, message) = r
        .fails(
            "pgp_import_secret",
            json!({ "account_id": r.account,
                    "armored": other.to_armored_string(ArmorOptions::default()).unwrap() }),
        )
        .await;
    assert!(message.contains("This key is for bob@x.test"), "{message}");

    let status = r
        .call(
            "pgp_import_secret",
            json!({ "account_id": r.account, "armored": armored, "passphrase": "correct horse" }),
        )
        .await;
    assert_eq!(status["has_key"], true);
    assert_eq!(status["fingerprint"], print(&key));

    // Saved without the passphrase: mail to it opens with nothing asked.
    let bob = bob_key();
    r.imap.add_message(
        "INBOX",
        &encrypted_mail(
            &key.to_public_key(),
            &bob,
            "Imported",
            "Content-Type: text/plain\r\n\r\nWorks\r\n",
        ),
        false,
    );
    r.sync().await;
    let shown = r.message("inbox", "Imported").await;
    assert!(shown["body"]["text"].as_str().unwrap().contains("Works"));
    assert_eq!(shown["body"]["pgp"]["encrypted"], true);
    // Bob's key isn't known here, so the signature can't be checked yet.
    assert_eq!(shown["body"]["pgp"]["verified"], false);
    assert!(shown["body"]["pgp"]["problem"]
        .as_str()
        .unwrap()
        .contains("don't have the sender's key"));
}

// ---- Encrypted mail never reaches AI ----

/// The words inside the encrypted email, and its attachment.
const SECRET_WORDS: [&str; 3] = ["Meet at noon", "Bring the map", "X marks the spot"];
/// Mail that arrives after the first look, so it counts as new.
const NEW_MAIL: &str = "01-Nov-2036 10:00:00 +0000";

/// An inbox with a key, one encrypted email from Bob ("Secret") and one plain one from
/// Ana ("Plans"), both new, with AI connected.
async fn secret_rig() -> Rig {
    let r = rig().await;
    r.generate().await;
    r.sync().await;
    let mine = r.my_public_key().await;
    let bob = bob_key();
    let inner = "Content-Type: multipart/mixed; boundary=\"in\"\r\n\r\n--in\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon. Bring the map.\r\n--in\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"map.txt\"\r\n\r\nX marks the spot\r\n--in--\r\n";
    r.imap.add_message_dated(
        "INBOX",
        &encrypted_mail(&mine, &bob, "Secret", inner),
        false,
        NEW_MAIL,
    );
    r.imap.add_message_dated(
        "INBOX",
        &format!(
            "From: Ana <ana@x.test>\r\nTo: {ME}\r\nSubject: Plans\r\nMessage-ID: <plans@x.test>\r\nDate: Tue, 03 Nov 2026 10:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nLunch at the noon market on Friday?\r\n"
        ),
        false,
        NEW_MAIL,
    );
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    r.sync().await;
    r
}

impl Rig {
    /// Whether anything sent to the AI so far holds the encrypted email's words.
    /// Whether the person's own AI, or Mailzo Cloud's, was ever sent the secret's words.
    fn ai_saw_secret(&self) -> bool {
        let to_cloud = self.paid.everything_sent();
        self.ai.requests().iter().any(|q| {
            let q = q.to_string();
            SECRET_WORDS.iter().any(|w| q.contains(w))
        }) || SECRET_WORDS.iter().any(|w| to_cloud.contains(w))
    }

    fn ai_was_sent(&self, words: &str) -> bool {
        self.ai
            .requests()
            .iter()
            .any(|q| q.to_string().contains(words))
    }

    fn cloud_was_sent(&self, words: &str) -> bool {
        self.paid.everything_sent().contains(words)
    }
}

#[tokio::test]
async fn asking_ai_about_an_encrypted_email_is_refused() {
    let r = secret_rig().await;
    let shown = r.message("inbox", "Secret").await;
    assert_eq!(shown["body"]["pgp"]["encrypted"], true, "{shown}");
    assert_eq!(
        shown["risk"].as_i64().unwrap_or(0),
        0,
        "not held back as suspicious"
    );
    let thread = json!({ "account_id": r.account, "thread_id": shown["thread_id"] });
    let asked = r.ai.requests().len();
    for cmd in ["ai_summarize", "ai_explain"] {
        let (code, message) = r.fails(cmd, thread.clone()).await;
        assert_eq!(code, "ai_blocked", "{cmd}: {message}");
        assert!(message.contains("encrypted"), "{cmd}: {message}");
    }
    let (code, message) = r
        .fails(
            "ai_write_reply",
            json!({ "message_id": shown["id"], "instruction": "say yes" }),
        )
        .await;
    assert_eq!(
        (code.as_str(), message.contains("encrypted")),
        ("ai_blocked", true)
    );
    let (code, _) = r
        .fails(
            "ai_translate",
            json!({ "message_id": shown["id"], "language": "French" }),
        )
        .await;
    assert_eq!(code, "ai_blocked");
    // Opening it only looks for a saved summary, and quietly finds none.
    let mut opening = thread.clone();
    opening["saved_only"] = json!(true);
    assert_eq!(r.call("ai_summarize", opening.clone()).await, Value::Null);
    assert_eq!(r.call("ai_explain", opening).await, Value::Null);
    assert_eq!(r.ai.requests().len(), asked, "nothing was sent at all");
    assert!(!r.ai_saw_secret());

    // A plain email still works.
    let plain = r.message("inbox", "Plans").await;
    r.call(
        "ai_summarize",
        json!({ "account_id": r.account, "thread_id": plain["thread_id"] }),
    )
    .await;
    assert!(r.ai_was_sent("Lunch at the noon market"));
    assert!(!r.ai_saw_secret());
}

#[tokio::test]
async fn ask_ai_and_the_brief_leave_out_encrypted_words() {
    let r = secret_rig().await;
    let secret = r.message("inbox", "Secret").await;
    let plain = r.message("inbox", "Plans").await;

    // Both hold "noon", but only the plain one is offered as a source.
    let found = r
        .call(
            "ai_ask_sources",
            json!({ "question": "what about noon", "filters": { "words": ["noon"] } }),
        )
        .await;
    let ids: Vec<i64> = found["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert!(ids.contains(&plain["id"].as_i64().unwrap()), "{found}");
    assert!(!ids.contains(&secret["id"].as_i64().unwrap()), "{found}");

    // Even when asked by its id, its words aren't sent.
    r.paid.answer_ask(json!({
        "headline": "Lunch.",
        "items": [{ "id": plain["id"], "title": "Lunch", "value": "" }, { "id": secret["id"], "title": "Made up", "value": "" }],
    }));
    let answer = r
        .call(
            "ai_ask",
            json!({ "question": "what about noon", "ids": [secret["id"], plain["id"]] }),
        )
        .await;
    assert_eq!(answer["items"].as_array().unwrap().len(), 1, "{answer}");
    let asked = r.paid.sent("ask");
    let given: Vec<&Value> = asked[0]["sources"].as_array().unwrap().iter().collect();
    assert_eq!(given.len(), 1, "{asked:?}");
    assert_eq!(given[0]["id"], plain["id"]);
    assert!(r.cloud_was_sent("Lunch at the noon market"));
    assert!(!r.ai_saw_secret());

    // The brief names the encrypted email by sender and subject only.
    for t in [&secret, &plain] {
        r.call(
            "thread_priority",
            json!({ "account_id": r.account, "thread_id": t["thread_id"], "group": "needs_you" }),
        )
        .await;
    }
    r.paid.answer_brief("Bob and Ana wrote.");
    let brief = r.call("ai_brief", json!({})).await;
    assert_eq!(brief["text"], "Bob and Ana wrote.", "{brief}");
    let items = r.paid.sent("brief")[0]["items"].clone();
    let secret_item = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["subject"] == "Secret")
        .unwrap_or_else(|| panic!("{items}"))
        .clone();
    assert_eq!(secret_item["line"], "", "{items}");
    assert!(
        items.to_string().contains("Lunch at the noon market"),
        "{items}"
    );
    assert!(!r.ai_saw_secret());
}

#[tokio::test]
async fn ai_sorting_sees_only_who_and_what_about() {
    let r = rig().await;
    r.generate().await;
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    r.call(
        "set_setting",
        json!({ "key": "ai_auto", "value": "everything" }),
    )
    .await;
    r.sync().await;
    let mine = r.my_public_key().await;
    let inner = "Content-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon. Bring the map.\r\n";
    r.imap.add_message_dated(
        "INBOX",
        &encrypted_mail(&mine, &bob_key(), "Secret", inner),
        false,
        NEW_MAIL,
    );
    r.sync().await;
    let sorted = r.paid.sent("sort");
    assert!(
        sorted
            .iter()
            .flat_map(|s| s["items"].as_array().unwrap().clone())
            .any(|i| i["subject"] == "Secret" && i["text"] == ""),
        "it was sorted by who and what about: {sorted:?}"
    );
    assert!(!r.ai_saw_secret());
}

#[tokio::test]
async fn ai_apps_never_get_an_encrypted_emails_words_or_files() {
    let r = secret_rig().await;
    for key in ["mcp_on", "mcp_attachments"] {
        r.call("set_setting", json!({ "key": key, "value": "on" }))
            .await;
    }
    let tool = |name: &str, args: Value| {
        let engine = &r.engine;
        let message = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });
        async move { engine.mcp_message(message).await.unwrap()["result"].clone() }
    };
    let text = |result: &Value| result["content"][0]["text"].as_str().unwrap().to_string();
    let secret = r.message("inbox", "Secret").await;

    // Reading the conversation gives who, what about and when, and nothing else.
    let read = tool(
        "read_conversation",
        json!({ "account_id": r.account, "thread_id": secret["thread_id"] }),
    )
    .await;
    assert_ne!(read["isError"], true, "{read}");
    let mail: Value = serde_json::from_str(&text(&read)).unwrap();
    assert_eq!(mail[0]["subject"], "Secret");
    assert_eq!(mail[0]["encrypted"], true);
    assert_eq!(mail[0]["text"], "");
    assert_eq!(mail[0]["attachments"], json!([]));
    assert!(!SECRET_WORDS.iter().any(|w| read.to_string().contains(w)));

    // The newest mail lists it without a preview.
    let recent = tool("recent_mail", json!({})).await;
    let list: Value = serde_json::from_str(&text(&recent)).unwrap();
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["subject"] == "Secret")
        .unwrap_or_else(|| panic!("{list}"));
    assert_eq!(row["preview"], "");
    assert!(!SECRET_WORDS.iter().any(|w| recent.to_string().contains(w)));

    // Searching its words doesn't find it, so they can't be guessed at either.
    let found = tool("search_mail", json!({ "query": "noon" })).await;
    let list: Value = serde_json::from_str(&text(&found)).unwrap();
    let subjects: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["subject"].as_str())
        .collect();
    assert_eq!(subjects, ["Plans"], "{list}");

    // Its attachment can't be opened.
    let part = secret["body"]["attachments"][0]["part"].clone();
    assert!(part.is_number(), "{secret}");
    let file = tool(
        "read_attachment",
        json!({ "message_id": secret["id"], "part": part }),
    )
    .await;
    assert_eq!(file["isError"], true, "{file}");
    assert!(!file.to_string().contains("X marks the spot"));
}

/// Plain mail with a signed OpenPGP message pasted into its text, and other text around it.
fn inline_signed(signer: &SignedSecretKey, subject: &str, inside: &str, outside: &str) -> String {
    let mut rng = rand::thread_rng();
    let mut b = MessageBuilder::from_bytes("", inside.as_bytes().to_vec());
    b.sign(
        &signer.primary_key,
        Password::empty(),
        HashAlgorithm::Sha256,
    );
    let armored = b
        .to_armored_string(&mut rng, ArmorOptions::default())
        .unwrap()
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    format!(
        "From: Bob <{BOB}>\r\nTo: {ME}\r\nSubject: {subject}\r\nDate: Tue, 03 Nov 2026 09:00:00 +0000\r\nMessage-ID: <{subject}@x.test>\r\nContent-Type: text/plain\r\n\r\n{outside}\r\n{armored}\r\n"
    )
}

#[tokio::test]
async fn text_outside_an_inline_signed_block_is_not_badged() {
    let r = rig().await;
    let bob = bob_key();
    r.call("pgp_key_import", json!({ "armored": armored_public(&bob) }))
        .await;
    r.imap.add_message(
        "INBOX",
        &inline_signed(&bob, "Whole", "All of this is signed.", ""),
        false,
    );
    r.imap.add_message(
        "INBOX",
        &inline_signed(
            &bob,
            "Part",
            "Lunch on Friday?",
            "New bank details: pay to account 999.",
        ),
        false,
    );
    r.sync().await;
    let whole = &r.message("inbox", "Whole").await["body"];
    assert_eq!(whole["pgp"]["verified"], true, "{whole}");
    assert!(whole["pgp"].get("partial").is_none(), "{whole}");

    let part = &r.message("inbox", "Part").await["body"];
    let pgp = &part["pgp"];
    assert_eq!(pgp["partial"], true, "{pgp}");
    assert_eq!(pgp["verified"], false, "{pgp}");
    assert_eq!(
        pgp["problem"],
        "Only part of this email is signed. Text outside the signed part could have been added by anyone."
    );
    // The signed words come first; the rest is set apart under a plain marker.
    let text = part["text"].as_str().unwrap();
    let marker = text.find("----- Not signed").unwrap();
    assert!(text[..marker].contains("Lunch on Friday?"), "{text}");
    assert!(text[marker..].contains("pay to account 999"), "{text}");
}

#[tokio::test]
async fn replies_to_encrypted_mail_stay_encrypted_and_off_the_server() {
    let r = rig().await;
    r.imap.add_folder("Drafts", &["\\Drafts"], 1);
    r.call(
        "set_setting",
        json!({ "key": "drafts_on_server", "value": "on" }),
    )
    .await;
    r.generate().await;
    let bob = bob_key();
    r.imap.add_message(
        "INBOX",
        &mail_with_autocrypt(&bob, "a1@x.test", "Mon, 02 Nov 2026 09:00:00 +0000"),
        false,
    );
    r.sync().await;
    let mine = r.my_public_key().await;
    r.imap.add_message(
        "INBOX",
        &encrypted_mail(
            &mine,
            &bob,
            "Secret",
            "Content-Type: text/plain\r\n\r\nMeet at noon.\r\n",
        ),
        false,
    );
    r.sync().await;
    let parent = r.message("inbox", "Secret").await;

    let d = r
        .call(
            "draft_new",
            json!({ "account_id": r.account, "mode": "forward", "message_id": parent["id"] }),
        )
        .await;
    assert_eq!(
        (&d["encrypt"], &d["sign"]),
        (&json!(true), &json!(true)),
        "{d}"
    );
    assert_eq!(d["from_encrypted"], true);
    assert!(d["quoted_text"].as_str().unwrap().contains("Meet at noon"));

    // Turned off, it still never goes to the server's Drafts.
    r.call(
        "draft_save",
        json!({
            "id": d["id"], "account_id": r.account, "encrypt": false,
            "to": [{ "name": "", "address": "dave@x.test" }],
            "subject": "Fwd: Secret", "html": "<p>See below</p>", "text": "See below",
        }),
    )
    .await;
    r.call("draft_age_for_tests", json!({})).await;
    r.call("due_tick", json!({})).await;
    assert!(r.imap.folder_messages("Drafts").is_empty());

    // Sending it unencrypted needs the person to say so.
    let (code, message) = r.fails("send_draft", json!({ "id": d["id"] })).await;
    assert_eq!(code, "plain_reply_to_encrypted", "{message}");
    assert!(r.smtp.received().is_empty());
    let sent = r
        .call("send_draft", json!({ "id": d["id"], "allow_plain": true }))
        .await;
    assert_eq!(sent["sent"], true, "{sent}");
    assert!(r.smtp.received()[0].data.contains("Meet at noon"));
}
