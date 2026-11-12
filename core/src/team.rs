//! Teams, on this device: shared inboxes, who has what, and private comments.
//!
//! Mail stays here. Everything else reaches the account server locked with a team
//! key it never sees, and conversations are known to it only by a scrambled number.
//! The server settles who got there first, and this file keeps a copy of the result
//! so the app keeps working when it can't be reached.

use crate::crypto;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::unix_now;
use crate::model::{Account, Security, SmtpSecurity, ThreadSummary, ThreadTeam};
use crate::secrets::account_secret;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// Where one key lived, before a person could be in several teams.
pub(crate) const TEAM_KEY: &str = "team-key";
pub(crate) const TEAM_KEYS: &str = "team-keys";
const VAULT_TRIES: usize = 8;
const MAX_CATCH_UP_PAGES: usize = 20;
const SEND_CHECK_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// A key, which team it opens, and when this person got it.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct TeamKey {
    pub(crate) team_id: i64,
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) at: i64,
}

#[derive(Clone)]
pub(crate) struct PresenceRow {
    thread_key: String,
    user_id: i64,
    kind: String,
}

/// What's only worth keeping while the app is running.
#[derive(Default)]
pub(crate) struct TeamRuntime {
    presence: Mutex<HashMap<i64, (i64, Vec<PresenceRow>)>>,
}

/// Deliberately has no Debug, because it holds passwords.
#[derive(Serialize, Deserialize, Clone)]
struct TeamInbox {
    email: String,
    display_name: String,
    imap_host: String,
    imap_port: u16,
    security: Security,
    username: String,
    secret: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_security: SmtpSecurity,
    auth: String,
    updated_at: i64,
}

#[derive(Serialize, Deserialize, Default)]
struct TeamVault {
    version: i64,
    inboxes: BTreeMap<String, TeamInbox>,
}

#[derive(Deserialize)]
struct VaultReply {
    version: i64,
    blob: Option<String>,
}

#[derive(Deserialize)]
struct ServerTeam {
    id: i64,
    name_sealed: String,
    role: String,
    me: i64,
    members_version: i64,
    vault_version: i64,
    members: Vec<ServerMember>,
}

#[derive(Deserialize)]
struct ServerMember {
    user_id: i64,
    email: String,
    role: String,
}

#[derive(Deserialize)]
struct TeamsReply {
    teams: Vec<ServerTeam>,
}

#[derive(Deserialize)]
struct ChangedThread {
    #[serde(default)]
    sending_until: i64,
    thread_key: String,
    assignee: Option<i64>,
    #[serde(default)]
    assigned_by: Option<i64>,
    status: String,
    status_at: i64,
    sent_count: i64,
    last_sent_by: Option<i64>,
    last_sent_at: i64,
    sending_by: Option<i64>,
    seq: i64,
}

#[derive(Deserialize)]
struct ChangedComment {
    id: String,
    thread_key: String,
    author: i64,
    blob: Option<String>,
    created_at: i64,
    seq: i64,
}

#[derive(Deserialize)]
struct ChangedTemplate {
    id: String,
    blob: Option<String>,
    version: i64,
    seq: i64,
}

#[derive(Deserialize)]
struct ChangedPresence {
    thread_key: String,
    user_id: i64,
    kind: String,
}

#[derive(Deserialize)]
struct Changes {
    team_id: i64,
    seq: i64,
    more: bool,
    revision: i64,
    members_version: i64,
    vault_version: i64,
    threads: Vec<ChangedThread>,
    comments: Vec<ChangedComment>,
    templates: Vec<ChangedTemplate>,
    presence: Vec<ChangedPresence>,
}

#[derive(Serialize, Deserialize)]
struct CommentBody {
    text: String,
    #[serde(default)]
    mentions: Vec<i64>,
}

#[derive(Serialize, Deserialize)]
struct TemplateBody {
    name: String,
    body: String,
}

fn no_team() -> CoreError {
    CoreError::Cloud {
        code: "no_team".into(),
        message: "You're not in a team on this device.".into(),
    }
}

/// Whether a team's list has this inbox, by its lasting name.
fn lists(vault: &TeamVault, a: &Account) -> bool {
    vault.inboxes.contains_key(&a.sync_id)
}

fn offline_or(e: &CoreError, code: &str) -> bool {
    matches!(e, CoreError::Cloud { code: c, .. } if c == code)
}

impl Engine {
    /// The keys to all this person's teams.
    pub(crate) fn team_keys(&self) -> Result<Vec<TeamKey>> {
        if let Some(raw) = self.secrets.get(TEAM_KEYS)? {
            return Ok(serde_json::from_str(&raw).unwrap_or_default());
        }
        let older: Option<TeamKey> = self
            .secrets
            .get(TEAM_KEY)?
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let Some(mut key) = older else {
            return Ok(vec![]);
        };
        key.at = self.older_key_time()?;
        let keys = vec![key];
        self.set_team_keys(&keys)?;
        self.secrets.delete(TEAM_KEY)?;
        Ok(keys)
    }

