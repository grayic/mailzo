//! The address book: people the person chose to keep, with their names, addresses, phones
//! and notes, beside the people they email most who aren't kept yet. Kept people follow
//! the person to their other devices in the vault, one setting each, newest change winning.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::identity::unix_now;
use crate::model::Contact;
use crate::store::Store;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Each kept person travels as `person:<uid>`.
pub(crate) const PERSON_PREFIX: &str = "person:";
const EXPORT_NAME: &str = "Mailzo contacts.vcf";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Person {
    pub id: i64,
    pub uid: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub company: String,
    pub notes: String,
    pub updated_at: i64,
    #[serde(skip)]
    pub deleted: bool,
}

impl Person {
    fn new(uid: String) -> Person {
        Person {
            id: 0,
            uid,
            name: String::new(),
            emails: Vec::new(),
            phones: Vec::new(),
            company: String::new(),
            notes: String::new(),
            updated_at: 0,
            deleted: false,
        }
    }

    /// The name to show: their own, or else their main address.
    fn shown_name(&self) -> &str {
        if self.name.is_empty() {
            self.emails.first().map(String::as_str).unwrap_or("")
        } else {
            &self.name
        }
    }

    fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.name.to_lowercase().contains(query)
            || self.company.to_lowercase().contains(query)
            || self.emails.iter().any(|e| e.contains(query))
    }
}

/// A person as they travel in the vault.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PersonCopy {
    pub uid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub phones: Vec<String>,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub deleted: bool,
}

/// What the address book screen sends to save a person.
#[derive(Deserialize)]
pub(crate) struct PersonInput {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub phones: Vec<String>,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub notes: String,
}

/// Who a name in a question means: a person's addresses, or a company's sender domains.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// As it was asked.
    pub name: String,
    pub addresses: Vec<String>,
    pub domains: Vec<String>,
    /// Someone else with the same name, emailed nearly as often, so the answer can say which.
    pub other_match: Option<String>,
}

/// Someone emailed at least half as often as the closest match is worth mentioning.
const CLOSE_RUNNER_UP: i64 = 2;

/// Someone seen in mail: their address, a name from mail, how many emails, and the latest.
struct Seen {
    address: String,
    name: String,
    count: i64,
    last_at: Option<i64>,
}

const COLUMNS: &str =
    "id, uid, name, emails_json, phones_json, company, notes, updated_at, deleted";

/// Every address an email went between: whoever it's from, and whoever was written to from
/// a sent folder. One email kept in two folders counts once.
const EXCHANGED: &str = "ex AS (
    SELECT lower(m.from_addr) AS address, m.from_name AS name, m.date AS date,
           COALESCE(m.rfc_id, '#' || m.id) AS mid
    FROM messages m JOIN folders f ON f.id = m.folder_id WHERE f.role != 'junk'
    UNION ALL
    SELECT lower(json_extract(j.value, '$.address')), json_extract(j.value, '$.name'), m.date,
           COALESCE(m.rfc_id, '#' || m.id)
    FROM messages m JOIN folders f ON f.id = m.folder_id, json_each(m.to_json) j
    WHERE f.role = 'sent'
    UNION ALL
    SELECT lower(json_extract(j.value, '$.address')), json_extract(j.value, '$.name'), m.date,
           COALESCE(m.rfc_id, '#' || m.id)
    FROM messages m JOIN folders f ON f.id = m.folder_id, json_each(m.cc_json) j
    WHERE f.role = 'sent'
)";

fn person_row(r: &Row) -> rusqlite::Result<Person> {
    let list = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
    Ok(Person {
        id: r.get(0)?,
        uid: r.get(1)?,
        name: r.get(2)?,
        emails: list(r.get(3)?),
        phones: list(r.get(4)?),
        company: r.get(5)?,
        notes: r.get(6)?,
        updated_at: r.get(7)?,
        deleted: r.get(8)?,
    })
}

