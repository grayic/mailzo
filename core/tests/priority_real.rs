//! Only runs when pointed at a copy of real mail, to measure how good the sorting is. It prints
//! counts, and with INBOX_REAL_SHOW also subjects, so it stays on the machine it runs on.

use mailcore::model::Addr;
use mailcore::priority::{decide, Facts, GOOD_TO_KNOW, NEEDS_YOU, THE_REST};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[test]
#[ignore]
fn measure_on_a_copy_of_real_mail() {
    let Ok(path) = std::env::var("INBOX_REAL_COPY") else {
        return;
    };
    let db = Connection::open(path).unwrap();
    let mine: BTreeSet<String> = db
        .prepare("SELECT lower(email) FROM accounts")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    // Who had been written to, and when, so a reply doesn't count as evidence for itself.
    let mut first_written: HashMap<String, i64> = HashMap::new();
    let mut replied: BTreeMap<String, i64> = BTreeMap::new();
    let mut sent = db
        .prepare(
            "SELECT m.to_json, m.date, m.thread_id FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE f.role = 'sent' ORDER BY m.date",
        )
        .unwrap();
    for row in sent
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
    {
        let (to, date, thread) = row.unwrap();
        let to: Vec<Addr> = serde_json::from_str(&to).unwrap_or_default();
        for a in to {
            first_written
                .entry(a.address.to_lowercase())
                .or_insert(date);
        }
        let e = replied.entry(thread).or_insert(date);
        *e = (*e).max(date);
    }
    // When each conversation was first written in by this person.
    let mut first_in_thread: HashMap<String, i64> = HashMap::new();
    let mut sent2 = db
        .prepare(
            "SELECT m.thread_id, MIN(m.date) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE f.role = 'sent' GROUP BY m.thread_id",
        )
        .unwrap();
    for row in sent2
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
    {
        let (t, d) = row.unwrap();
        first_in_thread.insert(t, d);
    }
    let days: i64 = std::env::var("INBOX_REAL_DAYS")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(365);
    let mut st = db
        .prepare(
            "SELECT m.from_addr, m.to_json, m.subject, COALESCE(b.text, m.snippet), m.category,
                    COALESCE(v.risk, 0), m.date, m.thread_id, m.auth
             FROM messages m JOIN folders f ON f.id = m.folder_id
             LEFT JOIN bodies b ON b.message_id = m.id
             LEFT JOIN verdicts v ON v.message_key = m.message_key
             WHERE f.role IN ('inbox', 'archive') AND m.date > strftime('%s', 'now') - ?1 * 86400
             ORDER BY m.date DESC",
        )
        .unwrap();
    let rows = st
        .query_map([days], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
            ))
        })
        .unwrap();
    let show = std::env::var("INBOX_REAL_SHOW").is_ok();
    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    let (mut answered, mut answered_found, mut needs, mut needs_answered) = (0, 0, 0, 0);
    for row in rows {
        let (from, to, subject, text, category, risk, date, thread, auth) = row.unwrap();
        let from_lower = from.to_lowercase();
        if mine.contains(&from_lower) {
            continue;
        }
        let to: Vec<Addr> = serde_json::from_str(&to).unwrap_or_default();
        let d = decide(&Facts {
            from: &from,
            to: &to,
            subject: &subject,
            text: &text,
            category: &category,
            risk,
            is_contact: first_written.get(&from_lower).is_some_and(|f| *f < date),
            answers_me: first_in_thread.get(&thread).is_some_and(|f| *f < date),
            chases: false,
            auth: &auth,
            mine: &mine,
            chosen: None,
        });
        *counts.entry(d.group).or_default() += 1;
        let was_answered = replied
            .get(&thread)
            .is_some_and(|r| *r > date && *r < date + 14 * 86400);
        if was_answered {
            answered += 1;
            if d.group == NEEDS_YOU {
                answered_found += 1;
            } else if show {
                println!("MISSED {} | {from} | {subject} | {category}", d.group);
            }
        }
        if d.group == NEEDS_YOU {
            needs += 1;
            if was_answered {
                needs_answered += 1;
            }
            if show {
                println!("NEEDS {} | {from} | {subject} | {:?}", was_answered, d.why);
            }
        }
    }
    println!(
        "needs_you {} good_to_know {} rest {}",
        counts.get(NEEDS_YOU).unwrap_or(&0),
        counts.get(GOOD_TO_KNOW).unwrap_or(&0),
        counts.get(THE_REST).unwrap_or(&0)
    );
    println!("emails you answered: {answered}, sorted into Needs you: {answered_found}");
    println!("in Needs you: {needs}, of which you answered: {needs_answered}");
}

#[test]
#[ignore]
fn the_upgrade_and_lists_are_quick_on_real_mail() {
    let Ok(path) = std::env::var("INBOX_REAL_COPY") else {
        return;
    };
    let copy = format!("{path}.upgrade");
    std::fs::copy(&path, &copy).unwrap();
    let t = std::time::Instant::now();
    let store = mailcore::Store::open(std::path::Path::new(&copy)).unwrap();
    println!("upgrade {:?}", t.elapsed());
    {
        let db = Connection::open(&copy).unwrap();
        db.execute(
            "UPDATE messages SET priority = 'needs_you', priority_why = 'test', priority_done = 1
             WHERE id IN (SELECT m.id FROM messages m JOIN folders f ON f.id = m.folder_id
                          WHERE f.role = 'inbox' ORDER BY m.date DESC LIMIT 40)",
            [],
        )
        .unwrap();
    }
    for (folder, _) in store.inbox_folder_ids().unwrap() {
        let t = std::time::Instant::now();
        let n = store.needs_you_count(folder, false).unwrap();
        let count_time = t.elapsed();
        let t = std::time::Instant::now();
        let list = store
            .list_threads_filtered(folder, 50, 0, true, Some("needs_you"), false)
            .unwrap();
        let list_time = t.elapsed();
        let t = std::time::Instant::now();
        let counts = store.tab_counts(folder, true).unwrap();
        println!("folder {folder}: count {n} in {count_time:?}, list {} in {list_time:?}, tabs {counts:?} in {:?}", list.len(), t.elapsed());
    }
    let t = std::time::Instant::now();
    let newsletters = store.newsletter_rows().unwrap().len();
    println!("newsletters {newsletters} in {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let later = store.list_marked("no_reply").unwrap().len();
    println!("no_reply {later} in {:?}", t.elapsed());
    std::fs::remove_file(&copy).unwrap();
}