    fn older_key_time(&self) -> Result<i64> {
        Ok(self
            .store
            .setting("team_key_at")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    pub(crate) fn set_team_keys(&self, keys: &[TeamKey]) -> Result<()> {
        if keys.is_empty() {
            return self.secrets.delete(TEAM_KEYS);
        }
        let raw = serde_json::to_string(keys)
            .map_err(|_| CoreError::Secret("Could not save the team key.".into()))?;
        self.secrets.set(TEAM_KEYS, &raw)
    }

    pub(crate) fn team_key(&self, team_id: i64) -> Result<Option<TeamKey>> {
        Ok(self.team_keys()?.into_iter().find(|k| k.team_id == team_id))
    }

    fn require_team_key(&self, team_id: i64) -> Result<TeamKey> {
        self.team_key(team_id)?.ok_or_else(no_team)
    }

    /// Later than leaving the team, so coming back counts even within the same second.
    fn add_team_key(&self, team_id: i64, key: String) -> Result<()> {
        let left = self.teams_left()?.get(&team_id).copied().unwrap_or(0);
        let mut keys = self.team_keys()?;
        keys.retain(|k| k.team_id != team_id);
        keys.push(TeamKey {
            team_id,
            key,
            at: unix_now().max(left + 1),
        });
        self.set_team_keys(&keys)
    }

    fn remove_team_key(&self, team_id: i64) -> Result<()> {
        let mut keys = self.team_keys()?;
        keys.retain(|k| k.team_id != team_id);
        self.set_team_keys(&keys)
    }

    /// Which teams this person has left and when, so a device still holding an old key
    /// lets go of it, without touching their other teams.
    pub(crate) fn teams_left(&self) -> Result<BTreeMap<i64, i64>> {
        if let Some(raw) = self.store.setting("teams_left")? {
            return Ok(serde_json::from_str(&raw).unwrap_or_default());
        }
        let older = self
            .store
            .setting("team_left")?
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let mut left = BTreeMap::new();
        if older != 0 {
            left.insert(older, self.older_key_time()?);
        }
        Ok(left)
    }

    pub(crate) fn set_teams_left(&self, left: &BTreeMap<i64, i64>) -> Result<()> {
        let raw = serde_json::to_string(left)
            .map_err(|_| CoreError::Invalid("Could not save the team list.".into()))?;
        self.store.set_setting("teams_left", &raw)
    }

    fn note_team_left(&self, team_id: i64) -> Result<()> {
        let mut left = self.teams_left()?;
        let got_key = self.team_key(team_id)?.map_or(0, |k| k.at);
        left.insert(team_id, unix_now().max(got_key));
        self.set_teams_left(&left)
    }

    /// Inboxes this person has handed to the team, so their other devices treat them
    /// as the team's rather than deleting them.
    pub(crate) fn team_moved(&self) -> Result<BTreeMap<String, i64>> {
        Ok(self
            .store
            .setting("team_moved")?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default())
    }

    pub(crate) fn set_team_moved(&self, moved: &BTreeMap<String, i64>) -> Result<()> {
        let raw = serde_json::to_string(moved)
            .map_err(|_| CoreError::Invalid("Could not save the inbox list.".into()))?;
        self.store.set_setting("team_moved", &raw)
    }

    fn note_team_moved(&self, sync_id: &str) -> Result<()> {
        let mut moved = self.team_moved()?;
        moved.insert(sync_id.to_string(), unix_now());
        self.set_team_moved(&moved)
    }

    /// The team a request is about. Not saying means the person's first.
    fn pick_team(&self, asked: Option<i64>) -> Result<i64> {
        match asked {
            Some(team) => Ok(team),
            None => self.first_team()?.ok_or_else(no_team),
        }
    }

    fn first_team(&self) -> Result<Option<i64>> {
        let known = self.store.team_infos()?.first().map(|i| i.team_id);
        Ok(match known {
            Some(team) => Some(team),
            None => self.team_keys()?.iter().map(|k| k.team_id).min(),
        })
    }

    /// Which team a shared inbox belongs to. One shared before people could have several
    /// teams may not say, and belongs to the team they were in then.
    pub(crate) fn account_team(&self, account: &Account) -> Result<Option<i64>> {
        if !account.prefs.team_shared {
            return Ok(None);
        }
        if account.prefs.team_id != 0 {
            return Ok(Some(account.prefs.team_id));
        }
        self.first_team()
    }

    fn thread_key_for(&self, key: &TeamKey, account: &Account, thread_id: &str) -> String {
        crypto::team_thread_key(&key.key, &account.email, thread_id)
    }

    fn shared_account(&self, account_id: i64) -> Result<Account> {
        let account = self.store.account(account_id)?;
        if account.prefs.team_shared {
            Ok(account)
        } else {
            Err(CoreError::Invalid(
                "This inbox isn't shared with your team.".into(),
            ))
        }
    }

    fn who(&self, team_id: i64, user_id: Option<i64>) -> Result<Option<String>> {
        Ok(match user_id {
            Some(id) => Some(
                self.store
                    .team_member_email(team_id, id)?
                    .unwrap_or_else(|| "A teammate".to_string()),
            ),
            None => None,
        })
    }

    /// What's known about one team on this device, for the settings page.
    pub(crate) fn team_status(&self, asked: Option<i64>) -> Result<Value> {
        let team = match asked {
            Some(team) => Some(team),
            None => self.first_team()?,
        };
        let info = match team {
            Some(team) => self.store.team_info(team)?,
            None => None,
        };
        let Some(info) = info else {
            return Ok(json!({ "state": "none" }));
        };
        let has_key = self.team_key(info.team_id)?.is_some();
        Ok(json!({
            "team_id": info.team_id,
            "state": if has_key { "ready" } else { "waiting_for_key" },
            "name": info.name,
            "role": info.role,
            "me": info.me,
            "members": self.store.team_members(info.team_id)?,
        }))
    }

    /// All this person's teams, oldest first.
    pub(crate) fn teams_status(&self) -> Result<Value> {
        let mut teams = Vec::new();
        for info in self.store.team_infos()? {
            teams.push(self.team_status(Some(info.team_id))?);
        }
        Ok(json!({ "teams": teams }))
    }

    /// Brings every team's name, people and shared inboxes up to date.
    pub(crate) async fn team_refresh(&self) -> Result<Value> {
        let reply: TeamsReply = self.cloud_send(Method::GET, "/teams", None).await?;
        // Anything the server no longer lists, this person is no longer in.
        let mut known: Vec<i64> = self
            .store
            .team_infos()?
            .iter()
            .map(|i| i.team_id)
            .chain(self.team_keys()?.iter().map(|k| k.team_id))
            .collect();
        known.sort_unstable();
        known.dedup();
        for team in known {
            if !reply.teams.iter().any(|t| t.id == team) {
                self.team_gone(team).await?;
            }
        }
        for team in &reply.teams {
            let key = self.team_key(team.id)?;
            let name = key
                .as_ref()
                .and_then(|k| crypto::vault_open(&k.key, &team.name_sealed).ok())
                .and_then(|plain| String::from_utf8(plain).ok())
                .unwrap_or_default();
            self.store.save_team_info(
                team.id,
                &name,
                team.me,
                &team.role,
                team.members_version,
                &team
                    .members
                    .iter()
                    .map(|m| (m.user_id, m.email.clone(), m.role.clone()))
                    .collect::<Vec<_>>(),
            )?;
            if key.is_some() && team.vault_version > 0 {
                self.team_vault_apply(team.id).await?;
            }
        }
        self.team_status(None)
    }

    /// This person is no longer in the team, so nothing of it stays on this device.
    pub(crate) async fn team_gone(&self, team_id: i64) -> Result<()> {
        self.note_team_left(team_id)?;
        self.clear_team_here(team_id)?;
        self.remove_team_key(team_id)?;
        // Their other devices find out through the account list too.
        let _ = self.vault_sync().await;
        Ok(())
    }

    pub(crate) fn clear_team_here(&self, team_id: i64) -> Result<()> {
        // A shared inbox that doesn't say its team only goes with this one when there's no
        // other team it could belong to.
        let other_teams = self.team_keys()?.iter().any(|k| k.team_id != team_id);
        for a in self.store.list_accounts()? {
            let theirs = if a.prefs.team_id == 0 {
                !other_teams && self.account_team(&a)? == Some(team_id)
            } else {
                a.prefs.team_shared && a.prefs.team_id == team_id
            };
            if theirs {
                self.remove_shared_account(&a)?;
            }
        }
        self.store.wipe_team(team_id)?;
        self.team
            .presence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&team_id);
        Ok(())
    }

    pub(crate) fn remove_shared_account(&self, a: &Account) -> Result<()> {
        for draft in self.store.draft_ids_for_account(a.id)? {
            self.remove_draft_files(draft);
        }
        self.secrets.delete(&account_secret(a.id))?;
        if let Ok(mut tokens) = self.mail_tokens.try_lock() {
            tokens.remove(&a.id);
        }
        self.store.delete_account(a.id)
    }

    pub(crate) async fn team_create(&self, name: &str) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(CoreError::Invalid("Give your team a short name.".into()));
        }
        let key = crypto::random_hex(32);
        let reply: Value = self
            .cloud_send(
                Method::POST,
                "/team",
                Some(json!({
                    "name_sealed": crypto::vault_seal(&key, name.as_bytes())?,
                    "multi": true,
                })),
            )
            .await?;
        let team_id = reply["team_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Network("unexpected reply".into()))?;
        self.add_team_key(team_id, key)?;
        let _ = self.vault_sync().await;
        self.team_refresh().await?;
        self.team_status(Some(team_id))
    }

