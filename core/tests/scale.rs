//! How the app holds up with a whole mailbox on it. Run by hand, as it takes a while:
//! cargo test --release -p mailcore --test scale -- --ignored --nocapture

use mailcore::model::{Addr, Security};
use mailcore::parse::ParsedHeaders;
use mailcore::search::SearchFilters;
use mailcore::store::IncomingMessage;
use mailcore::Store;
use std::time::Instant;

const MESSAGES: u32 = 100_000;
const BATCH: u32 = 1_000;

fn message(i: u32) -> IncomingMessage {
    // About one in three is a reply, in conversations of a few messages.
    let root = i - i % 3;
    let refs = if i.is_multiple_of(3) {
        vec![]
    } else {
        vec![format!("{root}@scale.test")]
    };
    IncomingMessage {
        uid: i + 1,
        is_draft: false,
        seen: !i.is_multiple_of(7),
        flagged: i.is_multiple_of(97),
        size: 10_000,
        internal_date: Some(1_500_000_000 + i64::from(i) * 300),
        signals: None,
        headers: ParsedHeaders {
            rfc_id: Some(format!("{i}@scale.test")),
            references: refs,
            subject: format!("Project update number {root} about invoices and travel"),
            from: Some(Addr {
                name: format!("Person {}", i % 500),
                address: format!("person{}@company{}.test", i % 500, i % 50),
            }),
            to: vec![Addr {
                name: "Me".into(),
                address: "me@scale.test".into(),
            }],
            ..Default::default()
        },
    }
}

fn timed<T>(what: &str, limit_ms: u128, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = run();
    let took = started.elapsed().as_millis();
    println!("{what}: {took} ms (limit {limit_ms})");
    assert!(took <= limit_ms, "{what} took {took} ms");
    out
}

#[test]
#[ignore = "takes a few minutes"]
fn a_hundred_thousand_emails_stay_quick() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mail.db")).unwrap();
    let account = store
        .insert_account("me@scale.test", "Me", "x.test", 993, Security::Tls, "me")
        .unwrap();
    let folders = store
        .replace_folders(
            account.id,
            &[
                ("INBOX".into(), "Inbox".into(), "inbox".into()),
                ("Sent".into(), "Sent".into(), "sent".into()),
            ],
        )
        .unwrap();
    let inbox = folders[0].0;

    let started = Instant::now();
    let mut i = 0;
    while i < MESSAGES {
        let batch: Vec<IncomingMessage> = (i..(i + BATCH).min(MESSAGES)).map(message).collect();
        store
            .insert_found_messages(account.id, inbox, &batch)
            .unwrap();
        i += BATCH;
    }
    println!(
        "saving {MESSAGES} emails: {} s",
        started.elapsed().as_secs()
    );

    timed("list the first page", 300, || {
        let list = store
            .list_threads_on_tab(inbox, 200, 0, true, None)
            .unwrap();
        assert_eq!(list.len(), 200);
    });
    timed("list a page far down", 300, || {
        store
            .list_threads_on_tab(inbox, 200, 20_000, true, None)
            .unwrap();
    });
    timed("list one tab", 300, || {
        store
            .list_threads_on_tab(inbox, 200, 0, true, Some("general"))
            .unwrap();
    });
    timed("emails one by one", 500, || {
        store
            .list_threads_on_tab(inbox, 200, 0, false, None)
            .unwrap();
    });
    timed("all inboxes", 300, || {
        store.list_all_on_tab("inbox", 200, 0, true, None).unwrap();
    });
    timed("unread on each tab", 300, || {
        store.tab_counts(inbox, true).unwrap()
    });
    timed("folder counts", 300, || {
        store.list_folders(account.id).unwrap()
    });
    timed("find a word", 300, || {
        let f = SearchFilters {
            words: vec!["invoices".into()],
            ..Default::default()
        };
        let (found, _) = store.search_threads(&f, &[], 200, true).unwrap();
        assert_eq!(found.len(), 200);
    });
    timed("find a sender", 500, || {
        let f = SearchFilters {
            from: vec!["person7@".into()],
            ..Default::default()
        };
        store.search_threads(&f, &[], 200, true).unwrap();
    });
    timed("people to suggest", 200, || {
        store.contacts(account.id, "pers", 8).unwrap();
    });
    timed("what mail to check", 100, || {
        store.mail_to_check(account.id, 50).unwrap();
    });
    timed("what the filters learn from", 100, || {
        store.junk_to_learn(account.id, 50).unwrap();
        store.good_to_learn(account.id, 50).unwrap();
    });
    timed("people written to", 100, || {
        store.note_contacts(account.id).unwrap()
    });
    timed("forgetting old opinions", 300, || {
        store.forget_old_verdicts().unwrap()
    });
    timed("everything on the server is still here", 100, || {
        store.local_uids(inbox).unwrap();
    });
    timed("the health page", 400, || {
        store.mail_health(5_000_000, 0).unwrap()
    });
    timed("open a conversation", 50, || {
        store
            .get_thread(account.id, "99999@scale.test", Some(inbox))
            .unwrap();
    });
}
