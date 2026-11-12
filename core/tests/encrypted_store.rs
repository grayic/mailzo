//! The mail database is encrypted on disk, and mail saved before that moves over safely.

use mailcore::model::{Addr, Body, Security};
use mailcore::parse::ParsedHeaders;
use mailcore::search::SearchFilters;
use mailcore::store::{IncomingMessage, Opening, StoreFile, STORE_KEY};
use mailcore::{CoreError, MemoryStore, SecretStore, Store};
use std::path::{Path, PathBuf};

const SUBJECT: &str = "Quarterly zanzibarite shipment";
const BODY: &str = "The vault combination is marmalade-falcon-7731.";
const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const OTHER_KEY: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

fn message(uid: u32, subject: &str) -> IncomingMessage {
    IncomingMessage {
        uid,
        is_draft: false,
        seen: false,
        flagged: false,
        size: 1_000,
        internal_date: Some(1_700_000_000 + i64::from(uid)),
        signals: None,
        headers: ParsedHeaders {
            rfc_id: Some(format!("{uid}@encrypted.test")),
            subject: subject.into(),
            from: Some(Addr {
                name: "Sender".into(),
                address: "sender@encrypted.test".into(),
            }),
            to: vec![Addr {
                name: "Me".into(),
                address: "me@encrypted.test".into(),
            }],
            ..Default::default()
        },
    }
}

fn body(text: &str) -> Body {
    Body {
        text: Some(text.into()),
        html: None,
        attachments: vec![],
        pgp: None,
    }
}

/// An unencrypted database as older versions left it, with an account and two emails.
fn plain_store_with_mail(path: &Path) {
    let store = Store::open(path).unwrap();
    let account = store
        .insert_account(
            "me@encrypted.test",
            "Me",
            "imap.test",
            993,
            Security::Tls,
            "me",
        )
        .unwrap();
    let folders = store
        .replace_folders(
            account.id,
            &[("INBOX".into(), "Inbox".into(), "inbox".into())],
        )
        .unwrap();
    let inbox = folders[0].0;
    store
        .insert_found_messages(
            account.id,
            inbox,
            &[message(1, SUBJECT), message(2, "Lunch")],
        )
        .unwrap();
    store.save_body_by_uid(inbox, 1, &body(BODY)).unwrap();
    store
        .save_body_by_uid(inbox, 2, &body("Sandwiches at noon."))
        .unwrap();
}

fn find(store: &Store, word: &str) -> Vec<String> {
    let f = SearchFilters {
        words: vec![word.into()],
        ..Default::default()
    };
    let (found, _) = store.search_threads(&f, &[], 50, true).unwrap();
    found.into_iter().map(|t| t.subject).collect()
}

/// Everything on disk for this database, side files included.
fn bytes_on_disk(path: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        if let Ok(b) = std::fs::read(PathBuf::from(name)) {
            all.extend(b);
        }
    }
    all
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

fn assert_mail_is_there(store: &Store) {
    assert_eq!(store.list_accounts().unwrap().len(), 1);
    assert_eq!(find(store, "zanzibarite"), vec![SUBJECT.to_string()]);
    assert_eq!(find(store, "marmalade"), vec![SUBJECT.to_string()]);
    assert_eq!(find(store, "sandwiches"), vec!["Lunch".to_string()]);
}

#[test]
fn unencrypted_mail_moves_into_an_encrypted_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);
    assert_eq!(Store::file_state(&path).unwrap(), StoreFile::Plain);
    assert!(contains(&bytes_on_disk(&path), "marmalade-falcon"));

    let secrets = MemoryStore::default();
    let opened = Store::open_protected(&path, &secrets).unwrap();
    assert_eq!(opened.opening, Opening::Migrated);
    let key = secrets.get(STORE_KEY).unwrap().unwrap();
    assert_eq!(key.len(), 64);
    assert_mail_is_there(&opened.store);

    // New mail goes through the search index's triggers as before.
    let account = opened.store.list_accounts().unwrap()[0].id;
    let inbox = opened.store.list_folders(account).unwrap()[0].id;
    opened
        .store
        .insert_found_messages(account, inbox, &[message(3, "Pelican invoice")])
        .unwrap();
    opened
        .store
        .save_body_by_uid(inbox, 3, &body("Pay the pelicans."))
        .unwrap();
    assert_eq!(find(&opened.store, "pelican"), vec!["Pelican invoice"]);
    drop(opened);

    assert_eq!(Store::file_state(&path).unwrap(), StoreFile::Encrypted);
    let disk = bytes_on_disk(&path);
    for secret in [
        "marmalade-falcon",
        "zanzibarite",
        "Sandwiches",
        "Pelican",
        "pelicans",
    ] {
        assert!(!contains(&disk, secret), "{secret} is readable in the file");
    }
    // Nothing of the old file or the copy is left beside it.
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "mail.db" && n != "mail.db-wal" && n != "mail.db-shm")
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");

    // Opens again with the same key, and only with it.
    let again = Store::open_protected(&path, &secrets).unwrap();
    assert_eq!(again.opening, Opening::Encrypted);
    assert_mail_is_there(&again.store);
    drop(again);
    assert!(Store::open_with_key(&path, &key).is_ok());
    assert!(Store::open(&path).is_err());
    match Store::open_with_key(&path, OTHER_KEY) {
        Err(CoreError::Invalid(why)) => assert_eq!(why, "The store's key is wrong."),
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("opened with the wrong key"),
    }
}