fn robot_filter(column: &str) -> String {
    crate::alerts::ROBOT_MARKS
        .iter()
        .map(|m| format!("{column} NOT LIKE '%{m}%'"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn new_uid() -> String {
    let b: [u8; 16] = rand::random();
    let hex = data_encoding::HEXLOWER.encode(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A plain check that this is an address mail could go to.
fn valid_email(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !address
            .chars()
            .any(|c| c.is_whitespace() || "<>,;:\"()[]\\".contains(c))
}

fn clean_list(items: &[String], lower: bool) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let item = item.trim();
        let item = if lower {
            item.to_lowercase()
        } else {
            item.to_string()
        };
        if !item.is_empty() && seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

impl Store {
    /// Everyone kept, in name order.
    pub fn saved_people(&self) -> Result<Vec<Person>> {
        let mut people: Vec<Person> = {
            let db = self.db();
            let mut st = db.prepare(&format!("SELECT {COLUMNS} FROM people WHERE deleted = 0"))?;
            let rows = st.query_map([], person_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        people.sort_by_key(|p| p.shown_name().to_lowercase());
        Ok(people)
    }

    pub fn person(&self, id: i64) -> Result<Option<Person>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLUMNS} FROM people WHERE id = ?1 AND deleted = 0"),
                [id],
                person_row,
            )
            .optional()?)
    }

    /// By lasting name, removed ones included.
    fn person_by_uid(&self, uid: &str) -> Result<Option<Person>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLUMNS} FROM people WHERE uid = ?1"),
                [uid],
                person_row,
            )
            .optional()?)
    }

    /// The kept person with this address, if there is one.
    pub fn person_with_address(&self, address: &str) -> Result<Option<Person>> {
        Ok(self
            .db()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM people p WHERE deleted = 0 AND EXISTS
                       (SELECT 1 FROM json_each(p.emails_json) j WHERE j.value = ?1)
                     ORDER BY id LIMIT 1"
                ),
                [address.trim().to_lowercase()],
                person_row,
            )
            .optional()?)
    }

    /// Saves a person, adding them when they're new. Returns their id.
    fn write_person(&self, p: &Person) -> Result<i64> {
        let db = self.db();
        let emails = serde_json::to_string(&p.emails).unwrap_or_else(|_| "[]".into());
        let phones = serde_json::to_string(&p.phones).unwrap_or_else(|_| "[]".into());
        if p.id == 0 {
            db.execute(
                "INSERT INTO people (uid, name, emails_json, phones_json, company, notes, updated_at, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![p.uid, p.name, emails, phones, p.company, p.notes, p.updated_at, p.deleted],
            )?;
            Ok(db.last_insert_rowid())
        } else {
            db.execute(
                "UPDATE people SET name = ?2, emails_json = ?3, phones_json = ?4, company = ?5,
                   notes = ?6, updated_at = ?7, deleted = ?8 WHERE id = ?1",
                params![
                    p.id,
                    p.name,
                    emails,
                    phones,
                    p.company,
                    p.notes,
                    p.updated_at,
                    p.deleted
                ],
            )?;
            Ok(p.id)
        }
    }

    /// Everyone, removed ones included, as they travel to other devices.
    pub(crate) fn person_copies(&self) -> Result<Vec<PersonCopy>> {
        let db = self.db();
        let mut st = db.prepare(&format!("SELECT {COLUMNS} FROM people ORDER BY uid"))?;
        let rows = st.query_map([], person_row)?;
        let people: Vec<Person> = rows.collect::<rusqlite::Result<_>>()?;
        Ok(people
            .into_iter()
            .map(|p| PersonCopy {
                uid: p.uid,
                name: p.name,
                emails: p.emails,
                phones: p.phones,
                company: p.company,
                notes: p.notes,
                updated_at: p.updated_at,
                deleted: p.deleted,
            })
            .collect())
    }

    /// Takes in people from another device. For each person the newest change wins.
    pub(crate) fn receive_people(&self, copies: &[PersonCopy]) -> Result<()> {
        for c in copies {
            if c.uid.is_empty() {
                continue;
            }
            let here = self.person_by_uid(&c.uid)?;
            if here.as_ref().is_some_and(|p| p.updated_at >= c.updated_at) {
                continue;
            }
            let person = Person {
                id: here.map(|p| p.id).unwrap_or(0),
                uid: c.uid.clone(),
                name: c.name.trim().to_string(),
                emails: clean_list(&c.emails, true),
                phones: clean_list(&c.phones, false),
                company: c.company.clone(),
                notes: c.notes.clone(),
                updated_at: c.updated_at,
                deleted: c.deleted,
            };
            self.write_person(&person)?;
        }
        Ok(())
    }

    /// People most emailed with who aren't kept, leaving out machines and the person's own
    /// addresses. The query matches an address or any name they've been seen under.
    fn mail_people(&self, own: &[String], query: &str, limit: i64) -> Result<Vec<Seen>> {
        let db = self.db();
        let sql = format!(
            "WITH {EXCHANGED}
             SELECT address, MAX(NULLIF(name, '')), COUNT(DISTINCT mid), MAX(date) FROM ex
             WHERE address IS NOT NULL AND address != '' AND {robots}
               AND address NOT IN (SELECT value FROM json_each(?1))
               AND address NOT IN (SELECT j.value FROM people p, json_each(p.emails_json) j
                                   WHERE p.deleted = 0)
             GROUP BY address
             HAVING ?2 = '' OR instr(address, ?2) > 0
                 OR SUM(instr(lower(COALESCE(name, '')), ?2) > 0) > 0
             ORDER BY COUNT(DISTINCT mid) DESC, address LIMIT ?3",
            robots = robot_filter("address"),
        );
        let own = serde_json::to_string(own).unwrap_or_else(|_| "[]".into());
        let mut st = db.prepare(&sql)?;
        let rows = st.query_map(params![own, query, limit], |r| {
            Ok(Seen {
                address: r.get(0)?,
                name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                count: r.get(2)?,
                last_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many emails each kept person has exchanged, and the latest, by their id.
    fn saved_counts(&self) -> Result<HashMap<i64, (i64, Option<i64>)>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "WITH {EXCHANGED}, saved AS (
                SELECT p.id AS pid, j.value AS address FROM people p, json_each(p.emails_json) j
                WHERE p.deleted = 0)
             SELECT s.pid, COUNT(DISTINCT ex.mid), MAX(ex.date)
             FROM ex JOIN saved s ON s.address = ex.address GROUP BY s.pid"
        ))?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The name mail most often gives these addresses, how many emails, the latest and first.
    fn mail_stats(&self, addresses: &[String]) -> Result<(String, i64, Option<i64>, Option<i64>)> {
        let list = serde_json::to_string(addresses).unwrap_or_else(|_| "[]".into());
        let db = self.db();
        let name: Option<String> = db
            .query_row(
                &format!(
                    "WITH {EXCHANGED}
                     SELECT name FROM ex WHERE address IN (SELECT value FROM json_each(?1))
                       AND COALESCE(name, '') != ''
                     GROUP BY name ORDER BY COUNT(*) DESC, name LIMIT 1"
                ),
                [&list],
                |r| r.get(0),
            )
            .optional()?;
        let (count, last, first) = db.query_row(
            &format!(
                "WITH {EXCHANGED}
                 SELECT COUNT(DISTINCT mid), MAX(date), MIN(date) FROM ex
                 WHERE address IN (SELECT value FROM json_each(?1))"
            ),
            [&list],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok((name.unwrap_or_default(), count, last, first))
    }

    /// Kept people matching what's been typed, one entry for each of their addresses.
    pub(crate) fn saved_contacts(&self, query: &str) -> Result<Vec<Contact>> {
        let query = query.trim().to_lowercase();
        Ok(self
            .saved_people()?
            .into_iter()
            .filter(|p| p.matches(&query))
            .flat_map(|p| {
                let name = p.name.clone();
                p.emails.into_iter().map(move |address| Contact {
                    name: name.clone(),
                    address,
                })
            })
            .collect())
    }
}

impl Store {
    /// Sender domains whose company is this name, exactly: "Amazon" is amazon.com and
    /// amazon.co.uk, never amazonia-deals.com.
    fn sender_domains_named(&self, name: &str) -> Result<Vec<String>> {
        let wanted: String = crate::search::fold(name).split_whitespace().collect();
        let db = self.db();
        let mut st = db.prepare(
            "SELECT DISTINCT lower(substr(m.from_addr, instr(m.from_addr, '@') + 1))
             FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE instr(m.from_addr, '@') > 0 AND f.role != 'junk'",
        )?;
        let mut domains = Vec::new();
        for domain in st.query_map([], |r| r.get::<_, String>(0))? {
            let domain = domain?;
            if !wanted.is_empty() && company_of(&domain) == Some(wanted.as_str()) {
                domains.push(domain);
            }
        }
        Ok(domains)
    }
}

/// Endings made of two parts, where the company's name comes before both: amazon.co.uk.
const TWO_PART_ENDINGS: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp", "co.in",
    "com.br", "com.mx", "co.za", "com.sg", "com.bd", "com.tr", "com.cn", "com.hk", "co.kr",
    "com.ar",
];

/// The company part of a domain: amazon for amazon.com, mail.amazon.co.uk and amazon.co.uk.
fn company_of(domain: &str) -> Option<&str> {
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    let ending = labels[labels.len() - 2..].join(".");
    let back = if labels.len() >= 3 && TWO_PART_ENDINGS.contains(&ending.as_str()) {
        3
    } else {
        2
    };
    labels.get(labels.len() - back).copied()
}

impl Engine {
    /// What each name in a question means. "me" is the person's own addresses. A person is
    /// whoever by that name has been emailed most, then most lately. A name no person has may
    /// be a company's sender domains. A name nothing matches stays as it is, to be looked for
    /// as a word.
    pub fn resolve_names(&self, names: &[String]) -> Result<Vec<Resolved>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let own = self.own_addresses()?;
        let saved = self.store.saved_people()?;
        let counts = self.store.saved_counts()?;
        let written_to = self.store.written_to()?;
        let mut out = Vec::new();
        for name in names {
            let mut found = Resolved {
                name: name.clone(),
                addresses: Vec::new(),
                domains: Vec::new(),
                other_match: None,
            };
            if matches!(name.to_lowercase().as_str(), "me" | "my" | "i" | "myself") {
                found.addresses = own.clone();
                out.push(found);
                continue;
            }
            // A saved contact by that name is who they mean, at the addresses saved for them.
            let mut kept: Vec<(&Person, i64)> = saved
                .iter()
                .filter(|p| crate::search::is_named(name, &p.name, ""))
                .map(|p| (p, counts.get(&p.id).map_or(0, |c| c.0)))
                .collect();
            kept.sort_by_key(|k| std::cmp::Reverse(k.1));
            if let Some((first, count)) = kept.first() {
                found.addresses = first.emails.clone();
                found.other_match = kept
                    .get(1)
                    .filter(|second| second.1 * CLOSE_RUNNER_UP >= *count)
                    .map(|second| second.0.name.clone());
                out.push(found);
                continue;
            }
            // A name that is a company's own domain is that company, however its emails are signed.
            found.domains = self.store.sender_domains_named(name)?;
            if !found.domains.is_empty() {
                out.push(found);
                continue;
            }
            // Anyone can sign an email with any name, so someone written to from here comes
            // first, and another address only joins them if it has been written to as well.
            let mut seen: Vec<(Seen, bool)> = Vec::new();
            for s in self
                .store
                .mail_people(&own, &crate::search::fold(name), 50)?
            {
                if crate::search::is_named(name, &s.name, &s.address) {
                    let written = written_to.contains(&s.address.to_lowercase());
                    seen.push((s, written));
                }
            }
            seen.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then(b.0.count.cmp(&a.0.count))
                    .then(b.0.last_at.cmp(&a.0.last_at))
            });
            if let Some((first, first_written)) = seen.first() {
                let same = crate::search::fold(first.name.trim());
                found.addresses.push(first.address.clone());
                for (s, written) in &seen[1..] {
                    let is_them = !same.is_empty() && crate::search::fold(s.name.trim()) == same;
                    if is_them && *written && *first_written {
                        found.addresses.push(s.address.clone());
                    } else if !is_them
                        && found.other_match.is_none()
                        && s.count * CLOSE_RUNNER_UP >= first.count
                    {
                        found.other_match = Some(s.name.clone());
                    }
                }
            }
            out.push(found);
        }
        Ok(out)
    }

    pub(crate) fn people_list(&self, query: &str, limit: i64) -> Result<Value> {
        let query = query.trim().to_lowercase();
        let limit = limit.clamp(1, 5000);
        let counts = self.store.saved_counts()?;
        let mut out: Vec<Value> = self
            .store
            .saved_people()?
            .into_iter()
            .filter(|p| p.matches(&query))
            .take(limit as usize)
            .map(|p| {
                let (count, last_at) = counts.get(&p.id).copied().unwrap_or((0, None));
                json!({
                    "id": p.id, "name": p.name, "emails": p.emails, "company": p.company,
                    "saved": true, "count": count, "last_at": last_at,
                })
            })
            .collect();
        let room = limit - out.len() as i64;
        if room > 0 {
            for s in self
                .store
                .mail_people(&self.own_addresses()?, &query, room)?
            {
                out.push(json!({
                    "id": null, "name": s.name, "emails": [s.address], "company": "",
                    "saved": false, "count": s.count, "last_at": s.last_at,
                }));
            }
        }
        Ok(Value::Array(out))
    }

    pub(crate) fn person_get(&self, address: &str) -> Result<Value> {
        let address = address.trim().to_lowercase();
        let person = self.store.person_with_address(&address)?;
        let addresses = person
            .as_ref()
            .map(|p| p.emails.clone())
            .unwrap_or_else(|| vec![address.clone()]);
        let (name, count, last_at, first_at) = self.store.mail_stats(&addresses)?;
        Ok(json!({
            "person": person, "name_from_mail": name, "count": count,
            "last_at": last_at, "first_at": first_at,
        }))
    }

    pub(crate) async fn person_save(&self, input: PersonInput) -> Result<Value> {
        let mut person = match input.id {
            Some(id) => self.store.person(id)?.ok_or_else(|| {
                CoreError::Invalid("That person isn't in your address book any more.".into())
            })?,
            None => Person::new(new_uid()),
        };
        let emails = clean_list(&input.emails, true);
        if let Some(bad) = emails.iter().find(|e| !valid_email(e)) {
            return Err(CoreError::Invalid(format!(
                "\u{201c}{bad}\u{201d} isn't an email address."
            )));
        }
        let name = input.name.trim().to_string();
        if name.is_empty() && emails.is_empty() {
            return Err(CoreError::Invalid("Add a name or an email address.".into()));
        }
        for e in &emails {
            if let Some(other) = self.store.person_with_address(e)? {
                if other.id != person.id {
                    return Err(CoreError::Invalid(format!(
                        "{e} is already saved for {}.",
                        other.shown_name()
                    )));
                }
            }
        }
        person.name = name;
        person.emails = emails;
        person.phones = clean_list(&input.phones, false);
        person.company = input.company.trim().to_string();
        person.notes = input.notes.trim().to_string();
        person.updated_at = unix_now();
        person.id = self.store.write_person(&person)?;
        self.people_changed().await?;
        Ok(json!(person))
    }

    pub(crate) async fn person_delete(&self, id: i64) -> Result<Value> {
        let mut person = self.store.person(id)?.ok_or_else(|| {
            CoreError::Invalid("That person isn't in your address book any more.".into())
        })?;
        person.deleted = true;
        person.updated_at = unix_now();
        self.store.write_person(&person)?;
        self.people_changed().await?;
        Ok(json!({ "deleted": true }))
    }

    pub(crate) async fn people_import_vcard(&self, text: &str) -> Result<Value> {
        let (mut added, mut updated, mut skipped) = (0, 0, 0);
        let now = unix_now();
        for card in parse_vcards(text) {
            let emails: Vec<String> = clean_list(&card.emails, true)
                .into_iter()
                .filter(|e| valid_email(e))
                .collect();
            let phones = clean_list(&card.phones, false);
            let name = card.name.trim().to_string();
            let mut existing = None;
            if !card.uid.is_empty() {
                existing = self.store.person_by_uid(&card.uid)?.filter(|p| !p.deleted);
            }
            if existing.is_none() {
                for e in &emails {
                    if let Some(p) = self.store.person_with_address(e)? {
                        existing = Some(p);
                        break;
                    }
                }
            }
            let is_new = existing.is_none();
            let mut person = match existing {
                Some(p) => p,
                None => {
                    let uid =
                        if !card.uid.is_empty() && self.store.person_by_uid(&card.uid)?.is_none() {
                            card.uid.clone()
                        } else {
                            new_uid()
                        };
                    Person::new(uid)
                }
            };
            let before = person.clone();
            for e in emails {
                if person.emails.contains(&e) {
                    continue;
                }
                // An address already kept for someone else stays with them.
                let taken = self
                    .store
                    .person_with_address(&e)?
                    .is_some_and(|o| o.id != person.id);
                if !taken {
                    person.emails.push(e);
                }
            }
            for t in phones {
                if !person.phones.contains(&t) {
                    person.phones.push(t);
                }
            }
            if !name.is_empty() {
                person.name = name;
            }
            if !card.company.trim().is_empty() {
                person.company = card.company.trim().to_string();
            }
            if !card.notes.trim().is_empty() {
                person.notes = card.notes.trim().to_string();
            }
            if person == before || (person.name.is_empty() && person.emails.is_empty()) {
                skipped += 1;
                continue;
            }
            person.updated_at = now;
            self.store.write_person(&person)?;
            if is_new {
                added += 1;
            } else {
                updated += 1;
            }
        }
        if added + updated > 0 {
            self.people_changed().await?;
        }
        Ok(json!({ "added": added, "updated": updated, "skipped": skipped }))
    }

    pub(crate) fn people_export_vcard(&self) -> Result<Value> {
        Ok(json!({ "text": export_vcards(&self.store.saved_people()?) }))
    }

    /// The address book as a vCard file in Downloads, next to whatever's there already.
    pub(crate) fn people_export_file(&self) -> Result<Value> {
        let text = export_vcards(&self.store.saved_people()?);
        let saved = self.save_to_downloads(EXPORT_NAME, text.as_bytes())?;
        Ok(json!({ "path": saved["path"], "name": saved["name"] }))
    }

    /// Copies the address book into the settings that travel, then tells other devices.
    async fn people_changed(&self) -> Result<()> {
        let mut wanted = BTreeMap::new();
        for copy in self.store.person_copies()? {
            wanted.insert(
                format!("{PERSON_PREFIX}{}", copy.uid),
                serde_json::to_string(&copy).unwrap_or_default(),
            );
        }
        let all = self.store.all_settings()?;
        for key in all.keys() {
            if key.starts_with(PERSON_PREFIX) && !wanted.contains_key(key) {
                self.store.delete_setting(key)?;
            }
        }
        for (key, value) in &wanted {
            if all.get(key) != Some(value) {
                self.store.set_setting(key, value)?;
            }
        }
        self.settings_changed().await
    }
}

