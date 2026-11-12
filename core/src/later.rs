//! Dealing with mail later: snooze, reminders, pin, mute and scheduled sending.
//!
//! All of this lives on the device and travels between devices in the vault, so the
//! mail server never sees it. Mail stays in place on the server the whole time.

use crate::alerts::Alert;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::unix_now;
use crate::model::*;
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Marks can outlive their mail, and shouldn't be counted once it's gone.
const HAS_MAIL: &str = "AND EXISTS (SELECT 1 FROM messages m
    WHERE m.account_id = k.account_id AND m.thread_id = k.thread_id)";

/// What's kept for one conversation, as it travels between devices.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarkCopy {
    /// Which inbox, by the name every device knows it by.
    pub inbox: String,
    pub thread: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub snooze_until: Option<i64>,
    #[serde(default)]
    pub snoozed_at: Option<i64>,
    #[serde(default)]
    pub follow_at: Option<i64>,
    #[serde(default)]
    pub follow_set_at: Option<i64>,
}

/// A conversation that just came back, for telling the person.
pub struct Returned {
    pub account_id: i64,
    pub thread_id: String,
    /// `snooze` or `follow_up`.
    pub why: &'static str,
}

impl Store {
    /// Rows with nothing left to remember are removed, so this table stays small.
    fn tidy_mark(db: &rusqlite::Connection, account_id: i64, thread_id: &str) -> Result<()> {
        db.execute(
            "DELETE FROM thread_marks WHERE account_id = ?1 AND thread_id = ?2 AND pinned = 0 AND muted = 0
               AND snooze_until IS NULL AND returned_at IS NULL AND follow_at IS NULL AND follow_due = 0 AND priority IS NULL",
            params![account_id, thread_id],
        )?;
        Ok(())
    }

    fn with_mark(
        &self,
        account_id: i64,
        thread_id: &str,
        change: &str,
        value: Option<i64>,
    ) -> Result<()> {
        let db = self.db();
        db.execute(
            "INSERT OR IGNORE INTO thread_marks (account_id, thread_id) VALUES (?1, ?2)",
            params![account_id, thread_id],
        )?;
        let sql =
            format!("UPDATE thread_marks SET {change} WHERE account_id = ?1 AND thread_id = ?2");
        if change.contains("?3") {
            db.execute(&sql, params![account_id, thread_id, value])?;
        } else {
            db.execute(&sql, params![account_id, thread_id])?;
        }
        Self::tidy_mark(&db, account_id, thread_id)
    }

    pub fn set_pinned(&self, account_id: i64, thread_id: &str, on: bool) -> Result<()> {
        self.with_mark(account_id, thread_id, "pinned = ?3", Some(i64::from(on)))
    }

    pub fn set_muted(&self, account_id: i64, thread_id: &str, on: bool) -> Result<()> {
        self.with_mark(account_id, thread_id, "muted = ?3", Some(i64::from(on)))
    }

    /// `None` brings it back now.
    pub fn set_snooze(&self, account_id: i64, thread_id: &str, until: Option<i64>) -> Result<()> {
        self.with_mark(
            account_id,
            thread_id,
            "snooze_until = ?3, snoozed_at = CASE WHEN ?3 IS NULL THEN NULL ELSE unixepoch() END,
             returned_at = NULL",
            until,
        )
    }

    pub fn set_follow_up(&self, account_id: i64, thread_id: &str, at: Option<i64>) -> Result<()> {
        self.with_mark(
            account_id,
            thread_id,
            "follow_at = ?3, follow_set_at = CASE WHEN ?3 IS NULL THEN NULL ELSE unixepoch() END,
             follow_due = 0",
            at,
        )
    }

    /// It's been looked at or dealt with, so it no longer needs to stand out.
    pub fn clear_returned(&self, account_id: i64, thread_id: &str) -> Result<()> {
        let db = self.db();
        db.execute(
            "UPDATE thread_marks SET returned_at = NULL, follow_due = 0
             WHERE account_id = ?1 AND thread_id = ?2 AND (returned_at IS NOT NULL OR follow_due = 1)",
            params![account_id, thread_id],
        )?;
        Self::tidy_mark(&db, account_id, thread_id)
    }

