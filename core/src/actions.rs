//! Changing mail: read state, stars, moving, deleting, saving attachments.
//!
//! The change shows here straight away and then goes to the server. If the server
//! refuses, the next sync brings this device back in line.

use crate::compose::safe_file_name;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::imap::ImapConnection;
use crate::model::*;
use crate::parse;
use crate::secrets::account_secret;
use crate::store::ThreadCopy;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The most of an email's original that's shown on screen.
const SOURCE_SHOWN: usize = 1_000_000;

/// The only kinds of file opened straight from the app: documents, pictures, sound and
/// video that open in a viewer and can't run anything by themselves. Anything else can
/// only be shown in Finder, where the Mac's own checks apply when it's opened.
const SAFE_TO_OPEN: &[&str] = &[
    // Documents
    // Not the old Office kinds (doc, xls, ppt), which can carry macros.
    "pdf", "txt", "text", "md", "rtf", "csv", "tsv", "log", "docx", "xlsx", "pptx", "odt", "ods",
    "odp", "pages", "numbers", "key", "epub", // Pictures
    "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "tif", "tiff", "bmp", "avif",
    // Sound and video
    "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg", "mp4", "m4v", "mov", "avi", "webm",
];

fn by_folder(copies: Vec<&ThreadCopy>) -> BTreeMap<String, (Option<u32>, Vec<u32>)> {
    let mut map: BTreeMap<String, (Option<u32>, Vec<u32>)> = BTreeMap::new();
    for c in copies {
        map.entry(c.folder_path.clone())
            .or_insert((c.uid_validity, vec![]))
            .1
            .push(c.uid);
    }
    map
}

fn renumbered() -> CoreError {
    CoreError::Invalid("This folder changed on the server. Refresh and try again.".into())
}

impl Engine {
    async fn connect_account(&self, account_id: i64) -> Result<ImapConnection> {
        let account = self.store.account(account_id)?;
        let password = self.password(account_id).await?;
        let connected = ImapConnection::connect(&account, &password, self.reach()).await;
        if matches!(connected, Err(CoreError::NeedsSignIn(_))) {
            self.forget_token(account_id).await;
        }
        connected
    }

    /// Marks a whole conversation read or unread, in every folder it is in.
    pub(crate) async fn set_thread_seen(
        &self,
        account_id: i64,
        thread_id: &str,
        seen: bool,
    ) -> Result<Value> {
        if seen {
            let thread = self.store.real_thread(thread_id)?;
            self.store.clear_returned(account_id, &thread)?;
        }
        let copies = self.store.thread_copies(account_id, thread_id)?;
        let changing: Vec<&ThreadCopy> = copies.iter().filter(|c| c.seen != seen).collect();
        if changing.is_empty() {
            return Ok(json!({ "changed": 0 }));
        }
        let count = changing.len();
        // In a shared inbox, reading something doesn't read it for everyone.
        if self.store.account(account_id)?.prefs.team_shared {
            for c in &changing {
                self.store.set_message_seen(c.id, seen)?;
            }
            return Ok(json!({ "changed": count }));
        }
        self.store.set_seen_held(&changing, seen)?;
        let query = if seen {
            "+FLAGS.SILENT (\\Seen)"
        } else {
            "-FLAGS.SILENT (\\Seen)"
        };
        self.store_on_server(account_id, changing, query).await?;
        Ok(json!({ "changed": count }))
    }

    /// Stars the newest message, or removes the star from all of them.
    pub(crate) async fn set_thread_flagged(
        &self,
        account_id: i64,
        thread_id: &str,
        flagged: bool,
    ) -> Result<Value> {
        let copies = self.store.thread_copies(account_id, thread_id)?;
        let changing: Vec<&ThreadCopy> = if flagged {
            // Not a draft sitting at the end of the conversation.
            copies
                .iter()
                .rev()
                .filter(|c| c.folder_role != "drafts" && !c.is_draft)
                .take(1)
                .filter(|c| !c.flagged)
                .collect()
        } else {
            copies.iter().filter(|c| c.flagged).collect()
        };
        for c in &changing {
            self.store.set_message_flagged(c.id, flagged)?;
        }
        let query = if flagged {
            "+FLAGS.SILENT (\\Flagged)"
        } else {
            "-FLAGS.SILENT (\\Flagged)"
        };
        let count = changing.len();
        if count > 0 {
            self.store_on_server(account_id, changing, query).await?;
        }
        Ok(json!({ "changed": count }))
    }

