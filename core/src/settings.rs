//! The person's choices about how the app behaves.
//!
//! Each setting has a fixed list of allowed values. Some follow the person to their
//! other devices, encrypted in the vault, and some belong to this device because a
//! laptop and a desktop often differ.

use crate::ai::AI_KEY;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

struct Def {
    key: &'static str,
    default: &'static str,
    allowed: &'static [&'static str],
    follows_you: bool,
}

const ON_OFF: &[&str] = &["on", "off"];
const HOURS: &[&str] = &[
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
    "17", "18", "19", "20", "21", "22", "23",
];

const fn follows(
    key: &'static str,
    default: &'static str,
    allowed: &'static [&'static str],
) -> Def {
    Def {
        key,
        default,
        allowed,
        follows_you: true,
    }
}

const fn here(key: &'static str, default: &'static str, allowed: &'static [&'static str]) -> Def {
    Def {
        key,
        default,
        allowed,
        follows_you: false,
    }
}

/// How the app looks and sits on this device, which is nothing to do with who's signed in.
pub(crate) const STAYS_ON_THIS_DEVICE: &[&str] = &[
    "theme",
    "reading_size",
    "density",
    "layout",
    "preview_lines",
    "list_pictures",
    "delete_effect",
    "ai_on_device",
    "dock_badge",
    "open_at_login",
    "show_in_menu_bar",
    "show_in_dock",
    "auto_update",
    "update_channel",
];

const DEFS: &[Def] = &[
    // General
    follows("open_first", "all", &["all", "selected"]),
    follows(
        "open_tab",
        "all",
        &["all", "needs_you", "good_to_know", "rest"],
    ),
    follows("conversations", "on", ON_OFF),
    follows("after_action", "next", &["next", "previous", "list"]),
    follows("mark_read", "now", &["now", "delay", "manual"]),
    follows("delete_key", "trash", &["trash", "archive"]),
    // What happens to a sign-in email once its code is copied.
    follows(
        "after_code_copy",
        "trash",
        &["trash", "archive", "read", "keep"],
    ),
    // Writing
    follows("undo_send", "10", &["0", "5", "10", "20", "30"]),
    follows(
        "font",
        "system",
        &["system", "arial", "georgia", "times", "courier"],
    ),
    follows("font_size", "14", &["13", "14", "16", "18"]),
    follows("reply_default", "reply", &["reply", "reply_all"]),
    follows("quote", "on", ON_OFF),
    follows("spell_check", "on", ON_OFF),
    // Privacy
    follows("remote_images", "always", &["ask", "always"]),
    follows(SENDER_PICTURES, "on", ON_OFF),
    follows("ai_search", "on", ON_OFF),
    follows("ai_spam", "off", ON_OFF),
    follows("ai_sort", "off", ON_OFF),
    follows("ai_auto", "off", &["off", "important", "everything"]),
    // Writes a reply ahead of time for mail that needs the person, with their own AI.
    follows("ai_draft_ahead", "off", ON_OFF),
    follows("ai_summaries", "on", ON_OFF),
    follows("ai_replies", "on", ON_OFF),
    // Checks each AI draft with the quick model and rewrites it once if it's weak.
    follows("ai_best_drafts", "off", ON_OFF),
    follows("ai_translate", "on", ON_OFF),
    follows("ai_chat", "on", ON_OFF),
    follows("ai_rules", "on", ON_OFF),
    follows("ai_service", "anthropic", crate::ai::AI_SERVICE_IDS),
    // A plan runs a program installed on this Mac, so it belongs to this Mac alone.
    here(
        "ai_via",
        crate::plans::VIA_KEY,
        &[crate::plans::VIA_KEY, "claude_code", "codex"],
    ),
    // Apple Intelligence, when this Mac has it and nothing else is connected.
    here("ai_on_device", "on", ON_OFF),
    // Ask AI on Apple Intelligence, picked in its header. Only Ask AI: replies and the rest
    // keep the plan or key connected.
    here("ask_ai_apple", "off", ON_OFF),
    // Other AI apps on this Mac. Off until the person turns it on.
    // Asks for Touch ID or the Mac's password: when the app opens, and after time away.
    here("app_lock", "off", &["off", "start", "away"]),
    here("mcp_on", "off", ON_OFF),
    here("mcp_drafts", "off", ON_OFF),
    here("mcp_organize", "off", ON_OFF),
    here("mcp_delete", "off", ON_OFF),
    here("mcp_attachments", "off", ON_OFF),
    here("mcp_spam_trash", "off", ON_OFF),
    here("mcp_send", "off", ON_OFF),
    here("mcp_send_unasked", "off", ON_OFF),
    here("mcp_delete_forever", "off", ON_OFF),
    here("mcp_suspicious", "off", ON_OFF),
    here("mcp_drafts_shared", "off", ON_OFF),
    follows("screener", "off", ON_OFF),
    follows("search_meaning", "on", ON_OFF),
    follows("drafts_on_server", "on", ON_OFF),
    follows("keyboard_keys", "on", ON_OFF),
    // Spam and sorting. Nothing moves until the person has seen the trial.
    follows("spam_mode", "trial", &["trial", "move"]),
    follows("smart_tabs", "on", ON_OFF),
    // Appearance, on this device
    here("theme", "system", &["system", "light", "dark"]),
    here(
        "reading_size",
        "normal",
        &["small", "normal", "large", "xlarge"],
    ),
    here("density", "comfortable", &["comfortable", "compact"]),
    // The list beside the open email, or the list alone with the email opening over it, like Gmail.
    here("layout", "side", &["side", "full"]),
    here("preview_lines", "1", &["0", "1", "2"]),
    here("list_pictures", "on", ON_OFF),
    // The paper shredder when deleting or unsubscribing.
    here("delete_effect", "sound", &["sound", "quiet", "off"]),
    here("dock_badge", "on", ON_OFF),
    // How the app sits on this Mac. Nothing starts by itself until the person asks.
    here("open_at_login", "off", ON_OFF),
    here("show_in_menu_bar", "on", ON_OFF),
    // A newly installed app is easy to find in the Dock, not only beside the clock.
    here("show_in_dock", "on", ON_OFF),
    here("auto_update", "on", ON_OFF),
    here("update_channel", "stable", &["stable", "beta"]),
    // Notifications, on this device
    here("alerts", "on", ON_OFF),
    here("alert_which", "all", &["all", "people", "important"]),
    here("alert_sound", "on", ON_OFF),
    here("alert_tone", "Chime", ALERT_TONES),
    here("alert_details", "on", ON_OFF),
    here("alert_preview", "off", ON_OFF),
    // Who a new email is from, next to the menu bar icon for a few seconds.
    here("alert_menu_bar", "on", ON_OFF),
    here("quiet_hours", "off", ON_OFF),
    here("quiet_start", "22", HOURS),
    here("quiet_end", "7", HOURS),
    // How much mail stays on this device. A year by default: a big mailbox could otherwise
    // fill many gigabytes with mail nobody opens. Older mail still shows and downloads when
    // opened, and nothing already saved is removed.
    here("keep_mail", "year", &["all", "year", "quarter"]),
    // Stops bringing down older mail until switched off again. New mail still comes in.
    here("download_paused", "off", ON_OFF),
];

