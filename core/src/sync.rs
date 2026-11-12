//! Brings the local database up to date with the mail server.
//!
//! This phase only reads. Folders are opened read-only and bodies are fetched
//! with PEEK, so syncing can never mark mail as read or delete anything.

use crate::error::{CoreError, Result};
use crate::imap::{ImapConnection, RemoteFolder, RemoteHeader};
use crate::model::{Account, NewMail, SyncStats};
use crate::parse;
use crate::pgp::Ring;
use crate::store::{IncomingMessage, Store};
use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// How many of the newest messages to keep on first sync of the inbox.
    pub inbox_window: u32,
    pub other_window: u32,
    /// How many bodies to download per sync, so mail reads offline.
    pub inbox_bodies: usize,
    pub other_bodies: usize,
    pub body_days: i64,
    /// Larger messages are downloaded only when opened.
    pub body_max_bytes: u32,
    /// How long each refresh may spend bringing down older mail. Zero means never.
    pub backfill_seconds: u64,
    /// Older mail than this keeps only its headers until opened. Zero means keep it all.
    pub keep_since: i64,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            inbox_window: 2000,
            other_window: 500,
            inbox_bodies: 200,
            other_bodies: 25,
            body_days: 90,
            body_max_bytes: 5_000_000,
            backfill_seconds: 20,
            keep_since: 0,
        }
    }
}

const BODY_BATCH: usize = 10;
/// Downloads are kept to about this much at a time, so one can't run out the clock.
const BODY_BATCH_BYTES: u64 = 4_000_000;
/// How often every message's marks are compared, whatever the server says has changed.
const COMPARE_EVERY_SECONDS: i64 = 86_400;
const HEADER_BATCH: usize = 500;
const FLAG_BATCH: usize = 5_000;
const BODIES_PER_ROUND: usize = 200;

/// Applies the person's choices of which folder does which job. A folder they chose
/// takes the job from whichever folder the server gave it to.
pub fn with_chosen_roles(mut folders: Vec<RemoteFolder>, account: &Account) -> Vec<RemoteFolder> {
    let chosen = &account.prefs.folder_roles;
    for f in &mut folders {
        if let Some(role) = chosen.get(&f.path) {
            f.role = role.clone();
        } else if chosen.values().any(|r| *r == f.role) {
            f.role = "folder".to_string();
        }
    }
    folders
}

pub async fn sync_account(
    store: &Store,
    account: &Account,
    password: &str,
    opts: &SyncOptions,
    ring: &Ring<'_>,
    reach: crate::net::Reach,
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let mut conn = ImapConnection::connect(account, password, reach).await?;

    let remote = with_chosen_roles(conn.folders().await?, account);
    let list: Vec<(String, String, String)> = remote
        .into_iter()
        .map(|f| (f.path, f.name, f.role))
        .collect();
    let folders = store.replace_folders(account.id, &list)?;

    let mut gone = false;
    for (folder_id, path, role) in &folders {
        let done = sync_folder(
            &mut conn, store, account.id, *folder_id, path, role, opts, ring, &mut stats,
        )
        .await;
        store.note_folder_problem(
            *folder_id,
            done.as_ref().err().map(|e| e.to_string()).as_deref(),
        )?;
        match done {
            Ok(()) => stats.folders += 1,
            Err(e @ CoreError::Network(_)) => {
                // The connection is gone. Try once more for the remaining folders.
                stats.errors.push(format!("{path}: {e}"));
                match ImapConnection::connect(account, password, reach).await {
                    Ok(fresh) => conn = fresh,
                    Err(e) => {
                        stats.errors.push(e.to_string());
                        gone = true;
                        break;
                    }
                }
            }
            // One bad folder must not stop the rest.
            Err(e) => stats.errors.push(format!("{path}: {e}")),
        }
    }
    if gone {
        return Ok(stats);
    }
    if opts.backfill_seconds > 0 {
        let until = Instant::now() + Duration::from_secs(opts.backfill_seconds);
        // Not being able to fetch older mail right now doesn't spoil the refresh.
        if let Err(e) = backfill(
            &mut conn, store, account.id, &folders, opts, ring, until, &mut stats,
        )
        .await
        {
            stats.errors.push(e.to_string());
            stats.more_to_download = false;
            return Ok(stats);
        }
    }
    conn.logout().await;
    Ok(stats)
}