    async fn store_on_server(
        &self,
        account_id: i64,
        copies: Vec<&ThreadCopy>,
        query: &str,
    ) -> Result<()> {
        let read_marks: Vec<(i64, u32)> = if query.contains("\\Seen") {
            copies.iter().map(|c| (c.folder_id, c.uid)).collect()
        } else {
            Vec::new()
        };
        let pushed = async {
            let mut conn = self.connect_account(account_id).await?;
            for (path, (validity, uids)) in by_folder(copies) {
                let mailbox = conn.select_folder(&path).await?;
                if mailbox.uid_validity != validity {
                    return Err(renumbered());
                }
                conn.store_flags(&uids, query).await?;
            }
            conn.logout().await;
            Ok(())
        }
        .await;
        // The change didn't get there, and the server's counter won't show one. The next
        // refresh compares every message, which brings this device back in line.
        if pushed.is_err() {
            self.store.forget_change_counters(account_id)?;
        }
        self.store.seen_heard(&read_marks, pushed.is_ok());
        pushed
    }

    /// Moves a conversation's messages out of one folder. `to` is `archive`,
    /// `trash`, `junk`, `inbox`, or a folder's path.
    pub(crate) async fn move_thread(
        &self,
        account_id: i64,
        thread_id: &str,
        from_folder_id: i64,
        to: &str,
    ) -> Result<Value> {
        // Acting on it means it's been dealt with, wherever its copies are.
        let thread = self.store.real_thread(thread_id)?;
        self.store.clear_returned(account_id, &thread)?;
        let copies = self.store.thread_copies(account_id, thread_id)?;
        let here: Vec<&ThreadCopy> = copies
            .iter()
            .filter(|c| c.folder_id == from_folder_id)
            .collect();
        let Some(first) = here.first() else {
            return Ok(json!({ "moved": 0 }));
        };
        let from_path = first.folder_path.clone();
        let validity = first.uid_validity;
        let uids: Vec<u32> = here.iter().map(|c| c.uid).collect();

        let mut conn = self.connect_account(account_id).await?;
        let account = self.store.account(account_id)?;
        let target = self.resolve_target(&mut conn, &account, to).await?;
        if target == from_path {
            conn.logout().await;
            return Ok(json!({ "moved": 0 }));
        }
        let mailbox = conn.select_folder(&from_path).await?;
        if mailbox.uid_validity != validity {
            return Err(renumbered());
        }
        conn.move_uids(&uids, &target).await?;
        // If this doesn't work, the next refresh brings it in.
        let _ = crate::sync::pull_folder(&mut conn, &self.store, account_id, &target).await;
        conn.logout().await;
        for c in &here {
            self.store.delete_message(c.id)?;
        }
        Ok(json!({ "moved": here.len() }))
    }