#[test]
fn a_new_database_is_encrypted_from_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    let secrets = MemoryStore::default();
    let opened = Store::open_protected(&path, &secrets).unwrap();
    assert_eq!(opened.opening, Opening::Encrypted);
    opened
        .store
        .insert_account(
            "me@encrypted.test",
            "Me",
            "imap.test",
            993,
            Security::Tls,
            "me",
        )
        .unwrap();
    drop(opened);
    assert_eq!(Store::file_state(&path).unwrap(), StoreFile::Encrypted);
    assert!(!contains(&bytes_on_disk(&path), "me@encrypted.test"));
    let again = Store::open_protected(&path, &secrets).unwrap();
    assert_eq!(again.store.list_accounts().unwrap().len(), 1);
}

#[test]
fn mail_still_in_the_side_file_comes_along() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);
    // Left open without closing, as when the app is forced to quit: the newest writes are
    // still only in the -wal file.
    let busy = Store::open(&path).unwrap();
    busy.insert_account(
        "two@encrypted.test",
        "Two",
        "imap.test",
        993,
        Security::Tls,
        "two",
    )
    .unwrap();
    std::mem::forget(busy);

    Store::encrypt_file(&path, KEY).unwrap();
    let store = Store::open_with_key(&path, KEY).unwrap();
    assert_eq!(store.list_accounts().unwrap().len(), 2);
    assert_eq!(find(&store, "marmalade"), vec![SUBJECT.to_string()]);
}

#[test]
fn a_failed_move_leaves_the_unencrypted_database_working() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);
    // Something in the way of the encrypted copy, so it can't be made.
    let blocker = dir.path().join("mail.db.encrypting");
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("x"), "x").unwrap();

    let secrets = MemoryStore::default();
    let opened = Store::open_protected(&path, &secrets).unwrap();
    assert!(
        matches!(opened.opening, Opening::LeftUnencrypted(_)),
        "{:?}",
        opened.opening
    );
    assert_mail_is_there(&opened.store);
    drop(opened);
    assert_eq!(Store::file_state(&path).unwrap(), StoreFile::Plain);

    // Next time, with the way clear, it moves over with the key saved the first time.
    let key = secrets.get(STORE_KEY).unwrap().unwrap();
    std::fs::remove_dir_all(&blocker).unwrap();
    let opened = Store::open_protected(&path, &secrets).unwrap();
    assert_eq!(opened.opening, Opening::Migrated);
    assert_eq!(secrets.get(STORE_KEY).unwrap().unwrap(), key);
    assert_mail_is_there(&opened.store);
}

#[test]
fn a_bad_key_never_touches_the_unencrypted_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);
    assert!(Store::encrypt_file(&path, "not-a-key").is_err());
    assert_eq!(Store::file_state(&path).unwrap(), StoreFile::Plain);
    assert_mail_is_there(&Store::open(&path).unwrap());
}

#[test]
fn a_missing_key_keeps_the_old_mail_aside_and_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);
    let first = MemoryStore::default();
    drop(Store::open_protected(&path, &first).unwrap());
    let old_key = first.get(STORE_KEY).unwrap().unwrap();

    // The Keychain item is gone.
    let emptied = MemoryStore::default();
    let opened = Store::open_protected(&path, &emptied).unwrap();
    let Opening::StartedFresh(moved) = opened.opening.clone() else {
        panic!("{:?}", opened.opening);
    };
    assert!(opened.store.list_accounts().unwrap().is_empty());
    drop(opened);
    // Kept, not deleted, and still readable should the old key ever come back.
    let kept = Store::open_with_key(&moved, &old_key).unwrap();
    assert_mail_is_there(&kept);
    assert_ne!(emptied.get(STORE_KEY).unwrap().unwrap(), old_key);
}

#[test]
fn a_key_from_somewhere_else_keeps_the_old_mail_aside_and_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);
    Store::encrypt_file(&path, KEY).unwrap();
    let secrets = MemoryStore::default();
    secrets.set(STORE_KEY, OTHER_KEY).unwrap();
    let opened = Store::open_protected(&path, &secrets).unwrap();
    let Opening::StartedFresh(moved) = opened.opening.clone() else {
        panic!("{:?}", opened.opening);
    };
    assert!(opened.store.list_accounts().unwrap().is_empty());
    assert_mail_is_there(&Store::open_with_key(&moved, KEY).unwrap());
}

/// A Keychain that can't be read, as when the person says no to the Mac's question.
struct Refusing;

impl SecretStore for Refusing {
    fn set(&self, _: &str, _: &str) -> mailcore::Result<()> {
        Err(CoreError::Secret("refused".into()))
    }
    fn get(&self, _: &str) -> mailcore::Result<Option<String>> {
        Err(CoreError::Secret("refused".into()))
    }
    fn delete(&self, _: &str) -> mailcore::Result<()> {
        Err(CoreError::Secret("refused".into()))
    }
}

#[test]
fn an_unreadable_keychain_never_loses_mail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.db");
    plain_store_with_mail(&path);

    // Unencrypted mail just stays as it is for now.
    let opened = Store::open_protected(&path, &Refusing).unwrap();
    assert!(matches!(opened.opening, Opening::LeftUnencrypted(_)));
    assert_mail_is_there(&opened.store);
    drop(opened);

    // Encrypted mail can't be opened, and is left exactly where it is.
    Store::encrypt_file(&path, KEY).unwrap();
    match Store::open_protected(&path, &Refusing) {
        Err(CoreError::Invalid(why)) => assert!(why.contains("Nothing has been deleted"), "{why}"),
        Err(e) => panic!("wrong error: {e}"),
        Ok(o) => panic!("opened: {:?}", o.opening),
    }
    assert_mail_is_there(&Store::open_with_key(&path, KEY).unwrap());
}