/// For a person in both copies of the settings, whether theirs is the newer change.
/// None when it isn't a person, or both changed at the same moment.
pub(crate) fn newer_copy(key: &str, here: Option<&String>, there: Option<&String>) -> Option<bool> {
    if !key.starts_with(PERSON_PREFIX) {
        return None;
    }
    let at = |v: Option<&String>| {
        v.and_then(|v| serde_json::from_str::<PersonCopy>(v).ok())
            .map(|c| c.updated_at)
    };
    let (mine, theirs) = (at(here)?, at(there)?);
    (mine != theirs).then_some(theirs > mine)
}

/// Takes in kept people that arrived with the shared settings.
pub(crate) fn receive_from_settings(
    store: &Store,
    incoming: &BTreeMap<String, String>,
) -> Result<()> {
    let copies: Vec<PersonCopy> = incoming
        .iter()
        .filter(|(k, _)| k.starts_with(PERSON_PREFIX))
        .filter_map(|(_, v)| serde_json::from_str(v).ok())
        .collect();
    store.receive_people(&copies)
}

// ---- vCard ----

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Card {
    pub uid: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub company: String,
    pub notes: String,
}

/// Lines as written, with folded lines joined back up. Quoted-printable values that run
/// on with a soft break are joined too.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let continues_qp = lines.last().is_some_and(|l| {
            l.ends_with('=')
                && l.split_once(':')
                    .is_some_and(|(head, _)| head.to_ascii_uppercase().contains("QUOTED-PRINTABLE"))
        });
        if continues_qp {
            let last = lines.last_mut().unwrap();
            last.pop();
            last.push_str(raw.trim_start());
        } else if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&raw[1..]);
        } else if !raw.trim().is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
}