    /// Only a folder the person made. Any mail in it goes to Archive first, and if that doesn't
    /// work nothing is deleted.
    pub(crate) async fn delete_folder(&self, account_id: i64, folder_id: i64) -> Result<Value> {
        let account = self.store.account(account_id)?;
        if account.prefs.team_shared {
            return Err(CoreError::Invalid(
                "A shared inbox's folders belong to the whole team.".into(),
            ));
        }
        let folders = self.store.list_folders(account_id)?;
        let folder = folders
            .iter()
            .find(|f| f.id == folder_id)
            .ok_or_else(|| CoreError::NotFound("That folder no longer exists.".into()))?;
        if folder.role != "folder" {
            return Err(CoreError::Invalid(
                "Only folders you made can be deleted.".into(),
            ));
        }
        let holds_others = folders.iter().any(|f| {
            f.path.len() > folder.path.len()
                && f.path.starts_with(&folder.path)
                && matches!(f.path.as_bytes()[folder.path.len()], b'.' | b'/')
        });
        if holds_others {
            return Err(CoreError::Invalid(
                "This folder has folders inside it. Delete those first.".into(),
            ));
        }
        let path = folder.path.clone();
        let mut conn = self.connect_account(account_id).await?;
        conn.select_folder(&path).await?;
        let uids = conn.search_uids("ALL").await?;
        let moved = uids.len();
        if !uids.is_empty() {
            let target = self.resolve_target(&mut conn, &account, "archive").await?;
            conn.select_folder(&path).await?;
            conn.move_uids(&uids, &target).await?;
            conn.select_folder(&path).await?;
            if !conn.search_uids("ALL").await?.is_empty() {
                conn.logout().await;
                return Err(CoreError::Network(
                    "Some emails couldn't be moved out, so the folder was kept.".into(),
                ));
            }
        }
        conn.delete_folder(&path).await?;
        let list: Vec<(String, String, String)> =
            crate::sync::with_chosen_roles(conn.folders().await?, &account)
                .into_iter()
                .map(|f| (f.path, f.name, f.role))
                .collect();
        self.store.replace_folders(account_id, &list)?;
        conn.logout().await;
        Ok(json!({ "moved": moved }))
    }

    async fn resolve_target(
        &self,
        conn: &mut ImapConnection,
        account: &Account,
        to: &str,
    ) -> Result<String> {
        let folders = crate::sync::with_chosen_roles(conn.folders().await?, account);
        let by_role = |role: &str| {
            folders
                .iter()
                .find(|f| f.role == role)
                .map(|f| f.path.clone())
        };
        Ok(match to {
            "inbox" => "INBOX".to_string(),
            "trash" => match by_role("trash") {
                Some(path) => path,
                None => conn.ensure_folder("Trash").await?,
            },
            "archive" => match by_role("archive") {
                Some(path) => path,
                // Gmail has no Archive folder. Its "All Mail" does the same job.
                None => match conn.all_mail_folder().await? {
                    Some(path) => path,
                    None => conn.ensure_folder("Archive").await?,
                },
            },
            "junk" => match by_role("junk") {
                Some(path) => path,
                None => conn.ensure_folder("Junk").await?,
            },
            path => {
                if !folders.iter().any(|f| f.path == path) {
                    return Err(CoreError::NotFound("That folder no longer exists.".into()));
                }
                path.to_string()
            }
        })
    }

    /// One action on many conversations, possibly across inboxes. Each inbox gets a
    /// single connection, so a long selection doesn't sign in over and over. A problem
    /// with one inbox doesn't stop the others.
    pub(crate) async fn bulk_threads(
        &self,
        threads: Vec<BulkThread>,
        action: &str,
        to: Option<&str>,
    ) -> Result<Value> {
        let mut by_account: BTreeMap<i64, Vec<BulkThread>> = BTreeMap::new();
        for t in threads {
            by_account.entry(t.account_id).or_default().push(t);
        }
        if matches!(action, "seen" | "unseen") {
            return self
                .bulk_seen(by_account, action == "seen", true, true)
                .await;
        }
        let mut done = 0usize;
        let mut first_problem: Option<CoreError> = None;
        for (account_id, list) in by_account {
            let count = list.len();
            match self.bulk_for_account(account_id, list, action, to).await {
                Ok(()) => done += count,
                Err(e) => {
                    first_problem.get_or_insert(e);
                }
            }
        }
        match first_problem {
            Some(e) if done == 0 => Err(e),
            Some(e) => Ok(json!({ "done": done, "problem": e.to_string() })),
            None => Ok(json!({ "done": done })),
        }
    }

    /// Read or unread in two steps, so every window shows it before any server has heard:
    /// `here` changes this Mac, then `servers` tells them.
    pub(crate) async fn bulk_seen_step(
        &self,
        threads: Vec<BulkThread>,
        action: &str,
        step: &str,
    ) -> Result<Value> {
        if !matches!(action, "seen" | "unseen") || !matches!(step, "here" | "servers") {
            return Err(CoreError::Invalid(
                "Only read and unread are done in steps.".into(),
            ));
        }
        let mut by_account: BTreeMap<i64, Vec<BulkThread>> = BTreeMap::new();
        for t in threads {
            by_account.entry(t.account_id).or_default().push(t);
        }
        self.bulk_seen(
            by_account,
            action == "seen",
            step == "here",
            step == "servers",
        )
        .await
    }