    /// Which conversation an email belongs to, for rows named `msg:<id>`.
    pub fn real_thread(&self, thread_id: &str) -> Result<String> {
        match thread_id
            .strip_prefix("msg:")
            .and_then(|i| i.parse::<i64>().ok())
        {
            Some(id) => self
                .message_thread(id)?
                .ok_or_else(|| CoreError::NotFound("message".into())),
            None => Ok(thread_id.to_string()),
        }
    }

    pub fn is_muted(&self, account_id: i64, rfc_id: &str) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM messages m JOIN thread_marks k
                   ON k.account_id = m.account_id AND k.thread_id = m.thread_id
                 WHERE m.rfc_id = ?1 AND m.account_id = ?2 AND k.muted = 1 LIMIT 1",
                params![rfc_id, account_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn marked_with(&self, condition: &str) -> Result<Vec<(i64, String)>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT account_id, thread_id FROM thread_marks WHERE {condition}"
        ))?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Did someone else write in this conversation after this time?
    fn replied_since(
        &self,
        account_id: i64,
        thread_id: &str,
        since: i64,
        mine: &[String],
    ) -> Result<bool> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT lower(m.from_addr) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.thread_id = ?2 AND m.date > ?3
               AND f.role NOT IN ('sent', 'drafts', 'trash', 'junk')",
        )?;
        let rows = st.query_map(params![account_id, thread_id, since], |r| {
            r.get::<_, String>(0)
        })?;
        for row in rows {
            if !mine.contains(&row?) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether, when a reminder was set, the newest email in the conversation was someone
    /// else's: then the reminder is to answer it, not to chase a reply.
    fn was_waiting_on_me(
        &self,
        account_id: i64,
        thread_id: &str,
        at: i64,
        mine: &[String],
    ) -> Result<bool> {
        let last: Option<String> = self
            .db()
            .query_row(
                "SELECT lower(m.from_addr) FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE m.account_id = ?1 AND m.thread_id = ?2 AND m.date <= ?3
                   AND f.role NOT IN ('drafts', 'trash', 'junk')
                 ORDER BY m.date DESC LIMIT 1",
                params![account_id, thread_id, at],
                |r| r.get(0),
            )
            .optional()?;
        Ok(last.is_some_and(|from| !mine.contains(&from)))
    }

    /// Whether the person wrote in the conversation after `since`.
    fn i_wrote_since(
        &self,
        account_id: i64,
        thread_id: &str,
        since: i64,
        mine: &[String],
    ) -> Result<bool> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT lower(m.from_addr) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.thread_id = ?2 AND m.date > ?3
               AND f.role NOT IN ('drafts', 'trash', 'junk') AND m.is_draft = 0",
        )?;
        let rows = st.query_map(params![account_id, thread_id, since], |r| {
            r.get::<_, String>(0)
        })?;
        for row in rows {
            if mine.contains(&row?) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A reminder is done with once someone else replies. One set to answer someone else's
    /// email is also done once the person writes back.
    fn reminder_answered(
        &self,
        account_id: i64,
        thread_id: &str,
        set_at: i64,
        mine: &[String],
    ) -> Result<bool> {
        Ok(self.replied_since(account_id, thread_id, set_at, mine)?
            || (self.was_waiting_on_me(account_id, thread_id, set_at, mine)?
                && self.i_wrote_since(account_id, thread_id, set_at, mine)?))
    }

    /// Brings back whatever's due. New mail in a snoozed conversation brings it back
    /// early, and a reply cancels a reminder.
    pub fn bring_back_due(&self, now: i64, mine: &[String]) -> Result<Vec<Returned>> {
        let mut back = Vec::new();
        let mine_of = |_account: i64| mine;
        for (account_id, thread_id) in self.marked_with("snooze_until IS NOT NULL")? {
            let (until, at): (i64, i64) = self.db().query_row(
                "SELECT snooze_until, COALESCE(snoozed_at, 0) FROM thread_marks
                 WHERE account_id = ?1 AND thread_id = ?2",
                params![account_id, thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if until <= now
                || self.replied_since(account_id, &thread_id, at, mine_of(account_id))?
            {
                self.with_mark(
                    account_id,
                    &thread_id,
                    "snooze_until = NULL, snoozed_at = NULL, returned_at = ?3",
                    Some(now),
                )?;
                back.push(Returned {
                    account_id,
                    thread_id,
                    why: "snooze",
                });
            }
        }
        for (account_id, thread_id) in self.marked_with("follow_at IS NOT NULL")? {
            let (at, set_at): (i64, i64) = self.db().query_row(
                "SELECT follow_at, COALESCE(follow_set_at, 0) FROM thread_marks
                 WHERE account_id = ?1 AND thread_id = ?2",
                params![account_id, thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if self.reminder_answered(account_id, &thread_id, set_at, mine_of(account_id))? {
                self.set_follow_up(account_id, &thread_id, None)?;
            } else if at <= now {
                self.with_mark(
                    account_id,
                    &thread_id,
                    "follow_at = NULL, follow_due = 1",
                    None,
                )?;
                back.push(Returned {
                    account_id,
                    thread_id,
                    why: "follow_up",
                });
            }
        }
        // A reply can still arrive after the reminder came up.
        for (account_id, thread_id) in self.marked_with("follow_due = 1")? {
            let set_at: i64 = self.db().query_row(
                "SELECT COALESCE(follow_set_at, 0) FROM thread_marks WHERE account_id = ?1 AND thread_id = ?2",
                params![account_id, thread_id],
                |r| r.get(0),
            )?;
            if self.reminder_answered(account_id, &thread_id, set_at, mine_of(account_id))? {
                self.with_mark(
                    account_id,
                    &thread_id,
                    "follow_due = 0, follow_set_at = NULL",
                    None,
                )?;
            }
        }
        Ok(back)
    }

    pub fn later_counts(&self) -> Result<Value> {
        let db = self.db();
        let count = |sql: &str| -> Result<i64> { Ok(db.query_row(sql, [], |r| r.get(0))?) };
        Ok(json!({
            "snoozed": count(&format!("SELECT COUNT(*) FROM thread_marks k WHERE snooze_until IS NOT NULL {HAS_MAIL}"))?,
            "reminders": count(&format!("SELECT COUNT(*) FROM thread_marks k WHERE follow_at IS NOT NULL {HAS_MAIL}"))?,
            "muted": count(&format!("SELECT COUNT(*) FROM thread_marks k WHERE muted = 1 {HAS_MAIL}"))?,
            "screener": count(
                "SELECT COUNT(DISTINCT lower(m.from_addr)) FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE m.screened = 1 AND f.role = 'inbox'"
            )?,
        }))
    }

    /// The conversations put aside, with where to act on them from.
    pub fn list_marked(&self, kind: &str) -> Result<Vec<ThreadSummary>> {
        let which = match kind {
            "snoozed" => "snooze_until IS NOT NULL",
            "reminders" => "follow_at IS NOT NULL",
            "muted" => "muted = 1",
            "no_reply" => return self.no_reply_threads(),
            "deadlines" => return self.threads_with_deadlines(),
            "purchases" => return self.purchase_threads(),
            _ => return Err(CoreError::Invalid("Bad request.".into())),
        };
        let mut out = Vec::new();
        for (account_id, thread_id) in self.marked_with(which)? {
            if let Some(summary) = self.summary_of(account_id, &thread_id)? {
                out.push(summary);
            }
        }
        out.sort_by_key(|t| match kind {
            "snoozed" => t.snooze_until.unwrap_or(0),
            "reminders" => t.follow_at.unwrap_or(0),
            _ => -t.last_date,
        });
        Ok(out)
    }

    /// Pinned, snoozed and muted conversations, for carrying to other devices.
    pub fn mark_copies(&self) -> Result<Vec<MarkCopy>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT a.sync_id, k.thread_id, k.pinned, k.muted, k.snooze_until, k.snoozed_at,
                    k.follow_at, k.follow_set_at
             FROM thread_marks k JOIN accounts a ON a.id = k.account_id
             WHERE k.pinned = 1 OR k.muted = 1 OR k.snooze_until IS NOT NULL OR k.follow_at IS NOT NULL
             ORDER BY a.sync_id, k.thread_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(MarkCopy {
                inbox: r.get(0)?,
                thread: r.get(1)?,
                pinned: r.get(2)?,
                muted: r.get(3)?,
                snooze_until: r.get(4)?,
                snoozed_at: r.get(5)?,
                follow_at: r.get(6)?,
                follow_set_at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces this device's marks with the ones that came from another device.
    pub fn replace_marks(&self, copies: &[MarkCopy]) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        // What's come back on this device stays until it's been looked at.
        tx.execute(
            "UPDATE thread_marks SET pinned = 0, muted = 0, snooze_until = NULL, snoozed_at = NULL,
               follow_at = NULL, follow_set_at = CASE WHEN follow_due = 1 THEN follow_set_at END",
            [],
        )?;
        for c in copies {
            let account: Option<i64> = tx
                .query_row(
                    "SELECT id FROM accounts WHERE sync_id = ?1",
                    [&c.inbox],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(account) = account else { continue };
            tx.execute(
                "INSERT INTO thread_marks (account_id, thread_id, pinned, muted, snooze_until, snoozed_at, follow_at, follow_set_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (account_id, thread_id) DO UPDATE SET pinned = ?3, muted = ?4,
                   snooze_until = ?5, snoozed_at = ?6, follow_at = ?7, follow_set_at = ?8",
                params![account, c.thread, c.pinned, c.muted, c.snooze_until, c.snoozed_at, c.follow_at, c.follow_set_at],
            )?;
        }
        tx.execute(
            "DELETE FROM thread_marks WHERE pinned = 0 AND muted = 0 AND snooze_until IS NULL
               AND returned_at IS NULL AND follow_at IS NULL AND follow_due = 0 AND priority IS NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ---- scheduled sending ----

    pub fn set_send_at(
        &self,
        draft_id: i64,
        send_at: Option<i64>,
        remind_days: Option<i64>,
    ) -> Result<()> {
        let n = self.db().execute(
            "UPDATE drafts SET send_at = ?2, remind_days = ?3, send_problem = NULL WHERE id = ?1",
            params![draft_id, send_at, remind_days],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound("draft".into()));
        }
        Ok(())
    }

    pub fn set_send_problem(&self, draft_id: i64, problem: &str) -> Result<()> {
        self.db().execute(
            "UPDATE drafts SET send_at = NULL, send_problem = ?2 WHERE id = ?1",
            params![draft_id, problem],
        )?;
        Ok(())
    }

    pub fn drafts_due(&self, now: i64) -> Result<Vec<i64>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id FROM drafts WHERE send_at IS NOT NULL AND send_at <= ?1 AND sent_at IS NULL
             ORDER BY send_at",
        )?;
        let rows = st.query_map([now], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

impl Engine {
    pub(crate) fn own_addresses(&self) -> Result<Vec<String>> {
        Ok(self
            .store
            .list_accounts()?
            .iter()
            .flat_map(|a| {
                std::iter::once(a.email.to_lowercase())
                    .chain(a.prefs.aliases.iter().map(|x| x.address.to_lowercase()))
            })
            .collect())
    }

    /// Other devices find out about pins and snoozes the same way as settings.
    pub(crate) async fn marks_changed(&self) -> Result<()> {
        // One setting each, so two devices changing different conversations both count.
        let mut wanted = std::collections::BTreeMap::new();
        for copy in self.store.mark_copies()? {
            wanted.insert(
                format!(
                    "{}{}:{}",
                    crate::settings::MARK_PREFIX,
                    copy.inbox,
                    copy.thread
                ),
                serde_json::to_string(&copy).unwrap_or_default(),
            );
        }
        for key in self.store.all_settings()?.into_keys() {
            if key.starts_with(crate::settings::MARK_PREFIX) && !wanted.contains_key(&key) {
                self.store.delete_setting(&key)?;
            }
        }
        for (key, value) in &wanted {
            self.store.set_setting(key, value)?;
        }
        self.settings_changed().await
    }

    pub(crate) async fn thread_pin(
        &self,
        account_id: i64,
        thread_id: &str,
        on: bool,
    ) -> Result<Value> {
        let thread = self.store.real_thread(thread_id)?;
        self.store.set_pinned(account_id, &thread, on)?;
        self.marks_changed().await?;
        Ok(json!({ "pinned": on }))
    }

    /// Muted conversations leave the inbox and stop notifying, including on new replies.
    pub(crate) async fn thread_mute(
        &self,
        account_id: i64,
        thread_id: &str,
        on: bool,
    ) -> Result<Value> {
        let thread = self.store.real_thread(thread_id)?;
        self.store.set_muted(account_id, &thread, on)?;
        self.marks_changed().await?;
        Ok(json!({ "muted": on }))
    }

    pub(crate) async fn thread_snooze(
        &self,
        account_id: i64,
        thread_id: &str,
        until: Option<i64>,
    ) -> Result<Value> {
        if until.is_some_and(|u| u <= unix_now()) {
            return Err(CoreError::Invalid(
                "Pick a time that's still to come.".into(),
            ));
        }
        let thread = self.store.real_thread(thread_id)?;
        self.store.set_snooze(account_id, &thread, until)?;
        self.marks_changed().await?;
        Ok(json!({ "snooze_until": until }))
    }

    pub(crate) async fn thread_follow_up(
        &self,
        account_id: i64,
        thread_id: &str,
        at: Option<i64>,
    ) -> Result<Value> {
        if at.is_some_and(|u| u <= unix_now()) {
            return Err(CoreError::Invalid(
                "Pick a time that's still to come.".into(),
            ));
        }
        let thread = self.store.real_thread(thread_id)?;
        self.store.set_follow_up(account_id, &thread, at)?;
        self.marks_changed().await?;
        Ok(json!({ "follow_at": at }))
    }

    /// Sends this at a chosen time. It's kept as a draft until then, and goes when the
    /// app is open at or after that time.
    pub(crate) fn schedule_draft(&self, id: i64, send_at: i64) -> Result<Value> {
        if send_at <= unix_now() {
            return Err(CoreError::Invalid(
                "Pick a time that's still to come.".into(),
            ));
        }
        let draft = self.store.draft(id)?;
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            return Err(CoreError::Send {
                code: "no_recipients".into(),
                message: "Add at least one person to send this to.".into(),
            });
        }
        self.store
            .set_send_at(id, Some(send_at), draft.remind_days)?;
        Ok(json!({ "send_at": send_at }))
    }

    /// Everything that happens at a set time. The screens call this every so often.
    pub(crate) async fn due_tick(&self, quiet_now: bool, at: Option<i64>) -> Result<Value> {
        // Only a test may say what time it is. Anything else could send scheduled mail early.
        let now = at
            .filter(|_| cfg!(debug_assertions))
            .unwrap_or_else(unix_now);
        let mine = self.own_addresses()?;
        let back = self.store.bring_back_due(now, &mine)?;
        let mut changed = !back.is_empty();
        for r in &back {
            // A conversation that's back needs reading again.
            if r.why == "snooze" {
                let _ = self
                    .set_thread_seen(r.account_id, &r.thread_id, false)
                    .await;
            }
            self.tell_returned(r, quiet_now);
        }
        if !back.is_empty() {
            let _ = self.marks_changed().await;
        }
        let mut sent = 0;
        let mut failed = Vec::new();
        for id in self.store.drafts_due(now)? {
            // Not being able to reach the outgoing server means waiting and trying again.
            // Once it's being sent, a failure might have come after the mail went, so
            // it's never sent again by itself: that could send it twice.
            let Ok(draft) = self.store.draft(id) else {
                continue;
            };
            if let Err(e) = self.check_smtp(draft.account_id).await {
                let waiting = matches!(&e, CoreError::Network(_) | CoreError::Offline)
                    || matches!(&e, CoreError::Send { code, .. } if matches!(code.as_str(), "smtp_connect" | "smtp_busy"));
                if waiting {
                    continue;
                }
            }
            match self.send_draft(id, false).await {
                Ok(_) => {
                    sent += 1;
                    changed = true;
                }
                Err(CoreError::Send { code, .. }) if code == "already_sending" => {}
                Err(e) => {
                    let unsure = matches!(&e, CoreError::Send { code, .. } if matches!(code.as_str(), "smtp_connect" | "smtp_busy"))
                        || matches!(&e, CoreError::Network(_) | CoreError::Offline);
                    let text = if unsure {
                        format!("{e} It may or may not have gone, so check Sent before sending it again.")
                    } else {
                        e.to_string()
                    };
                    self.store.set_send_problem(id, &text)?;
                    self.tell_send_failed(id, &text, quiet_now);
                    failed.push(json!({ "id": id, "problem": text }));
                    changed = true;
                }
            }
        }
        // Follow-ups: stopped by a reply, otherwise sent when due.
        match self.sequences_tick(now).await {
            Ok(n) if n > 0 => {
                sent += n;
                changed = true;
            }
            _ => {}
        }
        // Quiet background work. A problem here just means it's tried again next time.
        let _ = self.calendar_sync(false).await;
        let _ = self.calendar_reminders(quiet_now);
        let _ = self.upload_drafts().await;
        let _ = self.clear_draft_leftovers().await;
        self.clear_sent_drafts();
        // Mail saved since the last look, by anything at all, means the lists are out of date.
        if let Ok(newest) = self.store.newest_saved_message() {
            let before = self
                .newest_told
                .swap(newest, std::sync::atomic::Ordering::Relaxed);
            if before != 0 && newest > before {
                changed = true;
            }
        }
        Ok(json!({ "changed": changed, "sent": sent, "failed": failed }))
    }

    fn tell_returned(&self, r: &Returned, quiet_now: bool) {
        let Some(sender) = &self.alert_sender else {
            return;
        };
        if quiet_now || self.setting("alerts").is_ok_and(|v| v == "off") {
            return;
        }
        let subject = self
            .store
            .summary_of(r.account_id, &r.thread_id)
            .ok()
            .flatten()
            .map(|t| t.subject)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "A conversation".into());
        let details = self.setting("alert_details").is_ok_and(|v| v == "on");
        sender(&Alert {
            subtitle: String::new(),
            title: if r.why == "snooze" {
                "Back from snooze".to_string()
            } else {
                "No reply yet".to_string()
            },
            body: if details {
                subject
            } else {
                "Open Mailzo to see it.".to_string()
            },
            sound: self.setting("alert_sound").is_ok_and(|v| v == "on"),
            thread: Some((r.account_id, r.thread_id.clone())),
        });
    }

    fn tell_send_failed(&self, id: i64, problem: &str, quiet_now: bool) {
        let Some(sender) = &self.alert_sender else {
            return;
        };
        if quiet_now || self.setting("alerts").is_ok_and(|v| v == "off") {
            return;
        }
        let subject = self.store.draft(id).map(|d| d.subject).unwrap_or_default();
        sender(&Alert {
            subtitle: String::new(),
            title: "A scheduled message didn't send".to_string(),
            body: format!("{subject}: {problem}"),
            sound: self.setting("alert_sound").is_ok_and(|v| v == "on"),
            thread: None,
        });
    }
}
