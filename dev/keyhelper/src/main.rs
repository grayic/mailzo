//! For builds on this Mac only, until the app has its Apple signature.
//!
//! macOS ties Keychain permission to the exact program, so every rebuild of an app
//! without an Apple signature gets asked again. This program holds the Keychain
//! access and is never rebuilt, so the app can change freely. Apple-signed builds
//! don't use it: they talk to the Keychain directly and macOS keeps trusting them.
//!
//!   inbox-keyhelper SERVICE get NAME        prints the secret, exit 3 if none
//!   inbox-keyhelper SERVICE set NAME        reads the secret from standard input
//!   inbox-keyhelper SERVICE delete NAME

use mailcore::{KeychainStore, SecretStore};
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [service, action, name] = args.as_slice() else {
        eprintln!("usage: inbox-keyhelper SERVICE get|set|delete NAME");
        std::process::exit(2);
    };
    let store = KeychainStore::new(service);
    let result = match action.as_str() {
        "get" => store.get(name).map(|found| match found {
            Some(secret) => {
                let _ = std::io::stdout().write_all(secret.as_bytes());
            }
            None => std::process::exit(3),
        }),
        "set" => {
            let mut secret = String::new();
            if std::io::stdin().read_to_string(&mut secret).is_err() {
                std::process::exit(2);
            }
            store.set(name, &secret)
        }
        "delete" => store.delete(name),
        _ => std::process::exit(2),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