    /// Read or unread, for many conversations across inboxes. Every inbox changes on this
    /// Mac first, so the app shows it at once, and then the servers are told all at the same
    /// time: ten inboxes take as long as the slowest, not as long as all of them in a row.
    /// Until they've heard, a refresh doesn't put the old marks back.
    pub(crate) async fn bulk_seen(
        &self,
        by_account: BTreeMap<i64, Vec<BulkThread>>,
        seen: bool,
        here: bool,
        servers: bool,
    ) -> Result<Value> {
        let mut done = 0usize;
        let mut pushes: Vec<(i64, Vec<ThreadCopy>)> = Vec::new();
        for (account_id, list) in by_account {
            let shared = self.store.account(account_id)?.prefs.team_shared;
            let mut changing: Vec<ThreadCopy> = Vec::new();
            for t in &list {
                if seen && here {
                    let thread = self.store.real_thread(&t.thread_id)?;
                    self.store.clear_returned(account_id, &thread)?;
                }
                // Told after the change here, the servers get what's now marked that way.
                changing.extend(
                    self.store
                        .thread_copies(account_id, &t.thread_id)?
                        .into_iter()
                        .filter(|c| (c.seen != seen) == here),
                );
            }
            if here && shared {
                for c in &changing {
                    self.store.set_message_seen(c.id, seen)?;
                }
            } else if here {
                self.store
                    .set_seen_held(&changing.iter().collect::<Vec<_>>(), seen)?;
            }
            done += list.len();
            // In a shared inbox, reading something doesn't read it for everyone.
            if servers && !changing.is_empty() && !shared {
                pushes.push((account_id, changing));
            }
        }
        let query = if seen {
            "+FLAGS.SILENT (\\Seen)"
        } else {
            "-FLAGS.SILENT (\\Seen)"
        };
        let told = futures_util::future::join_all(pushes.iter().map(|(account_id, copies)| {
            self.store_on_server(*account_id, copies.iter().collect(), query)
        }))
        .await;
        // Changed here either way; a server that didn't hear is put right on its next refresh.
        match told.into_iter().find_map(|r| r.err()) {
            Some(e) => Ok(json!({ "done": done, "problem": e.to_string() })),
            None => Ok(json!({ "done": done })),
        }
    }

