//! Where secrets live. On a Mac that is the Keychain, never the database.

use crate::error::{CoreError, Result};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// Secrets are stored by name, such as `account-3` or `vault-key`.
pub trait SecretStore: Send + Sync {
    fn set(&self, name: &str, secret: &str) -> Result<()>;
    fn get(&self, name: &str) -> Result<Option<String>>;
    fn delete(&self, name: &str) -> Result<()>;
}

/// The name for an email account's password.
pub fn account_secret(account_id: i64) -> String {
    format!("account-{account_id}")
}

/// All of this app's secrets live together in one Keychain item, read once per
/// launch. macOS asks permission per item, so this means one approval at most,
/// never a run of them. Items saved separately by older versions move in here the
/// first time they're needed.
pub struct KeychainStore {
    service: String,
    loaded: Mutex<Option<BTreeMap<String, String>>>,
}

const COMBINED_ITEM: &str = "secrets";

fn keychain_error(e: keyring::Error) -> CoreError {
    CoreError::Secret(e.to_string())
}

impl KeychainStore {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
            loaded: Mutex::new(None),
        }
    }

    /// The names of the secrets kept, never the secrets themselves.
    pub fn names(&self) -> Result<Vec<String>> {
        self.with_secrets(|map| Ok(map.keys().cloned().collect()))
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, name).map_err(keychain_error)
    }

    /// Runs `work` on the secrets, reading them from the Keychain the first time.
    fn with_secrets<T>(
        &self,
        work: impl FnOnce(&mut BTreeMap<String, String>) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let map = match self.entry(COMBINED_ITEM)?.get_password() {
                Ok(json) => serde_json::from_str(&json)
                    .map_err(|_| CoreError::Secret("Saved secrets are damaged.".into()))?,
                Err(keyring::Error::NoEntry) => BTreeMap::new(),
                Err(e) => return Err(keychain_error(e)),
            };
            *guard = Some(map);
        }
        match guard.as_mut() {
            Some(map) => work(map),
            None => Err(CoreError::Secret("Secrets could not be read.".into())),
        }
    }

    fn save(&self, map: &BTreeMap<String, String>) -> Result<()> {
        let item = self.entry(COMBINED_ITEM)?;
        if map.is_empty() {
            return match item.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(keychain_error(e)),
            };
        }
        let json = serde_json::to_string(map)
            .map_err(|_| CoreError::Secret("Secrets could not be saved.".into()))?;
        item.set_password(&json).map_err(keychain_error)
    }
}

impl SecretStore for KeychainStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        self.with_secrets(|map| {
            let mut next = map.clone();
            next.insert(name.to_string(), secret.to_string());
            self.save(&next)?;
            *map = next;
            Ok(())
        })
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        self.with_secrets(|map| {
            if let Some(found) = map.get(name) {
                return Ok(Some(found.clone()));
            }
            // Saved separately by an older version, so move it in with the rest.
            match self.entry(name)?.get_password() {
                Ok(old) => {
                    let mut next = map.clone();
                    next.insert(name.to_string(), old.clone());
                    self.save(&next)?;
                    *map = next;
                    // The move is done, so the old copy isn't needed. If this fails,
                    // removing the secret later still clears it.
                    let _ = self.entry(name)?.delete_credential();
                    Ok(Some(old))
                }
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(keychain_error(e)),
            }
        })
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.with_secrets(|map| {
            let mut next = map.clone();
            next.remove(name);
            self.save(&next)?;
            *map = next;
            // Any older copy must go too, or it would come back.
            match self.entry(name)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(keychain_error(e)),
            }
        })
    }
}

/// Reaches the Keychain through a separate program that is never rebuilt, for builds
/// on this Mac until the app has its Apple signature. See `dev/keyhelper`.
pub struct HelperStore {
    helper: String,
    service: String,
    /// One request at a time, and each secret is fetched once per launch.
    known: Mutex<HashMap<String, Option<String>>>,
}

impl HelperStore {
    pub fn new(helper: &str, service: &str) -> Self {
        Self {
            helper: helper.to_string(),
            service: service.to_string(),
            known: Mutex::new(HashMap::new()),
        }
    }

    fn command(&self, action: &str, name: &str) -> std::process::Command {
        let mut c = std::process::Command::new(&self.helper);
        c.args([&self.service, action, name]);
        c
    }

    fn failed(output: &std::process::Output) -> CoreError {
        CoreError::Secret(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

impl SecretStore for HelperStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        use std::io::Write;
        use std::process::Stdio;
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let mut child = self
            .command("set", name)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| CoreError::Secret(e.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(secret.as_bytes())
                .map_err(|e| CoreError::Secret(e.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| CoreError::Secret(e.to_string()))?;
        if !output.status.success() {
            return Err(Self::failed(&output));
        }
        known.insert(name.to_string(), Some(secret.to_string()));
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = known.get(name) {
            return Ok(found.clone());
        }
        let output = self
            .command("get", name)
            .output()
            .map_err(|e| CoreError::Secret(e.to_string()))?;
        let found = match output.status.code() {
            Some(0) => Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            Some(3) => None,
            _ => return Err(Self::failed(&output)),
        };
        known.insert(name.to_string(), found.clone());
        Ok(found)
    }

    fn delete(&self, name: &str) -> Result<()> {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let output = self
            .command("delete", name)
            .output()
            .map_err(|e| CoreError::Secret(e.to_string()))?;
        if !output.status.success() {
            return Err(Self::failed(&output));
        }
        known.insert(name.to_string(), None);
        Ok(())
    }
}

/// Holds secrets in memory only. For tests and the demo mailbox.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), secret.into());
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}
