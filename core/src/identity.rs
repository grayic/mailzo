//! Signing in, linking new devices, and keeping the encrypted vault in step.
//!
//! The vault holds the list of email accounts and their passwords. It is
//! encrypted with a key only the person's devices hold. Linking a device means
//! passing that key across, encrypted for that device alone, after the person
//! confirms a number shown on the new device.

use crate::crypto;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::imap::ImapConnection;
use crate::model::{Account, AccountPrefs, Security, SmtpSecurity};
use crate::parse;
use crate::secrets::account_secret;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(crate) const SESSION: &str = "session";
const VAULT_KEY: &str = "vault-key";
const LINK_KEYS: &str = "link-keys";
/// What lets this device sign back in by itself after signing out.
const KEPT_DEVICE: &str = "kept-device";
const MAX_NUMBER_TRIES: u32 = 3;
const PUSH_TRIES: usize = 5;
/// Must match the email the account server sends.
/// The others are what the account server said under the app's earlier names, Dakly and Inbox.
const CODE_SUBJECTS: [&str; 3] = [
    "Your Mailzo sign-in code",
    "Your Dakly sign-in code",
    "Your Inbox sign-in code",
];
const CODE_LINE: &str = "Your sign-in code is ";
/// How many of the newest messages to look through in each folder.
const CODE_LOOK_BACK: u32 = 15;
/// Allows for a mail server whose clock is a little off.
const CODE_CLOCK_SLACK: i64 = 5 * 60;

/// How long the account server waits before starting an account fresh.
const FRESH_WAIT_SECONDS: i64 = 48 * 60 * 60;
/// When starting fresh, as booked from this device, is due.
const FRESH_DUE: &str = "fresh_due_at";

/// Deliberately has no Debug, because it holds passwords.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct VaultEntry {
    email: String,
    display_name: String,
    imap_host: String,
    imap_port: u16,
    security: Security,
    username: String,
    secret: String,
    // Older vaults don't have these fields.
    #[serde(default)]
    smtp_host: String,
    #[serde(default)]
    smtp_port: u16,
    #[serde(default = "default_smtp_security")]
    smtp_security: SmtpSecurity,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    settings_updated_at: i64,
    #[serde(default)]
    prefs: AccountPrefs,
    #[serde(default = "password_auth")]
    auth: String,
}

fn password_auth() -> String {
    crate::signin::PASSWORD.to_string()
}

fn default_smtp_security() -> SmtpSecurity {
    SmtpSecurity::Tls
}