    async fn bulk_for_account(
        &self,
        account_id: i64,
        threads: Vec<BulkThread>,
        action: &str,
        to: Option<&str>,
    ) -> Result<()> {
        let mut all: Vec<(BulkThread, Vec<ThreadCopy>)> = Vec::new();
        for t in threads {
            if matches!(action, "seen" | "move" | "delete_forever") {
                let thread = self.store.real_thread(&t.thread_id)?;
                self.store.clear_returned(account_id, &thread)?;
            }
            let copies = self.store.thread_copies(account_id, &t.thread_id)?;
            all.push((t, copies));
        }
        match action {
            "seen" | "unseen" => {
                let seen = action == "seen";
                let changing: Vec<&ThreadCopy> = all
                    .iter()
                    .flat_map(|(_, copies)| copies.iter())
                    .filter(|c| c.seen != seen)
                    .collect();
                if changing.is_empty() {
                    return Ok(());
                }
                for c in &changing {
                    self.store.set_message_seen(c.id, seen)?;
                }
                if self.store.account(account_id)?.prefs.team_shared {
                    return Ok(());
                }
                let query = if seen {
                    "+FLAGS.SILENT (\\Seen)"
                } else {
                    "-FLAGS.SILENT (\\Seen)"
                };
                self.store_on_server(account_id, changing, query).await
            }
            "star" | "unstar" => {
                let on = action == "star";
                let changing: Vec<&ThreadCopy> = all
                    .iter()
                    .flat_map(|(_, copies)| -> Vec<&ThreadCopy> {
                        if on {
                            copies
                                .iter()
                                .rev()
                                .filter(|c| c.folder_role != "drafts" && !c.is_draft)
                                .take(1)
                                .filter(|c| !c.flagged)
                                .collect()
                        } else {
                            copies.iter().filter(|c| c.flagged).collect()
                        }
                    })
                    .collect();
                if changing.is_empty() {
                    return Ok(());
                }
                for c in &changing {
                    self.store.set_message_flagged(c.id, on)?;
                }
                let query = if on {
                    "+FLAGS.SILENT (\\Flagged)"
                } else {
                    "-FLAGS.SILENT (\\Flagged)"
                };
                self.store_on_server(account_id, changing, query).await
            }
            "move" | "delete_forever" => {
                let here: Vec<&ThreadCopy> = all
                    .iter()
                    .flat_map(|(t, copies)| {
                        copies.iter().filter(move |c| c.folder_id == t.folder_id)
                    })
                    .collect();
                if here.is_empty() {
                    return Ok(());
                }
                if action == "delete_forever"
                    && here
                        .iter()
                        .any(|c| c.folder_role != "trash" && c.folder_role != "junk")
                {
                    return Err(CoreError::Invalid(
                        "Move these to the bin first. Only mail in the bin or spam can be deleted for good."
                            .into(),
                    ));
                }
                let mut conn = self.connect_account(account_id).await?;
                let target = match (action, to) {
                    ("move", Some(to)) => {
                        let account = self.store.account(account_id)?;
                        Some(self.resolve_target(&mut conn, &account, to).await?)
                    }
                    ("move", None) => return Err(CoreError::Invalid("Bad request.".into())),
                    _ => None,
                };
                for (path, (validity, uids)) in by_folder(here.clone()) {
                    if target.as_deref() == Some(path.as_str()) {
                        continue;
                    }
                    let mailbox = conn.select_folder(&path).await?;
                    if mailbox.uid_validity != validity {
                        return Err(renumbered());
                    }
                    match &target {
                        Some(target) => {
                            conn.move_uids(&uids, target).await?;
                            // If this doesn't work, the next refresh brings it in.
                            let _ = crate::sync::pull_folder(
                                &mut conn,
                                &self.store,
                                account_id,
                                target,
                            )
                            .await;
                        }
                        None => conn.delete_uids(&uids).await?,
                    }
                    for c in here.iter().filter(|c| c.folder_path == path) {
                        self.store.delete_message(c.id)?;
                    }
                }
                conn.logout().await;
                Ok(())
            }
            _ => Err(CoreError::Invalid("Bad request.".into())),
        }
    }

    /// Only from the bin or spam, so nothing is lost by a single click elsewhere.
    pub(crate) async fn delete_forever(
        &self,
        account_id: i64,
        thread_id: &str,
        folder_id: i64,
    ) -> Result<Value> {
        let copies = self.store.thread_copies(account_id, thread_id)?;
        let here: Vec<&ThreadCopy> = copies.iter().filter(|c| c.folder_id == folder_id).collect();
        let Some(first) = here.first() else {
            return Ok(json!({ "deleted": 0 }));
        };
        if first.folder_role != "trash" && first.folder_role != "junk" {
            return Err(CoreError::Invalid(
                "Move this to the bin first. Only mail in the bin or spam can be deleted for good."
                    .into(),
            ));
        }
        let uids: Vec<u32> = here.iter().map(|c| c.uid).collect();
        let mut conn = self.connect_account(account_id).await?;
        let mailbox = conn.select_folder(&first.folder_path).await?;
        if mailbox.uid_validity != first.uid_validity {
            return Err(renumbered());
        }
        conn.delete_uids(&uids).await?;
        conn.logout().await;
        for c in &here {
            self.store.delete_message(c.id)?;
        }
        Ok(json!({ "deleted": here.len() }))
    }