    /// The code is the only way in, and only the person holding it can open what the
    /// server keeps. It's shown once and never sent to the server.
    pub(crate) async fn team_invite(&self, asked: Option<i64>, role: &str) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let key = self.require_team_key(team)?;
        let code = crypto::new_recovery_key();
        let (proof, wrap) = crypto::recovery_parts(&code)?;
        let reply: Value = self
            .cloud_send_for_team(
                Some(team),
                Method::POST,
                "/team/invites",
                Some(json!({
                    "code_hash": crypto::proof_hash(&proof),
                    "sealed_key": crypto::recovery_seal(&wrap, &key.key)?,
                    "role": role,
                })),
            )
            .await?;
        Ok(json!({ "code": code, "expires_at": reply["expires_at"], "id": reply["id"] }))
    }

    pub(crate) async fn team_invites(&self, asked: Option<i64>) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let reply: Value = self
            .cloud_send_for_team(Some(team), Method::GET, "/team/invites", None)
            .await?;
        Ok(reply["invites"].clone())
    }

    pub(crate) async fn team_cancel_invite(&self, asked: Option<i64>, id: i64) -> Result<Value> {
        let team = self.pick_team(asked)?;
        self.cloud_send_for_team(
            Some(team),
            Method::DELETE,
            &format!("/team/invites/{id}"),
            None,
        )
        .await
    }

    pub(crate) async fn team_join(&self, code: &str) -> Result<Value> {
        let wrong = || CoreError::Cloud {
            code: "wrong_invite".into(),
            message: "That invite code isn't right, or it has already been used or run out.".into(),
        };
        let (proof, wrap) = crypto::recovery_parts(code).map_err(|_| wrong())?;
        let reply: Value = self
            .cloud_send(
                Method::POST,
                "/team/join",
                Some(json!({ "proof": proof, "multi": true })),
            )
            .await?;
        let team_id = reply["team_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Network("unexpected reply".into()))?;
        let sealed = reply["sealed_key"]
            .as_str()
            .ok_or_else(|| CoreError::Network("unexpected reply".into()))?;
        let key = crypto::recovery_open(&wrap, sealed).map_err(|_| wrong())?;
        self.add_team_key(team_id, key)?;
        let _ = self.vault_sync().await;
        self.team_refresh().await?;
        self.team_status(Some(team_id))
    }

    pub(crate) async fn team_leave(&self, asked: Option<i64>) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let _: Value = self
            .cloud_send_for_team(Some(team), Method::POST, "/team/leave", None)
            .await?;
        self.team_gone(team).await?;
        self.team_status(None)
    }

    pub(crate) async fn team_delete(&self, asked: Option<i64>) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let _: Value = self
            .cloud_send_for_team(Some(team), Method::DELETE, "/team", None)
            .await?;
        self.team_gone(team).await?;
        self.team_status(None)
    }

    pub(crate) async fn team_remove_member(
        &self,
        asked: Option<i64>,
        user_id: i64,
    ) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let _: Value = self
            .cloud_send_for_team(
                Some(team),
                Method::DELETE,
                &format!("/team/members/{user_id}"),
                None,
            )
            .await?;
        self.team_refresh().await?;
        self.team_status(Some(team))
    }

    pub(crate) async fn team_set_role(
        &self,
        asked: Option<i64>,
        user_id: i64,
        role: &str,
    ) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let _: Value = self
            .cloud_send_for_team(
                Some(team),
                Method::PUT,
                &format!("/team/members/{user_id}"),
                Some(json!({ "role": role })),
            )
            .await?;
        self.team_refresh().await?;
        self.team_status(Some(team))
    }

    fn read_team_vault(&self, key: &TeamKey, reply: &VaultReply) -> Result<TeamVault> {
        let Some(blob) = &reply.blob else {
            // Only a team that has never had a list may have none. Otherwise a missing
            // list would read as an empty one, and every shared inbox would go.
            if reply.version > 0 || self.store.team_vault_seen(key.team_id)? > 0 {
                return Err(CoreError::Invalid(
                    "The account server didn't send the team's inboxes, so nothing was changed."
                        .into(),
                ));
            }
            return Ok(TeamVault::default());
        };
        let plain = crypto::vault_open(&key.key, blob)?;
        let vault: TeamVault = serde_json::from_slice(&plain)
            .map_err(|_| CoreError::Invalid("The team's inbox list could not be read.".into()))?;
        // The version is inside the encryption, so the server can't bring back an
        // older copy with a removed inbox or an old password still in it.
        if vault.version != reply.version
            || reply.version < self.store.team_vault_seen(key.team_id)?
        {
            return Err(CoreError::Invalid(
                "The account server sent an out-of-date copy of the team's inboxes, so it was ignored."
                    .into(),
            ));
        }
        Ok(vault)
    }

    /// Reads the newest list, changes it and saves it, starting again if another
    /// admin saved first. Nothing is merged, so nobody's change is lost.
    async fn change_team_vault(&self, team: i64, edit: impl Fn(&mut TeamVault)) -> Result<()> {
        let key = self.require_team_key(team)?;
        for _ in 0..VAULT_TRIES {
            let reply: VaultReply = self
                .cloud_send_for_team(Some(team), Method::GET, "/team/vault", None)
                .await?;
            let mut vault = self.read_team_vault(&key, &reply)?;
            edit(&mut vault);
            vault.version = reply.version + 1;
            let plain = serde_json::to_vec(&vault)
                .map_err(|_| CoreError::Invalid("Could not save the team's inboxes.".into()))?;
            let saved: Result<Value> = self
                .cloud_send_for_team(
                    Some(team),
                    Method::PUT,
                    "/team/vault",
                    Some(json!({
                        "expected_version": reply.version,
                        "blob": crypto::vault_seal(&key.key, &plain)?,
                    })),
                )
                .await;
            match saved {
                Ok(_) => {
                    self.store.set_team_vault_seen(team, vault.version)?;
                    return Ok(());
                }
                Err(e) if offline_or(&e, "vault_changed") => continue,
                Err(e) => return Err(e),
            }
        }
        Err(CoreError::Network(
            "Your team is changing its inboxes right now. Try again shortly.".into(),
        ))
    }

    /// Makes this device's shared inboxes in this team match the team's list.
    pub(crate) async fn team_vault_apply(&self, team: i64) -> Result<bool> {
        let key = self.require_team_key(team)?;
        let reply: VaultReply = self
            .cloud_send_for_team(Some(team), Method::GET, "/team/vault", None)
            .await?;
        let vault = self.read_team_vault(&key, &reply)?;
        // A shared inbox that doesn't say its team, and isn't on this list, may be on
        // another team's. Those lists are read too, so it goes to the right team, and is
        // only removed when none of them has it.
        let unclaimed = self
            .store
            .list_accounts()?
            .into_iter()
            .any(|a| a.prefs.team_shared && a.prefs.team_id == 0 && !lists(&vault, &a));
        let mut others = BTreeMap::new();
        if unclaimed {
            for other in self.team_keys()? {
                if other.team_id == team {
                    continue;
                }
                let reply: Result<VaultReply> = self
                    .cloud_send_for_team(Some(other.team_id), Method::GET, "/team/vault", None)
                    .await;
                let listed = reply.and_then(|r| self.read_team_vault(&other, &r)).ok();
                others.insert(other.team_id, listed);
            }
        }
        let changed = self.apply_team_vault(team, &vault, &others)?;
        self.store.set_team_vault_seen(team, reply.version)?;
        Ok(changed)
    }

    /// Applies one team's list here. `others` has the other teams' lists, where they
    /// were needed and could be read, for inboxes that don't say which team they're in.
    fn apply_team_vault(
        &self,
        team: i64,
        vault: &TeamVault,
        others: &BTreeMap<i64, Option<TeamVault>>,
    ) -> Result<bool> {
        let mut changed = false;
        let several = self.team_keys()?.iter().any(|k| k.team_id != team);
        for a in self.store.list_accounts()? {
            if !a.prefs.team_shared {
                continue;
            }
            if a.prefs.team_id == team {
                if !lists(vault, &a) {
                    self.remove_shared_account(&a)?;
                    changed = true;
                }
                continue;
            }
            if a.prefs.team_id != 0 || lists(vault, &a) {
                continue;
            }
            // Doesn't say its team, and isn't on this list.
            if !several {
                // From before people could be in several teams: it's this one's.
                if self.account_team(&a)? == Some(team) {
                    self.remove_shared_account(&a)?;
                    changed = true;
                }
                continue;
            }
            let mut unsure = false;
            let mut belongs = None;
            for k in self.team_keys()? {
                if k.team_id == team {
                    continue;
                }
                match others.get(&k.team_id) {
                    Some(Some(v)) if lists(v, &a) => belongs = belongs.or(Some(k.team_id)),
                    Some(Some(_)) => {}
                    // Couldn't be read, so it may well be there.
                    _ => unsure = true,
                }
            }
            if let Some(other) = belongs {
                let mut next = a.clone();
                next.prefs.team_id = other;
                self.store.save_account(&next)?;
                changed = true;
            } else if !unsure {
                self.remove_shared_account(&a)?;
                changed = true;
            }
        }
        // Read after those removals, so nothing below matches an inbox that just went.
        let local = self.store.list_accounts()?;
        for (id, entry) in &vault.inboxes {
            let found = local.iter().find(|a| a.sync_id == *id).or_else(|| {
                local
                    .iter()
                    .find(|a| a.email == entry.email && a.imap_host == entry.imap_host)
            });
            let account = match found {
                // Already shared with another of their teams, and one team keeps it. One
                // that doesn't say its team yet is this team's, as this list has it.
                Some(a)
                    if a.prefs.team_shared && a.prefs.team_id != 0 && a.prefs.team_id != team =>
                {
                    continue
                }
                Some(a) if a.prefs.team_shared && self.same_connection(a, entry)? => {
                    if a.prefs.team_id != team {
                        let mut next = a.clone();
                        next.prefs.team_id = team;
                        self.store.save_account(&next)?;
                    }
                    continue;
                }
                // Someone's own inbox is only taken into the team when the list has exactly
                // its servers and password, so a teammate can't redirect it or take it over.
                Some(a) if !a.prefs.team_shared && !self.same_connection(a, entry)? => continue,
                Some(a) => {
                    // Already connected here by this person. It's the same inbox.
                    if a.sync_id != *id {
                        self.note_team_moved(&a.sync_id)?;
                        self.store.set_sync_id(a.id, id)?;
                    }
                    a.clone()
                }
                None => self.store.insert_account_with_sync_id(
                    &entry.email,
                    &entry.display_name,
                    &entry.imap_host,
                    entry.imap_port,
                    entry.security,
                    &entry.username,
                    id,
                )?,
            };
            let mut next = account.clone();
            next.display_name = entry.display_name.clone();
            next.imap_host = entry.imap_host.clone();
            next.imap_port = entry.imap_port;
            next.security = entry.security;
            next.username = entry.username.clone();
            next.smtp_host = entry.smtp_host.clone();
            next.smtp_port = entry.smtp_port;
            next.smtp_security = entry.smtp_security;
            next.auth = entry.auth.clone();
            next.settings_updated_at = entry.updated_at;
            next.prefs.team_shared = true;
            next.prefs.team_id = team;
            self.store.save_account(&next)?;
            // A browser sign-in renews itself on each device, so an older copy from
            // the list mustn't undo that. A newer one means an admin signed in again.
            let renewing = entry.auth != crate::signin::PASSWORD
                && account.prefs.team_shared
                && entry.updated_at <= account.settings_updated_at;
            if !renewing {
                self.secrets
                    .set(&account_secret(account.id), &entry.secret)?;
                if let Ok(mut tokens) = self.mail_tokens.try_lock() {
                    tokens.remove(&account.id);
                }
            }
            changed = true;
        }
        Ok(changed)
    }

    /// The team's list has the last word on how a shared inbox connects.
    fn same_connection(&self, a: &Account, e: &TeamInbox) -> Result<bool> {
        Ok(a.imap_host == e.imap_host
            && a.imap_port == e.imap_port
            && a.security == e.security
            && a.username == e.username
            && a.smtp_host == e.smtp_host
            && a.smtp_port == e.smtp_port
            && a.smtp_security == e.smtp_security
            && a.auth == e.auth
            && a.display_name == e.display_name
            && (e.auth != crate::signin::PASSWORD && e.updated_at <= a.settings_updated_at
                || self.secrets.get(&account_secret(a.id))?.as_deref() == Some(e.secret.as_str())))
    }

    fn team_inbox_entry(&self, a: &Account) -> Result<TeamInbox> {
        let secret = self
            .secrets
            .get(&account_secret(a.id))?
            .ok_or_else(|| CoreError::Secret("This inbox's password is missing.".into()))?;
        Ok(TeamInbox {
            email: a.email.clone(),
            display_name: a.display_name.clone(),
            imap_host: a.imap_host.clone(),
            imap_port: a.imap_port,
            security: a.security,
            username: a.username.clone(),
            secret,
            smtp_host: a.smtp_host.clone(),
            smtp_port: a.smtp_port,
            smtp_security: a.smtp_security,
            auth: a.auth.clone(),
            updated_at: unix_now(),
        })
    }

    /// Everyone in the team gets this inbox on all their devices, without being
    /// told its password. It leaves this person's own account list.
    pub(crate) async fn team_share_inbox(
        &self,
        asked: Option<i64>,
        account_id: i64,
    ) -> Result<Value> {
        let mut account = self.store.account(account_id)?;
        if account.prefs.team_shared {
            return Ok(json!({ "shared": true }));
        }
        let team = self.pick_team(asked)?;
        let entry = self.team_inbox_entry(&account)?;
        let at = entry.updated_at;
        let id = account.sync_id.clone();
        self.change_team_vault(team, |v| {
            v.inboxes.insert(id.clone(), entry.clone());
        })
        .await?;
        account.prefs.team_shared = true;
        account.prefs.team_id = team;
        account.settings_updated_at = at;
        self.store.save_account(&account)?;
        self.note_team_moved(&account.sync_id)?;
        let _ = self.vault_sync().await;
        Ok(json!({ "shared": true }))
    }

    /// It goes from everyone's devices. The person who stops sharing it keeps it as
    /// their own, under a new lasting name so the old removal doesn't catch it.
    pub(crate) async fn team_unshare_inbox(&self, account_id: i64) -> Result<Value> {
        let mut account = self.shared_account(account_id)?;
        let team = self.account_team(&account)?.ok_or_else(no_team)?;
        let id = account.sync_id.clone();
        if self
            .store
            .team_info(team)?
            .is_some_and(|i| i.role == "member")
        {
            return Err(CoreError::Cloud {
                code: "team_not_allowed".into(),
                message: "Only a team admin can do that.".into(),
            });
        }
        // Theirs again first, so that however this next step turns out, it can't delete
        // their copy. If the team's list keeps it after all, the list makes it shared.
        let fresh = crypto::random_hex(16);
        self.store.set_sync_id(account.id, &fresh)?;
        account.sync_id = fresh;
        account.prefs.team_shared = false;
        account.prefs.team_id = 0;
        account.settings_updated_at = unix_now();
        self.store.save_account(&account)?;
        let removed = self
            .change_team_vault(team, |v| {
                v.inboxes.remove(&id);
            })
            .await;
        if let Err(e) = removed {
            let _ = self.team_vault_apply(team).await;
            return Err(e);
        }
        let _ = self.vault_sync().await;
        Ok(json!({ "shared": false }))
    }

    /// A new password for a shared inbox reaches everyone.
    pub(crate) async fn team_publish_inbox(&self, account_id: i64) -> Result<()> {
        let account = self.shared_account(account_id)?;
        let team = self.account_team(&account)?.ok_or_else(no_team)?;
        let entry = self.team_inbox_entry(&account)?;
        let id = account.sync_id.clone();
        self.change_team_vault(team, |v| {
            v.inboxes.insert(id.clone(), entry.clone());
        })
        .await
    }

    /// One look for what changed in any team. With `wait`, the server holds its answer
    /// until there's something to say, so changes arrive within moments. The first team
    /// with news answers, and the app simply asks again for the rest.
    pub(crate) async fn team_poll(&self, wait: bool) -> Result<Value> {
        if !self.is_ready()? {
            return Ok(json!({ "active": false, "changed": false }));
        }
        let mut keys = self.team_keys()?;
        if keys.is_empty() {
            return Ok(json!({ "active": false, "changed": false }));
        }
        let known = self.store.team_infos()?;
        if keys
            .iter()
            .any(|k| !known.iter().any(|i| i.team_id == k.team_id))
        {
            self.team_refresh().await?;
            keys = self.team_keys()?;
        }
        let mut changed = false;
        if wait && keys.len() > 1 {
            let looks = keys.iter().map(|k| Box::pin(self.team_poll_one(k, true)));
            let (first, _, _) = futures_util::future::select_all(looks).await;
            changed = first?;
        } else {
            for key in &keys {
                changed |= self.team_poll_one(key, wait).await?;
            }
        }
        let active = !self.team_keys()?.is_empty();
        Ok(json!({ "active": active, "changed": changed || !active }))
    }

    async fn team_poll_one(&self, key: &TeamKey, wait: bool) -> Result<bool> {
        let team = key.team_id;
        let Some(info) = self.store.team_info(team)? else {
            return Ok(true);
        };
        let mut changed = false;
        let mut wait = wait;
        // Catching up isn't news, however many pages it takes.
        let first_look = self.store.team_seq(team)? == 0;
        for _ in 0..MAX_CATCH_UP_PAGES {
            let seq = self.store.team_seq(team)?;
            let revision = self
                .team
                .presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&team)
                .map_or(0, |p| p.0);
            let path = format!("/team/changes?after={seq}&revision={revision}&wait={wait}");
            let reply: Result<Changes> = self
                .cloud_send_for_team(Some(team), Method::GET, &path, None)
                .await;
            let reply = match reply {
                Ok(r) => r,
                Err(e) if offline_or(&e, "not_in_team") => {
                    self.team_gone(team).await?;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            };
            // Only ever open a team's data with that team's key.
            if reply.team_id != team {
                self.team_refresh().await?;
                return Ok(true);
            }
            changed |= self.apply_changes(key, info.me, first_look, &reply)?;
            if reply.members_version != self.store.team_members_version(team)? {
                self.team_refresh().await?;
                changed = true;
            } else if reply.vault_version > self.store.team_vault_seen(team)? {
                changed |= self.team_vault_apply(team).await?;
            }
            if !reply.more {
                break;
            }
            wait = false;
        }
        Ok(changed)
    }

    fn apply_changes(&self, key: &TeamKey, me: i64, first_look: bool, c: &Changes) -> Result<bool> {
        let team = key.team_id;
        let mut changed = false;
        let mut alerts = Vec::new();
        for t in &c.threads {
            let before = self.store.team_thread(&t.thread_key)?;
            // Replies can arrive out of order. The server's number says which is newer.
            if before.as_ref().is_some_and(|b| b.seq >= t.seq) {
                continue;
            }
            self.store.save_team_thread(
                team,
                &crate::store::TeamThreadRow {
                    thread_key: t.thread_key.clone(),
                    assignee: t.assignee,
                    status: t.status.clone(),
                    status_at: t.status_at,
                    sent_count: t.sent_count,
                    last_sent_by: t.last_sent_by,
                    last_sent_at: t.last_sent_at,
                    sending_by: t.sending_by,
                    sending_until: t.sending_until,
                    seq: t.seq,
                },
            )?;
            changed = true;
            let was_mine = before.as_ref().is_some_and(|b| b.assignee == Some(me));
            if !first_look && t.assignee == Some(me) && !was_mine && t.assigned_by != Some(me) {
                alerts.push((
                    "Assigned to you".to_string(),
                    self.who(team, t.assigned_by)?
                        .map_or("A conversation".to_string(), |w| {
                            format!("{w} gave you a conversation")
                        }),
                ));
            }
        }
        for row in &c.comments {
            if self
                .store
                .team_comment_seq(&row.id)?
                .is_some_and(|s| s >= row.seq)
            {
                continue;
            }
            let body: Option<CommentBody> = row
                .blob
                .as_ref()
                .and_then(|b| crypto::vault_open(&key.key, b).ok())
                .and_then(|plain| serde_json::from_slice(&plain).ok());
            let is_new = self.store.team_comment_seq(&row.id)?.is_none();
            self.store.save_team_comment(
                team,
                &row.id,
                &row.thread_key,
                row.author,
                body.as_ref().map(|b| b.text.as_str()),
                row.created_at,
                row.seq,
            )?;
            changed = true;
            let mentioned = body.as_ref().is_some_and(|b| b.mentions.contains(&me));
            if !first_look && is_new && mentioned && row.author != me {
                alerts.push((
                    format!(
                        "{} mentioned you",
                        self.who(team, Some(row.author))?.unwrap_or_default()
                    ),
                    body.map(|b| b.text).unwrap_or_default(),
                ));
            }
        }
        for row in &c.templates {
            if self
                .store
                .team_template_seq(team, &row.id)?
                .is_some_and(|s| s >= row.seq)
            {
                continue;
            }
            let body: Option<TemplateBody> = row
                .blob
                .as_ref()
                .and_then(|b| crypto::vault_open(&key.key, b).ok())
                .and_then(|plain| serde_json::from_slice(&plain).ok());
            self.store.save_team_template(
                team,
                &row.id,
                body.as_ref().map(|b| b.name.as_str()),
                body.as_ref().map(|b| b.body.as_str()),
                row.version,
                row.seq,
            )?;
            changed = true;
        }
        {
            let mut all = self.team.presence.lock().unwrap_or_else(|e| e.into_inner());
            let presence = all.entry(team).or_insert((0, vec![]));
            if c.revision >= presence.0 {
                if c.revision != presence.0 {
                    changed = true;
                }
                presence.0 = c.revision;
                presence.1 = c
                    .presence
                    .iter()
                    .map(|p| PresenceRow {
                        thread_key: p.thread_key.clone(),
                        user_id: p.user_id,
                        kind: p.kind.clone(),
                    })
                    .collect();
            }
        }
        self.store.advance_team_seq(team, c.seq)?;
        for (title, body) in alerts {
            self.send_team_alert(&title, &body)?;
        }
        Ok(changed)
    }

    fn presence_on(&self, team: i64, key: &str, me: i64) -> (Vec<String>, Vec<String>) {
        let rows = self
            .team
            .presence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&team)
            .map_or_else(Vec::new, |p| p.1.clone());
        let (mut viewing, mut replying) = (Vec::new(), Vec::new());
        for r in rows
            .iter()
            .filter(|r| r.thread_key == key && r.user_id != me)
        {
            let Ok(Some(email)) = self.store.team_member_email(team, r.user_id) else {
                continue;
            };
            if r.kind == "replying" {
                replying.push(email);
            } else {
                viewing.push(email);
            }
        }
        (viewing, replying)
    }

    /// Adds who has what to a list of emails, for the shared inboxes in it.
    pub(crate) fn add_team_info(&self, threads: &mut [ThreadSummary]) -> Result<()> {
        let keys = self.team_keys()?;
        if keys.is_empty() {
            return Ok(());
        }
        let infos = self.store.team_infos()?;
        let mut shared: HashMap<i64, (Account, i64)> = HashMap::new();
        for a in self.store.list_accounts()? {
            if let Some(team) = self.account_team(&a)? {
                shared.insert(a.id, (a, team));
            }
        }
        if shared.is_empty() {
            return Ok(());
        }
        let mut members: HashMap<i64, HashMap<i64, String>> = HashMap::new();
        for info in &infos {
            members.insert(info.team_id, self.store.team_member_emails(info.team_id)?);
        }
        let counts = self.store.team_comment_counts()?;
        for t in threads.iter_mut() {
            let Some((account, team)) = shared.get(&t.account_id) else {
                continue;
            };
            let Some(key) = keys.iter().find(|k| k.team_id == *team) else {
                continue;
            };
            let Some(info) = infos.iter().find(|i| i.team_id == *team) else {
                continue;
            };
            let thread_key = self.thread_key_for(key, account, &t.thread_id);
            let row = self.store.team_thread(&thread_key)?;
            let (viewing, replying) = self.presence_on(*team, &thread_key, info.me);
            let assignee = row.as_ref().and_then(|r| r.assignee);
            t.team = Some(ThreadTeam {
                assignee,
                assignee_email: assignee
                    .and_then(|a| members.get(team).and_then(|m| m.get(&a)).cloned()),
                mine: assignee == Some(info.me),
                done: is_done(
                    row.as_ref(),
                    self.store
                        .thread_last_incoming(t.account_id, &t.thread_id)?,
                ),
                comments: counts.get(&thread_key).copied().unwrap_or(0),
                viewing,
                replying,
            });
        }
        Ok(())
    }

    /// Filters for the team's views of a shared inbox.
    pub(crate) fn keep_team_view(&self, threads: &mut Vec<ThreadSummary>, view: &str) {
        threads.retain(|t| {
            let Some(team) = &t.team else {
                return view == "team_open";
            };
            match view {
                "team_mine" => team.mine && !team.done,
                "team_unassigned" => team.assignee.is_none() && !team.done,
                "team_done" => team.done,
                _ => !team.done,
            }
        });
    }

    /// Everything about one shared conversation, for showing it.
    pub(crate) fn team_thread_view(&self, account_id: i64, thread_id: &str) -> Result<Value> {
        let account = self.store.account(account_id)?;
        let Some(team) = self.account_team(&account)? else {
            return Ok(Value::Null);
        };
        let key = self.require_team_key(team)?;
        let Some(info) = self.store.team_info(team)? else {
            return Ok(Value::Null);
        };
        let thread_key = self.thread_key_for(&key, &account, thread_id);
        let row = self.store.team_thread(&thread_key)?;
        let last_date = self.store.thread_last_incoming(account_id, thread_id)?;
        let members = self.store.team_member_emails(team)?;
        let name = |id: i64| {
            members
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "A former teammate".to_string())
        };
        let comments: Vec<Value> = self
            .store
            .team_comments_on(&thread_key)?
            .into_iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "author": name(c.author),
                    "mine": c.author == info.me,
                    "text": c.text,
                    "created_at": c.created_at,
                })
            })
            .collect();
        let (viewing, replying) = self.presence_on(team, &thread_key, info.me);
        Ok(json!({
            "team_id": team,
            "team_name": info.name,
            "assignee": row.as_ref().and_then(|r| r.assignee),
            "status": row.as_ref().map_or("open", |r| r.status.as_str()),
            "done": is_done(row.as_ref(), last_date),
            "sent_count": row.as_ref().map_or(0, |r| r.sent_count),
            "last_sent_by": row.as_ref().and_then(|r| r.last_sent_by).map(name),
            "sending_by": row
                .as_ref()
                .filter(|r| r.sending_until > unix_now())
                .and_then(|r| r.sending_by)
                .filter(|u| *u != info.me)
                .map(name),
            "comments": comments,
            "viewing": viewing,
            "replying": replying,
            "me": info.me,
            "can_manage": info.role != "member",
            "members": self.store.team_members(team)?,
        }))
    }

    /// The team a shared inbox belongs to, and its key.
    fn shared_key(&self, account: &Account) -> Result<TeamKey> {
        let team = self.account_team(account)?.ok_or_else(no_team)?;
        self.require_team_key(team)
    }

    pub(crate) async fn team_assign(
        &self,
        account_id: i64,
        thread_id: &str,
        expected: Option<i64>,
        assignee: Option<i64>,
    ) -> Result<Value> {
        let account = self.shared_account(account_id)?;
        let key = self.shared_key(&account)?;
        let thread_key = self.thread_key_for(&key, &account, thread_id);
        let result: Result<Value> = self
            .cloud_send_for_team(
                Some(key.team_id),
                Method::POST,
                &format!("/team/threads/{thread_key}/assign"),
                Some(json!({ "expected": expected, "assignee": assignee })),
            )
            .await;
        let _ = self.team_poll_one(&key, false).await;
        match result {
            Err(e) if offline_or(&e, "team_stale") => {
                let holder = self
                    .store
                    .team_thread(&thread_key)?
                    .and_then(|r| r.assignee);
                let message = match self.who(key.team_id, holder)? {
                    Some(who) => format!("{who} already has this one."),
                    None => "Someone just changed this. Check it and try again.".to_string(),
                };
                Err(CoreError::Cloud {
                    code: "team_stale".into(),
                    message,
                })
            }
            other => other,
        }
    }

    pub(crate) async fn team_set_status(
        &self,
        account_id: i64,
        thread_id: &str,
        status: &str,
    ) -> Result<Value> {
        let account = self.shared_account(account_id)?;
        let key = self.shared_key(&account)?;
        let thread_key = self.thread_key_for(&key, &account, thread_id);
        let expected = self
            .store
            .team_thread(&thread_key)?
            .map_or("open".to_string(), |r| r.status);
        let result: Result<Value> = self
            .cloud_send_for_team(
                Some(key.team_id),
                Method::POST,
                &format!("/team/threads/{thread_key}/status"),
                Some(json!({
                    "expected": expected,
                    "status": status,
                    "through": self.store.thread_last_incoming(account_id, thread_id)?,
                })),
            )
            .await;
        let _ = self.team_poll_one(&key, false).await;
        result
    }

    pub(crate) async fn team_comment(
        &self,
        account_id: i64,
        thread_id: &str,
        text: &str,
        mentions: Vec<i64>,
    ) -> Result<Value> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CoreError::Invalid("Write something first.".into()));
        }
        let account = self.shared_account(account_id)?;
        let key = self.shared_key(&account)?;
        let thread_key = self.thread_key_for(&key, &account, thread_id);
        let plain = serde_json::to_vec(&CommentBody {
            text: text.to_string(),
            mentions,
        })
        .map_err(|_| CoreError::Invalid("Could not save that comment.".into()))?;
        // The same number on a retry, so a dropped connection can't post it twice.
        let id = crypto::random_hex(16);
        let body = json!({ "id": id, "blob": crypto::vault_seal(&key.key, &plain)? });
        let path = format!("/team/threads/{thread_key}/comments");
        let team = Some(key.team_id);
        let mut sent: Result<Value> = self
            .cloud_send_for_team(team, Method::POST, &path, Some(body.clone()))
            .await;
        if sent.as_ref().is_err_and(|e| offline_or(e, "offline")) {
            sent = self
                .cloud_send_for_team(team, Method::POST, &path, Some(body))
                .await;
        }
        sent?;
        let _ = self.team_poll_one(&key, false).await;
        Ok(json!({ "id": id }))
    }

    pub(crate) async fn team_delete_comment(&self, id: &str) -> Result<Value> {
        let team = self.pick_team(self.store.team_comment_team(id)?)?;
        let key = self.require_team_key(team)?;
        let reply: Value = self
            .cloud_send_for_team(
                Some(team),
                Method::DELETE,
                &format!("/team/comments/{id}"),
                None,
            )
            .await?;
        let _ = self.team_poll_one(&key, false).await;
        Ok(reply)
    }

    /// Says what this person is doing with a conversation. Never worth an error.
    pub(crate) async fn team_presence(
        &self,
        account_id: i64,
        thread_id: &str,
        kind: &str,
    ) -> Result<Value> {
        let Ok(account) = self.shared_account(account_id) else {
            return Ok(json!({ "ok": false }));
        };
        let Ok(key) = self.shared_key(&account) else {
            return Ok(json!({ "ok": false }));
        };
        let thread_key = self.thread_key_for(&key, &account, thread_id);
        let _: Result<Value> = self
            .cloud_send_for_team(
                Some(key.team_id),
                Method::POST,
                &format!("/team/threads/{thread_key}/presence"),
                Some(json!({ "kind": kind })),
            )
            .await;
        Ok(json!({ "ok": true }))
    }

    pub(crate) fn team_templates(&self, team: Option<i64>) -> Result<Value> {
        Ok(json!(self.store.team_templates(team)?))
    }

    pub(crate) async fn team_save_template(
        &self,
        asked: Option<i64>,
        id: Option<&str>,
        name: &str,
        body: &str,
        expected_version: i64,
    ) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let key = self.require_team_key(team)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::Invalid("Give the template a name.".into()));
        }
        let plain = serde_json::to_vec(&TemplateBody {
            name: name.to_string(),
            body: body.to_string(),
        })
        .map_err(|_| CoreError::Invalid("Could not save that template.".into()))?;
        let id = id.map_or_else(|| crypto::random_hex(16), str::to_string);
        let result: Result<Value> = self
            .cloud_send_for_team(
                Some(team),
                Method::PUT,
                &format!("/team/templates/{id}"),
                Some(json!({
                    "expected_version": expected_version,
                    "blob": crypto::vault_seal(&key.key, &plain)?,
                })),
            )
            .await;
        let _ = self.team_poll_one(&key, false).await;
        match result {
            Err(e) if offline_or(&e, "team_stale") => Err(CoreError::Cloud {
                code: "team_stale".into(),
                message:
                    "A teammate just changed this template. Check their version and try again."
                        .into(),
            }),
            other => other,
        }
    }

    pub(crate) async fn team_delete_template(
        &self,
        asked: Option<i64>,
        id: &str,
        expected_version: i64,
    ) -> Result<Value> {
        let team = self.pick_team(asked)?;
        let key = self.require_team_key(team)?;
        let result: Result<Value> = self
            .cloud_send_for_team(
                Some(team),
                Method::PUT,
                &format!("/team/templates/{id}"),
                Some(json!({ "expected_version": expected_version, "blob": null })),
            )
            .await;
        let _ = self.team_poll_one(&key, false).await;
        result
    }

    /// Remembers how many team replies had gone out when someone starts a reply, so
    /// that's what they're warned against later.
    pub(crate) fn note_team_draft(
        &self,
        draft_id: i64,
        account: &Account,
        thread_id: &str,
    ) -> Result<()> {
        let Ok(key) = self.shared_key(account) else {
            return Ok(());
        };
        let thread_key = self.thread_key_for(&key, account, thread_id);
        let count = self
            .store
            .team_thread(&thread_key)?
            .map_or(0, |r| r.sent_count);
        self.store
            .save_draft_team_base(key.team_id, draft_id, &thread_key, count)
    }

    /// Asked at the very moment a reply is about to go. Our server being down must
    /// never stop an email, so only a clear "someone else did" holds it back.
    pub(crate) async fn team_before_send(
        &self,
        draft: &crate::model::Draft,
        account: &Account,
        force: bool,
    ) -> Result<Option<(TeamKey, String)>> {
        if !account.prefs.team_shared || (draft.mode != "reply" && draft.mode != "reply_all") {
            return Ok(None);
        }
        let Ok(key) = self.shared_key(account) else {
            return Ok(None);
        };
        // Saved when the reply was started, as a teammate may have moved the email since.
        let (thread_key, seen) = match self.store.draft_team_base(draft.id)? {
            Some(base) => base,
            None => {
                let Some(parent) = draft.parent_message_id else {
                    return Ok(None);
                };
                let Some(thread_id) = self.store.message_thread(parent)? else {
                    return Ok(None);
                };
                let thread_key = self.thread_key_for(&key, account, &thread_id);
                let count = self
                    .store
                    .team_thread(&thread_key)?
                    .map_or(0, |r| r.sent_count);
                (thread_key, count)
            }
        };
        // Quick, or not at all: a slow server of ours mustn't hold up an email.
        let asked = tokio::time::timeout(
            SEND_CHECK_LIMIT,
            self.cloud_send_for_team::<Value>(
                Some(key.team_id),
                Method::POST,
                &format!("/team/threads/{thread_key}/sending"),
                Some(json!({ "seen_count": seen, "force": force })),
            ),
        )
        .await;
        let Ok(result) = asked else {
            return Ok(None);
        };
        match result {
            Ok(_) => Ok(Some((key, thread_key))),
            Err(e) if offline_or(&e, "teammate_replied") || offline_or(&e, "teammate_sending") => {
                let replying = offline_or(&e, "teammate_sending");
                let _ = self.team_poll_one(&key, false).await;
                let row = self.store.team_thread(&thread_key)?;
                let who = self
                    .who(
                        key.team_id,
                        row.and_then(|r| {
                            if replying {
                                r.sending_by
                            } else {
                                r.last_sent_by
                            }
                        }),
                    )?
                    .unwrap_or_else(|| "A teammate".to_string());
                let (code, message) = if replying {
                    (
                        "teammate_sending",
                        format!("{who} is sending a reply to this right now."),
                    )
                } else {
                    (
                        "teammate_replied",
                        format!("{who} replied while you were writing."),
                    )
                };
                Err(CoreError::Send {
                    code: code.into(),
                    message,
                })
            }
            Err(_) => Ok(None),
        }
    }

    /// Whether the reply really went out. Failing to say so doesn't matter much:
    /// the hold runs out, and the Sent copy still shows up in the conversation.
    pub(crate) async fn team_after_send(&self, key: &TeamKey, thread_key: &str, sent: bool) {
        let _: Result<Value> = self
            .cloud_send_for_team(
                Some(key.team_id),
                Method::POST,
                &format!("/team/threads/{thread_key}/sent"),
                Some(json!({ "sent": sent })),
            )
            .await;
        let _ = self.team_poll_one(key, false).await;
    }
}