pub const SENDER_PICTURES: &str = "sender_pictures";
/// The alert sounds a person can pick from, which come with the app.
/// The one the Mac's own Mail app plays.
pub const MAC_MAIL_TONE: &str = "Mac Mail";
pub const ALERT_TONES: &[&str] = &["Chime", "Marimba", "Glass", "Keys", MAC_MAIL_TONE];
/// Pins, snoozes and reminders, one setting each as they travel between devices.
pub(crate) const MARK_PREFIX: &str = "mark:";
/// When each setting that follows you last changed, so changes on two devices both count.
pub(crate) const KEY_TIMES: &str = "shared:key_times";
/// What the settings were last time, kept scrambled, to tell what's changed since.
const SNAPSHOT: &str = "local:settings_snapshot";
/// When the settings that follow you last changed, so the newest wins.
pub(crate) const SETTINGS_STAMP: &str = "settings_changed_at";
/// Senders the person chose to show pictures for are kept as `images:<address>`.
const IMAGE_SENDER: &str = "images:";
const AI_KEY_IN_VAULT: &str = "secret:ai_key";

fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// Whether a stored setting travels in the vault.
fn follows_you(key: &str) -> bool {
    key.starts_with(IMAGE_SENDER)
        || key.starts_with(crate::spam::BLOCKED)
        || key.starts_with(crate::spam::TRUSTED)
        || key.starts_with(crate::spam::SORTED)
        || key == crate::spam::SHARED_WORDS
        || key == KEY_TIMES
        || key == crate::ai::AI_PICKS
        || key.starts_with(MARK_PREFIX)
        || key.starts_with(crate::people::PERSON_PREFIX)
        || key.starts_with(crate::rules::RULE_PREFIX)
        || key.starts_with(crate::screen::LET_IN)
        || key.starts_with(crate::screen::TEMPLATE_PREFIX)
        || key.starts_with(crate::newsletters::UNSUBSCRIBED)
        || def(key).is_some_and(|d| d.follows_you)
}

fn key_times(map: &BTreeMap<String, String>) -> BTreeMap<String, i64> {
    map.get(KEY_TIMES)
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default()
}