impl VaultEntry {
    /// An account here, updated with what's in this entry.
    fn applied_to(&self, a: &Account) -> Account {
        let mut next = a.clone();
        next.display_name = self.display_name.clone();
        next.imap_host = self.imap_host.clone();
        next.imap_port = self.imap_port;
        next.security = self.security;
        next.username = self.username.clone();
        next.smtp_host = self.smtp_host.clone();
        next.smtp_port = self.smtp_port;
        next.smtp_security = self.smtp_security;
        next.signature = self.signature.clone();
        next.settings_updated_at = self.settings_updated_at;
        next.prefs = self.prefs.clone();
        next.auth = self.auth.clone();
        next
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Vault {
    version: i64,
    entries: BTreeMap<String, VaultEntry>,
    removed: BTreeMap<String, i64>,
    /// The person's choices that follow them between devices, and when they changed.
    #[serde(default)]
    settings: BTreeMap<String, String>,
    #[serde(default)]
    settings_at: i64,
    /// The keys to this person's teams, so all their devices are in them. For each team
    /// the newest choice wins, and leaving is a choice too.
    #[serde(default)]
    team_keys: Vec<crate::team::TeamKey>,
    /// Which teams were left, and when.
    #[serde(default)]
    teams_left: BTreeMap<i64, i64>,
    /// What a device from before several teams reads and writes: one key, the time of
    /// the last change, and the team last left.
    #[serde(default)]
    team_key: Option<crate::team::TeamKey>,
    #[serde(default)]
    team_key_at: i64,
    #[serde(default)]
    team_left: i64,
    /// Inboxes handed to the team. Other devices stop treating these as personal, and
    /// the team's list decides what happens next.
    #[serde(default)]
    team_moved: BTreeMap<String, i64>,
}

/// The new device's keys, kept until the link finishes.
#[derive(Serialize, Deserialize)]
pub(crate) struct LinkKeys {
    secret: String,
    public: String,
    salt: String,
}

impl LinkKeys {
    pub(crate) fn new() -> LinkKeys {
        let (secret, public) = crypto::device_keys();
        LinkKeys {
            secret,
            public,
            salt: crypto::random_hex(32),
        }
    }

    pub(crate) fn commitment(&self) -> String {
        crypto::commitment(&self.public, &self.salt)
    }
}

#[derive(Deserialize)]
pub(crate) struct VerifyReply {
    pub(crate) token: String,
    pub(crate) device_id: i64,
    pub(crate) has_vault: bool,
    #[serde(default)]
    pub(crate) returned: bool,
}

/// Kept in the secret store when signing out, one per account, and used up when
/// signing back in. Useless without a new sign-in code, and another device can
/// cancel it.
#[derive(Serialize, Deserialize)]
struct KeptDevice {
    email: String,
    device_id: i64,
    pass: String,
    vault_key: String,
}

#[derive(Deserialize)]
struct SelfReply {
    status: String,
    nonce: Option<String>,
    revealed: bool,
    sealed_key: Option<String>,
    #[serde(default)]
    other_devices: i64,
    #[serde(default)]
    fresh_after: i64,
}

#[derive(Deserialize)]
struct VaultReply {
    version: i64,
    blob: Option<String>,
}

#[derive(Deserialize)]
struct DeviceRow {
    id: i64,
    name: String,
    status: String,
    created_at: i64,
    commitment: String,
    nonce: Option<String>,
    public_key: Option<String>,
    salt: Option<String>,
    is_self: bool,
}

#[derive(Deserialize)]
struct DevicesReply {
    devices: Vec<DeviceRow>,
}

impl Engine {
    fn token(&self) -> Result<String> {
        self.secrets.get(SESSION)?.ok_or_else(|| CoreError::Cloud {
            code: "signed_out".into(),
            message: "You are signed out.".into(),
        })
    }

    pub(crate) async fn cloud_send<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        self.cloud_send_for_team(None, method, path, body).await
    }

    pub(crate) async fn cloud_send_for_team<T: serde::de::DeserializeOwned>(
        &self,
        team: Option<i64>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let token = self.token()?;
        let result = self
            .cloud()?
            .send_for_team(team, method, path, Some(&token), body.as_ref())
            .await;
        // The account removed this device, so its copy of the mail must go too.
        if let Err(CoreError::Cloud { code, .. }) = &result {
            if code == "session_revoked" {
                self.wipe_local()?;
            }
        }
        result
    }

    /// Removes everything this device holds for the signed-in person. Mail stays
    /// on the email servers, and the list of inboxes stays in the vault.
    ///
    /// A device that was never approved got nothing from the account, so anything
    /// on it was put there by the person at this computer, and it stays.
    pub(crate) fn wipe_local(&self) -> Result<()> {
        let approved = self.store.identity()?.filter(|i| i.state == "ready");
        self.wipe_this_copy()?;
        if let Some(identity) = approved {
            self.forget_kept(&identity.email)?;
        }
        Ok(())
    }

    fn wipe_this_copy(&self) -> Result<()> {
        let approved = self.store.identity()?.is_some_and(|i| i.state == "ready");
        if !approved {
            self.store.clear_identity()?;
            for name in [SESSION, VAULT_KEY, LINK_KEYS] {
                self.secrets.delete(name)?;
            }
            return Ok(());
        }
        for account in self.store.list_accounts()? {
            self.secrets.delete(&account_secret(account.id))?;
        }
        let staying: Vec<_> = self
            .store
            .all_settings()?
            .into_iter()
            .filter(|(key, _)| crate::settings::STAYS_ON_THIS_DEVICE.contains(&key.as_str()))
            .collect();
        self.store.wipe_identity_and_mail()?;
        for (key, value) in staying {
            self.store.set_setting(&key, &value)?;
        }
        self.remove_all_draft_files();
        for name in [
            SESSION,
            VAULT_KEY,
            LINK_KEYS,
            crate::ai::AI_KEY,
            crate::team::TEAM_KEY,
            crate::team::TEAM_KEYS,
        ] {
            self.secrets.delete(name)?;
        }
        Ok(())
    }

    pub(crate) fn auth_status(&self) -> Result<Value> {
        Ok(match self.store.identity()? {
            Some(i) => json!({
                "state": i.state, "email": i.email,
                "device_id": i.device_id, "device_name": i.device_name,
            }),
            None => json!({ "state": "signed_out" }),
        })
    }

    pub(crate) async fn auth_start(&self, email: &str) -> Result<Value> {
        let reply = self
            .cloud()?
            .send(
                Method::POST,
                "/auth/start",
                None,
                Some(&json!({ "email": email })),
            )
            .await?;
        *self.code_asked_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(unix_now());
        Ok(reply)
    }

    fn kept_devices(&self) -> Result<Vec<KeptDevice>> {
        Ok(self
            .secrets
            .get(KEPT_DEVICE)?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default())
    }

    fn save_kept(&self, list: &[KeptDevice]) -> Result<()> {
        if list.is_empty() {
            return self.secrets.delete(KEPT_DEVICE);
        }
        let raw = serde_json::to_string(list)
            .map_err(|_| CoreError::Secret("Could not save this device's key.".into()))?;
        self.secrets.set(KEPT_DEVICE, &raw)
    }

    fn forget_kept(&self, email: &str) -> Result<()> {
        let mut list = self.kept_devices()?;
        list.retain(|k| k.email != email);
        self.save_kept(&list)
    }

    /// Offered with every sign-in, as Google only says which account it is later.
    /// The server only looks at the ones belonging to that account.
    pub(crate) fn return_proofs(&self) -> Result<Value> {
        let list = self.kept_devices()?;
        Ok(json!(list
            .iter()
            .map(|k| json!({ "device_id": k.device_id, "pass": k.pass }))
            .collect::<Vec<_>>()))
    }

    pub(crate) async fn auth_verify(&self, email: &str, code: &str) -> Result<Value> {
        let keys = LinkKeys::new();
        let reply: VerifyReply = self
            .cloud()?
            .send(
                Method::POST,
                "/auth/verify",
                None,
                Some(&json!({
                    "email": email, "code": code, "device_name": self.device_name,
                    "commitment": keys.commitment(),
                    "returns": self.return_proofs()?,
                })),
            )
            .await?;
        self.begin_session(email, &reply, &keys).await
    }

    /// Saves a proven sign-in on this device. The address is known to be real by
    /// now, but the device still has to be set up or approved.
    pub(crate) async fn begin_session(
        &self,
        email: &str,
        reply: &VerifyReply,
        keys: &LinkKeys,
    ) -> Result<Value> {
        self.secrets.set(SESSION, &reply.token)?;
        self.secrets
            .set(LINK_KEYS, &serde_json::to_string(keys).unwrap_or_default())?;
        let state = if reply.has_vault {
            "linking"
        } else {
            "needs_setup"
        };
        let email = email.trim().to_ascii_lowercase();
        self.store
            .save_identity(&email, reply.device_id, &self.device_name, state)?;

        let Some(kept) = self.kept_devices()?.into_iter().find(|k| k.email == email) else {
            return Ok(json!({ "state": state }));
        };
        if !reply.returned {
            // Removed from another device, or the account started fresh.
            self.forget_kept(&email)?;
            return Ok(json!({ "state": state }));
        }
        self.come_back(&kept).await
    }

    /// The server has approved this device again, so it uses its kept key. The
    /// key isn't forgotten until it's safely in place.
    async fn come_back(&self, kept: &KeptDevice) -> Result<Value> {
        let finished = self.finish_link(&kept.vault_key).await;
        // Once approved, a failed refresh just waits for the next one.
        if finished.is_err() && !self.is_ready()? {
            finished?;
        }
        self.forget_kept(&kept.email)?;
        Ok(json!({ "state": "ready" }))
    }

    /// Looks in an inbox already connected on this device for the sign-in code
    /// we just emailed to it, so the person doesn't have to go and find it. The
    /// account server still checks the code, so a fake email can't sign anyone in.
    pub(crate) async fn find_signin_code(&self, email: &str) -> Result<Value> {
        let nothing = json!({ "code": null });
        let asked_at = *self.code_asked_at.lock().unwrap_or_else(|e| e.into_inner());
        let Some(asked_at) = asked_at else {
            return Ok(nothing);
        };
        if self.store.identity()?.is_some() {
            return Ok(nothing);
        }
        let wanted = email.trim();
        let accounts = self.store.list_accounts()?;
        let Some(account) = accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(wanted))
        else {
            return Ok(nothing);
        };
        let Ok(password) = self.password(account.id).await else {
            return Ok(nothing);
        };
        let code = newest_code(
            account,
            &password,
            asked_at - CODE_CLOCK_SLACK,
            self.reach(),
        )
        .await?;
        Ok(json!({ "code": code }))
    }

