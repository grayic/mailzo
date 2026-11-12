//! Finding mail: in what's saved on this device straight away, then on the mail
//! servers for older mail that was never downloaded.

use crate::engine::Engine;
use crate::error::Result;
use crate::imap::ImapConnection;
use crate::model::Account;
use crate::store::IncomingMessage;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// Conversations in the first lot of results. Scrolling down asks for more.
const MOST_RESULTS: i64 = 300;
/// The newest matches taken from each folder on a mail server.
const MOST_PER_FOLDER: usize = 50;
/// How many found emails get downloaded in full, so their words can be searched here.
const BODIES_PER_FOLDER: usize = 25;
const SERVER_TIMEOUT: Duration = Duration::from_secs(60);

/// What to look for. Plain text and AI both end up as this.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchFilters {
    /// Every one of these must appear.
    #[serde(default)]
    pub words: Vec<String>,
    /// At least one of these must appear: other ways of saying the same thing.
    #[serde(default)]
    pub similar: Vec<String>,
    /// Any of these people.
    #[serde(default)]
    pub from: Vec<String>,
    #[serde(default)]
    pub to: Vec<String>,
    /// Mail from, to or copied to any of these: an address, or "@" and a company's domain.
    #[serde(default)]
    pub with: Vec<String>,
    #[serde(default)]
    pub after: Option<i64>,
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub has_attachment: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub starred: bool,
    /// One kind of folder, such as `sent`.
    #[serde(default)]
    pub folder: Option<String>,
    /// Include the bin and spam, which are left out unless asked for.
    #[serde(default)]
    pub include_bin: bool,
    /// Only this inbox. Where to look, not something to find, so alone it isn't a search.
    #[serde(default)]
    pub account: Option<i64>,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.similar.is_empty()
            && self.from.is_empty()
            && self.to.is_empty()
            && self.with.is_empty()
            && self.after.is_none()
            && self.before.is_none()
            && !self.has_attachment
            && !self.unread
            && !self.starred
            && self.folder.is_none()
    }
}

/// Reads what was typed. Understands quotes, `from:`, `to:`, `has:attachment`,
/// `is:unread`, `is:starred` and `in:sent`. Everything else is words to find.
pub fn parse_plain(text: &str) -> SearchFilters {
    let mut f = SearchFilters::default();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (token, quoted, after) = next_token(rest);
        rest = after.trim_start();
        if quoted {
            f.words.push(token);
            continue;
        }
        let lower = token.to_lowercase();
        match lower.split_once(':') {
            Some(("from", v)) if !v.is_empty() => f.from.push(unquote(&token, "from:")),
            Some(("to", v)) if !v.is_empty() => f.to.push(unquote(&token, "to:")),
            Some(("has", "attachment" | "attachments" | "file" | "files")) => {
                f.has_attachment = true
            }
            Some(("is", "unread")) => f.unread = true,
            Some(("is", "starred")) => f.starred = true,
            Some(("in", v)) if !v.is_empty() => f.folder = Some(folder_role(v)),
            _ => f.words.push(token),
        }
    }
    f.include_bin = matches!(f.folder.as_deref(), Some("trash" | "junk"));
    f
}

fn unquote(token: &str, prefix: &str) -> String {
    token[prefix.len()..].trim_matches('"').to_string()
}

fn folder_role(name: &str) -> String {
    match name {
        "bin" | "deleted" => "trash",
        "spam" => "junk",
        other => other,
    }
    .to_string()
}

/// The next word or quoted phrase, whether it was quoted, and what's left.
fn next_token(text: &str) -> (String, bool, &str) {
    if let Some(inner) = text.strip_prefix('"') {
        return match inner.split_once('"') {
            Some((phrase, rest)) => (phrase.to_string(), true, rest),
            None => (inner.to_string(), true, ""),
        };
    }
    // A quoted value after a label, such as from:"Maya Chen", stays in one piece.
    let mut in_quotes = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                return (text[..i].to_string(), false, &text[i..])
            }
            _ => {}
        }
    }
    (text.to_string(), false, "")
}

fn has_letters(word: &str) -> bool {
    word.chars().any(char::is_alphanumeric)
}

/// One word for the saved-mail index, matching the start of longer words too.
fn index_term(word: &str) -> String {
    let safe = word.replace('"', "\"\"");
    if word.trim().contains(char::is_whitespace) {
        format!("\"{safe}\"")
    } else {
        format!("\"{safe}\"*")
    }
}

/// Base letters for Latin Extended-A (U+0100 to U+017F), in order, so Ł, ő and š compare as l, o, s.
const LATIN_EXTENDED_A: &str = "aaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiiiiijjkkkllllllllllnnnnnnnnnoooooooorrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs";