/// Enough to tell whether a value changed, without keeping another copy of it.
fn fingerprints(map: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    map.iter()
        .map(|(k, v)| {
            (
                k.clone(),
                data_encoding::HEXLOWER.encode(&Sha256::digest(v.as_bytes())),
            )
        })
        .collect()
}

impl Engine {
    pub fn setting(&self, key: &str) -> Result<String> {
        Ok(match self.store.setting(key)? {
            Some(value) if def(key).is_none_or(|d| d.allowed.contains(&value.as_str())) => value,
            _ => def(key).map(|d| d.default).unwrap_or("").to_string(),
        })
    }

    pub(crate) fn app_settings(&self) -> Result<Value> {
        let mut out = Map::new();
        for d in DEFS {
            out.insert(d.key.to_string(), json!(self.setting(d.key)?));
        }
        Ok(Value::Object(out))
    }

    pub(crate) async fn set_app_setting(&self, key: &str, value: &str) -> Result<Value> {
        let d = def(key).ok_or_else(|| CoreError::Invalid("Bad request.".into()))?;
        if !d.allowed.contains(&value) {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        // With neither, there'd be no way back to the app.
        let other_place = match key {
            "show_in_menu_bar" => Some("show_in_dock"),
            "show_in_dock" => Some("show_in_menu_bar"),
            _ => None,
        };
        if let (Some(other), "off") = (other_place, value) {
            if self.setting(other)? == "off" {
                return Err(CoreError::Invalid(
                    "Mailzo has to stay in the menu bar or the Dock.".into(),
                ));
            }
        }
        self.store.set_setting(key, value)?;
        if key == SENDER_PICTURES && value == "off" {
            self.store.clear_avatars()?;
        }
        // Whoever was waiting comes in, rather than being stuck where it can't be seen.
        if key == "screener" && value == "off" {
            self.store.clear_all_screened()?;
        }
        // Choosing to move spam here is a yes to what the trial has already found.
        if key == "spam_mode" && value == "move" {
            self.store.first_look_is_news()?;
        }
        if key == "drafts_on_server" && value == "off" {
            self.take_drafts_off_server()?;
        }
        if d.follows_you {
            self.settings_changed().await?;
        }
        Ok(json!({ "saved": true }))
    }

    pub(crate) async fn set_sender_images(&self, address: &str, allowed: bool) -> Result<Value> {
        self.store.set_image_sender(address, allowed)?;
        self.settings_changed().await?;
        Ok(json!({ "allowed": allowed }))
    }

    /// Marks the shared settings as changed, and tells other devices if it can.
    pub(crate) async fn settings_changed(&self) -> Result<()> {
        self.store
            .set_setting(SETTINGS_STAMP, &crate::identity::unix_now().to_string())?;
        // Telling other devices can wait if this one is offline.
        let _ = self.vault_sync().await;
        Ok(())
    }

    /// The settings that travel in the vault, and when they last changed.
    pub(crate) fn shared_settings(&self) -> Result<(BTreeMap<String, String>, i64)> {
        let all = self.store.all_settings()?;
        let stamp = all
            .get(SETTINGS_STAMP)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut shared: BTreeMap<String, String> =
            all.into_iter().filter(|(k, _)| follows_you(k)).collect();
        // The vault is sealed, so the key is safe here. It stays out of the database.
        if let Some(key) = self.secrets.get(AI_KEY)? {
            shared.insert(AI_KEY_IN_VAULT.into(), key);
        }
        self.note_key_times(&mut shared)?;
        Ok((shared, stamp))
    }

    /// Notices which settings have changed since last time, and when.
    fn note_key_times(&self, shared: &mut BTreeMap<String, String>) -> Result<()> {
        let mut times = key_times(shared);
        shared.remove(KEY_TIMES);
        let before: BTreeMap<String, String> = self
            .store
            .setting(SNAPSHOT)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let now = crate::identity::unix_now();
        let current = fingerprints(shared);
        let mut touched = false;
        for key in before.keys().chain(current.keys()) {
            if before.get(key) != current.get(key) {
                times.insert(key.clone(), now);
                touched = true;
            }
        }
        let times_text = serde_json::to_string(&times).unwrap_or_else(|_| "{}".into());
        if touched {
            self.store.set_setting(
                SNAPSHOT,
                &serde_json::to_string(&current).unwrap_or_default(),
            )?;
            self.store.set_setting(KEY_TIMES, &times_text)?;
        }
        shared.insert(KEY_TIMES.to_string(), times_text);
        Ok(())
    }

    /// Two copies of the settings as one. Whichever device changed a setting last wins for
    /// that setting, so a device that's been closed for days can't undo everything.
    pub(crate) fn merge_settings(
        &self,
        here: &BTreeMap<String, String>,
        here_at: i64,
        there: &BTreeMap<String, String>,
        there_at: i64,
    ) -> BTreeMap<String, String> {
        let (here_times, there_times) = (key_times(here), key_times(there));
        let mut merged = BTreeMap::new();
        let mut times = BTreeMap::new();
        for key in here.keys().chain(there.keys()) {
            if key == KEY_TIMES || merged.contains_key(key) || times.contains_key(key) {
                continue;
            }
            let (a, b) = (
                here_times.get(key).copied().unwrap_or(0),
                there_times.get(key).copied().unwrap_or(0),
            );
            // A person kept on both devices goes by when the person themselves last changed.
            let theirs = match crate::people::newer_copy(key, here.get(key), there.get(key)) {
                Some(theirs) => theirs,
                None => b > a || (a == b && there_at > here_at),
            };
            let value = if theirs {
                there.get(key)
            } else {
                here.get(key)
            };
            if let Some(v) = value {
                merged.insert(key.clone(), v.clone());
            }
            times.insert(key.clone(), a.max(b));
        }
        // Remembering when something was removed stops an older copy bringing it back.
        for (key, t) in here_times.iter().chain(there_times.iter()) {
            let at = times.entry(key.clone()).or_insert(0);
            *at = (*at).max(*t);
        }
        merged.insert(
            KEY_TIMES.to_string(),
            serde_json::to_string(&times).unwrap_or_else(|_| "{}".into()),
        );
        merged
    }

    /// Replaces the shared settings here with newer ones from another device.
    pub(crate) fn apply_shared_settings(
        &self,
        incoming: &BTreeMap<String, String>,
        stamp: i64,
    ) -> Result<()> {
        for (key, _) in self.store.all_settings()? {
            if follows_you(&key) && !incoming.contains_key(&key) {
                self.store.delete_setting(&key)?;
            }
        }
        for (key, value) in incoming {
            // Only what this version understands. Anything else stays out.
            if follows_you(key) {
                self.store.set_setting(key, value)?;
            }
        }
        if let Some(words) = incoming.get(crate::spam::SHARED_WORDS) {
            self.merge_shared_words(words)?;
        }
        let copies: Vec<crate::later::MarkCopy> = incoming
            .iter()
            .filter(|(k, _)| k.starts_with(MARK_PREFIX))
            .filter_map(|(_, v)| serde_json::from_str(v).ok())
            .collect();
        self.store.replace_marks(&copies)?;
        crate::people::receive_from_settings(&self.store, incoming)?;
        // A sender sorted elsewhere moves to that tab here too.
        for (key, value) in incoming {
            if let Some(address) = key.strip_prefix(crate::spam::SORTED) {
                // Another device may be on an older version, with the old tab names.
                let tab = match value.as_str() {
                    "people" | "notifications" => "general",
                    other => other,
                };
                self.store.set_sender_category(address, tab)?;
            }
        }
        match incoming.get(AI_KEY_IN_VAULT) {
            Some(key) => self.secrets.set(AI_KEY, key)?,
            None => self.secrets.delete(AI_KEY)?,
        }
        self.store.set_setting(SETTINGS_STAMP, &stamp.to_string())?;
        // What just arrived isn't a change made here.
        let mut plain = incoming.clone();
        plain.remove(KEY_TIMES);
        self.store.set_setting(
            SNAPSHOT,
            &serde_json::to_string(&fingerprints(&plain)).unwrap_or_default(),
        )?;
        if incoming.get("screener").map(String::as_str) != Some("on") {
            self.store.clear_all_screened()?;
        }
        for key in incoming.keys() {
            if let Some(address) = key.strip_prefix(crate::screen::LET_IN) {
                self.store.clear_screened_for(address)?;
            }
        }
        if incoming.get(SENDER_PICTURES).map(String::as_str) == Some("off") {
            self.store.clear_avatars()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryStore, Store};
    use std::sync::Arc;

    fn copy(pairs: &[(&str, &str, i64)]) -> BTreeMap<String, String> {
        let mut map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v, _)| (k.to_string(), v.to_string()))
            .collect();
        let times: BTreeMap<&str, i64> = pairs.iter().map(|(k, _, t)| (*k, *t)).collect();
        map.insert(KEY_TIMES.into(), serde_json::to_string(&times).unwrap());
        map
    }

    #[test]
    fn each_setting_keeps_whichever_device_changed_it_last() {
        let engine = Engine::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryStore::default()),
        );
        let here = copy(&[("theme", "dark", 200), ("font", "big", 100)]);
        let there = copy(&[("theme", "light", 100), ("font", "small", 200)]);
        // The other copy was saved later overall, which isn't what decides it.
        let merged = engine.merge_settings(&here, 100, &there, 900);
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["font"], "small");
    }
}