    /// First device on an account: creates the vault and the recovery key. The
    /// recovery key is returned this once and stored nowhere.
    pub(crate) async fn finish_setup(&self) -> Result<Value> {
        let recovery_key = crypto::new_recovery_key();
        let (proof, wrap) = crypto::recovery_parts(&recovery_key)?;
        let vault_key = crypto::random_hex(32);
        let (settings, settings_at) = self.shared_settings()?;
        let (team_keys, teams_left) = (self.team_keys()?, self.teams_left()?);
        let older = OlderTeamFields::from(&team_keys, &teams_left);
        let vault = Vault {
            version: 1,
            entries: self.local_entries()?,
            removed: self.store.tombstones()?,
            settings,
            settings_at,
            team_keys,
            teams_left,
            team_key: older.key,
            team_key_at: older.at,
            team_left: older.left,
            team_moved: self.team_moved()?,
        };
        let blob = self.seal_vault(&vault_key, &vault)?;
        let result: Result<Value> = self
            .cloud_send(
                Method::POST,
                "/vault/init",
                Some(json!({
                    "blob": blob,
                    "recovery_sealed": crypto::recovery_seal(&wrap, &vault_key)?,
                    "recovery_auth_hash": crypto::proof_hash(&proof),
                })),
            )
            .await;
        match result {
            Ok(_) => {}
            // Another device finished first, so this one links to it instead.
            Err(CoreError::Cloud { code, .. }) if code == "vault_exists" => {
                self.store.set_identity_state("linking")?;
                return Ok(json!({ "state": "linking" }));
            }
            Err(e) => return Err(e),
        }
        self.secrets.set(VAULT_KEY, &vault_key)?;
        self.secrets.delete(LINK_KEYS)?;
        self.store.set_vault_version(1)?;
        self.store.set_identity_state("ready")?;
        Ok(json!({ "state": "ready", "recovery_key": recovery_key }))
    }

    /// New device: moves the link along, and returns the number once it exists.
    pub(crate) async fn link_status(&self) -> Result<Value> {
        let me: SelfReply = match self.cloud_send(Method::GET, "/devices/self", None).await {
            Err(CoreError::Cloud { code, .. }) if code == "session_revoked" => {
                return Ok(json!({ "state": "signed_out" }));
            }
            other => other?,
        };
        if me.status != "pending" && me.status != "approved" {
            self.wipe_local()?;
            return Ok(json!({ "state": "signed_out" }));
        }
        // Approved as a returning device, but stopped before it could finish.
        if me.status == "approved" && me.sealed_key.is_none() {
            let email = self.store.identity()?.map(|i| i.email).unwrap_or_default();
            if let Some(kept) = self.kept_devices()?.into_iter().find(|k| k.email == email) {
                return self.come_back(&kept).await;
            }
        }
        let keys = self.link_keys()?;

        if let Some(sealed) = me.sealed_key {
            let vault_key = crypto::unseal(&keys.secret, &sealed)?;
            let vault_key = String::from_utf8(vault_key).map_err(|_| {
                CoreError::Invalid("The other device sent something unreadable.".into())
            })?;
            self.finish_link(&vault_key).await?;
            return Ok(json!({ "state": "ready" }));
        }

        let others = me.other_devices;
        let fresh_after = me.fresh_after;
        let Some(nonce) = me.nonce else {
            return Ok(json!({
                "state": "linking", "number": null,
                "other_devices": others, "fresh_after": fresh_after,
            }));
        };
        if !me.revealed {
            let _: Value = self
                .cloud_send(
                    Method::POST,
                    "/devices/self/reveal",
                    Some(json!({ "public_key": keys.public, "salt": keys.salt })),
                )
                .await?;
        }
        let number = crypto::link_number(&keys.public, &keys.salt, &nonce);
        Ok(json!({
            "state": "linking", "number": number,
            "other_devices": others, "fresh_after": fresh_after,
        }))
    }

    fn link_keys(&self) -> Result<LinkKeys> {
        let raw = self
            .secrets
            .get(LINK_KEYS)?
            .ok_or_else(|| CoreError::Cloud {
                code: "signed_out".into(),
                message: "Please sign in again.".into(),
            })?;
        serde_json::from_str(&raw).map_err(|_| CoreError::Secret("Saved keys are damaged.".into()))
    }

