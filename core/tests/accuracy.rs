//! How well the filter does on a public collection of real emails. Not part of the
//! normal tests, since the collection isn't kept here. To run it, point it at a
//! folder holding `good/` and `spam/` folders of raw emails:
//! SPAM_CORPUS=/path cargo test -p mailcore --test accuracy -- --ignored --nocapture

use mailcore::parse::{parse_body, parse_headers};
use mailcore::spam::{check_mail, read_signals, spam_probability, words_of, Mail, SPAM_AT};
use mailcore::Store;
use std::collections::BTreeSet;
use std::path::Path;

struct Email {
    from_name: String,
    from_address: String,
    subject: String,
    text: String,
    html: String,
    auth: String,
    provider_spam: bool,
}

fn read_folder(dir: &Path) -> Vec<Email> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    names.sort();
    names
        .iter()
        .filter_map(|path| {
            let raw = std::fs::read(path).ok()?;
            let split = raw
                .windows(2)
                .position(|w| w == b"\n\n")
                .unwrap_or(raw.len());
            let head = &raw[..split];
            let headers = parse_headers(head);
            let from = headers.from.clone()?;
            let body = parse_body(&raw);
            let signals = read_signals(head, &from.address);
            Some(Email {
                from_name: from.name,
                from_address: from.address,
                subject: headers.subject,
                text: body.text.unwrap_or_default(),
                html: body.html.unwrap_or_default(),
                auth: signals.auth,
                // The collection's headers were added by SpamAssassin itself when it
                // was built, so reading them here would be cheating.
                provider_spam: false,
            })
        })
        .collect()
}

fn text_of(e: &Email) -> String {
    format!("{} {}", e.subject, e.text)
}

#[test]
#[ignore]
fn measure_on_a_public_collection() {
    let root = std::env::var("SPAM_CORPUS").expect("set SPAM_CORPUS");
    let root = Path::new(&root);
    let good = read_folder(&root.join("good"));
    let spam = read_folder(&root.join("spam"));
    println!("good emails: {}, spam emails: {}", good.len(), spam.len());
    let store = Store::open_in_memory().unwrap();
    // Learn from half, then judge the other half, which it has never seen.
    for (i, e) in good.iter().enumerate().filter(|(i, _)| i % 2 == 0) {
        store
            .learn(&format!("g{i}"), &words_of(&text_of(e)), false)
            .unwrap();
    }
    for (i, e) in spam.iter().enumerate().filter(|(i, _)| i % 2 == 0) {
        store
            .learn(&format!("s{i}"), &words_of(&text_of(e)), true)
            .unwrap();
    }
    let (ts, tg) = store.learned_totals().unwrap();
    let judge = |e: &Email, learning: bool| {
        let learned = learning.then(|| {
            let counts = store.word_counts(&words_of(&text_of(e))).unwrap();
            spam_probability(&counts, ts, tg)
        });
        let mail = Mail {
            from_name: &e.from_name,
            from_address: &e.from_address,
            reply_to: &[],
            subject: &e.subject,
            text: &e.text,
            html: &e.html,
            auth: &e.auth,
            provider_spam: e.provider_spam,
        };
        check_mail(&mail, &BTreeSet::new(), learned).0 >= SPAM_AT
    };
    let probability = |e: &Email| {
        let counts = store.word_counts(&words_of(&text_of(e))).unwrap();
        spam_probability(&counts, ts, tg)
    };
    for bar in [0.5, 0.9, 0.99, 0.999, 0.9999, 0.99999] {
        let bad = good
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|e| probability(e) >= bar)
            .count();
        let hit = spam
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|e| probability(e) >= bar)
            .count();
        println!(
            "learning alone at {bar}: caught {hit}/{} spam, wrongly flagged {bad}/{} good",
            spam.len() / 2,
            good.len() / 2
        );
    }
    for (name, learning) in [("rules only", false), ("rules and learning", true)] {
        let wrong_good: Vec<&Email> = good
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|e| judge(e, learning))
            .collect();
        let tested_good = good.len() / 2;
        let tested_spam = spam.len() / 2;
        let caught = spam
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|e| judge(e, learning))
            .count();
        println!(
            "{name}: caught {caught}/{tested_spam} spam ({:.1}%), wrongly flagged {}/{tested_good} good ({:.2}%)",
            caught as f64 * 100.0 / tested_spam.max(1) as f64,
            wrong_good.len(),
            wrong_good.len() as f64 * 100.0 / tested_good.max(1) as f64,
        );
        for e in wrong_good.iter().take(8) {
            println!("   wrongly flagged: {} | {}", e.from_address, e.subject);
        }
    }
}