fn base_letter(c: char) -> char {
    match c {
        'À'..='Æ' | 'à'..='æ' => 'a',
        'Ç' | 'ç' => 'c',
        'È'..='Ë' | 'è'..='ë' => 'e',
        'Ì'..='Ï' | 'ì'..='ï' => 'i',
        'Ð' | 'ð' => 'd',
        'Ñ' | 'ñ' => 'n',
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' => 'o',
        'Ù'..='Ü' | 'ù'..='ü' => 'u',
        'Ý' | 'ý' | 'ÿ' => 'y',
        '\u{100}'..='\u{17f}' => LATIN_EXTENDED_A
            .as_bytes()
            .get(c as usize - 0x100)
            .map_or(c, |b| *b as char),
        _ => c,
    }
}

/// Lowercase, with the accents the saved-mail index ignores taken off, so "Café" and "cafe",
/// "Thời" and "thoi" match. Only letters written with accents are taken apart, so Hangul
/// stays as it's written.
pub(crate) fn fold(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let code = c as u32;
        if code < 0x1100 || (0x1e00..=0x1fff).contains(&code) {
            for d in std::iter::once(c).nfd() {
                if !unicode_normalization::char::is_combining_mark(d) {
                    out.extend(base_letter(d).to_lowercase());
                }
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Words the way the saved-mail index splits them: runs of letters and digits, so
/// "all-hands" is "all hands" and "client's" is "client s" whichever apostrophe was used.
fn words_of(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether `asked` is in `have` as a run of words: every word whole, except that the last
/// may only start one, as in the index, unless `whole`. Both are folded.
fn has_run(have: &[&str], asked: &str, whole: bool) -> bool {
    let want = words_of(asked);
    if want.is_empty() {
        return true;
    }
    have.windows(want.len()).any(|at| {
        at.iter().zip(&want).enumerate().all(|(i, (h, w))| {
            if i + 1 == want.len() && !whole {
                h.starts_with(w)
            } else {
                h == w
            }
        })
    })
}

/// Scripts written without spaces between words, where a word can sit anywhere in a run.
pub(crate) fn written_without_spaces(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
}

/// Whether an email really has every word asked for, in its subject, its people or its own
/// new text (not the old replies quoted under it). A quoted phrase must be there as a phrase.
pub(crate) fn matches_exactly(
    words: &[String],
    subject: &str,
    people: &str,
    new_text: &str,
) -> bool {
    let haystack = fold(&format!("{subject}\n{people}\n{new_text}"));
    let have = words_of(&haystack);
    words.iter().filter(|w| has_letters(w)).all(|w| {
        let w = fold(w.trim());
        if w.chars().next().is_some_and(written_without_spaces) {
            haystack.contains(&w)
        } else {
            has_run(&have, &w, false)
        }
    })
}

/// Whether a name as asked ("Sam") is this person's name or address, as whole words: Sam
/// isn't Samantha.
pub(crate) fn is_named(asked: &str, name: &str, address: &str) -> bool {
    let have = fold(&format!("{name} {address}"));
    has_letters(asked) && has_run(&words_of(&have), &fold(asked.trim()), true)
}

/// The question for the saved-mail index, or None if it isn't about words.
pub(crate) fn index_query(f: &SearchFilters) -> Option<String> {
    let mut parts: Vec<String> = f
        .words
        .iter()
        .filter(|w| has_letters(w))
        .map(|w| index_term(w))
        .collect();
    let similar: Vec<String> = f
        .similar
        .iter()
        .filter(|w| has_letters(w))
        .map(|w| index_term(w))
        .collect();
    if !similar.is_empty() {
        parts.push(format!("({})", similar.join(" OR ")));
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

fn imap_string(value: &str) -> String {
    let clean: String = value
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{clean}\"")
}

/// Asks for any of these, which IMAP writes as nested pairs.
fn imap_any(kind: &str, values: &[String]) -> Option<String> {
    let mut terms = values.iter().map(|v| format!("{kind} {}", imap_string(v)));
    let first = terms.next()?;
    Some(terms.fold(first, |all, term| format!("OR {all} {term}")))
}

pub(crate) fn imap_date(unix: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = civil_from_days(unix.div_euclid(86_400));
    format!("{d}-{}-{y}", MONTHS[(m - 1) as usize])
}

/// The calendar date for a count of days since 1970.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The question for a mail server, or None when there's nothing it can narrow by.
pub(crate) fn server_query(f: &SearchFilters) -> Option<String> {
    let mut parts: Vec<String> = f
        .words
        .iter()
        .filter(|w| has_letters(w))
        .map(|w| format!("TEXT {}", imap_string(w)))
        .collect();
    let useful: Vec<String> = f
        .similar
        .iter()
        .filter(|w| has_letters(w))
        .cloned()
        .collect();
    parts.extend(imap_any("TEXT", &useful));
    parts.extend(imap_any("FROM", &f.from));
    parts.extend(imap_any("TO", &f.to));
    if parts.is_empty() && f.after.is_none() && f.before.is_none() {
        return None;
    }
    if let Some(after) = f.after {
        parts.push(format!("SINCE {}", imap_date(after)));
    }
    if let Some(before) = f.before {
        parts.push(format!("BEFORE {}", imap_date(before)));
    }
    if f.unread {
        parts.push("UNSEEN".into());
    }
    if f.starred {
        parts.push("FLAGGED".into());
    }
    let query = parts.join(" ");
    Some(if query.is_ascii() {
        query
    } else {
        format!("CHARSET UTF-8 {query}")
    })
}

impl Engine {
    /// Everything saved on this device that matches, newest first.
    pub(crate) fn search_mail(
        &self,
        filters: &SearchFilters,
        extra_ids: &[i64],
        most: Option<i64>,
    ) -> Result<Value> {
        let grouped = self.setting("conversations")? == "on";
        let most = most.unwrap_or(MOST_RESULTS).max(1);
        let (threads, in_bin) = self
            .store
            .search_threads(filters, extra_ids, most, grouped)?;
        Ok(json!({ "threads": threads, "in_bin": in_bin, "filters": filters }))
    }

    /// Asks each mail server searched for older mail, saves what it finds, and returns all
    /// the matches. An inbox that can't be reached is named, and the rest still count.
    pub(crate) async fn search_servers(&self, filters: &SearchFilters) -> Result<Value> {
        let mut accounts = self.store.list_accounts()?;
        accounts.retain(|a| filters.account.is_none_or(|id| a.id == id));
        let Some(query) = server_query(filters) else {
            let mut out = self.search_mail(filters, &[], None)?;
            out["extra_ids"] = json!([]);
            out["unreached"] = json!([]);
            return Ok(out);
        };
        let searches = accounts.iter().map(|account| {
            let query = query.clone();
            async move {
                let found = tokio::time::timeout(
                    SERVER_TIMEOUT,
                    self.search_one_server(account, &query, filters),
                )
                .await;
                match found {
                    Ok(Ok(ids)) => Ok(ids),
                    _ => Err(account.email.clone()),
                }
            }
        });
        let mut ids = Vec::new();
        let mut unreached = Vec::new();
        for result in join_all(searches).await {
            match result {
                Ok(found) => ids.extend(found),
                Err(email) => unreached.push(email),
            }
        }
        let mut out = self.search_mail(filters, &ids, None)?;
        out["extra_ids"] = json!(ids);
        out["unreached"] = json!(unreached);
        Ok(out)
    }

    /// Mail stays exactly as it is on the server: folders open read-only here.
    async fn search_one_server(
        &self,
        account: &Account,
        query: &str,
        filters: &SearchFilters,
    ) -> Result<Vec<i64>> {
        let password = self.password(account.id).await?;
        let mut conn = ImapConnection::connect(account, &password, self.reach()).await?;
        let mut ids = Vec::new();
        for folder in self.store.list_folders(account.id)? {
            let wanted = match filters.folder.as_deref() {
                Some(role) => folder.role == role,
                None if folder.role == "drafts" => false,
                None => filters.include_bin || !matches!(folder.role.as_str(), "trash" | "junk"),
            };
            if !wanted {
                continue;
            }
            // One odd folder shouldn't cost the rest their results.
            if let Ok(found) = self.search_folder(&mut conn, account, &folder, query).await {
                ids.extend(found);
            }
        }
        conn.logout().await;
        Ok(ids)
    }

    async fn search_folder(
        &self,
        conn: &mut ImapConnection,
        account: &Account,
        folder: &crate::model::Folder,
        query: &str,
    ) -> Result<Vec<i64>> {
        let mailbox = conn.open_folder(&folder.path).await?;
        // Numbers from a folder that's been rebuilt are for the next refresh to sort out.
        let known = self.store.folder_uid_validity(folder.id)?;
        if known.is_some() && known != mailbox.uid_validity {
            return Ok(vec![]);
        }
        let mut uids = conn.search_uids(query).await?;
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.truncate(MOST_PER_FOLDER);
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let missing = self.store.uids_not_saved(folder.id, &uids)?;
        if !missing.is_empty() {
            let incoming: Vec<IncomingMessage> = conn
                .headers_for_uids(&missing)
                .await?
                .iter()
                .map(crate::sync::to_incoming)
                .collect();
            self.store
                .insert_found_messages(account.id, folder.id, &incoming)?;
        }
        let mut wanting =
            self.store
                .uids_wanting_body(folder.id, &uids, self.options.body_max_bytes)?;
        wanting.truncate(BODIES_PER_FOLDER);
        for (uid, raw) in conn.bodies(&wanting).await? {
            let body = crate::pgp::read_body(&raw, &self.pgp_ring(account.id));
            self.store.save_body_by_uid(folder.id, uid, &body)?;
        }
        self.store.message_ids_for_uids(folder.id, &uids)
    }
}
