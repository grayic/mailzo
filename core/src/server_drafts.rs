//! Keeps drafts in the mail server's Drafts folder as well, so they survive this device
//! and show up in other mail apps and on other devices. Quietly, and a little after the
//! typing stops: a problem here never gets in the way of writing.

use crate::compose::{parse_people_draft, DraftFromMail};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::unix_now;
use crate::imap::ImapConnection;
use crate::model::*;
use crate::parse;
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::time::SystemTime;

/// How long typing has to have stopped before a copy goes to the server.
const IDLE_SECONDS: i64 = 20;

/// Which draft, its inbox, the copy already on the server, and which version this is.
type DraftToUpload = (i64, i64, Option<String>, i64);

impl Store {
    /// Drafts newer than their copy on the server.
    pub fn drafts_to_upload(&self, idle_since: i64) -> Result<Vec<DraftToUpload>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, account_id, server_rfc_id, updated_at FROM drafts
             WHERE updated_at > server_saved_at AND updated_at <= ?1 AND sent_at IS NULL",
        )?;
        let rows = st.query_map([idle_since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn draft_with_server_copy(&self, account_id: i64, rfc_id: &str) -> Result<Option<i64>> {
        Ok(self
            .db()
            .query_row(
                "SELECT id FROM drafts WHERE account_id = ?1 AND server_rfc_id = ?2",
                params![account_id, rfc_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn draft_server_rfc(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "SELECT server_rfc_id FROM drafts WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn forget_server_copy(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE drafts SET server_rfc_id = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn all_server_copies(&self) -> Result<Vec<(i64, i64, String)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, account_id, server_rfc_id FROM drafts WHERE server_rfc_id IS NOT NULL",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_made_by_ai(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE drafts SET made_by_ai = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Remembers which version went up, so anything typed during the upload still follows.
    /// False if the draft has since gone.
    pub fn note_draft_uploaded(&self, id: i64, rfc_id: Option<&str>, version: i64) -> Result<bool> {
        Ok(self.db().execute(
            "UPDATE drafts SET server_saved_at = ?3, server_rfc_id = COALESCE(?2, server_rfc_id) WHERE id = ?1",
            params![id, rfc_id, version],
        )? > 0)
    }

    /// Server copies of drafts that have since been sent, discarded or replaced.
    pub fn add_draft_leftover(&self, account_id: i64, rfc_id: &str) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO draft_leftovers (account_id, rfc_id) VALUES (?1, ?2)",
            params![account_id, rfc_id],
        )?;
        Ok(())
    }

    pub fn draft_leftovers(&self) -> Result<Vec<(i64, String)>> {
        let db = self.db();
        let mut st = db.prepare("SELECT account_id, rfc_id FROM draft_leftovers")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_draft_leftover(&self, account_id: i64, rfc_id: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM draft_leftovers WHERE account_id = ?1 AND rfc_id = ?2",
            params![account_id, rfc_id],
        )?;
        Ok(())
    }

    /// The saved copy of a server draft, removed once it's replaced there.
    pub fn delete_messages_with_rfc(
        &self,
        account_id: i64,
        folder_id: i64,
        rfc_id: &str,
    ) -> Result<()> {
        self.db().execute(
            "DELETE FROM messages WHERE account_id = ?1 AND folder_id = ?2 AND rfc_id = ?3",
            params![account_id, folder_id, rfc_id],
        )?;
        Ok(())
    }
}

fn worth_saving(d: &Draft) -> bool {
    // A reply nobody has started writing isn't worth keeping anywhere.
    !d.text.trim().is_empty()
        || !d.attachments.is_empty()
        || (d.mode == "new" && (!d.subject.trim().is_empty() || !d.to.is_empty()))
}

impl Engine {
    pub(crate) async fn upload_drafts(&self) -> Result<usize> {
        if self.setting("drafts_on_server")? != "on" {
            return Ok(0);
        }
        let mut done = 0;
        for (id, account_id, old_rfc, version) in
            self.store.drafts_to_upload(unix_now() - IDLE_SECONDS)?
        {
            let Ok(draft) = self.store.draft(id) else {
                continue;
            };
            // Everyone on a team can see a shared inbox's Drafts folder.
            let shared = self.store.account(account_id)?.prefs.team_shared;
            // What's meant to go encrypted isn't left readable on the server meanwhile.
            // Nor is a reply that quotes encrypted mail, even if encryption was turned off.
            if shared || draft.encrypt || draft.from_encrypted || !worth_saving(&draft) {
                // Cleared back to nothing: the old words shouldn't stay on the server.
                if let Some(old) = &old_rfc {
                    self.store.add_draft_leftover(account_id, old)?;
                    self.store.forget_server_copy(id)?;
                }
                self.store.note_draft_uploaded(id, None, version)?;
                continue;
            }
            match self.upload_one(&draft).await {
                Ok(new_rfc) => {
                    let still_here =
                        self.store
                            .note_draft_uploaded(id, new_rfc.as_deref(), version)?;
                    if let (Some(old), Some(_)) = (&old_rfc, &new_rfc) {
                        self.store.add_draft_leftover(account_id, old)?;
                    }
                    // Sent or thrown away while this was on its way, so this copy has to go too.
                    if let (false, Some(new)) = (still_here, &new_rfc) {
                        self.store.add_draft_leftover(account_id, new)?;
                    }
                    done += 1;
                }
                // Can't reach it now: this comes round again by itself.
                Err(CoreError::Network(_))
                | Err(CoreError::Offline)
                | Err(CoreError::Auth)
                | Err(CoreError::NeedsSignIn(_)) => {}
                // It can't be saved as it stands, such as an attachment that's too big.
                // The local draft is fine, so this waits for the next change.
                Err(_) => {
                    self.store.note_draft_uploaded(id, None, version)?;
                }
            }
        }
        Ok(done)
    }

    /// The setting's been turned off, so nothing should stay up there.
    pub(crate) fn take_drafts_off_server(&self) -> Result<()> {
        for (id, account_id, rfc) in self.store.all_server_copies()? {
            self.store.add_draft_leftover(account_id, &rfc)?;
            self.store.forget_server_copy(id)?;
        }
        Ok(())
    }

    async fn upload_one(&self, draft: &Draft) -> Result<Option<String>> {
        let account = self.store.account(draft.account_id)?;
        let Some(folder) = self
            .store
            .list_folders(account.id)?
            .into_iter()
            .find(|f| f.role == "drafts")
        else {
            return Ok(None);
        };
        let domain = account.email.rsplit('@').next().unwrap_or("localhost");
        let rfc_id = format!("draft-{}@{domain}", crate::crypto::random_hex(16));
        // A message has to name someone, and this one may not be finished. Half-typed addresses
        // are left out, and with nobody at all it's addressed to this inbox until reopened.
        let mut copy = draft.clone();
        let valid = |a: &Addr| a.address.trim().parse::<lettre::Address>().is_ok();
        copy.to.retain(valid);
        copy.cc.retain(valid);
        copy.bcc.retain(valid);
        if copy.to.is_empty() && copy.cc.is_empty() && copy.bcc.is_empty() {
            copy.to = vec![Addr {
                name: String::new(),
                address: account.email.clone(),
            }];
        }
        let message = self.build_message(
            &copy,
            &account,
            &format!("<{rfc_id}>"),
            SystemTime::now(),
            true,
        )?;
        let password = self.password(account.id).await?;
        let mut conn = ImapConnection::connect(&account, &password, self.reach()).await?;
        conn.append_draft(&folder.path, &message.formatted())
            .await?;
        conn.logout().await;
        Ok(Some(rfc_id))
    }

    /// Removes server copies that are no longer wanted.
    pub(crate) async fn clear_draft_leftovers(&self) -> Result<()> {
        for (account_id, rfc_id) in self.store.draft_leftovers()? {
            // One inbox being unreachable doesn't hold up the others.
            let _ = self.clear_one_leftover(account_id, &rfc_id).await;
        }
        Ok(())
    }

    async fn clear_one_leftover(&self, account_id: i64, rfc_id: &str) -> Result<()> {
        let Ok(account) = self.store.account(account_id) else {
            return self.store.remove_draft_leftover(account_id, rfc_id);
        };
        let Some(folder) = self
            .store
            .list_folders(account_id)?
            .into_iter()
            .find(|f| f.role == "drafts")
        else {
            return self.store.remove_draft_leftover(account_id, rfc_id);
        };
        let password = self.password(account_id).await?;
        let mut conn = ImapConnection::connect(&account, &password, self.reach()).await?;
        conn.select_folder(&folder.path).await?;
        let uids = conn
            .search_uids(&format!("HEADER Message-ID \"<{rfc_id}>\""))
            .await?;
        if !uids.is_empty() {
            conn.delete_uids(&uids).await?;
        }
        conn.logout().await;
        self.store
            .delete_messages_with_rfc(account_id, folder.id, rfc_id)?;
        self.store.remove_draft_leftover(account_id, rfc_id)
    }

    /// Opens a draft that's only on the server, such as one started on another device,
    /// so it can be finished here.
    pub(crate) async fn draft_from_message(&self, message_id: i64) -> Result<Value> {
        let detail = self.store.message_detail(message_id)?;
        let account = self.store.account(detail.account_id)?;
        // Already opened here, so it isn't made into a second draft.
        if let Some(rfc) = &detail.rfc_id {
            if let Some(id) = self.store.draft_with_server_copy(account.id, rfc)? {
                return Ok(json!(self.store.draft(id)?));
            }
        }
        let raw = self.raw_message(message_id).await?;
        let body = parse::parse_body(&raw);
        let headers = parse::parse_headers(&raw);
        // The conversation it's a reply to, if that's here.
        let parent = headers
            .in_reply_to
            .first()
            .map(|id| self.store.message_with_rfc(account.id, id))
            .transpose()?
            .flatten();
        let mut made = parse_people_draft(DraftFromMail {
            account_id: account.id,
            parent,
            detail: &detail,
            body: &body,
        });
        // Still comes from the address it was being written from.
        if account
            .prefs
            .aliases
            .iter()
            .any(|a| a.address.eq_ignore_ascii_case(&detail.from.address))
        {
            made.from_address = detail.from.address.clone();
        }
        // Addressed to this inbox only because it had to be addressed to someone.
        if detail
            .rfc_id
            .as_deref()
            .is_some_and(|r| r.starts_with("draft-"))
            && made.cc.is_empty()
            && made.bcc.is_empty()
            && made.to.len() == 1
            && made.to[0].address.eq_ignore_ascii_case(&account.email)
        {
            made.to.clear();
        }
        let id = self.store.insert_draft(&made)?;
        if let Some(rfc) = &detail.rfc_id {
            self.store
                .note_draft_uploaded(id, Some(rfc), crate::identity::unix_now())?;
        }
        let mut draft = self.store.draft(id)?;
        for (i, a) in body
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| !a.inline)
        {
            if let Some((name, mime, bytes)) =
                parse::attachment_part(&raw, a.part.unwrap_or(i as u32))
            {
                draft
                    .attachments
                    .push(self.write_draft_file(id, &name, &mime, &bytes)?);
            }
        }
        self.store.update_attachments(&draft)?;
        Ok(json!(self.store.draft(id)?))
    }
}