    async fn finish_link(&self, vault_key: &str) -> Result<()> {
        self.secrets.set(VAULT_KEY, vault_key)?;
        self.secrets.delete(LINK_KEYS)?;
        self.store.set_identity_state("ready")?;
        self.vault_sync().await?;
        Ok(())
    }

    pub(crate) async fn link_with_recovery(&self, recovery_key: &str) -> Result<Value> {
        let (proof, wrap) = crypto::recovery_parts(recovery_key)?;
        let reply: Value = self
            .cloud_send(Method::POST, "/recovery", Some(json!({ "auth": proof })))
            .await?;
        let sealed = reply["recovery_sealed"]
            .as_str()
            .ok_or_else(|| CoreError::Network("unexpected reply".into()))?;
        let vault_key = crypto::recovery_open(&wrap, sealed)?;
        self.finish_link(&vault_key).await?;
        Ok(json!({ "state": "ready" }))
    }

    /// Replaces a lost recovery key. The new one is returned this once.
    pub(crate) async fn new_recovery_key(&self) -> Result<Value> {
        let vault_key = self.vault_key()?;
        let recovery_key = crypto::new_recovery_key();
        let (proof, wrap) = crypto::recovery_parts(&recovery_key)?;
        let _: Value = self
            .cloud_send(
                Method::PUT,
                "/recovery",
                Some(json!({
                    "recovery_sealed": crypto::recovery_seal(&wrap, &vault_key)?,
                    "recovery_auth_hash": crypto::proof_hash(&proof),
                })),
            )
            .await?;
        Ok(json!({ "recovery_key": recovery_key }))
    }