/// Only what's just landed, so someone hears about it straight away. Going through a big inbox
/// for what's been read or deleted can take many seconds, and `sync_inbox` does that next.
#[allow(clippy::too_many_arguments)]
pub async fn look_for_new_mail(
    conn: &mut ImapConnection,
    store: &Store,
    account_id: i64,
    folder_id: i64,
    path: &str,
    role: &str,
    opts: &SyncOptions,
    ring: &Ring<'_>,
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let mailbox = conn.open_folder(path).await?;
    let (validity, last_uid) = store.folder_state(folder_id)?;
    // A folder never seen, or renumbered, needs the whole look. One seen empty is fine.
    if validity.is_none() || validity != mailbox.uid_validity {
        return Ok(stats);
    }
    if mailbox.uid_next.is_some_and(|next| next <= last_uid + 1) {
        return Ok(stats);
    }
    // Servers always include their highest number in this, even when it's one already held.
    let mut new_uids: Vec<u32> = conn
        .search_uids(&format!("UID {}:*", last_uid + 1))
        .await?
        .into_iter()
        .filter(|uid| *uid > last_uid)
        .collect();
    new_uids.sort_unstable();
    save_new_mail(
        conn, store, account_id, folder_id, role, &new_uids, false, &mut stats,
    )
    .await?;
    // The spam check reads these, and it decides whether there's a banner at all.
    let budget = new_uids.len().min(opts.inbox_bodies);
    let since = now() - opts.body_days * 86_400;
    let wanted = store.bodies_wanted(folder_id, since, opts.body_max_bytes, budget)?;
    fetch_bodies(conn, store, folder_id, &wanted, None, ring, &mut stats).await?;
    Ok(stats)
}

/// Only the inbox, for when the server has said something changed there.
#[allow(clippy::too_many_arguments)]
pub async fn sync_inbox(
    mut conn: ImapConnection,
    store: &Store,
    account: &Account,
    folder_id: i64,
    path: &str,
    opts: &SyncOptions,
    ring: &Ring<'_>,
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let done = sync_folder(
        &mut conn, store, account.id, folder_id, path, "inbox", opts, ring, &mut stats,
    )
    .await;
    store.note_folder_problem(
        folder_id,
        done.as_ref().err().map(|e| e.to_string()).as_deref(),
    )?;
    done?;
    conn.logout().await;
    stats.folders = 1;
    Ok(stats)
}