    /// Saves an attachment to the Downloads folder.
    /// One attachment's name, type and bytes, for a host that hands it to a browser.
    pub async fn attachment(
        &self,
        message_id: i64,
        part: u32,
    ) -> Result<(String, String, Vec<u8>)> {
        let raw = self.readable_message(message_id).await?;
        parse::attachment_part(&raw, part)
            .ok_or_else(|| CoreError::NotFound("That attachment is no longer there.".into()))
    }

    /// A look at an attachment without saving it anywhere: pictures, PDFs and plain text,
    /// up to a size. Anything else, or anything bigger, says there's no preview.
    pub(crate) async fn attachment_preview(&self, message_id: i64, part: u32) -> Result<Value> {
        const MOST_BYTES: usize = 15 * 1024 * 1024;
        const MOST_TEXT: usize = 200_000;
        let (name, mime, bytes) = self.attachment(message_id, part).await?;
        let kind = preview_kind(&name, &mime);
        if kind.is_none() || bytes.len() > MOST_BYTES {
            return Ok(json!({ "kind": "none", "name": name }));
        }
        let (kind, shown_as) = kind.unwrap_or(("none", ""));
        if kind == "text" {
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MOST_TEXT)]).into_owned();
            return Ok(
                json!({ "kind": "text", "name": name, "text": text, "cut_short": bytes.len() > MOST_TEXT }),
            );
        }
        Ok(json!({
            "kind": kind,
            "name": name,
            "mime": shown_as,
            "data": data_encoding::BASE64.encode(&bytes),
        }))
    }

    pub(crate) async fn save_attachment(&self, message_id: i64, part: u32) -> Result<Value> {
        let raw = self.readable_message(message_id).await?;
        let (name, _, bytes) = parse::attachment_part(&raw, part)
            .ok_or_else(|| CoreError::NotFound("That attachment is no longer there.".into()))?;
        self.save_to_downloads(&name, &bytes)
    }

    /// An email exactly as it came from the server, to read. A very large one is cut short, as
    /// attachments are what make it large, and it can still be downloaded whole.
    pub(crate) async fn message_source(&self, message_id: i64) -> Result<Value> {
        let raw = self.raw_message(message_id).await?;
        let mut source = crate::spam::original_facts(&raw);
        source["text"] = json!(String::from_utf8_lossy(&raw[..raw.len().min(SOURCE_SHOWN)]));
        source["cut_short"] = json!(raw.len() > SOURCE_SHOWN);
        Ok(source)
    }

    /// An email exactly as it came, with a file name, for a host that hands it to a browser.
    pub async fn original(&self, message_id: i64) -> Result<(String, Vec<u8>)> {
        let raw = self.raw_message(message_id).await?;
        let subject = parse::parse_headers(&raw).subject;
        let name = match subject.trim() {
            "" => "Email".to_string(),
            subject => subject.replace(['/', '\\'], "-"),
        };
        Ok((format!("{name}.eml"), raw))
    }

    /// An email as a file any mail app can open.
    pub(crate) async fn save_original(&self, message_id: i64) -> Result<Value> {
        let raw = self.raw_message(message_id).await?;
        let subject = parse::parse_headers(&raw).subject;
        let name = match subject.trim() {
            "" => "Email".to_string(),
            subject => subject.replace(['/', '\\'], "-"),
        };
        self.save_to_downloads(&format!("{name}.eml"), &raw)
    }

    pub(crate) fn save_to_downloads(&self, name: &str, bytes: &[u8]) -> Result<Value> {
        std::fs::create_dir_all(&self.downloads_dir)
            .map_err(|e| CoreError::Invalid(format!("Could not save the file: {e}")))?;
        let path = unique_path(&self.downloads_dir, &safe_file_name(name));
        std::fs::write(&path, bytes)
            .map_err(|e| CoreError::Invalid(format!("Could not save the file: {e}")))?;
        mark_as_from_the_internet(&path);
        let shown = path.to_string_lossy().to_string();
        self.saved_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(shown.clone());
        Ok(json!({
            "path": shown,
            "name": path.file_name().map(|n| n.to_string_lossy().to_string()),
            "can_open": can_open(&path),
        }))
    }

    /// Opens or reveals a file. Only ones this app saved, so nothing else can use
    /// this to open arbitrary files.
    pub(crate) fn use_saved_file(&self, path: &str, open: bool) -> Result<Value> {
        let known = self
            .saved_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(path);
        if !known {
            return Err(CoreError::Invalid(
                "That file isn't one this app saved.".into(),
            ));
        }
        let handler = self
            .file_opener
            .as_ref()
            .ok_or_else(|| CoreError::Invalid("This app can't open files.".into()))?;
        if open && !can_open(Path::new(path)) {
            return Err(CoreError::Invalid(
                "This kind of file can run programs, so it can only be shown in Finder.".into(),
            ));
        }
        handler(path, open);
        Ok(json!({ "ok": true }))
    }

    pub(crate) fn update_account_settings(&self, a: SettingsUpdate) -> Result<Value> {
        let host = a.smtp_host.trim().to_ascii_lowercase();
        if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CoreError::Invalid("Enter a valid server name.".into()));
        }
        if !host.is_empty() && a.smtp_port == 0 {
            return Err(CoreError::Invalid("Enter a valid server port.".into()));
        }
        if a.smtp_security == SmtpSecurity::Plain
            && !host.is_empty()
            && !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
        {
            return Err(CoreError::Invalid(
                "Unencrypted connections are only allowed to this computer.".into(),
            ));
        }
        let mut account = self.store.account(a.account_id)?;
        account.display_name = a.display_name.trim().to_string();
        account.signature = a.signature;
        account.smtp_host = host;
        account.smtp_port = a.smtp_port;
        account.smtp_security = a.smtp_security;
        account.settings_updated_at = crate::identity::unix_now();
        self.store.save_account(&account)?;
        Ok(json!({ "saved": true }))
    }

    /// Saves an inbox's own choices, and tells other devices if it can.
    pub(crate) async fn update_account_prefs(
        &self,
        account_id: i64,
        prefs: AccountPrefs,
    ) -> Result<Value> {
        if !matches!(prefs.reply_from.as_str(), "arrived" | "main")
            || !matches!(prefs.alerts.as_str(), "default" | "all" | "people" | "none")
            || prefs.color.is_some_and(|c| c > 360)
            || prefs
                .folder_roles
                .values()
                .any(|r| !matches!(r.as_str(), "sent" | "drafts" | "archive" | "junk" | "trash"))
        {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        if let Some(bad) = prefs
            .aliases
            .iter()
            .find(|a| a.address.trim().parse::<lettre::Address>().is_err())
        {
            return Err(CoreError::Invalid(format!(
                "\"{}\" isn't a valid email address.",
                bad.address
            )));
        }
        let mut account = self.store.account(account_id)?;
        // A settings page opened before this changed mustn't change it back.
        let (team_shared, team_id) = (account.prefs.team_shared, account.prefs.team_id);
        account.prefs = prefs;
        account.prefs.team_shared = team_shared;
        account.prefs.team_id = team_id;
        account.settings_updated_at = crate::identity::unix_now();
        self.store.save_account(&account)?;
        // Telling other devices can wait if this one is offline.
        let _ = self.vault_sync().await;
        Ok(json!({ "saved": true }))
    }

    /// Changes how an inbox signs in. The new details must work before they're saved,
    /// so a typing mistake can't lock the person out of an inbox that was working.
    pub(crate) async fn update_connection(&self, c: ConnectionUpdate) -> Result<Value> {
        let current = self.store.account(c.account_id)?;
        let host = c.imap_host.trim().to_ascii_lowercase();
        if host.is_empty() || host.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
            return Err(CoreError::Invalid("Enter a valid server name.".into()));
        }
        if c.imap_port == 0 {
            return Err(CoreError::Invalid("Enter a valid server port.".into()));
        }
        let password = match c.password.as_deref() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => self
                .secrets
                .get(&account_secret(c.account_id))?
                .ok_or_else(|| {
                    CoreError::Secret("The password for this account is missing.".into())
                })?,
        };
        let mut next = current;
        next.imap_host = host;
        next.imap_port = c.imap_port;
        next.security = c.security;
        next.username = match c.username.trim() {
            "" => next.email.clone(),
            u => u.to_string(),
        };
        crate::imap::check_security(&next.imap_host, next.security)?;
        ImapConnection::connect(&next, &password, self.reach())
            .await?
            .logout()
            .await;
        next.settings_updated_at = crate::identity::unix_now();
        self.store.save_account(&next)?;
        self.secrets.set(&account_secret(next.id), &password)?;
        // Telling other devices can wait if this one is offline.
        let _ = self.vault_sync().await;
        Ok(json!({ "saved": true }))
    }

    pub(crate) fn privacy_stats(&self) -> Result<Value> {
        let trackers: Vec<Value> = self
            .store
            .tracker_counts()?
            .into_iter()
            .map(|(company, seen)| json!({ "company": company, "seen": seen }))
            .collect();
        Ok(json!({ "images_held": self.store.images_held_count()?, "trackers": trackers }))
    }
}

