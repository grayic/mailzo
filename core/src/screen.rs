//! Deciding who reaches the inbox, grouping by sender, trackers and personal templates.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

pub(crate) const LET_IN: &str = "screen_ok:";
pub(crate) const TEMPLATE_PREFIX: &str = "template:";

/// Someone with mail in an inbox, as one row.
#[derive(Debug, Clone, Serialize)]
pub struct SenderRow {
    pub address: String,
    pub name: String,
    pub threads: i64,
    pub unread: i64,
    pub last_date: i64,
    pub subject: String,
    pub account_id: i64,
    pub folder_id: i64,
}

impl Store {
    /// Whether any earlier mail from this address got through. The same email's copy under
    /// another label doesn't count, and nor does mail nobody has looked at yet.
    pub fn sender_is_known(&self, account_id: i64, address: &str, not_this: &str) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM messages m INDEXED BY messages_from_addr
                 LEFT JOIN verdicts v ON v.message_key = m.message_key
                 WHERE m.from_addr = ?2 COLLATE NOCASE AND m.account_id = ?1 AND m.screened = 0
                   AND m.message_key != ?3
                   AND (m.old_mail != 0 OR v.message_key IS NOT NULL) LIMIT 1",
                params![account_id, address, not_this],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn set_screened(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE messages SET screened = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear_screened_for(&self, address: &str) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET screened = 0 WHERE from_addr = ?1 COLLATE NOCASE AND screened = 1",
            [address],
        )?;
        Ok(())
    }

    pub fn screened_mail_from(&self, address: &str) -> Result<Vec<(i64, i64, i64)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT account_id, id, folder_id FROM messages
             WHERE from_addr = ?1 COLLATE NOCASE AND screened = 1",
        )?;
        let rows = st.query_map([address], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn clear_all_screened(&self) -> Result<()> {
        self.db()
            .execute("UPDATE messages SET screened = 0 WHERE screened = 1", [])?;
        Ok(())
    }

    pub fn is_screened(&self, account_id: i64, rfc_id: &str) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM messages WHERE rfc_id = ?1 AND account_id = ?2 AND screened = 1 LIMIT 1",
                params![rfc_id, account_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// New senders waiting for a decision, most recent first.
    pub fn screener_rows(&self) -> Result<Vec<SenderRow>> {
        self.sender_rows("m.screened = 1 AND f.role = 'inbox'", &[])
    }

    /// Everyone with mail in one inbox folder or tab.
    pub fn senders_in(&self, folder_id: i64) -> Result<Vec<SenderRow>> {
        self.sender_rows(
            "m.folder_id = ?1 AND m.screened = 0
             AND m.thread_id NOT IN (SELECT thread_id FROM thread_marks
                 WHERE account_id = m.account_id AND (muted = 1 OR snooze_until IS NOT NULL))",
            &[&folder_id],
        )
    }

    fn sender_rows(
        &self,
        condition: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<SenderRow>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT lower(m.from_addr), MAX(m.from_name), COUNT(DISTINCT m.thread_id),
                    COUNT(DISTINCT CASE WHEN m.seen = 0 THEN m.thread_id END), MAX(m.date), m.subject,
                    m.account_id, m.folder_id
             FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE {condition}
             GROUP BY m.account_id, lower(m.from_addr)
             ORDER BY MAX(m.date) DESC LIMIT 500"
        ))?;
        let rows = st.query_map(args, |r| {
            Ok(SenderRow {
                address: r.get(0)?,
                name: r.get(1)?,
                threads: r.get(2)?,
                unread: r.get(3)?,
                last_date: r.get(4)?,
                subject: r.get(5)?,
                account_id: r.get(6)?,
                folder_id: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Conversations from one sender in one folder, newest first.
    pub fn threads_from(
        &self,
        folder_id: i64,
        address: &str,
        screened: bool,
    ) -> Result<Vec<ThreadSummary>> {
        let ids: Vec<(i64, String)> = {
            let db = self.db();
            let mut st = db.prepare(
                "SELECT account_id, thread_id FROM messages INDEXED BY messages_from_addr
                 WHERE from_addr = ?2 COLLATE NOCASE AND folder_id = ?1 AND screened = ?3
                 GROUP BY thread_id ORDER BY MAX(date) DESC LIMIT 500",
            )?;
            let rows = st.query_map(params![folder_id, address, screened], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for (account, thread) in ids {
            if let Some(row) = self.summary_of(account, &thread)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Adds up which companies have tried to track this person, once per email.
    pub fn note_trackers(&self, message_id: i64, companies: &[String]) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let key: Option<String> = tx
            .query_row(
                "SELECT COALESCE(rfc_id, '#' || id) FROM messages WHERE id = ?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(key) = key else { return Ok(()) };
        let added = tx.execute(
            "INSERT OR IGNORE INTO tracked_mail (message_key) VALUES (?1)",
            [&key],
        )?;
        if added == 1 {
            for company in companies {
                tx.execute(
                    "INSERT INTO tracker_counts (company, seen) VALUES (?1, 1)
                     ON CONFLICT (company) DO UPDATE SET seen = seen + 1",
                    [company],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn tracker_counts(&self) -> Result<Vec<(String, i64)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT company, seen FROM tracker_counts ORDER BY seen DESC, company LIMIT 50",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

impl Engine {
    /// Whether this new email waits in the screener. Runs once per email.
    pub(crate) fn should_screen(
        &self,
        account: &Account,
        message: &crate::store::MailToCheck,
        address: &str,
        known: bool,
    ) -> Result<bool> {
        if account.prefs.team_shared || known || address.is_empty() {
            return Ok(false);
        }
        if self.setting("screener")? != "on" {
            return Ok(false);
        }
        if self.store.setting(&format!("{LET_IN}{address}"))?.is_some() {
            return Ok(false);
        }
        Ok(!self
            .store
            .sender_is_known(account.id, address, &message.key)?)
    }

    /// Let them in, or block them. Nothing is deleted either way.
    pub(crate) async fn screener_decide(&self, address: &str, allow: bool) -> Result<Value> {
        let address = address.trim().to_lowercase();
        if address.is_empty() {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        if allow {
            self.store
                .set_setting(&format!("{LET_IN}{address}"), "on")?;
            self.store.clear_screened_for(&address)?;
            self.settings_changed().await?;
            return Ok(json!({ "allowed": true }));
        }
        // Each of their emails on its own: blocking someone who joined a conversation
        // mustn't send the rest of it to Junk, or block the others in it.
        let threads: Vec<crate::actions::BulkThread> = self
            .store
            .screened_mail_from(&address)?
            .into_iter()
            .map(|(account_id, id, folder_id)| crate::actions::BulkThread {
                account_id,
                thread_id: format!("msg:{id}"),
                folder_id,
            })
            .collect();
        // Stops them counting as waiting, wherever else this is decided.
        self.store.clear_screened_for(&address)?;
        if threads.is_empty() {
            self.store
                .set_setting(&format!("{}{address}", crate::spam::BLOCKED), "on")?;
            self.settings_changed().await?;
        } else {
            self.spam_mark(threads, true).await?;
        }
        Ok(json!({ "allowed": false }))
    }

    pub(crate) fn my_templates(&self) -> Result<Value> {
        let mut list: Vec<Value> = self
            .store
            .all_settings()?
            .into_iter()
            .filter(|(k, _)| k.starts_with(TEMPLATE_PREFIX))
            .filter_map(|(k, v)| {
                let mut one: Value = serde_json::from_str(&v).ok()?;
                one["id"] = json!(k.trim_start_matches(TEMPLATE_PREFIX));
                Some(one)
            })
            .collect();
        list.sort_by_key(|t| t["name"].as_str().unwrap_or("").to_lowercase());
        Ok(json!(list))
    }

    pub(crate) async fn save_my_template(
        &self,
        id: Option<&str>,
        name: &str,
        body: &str,
    ) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::Invalid("Give it a name first.".into()));
        }
        if body.trim().is_empty() {
            return Err(CoreError::Invalid("Write what it should say first.".into()));
        }
        let id = id
            .map(str::to_string)
            .unwrap_or_else(|| crate::crypto::random_hex(8));
        self.store.set_setting(
            &format!("{TEMPLATE_PREFIX}{id}"),
            &json!({ "name": name, "body": body }).to_string(),
        )?;
        self.settings_changed().await?;
        Ok(json!({ "id": id }))
    }

    pub(crate) async fn delete_my_template(&self, id: &str) -> Result<Value> {
        self.store
            .delete_setting(&format!("{TEMPLATE_PREFIX}{id}"))?;
        self.settings_changed().await?;
        Ok(json!({ "deleted": true }))
    }
}
