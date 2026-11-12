//! The real macOS Keychain, with a throwaway name. Runs on request:
//!
//!   cargo test -p mailcore --test keychain -- --ignored

use mailcore::{KeychainStore, SecretStore};

const SERVICE: &str = "com.grayic.inbox.keychain-test";

fn raw(name: &str) -> keyring::Entry {
    keyring::Entry::new(SERVICE, name).unwrap()
}

fn clean() {
    for name in ["secrets", "session", "vault-key", "account-1"] {
        let _ = raw(name).delete_credential();
    }
}

#[test]
#[ignore]
fn secrets_share_one_item_and_older_items_move_in() {
    clean();
    // Saved separately by an older version.
    raw("session").set_password("old-session").unwrap();

    let store = KeychainStore::new(SERVICE);
    store.set("vault-key", "the-key").unwrap();
    store.set("account-1", "pass").unwrap();
    assert_eq!(
        store.get("session").unwrap().as_deref(),
        Some("old-session")
    );
    assert_eq!(store.get("missing").unwrap(), None);
    assert!(
        raw("session").get_password().is_err(),
        "the old copy is gone"
    );

    // A new launch reads them all from the one item.
    let next = KeychainStore::new(SERVICE);
    assert_eq!(next.get("vault-key").unwrap().as_deref(), Some("the-key"));
    assert_eq!(next.get("account-1").unwrap().as_deref(), Some("pass"));
    assert_eq!(next.get("session").unwrap().as_deref(), Some("old-session"));
    assert!(
        raw("vault-key").get_password().is_err(),
        "only one item exists"
    );

    // Removing a secret also removes any older copy, so it can't come back.
    raw("account-1").set_password("stale").unwrap();
    next.delete("account-1").unwrap();
    assert_eq!(KeychainStore::new(SERVICE).get("account-1").unwrap(), None);

    // With nothing left, the item itself goes.
    next.delete("vault-key").unwrap();
    next.delete("session").unwrap();
    assert!(matches!(
        raw("secrets").get_password(),
        Err(keyring::Error::NoEntry)
    ));
    clean();
}

/// Needs the helper's path, and never asks for a password, because the helper made
/// the item itself and hasn't changed.
#[test]
#[ignore]
fn the_helper_stores_and_returns_secrets() {
    use mailcore::HelperStore;
    let helper = std::env::var("INBOX_SECRETS_HELPER").expect("the helper's path");
    let service = "com.grayic.inbox.helper-test";
    let store = HelperStore::new(&helper, service);
    store.set("session", "s p a c e s\nand lines").unwrap();
    store.set("vault-key", "the-key").unwrap();

    let next = HelperStore::new(&helper, service);
    assert_eq!(
        next.get("session").unwrap().as_deref(),
        Some("s p a c e s\nand lines")
    );
    assert_eq!(next.get("missing").unwrap(), None);
    next.delete("session").unwrap();
    next.delete("vault-key").unwrap();
    assert_eq!(
        HelperStore::new(&helper, service).get("session").unwrap(),
        None
    );
}