pub(crate) struct ConnectionUpdate {
    pub account_id: i64,
    pub imap_host: String,
    pub imap_port: u16,
    pub security: Security,
    pub username: String,
    pub password: Option<String>,
}

/// One conversation in a selection, identified together with its inbox and the
/// folder it was selected in.
#[derive(serde::Deserialize, Clone)]
pub(crate) struct BulkThread {
    pub account_id: i64,
    pub thread_id: String,
    pub folder_id: i64,
}

pub(crate) struct SettingsUpdate {
    pub account_id: i64,
    pub display_name: String,
    pub signature: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
}

fn can_open(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    SAFE_TO_OPEN.contains(&ext.as_str())
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..10_000 {
        let next = dir.join(format!("{stem} ({n}){ext}"));
        if !next.exists() {
            return next;
        }
    }
    dir.join(format!("{}-{name}", crate::crypto::random_hex(4)))
}

/// Tells macOS this came from the internet, so it warns before running it, as it
/// does for browser downloads. Without this, a harmful attachment skips that check.
fn mark_as_from_the_internet(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let stamp = crate::identity::unix_now();
        let value = format!("0081;{stamp:x};Mailzo;");
        let _ = std::process::Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", &value])
            .arg(path)
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

/// What an attachment can be shown as, and the type it's shown with. Decided by its name
/// first, so a file can't pass itself off as something else, then its stated type.
fn preview_kind(name: &str, mime: &str) -> Option<(&'static str, &'static str)> {
    let name = name.to_ascii_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let mime = mime.to_ascii_lowercase();
    let picture = |m: &'static str| Some(("image", m));
    match ext {
        "pdf" => Some(("pdf", "application/pdf")),
        "png" => picture("image/png"),
        "jpg" | "jpeg" => picture("image/jpeg"),
        "gif" => picture("image/gif"),
        "webp" => picture("image/webp"),
        "heic" => picture("image/heic"),
        "txt" | "csv" | "md" | "log" | "json" | "ics" | "vcf" => Some(("text", "text/plain")),
        "" => match mime.as_str() {
            "application/pdf" => Some(("pdf", "application/pdf")),
            "image/png" => picture("image/png"),
            "image/jpeg" => picture("image/jpeg"),
            "image/gif" => picture("image/gif"),
            "text/plain" => Some(("text", "text/plain")),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_files_cannot_be_opened_directly() {
        assert!(can_open(Path::new("/x/plan.pdf")));
        assert!(can_open(Path::new("/x/photo.JPG")));
        assert!(!can_open(Path::new("/x/Evil.app")));
        assert!(!can_open(Path::new("/x/run.COMMAND")));
        assert!(!can_open(Path::new("/x/noextension")));
        // Not on the list means Finder only, even kinds nobody thought of yet.
        assert!(!can_open(Path::new("/x/invoice.terminal")));
        assert!(!can_open(Path::new("/x/notes.html")));
        assert!(!can_open(Path::new("/x/thing.someday")));
        assert!(can_open(Path::new("/x/song.mp3")));
    }

    #[test]
    fn saved_files_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("inbox-{}", crate::crypto::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), b"1").unwrap();
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a (1).pdf"));
        assert_eq!(unique_path(&dir, "b.pdf"), dir.join("b.pdf"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