    /// Existing device: lists new devices that are ready for their number.
    pub(crate) async fn pending_devices(&self) -> Result<Value> {
        if !self.is_ready()? {
            return Ok(json!([]));
        }
        let list: DevicesReply = self.cloud_send(Method::GET, "/devices", None).await?;
        let vault_key = self.vault_key()?;
        let mut ready = Vec::new();
        for d in list.devices.iter().filter(|d| d.status == "pending") {
            if d.nonce.is_none() {
                let nonce = crypto::new_link_nonce(&vault_key, d.id);
                // Saved before sending, so the number can be checked against
                // what this device chose and not what the server repeats back.
                self.store.save_link_nonce(d.id, &nonce)?;
                let sent: Result<Value> = self
                    .cloud_send(
                        Method::POST,
                        &format!("/devices/{}/nonce", d.id),
                        Some(json!({ "nonce": nonce })),
                    )
                    .await;
                match sent {
                    Ok(_) => {}
                    // It went ahead with a value set in the meantime. If one of the
                    // person's devices made it, the next look shows its number;
                    // otherwise it's refused then.
                    Err(CoreError::Cloud { code, .. }) if code == "nonce_taken" => {
                        self.store.delete_link_nonce(d.id)?;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            // The number both screens show, worked out here from this device's own random
            // value, so the person only has to see that they match and tap Allow.
            if let (Some(public), Some(salt)) = (&d.public_key, &d.salt) {
                let stored = self.store.link_nonce(d.id)?;
                let server = d.nonce.as_deref().or(stored.as_deref());
                // Only a value one of the person's own devices made; one the server
                // supplied could have been picked to make a fake key's number match.
                let Some(nonce) = trusted_link_nonce(&vault_key, d.id, stored.as_deref(), server)
                else {
                    // It showed its key against a value none of the person's devices
                    // made, so it can never be allowed. It has to start again.
                    let _ = self.reject_device(d.id).await;
                    continue;
                };
                ready.push(json!({
                    "id": d.id, "name": d.name, "created_at": d.created_at,
                    "number": crypto::link_number(public, salt, &nonce),
                }));
            }
        }
        Ok(json!(ready))
    }

    pub(crate) async fn approve_device(&self, id: i64, number: &str) -> Result<Value> {
        let vault_key = self.vault_key()?;
        let list: DevicesReply = self.cloud_send(Method::GET, "/devices", None).await?;
        let device = list
            .devices
            .into_iter()
            .find(|d| d.id == id && d.status == "pending")
            .ok_or_else(|| CoreError::NotFound("That device is no longer waiting.".into()))?;
        let (Some(public), Some(salt), Some(server_nonce)) =
            (&device.public_key, &device.salt, &device.nonce)
        else {
            return Err(CoreError::Invalid("That device isn't ready yet.".into()));
        };
        // The key must be the one promised before it could know our random value.
        if crypto::commitment(public, salt) != device.commitment {
            self.reject_device(id).await?;
            return Err(CoreError::Invalid(
                "That device failed a security check, so it was refused.".into(),
            ));
        }
        let stored = self.store.link_nonce(id)?;
        let Some(nonce) = trusted_link_nonce(&vault_key, id, stored.as_deref(), Some(server_nonce))
        else {
            // Its number was worked out from a value none of the person's devices made
            // (the server answers `nonce_taken` to a device that tries to set its own once
            // the waiting device has shown its key), so it can never be allowed.
            self.reject_device(id).await?;
            return Err(link_again());
        };
        let typed: String = number.chars().filter(char::is_ascii_digit).collect();
        if typed != crypto::link_number(public, salt, &nonce) {
            let tries = {
                let mut map = self.number_tries.lock().unwrap_or_else(|e| e.into_inner());
                let n = map.entry(id).or_insert(0);
                *n += 1;
                *n
            };
            if tries >= MAX_NUMBER_TRIES {
                self.reject_device(id).await?;
                return Err(CoreError::Invalid(
                    "Too many wrong numbers, so that device was refused.".into(),
                ));
            }
            return Err(CoreError::Invalid(
                "That number doesn't match. Check the number on the new device.".into(),
            ));
        }
        let sealed = crypto::seal_to(public, vault_key.as_bytes())?;
        let _: Value = self
            .cloud_send(
                Method::POST,
                &format!("/devices/{id}/approve"),
                Some(json!({ "sealed_key": sealed })),
            )
            .await?;
        self.store.delete_link_nonce(id)?;
        Ok(json!({ "approved": true }))
    }

    pub(crate) async fn reject_device(&self, id: i64) -> Result<Value> {
        let reply: Value = self
            .cloud_send(Method::POST, &format!("/devices/{id}/reject"), None)
            .await?;
        self.store.delete_link_nonce(id)?;
        Ok(reply)
    }

    pub(crate) async fn list_devices(&self) -> Result<Value> {
        let list: DevicesReply = self.cloud_send(Method::GET, "/devices", None).await?;
        let rows: Vec<Value> = list
            .devices
            .iter()
            .filter(|d| d.status == "approved" || d.status == "signed_out")
            .map(|d| {
                json!({
                    "id": d.id, "name": d.name, "created_at": d.created_at, "is_self": d.is_self,
                    "signed_out": d.status == "signed_out",
                })
            })
            .collect();
        Ok(json!(rows))
    }

    pub(crate) async fn remove_device(&self, id: i64) -> Result<Value> {
        self.cloud_send(Method::DELETE, &format!("/devices/{id}"), None)
            .await
    }

    /// Signs this device out and removes its saved mail and passwords, but keeps
    /// what it needs to come back with just a new sign-in code.
    pub(crate) async fn sign_out(&self) -> Result<Value> {
        let identity = match self.store.identity()? {
            Some(i) if i.state == "ready" => i,
            // Never got into the account, so there's nothing to keep.
            _ => return self.erase_device().await,
        };
        // Anything not yet saved to the account would otherwise be lost.
        let _ = self.vault_sync().await;
        // Removed from another device, which that refresh has just found out.
        if !self.is_ready()? {
            return Ok(json!({ "state": "signed_out" }));
        }
        let pass = crypto::random_hex(32);
        let email = identity.email;
        // Saved first, so the server can't forget a device that couldn't keep it.
        let mut kept = self.kept_devices()?;
        kept.retain(|k| k.email != email);
        kept.push(KeptDevice {
            email: email.clone(),
            device_id: identity.device_id,
            pass: pass.clone(),
            vault_key: self.vault_key()?,
        });
        self.save_kept(&kept)?;
        let result: Result<Value> = self
            .cloud_send(
                Method::POST,
                "/devices/self/sign-out",
                Some(json!({ "return_hash": crypto::proof_hash(&pass) })),
            )
            .await;
        match result {
            Ok(_) => {}
            // Removed from another device. That has already cleared everything.
            Err(CoreError::Cloud { code, .. }) if code == "session_revoked" => {
                return Ok(json!({ "state": "signed_out" }));
            }
            Err(e) => {
                self.forget_kept(&email)?;
                return Err(e);
            }
        }
        self.wipe_this_copy()?;
        Ok(json!({ "state": "signed_out" }))
    }

    /// For someone with no device and no recovery key: replaces what's saved on
    /// the account, which nobody can open now, with a new empty one.
    ///
    /// To keep the account safe the server doesn't do it straight away. The first ask
    /// books it two days ahead and emails a link that stops it; that answers
    /// `fresh_booked`, with when it's due. Asking again before then answers
    /// `fresh_waiting`, with when it's due if this device booked it.
    pub(crate) async fn start_fresh(&self) -> Result<Value> {
        let asked: Result<Value> = self.cloud_send(Method::POST, "/vault/reset", None).await;
        match asked {
            Ok(_) => {}
            Err(CoreError::Cloud { code, .. }) if code == "fresh_booked" => {
                let due = unix_now() + FRESH_WAIT_SECONDS;
                self.store.set_setting(FRESH_DUE, &due.to_string())?;
                return Ok(json!({ "state": "fresh_booked", "due_at": due }));
            }
            Err(CoreError::Cloud { code, .. }) if code == "fresh_waiting" => {
                let due = self
                    .store
                    .setting(FRESH_DUE)?
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|due| *due > unix_now());
                return Ok(json!({ "state": "fresh_waiting", "due_at": due }));
            }
            Err(e) => return Err(e),
        }
        self.store.delete_setting(FRESH_DUE)?;
        if let Some(identity) = self.store.identity()? {
            self.forget_kept(&identity.email)?;
        }
        self.store.set_identity_state("needs_setup")?;
        Ok(json!({ "state": "needs_setup" }))
    }

    /// Emails a fresh sign-in code to the account's address, which deleting the account
    /// asks for, so a device left signed in can't delete it alone.
    pub(crate) async fn account_delete_start(&self) -> Result<Value> {
        let identity = self
            .store
            .identity()?
            .filter(|i| i.state == "ready")
            .ok_or_else(|| CoreError::Invalid("Sign in to your Mailzo account first.".into()))?;
        self.auth_start(&identity.email).await
    }

    /// Deletes the Mailzo account: its devices, synced settings and list of inboxes,
    /// recovery key, teams and plan. Email stays with the email providers. This device
    /// carries on with no account, keeping its own inboxes and mail, as it would if it
    /// had never had one. Inboxes shared by a team go, as the teams are gone.
    pub(crate) async fn account_delete(&self, code: &str) -> Result<Value> {
        let identity = self
            .store
            .identity()?
            .filter(|i| i.state == "ready")
            .ok_or_else(|| CoreError::Invalid("Sign in to your Mailzo account first.".into()))?;
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.is_empty() {
            return Err(CoreError::Invalid("Enter the code we emailed you.".into()));
        }
        let _: Value = self
            .cloud_send(Method::DELETE, "/account", Some(json!({ "code": code })))
            .await?;
        for key in self.team_keys()? {
            self.clear_team_here(key.team_id)?;
        }
        for team in self.store.team_infos()? {
            self.clear_team_here(team.team_id)?;
        }
        for a in self.store.list_accounts()? {
            if a.prefs.team_shared {
                self.remove_shared_account(&a)?;
            }
        }
        self.forget_kept(&identity.email)?;
        self.store.clear_identity()?;
        for name in [
            SESSION,
            VAULT_KEY,
            LINK_KEYS,
            crate::team::TEAM_KEY,
            crate::team::TEAM_KEYS,
        ] {
            self.secrets.delete(name)?;
        }
        Ok(json!({ "deleted": true, "state": "signed_out" }))
    }

    /// Removes this device from the account and erases all it holds, so getting
    /// back in on it takes another device or the recovery key.
    pub(crate) async fn erase_device(&self) -> Result<Value> {
        if let Some(identity) = self.store.identity()? {
            // Needs the server, so the account forgets this device right away.
            let result: Result<Value> = self
                .cloud_send(
                    Method::DELETE,
                    &format!("/devices/{}", identity.device_id),
                    None,
                )
                .await;
            match result {
                Ok(_) => {}
                Err(CoreError::Cloud { code, .. }) if code == "session_revoked" => {}
                Err(e) => return Err(e),
            }
        }
        self.wipe_local()?;
        Ok(json!({ "state": "signed_out" }))
    }

    pub(crate) fn is_ready(&self) -> Result<bool> {
        Ok(self.cloud.is_some() && self.store.identity()?.is_some_and(|i| i.state == "ready"))
    }

    fn vault_key(&self) -> Result<String> {
        self.secrets
            .get(VAULT_KEY)?
            .ok_or_else(|| CoreError::Secret("This device's vault key is missing.".into()))
    }

    fn local_entries(&self) -> Result<BTreeMap<String, VaultEntry>> {
        let mut out = BTreeMap::new();
        for a in self.store.list_accounts()? {
            // Shared inboxes belong to the team's list, not this person's. Leaving the
            // team must not leave them behind.
            if a.prefs.team_shared {
                continue;
            }
            if let Some(secret) = self.secrets.get(&account_secret(a.id))? {
                out.insert(
                    a.sync_id.clone(),
                    VaultEntry {
                        email: a.email,
                        display_name: a.display_name,
                        imap_host: a.imap_host,
                        imap_port: a.imap_port,
                        security: a.security,
                        username: a.username,
                        secret,
                        smtp_host: a.smtp_host,
                        smtp_port: a.smtp_port,
                        smtp_security: a.smtp_security,
                        signature: a.signature,
                        settings_updated_at: a.settings_updated_at,
                        prefs: a.prefs,
                        auth: a.auth,
                    },
                );
            }
        }
        Ok(out)
    }

    fn seal_vault(&self, vault_key: &str, vault: &Vault) -> Result<String> {
        let plain = serde_json::to_vec(vault)
            .map_err(|_| CoreError::Invalid("Could not prepare the vault.".into()))?;
        crypto::vault_seal(vault_key, &plain)
    }

    /// Brings this device and the vault into step. Returns whether this device's
    /// list of accounts changed.
    pub(crate) async fn vault_sync(&self) -> Result<Value> {
        if !self.is_ready()? {
            return Ok(json!({ "changed": false }));
        }
        let vault_key = self.vault_key()?;
        let mut changed = false;
        for _ in 0..PUSH_TRIES {
            let reply: VaultReply = self.cloud_send(Method::GET, "/vault", None).await?;
            let remote = self.read_vault(&vault_key, &reply)?;
            let (merged, applied) = self.merge_and_apply(remote.as_ref())?;
            changed |= applied;
            let same_as_remote = remote.as_ref().is_some_and(|r| same_vault(r, &merged));
            self.store.set_vault_version(reply.version)?;
            if same_as_remote {
                return Ok(json!({ "changed": changed }));
            }
            let next = Vault {
                version: reply.version + 1,
                ..merged
            };
            let blob = self.seal_vault(&vault_key, &next)?;
            let put: Result<Value> = self
                .cloud_send(
                    Method::PUT,
                    "/vault",
                    Some(json!({ "expected_version": reply.version, "blob": blob })),
                )
                .await;
            match put {
                Ok(_) => {
                    self.store.set_vault_version(next.version)?;
                    return Ok(json!({ "changed": changed }));
                }
                // Another device saved first, so merge with its copy and retry.
                Err(CoreError::Cloud { code, .. }) if code == "vault_changed" => continue,
                Err(e) => return Err(e),
            }
        }
        Err(CoreError::Network(
            "Your devices are changing accounts at the same time. Try again shortly.".into(),
        ))
    }

    fn read_vault(&self, vault_key: &str, reply: &VaultReply) -> Result<Option<Vault>> {
        let Some(blob) = &reply.blob else {
            return Ok(None);
        };
        let plain = crypto::vault_open(vault_key, blob)?;
        let vault: Vault = serde_json::from_slice(&plain)
            .map_err(|_| CoreError::Invalid("The vault could not be read.".into()))?;
        // The version is inside the encryption, so the server can't lie about
        // it, or quietly restore an older copy with a removed inbox still in it.
        let seen = self.store.vault_version()?;
        if vault.version != reply.version || reply.version < seen {
            return Err(CoreError::Invalid(
                "The account server sent an out-of-date copy of your account list, so it was ignored.".into(),
            ));
        }
        Ok(Some(vault))
    }

    /// Which teams this person is in. Each team is settled by itself, so leaving one can
    /// never take away the key to another, however wrong a device's clock is.
    fn merge_team_keys(
        &self,
        remote: Option<&Vault>,
    ) -> Result<(Vec<crate::team::TeamKey>, BTreeMap<i64, i64>)> {
        let mine = self.team_keys()?;
        let mut left = self.teams_left()?;
        let Some(r) = remote else {
            return Ok((mine, left));
        };
        let (theirs, their_left) = r.team_parts();
        for (team, at) in their_left {
            let when = left.entry(team).or_insert(at);
            *when = (*when).max(at);
        }
        let mut newest: BTreeMap<i64, crate::team::TeamKey> = BTreeMap::new();
        for key in mine.iter().chain(&theirs) {
            if newest.get(&key.team_id).is_none_or(|k| key.at > k.at) {
                newest.insert(key.team_id, key.clone());
            }
        }
        // Left at or after the key was got means they're out, on this device too.
        newest.retain(|team, key| left.get(team).is_none_or(|at| *at < key.at));
        for key in &mine {
            if !newest.contains_key(&key.team_id) {
                self.clear_team_here(key.team_id)?;
            }
        }
        let next: Vec<_> = newest.into_values().collect();
        self.set_team_keys(&next)?;
        self.set_teams_left(&left)?;
        Ok((next, left))
    }

    /// Combines the vault with this device, and applies the result here.
    fn merge_and_apply(&self, remote: Option<&Vault>) -> Result<(Vault, bool)> {
        let mut changed = false;
        let mut removed = self.store.tombstones()?;
        let mut entries = BTreeMap::new();
        if let Some(r) = remote {
            removed.extend(r.removed.iter().map(|(k, v)| (k.clone(), *v)));
            entries.extend(r.entries.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        // This device's copy wins, unless the vault's settings are newer.
        for (id, mine) in self.local_entries()? {
            let newer = entries
                .get(&id)
                .is_some_and(|theirs| theirs.settings_updated_at > mine.settings_updated_at);
            if !newer {
                entries.insert(id, mine);
            }
        }
        let mut moved = self.team_moved()?;
        if let Some(r) = remote {
            moved.extend(r.team_moved.iter().map(|(k, v)| (k.clone(), *v)));
        }
        self.set_team_moved(&moved)?;
        entries.retain(|id, _| !removed.contains_key(id) && !moved.contains_key(id));

        let local = self.store.list_accounts()?;
        for a in &local {
            // Handed to the team on another device. It stays, for the team's list to decide.
            // Which team isn't written down here, so it says none: the first of their teams
            // whose list has it claims it, and none removes it just for not having it.
            if moved.contains_key(&a.sync_id) && !a.prefs.team_shared {
                let mut next = a.clone();
                next.prefs.team_shared = true;
                self.store.save_account(&next)?;
                changed = true;
                continue;
            }
            if removed.contains_key(&a.sync_id) && !a.prefs.team_shared {
                self.secrets.delete(&account_secret(a.id))?;
                self.store.delete_account(a.id)?;
                changed = true;
            }
        }
        for id in removed.keys() {
            self.store.add_tombstone(id)?;
        }

        // The same inbox added on two devices becomes one entry.
        let local = self.store.list_accounts()?;
        let mut duplicates = Vec::new();
        for (id, e) in &entries {
            // By its lasting name first, because its server details can change.
            let same_inbox = local.iter().find(|a| a.sync_id == *id).or_else(|| {
                local.iter().find(|a| {
                    !a.prefs.team_shared && a.email == e.email && a.imap_host == e.imap_host
                })
            });
            match same_inbox {
                Some(a) if a.sync_id != *id => {
                    // Both keep working: the smaller name wins on every device.
                    if *id < a.sync_id {
                        duplicates.push(a.sync_id.clone());
                        self.store.set_sync_id(a.id, id)?;
                    } else {
                        duplicates.push(id.clone());
                    }
                }
                Some(a) => {
                    if e.settings_updated_at > a.settings_updated_at {
                        self.store.save_account(&e.applied_to(a))?;
                        // The password may be what changed.
                        self.secrets.set(&account_secret(a.id), &e.secret)?;
                        if let Ok(mut tokens) = self.mail_tokens.try_lock() {
                            tokens.remove(&a.id);
                        }
                        changed = true;
                    }
                }
                None => {
                    let account = self.store.insert_account_with_sync_id(
                        &e.email,
                        &e.display_name,
                        &e.imap_host,
                        e.imap_port,
                        e.security,
                        &e.username,
                        id,
                    )?;
                    self.secrets.set(&account_secret(account.id), &e.secret)?;
                    self.store.save_account(&e.applied_to(&account))?;
                    changed = true;
                }
            }
        }
        for id in duplicates {
            entries.remove(&id);
        }
        // The person's choices: for each one, the newest change wins.
        let (mut settings, mut settings_at) = self.shared_settings()?;
        if let Some(r) = remote {
            let merged = self.merge_settings(&settings, settings_at, &r.settings, r.settings_at);
            let at = settings_at.max(r.settings_at);
            let without_times = |m: &std::collections::BTreeMap<String, String>| {
                let mut copy = m.clone();
                copy.remove(crate::settings::KEY_TIMES);
                copy
            };
            if without_times(&merged) != without_times(&settings) {
                self.apply_shared_settings(&merged, at)?;
                changed = true;
            }
            // Remembered even when nothing else changed, so it isn't undone next time.
            if let Some(times) = merged.get(crate::settings::KEY_TIMES) {
                self.store.set_setting(crate::settings::KEY_TIMES, times)?;
            }
            settings = merged;
            settings_at = at;
        }
        let (team_keys, teams_left) = self.merge_team_keys(remote)?;
        let older = OlderTeamFields::from(&team_keys, &teams_left);
        Ok((
            Vault {
                version: 0,
                entries,
                removed,
                settings,
                settings_at,
                team_keys,
                teams_left,
                team_key: older.key,
                team_key_at: older.at,
                team_left: older.left,
                team_moved: moved,
            },
            changed,
        ))
    }
}

/// The vault's team fields as a device from before several teams understands them.
struct OlderTeamFields {
    key: Option<crate::team::TeamKey>,
    at: i64,
    left: i64,
}

impl OlderTeamFields {
    fn from(keys: &[crate::team::TeamKey], left: &BTreeMap<i64, i64>) -> Self {
        let last_left = left.iter().max_by_key(|(_, at)| **at);
        OlderTeamFields {
            key: keys.first().cloned(),
            at: keys
                .iter()
                .map(|k| k.at)
                .chain(left.values().copied())
                .max()
                .unwrap_or(0),
            left: last_left.map_or(0, |(team, _)| *team),
        }
    }
}

impl Vault {
    /// The teams in this vault. One saved by a device from before several teams, or that
    /// such a device has since saved over, only has the older fields.
    fn team_parts(&self) -> (Vec<crate::team::TeamKey>, BTreeMap<i64, i64>) {
        let mut keys = self.team_keys.clone();
        let mut left = self.teams_left.clone();
        if let Some(older) = &self.team_key {
            if !keys.iter().any(|k| k.team_id == older.team_id) {
                let mut older = older.clone();
                older.at = older.at.max(self.team_key_at);
                keys.push(older);
            }
        }
        if self.team_left != 0 {
            let at = left.entry(self.team_left).or_insert(self.team_key_at);
            *at = (*at).max(self.team_key_at);
        }
        (keys, left)
    }
}

/// The link nonce to work the number out from, or None if it can't be trusted.
///
/// The new device shows a number made from the nonce the server hands it, so the
/// nonce the server holds (`server`) is what counts. It's trusted only when it is
/// the one this device made, or carries proof that another of the person's
/// devices made it. Anything else could have been chosen by the server.
fn trusted_link_nonce(
    vault_key: &str,
    device_id: i64,
    stored: Option<&str>,
    server: Option<&str>,
) -> Option<String> {
    let server = server?;
    if stored == Some(server) || crypto::link_nonce_is_ours(vault_key, device_id, server) {
        Some(server.to_string())
    } else {
        None
    }
}

fn link_again() -> CoreError {
    CoreError::Invalid("That device needs to start signing in again.".into())
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The newest sign-in code in the inbox or spam folder. Reads without marking
/// anything as read.
async fn newest_code(
    account: &Account,
    password: &str,
    since: i64,
    reach: crate::net::Reach,
) -> Result<Option<String>> {
    let mut conn = ImapConnection::connect(account, password, reach).await?;
    let mut folders = vec!["INBOX".to_string()];
    folders.extend(
        crate::sync::with_chosen_roles(conn.folders().await?, account)
            .into_iter()
            .filter(|f| f.role == "junk")
            .map(|f| f.path),
    );
    let mut best: Option<(i64, String)> = None;
    for folder in folders {
        let exists = conn.open_folder(&folder).await?.exists;
        if exists == 0 {
            continue;
        }
        let start = exists.saturating_sub(CODE_LOOK_BACK - 1).max(1);
        let headers = conn.headers_from_position(start).await?;
        let matching: Vec<(u32, i64)> = headers
            .iter()
            .filter(|h| {
                let date = h.internal_date.unwrap_or(0);
                date >= since
                    && CODE_SUBJECTS.contains(&parse::parse_headers(&h.raw_header).subject.as_str())
            })
            .map(|h| (h.uid, h.internal_date.unwrap_or(0)))
            .collect();
        if matching.is_empty() {
            continue;
        }
        let uids: Vec<u32> = matching.iter().map(|m| m.0).collect();
        for (uid, raw) in conn.bodies(&uids).await? {
            let date = matching.iter().find(|m| m.0 == uid).map_or(0, |m| m.1);
            let text = parse::parse_body(&raw).text.unwrap_or_default();
            if let Some(code) = code_in(&text) {
                if best.as_ref().is_none_or(|(d, _)| date >= *d) {
                    best = Some((date, code));
                }
            }
        }
    }
    conn.logout().await;
    Ok(best.map(|b| b.1))
}

fn code_in(text: &str) -> Option<String> {
    let rest = text.split(CODE_LINE).nth(1)?;
    let code: String = rest.chars().take(6).collect();
    (code.len() == 6 && code.chars().all(|c| c.is_ascii_digit())).then_some(code)
}

fn same_vault(a: &Vault, b: &Vault) -> bool {
    let list = |v: &Vault| {
        serde_json::to_string(&(
            &v.entries,
            &v.removed,
            &v.settings,
            v.settings_at,
            (&v.team_keys, &v.teams_left, &v.team_moved),
        ))
        .unwrap_or_default()
    };
    list(a) == list(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonce_the_server_made_up_is_refused() {
        let vault = crypto::random_hex(32);
        let ours = crypto::new_link_nonce(&vault, 3);
        let fake = crypto::random_hex(32);
        // This device's own value, as it saved it.
        assert_eq!(
            trusted_link_nonce(&vault, 3, Some(&ours), Some(&ours)),
            Some(ours.clone())
        );
        // Made by another of the person's devices: carries the proof.
        assert_eq!(
            trusted_link_nonce(&vault, 3, None, Some(&ours)),
            Some(ours.clone())
        );
        // Made up by the server, with or without our own value saved here.
        assert_eq!(trusted_link_nonce(&vault, 3, None, Some(&fake)), None);
        assert_eq!(
            trusted_link_nonce(&vault, 3, Some(&ours), Some(&fake)),
            None
        );
        // Our proof for one device can't be moved to another.
        assert_eq!(trusted_link_nonce(&vault, 4, None, Some(&ours)), None);
        assert_eq!(trusted_link_nonce(&vault, 3, Some(&ours), None), None);
    }
}