/// Splits at a separator that isn't escaped with a backslash.
fn split_unescaped(value: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in value.chars() {
        if escaped {
            parts.last_mut().unwrap().push('\\');
            parts.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn decode_qp(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads every card in a vCard 3.0 or 4.0 file.
pub fn parse_vcards(text: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut card: Option<Card> = None;
    let mut structured_name = String::new();
    for line in unfold(text) {
        // The name and its details end at the first colon outside quotes.
        let mut quoted = false;
        let Some(at) = line.char_indices().find_map(|(i, c)| {
            if c == '"' {
                quoted = !quoted;
            }
            (c == ':' && !quoted).then_some(i)
        }) else {
            continue;
        };
        let (head, value) = (&line[..at], &line[at + 1..]);
        let mut params = head.split(';');
        let prop = params.next().unwrap_or("");
        let prop = prop.rsplit('.').next().unwrap_or(prop).to_ascii_uppercase();
        let params: Vec<String> = params.map(|p| p.to_ascii_uppercase()).collect();
        let value = if params.iter().any(|p| p.contains("QUOTED-PRINTABLE")) {
            decode_qp(value)
        } else {
            value.to_string()
        };
        match prop.as_str() {
            "BEGIN" if value.trim().eq_ignore_ascii_case("VCARD") => {
                card = Some(Card::default());
                structured_name.clear();
            }
            "END" if value.trim().eq_ignore_ascii_case("VCARD") => {
                if let Some(mut c) = card.take() {
                    if c.name.trim().is_empty() {
                        c.name = structured_name.clone();
                    }
                    cards.push(c);
                }
            }
            _ => {
                let Some(c) = card.as_mut() else { continue };
                match prop.as_str() {
                    "FN" => c.name = unescape(value.trim()),
                    "N" => {
                        let parts: Vec<String> = split_unescaped(&value, ';')
                            .iter()
                            .map(|p| unescape(p).trim().to_string())
                            .collect();
                        let get = |i: usize| parts.get(i).cloned().unwrap_or_default();
                        structured_name = [get(3), get(1), get(2), get(0), get(4)]
                            .into_iter()
                            .filter(|p| !p.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                    }
                    "EMAIL" => {
                        let v = unescape(value.trim());
                        let v = v.strip_prefix("mailto:").unwrap_or(&v).trim().to_string();
                        c.emails.push(v);
                    }
                    "TEL" => {
                        let v = unescape(value.trim());
                        let v = v.strip_prefix("tel:").unwrap_or(&v).trim().to_string();
                        c.phones.push(v);
                    }
                    "ORG" => {
                        c.company = split_unescaped(&value, ';')
                            .first()
                            .map(|p| unescape(p).trim().to_string())
                            .unwrap_or_default();
                    }
                    "NOTE" => c.notes = unescape(&value),
                    "UID" => {
                        let v = unescape(value.trim());
                        c.uid = v.strip_prefix("urn:uuid:").unwrap_or(&v).to_string();
                    }
                    _ => {}
                }
            }
        }
    }
    cards
}

fn escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// Breaks a line so no piece is longer than 75 bytes, never inside a character.
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    let mut limit = 75;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > limit {
            out.push_str("\r\n ");
            width = 1;
            limit = 75;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

/// Everyone given, as a vCard 3.0 file.
pub fn export_vcards(people: &[Person]) -> String {
    let mut out = String::new();
    for p in people {
        let shown = p.shown_name().to_string();
        let words: Vec<&str> = p.name.split_whitespace().collect();
        let (given, family) = match words.split_last() {
            Some((last, rest)) if !rest.is_empty() => (rest.join(" "), last.to_string()),
            Some((only, _)) => (only.to_string(), String::new()),
            None => (String::new(), String::new()),
        };
        let mut lines = vec![
            "BEGIN:VCARD".to_string(),
            "VERSION:3.0".to_string(),
            format!("UID:{}", escape(&p.uid)),
            format!("FN:{}", escape(&shown)),
            format!("N:{};{};;;", escape(&family), escape(&given)),
        ];
        for (i, e) in p.emails.iter().enumerate() {
            let pref = if i == 0 { ",PREF" } else { "" };
            lines.push(format!("EMAIL;TYPE=INTERNET{pref}:{}", escape(e)));
        }
        for t in &p.phones {
            lines.push(format!("TEL:{}", escape(t)));
        }
        if !p.company.is_empty() {
            lines.push(format!("ORG:{}", escape(&p.company)));
        }
        if !p.notes.is_empty() {
            lines.push(format!("NOTE:{}", escape(&p.notes)));
        }
        lines.push("END:VCARD".to_string());
        for line in lines {
            fold(&line, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStore;
    use std::sync::Arc;

    fn device() -> Engine {
        Engine::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryStore::default()),
        )
    }

    /// What the vault does between two devices: merge both copies, apply the result here.
    fn hand_over(from: &Engine, to: &Engine) {
        let (there, there_at) = from.shared_settings().unwrap();
        let (here, here_at) = to.shared_settings().unwrap();
        let merged = to.merge_settings(&here, here_at, &there, there_at);
        to.apply_shared_settings(&merged, there_at.max(here_at))
            .unwrap();
    }

    fn input(id: Option<i64>, name: &str) -> PersonInput {
        PersonInput {
            id,
            name: name.into(),
            emails: vec!["ana@x.test".into()],
            phones: vec![],
            company: String::new(),
            notes: String::new(),
        }
    }

    #[tokio::test]
    async fn people_travel_between_devices_and_the_newest_change_wins() {
        let (a, b) = (device(), device());
        let saved = a.person_save(input(None, "Ana")).await.unwrap();
        hand_over(&a, &b);
        let on_b = b.store.person_with_address("ana@x.test").unwrap().unwrap();
        assert_eq!(on_b.name, "Ana");
        assert_eq!(on_b.uid, saved["uid"]);

        // Changed later on B: A takes it, and A's older copy can't undo it.
        let mut newer = on_b.clone();
        newer.name = "Ana Silva".into();
        newer.updated_at += 10;
        b.store.write_person(&newer).unwrap();
        b.people_changed().await.unwrap();
        a.store
            .receive_people(&[PersonCopy {
                uid: newer.uid.clone(),
                name: "Old".into(),
                emails: vec!["ana@x.test".into()],
                phones: vec![],
                company: String::new(),
                notes: String::new(),
                updated_at: newer.updated_at - 20,
                deleted: false,
            }])
            .unwrap();
        hand_over(&b, &a);
        let on_a = a.store.person_with_address("ana@x.test").unwrap().unwrap();
        assert_eq!(on_a.name, "Ana Silva");

        // A removal reaches the other device too.
        let mut gone = on_a.clone();
        gone.deleted = true;
        gone.updated_at += 10;
        a.store.write_person(&gone).unwrap();
        a.people_changed().await.unwrap();
        hand_over(&a, &b);
        assert!(b.store.person_with_address("ana@x.test").unwrap().is_none());
    }

    #[test]
    fn folding_keeps_lines_short_and_reads_back() {
        let mut p = Person::new("u1".into());
        p.name = "Ana Silva".into();
        p.emails = vec!["ana@x.test".into()];
        p.notes = "é".repeat(100) + ", with; a\nnew line";
        let text = export_vcards(&[p.clone()]);
        assert!(text.split("\r\n").all(|l| l.len() <= 75));
        let cards = parse_vcards(&text);
        assert_eq!(cards[0].notes, p.notes);
        assert_eq!(cards[0].name, "Ana Silva");
    }

    #[test]
    fn quoted_printable_and_structured_names() {
        let text = "BEGIN:VCARD\nVERSION:2.1\nN:Gon=C3=A7alves;Jo=\n=C3=A3o;;;\nNOTE;ENCODING=QUOTED-PRINTABLE:Ol=C3=A1\nEND:VCARD\n";
        let text = text.replace("N:Gon", "N;ENCODING=QUOTED-PRINTABLE:Gon");
        let cards = parse_vcards(&text);
        assert_eq!(cards[0].name, "João Gonçalves");
        assert_eq!(cards[0].notes, "Olá");
    }

    #[test]
    fn addresses_are_checked() {
        assert!(valid_email("ana@x.test"));
        assert!(!valid_email("ana"));
        assert!(!valid_email("ana@x"));
        assert!(!valid_email("a b@x.test"));
    }
}