/// Done stays done only until something newer arrives in the conversation. Each
/// device works this out for itself, so reopening needs nothing saved anywhere and
/// can't conflict with anyone.
fn is_done(row: Option<&crate::store::TeamThreadRow>, last_date: i64) -> bool {
    row.is_some_and(|r| r.status == "done" && last_date <= r.status_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use crate::store::Store;
    use std::sync::Arc;

    /// Someone in teams 3 and 7, with an inbox another of their devices handed to a team,
    /// so this device knows it's shared but not which team has it.
    fn two_teams() -> (Engine, Account, TeamVault) {
        let engine = Engine::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryStore::default()),
        );
        engine.add_team_key(3, "k3".into()).unwrap();
        engine.add_team_key(7, "k7".into()).unwrap();
        let mut a = engine
            .store
            .insert_account_with_sync_id(
                "help@shop.test",
                "Help",
                "imap.shop.test",
                993,
                Security::Tls,
                "help@shop.test",
                "inbox-1",
            )
            .unwrap();
        engine.secrets.set(&account_secret(a.id), "pw").unwrap();
        a.prefs.team_shared = true;
        a.prefs.team_id = 0;
        engine.store.save_account(&a).unwrap();
        let mut seven = TeamVault {
            version: 1,
            ..Default::default()
        };
        seven
            .inboxes
            .insert("inbox-1".into(), engine.team_inbox_entry(&a).unwrap());
        (engine, a, seven)
    }

    fn team_of(engine: &Engine, a: &Account) -> Option<i64> {
        engine
            .store
            .list_accounts()
            .unwrap()
            .into_iter()
            .find(|x| x.id == a.id)
            .map(|x| x.prefs.team_id)
    }

    fn empty() -> TeamVault {
        TeamVault {
            version: 1,
            ..Default::default()
        }
    }

    #[test]
    fn a_shared_inbox_that_doesnt_say_its_team_goes_to_the_team_whose_list_has_it() {
        // Its own team's list first: claimed there, and the other list leaves it alone.
        let (engine, a, seven) = two_teams();
        engine
            .apply_team_vault(7, &seven, &BTreeMap::new())
            .unwrap();
        assert_eq!(team_of(&engine, &a), Some(7));
        engine
            .apply_team_vault(3, &empty(), &BTreeMap::new())
            .unwrap();
        assert_eq!(team_of(&engine, &a), Some(7));

        // The first team's list first, which is the one it would have been taken for.
        let (engine, a, seven) = two_teams();
        let others = BTreeMap::from([(7, Some(seven))]);
        engine.apply_team_vault(3, &empty(), &others).unwrap();
        assert_eq!(team_of(&engine, &a), Some(7));

        // When the other list can't be read, it waits rather than going.
        let (engine, a, seven) = two_teams();
        let unknown = BTreeMap::from([(7, None)]);
        engine.apply_team_vault(3, &empty(), &unknown).unwrap();
        assert_eq!(team_of(&engine, &a), Some(0));
        engine
            .apply_team_vault(7, &seven, &BTreeMap::new())
            .unwrap();
        assert_eq!(team_of(&engine, &a), Some(7));

        // On no team's list at all, it goes.
        let (engine, a, _) = two_teams();
        let neither = BTreeMap::from([(7, Some(empty()))]);
        engine.apply_team_vault(3, &empty(), &neither).unwrap();
        assert_eq!(team_of(&engine, &a), None);
    }

    #[test]
    fn leaving_one_team_keeps_a_shared_inbox_another_team_may_have() {
        let (engine, a, _) = two_teams();
        engine.clear_team_here(3).unwrap();
        assert_eq!(team_of(&engine, &a), Some(0));
    }

    #[test]
    fn a_missing_list_is_only_empty_for_a_team_that_never_had_one() {
        let (engine, _, _) = two_teams();
        let key = engine.team_key(3).unwrap().unwrap();
        let never = VaultReply {
            version: 0,
            blob: None,
        };
        assert!(engine
            .read_team_vault(&key, &never)
            .unwrap()
            .inboxes
            .is_empty());
        let lost = VaultReply {
            version: 4,
            blob: None,
        };
        assert!(engine.read_team_vault(&key, &lost).is_err());
        engine
            .store
            .save_team_info(3, "Shop", 1, "admin", 1, &[])
            .unwrap();
        engine.store.set_team_vault_seen(3, 2).unwrap();
        assert!(engine.read_team_vault(&key, &never).is_err());
    }
}