/// Brings down the rest of the mailbox, newest first, a little at a time. Inboxes first,
/// because that's what's most likely to be looked for.
#[allow(clippy::too_many_arguments)]
async fn backfill(
    conn: &mut ImapConnection,
    store: &Store,
    account_id: i64,
    folders: &[(i64, String, String)],
    opts: &SyncOptions,
    ring: &Ring<'_>,
    until: Instant,
    stats: &mut SyncStats,
) -> Result<()> {
    let mut opened: Option<i64> = None;
    for (folder_id, path, _) in folders {
        let (_, _, total) = store.folder_server_state(*folder_id)?;
        let Some(total) = total else { continue };
        if store.saved_count(*folder_id)? >= total {
            continue;
        }
        if Instant::now() >= until {
            stats.more_to_download = true;
            return Ok(());
        }
        conn.open_folder(path).await?;
        opened = Some(*folder_id);
        let on_server = conn.search_uids("ALL").await?;
        let (_, last_uid) = store.folder_state(*folder_id)?;
        let saved: BTreeSet<u32> = store.local_uids(*folder_id)?.into_iter().collect();
        // Anything newer than the refresh's position is new mail, which it brings in itself
        // so that filters and rules get to see it.
        let mut missing: Vec<u32> = on_server
            .into_iter()
            .filter(|u| *u <= last_uid && !saved.contains(u))
            .collect();
        missing.sort_unstable_by(|a, b| b.cmp(a));
        for batch in missing.chunks(HEADER_BATCH) {
            if Instant::now() >= until {
                stats.more_to_download = true;
                return Ok(());
            }
            let headers = conn.headers_for_uids(batch).await?;
            // A refusal looks the same as an empty answer, so this waits for the next refresh.
            if headers.is_empty() {
                return Ok(());
            }
            let incoming: Vec<IncomingMessage> = headers.iter().map(to_incoming).collect();
            stats.older_saved += store.insert_found_messages(account_id, *folder_id, &incoming)?;
        }
    }
    for (folder_id, path, _) in folders {
        loop {
            // Twin copies first, which costs nothing.
            while store.copy_twin_bodies(*folder_id, 500)? > 0 {}
            let wanted = store.bodies_wanted(
                *folder_id,
                opts.keep_since,
                opts.body_max_bytes,
                BODIES_PER_ROUND,
            )?;
            if wanted.is_empty() {
                break;
            }
            if Instant::now() >= until {
                stats.more_to_download = true;
                return Ok(());
            }
            if opened != Some(*folder_id) {
                conn.open_folder(path).await?;
                opened = Some(*folder_id);
            }
            if !fetch_bodies(conn, store, *folder_id, &wanted, Some(until), ring, stats).await? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Downloads these bodies in sensible amounts. Says whether it's worth carrying on: an
/// empty answer can mean the server is refusing for now, and saying so isn't always
/// passed on. Mail is never removed here. Only comparing with the server's list does that.
async fn fetch_bodies(
    conn: &mut ImapConnection,
    store: &Store,
    folder_id: i64,
    wanted: &[(u32, u32)],
    until: Option<Instant>,
    ring: &Ring<'_>,
    stats: &mut SyncStats,
) -> Result<bool> {
    let mut batch: Vec<u32> = Vec::new();
    let mut bytes = 0u64;
    let mut position = 0;
    while position < wanted.len() || !batch.is_empty() {
        let full = batch.len() >= BODY_BATCH || bytes >= BODY_BATCH_BYTES;
        if !full && position < wanted.len() {
            let (uid, size) = wanted[position];
            batch.push(uid);
            bytes += u64::from(size);
            position += 1;
            continue;
        }
        if until.is_some_and(|u| Instant::now() >= u) {
            stats.more_to_download = true;
            return Ok(true);
        }
        let got = conn.bodies(&batch).await?;
        if got.is_empty() {
            // Either the server is refusing for now, or these are ones it can't hand over.
            // One at a time tells which, and stops a bad email holding up the rest forever.
            let first = batch[0];
            let one = conn.bodies(&[first]).await?;
            if one.is_empty() {
                store.note_body_tries(folder_id, &[first])?;
                return Ok(false);
            }
            for (uid, raw) in one {
                store.save_body_by_uid(folder_id, uid, &crate::pgp::read_body(&raw, ring))?;
                stats.bodies_fetched += 1;
            }
            batch.remove(0);
            bytes = 0;
            continue;
        }
        let returned: BTreeSet<u32> = got.iter().map(|(u, _)| *u).collect();
        for (uid, raw) in got {
            store.save_body_by_uid(folder_id, uid, &crate::pgp::read_body(&raw, ring))?;
            stats.bodies_fetched += 1;
        }
        let skipped: Vec<u32> = batch
            .iter()
            .copied()
            .filter(|u| !returned.contains(u))
            .collect();
        store.note_body_tries(folder_id, &skipped)?;
        batch.clear();
        bytes = 0;
    }
    Ok(true)
}

/// What a first look at a folder brings down: as much as fits, newest first, so a new inbox
/// shows its latest mail straight away. Should it stop partway, filling in older mail picks
/// up the rest.
fn first_look_uids(on_server: &BTreeSet<u32>, window: usize) -> Vec<u32> {
    on_server.iter().rev().take(window).copied().collect()
}

/// Brings a folder up to date right after mail was moved into it, so it's there without waiting
/// for the next refresh. Call it while the moved mail's old copies are still saved, as their
/// bodies carry over from them.
pub(crate) async fn pull_folder(
    conn: &mut ImapConnection,
    store: &Store,
    account_id: i64,
    path: &str,
) -> Result<()> {
    let folders = store.list_folders(account_id)?;
    let Some(folder) = folders.iter().find(|f| f.path == path) else {
        return Ok(());
    };
    let opts = SyncOptions {
        inbox_bodies: 0,
        other_bodies: 0,
        ..SyncOptions::default()
    };
    let mut stats = SyncStats::default();
    sync_folder(
        conn,
        store,
        account_id,
        folder.id,
        path,
        &folder.role,
        &opts,
        // No bodies come down here, so there's nothing to open.
        &Ring { own: None, store },
        &mut stats,
    )
    .await
}

/// Saves what's newly landed, and notes which of it is news.
#[allow(clippy::too_many_arguments)]
async fn save_new_mail(
    conn: &mut ImapConnection,
    store: &Store,
    account_id: i64,
    folder_id: i64,
    role: &str,
    new_uids: &[u32],
    first_look: bool,
    stats: &mut SyncStats,
) -> Result<()> {
    let mut new: Vec<RemoteHeader> = Vec::new();
    for batch in new_uids.chunks(HEADER_BATCH) {
        let headers = conn.headers_for_uids(batch).await?;
        let incoming: Vec<IncomingMessage> = headers.iter().map(to_incoming).collect();
        // Keys offered by the senders, so mail to them can be encrypted. Not from spam.
        if role != "junk" && role != "trash" {
            for h in &headers {
                crate::pgp::learn_from_header(store, &h.raw_header)?;
            }
        }
        if first_look {
            stats.new_messages += store.insert_first_look(account_id, folder_id, &incoming)?;
        } else {
            // Another look can be bringing in the same mail. Whichever saves it tells people.
            let added = store.insert_new_mail(account_id, folder_id, &incoming)?;
            stats.new_messages += added.len();
            new.extend(headers.into_iter().filter(|h| added.contains(&h.uid)));
        }
    }
    // The first look at a folder finds old mail, which isn't news.
    if role == "inbox" && !first_look {
        for raw in &new {
            let m = to_incoming(raw);
            let m = &m;
            // Mail moved back into the inbox gets a new number, but isn't new.
            let old = m
                .headers
                .date
                .is_some_and(|d| d < now() - NEWS_WITHIN_SECONDS);
            if m.seen || old {
                continue;
            }
            let from = m.headers.from.clone().unwrap_or(crate::model::Addr {
                name: String::new(),
                address: String::new(),
            });
            let (key, thread_id) = store.key_and_thread(folder_id, m.uid)?;
            stats.new_mail.push(NewMail {
                account_id,
                thread_id,
                rfc_id: m.headers.rfc_id.clone(),
                key,
                is_person: crate::alerts::looks_like_a_person(&raw.raw_header, &from.address),
                from_name: from.name,
                from_address: from.address,
                subject: m.headers.subject.clone(),
                // Filled in once its text is here, before any notification.
                preview: String::new(),
                needs_you: false,
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn sync_folder(
    conn: &mut ImapConnection,
    store: &Store,
    account_id: i64,
    folder_id: i64,
    path: &str,
    role: &str,
    opts: &SyncOptions,
    ring: &Ring<'_>,
    stats: &mut SyncStats,
) -> Result<()> {
    let mailbox = conn.open_folder(path).await?;
    let (saved_validity, mut last_uid) = store.folder_state(folder_id)?;

    // A new UIDVALIDITY means the server renumbered the folder.
    let renumbered = saved_validity.is_some() && saved_validity != mailbox.uid_validity;
    if renumbered {
        stats.removed_messages += store.reset_folder(folder_id, mailbox.uid_validity)?;
        last_uid = 0;
    }

    if mailbox.exists == 0 {
        stats.removed_messages += store.reconcile(folder_id, 0, u32::MAX, &[], false)?;
        store.mark_folder_synced(folder_id, mailbox.uid_validity)?;
        store.save_folder_server_state(folder_id, mailbox.highest_modseq, mailbox.uid_next, 0)?;
        return Ok(());
    }

    // Older mail first: apply read and delete changes to what we already hold.
    let (saved_modseq, saved_next, saved_total) = store.folder_server_state(folder_id)?;
    let compare_all = now() - store.flags_checked_at(folder_id)? > COMPARE_EVERY_SECONDS;
    let nothing_changed = !compare_all
        && last_uid > 0
        && mailbox.highest_modseq.is_some()
        && saved_modseq == mailbox.highest_modseq
        && saved_next == mailbox.uid_next
        && saved_total == Some(mailbox.exists);
    let on_server: BTreeSet<u32> = if nothing_changed {
        BTreeSet::new()
    } else {
        conn.search_uids("ALL").await?.into_iter().collect()
    };
    if last_uid > 0 && !nothing_changed {
        // Mail has to be somewhere: an empty answer here means a broken reply.
        if !on_server.is_empty() {
            stats.removed_messages += store.remove_gone(folder_id, &on_server, last_uid)?;
        }
        // Reading in a shared inbox is each person's own business.
        let keep_read = store.account(account_id)?.prefs.team_shared;
        match (
            saved_modseq.filter(|_| !compare_all),
            mailbox.highest_modseq,
        ) {
            (Some(since), Some(_)) => {
                let changed = conn.flags_changed_since(last_uid, since).await?;
                stats.flags_changed += store.apply_flags(folder_id, &changed, keep_read)?;
            }
            _ => {
                let held: Vec<u32> = store
                    .local_uids(folder_id)?
                    .into_iter()
                    .filter(|u| *u <= last_uid)
                    .collect();
                for batch in held.chunks(FLAG_BATCH) {
                    let flags = conn.flags_for_uids(batch).await?;
                    stats.flags_changed += store.apply_flags(folder_id, &flags, keep_read)?;
                }
                store.note_flags_checked(folder_id)?;
            }
        }
    }

    // What's new, in small amounts saved as they arrive, so a slow connection still gets
    // somewhere instead of starting over each time.
    // A folder that was seen empty isn't new to us, and what lands in it now is new mail.
    let first_look = last_uid == 0 && (saved_validity.is_none() || renumbered);
    // Everything here is about to arrive with its flags, so there's nothing to compare yet.
    if last_uid == 0 {
        store.note_flags_checked(folder_id)?;
    }
    let window = if role == "inbox" {
        opts.inbox_window
    } else {
        opts.other_window
    };
    let new_uids: Vec<u32> = if first_look {
        first_look_uids(&on_server, window as usize)
    } else {
        on_server.range(last_uid + 1..).copied().collect()
    };
    save_new_mail(
        conn, store, account_id, folder_id, role, &new_uids, first_look, stats,
    )
    .await?;
    // Mail from before the filters existed gets its headers read once.
    if role == "inbox" {
        let missing = store.uids_missing_signals(folder_id, SIGNALS_PER_SYNC)?;
        if !missing.is_empty() {
            for h in conn.headers_for_uids(&missing).await? {
                let parsed = parse::parse_headers(&h.raw_header);
                let from = parsed.from.map(|a| a.address).unwrap_or_default();
                store.save_signals(
                    folder_id,
                    h.uid,
                    &crate::spam::read_signals(&h.raw_header, &from),
                )?;
            }
        }
    }
    store.mark_folder_synced(folder_id, mailbox.uid_validity)?;
    // Saved only once everything above has worked, so a failure gets looked at again.
    store.save_folder_server_state(
        folder_id,
        mailbox.highest_modseq,
        mailbox.uid_next,
        mailbox.exists,
    )?;

    let budget = if role == "inbox" {
        opts.inbox_bodies
    } else {
        opts.other_bodies
    };
    let since = now() - opts.body_days * 86_400;
    while store.copy_twin_bodies(folder_id, 500)? > 0 {}
    let wanted = store.bodies_wanted(folder_id, since, opts.body_max_bytes, budget)?;
    fetch_bodies(conn, store, folder_id, &wanted, None, ring, stats).await?;
    Ok(())
}

pub(crate) fn to_incoming(h: &RemoteHeader) -> IncomingMessage {
    let headers = parse::parse_headers(&h.raw_header);
    let from = headers
        .from
        .as_ref()
        .map(|a| a.address.clone())
        .unwrap_or_default();
    IncomingMessage {
        is_draft: h.is_draft,
        uid: h.uid,
        seen: h.seen,
        flagged: h.flagged,
        size: h.size,
        internal_date: h.internal_date,
        signals: Some(crate::spam::read_signals(&h.raw_header, &from)),
        headers,
    }
}

const SIGNALS_PER_SYNC: i64 = 200;
/// Older mail than this arriving in the inbox doesn't get a notification.
const NEWS_WITHIN_SECONDS: i64 = 2 * 86_400;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_look_at_a_folder_brings_the_newest_mail_first() {
        let on_server: BTreeSet<u32> = (1..=10).collect();
        assert_eq!(first_look_uids(&on_server, 3), [10, 9, 8]);
    }

    #[test]
    fn the_first_look_stops_at_what_the_folder_holds() {
        let on_server: BTreeSet<u32> = [4, 7].into();
        assert_eq!(first_look_uids(&on_server, 3), [7, 4]);
    }
}
