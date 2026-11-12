//! The local mail database. One SQLite file on the user's own Mac.

use crate::error::{CoreError, Result};
use crate::model::*;
use crate::parse::{self, ParsedHeaders};
use crate::search::SearchFilters;
use crate::secrets::SecretStore;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension, Row};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Each entry upgrades the database by one version. Never edit an old entry.
/// How far back new mail gets its quick replies written ahead: three days.
const QUICK_REPLIES_FOR_SECS: i64 = 3 * 86_400;

const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE accounts (
        id INTEGER PRIMARY KEY,
        email TEXT NOT NULL,
        display_name TEXT NOT NULL,
        imap_host TEXT NOT NULL,
        imap_port INTEGER NOT NULL,
        security TEXT NOT NULL,
        username TEXT NOT NULL,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        UNIQUE (email, imap_host)
    );
    CREATE TABLE folders (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        name TEXT NOT NULL,
        role TEXT NOT NULL,
        uid_validity INTEGER,
        last_uid INTEGER NOT NULL DEFAULT 0,
        last_synced_at INTEGER,
        UNIQUE (account_id, path)
    );
    CREATE TABLE messages (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        uid INTEGER NOT NULL,
        rfc_id TEXT,
        thread_id TEXT NOT NULL,
        subject TEXT NOT NULL,
        from_name TEXT NOT NULL,
        from_addr TEXT NOT NULL,
        to_json TEXT NOT NULL,
        cc_json TEXT NOT NULL,
        date INTEGER NOT NULL,
        seen INTEGER NOT NULL,
        flagged INTEGER NOT NULL,
        size INTEGER NOT NULL,
        snippet TEXT NOT NULL DEFAULT '',
        UNIQUE (folder_id, uid)
    );
    CREATE INDEX messages_folder_date ON messages (folder_id, date DESC);
    CREATE INDEX messages_thread ON messages (account_id, thread_id);
    CREATE INDEX messages_rfc ON messages (account_id, rfc_id);
    CREATE TABLE bodies (
        message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        text TEXT,
        html TEXT,
        attachments_json TEXT NOT NULL,
        fetched_at INTEGER NOT NULL DEFAULT (unixepoch())
    );
"#,
    r#"
    ALTER TABLE accounts ADD COLUMN sync_id TEXT;
    UPDATE accounts SET sync_id = lower(hex(randomblob(16)));
    CREATE UNIQUE INDEX accounts_sync_id ON accounts (sync_id);
    CREATE TABLE identity (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        email TEXT NOT NULL,
        device_id INTEGER NOT NULL,
        device_name TEXT NOT NULL,
        state TEXT NOT NULL,
        vault_version INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE tombstones (
        sync_id TEXT PRIMARY KEY,
        removed_at INTEGER NOT NULL DEFAULT (unixepoch())
    );
    CREATE TABLE link_nonces (
        device_id INTEGER PRIMARY KEY,
        nonce TEXT NOT NULL
    );
"#,
    r#"
    ALTER TABLE accounts ADD COLUMN smtp_host TEXT NOT NULL DEFAULT '';
    ALTER TABLE accounts ADD COLUMN smtp_port INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE accounts ADD COLUMN smtp_security TEXT NOT NULL DEFAULT 'tls';
    ALTER TABLE accounts ADD COLUMN signature TEXT NOT NULL DEFAULT '';
    ALTER TABLE accounts ADD COLUMN settings_updated_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN reply_to_json TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE messages ADD COLUMN bcc_json TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE messages ADD COLUMN base_subject TEXT;
    CREATE INDEX messages_base_subject ON messages (account_id, base_subject);
    CREATE TABLE drafts (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        mode TEXT NOT NULL,
        parent_message_id INTEGER,
        to_json TEXT NOT NULL DEFAULT '[]',
        cc_json TEXT NOT NULL DEFAULT '[]',
        bcc_json TEXT NOT NULL DEFAULT '[]',
        subject TEXT NOT NULL DEFAULT '',
        html TEXT NOT NULL DEFAULT '',
        text TEXT NOT NULL DEFAULT '',
        quoted_html TEXT NOT NULL DEFAULT '',
        quoted_text TEXT NOT NULL DEFAULT '',
        attachments_json TEXT NOT NULL DEFAULT '[]',
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())
    );
"#,
    r#"
    CREATE TABLE avatars (
        key TEXT PRIMARY KEY,
        data TEXT,
        fetched_at INTEGER NOT NULL
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
"#,
    r#"
    ALTER TABLE accounts ADD COLUMN prefs_json TEXT NOT NULL DEFAULT '{}';
    CREATE TABLE images_held (message_key TEXT PRIMARY KEY);
    ALTER TABLE drafts ADD COLUMN from_address TEXT NOT NULL DEFAULT '';
"#,
    r#"
    CREATE VIRTUAL TABLE search_index USING fts5(
        subject, people, body,
        prefix = '2 3',
        tokenize = 'unicode61 remove_diacritics 2 categories ''L* N* Co Mn Mc'''
    );
    INSERT INTO search_index (rowid, subject, people, body)
        SELECT m.id, m.subject, m.from_name || ' ' || m.from_addr || ' ' || m.to_json || ' ' || m.cc_json,
               COALESCE(b.text, m.snippet)
        FROM messages m LEFT JOIN bodies b ON b.message_id = m.id;
    CREATE TRIGGER search_message_added AFTER INSERT ON messages BEGIN
        INSERT INTO search_index (rowid, subject, people, body)
        VALUES (new.id, new.subject,
                new.from_name || ' ' || new.from_addr || ' ' || new.to_json || ' ' || new.cc_json, '');
    END;
    CREATE TRIGGER search_message_gone AFTER DELETE ON messages BEGIN
        DELETE FROM search_index WHERE rowid = old.id;
    END;
    CREATE TRIGGER search_body_added AFTER INSERT ON bodies BEGIN
        UPDATE search_index SET body = COALESCE(new.text, '') WHERE rowid = new.message_id;
    END;
"#,
    r#"
    ALTER TABLE messages ADD COLUMN category TEXT NOT NULL DEFAULT 'people';
    ALTER TABLE messages ADD COLUMN unsubscribe TEXT;
    ALTER TABLE messages ADD COLUMN unsubscribe_one_click INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN auth TEXT NOT NULL DEFAULT 'none';
    ALTER TABLE messages ADD COLUMN provider_spam INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN signals_done INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN ai_label TEXT;
    ALTER TABLE messages ADD COLUMN ai_checked INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX messages_signals ON messages (folder_id, signals_done);
    CREATE TABLE verdicts (
        message_key TEXT PRIMARY KEY,
        score INTEGER NOT NULL,
        risk INTEGER NOT NULL,
        reasons_json TEXT NOT NULL,
        auto_moved INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE contacts (address TEXT PRIMARY KEY);
    CREATE TABLE spam_words (
        word TEXT PRIMARY KEY,
        spam INTEGER NOT NULL DEFAULT 0,
        good INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE spam_trained (message_key TEXT PRIMARY KEY, spam INTEGER NOT NULL);
"#,
    r#"
    UPDATE messages SET signals_done = 0;
    DELETE FROM verdicts WHERE risk = 0 AND auto_moved = 0;
"#,
    r#"
    ALTER TABLE accounts ADD COLUMN auth TEXT NOT NULL DEFAULT 'password';
"#,
    r#"
    UPDATE messages SET category = 'general' WHERE category IN ('people', 'notifications');
    UPDATE settings SET value = 'general'
        WHERE key LIKE 'sort:%' AND value IN ('people', 'notifications');
    UPDATE messages SET signals_done = 0;
    DELETE FROM verdicts WHERE risk = 0 AND auto_moved = 0;
"#,
    r#"
    CREATE TABLE team_info (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        team_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        me INTEGER NOT NULL,
        role TEXT NOT NULL,
        members_version INTEGER NOT NULL DEFAULT 0,
        vault_seen INTEGER NOT NULL DEFAULT 0,
        seq INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE team_people (
        user_id INTEGER PRIMARY KEY,
        email TEXT NOT NULL,
        role TEXT NOT NULL
    );
    CREATE TABLE team_threads (
        thread_key TEXT PRIMARY KEY,
        assignee INTEGER,
        status TEXT NOT NULL,
        status_at INTEGER NOT NULL,
        sent_count INTEGER NOT NULL,
        last_sent_by INTEGER,
        last_sent_at INTEGER NOT NULL,
        sending_by INTEGER,
        sending_until INTEGER NOT NULL DEFAULT 0,
        seq INTEGER NOT NULL
    );
    CREATE TABLE team_comments (
        id TEXT PRIMARY KEY,
        thread_key TEXT NOT NULL,
        author INTEGER NOT NULL,
        text TEXT,
        created_at INTEGER NOT NULL,
        seq INTEGER NOT NULL
    );
    CREATE INDEX team_comments_thread ON team_comments (thread_key);
    CREATE TABLE team_templates (
        id TEXT PRIMARY KEY,
        name TEXT,
        body TEXT,
        version INTEGER NOT NULL,
        seq INTEGER NOT NULL
    );
    CREATE TABLE draft_team_base (
        draft_id INTEGER PRIMARY KEY,
        thread_key TEXT NOT NULL,
        sent_count INTEGER NOT NULL
    );
"#,
    r#"
    ALTER TABLE folders ADD COLUMN server_total INTEGER;
    ALTER TABLE folders ADD COLUMN uid_next INTEGER;
    ALTER TABLE folders ADD COLUMN modseq INTEGER;
    ALTER TABLE folders ADD COLUMN last_error TEXT;
    ALTER TABLE folders ADD COLUMN flags_checked_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN old_mail INTEGER NOT NULL DEFAULT 2;
    ALTER TABLE messages ADD COLUMN body_tries INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN is_draft INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE rule_seen (message_key TEXT PRIMARY KEY);
    ALTER TABLE messages ADD COLUMN screened INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN rule_done INTEGER NOT NULL DEFAULT 1;
    CREATE INDEX messages_folder_thread ON messages (folder_id, thread_id);
    CREATE INDEX messages_from_addr ON messages (from_addr COLLATE NOCASE);
    CREATE TABLE draft_leftovers (
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        rfc_id TEXT NOT NULL,
        PRIMARY KEY (account_id, rfc_id)
    );
    CREATE TABLE sync_status (
        account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
        last_try INTEGER,
        last_ok INTEGER,
        last_error TEXT
    );
    CREATE TABLE thread_marks (
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        thread_id TEXT NOT NULL,
        pinned INTEGER NOT NULL DEFAULT 0,
        muted INTEGER NOT NULL DEFAULT 0,
        snooze_until INTEGER,
        snoozed_at INTEGER,
        returned_at INTEGER,
        follow_at INTEGER,
        follow_set_at INTEGER,
        follow_due INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (account_id, thread_id)
    );
    ALTER TABLE drafts ADD COLUMN send_at INTEGER;
    ALTER TABLE drafts ADD COLUMN send_problem TEXT;
    ALTER TABLE drafts ADD COLUMN server_key TEXT;
    ALTER TABLE drafts ADD COLUMN server_saved_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE drafts ADD COLUMN server_rfc_id TEXT;
    ALTER TABLE drafts ADD COLUMN remind_days INTEGER;
    ALTER TABLE drafts ADD COLUMN made_by_ai INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE tracker_counts (company TEXT PRIMARY KEY, seen INTEGER NOT NULL);
    CREATE TABLE tracked_mail (message_key TEXT PRIMARY KEY);
    CREATE TABLE meanings (
        message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        vector BLOB NOT NULL
    );
    CREATE TABLE ai_usage (
        day TEXT PRIMARY KEY,
        input INTEGER NOT NULL DEFAULT 0,
        output INTEGER NOT NULL DEFAULT 0,
        calls INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE calendars (
        id INTEGER PRIMARY KEY,
        account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        url TEXT,
        name TEXT NOT NULL,
        hue INTEGER NOT NULL DEFAULT 230,
        problem TEXT,
        UNIQUE (account_id, kind, url)
    );
    CREATE TABLE events (
        id INTEGER PRIMARY KEY,
        calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
        uid TEXT NOT NULL,
        href TEXT,
        etag TEXT,
        title TEXT NOT NULL,
        location TEXT NOT NULL DEFAULT '',
        notes TEXT NOT NULL DEFAULT '',
        starts INTEGER NOT NULL,
        ends INTEGER NOT NULL,
        all_day INTEGER NOT NULL DEFAULT 0,
        rrule TEXT,
        exdates_json TEXT NOT NULL DEFAULT '[]',
        organizer TEXT,
        attendees_json TEXT NOT NULL DEFAULT '[]',
        my_status TEXT NOT NULL DEFAULT '',
        sequence INTEGER NOT NULL DEFAULT 0,
        tz TEXT,
        raw TEXT,
        is_override INTEGER NOT NULL DEFAULT 0,
        UNIQUE (calendar_id, uid)
    );
    CREATE INDEX events_starts ON events (starts);
    CREATE TABLE event_alerts (
        event_id INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
        starts INTEGER NOT NULL,
        PRIMARY KEY (event_id, starts)
    );
    CREATE TABLE ai_summaries (
        thread_key TEXT PRIMARY KEY,
        through INTEGER NOT NULL,
        text TEXT NOT NULL
    );
"#,
    // The spam check changed, so what it decided by itself is decided again. What the
    // person chose, and what was already moved, stays.
    r#"
    DELETE FROM verdicts WHERE auto_moved = 0 AND score < 100
      AND (risk = 2 OR reasons_json LIKE '%signature doesn''t match%');
"#,
    // What needs the person, worked out for recent mail as it's checked.
    r#"
    ALTER TABLE messages ADD COLUMN priority TEXT;
    ALTER TABLE messages ADD COLUMN priority_why TEXT;
    ALTER TABLE messages ADD COLUMN priority_unsure INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN priority_done INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN ai_line TEXT;
    ALTER TABLE messages ADD COLUMN due_at INTEGER;
    ALTER TABLE thread_marks ADD COLUMN priority TEXT;
    CREATE INDEX messages_priority_todo ON messages(account_id, date) WHERE priority_done = 0;
    CREATE INDEX messages_priority ON messages(folder_id, date) WHERE priority = 'needs_you';
    CREATE INDEX messages_due ON messages(due_at) WHERE due_at IS NOT NULL;
"#,
    // Worked out per request, since one month can mix quick and balanced models.
    r#"
    ALTER TABLE ai_usage ADD COLUMN dollars REAL NOT NULL DEFAULT 0;
    UPDATE ai_usage SET dollars = input / 1000000.0 * 0.10 + output / 1000000.0 * 0.50;
"#,
    // A person can be in several teams, and can have a different role in each.
    r#"
    UPDATE accounts SET prefs_json = json_set(prefs_json, '$.team_id', (SELECT team_id FROM team_info))
        WHERE json_extract(prefs_json, '$.team_shared') = 1 AND EXISTS (SELECT 1 FROM team_info);

    CREATE TABLE team_people_by_team (
        team_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        email TEXT NOT NULL,
        role TEXT NOT NULL,
        PRIMARY KEY (team_id, user_id)
    );
    INSERT INTO team_people_by_team (team_id, user_id, email, role)
        SELECT (SELECT team_id FROM team_info), user_id, email, role FROM team_people
        WHERE EXISTS (SELECT 1 FROM team_info) ORDER BY rowid;
    DROP TABLE team_people;
    ALTER TABLE team_people_by_team RENAME TO team_people;

    ALTER TABLE team_threads ADD COLUMN team_id INTEGER NOT NULL DEFAULT 0;
    UPDATE team_threads SET team_id = COALESCE((SELECT team_id FROM team_info), 0);
    ALTER TABLE team_comments ADD COLUMN team_id INTEGER NOT NULL DEFAULT 0;
    UPDATE team_comments SET team_id = COALESCE((SELECT team_id FROM team_info), 0);
    ALTER TABLE draft_team_base ADD COLUMN team_id INTEGER NOT NULL DEFAULT 0;
    UPDATE draft_team_base SET team_id = COALESCE((SELECT team_id FROM team_info), 0);

    CREATE TABLE team_templates_by_team (
        team_id INTEGER NOT NULL,
        id TEXT NOT NULL,
        name TEXT,
        body TEXT,
        version INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        PRIMARY KEY (team_id, id)
    );
    INSERT INTO team_templates_by_team (team_id, id, name, body, version, seq)
        SELECT (SELECT team_id FROM team_info), id, name, body, version, seq FROM team_templates
        WHERE EXISTS (SELECT 1 FROM team_info);
    DROP TABLE team_templates;
    ALTER TABLE team_templates_by_team RENAME TO team_templates;

    CREATE TABLE team_info_by_team (
        team_id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        me INTEGER NOT NULL,
        role TEXT NOT NULL,
        members_version INTEGER NOT NULL DEFAULT 0,
        vault_seen INTEGER NOT NULL DEFAULT 0,
        seq INTEGER NOT NULL DEFAULT 0
    );
    INSERT INTO team_info_by_team (team_id, name, me, role, members_version, vault_seen, seq)
        SELECT team_id, name, me, role, members_version, vault_seen, seq FROM team_info;
    DROP TABLE team_info;
    ALTER TABLE team_info_by_team RENAME TO team_info;
"#,
    // The person's own tags, with a colour, and an amount on each conversation if wanted.
    r#"
    CREATE TABLE tags (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        colour TEXT NOT NULL
    );
    CREATE TABLE thread_tags (
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        thread_id TEXT NOT NULL,
        tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        amount REAL,
        tagged_at INTEGER NOT NULL,
        PRIMARY KEY (account_id, thread_id, tag_id)
    );
    CREATE INDEX thread_tags_by_tag ON thread_tags (tag_id, tagged_at);
"#,
    // How the person writes, in a few lines they can read and change.
    r#"
    CREATE TABLE style_profiles (
        account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
        text TEXT NOT NULL,
        made_at INTEGER NOT NULL,
        edited INTEGER NOT NULL DEFAULT 0
    );
"#,
    // Follow-ups that send themselves until someone replies.
    r#"
    CREATE TABLE sequences (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        steps_json TEXT NOT NULL
    );
    CREATE TABLE sequence_runs (
        id INTEGER PRIMARY KEY,
        sequence_id INTEGER REFERENCES sequences(id) ON DELETE SET NULL,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        thread_id TEXT NOT NULL,
        message_id INTEGER NOT NULL,
        started_at INTEGER NOT NULL,
        seen_upto INTEGER NOT NULL,
        sent INTEGER NOT NULL DEFAULT 0,
        next_at INTEGER,
        state TEXT NOT NULL,
        problem TEXT
    );
    CREATE INDEX sequence_runs_thread ON sequence_runs (account_id, thread_id);
"#,
    // What each AI request cost, so a monthly budget can be kept to the cent.
    r#"
    CREATE TABLE ai_requests (
        id INTEGER PRIMARY KEY,
        at INTEGER NOT NULL,
        day TEXT NOT NULL,
        model TEXT NOT NULL,
        input INTEGER NOT NULL,
        output INTEGER NOT NULL,
        cache_read INTEGER NOT NULL DEFAULT 0,
        cache_write INTEGER NOT NULL DEFAULT 0,
        dollars REAL NOT NULL,
        included INTEGER NOT NULL
    );
    CREATE INDEX ai_requests_day ON ai_requests (included, day);
"#,
    // Follow-ups: only one set running per conversation, and a step is claimed just before
    // it goes, so two checks running at once can't both send it. Any conversation that
    // already had two running keeps the first, so the new rule can be added.
    r#"
    UPDATE sequence_runs SET state = 'stopped', next_at = NULL
     WHERE state = 'running' AND id NOT IN (
         SELECT MIN(id) FROM sequence_runs WHERE state = 'running' GROUP BY account_id, thread_id
     );
    ALTER TABLE sequence_runs ADD COLUMN sending_at INTEGER;
    CREATE UNIQUE INDEX sequence_runs_one_running ON sequence_runs (account_id, thread_id)
        WHERE state = 'running';
"#,
    // A draft that has gone is marked so before it's cleared away, so if clearing it fails
    // it can never be sent again. Follow-ups count a reply from the time of the email they
    // follow, so one that came in just before they started still stops them.
    r#"
    ALTER TABLE drafts ADD COLUMN sent_at INTEGER;
    ALTER TABLE sequence_runs ADD COLUMN reply_after INTEGER;
    UPDATE sequence_runs SET reply_after = (SELECT date FROM messages WHERE id = sequence_runs.message_id);
"#,
    // Repairs to saved mail that run once, by name.
    "CREATE TABLE IF NOT EXISTS repairs (name TEXT PRIMARY KEY);",
    // Encrypted email. Other people's public keys, what an email's encryption or signature
    // came to, and whether a draft goes out encrypted or signed. Bodies saved before this
    // showed encrypted mail as it arrived, so those are read again.
    r#"
    CREATE TABLE pgp_keys (
        address TEXT PRIMARY KEY,
        armored TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        source TEXT NOT NULL,
        prefer_encrypt INTEGER NOT NULL DEFAULT 0,
        seen_at INTEGER NOT NULL
    );
    ALTER TABLE bodies ADD COLUMN pgp_json TEXT;
    ALTER TABLE drafts ADD COLUMN pgp_encrypt INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE drafts ADD COLUMN pgp_sign INTEGER NOT NULL DEFAULT 0;
    DELETE FROM bodies WHERE attachments_json LIKE '%application/pgp-encrypted%'
        OR attachments_json LIKE '%application/pgp-signature%'
        OR text LIKE '%-----BEGIN PGP MESSAGE-----%';
"#,
    // The address book: people the person chose to keep. Removed ones stay as a mark,
    // so a removal reaches their other devices too.
    r#"
    CREATE TABLE people (
        id INTEGER PRIMARY KEY,
        uid TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL DEFAULT '',
        emails_json TEXT NOT NULL DEFAULT '[]',
        phones_json TEXT NOT NULL DEFAULT '[]',
        company TEXT NOT NULL DEFAULT '',
        notes TEXT NOT NULL DEFAULT '',
        updated_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0
    );
"#,
    // What still needs its meaning worked out, so finding it doesn't mean going through all the mail.
    r#"
    CREATE TABLE meaning_todo (
        message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE
    );
    INSERT INTO meaning_todo
        SELECT b.message_id FROM bodies b
        LEFT JOIN meanings x ON x.message_id = b.message_id
        WHERE x.message_id IS NULL;
    CREATE TRIGGER meaning_wanted AFTER INSERT ON bodies BEGIN
        INSERT OR IGNORE INTO meaning_todo (message_id) VALUES (new.message_id);
    END;
"#,
    // A key offered by a later email is kept beside the one already saved, never put in
    // its place without the person saying so. Saved signed bodies are read again, so
    // ones signed with a key that only came from email stop showing as verified.
    r#"
    ALTER TABLE pgp_keys ADD COLUMN offered_armored TEXT;
    ALTER TABLE pgp_keys ADD COLUMN offered_fingerprint TEXT;
    DELETE FROM bodies WHERE pgp_json IS NOT NULL;
"#,
    // A reply to or forward of an encrypted email, and whether the person agreed to send
    // it unencrypted anyway.
    r#"
    ALTER TABLE drafts ADD COLUMN from_encrypted INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE drafts ADD COLUMN plain_ok INTEGER NOT NULL DEFAULT 0;
"#,
    // What connected AI apps have done, for the person to check.
    r#"
    CREATE TABLE agent_activity (
        id INTEGER PRIMARY KEY,
        at INTEGER NOT NULL,
        app TEXT NOT NULL,
        tool TEXT NOT NULL,
        kind TEXT NOT NULL,
        outcome TEXT NOT NULL,
        problem TEXT,
        account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
        thread_id TEXT,
        folder_id INTEGER,
        subject TEXT,
        who TEXT,
        detail TEXT NOT NULL
    );
    CREATE INDEX agent_activity_at ON agent_activity(at);
"#,
    // Which email the filter's opinion belongs to. The sender chooses the Message-ID, sender
    // and subject, so a scam could copy all three from an email already judged clean. The
    // arrival time comes from the server, and a move or a copy under another label keeps it.
    // ponytail: two emails with the same headers arriving in the same second share a key, so a
    // scam judged in a later refresh than its twin inherits the verdict. Adding a hash of the
    // body would close that, but bodies aren't always here when the headers are.
    r#"
    ALTER TABLE messages ADD COLUMN message_key TEXT GENERATED ALWAYS AS
        (COALESCE(rfc_id, '#' || id) || char(31) || from_addr || char(31) || subject || char(31) || date)
        VIRTUAL;
    DELETE FROM verdicts;
"#,
    // Finds every sender with a way out, for Newsletters, without reading every email.
    r#"
    CREATE INDEX messages_newsletters
        ON messages (account_id, from_addr COLLATE NOCASE, date, thread_id, folder_id)
        WHERE unsubscribe IS NOT NULL AND screened = 0;
"#,
    // Follow-ups set up from inside a conversation keep their own steps, with no saved set.
    r#"
    ALTER TABLE sequence_runs ADD COLUMN steps_json TEXT;
"#,
    // Facts an email's own markup states (flights, stays, orders, invoices, parcels), so
    // questions about them are answered from the email itself.
    r#"
    CREATE TABLE facts (
        message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        starts_at INTEGER,
        place TEXT NOT NULL,
        value_json TEXT NOT NULL
    );
    CREATE INDEX facts_kind_time ON facts (kind, starts_at);
    CREATE INDEX facts_message ON facts (message_id);
"#,
    // The inbox an invitation came to, so an event kept on this Mac's own calendar still
    // follows that inbox's AI setting.
    r#"
    ALTER TABLE events ADD COLUMN from_account_id INTEGER;
"#,
    // Whether an email's body is saved here, kept by the database itself, and indexes that
    // hold what the refresh and the sidebar count: they read those instead of every email
    // and its body, which on a big encrypted mailbox kept the Mac busy all day.
    r#"
    ALTER TABLE messages ADD COLUMN has_body INTEGER NOT NULL DEFAULT 0;
    UPDATE messages SET has_body = 1 WHERE id IN (SELECT message_id FROM bodies);
    CREATE TRIGGER body_saved AFTER INSERT ON bodies BEGIN
        UPDATE messages SET has_body = 1 WHERE id = NEW.message_id;
    END;
    CREATE TRIGGER body_forgotten AFTER DELETE ON bodies BEGIN
        UPDATE messages SET has_body = 0 WHERE id = OLD.message_id;
    END;
    CREATE INDEX messages_bodies ON messages (folder_id, has_body, date, size, body_tries);
    CREATE INDEX messages_counts ON messages (folder_id, seen, screened, thread_id, rfc_id);
"#,
    // What each refresh looks for to check, learn from, sort and run rules on: the few emails
    // still to do, found from these instead of reading every email of the inbox.
    r#"
    CREATE INDEX messages_keys
        ON messages (account_id, date, folder_id, message_key, signals_done, old_mail);
    CREATE INDEX messages_rules_due ON messages (account_id, date) WHERE rule_done = 0;
    CREATE INDEX messages_mass ON messages (account_id) WHERE category = 'mass';
"#,
    // Everything a list groups, sorts and sorts into tabs by, so lists and their counts are read
    // from here and never from the emails' own rows.
    r#"
    CREATE INDEX messages_list ON messages (account_id, thread_id, folder_id, date, is_draft,
        screened, seen, category, priority, rfc_id);
"#,
    // Whether an email has files to open, kept with it, so a list row can show its paperclip
    // without reading the email's whole body, which is stored ahead of its file list.
    r#"
    ALTER TABLE messages ADD COLUMN has_files INTEGER NOT NULL DEFAULT 0;
    UPDATE messages SET has_files = 1 WHERE id IN (SELECT message_id FROM bodies
        WHERE attachments_json LIKE '%"inline":false%'
           OR (attachments_json != '[]' AND attachments_json NOT LIKE '%"inline":%'));
    DROP TRIGGER body_saved;
    CREATE TRIGGER body_saved AFTER INSERT ON bodies BEGIN
        UPDATE messages SET has_body = 1,
            has_files = (NEW.attachments_json LIKE '%"inline":false%'
                OR (NEW.attachments_json != '[]' AND NEW.attachments_json NOT LIKE '%"inline":%'))
        WHERE id = NEW.message_id;
    END;
    DROP TRIGGER body_forgotten;
    CREATE TRIGGER body_forgotten AFTER DELETE ON bodies BEGIN
        UPDATE messages SET has_body = 0, has_files = 0 WHERE id = OLD.message_id;
    END;
"#,
    // Receipts, orders, bills and payments, for the Purchases list. Left empty until
    // looked at, so mail already here is sorted in the background a batch at a time.
    r#"
    ALTER TABLE messages ADD COLUMN purchase INTEGER;
    CREATE INDEX messages_purchase_todo ON messages (id) WHERE purchase IS NULL;
    CREATE INDEX messages_purchases ON messages (account_id, thread_id) WHERE purchase = 1;
"#,
    // New mail waits out of sight while filters look at it, until this time on this Mac.
    // A tab a filter chose is kept when the spam checks sort the email afterwards.
    r#"
    ALTER TABLE messages ADD COLUMN held_until INTEGER;
    ALTER TABLE messages ADD COLUMN tab_chosen INTEGER;
    CREATE INDEX messages_held ON messages (held_until) WHERE rule_done = 0;
"#,
    // Bank alerts and receipts were called "Asks you something" for the questions in
    // their small print. Recent ones are looked at again with that fixed.
    r#"
    UPDATE messages SET priority_done = 0
    WHERE priority_why = 'Asks you something' AND date > CAST(strftime('%s', 'now') AS INTEGER) - 2592000;
"#,
    // Cold sales emails and bank transfer alerts are now told apart from real questions.
    // Recent ones that were called "needs you" by a guess are looked at again.
    r#"
    UPDATE messages SET priority_done = 0
    WHERE priority = 'needs_you'
      AND (priority_why IS NULL OR priority_why IN ('Asks you something', 'AI thinks this needs you'))
      AND date > CAST(strftime('%s', 'now') AS INTEGER) - 2592000;
"#,
    // Wallet and fund transfers now count as purchases, so recent mail is looked at again,
    // and recent "needs you" guesses get a second look now that the lists hear about it.
    r#"
    UPDATE messages SET purchase = NULL
    WHERE purchase = 0 AND date > CAST(strftime('%s', 'now') AS INTEGER) - 7776000;
    UPDATE messages SET priority_done = 0
    WHERE priority = 'needs_you'
      AND (priority_why IS NULL OR priority_why IN ('Asks you something', 'AI thinks this needs you'))
      AND date > CAST(strftime('%s', 'now') AS INTEGER) - 2592000;
"#,
    // Sign-in codes and links, mailings from people written to and more sales pitches are
    // told apart now. Recent mail that was sorted into Needs you is looked at again.
    r#"
    UPDATE messages SET priority_done = 0
    WHERE priority = 'needs_you'
      AND (priority_why IS NULL
           OR priority_why IN ('Asks you something', 'AI thinks this needs you', 'May need action',
                               'You''ve written to them before'))
      AND date > CAST(strftime('%s', 'now') AS INTEGER) - 2592000;
"#,
    // Quick replies are written once, when an email comes in, and kept, so opening it
    // again shows them at once instead of asking the AI each time.
    r#"
    CREATE TABLE quick_replies (
        message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        replies_json TEXT NOT NULL
    );
"#,
    // "Doesn't need me" covers the emails there were when it was chosen. A newer one that
    // needs the person brings the conversation back. Choices already made cover what's there now.
    r#"
    ALTER TABLE thread_marks ADD COLUMN priority_upto INTEGER;
    UPDATE thread_marks SET priority_upto = COALESCE(
        (SELECT MAX(m.date) FROM messages m
          WHERE m.account_id = thread_marks.account_id AND m.thread_id = thread_marks.thread_id), 0)
      WHERE priority IS NOT NULL;
"#,
];

const ACCOUNT_COLUMNS: &str =
    "id, email, display_name, imap_host, imap_port, security, username, sync_id,
     smtp_host, smtp_port, smtp_security, signature, settings_updated_at, prefs_json, auth";

const THREAD_WINDOW_SECONDS: i64 = 30 * 86_400;
/// A reply comes after what it answers, give or take a mail server's clock.
const CLOCK_SKEW_SECONDS: i64 = 3_600;

const READERS: usize = 4;
/// How long a read mark the server has heard is still kept from a refresh: one that read the
/// server just before it heard can land a moment later.
const HELD_AFTER_HEARD: Duration = Duration::from_secs(60);
/// A server that hasn't answered by now isn't waited for any longer.
const HELD_WAITING_MOST: Duration = Duration::from_secs(300);

/// Unused pages, of 4 KB, before giving them back to the Mac is worth rewriting the file: 40 MB.
const ROOM_WORTH_GIVING_BACK: i64 = 10_000;

pub struct Store {
    /// Shared, so giving room back to the Mac can carry on in its own thread.
    conn: Arc<Mutex<Connection>>,
    /// Let reads run at the same time as each other and as new mail being saved. With only the one
    /// connection, a search queues behind every other question the app is asking.
    readers: Vec<Mutex<Connection>>,
    /// Read marks changed here that a refresh mustn't put back, by folder and UID: one that read
    /// the server before it heard would. When each was changed or heard, and whether it's heard.
    seen_held: Mutex<HashMap<(i64, u32), (Instant, bool)>>,
}

/// One message as it arrives from the server.
pub struct IncomingMessage {
    pub uid: u32,
    /// Marked as a draft on the server, whichever folder this copy is in.
    pub is_draft: bool,
    pub seen: bool,
    pub flagged: bool,
    pub size: u32,
    pub internal_date: Option<i64>,
    pub headers: ParsedHeaders,
    pub signals: Option<crate::spam::Signals>,
}

/// One email, as the filters need to see it.
#[derive(Debug, Clone)]
pub struct MailToCheck {
    /// 0 for new mail, 1 for older mail coming down, 2 for a folder's first look.
    pub old_mail: i64,
    pub id: i64,
    pub key: String,
    pub folder_id: i64,
    pub folder_path: String,
    pub uid: u32,
    pub from_name: String,
    pub from_address: String,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub text: String,
    pub html: String,
    pub auth: String,
    pub provider_spam: bool,
    pub category: String,
    /// Sent encrypted, so its words never go to AI.
    pub encrypted: bool,
}

/// True for a row whose body (as `b`) was sent encrypted. Its words, and the snippet
/// made from them, are for the person only, never for AI.
pub(crate) const ENCRYPTED_SQL: &str = "COALESCE(b.pgp_json, '') LIKE '%\"encrypted\":true%'";

const CHECK_COLUMNS: &str = "m.id, m.message_key, m.folder_id, f.path, m.uid,
    m.from_name, m.from_addr, m.reply_to_json, m.subject,
    COALESCE(b.text, m.snippet), COALESCE(b.html, ''), m.auth, m.provider_spam, m.category, m.old_mail,
    COALESCE(b.pgp_json, '') LIKE '%\"encrypted\":true%'";

/// New inbox mail still to be checked for spam, newest first. The few are picked from an index
/// first, and only their bodies read.
fn mail_to_check_sql() -> String {
    format!(
        "SELECT {CHECK_COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
         LEFT JOIN bodies b ON b.message_id = m.id
         WHERE m.id IN (
           SELECT c.id FROM messages c JOIN folders cf ON cf.id = c.folder_id
           WHERE c.account_id = ?1 AND cf.role = 'inbox' AND c.signals_done = 1
             AND c.old_mail != 1
             AND NOT EXISTS (SELECT 1 FROM verdicts v WHERE v.message_key = c.message_key)
           ORDER BY c.date DESC LIMIT ?2)
         ORDER BY m.date DESC"
    )
}

/// Bulk mail in the inbox still to be put in a tab.
const MASS_MAIL_TO_SORT: &str = "SELECT m.id, m.from_addr, m.subject FROM messages m
     JOIN folders f ON f.id = m.folder_id
     WHERE m.account_id = ?1 AND f.role = 'inbox' AND m.category = 'mass'";

/// Examples of spam: whatever the provider's own filter put in Junk.
const LEARN_FROM_JUNK: &str = "cf.role = 'junk'";
/// Examples of good mail: what got past the provider's own filter and was checked and passed
/// here. Learning only from people they've written to made all sales mail and account notices
/// look like spam.
const LEARN_FROM_GOOD: &str =
    "cf.role = 'inbox' AND c.message_key IN (SELECT message_key FROM verdicts WHERE risk = 0)";

/// Mail not learned from yet that fits `condition`, newest first, with what it says. The few
/// are picked from an index first, and only their bodies read.
fn learn_sql(condition: &str) -> String {
    format!(
        "SELECT m.message_key, m.subject || ' ' || COALESCE(b.text, m.snippet)
         FROM messages m LEFT JOIN bodies b ON b.message_id = m.id
         WHERE m.id IN (
           SELECT c.id FROM messages c JOIN folders cf ON cf.id = c.folder_id
           WHERE c.account_id = ?1 AND {condition}
             AND NOT EXISTS (SELECT 1 FROM spam_trained t WHERE t.message_key = c.message_key)
           ORDER BY c.date DESC LIMIT ?2)
         ORDER BY m.date DESC"
    )
}

fn row_to_check(r: &Row<'_>) -> rusqlite::Result<MailToCheck> {
    Ok(MailToCheck {
        id: r.get(0)?,
        key: r.get(1)?,
        folder_id: r.get(2)?,
        folder_path: r.get(3)?,
        uid: r.get(4)?,
        from_name: r.get(5)?,
        from_address: r.get(6)?,
        reply_to: parse_json(r.get::<_, String>(7)?),
        subject: r.get(8)?,
        text: r.get(9)?,
        html: r.get(10)?,
        auth: r.get(11)?,
        provider_spam: r.get(12)?,
        category: r.get(13)?,
        old_mail: r.get(14)?,
        encrypted: r.get(15)?,
    })
}

/// What keeps a conversation on a tab. Anything they've replied to counts as General,
/// and otherwise the most personal kind of mail in it decides.
fn tab_having(tab: &str) -> String {
    const GENERAL: &str =
        "(SUM(f.role = 'sent' OR (f.role = 'inbox' AND m.category = 'general')) > 0
          OR MAX(k.returned_at) IS NOT NULL OR COALESCE(MAX(k.follow_due), 0) = 1)";
    const NEWS: &str = "SUM(f.role = 'inbox' AND m.category = 'newsletters') > 0";
    match tab {
        "needs_you" => format!("HAVING {NEEDS_YOU_SQL}"),
        "good_to_know" => format!("HAVING NOT {NEEDS_YOU_SQL} AND {GENERAL}"),
        "rest" => format!("HAVING NOT {NEEDS_YOU_SQL} AND NOT {GENERAL}"),
        "general" => format!("HAVING {GENERAL}"),
        "newsletters" => format!("HAVING NOT ({GENERAL}) AND {NEWS}"),
        _ => format!("HAVING NOT ({GENERAL}) AND NOT ({NEWS})"),
    }
}

/// The rows of one page of a folder's list: what each row is keyed by, whether it's pinned,
/// and when it last changed. Found from small columns only; the rows are filled in after.
/// The longest new mail is kept out of the lists while filters look at it.
pub(crate) const FILTER_WAIT_SECONDS: i64 = 120;

/// New mail filters haven't finished with yet, which nothing shows or counts. It never
/// waits longer than `FILTER_WAIT_SECONDS` from reaching this Mac, even if they can't finish.
const HELD: &str = "SELECT id FROM messages WHERE rule_done = 0
    AND held_until > CAST(strftime('%s', 'now') AS INTEGER)";

fn page_keys_sql(role: &str, grouped: bool, tab: Option<&str>, unread_only: bool) -> String {
    page_keys_sql_from(role, grouped, tab, unread_only, false)
}

/// How far back the quick looks at a list go, from the newest email in the folder, before
/// going through all of it.
const RECENT_LOOKS: [i64; 2] = [14 * 86_400, 90 * 86_400];
/// Needs you only counts what came in this long ago or since, unless marked by hand.
const NEEDS_YOU_SECONDS: i64 = 2_592_000;

/// `recent`: only conversations with something since `?4`, or pinned. Those are every row
/// that sorts above `?4`, so when there are enough of them they are the page, found without
/// going through the whole mailbox.
fn page_keys_sql_from(
    role: &str,
    grouped: bool,
    tab: Option<&str>,
    unread_only: bool,
    recent: bool,
) -> String {
    let inbox = role == "inbox";
    const ACCOUNT: &str = "(SELECT account_id FROM folders WHERE id = ?1)";
    // Marks that can put an older conversation near the top, or that Needs you counts.
    let marked = format!(
        "SELECT thread_id FROM thread_marks WHERE account_id = {ACCOUNT}
           AND (pinned = 1 OR returned_at >= ?4 OR follow_due = 1 OR priority = 'needs_you')"
    );
    let hidden = if inbox {
        // New mail waits out of sight until filters have looked at it, so nothing they
        // move or delete is ever seen. Never for long, even if they can't finish.
        format!(
            "AND m.thread_id NOT IN (SELECT thread_id FROM thread_marks
                   WHERE account_id = {ACCOUNT} AND (muted = 1 OR snooze_until IS NOT NULL))
             AND m.id NOT IN ({HELD})"
        )
    } else if role == "drafts" {
        // Already listed as the drafts they are, above the rest.
        "AND COALESCE(m.rfc_id, '') NOT IN (SELECT server_rfc_id FROM drafts WHERE server_rfc_id IS NOT NULL)"
                .to_string()
    } else {
        String::new()
    };
    let mut having: Vec<String> = Vec::new();
    if let Some(tab) = tab {
        having.push(tab_having(tab).trim_start_matches("HAVING ").to_string());
    }
    if inbox {
        // New senders wait in the screener until they're let in.
        having.push(
            "(SUM(f.role = 'inbox' AND m.screened = 0) > 0 OR MAX(k.returned_at) IS NOT NULL
                  OR COALESCE(MAX(k.follow_due), 0) = 1)"
                .to_string(),
        );
    }
    if unread_only {
        having.push("SUM(m.seen = 0) > 0".to_string());
    }
    let having = if having.is_empty() {
        String::new()
    } else {
        format!("HAVING {}", having.join(" AND "))
    };
    // Finds the rows first using only small columns, then fills in just those.
    // With a whole mailbox here, doing it all at once takes seconds.
    let (key, belongs, group) = if grouped {
        let returned = if inbox {
            format!(
                "OR m.thread_id IN (SELECT thread_id FROM thread_marks WHERE account_id = {ACCOUNT}
                       AND (returned_at IS NOT NULL OR follow_due = 1))"
            )
        } else {
            String::new()
        };
        // The conversations to look through. A quick look starts from what's recent anywhere
        // in the account, such as a reply sent today to an old email, and keeps those here.
        let threads = if recent {
            format!(
                "SELECT thread_id FROM messages INDEXED BY messages_folder_date
                   WHERE folder_id IN (SELECT id FROM folders WHERE account_id = {ACCOUNT}) AND date >= ?4
                     AND thread_id IN (SELECT thread_id FROM messages WHERE folder_id = ?1)
                 UNION SELECT thread_id FROM ({marked})
                   WHERE thread_id IN (SELECT thread_id FROM messages WHERE folder_id = ?1)"
            )
        } else {
            "SELECT thread_id FROM messages WHERE folder_id = ?1".to_string()
        };
        (
                "m.thread_id",
                format!(
                    "(m.thread_id IN ({threads}) {returned})
                     AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?1) AND (m.is_draft = 0 OR f.id = ?1)"
                ),
                "m.thread_id",
            )
    } else {
        (
            "MAX(m.id)",
            if recent {
                format!("m.folder_id = ?1 AND (m.date >= ?4 OR m.thread_id IN ({marked}))")
            } else {
                "m.folder_id = ?1".to_string()
            },
            // The same email delivered twice is still one row.
            "COALESCE(m.rfc_id, '#' || m.id)",
        )
    };
    format!(
                "SELECT {key}, COALESCE(MAX(k.pinned), 0),
                        MAX(MAX(m.date), COALESCE(MAX(k.returned_at), 0))
                 FROM messages m JOIN folders f ON f.id = m.folder_id
                 LEFT JOIN thread_marks k ON k.account_id = m.account_id AND k.thread_id = m.thread_id
                 WHERE m.account_id = {ACCOUNT} AND {belongs} {hidden}
                 GROUP BY {group} {having}
                 ORDER BY COALESCE(MAX(k.pinned), 0) DESC,
                          MAX(MAX(m.date), COALESCE(MAX(k.returned_at), 0)) DESC
                 LIMIT ?2 OFFSET ?3"
            )
}

/// A conversation needs the person if the newest thing marked as needing them came after their
/// own last message, and isn't over a month old. Anything they brought back counts too, and
/// their own choice for the conversation outranks all of it. Saying it doesn't need them covers
/// the emails there were then: a newer one that needs them brings it back.
pub(crate) const NEEDS_YOU_SQL: &str = "(COALESCE(MAX(k.priority), '') = 'needs_you'
    OR MAX(k.returned_at) IS NOT NULL OR COALESCE(MAX(k.follow_due), 0) = 1
    OR (COALESCE(MAX(k.priority), '') IN ('', 'good_to_know')
        AND COALESCE(MAX(CASE WHEN f.role = 'inbox' AND m.priority = 'needs_you' THEN m.date END), 0)
            > MAX(COALESCE(MAX(CASE WHEN f.role = 'sent' THEN m.date END), 0),
                  CAST(strftime('%s', 'now') AS INTEGER) - 2592000,
                  CASE WHEN MAX(k.priority) = 'good_to_know'
                       THEN COALESCE(MAX(k.priority_upto), 9223372036854775807) ELSE 0 END)))";

/// What the sync needs to find a message on the server.
pub struct MessageLocation {
    pub account_id: i64,
    pub folder_path: String,
    pub uid_validity: Option<u32>,
    pub uid: u32,
}

/// One stored copy of a message in a thread, for acting on it.
pub struct ThreadCopy {
    pub is_draft: bool,
    pub id: i64,
    pub folder_id: i64,
    pub folder_path: String,
    pub folder_role: String,
    pub uid_validity: Option<u32>,
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
}

/// Bodies still to come in a folder, newest first, read from the index alone.
const BODIES_WANTED: &str = "SELECT m.uid, m.size FROM messages m
     WHERE m.folder_id = ?1 AND m.has_body = 0 AND m.date >= ?2 AND m.size <= ?3
       AND m.body_tries < 3
     ORDER BY m.date DESC LIMIT ?4";

/// Each folder of an inbox, how much of it is here, and how many bodies are still to come.
const HEALTH_FOLDERS: &str = "SELECT f.id, f.name, f.role, f.server_total, f.last_synced_at,
            f.last_error, COUNT(m.id),
            SUM(m.has_body = 0 AND m.size <= ?2 AND m.date >= ?3 AND m.body_tries < 3)
     FROM folders f LEFT JOIN messages m ON m.folder_id = f.id
     WHERE f.account_id = ?1 GROUP BY f.id ORDER BY f.id";

/// Emails with no body here whose twin in another folder has one, with that twin.
const TWIN_BODIES: &str = "SELECT m.id,
            (SELECT t.id FROM messages t
             WHERE t.account_id = m.account_id AND t.rfc_id = m.rfc_id
               AND t.message_key = m.message_key AND t.id != m.id AND t.has_body = 1 LIMIT 1)
     FROM messages m
     WHERE m.folder_id = ?1 AND m.has_body = 0 AND m.rfc_id IS NOT NULL
       AND EXISTS (SELECT 1 FROM messages t
                   WHERE t.account_id = m.account_id AND t.rfc_id = m.rfc_id
                     AND t.message_key = m.message_key AND t.id != m.id AND t.has_body = 1)
     ORDER BY m.date DESC LIMIT ?2";

/// Each folder of an inbox with its totals. What's been put aside or is waiting in the
/// screener isn't asking for attention.
fn folder_counts_sql(grouped: bool) -> String {
    let unread = if grouped {
        "m.thread_id"
    } else {
        "COALESCE(m.rfc_id, '#' || m.id)"
    };
    format!(
        "SELECT f.id, f.account_id, f.path, f.name, f.role, f.last_synced_at,
                COUNT(m.id), COUNT(DISTINCT CASE WHEN m.seen = 0 AND (f.role != 'inbox' OR (m.screened = 0
                  AND m.id NOT IN ({HELD})
                  AND m.thread_id NOT IN (SELECT thread_id FROM thread_marks WHERE account_id = ?1
                     AND (muted = 1 OR snooze_until IS NOT NULL)))) THEN {unread} END)
         FROM folders f LEFT JOIN messages m ON m.folder_id = f.id
         WHERE f.account_id = ?1 GROUP BY f.id ORDER BY {FOLDER_ORDER}"
    )
}

/// The query behind each row of the email list.
fn summary_sql(name: &str, condition: &str, tail: &str) -> String {
    // With one MAX, SQLite fills the plain columns from the newest row.
    format!(
        "SELECT {name}, m.account_id, m.subject, m.snippet, MAX(m.date) AS last_date,
                COUNT(DISTINCT COALESCE(m.rfc_id, '#' || m.id)),
                COUNT(DISTINCT CASE WHEN m.seen = 0 THEN COALESCE(m.rfc_id, '#' || m.id) END),
                SUM(m.flagged) > 0,
                GROUP_CONCAT(CASE WHEN lower(m.from_addr) = lower(a.email) THEN 'me'
                                  WHEN m.from_name = '' THEN m.from_addr ELSE m.from_name END, char(31)),
                MAX(m.has_files) > 0,
                a.email,
                GROUP_CONCAT(m.from_addr, char(31)),
                CASE WHEN SUM(COALESCE(v.risk, 0) = 2) > 0 THEN 2
                     WHEN SUM(COALESCE(v.risk, 0) = 1) > 0 THEN 1 ELSE 0 END,
                GROUP_CONCAT(m.ai_label, char(31)),
                COALESCE(k.pinned, 0), COALESCE(k.muted, 0), k.snooze_until,
                k.returned_at, COALESCE(k.follow_due, 0), k.follow_at,
                {NEEDS_YOU_SQL},
                MAX(CASE WHEN m.priority_why IS NOT NULL THEN m.date || char(31) || m.priority_why END),
                MAX(CASE WHEN m.ai_line IS NOT NULL THEN m.date || char(31) || m.ai_line END),
                MAX(m.due_at),
                (SELECT GROUP_CONCAT(x.tag_id || ':' || COALESCE(x.amount, ''))
                 FROM thread_tags x WHERE x.account_id = m.account_id AND x.thread_id = m.thread_id)
         FROM messages m
         JOIN accounts a ON a.id = m.account_id
         JOIN folders f ON f.id = m.folder_id
         LEFT JOIN verdicts v ON v.message_key = m.message_key
         LEFT JOIN thread_marks k ON k.account_id = m.account_id AND k.thread_id = m.thread_id
         WHERE {condition}
         {tail}"
    )
}

/// Drops the date that picked the newest of something.
fn after_date(text: Option<String>) -> Option<String> {
    text.and_then(|t| t.split_once('\u{1f}').map(|(_, rest)| rest.to_string()))
}

fn read_summary(r: &Row, folder_id: i64) -> rusqlite::Result<ThreadSummary> {
    let names: String = r.get(8)?;
    let email: String = r.get(10)?;
    let addresses: String = r.get(11)?;
    let lead_address = addresses
        .split('\u{1f}')
        .find(|a| !a.eq_ignore_ascii_case(&email))
        .or_else(|| addresses.split('\u{1f}').next())
        .unwrap_or("")
        .to_string();
    let mut participants: Vec<String> = Vec::new();
    for n in names.split('\u{1f}') {
        if !participants.iter().any(|p| p == n) {
            participants.push(n.to_string());
        }
    }
    let subject = parse::clean_subject(&r.get::<_, String>(2)?);
    let snippet: String = r.get(3)?;
    let risk: i64 = r.get(12)?;
    // Never offered from mail that looks like spam or a trick.
    let code = (risk == 0)
        .then(|| crate::codes::one_time_code(&subject, &snippet))
        .flatten();
    Ok(ThreadSummary {
        code,
        team: None,
        pinned: r.get(14)?,
        muted: r.get(15)?,
        snooze_until: r.get(16)?,
        returned_at: r.get(17)?,
        follow_due: r.get(18)?,
        follow_at: r.get(19)?,
        best_match: false,
        tags: crate::tags::unpack(r.get(24)?),
        needs_you: r.get(20)?,
        why: after_date(r.get(21)?),
        ai_line: after_date(r.get(22)?),
        due_at: r.get(23)?,
        thread_id: r.get(0)?,
        account_id: r.get(1)?,
        subject,
        snippet,
        last_date: r.get(4)?,
        message_count: r.get(5)?,
        unread_count: r.get(6)?,
        flagged: r.get(7)?,
        has_attachments: r.get(9)?,
        risk,
        label: r
            .get::<_, Option<String>>(13)?
            .and_then(|l| l.split('\u{1f}').next().map(str::to_string)),
        account_email: email,
        lead_address,
        folder_id,
        participants,
    })
}

/// The secret holding the key the mail database is encrypted with.
pub const STORE_KEY: &str = "store-key";

const WRONG_KEY: &str = "The store's key is wrong.";
const KEY_UNREADABLE: &str =
    "Mailzo couldn't read its key from the Keychain, so the mail saved on \
this Mac can't be opened right now. Nothing has been deleted. Quit Mailzo and open it again, and \
choose Allow if your Mac asks about the Keychain.";
/// How every unencrypted SQLite file begins.
const PLAIN_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// What is saved where the mail database goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFile {
    Missing,
    Plain,
    Encrypted,
}

/// What opening the mail database did, for telling the person when it matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// Encrypted already, or new.
    Encrypted,
    /// Was unencrypted, and is encrypted from now on.
    Migrated,
    /// Still unencrypted this time, and tried again next time. Says why, for the log.
    LeftUnencrypted(String),
    /// Its key was gone, so the saved mail was moved here and a new database started.
    StartedFresh(std::path::PathBuf),
}

pub struct Opened {
    pub store: Store,
    pub opening: Opening,
}

fn valid_key(key_hex: &str) -> bool {
    key_hex.len() == 64 && key_hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 32 random bytes from the system, as hex.
pub fn new_store_key() -> String {
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    data_encoding::HEXLOWER.encode(&key)
}

/// Gives a connection its key, and checks it really is encrypted and the key fits.
fn unlock(conn: &Connection, key_hex: &str) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;
    let cipher: Option<String> = conn
        .query_row("PRAGMA cipher_version", [], |r| r.get(0))
        .optional()?;
    if cipher.is_none_or(|c| c.is_empty()) {
        return Err(CoreError::Invalid(
            "This build can't encrypt the store.".into(),
        ));
    }
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // Reading anything fails here if the key is wrong.
    match conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    }) {
        Ok(_) => Ok(()),
        Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) => {
            Err(CoreError::Invalid(WRONG_KEY.into()))
        }
        Err(e) => Err(e.into()),
    }
}

/// The same path with something added to the end, such as `-wal`.
fn beside(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn remove_with_side_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        remove_if_there(&beside(path, suffix))
            .map_err(|e| CoreError::Invalid(format!("Could not tidy up: {e}")))?;
    }
    Ok(())
}

/// How many rows every stored table has, the search index's own tables included.
fn table_counts(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND rootpage > 0")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut counts = BTreeMap::new();
    for name in names {
        let sql = format!("SELECT COUNT(*) FROM \"{}\"", name.replace('"', "\"\""));
        counts.insert(name, conn.query_row(&sql, [], |r| r.get(0))?);
    }
    Ok(counts)
}

/// Every table, index, trigger and view, by kind and name.
fn schema_objects(conn: &Connection) -> Result<BTreeSet<(String, String)>> {
    Ok(conn
        .prepare("SELECT type, name FROM sqlite_schema")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn has_object(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = ?1",
            [name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Something a database change makes, so its being there can be checked.
#[derive(Debug, PartialEq)]
enum Made {
    Object(String),
    Column(String, String),
}

/// The tables, indexes, triggers and columns a change makes, read from its SQL.
fn made_by(sql: &str) -> Vec<Made> {
    let words: Vec<String> = sql
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let mut made = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let at = |n: usize| words.get(i + n).map(String::as_str).unwrap_or("");
        if w == "create" {
            let mut n = 1;
            for skip in ["unique", "virtual"] {
                if at(n) == skip {
                    n += 1;
                }
            }
            if matches!(at(n), "table" | "index" | "trigger") {
                n += 1;
                if at(n) == "if" && at(n + 1) == "not" && at(n + 2) == "exists" {
                    n += 3;
                }
                if !at(n).is_empty() {
                    made.push(Made::Object(at(n).to_string()));
                }
            }
        } else if w == "alter" && at(1) == "table" && at(3) == "add" {
            let col = if at(4) == "column" { at(5) } else { at(4) };
            made.push(Made::Column(at(2).to_string(), col.to_string()));
        }
    }
    made
}

fn is_there(conn: &Connection, m: &Made) -> Result<bool> {
    match m {
        Made::Object(name) => has_object(conn, name),
        Made::Column(table, col) => Ok(conn
            .query_row(
                "SELECT 1 FROM pragma_table_xinfo(?1) WHERE name = ?2",
                [table, col],
                |_| Ok(()),
            )
            .optional()?
            .is_some()),
    }
}

/// How much of a change a database already has: all of it, none of it, or it can't tell.
fn how_much_there(conn: &Connection, sql: &str) -> Result<Option<bool>> {
    let made = made_by(sql);
    if made.is_empty() {
        return Ok(None);
    }
    let mut there = 0;
    for m in &made {
        if is_there(conn, m)? {
            there += 1;
        }
    }
    Ok(match there {
        0 => Some(false),
        n if n == made.len() => Some(true),
        _ => None,
    })
}

/// Names a later change drops or renames away, which must never be made again.
fn gone_later() -> Vec<String> {
    let mut gone = Vec::new();
    for sql in MIGRATIONS {
        let words: Vec<String> = sql
            .split(|c: char| c.is_whitespace() || c == ';')
            .filter(|w| !w.is_empty())
            .map(|w| w.to_ascii_lowercase())
            .collect();
        for (i, w) in words.iter().enumerate() {
            let at = |n: usize| words.get(i + n).cloned().unwrap_or_default();
            if w == "drop" && matches!(at(1).as_str(), "table" | "index" | "trigger") {
                let n = if at(2) == "if" { 4 } else { 2 };
                gone.push(at(n));
            } else if w == "alter" && at(1) == "table" && at(3) == "rename" {
                gone.push(at(2));
            }
        }
    }
    gone
}

/// Whether a database numbered in the other order has already had this one.
fn ran_in_other_order(conn: &Connection, sql: &str) -> Result<bool> {
    if sql.contains("ADD COLUMN message_key") {
        return Ok(conn
            .query_row(
                "SELECT 1 FROM pragma_table_xinfo('messages') WHERE name = 'message_key'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some());
    }
    for name in ["meaning_todo", "agent_activity", "messages_newsletters"] {
        if sql.contains(&format!("CREATE TABLE {name}"))
            || sql.contains(&format!("CREATE INDEX {name}"))
        {
            return has_object(conn, name);
        }
    }
    Ok(false)
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CoreError::Invalid(format!("Cannot create mail folder: {e}")))?;
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        let mut store = Self::init(conn)?;
        store.readers = (0..READERS)
            .filter_map(|_| {
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
            })
            .map(Mutex::new)
            .collect();
        Ok(store)
    }

    /// Opens a store that's encrypted on disk with this 32-byte key, given as hex. Only works
    /// in a build with SQLCipher, and says so rather than quietly writing it unencrypted.
    pub fn open_with_key(path: &Path, key_hex: &str) -> Result<Store> {
        if !valid_key(key_hex) {
            return Err(CoreError::Invalid("The store's key isn't valid.".into()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CoreError::Invalid(format!("Cannot create mail folder: {e}")))?;
        }
        let unlock = |conn: &Connection| unlock(conn, key_hex);
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        unlock(&conn)?;
        let mut store = Self::init(conn)?;
        store.readers = (0..READERS)
            .filter_map(|_| {
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .ok()
                    .filter(|r| unlock(r).is_ok())
            })
            .map(Mutex::new)
            .collect();
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Opens the mail database encrypted, with its key kept in `secrets` as [`STORE_KEY`].
    ///
    /// The first time, a new key is made and saved. A database from before encryption is
    /// copied into an encrypted one and swapped in; if that fails it is opened as it is and
    /// tried again next time, so no mail is ever lost on the way. A database whose key is
    /// gone can't be read by anyone, so it is moved aside and a new one started.
    pub fn open_protected(path: &Path, secrets: &dyn SecretStore) -> Result<Opened> {
        let found = Self::file_state(path)?;
        let unencrypted = |why: String| -> Result<Opened> {
            Ok(Opened {
                store: Store::open(path)?,
                opening: Opening::LeftUnencrypted(why),
            })
        };
        let saved = match secrets.get(STORE_KEY) {
            Ok(key) => key.filter(|k| valid_key(k)),
            Err(_) if found == StoreFile::Encrypted => {
                return Err(CoreError::Invalid(KEY_UNREADABLE.into()))
            }
            Err(e) => return unencrypted(e.to_string()),
        };
        let key = match saved {
            Some(key) => key,
            None => {
                let key = new_store_key();
                if let Err(e) = secrets.set(STORE_KEY, &key) {
                    if found == StoreFile::Encrypted {
                        return Err(CoreError::Invalid(KEY_UNREADABLE.into()));
                    }
                    return unencrypted(e.to_string());
                }
                if found == StoreFile::Encrypted {
                    return Self::start_fresh(path, &key);
                }
                key
            }
        };
        match found {
            StoreFile::Missing | StoreFile::Encrypted => match Store::open_with_key(path, &key) {
                Ok(store) => Ok(Opened {
                    store,
                    opening: Opening::Encrypted,
                }),
                Err(CoreError::Invalid(why))
                    if why == WRONG_KEY && found == StoreFile::Encrypted =>
                {
                    Self::start_fresh(path, &key)
                }
                Err(e) => Err(e),
            },
            StoreFile::Plain => match Self::encrypt_file(path, &key) {
                Ok(()) => Ok(Opened {
                    store: Store::open_with_key(path, &key)?,
                    opening: Opening::Migrated,
                }),
                Err(e) => unencrypted(e.to_string()),
            },
        }
    }

    /// Moves a database nobody can read out of the way, keeping it, and starts a new one.
    fn start_fresh(path: &Path, key_hex: &str) -> Result<Opened> {
        let moved = Self::move_aside(path)?;
        Ok(Opened {
            store: Store::open_with_key(path, key_hex)?,
            opening: Opening::StartedFresh(moved),
        })
    }

    /// What is saved where the mail database goes, told apart without needing a key.
    pub fn file_state(path: &Path) -> Result<StoreFile> {
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        let unreadable =
            |e: std::io::Error| CoreError::Invalid(format!("Cannot read the mail database: {e}"));
        match size(path) {
            None => Ok(StoreFile::Missing),
            // New mail can still be waiting in the side file before any reaches the main one.
            Some(0) if size(&beside(path, "-wal")).unwrap_or(0) == 0 => Ok(StoreFile::Missing),
            Some(0) => {
                let plain = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .and_then(|c| {
                        c.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| {
                            r.get::<_, i64>(0)
                        })
                    })
                    .is_ok();
                Ok(if plain {
                    StoreFile::Plain
                } else {
                    StoreFile::Encrypted
                })
            }
            Some(_) => {
                use std::io::Read;
                let mut start = [0u8; 16];
                let read = std::fs::File::open(path)
                    .and_then(|mut f| f.read(&mut start))
                    .map_err(unreadable)?;
                // An encrypted file starts with random bytes instead of SQLite's own words.
                Ok(if read == 16 && &start == PLAIN_HEADER {
                    StoreFile::Plain
                } else {
                    StoreFile::Encrypted
                })
            }
        }
    }

    /// Copies an unencrypted database into a new file encrypted with this key, checks the copy
    /// opens with the key and holds everything the original did, then puts it in the original's
    /// place in one step. Anything going wrong leaves the original as it was.
    pub fn encrypt_file(path: &Path, key_hex: &str) -> Result<()> {
        if !valid_key(key_hex) {
            return Err(CoreError::Invalid("The store's key isn't valid.".into()));
        }
        if Self::file_state(path)? != StoreFile::Plain {
            return Err(CoreError::Invalid(
                "Only an unencrypted mail database can be encrypted.".into(),
            ));
        }
        let next = beside(path, ".encrypting");
        // Left over from a try that stopped part way.
        remove_with_side_files(&next)?;
        if let Err(e) = Self::export_encrypted(path, &next, key_hex) {
            let _ = remove_with_side_files(&next);
            return Err(e);
        }
        // The old file's side files would be read into the new one, so they must be empty
        // and gone first. Closing the last connection has already emptied them.
        let wal = beside(path, "-wal");
        if std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) {
            let _ = remove_with_side_files(&next);
            return Err(CoreError::Invalid(
                "The mail database is still in use.".into(),
            ));
        }
        let swap = || -> std::io::Result<()> {
            remove_if_there(&wal)?;
            remove_if_there(&beside(path, "-shm"))?;
            std::fs::File::open(&next)?.sync_all()?;
            std::fs::rename(&next, path)?;
            if let Some(dir) = path.parent() {
                let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
            }
            Ok(())
        };
        swap().map_err(|e| {
            let _ = remove_with_side_files(&next);
            CoreError::Invalid(format!("Could not swap in the encrypted database: {e}"))
        })
    }

    fn export_encrypted(path: &Path, next: &Path, key_hex: &str) -> Result<()> {
        let Some(target) = next.to_str() else {
            return Err(CoreError::Invalid(
                "The mail folder's name can't be used.".into(),
            ));
        };
        let (version, tables, objects) = {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            // Everything waiting in the side file goes into the main one first.
            let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
            if busy != 0 {
                return Err(CoreError::Invalid(
                    "The mail database is still in use.".into(),
                ));
            }
            let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
            let tables = table_counts(&conn)?;
            let objects = schema_objects(&conn)?;
            // Made empty first, readable by this person only, as the connection can't make files.
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(next).map_err(|e| {
                CoreError::Invalid(format!("Could not make the encrypted copy: {e}"))
            })?;
            conn.execute_batch(&format!(
                "ATTACH DATABASE '{}' AS enc KEY \"x'{key_hex}'\";",
                target.replace('\'', "''")
            ))?;
            // Tables with their rows, indexes, triggers and the search index's own tables.
            conn.query_row("SELECT sqlcipher_export('enc')", [], |_| Ok(()))?;
            conn.pragma_update(Some("enc"), "user_version", version)?;
            conn.execute_batch("DETACH DATABASE enc;")?;
            (version, tables, objects)
        };
        let copy = Connection::open_with_flags(next, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        unlock(&copy, key_hex)?;
        let ok: String = copy.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        let same = ok == "ok"
            && copy.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == version
            && table_counts(&copy)? == tables
            && schema_objects(&copy)? == objects;
        if !same {
            return Err(CoreError::Invalid(
                "The encrypted copy didn't match the mail database.".into(),
            ));
        }
        Ok(())
    }

    /// Moves a database out of the way, with its side file, and says where it went.
    pub fn move_aside(path: &Path) -> Result<std::path::PathBuf> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |t| t.as_secs());
        let moved = beside(path, &format!(".unreadable-{stamp}"));
        let fail = |e: std::io::Error| {
            CoreError::Invalid(format!("Could not move the old mail database aside: {e}"))
        };
        std::fs::rename(path, &moved).map_err(fail)?;
        let wal = beside(path, "-wal");
        if wal.exists() {
            std::fs::rename(&wal, beside(&moved, "-wal")).map_err(fail)?;
        }
        remove_if_there(&beside(path, "-shm")).map_err(fail)?;
        Ok(moved)
    }

    fn init(mut conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        // Two lines of work each added to this list before they were joined. A database from the
        // one whose additions now sit last has a number that means something else in this order.
        let split = MIGRATIONS
            .iter()
            .position(|m| m.contains("CREATE TABLE tags"))
            .unwrap_or(MIGRATIONS.len());
        let other_order = version as usize > split && !has_object(&conn, "tags")?;
        let start = if other_order { split } else { version as usize };
        // A test copy built from other work can have used a number for a change of its own.
        // A change counted as done but with nothing of it there is made now, and one still to
        // come that's already all there is skipped, so neither leaves the app broken.
        let gone = gone_later();
        let touches_gone = |sql: &str| {
            made_by(sql).iter().any(|m| match m {
                Made::Object(n) | Made::Column(n, _) => gone.contains(n),
            })
        };
        for sql in MIGRATIONS.iter().take(start) {
            if !touches_gone(sql) && how_much_there(&conn, sql)? == Some(false) {
                let tx = conn.transaction()?;
                tx.execute_batch(sql)?;
                tx.commit()?;
            }
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(start) {
            let tx = conn.transaction()?;
            let done = (other_order && ran_in_other_order(&tx, sql)?)
                || how_much_there(&tx, sql)? == Some(true);
            if !done {
                tx.execute_batch(sql)?;
            }
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Self::fill_base_subjects(&conn)?;
        Self::join_split_replies(&mut conn)?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
            readers: Vec::new(),
            seen_held: Mutex::default(),
        })
    }

    /// Replies saved before a person answering again could be matched by subject each
    /// started a conversation of their own. Joins them once, by the same strict rule as
    /// new mail: a "Re:" subject, recent, between the same people, and only replies whose
    /// own conversation starts at an email this device doesn't have.
    fn join_split_replies(conn: &mut Connection) -> Result<()> {
        const NAME: &str = "join_split_replies";
        let done: bool = conn
            .query_row("SELECT 1 FROM repairs WHERE name = ?1", [NAME], |_| Ok(()))
            .optional()?
            .is_some();
        if done {
            return Ok(());
        }
        let tx = conn.transaction()?;
        let orphans: Vec<(i64, i64, String, String, String, String, i64)> = {
            let mut st = tx.prepare(
                "SELECT m.id, m.account_id, m.base_subject, m.from_addr, m.to_json, m.cc_json, m.date
                 FROM messages m
                 WHERE lower(ltrim(m.subject)) LIKE 're:%' AND COALESCE(m.base_subject, '') != ''
                   AND NOT EXISTS (SELECT 1 FROM messages x
                                   WHERE x.account_id = m.account_id AND x.rfc_id = m.thread_id)
                 ORDER BY m.account_id, m.date",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let lower = |json: String| -> HashSet<String> {
            parse_json::<Addr>(json)
                .iter()
                .map(|a| a.address.to_ascii_lowercase())
                .collect()
        };
        for (id, account_id, base, from, to, cc, date) in orphans {
            // Earlier joins may have moved it, so its conversation is read fresh.
            let Some(own) = tx
                .query_row("SELECT thread_id FROM messages WHERE id = ?1", [id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            else {
                continue;
            };
            let sender = from.to_ascii_lowercase();
            let mut people = lower(to);
            people.extend(lower(cc));
            let others: Vec<(String, String, String, String)> = {
                let mut st = tx.prepare_cached(
                    "SELECT thread_id, from_addr, to_json, cc_json FROM messages
                     WHERE account_id = ?1 AND base_subject = ?2 AND date BETWEEN ?3 AND ?4
                       AND thread_id != ?5 AND date <= ?6
                     ORDER BY date LIMIT 50",
                )?;
                let rows = st.query_map(
                    params![
                        account_id,
                        base,
                        date - THREAD_WINDOW_SECONDS,
                        date + CLOCK_SKEW_SECONDS,
                        own,
                        date
                    ],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let found = others.into_iter().find(|(_, f, t, c)| {
                let mut theirs = lower(t.clone());
                theirs.extend(lower(c.clone()));
                same_people(&sender, &people, &f.to_ascii_lowercase(), &theirs)
            });
            if let Some((thread, ..)) = found {
                // The same name on every device, as a team knows a conversation by it.
                let (first, other) = if thread < own {
                    (thread, own)
                } else {
                    (own, thread)
                };
                join_thread_into(&tx, account_id, &first, &other)?;
            }
        }
        tx.execute("INSERT INTO repairs (name) VALUES (?1)", [NAME])?;
        tx.commit()?;
        Ok(())
    }

    /// Mail saved before subjects were compared has no comparable subject yet.
    fn fill_base_subjects(conn: &Connection) -> Result<()> {
        // Every email saved since has one, so this looks once. Looking each time read every
        // email in a big mailbox and held up every start by seconds.
        const NAME: &str = "fill_base_subjects";
        let done = conn
            .query_row("SELECT 1 FROM repairs WHERE name = ?1", [NAME], |_| Ok(()))
            .optional()?
            .is_some();
        if done {
            return Ok(());
        }
        let rows: Vec<(i64, String)> = {
            let mut st =
                conn.prepare("SELECT id, subject FROM messages WHERE base_subject IS NULL")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, subject) in rows {
            conn.execute(
                "UPDATE messages SET base_subject = ?2 WHERE id = ?1",
                params![id, parse::base_subject(&subject)],
            )?;
        }
        conn.execute("INSERT OR IGNORE INTO repairs (name) VALUES (?1)", [NAME])?;
        Ok(())
    }

    pub(crate) fn db(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whichever is free. A database only held in memory, as in tests, has just the one connection.
    pub(crate) fn reading_db(&self) -> MutexGuard<'_, Connection> {
        for reader in &self.readers {
            if let Ok(free) = reader.try_lock() {
                return free;
            }
        }
        self.readers
            .first()
            .unwrap_or(&*self.conn)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    // ---- accounts ----

    pub fn insert_account(
        &self,
        email: &str,
        display_name: &str,
        host: &str,
        port: u16,
        security: Security,
        username: &str,
    ) -> Result<Account> {
        let sync_id = crate::crypto::random_hex(16);
        self.insert_account_with_sync_id(
            email,
            display_name,
            host,
            port,
            security,
            username,
            &sync_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_account_with_sync_id(
        &self,
        email: &str,
        display_name: &str,
        host: &str,
        port: u16,
        security: Security,
        username: &str,
        sync_id: &str,
    ) -> Result<Account> {
        let db = self.db();
        db.execute(
            "INSERT INTO accounts (email, display_name, imap_host, imap_port, security, username, sync_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![email, display_name, host, port, security.as_str(), username, sync_id],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                CoreError::Invalid("This account has already been added.".into())
            }
            e => e.into(),
        })?;
        Ok(Account {
            id: db.last_insert_rowid(),
            sync_id: sync_id.into(),
            email: email.into(),
            display_name: display_name.into(),
            imap_host: host.into(),
            imap_port: port,
            security,
            username: username.into(),
            smtp_host: String::new(),
            smtp_port: 0,
            smtp_security: SmtpSecurity::Tls,
            signature: String::new(),
            settings_updated_at: 0,
            prefs: AccountPrefs::default(),
            auth: crate::signin::PASSWORD.to_string(),
        })
    }

    /// Saves everything a person can change about an inbox.
    pub fn save_account(&self, a: &Account) -> Result<()> {
        self.db().execute(
            "UPDATE accounts SET display_name = ?2, signature = ?3, smtp_host = ?4, smtp_port = ?5,
               smtp_security = ?6, settings_updated_at = ?7, prefs_json = ?8, imap_host = ?9,
               imap_port = ?10, security = ?11, username = ?12, auth = ?13 WHERE id = ?1",
            params![
                a.id,
                a.display_name,
                a.signature,
                a.smtp_host,
                a.smtp_port,
                a.smtp_security.as_str(),
                a.settings_updated_at,
                serde_json::to_string(&a.prefs).unwrap_or_else(|_| "{}".into()),
                a.imap_host,
                a.imap_port,
                a.security.as_str(),
                a.username,
                a.auth,
            ],
        )?;
        Ok(())
    }

    /// Records the outgoing server once it has been found to work.
    pub fn set_smtp(&self, id: i64, host: &str, port: u16, security: SmtpSecurity) -> Result<()> {
        self.db().execute(
            "UPDATE accounts SET smtp_host = ?2, smtp_port = ?3, smtp_security = ?4 WHERE id = ?1",
            params![id, host, port, security.as_str()],
        )?;
        Ok(())
    }

    pub fn set_sync_id(&self, account_id: i64, sync_id: &str) -> Result<()> {
        self.db().execute(
            "UPDATE accounts SET sync_id = ?2 WHERE id = ?1",
            params![account_id, sync_id],
        )?;
        Ok(())
    }

    // ---- identity ----

    pub fn identity(&self) -> Result<Option<Identity>> {
        Ok(self
            .reading_db()
            .query_row(
                "SELECT email, device_id, device_name, state FROM identity WHERE id = 1",
                [],
                |r| {
                    Ok(Identity {
                        email: r.get(0)?,
                        device_id: r.get(1)?,
                        device_name: r.get(2)?,
                        state: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn save_identity(
        &self,
        email: &str,
        device_id: i64,
        device_name: &str,
        state: &str,
    ) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO identity (id, email, device_id, device_name, state, vault_version)
             VALUES (1, ?1, ?2, ?3, ?4, 0)",
            params![email, device_id, device_name, state],
        )?;
        Ok(())
    }

    pub fn set_identity_state(&self, state: &str) -> Result<()> {
        self.db()
            .execute("UPDATE identity SET state = ?1 WHERE id = 1", [state])?;
        Ok(())
    }

    pub fn vault_version(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT vault_version FROM identity WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0))
    }

    /// Never moves backwards, so an old copy of the vault can't be replayed.
    pub fn set_vault_version(&self, version: i64) -> Result<()> {
        self.db().execute(
            "UPDATE identity SET vault_version = MAX(vault_version, ?1) WHERE id = 1",
            [version],
        )?;
        Ok(())
    }

    pub fn tombstones(&self) -> Result<BTreeMap<String, i64>> {
        let db = self.db();
        let mut st = db.prepare("SELECT sync_id, removed_at FROM tombstones")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_tombstone(&self, sync_id: &str) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO tombstones (sync_id) VALUES (?1)",
            [sync_id],
        )?;
        Ok(())
    }

    pub fn save_link_nonce(&self, device_id: i64, nonce: &str) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO link_nonces (device_id, nonce) VALUES (?1, ?2)",
            params![device_id, nonce],
        )?;
        Ok(())
    }

    pub fn link_nonce(&self, device_id: i64) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "SELECT nonce FROM link_nonces WHERE device_id = ?1",
                [device_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn delete_link_nonce(&self, device_id: i64) -> Result<()> {
        self.db()
            .execute("DELETE FROM link_nonces WHERE device_id = ?1", [device_id])?;
        Ok(())
    }

    /// Everything tied to the signed-in person. Removes all saved mail.
    pub fn wipe_identity_and_mail(&self) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute_batch(
            "DELETE FROM agent_activity; DELETE FROM accounts; DELETE FROM identity; DELETE FROM tombstones; DELETE FROM link_nonces; DELETE FROM avatars; DELETE FROM settings; DELETE FROM images_held;
             DELETE FROM verdicts; DELETE FROM contacts; DELETE FROM spam_words; DELETE FROM spam_trained;
             DELETE FROM calendars; DELETE FROM tracker_counts; DELETE FROM tracked_mail; DELETE FROM ai_usage; DELETE FROM ai_summaries; DELETE FROM rule_seen;
             DELETE FROM team_info; DELETE FROM team_people; DELETE FROM team_threads;
             DELETE FROM team_comments; DELETE FROM team_templates; DELETE FROM draft_team_base;",
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Forgets the sign-in, and keeps the inboxes and mail on this device.
    pub fn clear_identity(&self) -> Result<()> {
        self.db()
            .execute_batch("DELETE FROM identity; DELETE FROM link_nonces;")?;
        Ok(())
    }

    pub fn account_exists(&self, email: &str, host: &str) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM accounts WHERE email = ?1 AND imap_host = ?2",
                params![email, host],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn row_to_account(r: &Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: r.get(0)?,
            email: r.get(1)?,
            display_name: r.get(2)?,
            imap_host: r.get(3)?,
            imap_port: r.get(4)?,
            security: Security::parse(&r.get::<_, String>(5)?),
            username: r.get(6)?,
            sync_id: r.get(7)?,
            smtp_host: r.get(8)?,
            smtp_port: r.get(9)?,
            smtp_security: SmtpSecurity::parse(&r.get::<_, String>(10)?),
            signature: r.get(11)?,
            settings_updated_at: r.get(12)?,
            prefs: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
            auth: r.get(14)?,
        })
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let db = self.reading_db();
        let mut st = db.prepare(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY id"
        ))?;
        let rows = st.query_map([], Self::row_to_account)?;
        let mut accounts: Vec<Account> = rows.collect::<rusqlite::Result<_>>()?;
        Self::in_chosen_order(&db, &mut accounts, |a| a.id);
        Ok(accounts)
    }

    /// The order from dragging inboxes in Settings. Any added since then go last.
    fn in_chosen_order<T>(db: &Connection, list: &mut [T], id: impl Fn(&T) -> i64) {
        let chosen: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key = 'inbox_order'",
                [],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let Some(chosen) = chosen else {
            return;
        };
        let order: Vec<i64> = chosen.split(',').filter_map(|p| p.parse().ok()).collect();
        list.sort_by_key(|item| {
            order
                .iter()
                .position(|&o| o == id(item))
                .unwrap_or(usize::MAX)
        });
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        self.reading_db()
            .query_row(
                &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1"),
                [id],
                Self::row_to_account,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("account".into()))
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        self.db()
            .execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Gives the room that deleted mail took back to the Mac. The database otherwise keeps it
    /// for new mail, so a disconnected inbox's gigabytes would never show as free. Rewriting the
    /// file takes a while for a big mailbox, so it runs in its own thread; saving new mail waits
    /// for it meanwhile, and reading carries on.
    pub fn give_room_back(&self) -> std::thread::JoinHandle<()> {
        let conn = Arc::clone(&self.conn);
        std::thread::spawn(move || {
            let db = conn.lock().unwrap_or_else(|e| e.into_inner());
            // Not worth it for a little.
            let free: i64 = db
                .query_row("PRAGMA freelist_count", [], |r| r.get(0))
                .unwrap_or(0);
            if free < ROOM_WORTH_GIVING_BACK {
                return;
            }
            let _ = db.execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            );
        })
    }

    // ---- folders ----

    /// Makes the local folder list match the server's, returning the folders
    /// with INBOX first and sent mail next.
    pub fn replace_folders(
        &self,
        account_id: i64,
        folders: &[(String, String, String)],
    ) -> Result<Vec<(i64, String, String)>> {
        let mut db = self.db();
        let tx = db.transaction()?;
        for (path, name, role) in folders {
            tx.execute(
                "INSERT INTO folders (account_id, path, name, role) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (account_id, path) DO UPDATE SET name = excluded.name, role = excluded.role",
                params![account_id, path, name, role],
            )?;
        }
        // An empty list means a broken reply, not a mailbox with no folders.
        if !folders.is_empty() {
            let keep: HashSet<&str> = folders.iter().map(|f| f.0.as_str()).collect();
            let existing: Vec<(i64, String)> = {
                let mut st = tx.prepare("SELECT id, path FROM folders WHERE account_id = ?1")?;
                let rows = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (id, path) in existing {
                if !keep.contains(path.as_str()) {
                    tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
                }
            }
        }
        let out = {
            let mut st = tx.prepare(&format!(
                "SELECT id, path, role FROM folders WHERE account_id = ?1 ORDER BY {FOLDER_ORDER}"
            ))?;
            let rows = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.commit()?;
        Ok(out)
    }

    /// One folder's role and path, without counting what's in every folder.
    pub fn folder_role_and_path(&self, folder_id: i64) -> Result<Option<(String, String)>> {
        Ok(self
            .reading_db()
            .query_row(
                "SELECT role, path FROM folders WHERE id = ?1",
                [folder_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn list_folders(&self, account_id: i64) -> Result<Vec<Folder>> {
        self.list_folders_counted(account_id, true)
    }

    /// Unread counts match the list: conversations when they're grouped, otherwise
    /// emails. An email delivered several times, to several of a person's addresses,
    /// is still one email.
    pub fn list_folders_counted(&self, account_id: i64, grouped: bool) -> Result<Vec<Folder>> {
        let db = self.reading_db();
        let mut st = db.prepare(&folder_counts_sql(grouped))?;
        let rows = st.query_map([account_id], |r| {
            Ok(Folder {
                id: r.get(0)?,
                account_id: r.get(1)?,
                path: r.get(2)?,
                name: r.get(3)?,
                role: r.get(4)?,
                last_synced_at: r.get(5)?,
                total: r.get(6)?,
                unread: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Returns the saved UIDVALIDITY and the highest UID seen.
    /// The number of the most recently saved email, or 0 when there are none.
    pub fn newest_saved_message(&self) -> Result<i64> {
        Ok(self
            .reading_db()
            .query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |r| {
                r.get(0)
            })?)
    }

    pub fn folder_state(&self, folder_id: i64) -> Result<(Option<u32>, u32)> {
        Ok(self.reading_db().query_row(
            "SELECT uid_validity, last_uid FROM folders WHERE id = ?1",
            [folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Wipes a folder when the server renumbers it, because every saved UID is
    /// now wrong. Returns how many messages were removed.
    pub fn reset_folder(&self, folder_id: i64, uid_validity: Option<u32>) -> Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let n = tx.execute("DELETE FROM messages WHERE folder_id = ?1", [folder_id])?;
        tx.execute(
            "UPDATE folders SET uid_validity = ?2, last_uid = 0, modseq = NULL, uid_next = NULL,
               server_total = NULL WHERE id = ?1",
            params![folder_id, uid_validity],
        )?;
        tx.commit()?;
        Ok(n)
    }

    pub fn mark_folder_synced(&self, folder_id: i64, uid_validity: Option<u32>) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET uid_validity = ?2, last_synced_at = unixepoch() WHERE id = ?1",
            params![folder_id, uid_validity],
        )?;
        Ok(())
    }

    // ---- messages ----

    /// Saves new messages and threads them, all or nothing.
    pub fn insert_messages(
        &self,
        account_id: i64,
        folder_id: i64,
        messages: &[IncomingMessage],
    ) -> Result<usize> {
        self.insert_messages_with(account_id, folder_id, messages, true, 0)
            .map(|added| added.len())
    }

    /// The numbers of the emails that weren't here yet, which only one of two looks can get.
    pub fn insert_new_mail(
        &self,
        account_id: i64,
        folder_id: i64,
        messages: &[IncomingMessage],
    ) -> Result<Vec<u32>> {
        self.insert_messages_with(account_id, folder_id, messages, true, 0)
    }

    /// The first look at a folder. It gets checked for showing warnings, but it isn't
    /// news, so nothing acts on it by itself.
    pub fn insert_first_look(
        &self,
        account_id: i64,
        folder_id: i64,
        messages: &[IncomingMessage],
    ) -> Result<usize> {
        self.insert_messages_with(account_id, folder_id, messages, true, 2)
            .map(|added| added.len())
    }

    /// Mail a search found. It says nothing about how far the refresh has got, so
    /// newer mail still arrives and is still announced.
    pub fn insert_found_messages(
        &self,
        account_id: i64,
        folder_id: i64,
        messages: &[IncomingMessage],
    ) -> Result<usize> {
        self.insert_messages_with(account_id, folder_id, messages, false, 1)
            .map(|added| added.len())
    }

    fn insert_messages_with(
        &self,
        account_id: i64,
        folder_id: i64,
        messages: &[IncomingMessage],
        move_position: bool,
        old_mail: i64,
    ) -> Result<Vec<u32>> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let mut added = Vec::new();
        let mut highest = 0u32;
        // Only the inbox waits for filters: they never look anywhere else.
        let in_inbox: bool = tx
            .query_row(
                "SELECT role = 'inbox' FROM folders WHERE id = ?1",
                [folder_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let held_until =
            (in_inbox && old_mail == 0).then(|| crate::identity::unix_now() + FILTER_WAIT_SECONDS);
        for m in messages {
            highest = highest.max(m.uid);
            let h = &m.headers;
            let date = m.internal_date.or(h.date).unwrap_or(0);
            let thread_id = assign_thread(&tx, account_id, folder_id, m.uid, h, date)?;
            let from = h.from.clone().unwrap_or(Addr {
                name: String::new(),
                address: String::new(),
            });
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO messages
                   (account_id, folder_id, uid, rfc_id, thread_id, subject, from_name, from_addr,
                    to_json, cc_json, date, seen, flagged, size, reply_to_json, bcc_json, base_subject,
                    category, unsubscribe, unsubscribe_one_click, auth, provider_spam, signals_done,
                    old_mail, rule_done, is_draft, held_until, purchase)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                         ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
                params![
                    account_id,
                    folder_id,
                    m.uid,
                    h.rfc_id,
                    thread_id,
                    h.subject,
                    from.name,
                    from.address,
                    json(&h.to),
                    json(&h.cc),
                    // The server's arrival time, because senders can fake Date.
                    date,
                    m.seen,
                    m.flagged,
                    m.size,
                    json(&h.reply_to),
                    json(&h.bcc),
                    parse::base_subject(&h.subject),
                    m.signals.as_ref().map_or("general", |x| x.category.as_str()),
                    m.signals.as_ref().and_then(|x| x.unsubscribe.as_deref()),
                    m.signals.as_ref().is_some_and(|x| x.one_click),
                    m.signals.as_ref().map_or("none", |x| x.auth.as_str()),
                    m.signals.as_ref().is_some_and(|x| x.provider_spam),
                    m.signals.is_some(),
                    // Older mail coming down isn't news: no filters or rules move it.
                    old_mail,
                    old_mail != 0,
                    m.is_draft,
                    // Counted from now, as the server's time can be long past or not agree.
                    held_until,
                    crate::purchases::is_purchase(&h.subject, &from.address),
                ],
            )?;
            if inserted > 0 {
                added.push(m.uid);
            }
        }
        if move_position {
            tx.execute(
                "UPDATE folders SET last_uid = MAX(last_uid, ?2) WHERE id = ?1",
                params![folder_id, highest],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// What the server last said about a folder, to tell whether anything has changed.
    pub fn folder_server_state(
        &self,
        folder_id: i64,
    ) -> Result<(Option<u64>, Option<u32>, Option<u32>)> {
        Ok(self.reading_db().query_row(
            "SELECT modseq, uid_next, server_total FROM folders WHERE id = ?1",
            [folder_id],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?.map(|m| m as u64),
                    r.get(1)?,
                    r.get(2)?,
                ))
            },
        )?)
    }

    pub fn save_folder_server_state(
        &self,
        folder_id: i64,
        modseq: Option<u64>,
        uid_next: Option<u32>,
        total: u32,
    ) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET modseq = ?2, uid_next = ?3, server_total = ?4 WHERE id = ?1",
            params![folder_id, modseq.map(|m| m as i64), uid_next, total],
        )?;
        Ok(())
    }

    /// When every message's marks were last compared, not only what the server says changed.
    pub fn flags_checked_at(&self, folder_id: i64) -> Result<i64> {
        Ok(self.db().query_row(
            "SELECT flags_checked_at FROM folders WHERE id = ?1",
            [folder_id],
            |r| r.get(0),
        )?)
    }

    pub fn note_flags_checked(&self, folder_id: i64) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET flags_checked_at = unixepoch() WHERE id = ?1",
            [folder_id],
        )?;
        Ok(())
    }

    /// A change didn't reach the server, so the next refresh has to compare everything
    /// rather than ask what changed, or this device and the server stay apart.
    pub fn forget_change_counters(&self, account_id: i64) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET modseq = NULL WHERE account_id = ?1",
            [account_id],
        )?;
        Ok(())
    }

    /// Someone chose to act on what the trial found, so it's no longer only a first look.
    pub fn first_look_is_news(&self) -> Result<()> {
        self.db()
            .execute("UPDATE messages SET old_mail = 0 WHERE old_mail = 2", [])?;
        Ok(())
    }

    pub fn note_body_tries(&self, folder_id: i64, uids: &[u32]) -> Result<()> {
        let db = self.db();
        for uid in uids {
            db.execute(
                "UPDATE messages SET body_tries = body_tries + 1 WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid],
            )?;
        }
        Ok(())
    }

    pub fn note_folder_problem(&self, folder_id: i64, problem: Option<&str>) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET last_error = ?2 WHERE id = ?1",
            params![folder_id, problem],
        )?;
        Ok(())
    }

    pub fn folder_has_problem(&self, folder_id: i64) -> Result<bool> {
        Ok(self.reading_db().query_row(
            "SELECT last_error IS NOT NULL FROM folders WHERE id = ?1",
            [folder_id],
            |r| r.get(0),
        )?)
    }

    pub fn saved_count(&self, folder_id: i64) -> Result<u32> {
        Ok(self.db().query_row(
            "SELECT COUNT(*) FROM messages WHERE folder_id = ?1",
            [folder_id],
            |r| r.get(0),
        )?)
    }

    pub fn local_uids(&self, folder_id: i64) -> Result<Vec<u32>> {
        let db = self.db();
        let mut st = db.prepare_cached("SELECT uid FROM messages WHERE folder_id = ?1")?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Mail up to `up_to` that the server no longer lists has gone from there.
    pub fn remove_gone(
        &self,
        folder_id: i64,
        on_server: &BTreeSet<u32>,
        up_to: u32,
    ) -> Result<usize> {
        let gone: Vec<u32> = self
            .local_uids(folder_id)?
            .into_iter()
            .filter(|u| *u <= up_to && !on_server.contains(u))
            .collect();
        let mut db = self.db();
        let tx = db.transaction()?;
        let mut removed = 0;
        for uid in gone {
            removed += tx.execute(
                "DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// The server's read and star marks. Only rows listed change, and read marks changed here
    /// that the server may not have had yet stay as they are.
    /// Says how many changed.
    pub fn apply_flags(
        &self,
        folder_id: i64,
        server: &[(u32, bool, bool)],
        keep_read_state: bool,
    ) -> Result<usize> {
        // Read with the database held, which a change here holds too, so neither lands between.
        let mut db = self.db();
        let held = self.held_seen_in(folder_id);
        let mut changed = 0;
        let tx = db.transaction()?;
        {
            let mut read = tx.prepare_cached(
                "UPDATE messages SET seen = ?3, flagged = ?4
                 WHERE folder_id = ?1 AND uid = ?2 AND (seen != ?3 OR flagged != ?4)",
            )?;
            let mut star = tx.prepare_cached(
                "UPDATE messages SET flagged = ?3
                 WHERE folder_id = ?1 AND uid = ?2 AND flagged != ?3",
            )?;
            for (uid, seen, flagged) in server {
                changed += if keep_read_state || held.contains(uid) {
                    star.execute(params![folder_id, uid, flagged])?
                } else {
                    read.execute(params![folder_id, uid, seen, flagged])?
                };
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// The same email in another folder may already be here. A provider that files
    /// mail under several labels would otherwise download it once for each. Only the
    /// Message-ID matching isn't enough: a scam would be read and shown with another email's words.
    pub fn copy_twin_bodies(&self, folder_id: i64, limit: i64) -> Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let twins: Vec<(i64, i64)> = {
            let mut st = tx.prepare_cached(TWIN_BODIES)?;
            let rows = st.query_map(params![folder_id, limit], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            rows.filter_map(|r| match r {
                Ok((id, Some(twin))) => Some(Ok((id, twin))),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect::<rusqlite::Result<_>>()?
        };
        for (id, twin) in &twins {
            tx.execute(
                "INSERT OR IGNORE INTO bodies (message_id, text, html, attachments_json, pgp_json)
                 SELECT ?1, text, html, attachments_json, pgp_json FROM bodies WHERE message_id = ?2",
                params![id, twin],
            )?;
            tx.execute(
                "UPDATE messages SET snippet = (SELECT snippet FROM messages WHERE id = ?2) WHERE id = ?1",
                params![id, twin],
            )?;
        }
        tx.commit()?;
        Ok(twins.len())
    }

    /// Bodies still to come, newest first, with their sizes.
    pub fn bodies_wanted(
        &self,
        folder_id: i64,
        since: i64,
        max_size: u32,
        limit: usize,
    ) -> Result<Vec<(u32, u32)>> {
        let db = self.db();
        let mut st = db.prepare_cached(BODIES_WANTED)?;
        let rows = st.query_map(params![folder_id, since, max_size, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn note_sync_try(&self, account_id: i64) -> Result<()> {
        self.db().execute(
            "INSERT INTO sync_status (account_id, last_try) VALUES (?1, unixepoch())
             ON CONFLICT (account_id) DO UPDATE SET last_try = unixepoch()",
            [account_id],
        )?;
        Ok(())
    }

    pub fn note_sync_result(&self, account_id: i64, problem: Option<&str>) -> Result<()> {
        let db = self.db();
        match problem {
            Some(text) => db.execute(
                "INSERT INTO sync_status (account_id, last_error) VALUES (?1, ?2)
                 ON CONFLICT (account_id) DO UPDATE SET last_error = ?2",
                params![account_id, text],
            )?,
            None => db.execute(
                "INSERT INTO sync_status (account_id, last_ok) VALUES (?1, unixepoch())
                 ON CONFLICT (account_id) DO UPDATE SET last_ok = unixepoch(), last_error = NULL",
                [account_id],
            )?,
        };
        Ok(())
    }

    /// Each inbox's health: when it last refreshed, what's still to come, what went wrong.
    pub fn mail_health(&self, body_max: u32, since: i64) -> Result<Vec<serde_json::Value>> {
        let db = self.reading_db();
        let mut accounts = db.prepare(
            "SELECT a.id, a.email, s.last_try, s.last_ok, s.last_error
             FROM accounts a LEFT JOIN sync_status s ON s.account_id = a.id ORDER BY a.id",
        )?;
        let mut folders = db.prepare(HEALTH_FOLDERS)?;
        #[allow(clippy::type_complexity)]
        let mut list: Vec<(i64, String, Option<i64>, Option<i64>, Option<String>)> = accounts
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Self::in_chosen_order(&db, &mut list, |inbox| inbox.0);
        let mut out = Vec::new();
        for (id, email, last_try, last_ok, last_error) in list {
            let rows: Vec<serde_json::Value> = folders
                .query_map(params![id, body_max, since], |r| {
                    let saved: i64 = r.get(6)?;
                    let total: Option<i64> = r.get(3)?;
                    Ok(serde_json::json!({
                        "id": r.get::<_, i64>(0)?,
                        "name": r.get::<_, String>(1)?,
                        "role": r.get::<_, String>(2)?,
                        "server_total": total,
                        "saved": saved,
                        "last_synced_at": r.get::<_, Option<i64>>(4)?,
                        "problem": r.get::<_, Option<String>>(5)?,
                        "bodies_to_come": r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                    }))
                })?
                .collect::<rusqlite::Result<_>>()?;
            out.push(serde_json::json!({
                "account_id": id,
                "email": email,
                "last_try": last_try,
                "last_ok": last_ok,
                "problem": last_error,
                "folders": rows,
            }));
        }
        Ok(out)
    }

    /// How much room the saved mail takes on this device, in bytes.
    pub fn database_size(&self) -> Result<i64> {
        Ok(self.reading_db().query_row(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn lowest_uid(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self.db().query_row(
            "SELECT MIN(uid) FROM messages WHERE folder_id = ?1",
            [folder_id],
            |r| r.get(0),
        )?)
    }

    /// Applies the server's flags for UIDs from `low` to `high`. Any saved
    /// message in that range the server did not list has been deleted there.
    pub fn reconcile(
        &self,
        folder_id: i64,
        low: u32,
        high: u32,
        server: &[(u32, bool, bool)],
        keep_read_state: bool,
    ) -> Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let present: BTreeSet<u32> = server.iter().map(|s| s.0).collect();
        for (uid, seen, flagged) in server {
            if keep_read_state {
                tx.execute(
                    "UPDATE messages SET flagged = ?3 WHERE folder_id = ?1 AND uid = ?2",
                    params![folder_id, uid, flagged],
                )?;
            } else {
                tx.execute(
                    "UPDATE messages SET seen = ?3, flagged = ?4 WHERE folder_id = ?1 AND uid = ?2",
                    params![folder_id, uid, seen, flagged],
                )?;
            }
        }
        let local: Vec<u32> = {
            let mut st = tx.prepare(
                "SELECT uid FROM messages WHERE folder_id = ?1 AND uid BETWEEN ?2 AND ?3",
            )?;
            let rows = st.query_map(params![folder_id, low, high], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut removed = 0;
        for uid in local {
            if !present.contains(&uid) {
                removed += tx.execute(
                    "DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2",
                    params![folder_id, uid],
                )?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Newest messages in a folder with no saved body yet, for offline reading.
    pub fn uids_missing_body(
        &self,
        folder_id: i64,
        since: i64,
        max_size: u32,
        limit: usize,
    ) -> Result<Vec<u32>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT m.uid FROM messages m LEFT JOIN bodies b ON b.message_id = m.id
             WHERE m.folder_id = ?1 AND b.message_id IS NULL AND m.date >= ?2 AND m.size <= ?3
             ORDER BY m.date DESC LIMIT ?4",
        )?;
        let rows = st.query_map(params![folder_id, since, max_size, limit as i64], |r| {
            r.get(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_gone_one(&self, folder_id: i64, uid: u32) -> Result<()> {
        self.db().execute(
            "DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2",
            params![folder_id, uid],
        )?;
        Ok(())
    }

    pub fn save_body_by_uid(&self, folder_id: i64, uid: u32, body: &Body) -> Result<()> {
        let id: Option<i64> = self
            .db()
            .query_row(
                "SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.save_body(id, body),
            None => Ok(()),
        }
    }

    pub fn save_body(&self, message_id: i64, body: &Body) -> Result<()> {
        let snippet = parse::snippet(body.text.as_deref().unwrap_or(""));
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO bodies (message_id, text, html, attachments_json, pgp_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message_id,
                body.text,
                body.html,
                json(&body.attachments),
                body.pgp.as_ref().map(json)
            ],
        )?;
        tx.execute(
            "UPDATE messages SET snippet = ?2 WHERE id = ?1",
            params![message_id, snippet],
        )?;
        save_facts(&tx, message_id, body.html.as_deref())?;
        note_way_out(&tx, message_id, body.html.as_deref())?;
        tx.commit()?;
        Ok(())
    }

    /// Unsubscribe links in mail saved before they were read from the email itself, newest
    /// first, a batch at a time after each refresh.
    pub(crate) fn fill_some_ways_out(&self, most: i64) -> Result<()> {
        if self.setting("ways_out_done")?.as_deref() == Some("1") {
            return Ok(());
        }
        let before: i64 = self
            .setting("ways_out_before")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(i64::MAX);
        let found: Vec<(i64, String)> = {
            let db = self.reading_db();
            let mut st = db.prepare(
                "SELECT b.message_id, b.html FROM bodies b JOIN messages m ON m.id = b.message_id
                 WHERE b.message_id < ?1 AND m.unsubscribe IS NULL AND m.auth = 'pass'
                   AND b.html LIKE '%unsubscribe%'
                 ORDER BY b.message_id DESC LIMIT ?2",
            )?;
            let rows = st.query_map([before, most], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        {
            let mut db = self.db();
            let tx = db.transaction()?;
            for (id, html) in &found {
                note_way_out(&tx, *id, Some(html))?;
            }
            tx.commit()?;
        }
        match found.last() {
            Some((id, _)) if found.len() as i64 == most => {
                self.set_setting("ways_out_before", &id.to_string())
            }
            _ => self.set_setting("ways_out_done", "1"),
        }
    }

    /// Facts for mail saved before facts were read, a batch at a time after each refresh,
    /// so a question never waits for every email to be read.
    pub(crate) fn fill_some_facts(&self, most: i64) -> Result<()> {
        if self.setting("facts_filled_2")?.as_deref() == Some("1") {
            return Ok(());
        }
        let after: i64 = self
            .setting("facts_filled_2_after")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let found: Vec<(i64, String)> = {
            let db = self.reading_db();
            let mut st = db.prepare(
                "SELECT message_id, html FROM bodies
                 WHERE message_id > ?1 AND html LIKE '%ld+json%' ORDER BY message_id LIMIT ?2",
            )?;
            let rows = st.query_map([after, most], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        {
            let mut db = self.db();
            let tx = db.transaction()?;
            for (id, html) in &found {
                save_facts(&tx, *id, Some(html))?;
            }
            tx.commit()?;
        }
        // The lock is let go first: saving the mark takes it again.
        match found.last() {
            Some((id, _)) if found.len() as i64 == most => {
                self.set_setting("facts_filled_2_after", &id.to_string())
            }
            _ => self.set_setting("facts_filled_2", "1"),
        }
    }

    /// Facts of one kind happening between two times, soonest first or newest first. Only
    /// from mail whose sender passed the provider's check: anyone can write booking markup.
    pub(crate) fn facts_of(
        &self,
        kind: &str,
        after: Option<i64>,
        before: Option<i64>,
        newest_first: bool,
    ) -> Result<Vec<(i64, String, serde_json::Value)>> {
        let db = self.reading_db();
        let mut st = db.prepare(&format!(
            "SELECT message_id, place, value_json FROM facts
             WHERE kind = ?1 AND (?2 IS NULL OR starts_at >= ?2) AND (?3 IS NULL OR starts_at < ?3)
               AND message_id IN (SELECT id FROM messages WHERE auth = 'pass')
             ORDER BY starts_at {} LIMIT 50",
            if newest_first { "DESC" } else { "ASC" }
        ))?;
        let rows = st.query_map(params![kind, after, before], |r| {
            let value: String = r.get(2)?;
            Ok((
                r.get(0)?,
                r.get(1)?,
                serde_json::from_str(&value).unwrap_or_default(),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn body(&self, message_id: i64) -> Result<Option<Body>> {
        Ok(self
            .db()
            .query_row(
                "SELECT text, html, attachments_json, pgp_json FROM bodies WHERE message_id = ?1",
                [message_id],
                row_to_body,
            )
            .optional()?)
    }

    pub fn message_location(&self, message_id: i64) -> Result<MessageLocation> {
        self.db()
            .query_row(
                "SELECT m.account_id, f.path, f.uid_validity, m.uid
                 FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
                [message_id],
                |r| {
                    Ok(MessageLocation {
                        account_id: r.get(0)?,
                        folder_path: r.get(1)?,
                        uid_validity: r.get(2)?,
                        uid: r.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("message".into()))
    }

    // ---- reading ----

    /// Conversations with a message in this folder. Each one counts its whole
    /// conversation, such as your replies in Sent, but not copies in the bin or
    /// spam unless that is the folder being viewed.
    pub fn list_threads(
        &self,
        folder_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ThreadSummary>> {
        self.list_threads_as(folder_id, limit, offset, true)
    }

    /// With `grouped` off, each email is its own row, named `msg:<id>`.
    pub fn list_threads_as(
        &self,
        folder_id: i64,
        limit: i64,
        offset: i64,
        grouped: bool,
    ) -> Result<Vec<ThreadSummary>> {
        self.list_threads_on_tab(folder_id, limit, offset, grouped, None)
    }

    pub fn list_threads_on_tab(
        &self,
        folder_id: i64,
        limit: i64,
        offset: i64,
        grouped: bool,
        tab: Option<&str>,
    ) -> Result<Vec<ThreadSummary>> {
        self.list_threads_filtered(folder_id, limit, offset, grouped, tab, false)
    }

    /// `unread_only` leaves out anything already read.
    pub fn list_threads_filtered(
        &self,
        folder_id: i64,
        limit: i64,
        offset: i64,
        grouped: bool,
        tab: Option<&str>,
        unread_only: bool,
    ) -> Result<Vec<ThreadSummary>> {
        let page = self.page_keys(folder_id, limit, offset, grouped, tab, unread_only)?;
        let keys: Vec<SqlValue> = page.into_iter().map(|k| k.key).collect();
        self.fill_summaries(folder_id, grouped, &keys)
    }

    /// Which rows a page of a folder's list holds, in order, with what they're sorted by,
    /// without filling them in. Cheap next to filling them in, which is done only for the
    /// rows really shown.
    fn page_keys(
        &self,
        folder_id: i64,
        limit: i64,
        offset: i64,
        grouped: bool,
        tab: Option<&str>,
        unread_only: bool,
    ) -> Result<Vec<PageKey>> {
        let db = self.reading_db();
        let role: String =
            db.query_row("SELECT role FROM folders WHERE id = ?1", [folder_id], |r| {
                r.get(0)
            })?;
        let key = |r: &rusqlite::Row| {
            Ok(PageKey {
                key: r.get(0)?,
                pinned: r.get(1)?,
                when: r.get(2)?,
            })
        };
        // Recent conversations first. Enough of them is the whole answer, and quick.
        let newest: Option<i64> = db.query_row(
            "SELECT MAX(date) FROM messages WHERE folder_id = ?1",
            [folder_id],
            |r| r.get(0),
        )?;
        if let Some(newest) = newest {
            let mut st =
                db.prepare_cached(&page_keys_sql_from(&role, grouped, tab, unread_only, true))?;
            // Needs you is all recent anyway, so one look back past its month is the answer.
            let needs_you = tab == Some("needs_you");
            let now = chrono::Utc::now().timestamp();
            for back in RECENT_LOOKS {
                let since = if needs_you {
                    (newest - back).min(now - NEEDS_YOU_SECONDS - 86_400)
                } else {
                    newest - back
                };
                let rows = st
                    .query_map(params![folder_id, limit, offset, since], key)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if needs_you || rows.len() as i64 >= limit {
                    return Ok(rows);
                }
            }
        }
        let mut st = db.prepare_cached(&page_keys_sql(&role, grouped, tab, unread_only))?;
        let rows = st.query_map(params![folder_id, limit, offset], key)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The rows of a list, filled in, in the order given.
    fn fill_summaries(
        &self,
        folder_id: i64,
        grouped: bool,
        page: &[SqlValue],
    ) -> Result<Vec<ThreadSummary>> {
        if page.is_empty() {
            return Ok(vec![]);
        }
        let db = self.reading_db();
        const ACCOUNT: &str = "(SELECT account_id FROM folders WHERE id = ?1)";
        let marks = vec!["?"; page.len()].join(",");
        let mut values = vec![SqlValue::Integer(folder_id)];
        values.extend(page.iter().cloned());
        let sql = if grouped {
            summary_sql(
                "m.thread_id",
                &format!(
                    "m.account_id = {ACCOUNT} AND m.thread_id IN ({marks}) AND m.id NOT IN ({HELD})
                     AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?1) AND (m.is_draft = 0 OR f.id = ?1)"
                ),
                "GROUP BY m.thread_id",
            )
        } else {
            summary_sql(
                "'msg:' || m.id",
                &format!("?1 IS NOT NULL AND m.id IN ({marks})"),
                "GROUP BY m.id",
            )
        };
        let mut st = db.prepare(&sql)?;
        let found: Vec<ThreadSummary> = st
            .query_map(params_from_iter(values), |r| read_summary(r, folder_id))?
            .collect::<rusqlite::Result<_>>()?;
        // Back in the order chosen above.
        let mut order: std::collections::HashMap<String, usize> = Default::default();
        for (i, v) in page.iter().enumerate() {
            let name = match v {
                SqlValue::Text(t) => t.clone(),
                SqlValue::Integer(n) => format!("msg:{n}"),
                _ => continue,
            };
            order.insert(name, i);
        }
        let mut found = found;
        found.sort_by_key(|t| order.get(&t.thread_id).copied().unwrap_or(usize::MAX));
        Ok(found)
    }

    /// Conversations with a match, newest first, and how many more are in the bin or
    /// spam when those are left out. `extra` is mail a server said matches.
    pub fn search_threads(
        &self,
        f: &SearchFilters,
        extra: &[i64],
        limit: i64,
        grouped: bool,
    ) -> Result<(Vec<ThreadSummary>, i64)> {
        self.search_threads_where(f, extra, limit, grouped, |_| true)
    }

    /// The same, but only what `keep` lets through counts towards the limit. The store is
    /// locked while this runs, so `keep` can't ask it anything.
    // ponytail: only sees the newest 2000 matching emails, or 8 for each conversation asked for,
    // so older allowed mail is missed if all of those are held back. Filter whole accounts in the
    // query if that ever happens.
    pub fn search_threads_where(
        &self,
        f: &SearchFilters,
        extra: &[i64],
        limit: i64,
        grouped: bool,
        keep: impl Fn(&ThreadSummary) -> bool,
    ) -> Result<(Vec<ThreadSummary>, i64)> {
        let mut wheres: Vec<String> = Vec::new();
        let mut values: Vec<SqlValue> = Vec::new();
        let like = |text: &str| {
            let safe = text
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            SqlValue::Text(format!("%{safe}%"))
        };
        let mut matches: Vec<String> = Vec::new();
        if let Some(query) = crate::search::index_query(f) {
            matches.push(
                "m.id IN (SELECT rowid FROM search_index WHERE search_index MATCH ?)".to_string(),
            );
            values.push(SqlValue::Text(query));
        }
        if !extra.is_empty() {
            let list: Vec<String> = extra.iter().map(i64::to_string).collect();
            matches.push(format!("m.id IN ({})", list.join(",")));
        }
        if !matches.is_empty() {
            wheres.push(format!("({})", matches.join(" OR ")));
        } else if f.is_empty() {
            return Ok((vec![], 0));
        }
        if !f.from.is_empty() {
            let any = vec![
                "(m.from_name LIKE ? ESCAPE '\\' OR m.from_addr LIKE ? ESCAPE '\\')";
                f.from.len()
            ];
            wheres.push(format!("({})", any.join(" OR ")));
            for who in &f.from {
                values.push(like(who));
                values.push(like(who));
            }
        }
        if !f.to.is_empty() {
            let any =
                vec!["(m.to_json LIKE ? ESCAPE '\\' OR m.cc_json LIKE ? ESCAPE '\\')"; f.to.len()];
            wheres.push(format!("({})", any.join(" OR ")));
            for who in &f.to {
                values.push(like(who));
                values.push(like(who));
            }
        }
        if !f.with.is_empty() {
            let any = vec![
                "(m.from_addr LIKE ? ESCAPE '\\' OR m.to_json LIKE ? ESCAPE '\\' OR m.cc_json LIKE ? ESCAPE '\\')";
                f.with.len()
            ];
            wheres.push(format!("({})", any.join(" OR ")));
            for who in &f.with {
                values.push(like(who));
                values.push(like(who));
                values.push(like(who));
            }
        }
        if let Some(after) = f.after {
            wheres.push("m.date >= ?".into());
            values.push(SqlValue::Integer(after));
        }
        if let Some(before) = f.before {
            wheres.push("m.date < ?".into());
            values.push(SqlValue::Integer(before));
        }
        if f.unread {
            wheres.push("m.seen = 0".into());
        }
        if f.starred {
            wheres.push("m.flagged = 1".into());
        }
        if let Some(account) = f.account {
            wheres.push("m.account_id = ?".into());
            values.push(SqlValue::Integer(account));
        }
        if f.has_attachment {
            wheres.push(
                "(COALESCE(b.attachments_json, '[]') LIKE '%\"inline\":false%'
                  OR (COALESCE(b.attachments_json, '[]') != '[]'
                      AND b.attachments_json NOT LIKE '%\"inline\":%'))"
                    .into(),
            );
        }
        let mut shown = wheres.clone();
        let mut shown_values = values.clone();
        match &f.folder {
            Some(role) => {
                shown.push("f.role = ?".into());
                shown_values.push(SqlValue::Text(role.clone()));
            }
            None if f.include_bin => shown.push("f.role != 'drafts' AND m.is_draft = 0".into()),
            None => {
                shown.push("f.role NOT IN ('trash', 'junk', 'drafts') AND m.is_draft = 0".into())
            }
        }
        const FROM: &str = "FROM messages m JOIN folders f ON f.id = m.folder_id
                            LEFT JOIN bodies b ON b.message_id = m.id";
        let db = self.reading_db();
        let mut st = db.prepare(&format!(
            "SELECT m.id, m.account_id, m.thread_id, m.folder_id, COALESCE(m.rfc_id, '#' || m.id) {FROM}
             WHERE {} ORDER BY m.date DESC LIMIT {}",
            shown.join(" AND "),
            (limit * 8).max(2000)
        ))?;
        let hits: Vec<(i64, i64, String, i64, String)> = st
            .query_map(params_from_iter(shown_values), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        // The index also holds old replies quoted under an email, so each word match is checked
        // against the email's own words. Mail a server found is already a match.
        let check_words = crate::search::index_query(f).is_some();
        for (id, account_id, thread_id, folder_id, email) in hits {
            if out.len() as i64 >= limit {
                break;
            }
            let row = if grouped { thread_id.clone() } else { email };
            // A conversation already shown needs no second look at its words.
            if seen.contains(&(account_id, row.clone())) {
                continue;
            }
            if check_words && !extra.contains(&id) && !own_words_match(&db, id, f)? {
                continue;
            }
            seen.insert((account_id, row));
            let summary = if grouped {
                db.query_row(
                    &summary_sql(
                        "m.thread_id",
                        "m.account_id = ?1 AND m.thread_id = ?2
                         AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?3) AND (m.is_draft = 0 OR f.id = ?3)",
                        "GROUP BY m.thread_id",
                    ),
                    params![account_id, thread_id, folder_id],
                    |r| read_summary(r, folder_id),
                )
            } else {
                db.query_row(
                    &summary_sql("'msg:' || m.id", "m.id = ?1", "GROUP BY m.id"),
                    [id],
                    |r| read_summary(r, folder_id),
                )
            };
            if let Some(found) = summary.optional()?.filter(&keep) {
                out.push(found);
            }
        }
        let in_bin = if f.folder.is_none() && !f.include_bin {
            let mut hidden = wheres;
            hidden.push("f.role IN ('trash', 'junk')".into());
            db.query_row(
                &format!(
                    "SELECT COUNT(DISTINCT m.account_id || ':' || m.thread_id) {FROM} WHERE {}",
                    hidden.join(" AND ")
                ),
                params_from_iter(values),
                |r| r.get(0),
            )?
        } else {
            0
        };
        mark_best_matches(&mut out, f);
        Ok((out, in_bin))
    }

    /// A conversation's summary again, made only from its emails in these folders. None if
    /// it has none there.
    pub fn thread_summary_within(
        &self,
        t: &ThreadSummary,
        folders: &HashSet<i64>,
    ) -> Result<Option<ThreadSummary>> {
        let list: Vec<String> = folders.iter().map(i64::to_string).collect();
        let summary = self
            .db()
            .query_row(
                &summary_sql(
                    "m.thread_id",
                    &format!(
                        "m.account_id = ?1 AND m.thread_id = ?2 AND m.folder_id IN ({})
                         AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?3) AND (m.is_draft = 0 OR f.id = ?3)",
                        list.join(",")
                    ),
                    "GROUP BY m.thread_id",
                ),
                params![t.account_id, t.thread_id, t.folder_id],
                |r| read_summary(r, t.folder_id),
            )
            .optional()?;
        Ok(summary)
    }

    /// Unread on each tab of an inbox, adding up to the number in the sidebar.
    pub fn tab_counts(&self, folder_id: i64, grouped: bool) -> Result<BTreeMap<String, i64>> {
        let db = self.reading_db();
        let group = if grouped {
            "m.thread_id"
        } else {
            "COALESCE(m.rfc_id, '#' || m.id)"
        };
        let (scope, here) = if grouped {
            (
                format!("m.account_id = (SELECT account_id FROM folders WHERE id = ?1)
                 AND m.thread_id IN (SELECT thread_id FROM messages WHERE folder_id = ?1 AND seen = 0 AND screened = 0)
                 AND m.id NOT IN ({HELD})
                 AND m.thread_id NOT IN (SELECT thread_id FROM thread_marks
                     WHERE account_id = (SELECT account_id FROM folders WHERE id = ?1)
                       AND (muted = 1 OR snooze_until IS NOT NULL))
                 AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?1) AND (m.is_draft = 0 OR f.id = ?1)"),
                "SUM(m.folder_id = ?1 AND m.seen = 0) > 0",
            )
        } else {
            (
                format!(
                    "m.folder_id = ?1 AND m.seen = 0 AND m.screened = 0
                 AND m.id NOT IN ({HELD})
                 AND m.thread_id NOT IN (SELECT thread_id FROM thread_marks
                     WHERE account_id = (SELECT account_id FROM folders WHERE id = ?1)
                       AND (muted = 1 OR snooze_until IS NOT NULL))"
                ),
                "1",
            )
        };
        let mut st = db.prepare(&format!(
            "SELECT SUM(f.role = 'sent' OR (f.role = 'inbox' AND m.category = 'general')) > 0,
                    SUM(f.role = 'inbox' AND m.category = 'newsletters') > 0,
                    {NEEDS_YOU_SQL}
             FROM messages m JOIN folders f ON f.id = m.folder_id
             LEFT JOIN thread_marks k ON k.account_id = m.account_id AND k.thread_id = m.thread_id
             WHERE {scope} GROUP BY {group} HAVING {here}"
        ))?;
        let rows = st.query_map([folder_id], |r| {
            Ok((
                r.get::<_, bool>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?;
        let mut counts: BTreeMap<String, i64> = [
            "general",
            "newsletters",
            "promotions",
            "good_to_know",
            "rest",
        ]
        .iter()
        .map(|t| (t.to_string(), 0))
        .collect();
        for row in rows {
            let (general, news, needs) = row?;
            if !needs {
                *counts
                    .entry(if general { "good_to_know" } else { "rest" }.to_string())
                    .or_default() += 1;
            }
            let tab = if general {
                "general"
            } else if news {
                "newsletters"
            } else {
                "promotions"
            };
            *counts.entry(tab.to_string()).or_default() += 1;
        }
        Ok(counts)
    }

    pub fn inbox_folder_ids(&self) -> Result<Vec<(i64, String)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT f.id, a.prefs_json FROM folders f JOIN accounts a ON a.id = f.account_id
             WHERE f.role = 'inbox' ORDER BY f.id",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn uids_missing_signals(&self, folder_id: i64, limit: i64) -> Result<Vec<u32>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT uid FROM messages WHERE folder_id = ?1 AND signals_done = 0
             ORDER BY date DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![folder_id, limit], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_signals(&self, folder_id: i64, uid: u32, x: &crate::spam::Signals) -> Result<()> {
        self.db().execute(
            // Headers with no way out keep one found in the email itself.
            "UPDATE messages SET category = CASE WHEN tab_chosen IS NULL THEN ?3 ELSE category END,
                    unsubscribe = COALESCE(?4, unsubscribe),
                    unsubscribe_one_click = CASE WHEN ?4 IS NULL THEN unsubscribe_one_click ELSE ?5 END,
                    auth = ?6, provider_spam = ?7, signals_done = 1
             WHERE folder_id = ?1 AND uid = ?2",
            params![
                folder_id,
                uid,
                x.category,
                x.unsubscribe,
                x.one_click,
                x.auth,
                x.provider_spam
            ],
        )?;
        Ok(())
    }

    /// Everyone this person has written to, from their sent mail.
    pub fn note_contacts(&self, account_id: i64) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO contacts (address)
             SELECT lower(json_extract(person.value, '$.address'))
             FROM messages m JOIN folders f ON f.id = m.folder_id,
                  json_each(m.to_json) AS person
             WHERE f.account_id = ?1 AND f.role = 'sent'
               AND json_extract(person.value, '$.address') IS NOT NULL",
            [account_id],
        )?;
        Ok(())
    }

    pub fn written_to(&self) -> Result<BTreeSet<String>> {
        let db = self.db();
        let mut st = db.prepare("SELECT address FROM contacts")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn contact_domains(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .written_to()?
            .iter()
            .filter_map(|a| a.rsplit_once('@').map(|(_, d)| d.to_string()))
            .collect())
    }

    /// Inbox mail nobody has checked yet, newest first.
    pub fn mail_to_check(&self, account_id: i64, limit: i64) -> Result<Vec<MailToCheck>> {
        let db = self.db();
        let mut st = db.prepare_cached(&mail_to_check_sql())?;
        let rows = st.query_map(params![account_id, limit], row_to_check)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Unread inbox mail AI hasn't sorted yet, leaving out anything that looks like spam.
    pub fn mail_to_sort(
        &self,
        account_id: i64,
        limit: i64,
        only_important: bool,
    ) -> Result<Vec<MailToCheck>> {
        let db = self.db();
        // Mail the simple sorting is sure doesn't matter isn't worth spending the person's limits on.
        let which = if only_important {
            "AND (m.priority_unsure = 1 OR m.priority = 'needs_you')"
        } else {
            ""
        };
        let mut st = db.prepare(&format!(
            "SELECT {CHECK_COLUMNS} FROM messages m
             JOIN folders f ON f.id = m.folder_id
             LEFT JOIN bodies b ON b.message_id = m.id
             JOIN verdicts v ON v.message_key = m.message_key
             WHERE m.account_id = ?1 AND f.role = 'inbox' AND m.seen = 0 AND m.old_mail = 0
               AND m.ai_checked = 0 AND v.risk < 2 {which}
             ORDER BY m.date DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(params![account_id, limit], row_to_check)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_ai_sort(&self, id: i64, tab: Option<&str>, label: Option<&str>) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET category = COALESCE(CASE WHEN tab_chosen IS NULL THEN ?2 END, category),
                    ai_label = ?3 WHERE id = ?1",
            params![id, tab, label],
        )?;
        Ok(())
    }

    /// Quick replies kept for an email, if they were written already.
    pub fn quick_replies(&self, message_id: i64) -> Result<Option<Vec<String>>> {
        let json: Option<String> = self
            .reading_db()
            .query_row(
                "SELECT replies_json FROM quick_replies WHERE message_id = ?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    pub fn save_quick_replies(&self, message_id: i64, replies: &[String]) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO quick_replies (message_id, replies_json) VALUES (?1, ?2)",
            params![
                message_id,
                serde_json::to_string(replies).unwrap_or_default()
            ],
        )?;
        Ok(())
    }

    /// Recent emails in the inbox that need this person, newest in their conversation,
    /// from someone else, whose quick replies aren't written yet. Newest first.
    pub fn mail_needing_quick_replies(&self, account_id: i64, limit: i64) -> Result<Vec<i64>> {
        let db = self.reading_db();
        let mut st = db.prepare_cached(
            "SELECT m.id FROM messages m
             JOIN folders f ON f.id = m.folder_id
             JOIN accounts a ON a.id = m.account_id
             WHERE m.account_id = ?1 AND f.role = 'inbox' AND m.priority = 'needs_you'
               AND m.date > CAST(strftime('%s', 'now') AS INTEGER) - ?3
               AND lower(COALESCE(m.from_addr, '')) <> lower(a.email)
               AND NOT EXISTS (SELECT 1 FROM quick_replies q WHERE q.message_id = m.id)
               AND NOT EXISTS (SELECT 1 FROM messages n WHERE n.account_id = m.account_id
                               AND n.thread_id = m.thread_id AND n.date > m.date)
             ORDER BY m.date DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![account_id, limit, QUICK_REPLIES_FOR_SECS], |r| {
            r.get(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn mark_ai_checked(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE messages SET ai_checked = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Mass mail in the inbox not yet given a tab. Mail that went back to the
    /// inbox was checked for spam long ago, so that step won't pick it up.
    pub fn mass_mail_to_sort(&self, account_id: i64) -> Result<Vec<(i64, String, String)>> {
        let db = self.db();
        let mut st = db.prepare_cached(MASS_MAIL_TO_SORT)?;
        let rows = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A filter's choice of tab, which sorting afterwards leaves alone.
    pub fn choose_tab(&self, id: i64, category: &str) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET category = ?2, tab_chosen = 1 WHERE id = ?1",
            params![id, category],
        )?;
        Ok(())
    }

    /// Sorting's own choice of tab, unless a filter already chose one.
    pub fn sort_into_tab(&self, id: i64, category: &str) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET category = ?2 WHERE id = ?1 AND tab_chosen IS NULL",
            params![id, category],
        )?;
        Ok(())
    }

    pub fn set_category(&self, id: i64, category: &str) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET category = ?2 WHERE id = ?1",
            params![id, category],
        )?;
        Ok(())
    }

    pub fn set_sender_category(&self, address: &str, category: &str) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET category = ?2 WHERE lower(from_addr) = ?1",
            params![address, category],
        )?;
        Ok(())
    }

    pub fn save_verdict(
        &self,
        key: &str,
        score: i64,
        risk: i64,
        reasons: &[crate::spam::Reason],
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO verdicts (message_key, score, risk, reasons_json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (message_key) DO UPDATE SET score = ?2, risk = ?3, reasons_json = ?4",
            params![key, score, risk, json(&reasons)],
        )?;
        Ok(())
    }

    pub fn has_verdict(&self, key: &str) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM verdicts WHERE message_key = ?1",
                [key],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// What the filter's opinion of this email is saved under, and the conversation it's in.
    pub fn key_and_thread(&self, folder_id: i64, uid: u32) -> Result<(String, String)> {
        Ok(self.db().query_row(
            "SELECT message_key, thread_id FROM messages WHERE folder_id = ?1 AND uid = ?2",
            params![folder_id, uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Warnings kept because of a link, one row for each copy of the email they were worked
    /// out from: the key, score, reasons, the copy's html, sender and sender check.
    #[allow(clippy::type_complexity)]
    pub(crate) fn link_warnings(
        &self,
    ) -> Result<Vec<(String, i64, String, String, String, String)>> {
        let db = self.reading_db();
        let mut st = db.prepare(
            "SELECT v.message_key, v.score, v.reasons_json, COALESCE(b.html, ''), m.from_addr,
                    COALESCE(m.auth, '')
             FROM verdicts v JOIN messages m ON m.message_key = v.message_key
             LEFT JOIN bodies b ON b.message_id = m.id
             WHERE v.risk = 1 AND v.reasons_json LIKE '%A link shows%'
             ORDER BY v.message_key",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn verdict_risk(&self, key: &str) -> Result<i64> {
        Ok(self
            .db()
            .query_row(
                "SELECT risk FROM verdicts WHERE message_key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn mark_auto_moved(&self, key: &str) -> Result<()> {
        self.db().execute(
            "UPDATE verdicts SET auto_moved = 1 WHERE message_key = ?1",
            [key],
        )?;
        Ok(())
    }

    /// Spam waiting to go to Junk. Anything already moved once stays where the person
    /// put it. Only blocked senders go unless everything is turned on.
    pub fn spam_to_move(&self, account_id: i64, everything: bool) -> Result<Vec<MailToCheck>> {
        let db = self.db();
        let which = if everything {
            "(v.score = 100 OR (v.risk = 2 AND m.old_mail = 0))"
        } else {
            "v.score = 100"
        };
        let mut st = db.prepare(&format!(
            "SELECT {CHECK_COLUMNS} FROM messages m
             JOIN folders f ON f.id = m.folder_id
             LEFT JOIN bodies b ON b.message_id = m.id
             JOIN verdicts v ON v.message_key = m.message_key
             WHERE m.account_id = ?1 AND f.role = 'inbox' AND v.auto_moved = 0 AND {which}"
        ))?;
        let rows = st.query_map([account_id], row_to_check)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn thread_mail(
        &self,
        account_id: i64,
        thread_id: &str,
        folder_id: i64,
    ) -> Result<Vec<MailToCheck>> {
        let copies = self.thread_copies(account_id, thread_id)?;
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {CHECK_COLUMNS} FROM messages m
             JOIN folders f ON f.id = m.folder_id
             LEFT JOIN bodies b ON b.message_id = m.id WHERE m.id = ?1"
        ))?;
        let mut out = Vec::new();
        for copy in copies.iter().filter(|c| c.folder_id == folder_id) {
            out.push(st.query_row([copy.id], row_to_check)?);
        }
        Ok(out)
    }

    pub fn folder_role(&self, folder_id: i64) -> Result<String> {
        Ok(self
            .db()
            .query_row("SELECT role FROM folders WHERE id = ?1", [folder_id], |r| {
                r.get(0)
            })?)
    }

    fn mail_to_learn(
        &self,
        account_id: i64,
        condition: &str,
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let db = self.db();
        let mut st = db.prepare_cached(&learn_sql(condition))?;
        let rows = st.query_map(params![account_id, limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn junk_to_learn(&self, account_id: i64, limit: i64) -> Result<Vec<(String, String)>> {
        self.mail_to_learn(account_id, LEARN_FROM_JUNK, limit)
    }

    pub fn good_to_learn(&self, account_id: i64, limit: i64) -> Result<Vec<(String, String)>> {
        self.mail_to_learn(account_id, LEARN_FROM_GOOD, limit)
    }

    /// Adds an example, moving it across if it was first learned the other way.
    pub fn learn(&self, key: &str, words: &BTreeSet<String>, spam: bool) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let before: Option<bool> = tx
            .query_row(
                "SELECT spam FROM spam_trained WHERE message_key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        if before == Some(spam) {
            return Ok(());
        }
        let total = crate::spam::TOTALS_WORD;
        for word in words.iter().map(String::as_str).chain([total]) {
            let (add_spam, add_good) = match (before, spam) {
                (Some(_), true) => (1, -1),
                (Some(_), false) => (-1, 1),
                (None, true) => (1, 0),
                (None, false) => (0, 1),
            };
            tx.execute(
                "INSERT INTO spam_words (word, spam, good) VALUES (?1, MAX(?2, 0), MAX(?3, 0))
                 ON CONFLICT (word) DO UPDATE
                 SET spam = MAX(spam + ?2, 0), good = MAX(good + ?3, 0)",
                params![word, add_spam, add_good],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO spam_trained (message_key, spam) VALUES (?1, ?2)",
            params![key, spam],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn learned_totals(&self) -> Result<(i64, i64)> {
        Ok(self
            .db()
            .query_row(
                "SELECT spam, good FROM spam_words WHERE word = ?1",
                [crate::spam::TOTALS_WORD],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, 0)))
    }

    pub fn word_counts(&self, words: &BTreeSet<String>) -> Result<Vec<(i64, i64)>> {
        let db = self.db();
        let mut st = db.prepare_cached("SELECT spam, good FROM spam_words WHERE word = ?1")?;
        let mut out = Vec::new();
        for word in words {
            if let Some(found) = st
                .query_row([word], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
            {
                out.push(found);
            }
        }
        Ok(out)
    }

    /// The most-seen words, always including the totals they're measured against.
    pub fn top_words(&self, limit: i64) -> Result<Vec<(String, i64, i64)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT word, spam, good FROM spam_words ORDER BY word = ?1 DESC, spam + good DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![crate::spam::TOTALS_WORD, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn merge_words(&self, words: &[(String, i64, i64)]) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        for (word, spam, good) in words {
            tx.execute(
                "INSERT INTO spam_words (word, spam, good) VALUES (?1, ?2, ?3)
                 ON CONFLICT (word) DO UPDATE SET spam = MAX(spam, ?2), good = MAX(good, ?3)",
                params![word, spam, good],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// What the filter thinks is spam, still in an inbox.
    pub fn spam_report(&self, limit: i64) -> Result<Vec<serde_json::Value>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.subject, v.reasons_json, v.score
             FROM messages m JOIN folders f ON f.id = m.folder_id
             JOIN verdicts v ON v.message_key = m.message_key
             WHERE f.role = 'inbox' AND v.risk = 2
             GROUP BY m.message_key
             ORDER BY m.date DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit], |r| {
            let id: i64 = r.get(0)?;
            let reasons: Vec<crate::spam::Reason> = parse_json(r.get::<_, String>(6)?);
            Ok(serde_json::json!({
                "thread_id": format!("msg:{id}"),
                "account_id": r.get::<_, i64>(1)?,
                "folder_id": r.get::<_, i64>(2)?,
                "from_name": r.get::<_, String>(3)?,
                "from_address": r.get::<_, String>(4)?,
                "subject": r.get::<_, String>(5)?,
                "reasons": reasons.into_iter().map(|x| x.text).collect::<Vec<_>>(),
                "score": r.get::<_, i64>(7)?,
            }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn unsubscribe_link(&self, message_id: i64) -> Result<crate::newsletters::WayOut> {
        Ok(self.db().query_row(
            "SELECT unsubscribe, unsubscribe_one_click, message_key, account_id, from_addr, from_name
             FROM messages WHERE id = ?1",
            [message_id],
            |r| {
                Ok(crate::newsletters::WayOut {
                    link: r.get(0)?,
                    one_click: r.get(1)?,
                    key: r.get(2)?,
                    account_id: r.get(3)?,
                    address: r.get(4)?,
                    name: r.get(5)?,
                })
            },
        )?)
    }

    /// Forgets what the filter said about mail that's gone.
    pub fn forget_old_verdicts(&self) -> Result<()> {
        self.db().execute(
            "DELETE FROM verdicts WHERE message_key NOT IN (SELECT message_key FROM messages)",
            [],
        )?;
        Ok(())
    }

    /// How many emails' words are held for searching.
    pub fn search_index_size(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT COUNT(*) FROM search_index", [], |r| r.get(0))?)
    }

    pub fn folder_uid_validity(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self.db().query_row(
            "SELECT uid_validity FROM folders WHERE id = ?1",
            [folder_id],
            |r| r.get(0),
        )?)
    }

    pub fn uids_not_saved(&self, folder_id: i64, uids: &[u32]) -> Result<Vec<u32>> {
        let saved = self.saved_uids(folder_id, uids, "")?;
        Ok(uids
            .iter()
            .copied()
            .filter(|u| !saved.contains(u))
            .collect())
    }

    /// Those without a downloaded body, leaving out very large messages.
    pub fn uids_wanting_body(
        &self,
        folder_id: i64,
        uids: &[u32],
        most_bytes: u32,
    ) -> Result<Vec<u32>> {
        let wanting = self.saved_uids(
            folder_id,
            uids,
            &format!(
                "AND size <= {most_bytes}
                 AND NOT EXISTS (SELECT 1 FROM bodies b WHERE b.message_id = messages.id)"
            ),
        )?;
        Ok(uids
            .iter()
            .copied()
            .filter(|u| wanting.contains(u))
            .collect())
    }

    fn saved_uids(&self, folder_id: i64, uids: &[u32], extra: &str) -> Result<BTreeSet<u32>> {
        if uids.is_empty() {
            return Ok(BTreeSet::new());
        }
        let list: Vec<String> = uids.iter().map(u32::to_string).collect();
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT uid FROM messages WHERE folder_id = ?1 AND uid IN ({}) {extra}",
            list.join(",")
        ))?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn message_ids_for_uids(&self, folder_id: i64, uids: &[u32]) -> Result<Vec<i64>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let list: Vec<String> = uids.iter().map(u32::to_string).collect();
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT id FROM messages WHERE folder_id = ?1 AND uid IN ({})",
            list.join(",")
        ))?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One conversation as a row, as seen from its inbox if it's in one.
    pub fn summary_of(&self, account_id: i64, thread_id: &str) -> Result<Option<ThreadSummary>> {
        summary_on(&self.db(), account_id, thread_id)
    }

    /// Many conversations' rows at once, read without waiting on anything being saved.
    pub(crate) fn summaries_of(&self, threads: &[(i64, String)]) -> Result<Vec<ThreadSummary>> {
        let db = self.reading_db();
        let mut out = Vec::new();
        for (account, thread) in threads {
            if let Some(summary) = summary_on(&db, *account, thread)? {
                out.push(summary);
            }
        }
        Ok(out)
    }
}

fn summary_on(db: &Connection, account_id: i64, thread_id: &str) -> Result<Option<ThreadSummary>> {
    let home: Option<i64> = db
        .prepare_cached(
            "SELECT m.folder_id FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.thread_id = ?2
             ORDER BY f.role = 'inbox' DESC, f.role IN ('trash', 'junk') ASC, m.date DESC LIMIT 1",
        )?
        .query_row(params![account_id, thread_id], |r| r.get(0))
        .optional()?;
    let Some(home) = home else { return Ok(None) };
    Ok(db
        .prepare_cached(&summary_sql(
            "m.thread_id",
            "m.account_id = ?1 AND m.thread_id = ?2
             AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?3) AND (m.is_draft = 0 OR f.id = ?3)",
            "GROUP BY m.thread_id",
        ))?
        .query_row(params![account_id, thread_id, home], |r| read_summary(r, home))
        .optional()?)
}

impl Store {
    /// One kind of folder, such as the inbox or Sent, across every connected
    /// account, newest first. The same email sent to two inboxes shows twice,
    /// because each is its own copy.
    pub fn list_all(
        &self,
        role: &str,
        limit: i64,
        offset: i64,
        grouped: bool,
    ) -> Result<Vec<ThreadSummary>> {
        self.list_all_on_tab(role, limit, offset, grouped, None)
    }

    pub fn list_all_on_tab(
        &self,
        role: &str,
        limit: i64,
        offset: i64,
        grouped: bool,
        tab: Option<&str>,
    ) -> Result<Vec<ThreadSummary>> {
        self.list_all_filtered(role, limit, offset, grouped, tab, false)
    }

    pub fn list_all_filtered(
        &self,
        role: &str,
        limit: i64,
        offset: i64,
        grouped: bool,
        tab: Option<&str>,
        unread_only: bool,
    ) -> Result<Vec<ThreadSummary>> {
        let folders = self.all_inbox_folders(role)?;
        self.list_across(&folders, limit, offset, grouped, tab, unread_only)
    }

    /// Every folder of this kind, except where an inbox is left out of All Inbox.
    pub fn all_inbox_folders(&self, role: &str) -> Result<Vec<i64>> {
        let db = self.reading_db();
        let mut st = db.prepare(
            "SELECT f.id, a.prefs_json FROM folders f JOIN accounts a ON a.id = f.account_id
             WHERE f.role = ?1 ORDER BY f.id",
        )?;
        let rows = st.query_map([role], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?;
        let folders: Vec<(i64, String)> = rows.collect::<rusqlite::Result<_>>()?;
        Ok(folders
            .into_iter()
            .filter(|(_, prefs)| {
                serde_json::from_str::<AccountPrefs>(prefs)
                    .unwrap_or_default()
                    .in_all_inbox
            })
            .map(|(id, _)| id)
            .collect())
    }

    /// These folders' conversations as one list.
    pub fn list_across(
        &self,
        folders: &[i64],
        limit: i64,
        offset: i64,
        grouped: bool,
        tab: Option<&str>,
        unread_only: bool,
    ) -> Result<Vec<ThreadSummary>> {
        // Only which rows and their order are worked out for every inbox. Just the page
        // asked for is filled in, so reading far down the list doesn't fill in everything
        // above it again each time.
        let mut all: Vec<(usize, i64, PageKey)> = Vec::new();
        for (i, folder) in folders.iter().enumerate() {
            let account_id: i64 = self.reading_db().query_row(
                "SELECT account_id FROM folders WHERE id = ?1",
                [folder],
                |r| r.get(0),
            )?;
            for k in self.page_keys(*folder, limit + offset, 0, grouped, tab, unread_only)? {
                all.push((i, account_id, k));
            }
        }
        all.sort_by(|(_, a_account, a), (_, b_account, b)| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.when.cmp(&a.when))
                .then_with(|| a_account.cmp(b_account))
        });
        let chosen: Vec<(usize, PageKey)> = all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(i, _, k)| (i, k))
            .collect();
        let mut filled: std::collections::HashMap<(usize, String), ThreadSummary> =
            Default::default();
        for (i, folder) in folders.iter().enumerate() {
            let keys: Vec<SqlValue> = chosen
                .iter()
                .filter(|(f, _)| *f == i)
                .map(|(_, k)| k.key.clone())
                .collect();
            for t in self.fill_summaries(*folder, grouped, &keys)? {
                filled.insert((i, t.thread_id.clone()), t);
            }
        }
        Ok(chosen
            .iter()
            .filter_map(|(i, k)| filled.remove(&(*i, k.name())))
            .collect())
    }

    /// Every message in a thread, oldest first, across all folders. A message
    /// stored in two folders appears once. Copies in the bin or spam appear only
    /// when that is the folder being viewed.
    pub fn get_thread(
        &self,
        account_id: i64,
        thread_id: &str,
        view_folder: Option<i64>,
    ) -> Result<Vec<MessageDetail>> {
        if let Some(id) = thread_id.strip_prefix("msg:").and_then(|i| i.parse().ok()) {
            return Ok(vec![self.message_detail(id)?]);
        }
        let db = self.reading_db();
        let mut st = db.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m
             JOIN folders f ON f.id = m.folder_id
             LEFT JOIN bodies b ON b.message_id = m.id
             LEFT JOIN verdicts v ON v.message_key = m.message_key
             WHERE m.account_id = ?1 AND m.thread_id = ?2
               AND (f.role NOT IN ('trash', 'junk', 'drafts') OR f.id = ?3) AND (m.is_draft = 0 OR f.id = ?3)
             ORDER BY m.date ASC, m.id ASC"
        ))?;
        let rows = st.query_map(params![account_id, thread_id, view_folder], row_to_detail)?;

        let mut out: Vec<MessageDetail> = Vec::new();
        let mut seen_ids: std::collections::HashMap<String, usize> = Default::default();
        for row in rows {
            let detail = row?;
            match detail.rfc_id.clone() {
                Some(rfc) => match seen_ids.get(&rfc) {
                    // Keep whichever copy already has its body downloaded.
                    Some(&i) => {
                        if out[i].body.is_none() && detail.body.is_some() {
                            out[i] = detail;
                        }
                    }
                    None => {
                        seen_ids.insert(rfc, out.len());
                        out.push(detail);
                    }
                },
                None => out.push(detail),
            }
        }
        Ok(out)
    }

    pub fn message_detail(&self, message_id: i64) -> Result<MessageDetail> {
        self.reading_db()
            .query_row(
                &format!(
                    "SELECT {MESSAGE_COLUMNS} FROM messages m
                     LEFT JOIN bodies b ON b.message_id = m.id
                     LEFT JOIN verdicts v ON v.message_key = m.message_key
                     WHERE m.id = ?1"
                ),
                [message_id],
                row_to_detail,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("message".into()))
    }

    /// The conversation's message IDs up to and including this message, oldest
    /// first, for a reply's References header.
    pub fn thread_rfc_ids(
        &self,
        account_id: i64,
        thread_id: &str,
        up_to: i64,
    ) -> Result<Vec<String>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT rfc_id FROM messages WHERE account_id = ?1 AND thread_id = ?2
               AND rfc_id IS NOT NULL AND date <= ?3 GROUP BY rfc_id ORDER BY MIN(date) ASC",
        )?;
        let rows = st.query_map(params![account_id, thread_id, up_to], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- acting on mail ----

    pub fn thread_copies(&self, account_id: i64, thread_id: &str) -> Result<Vec<ThreadCopy>> {
        let db = self.db();
        // A single email, when conversations are off, includes its copies in other
        // folders, so marking it read is true everywhere. The sender and subject have to match
        // too, or moving a scam that reused a Message-ID would take the real email with it. Not
        // the arrival time, because one email delivered to several addresses is still one email.
        let single = thread_id.starts_with("msg:");
        let mut st = db.prepare(&thread_copies_sql(single))?;
        let message: Option<i64> = thread_id.strip_prefix("msg:").and_then(|i| i.parse().ok());
        let rows = st.query_map(params![account_id, thread_id, message], |r| {
            Ok(ThreadCopy {
                id: r.get(0)?,
                folder_id: r.get(1)?,
                folder_path: r.get(2)?,
                folder_role: r.get(3)?,
                uid_validity: r.get(4)?,
                uid: r.get(5)?,
                seen: r.get(6)?,
                flagged: r.get(7)?,
                is_draft: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Read or unread here, kept from a refresh until the server has heard: see `seen_heard`.
    pub fn set_seen_held(&self, copies: &[&ThreadCopy], seen: bool) -> Result<()> {
        let mut db = self.db();
        let now = Instant::now();
        {
            let mut held = self.seen_held.lock().unwrap_or_else(|e| e.into_inner());
            for c in copies {
                held.insert((c.folder_id, c.uid), (now, false));
            }
        }
        let tx = db.transaction()?;
        for c in copies {
            tx.execute(
                "UPDATE messages SET seen = ?2 WHERE id = ?1",
                params![c.id, seen],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The server has these read marks now. One that couldn't be told lets go at once, so the
    /// next refresh shows what it really has.
    /// By folder and UID.
    pub fn seen_heard(&self, copies: &[(i64, u32)], heard: bool) {
        let mut held = self.seen_held.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        for key in copies {
            if heard {
                held.insert(*key, (now, true));
            } else {
                held.remove(key);
            }
        }
    }

    /// The UIDs in this folder whose read marks are still held, letting go of those done with.
    fn held_seen_in(&self, folder_id: i64) -> HashSet<u32> {
        let mut held = self.seen_held.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|_, (at, heard)| {
            at.elapsed()
                < if *heard {
                    HELD_AFTER_HEARD
                } else {
                    HELD_WAITING_MOST
                }
        });
        held.keys()
            .filter(|(folder, _)| *folder == folder_id)
            .map(|(_, uid)| *uid)
            .collect()
    }

    pub fn set_message_seen(&self, id: i64, seen: bool) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET seen = ?2 WHERE id = ?1",
            params![id, seen],
        )?;
        Ok(())
    }

    pub fn set_message_flagged(&self, id: i64, flagged: bool) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET flagged = ?2 WHERE id = ?1",
            params![id, flagged],
        )?;
        Ok(())
    }

    /// Removes this copy here. If it moved, the next sync finds it in its new folder.
    pub fn delete_message(&self, id: i64) -> Result<()> {
        self.db()
            .execute("DELETE FROM messages WHERE id = ?1", [id])?;
        Ok(())
    }

    /// People this person has heard from or written to, most frequent first.
    /// People kept in the address book come first, under the name saved for them.
    pub fn contacts(&self, account_id: i64, query: &str, limit: i64) -> Result<Vec<Contact>> {
        let mut out = self.saved_contacts(query)?;
        out.truncate(limit.max(0) as usize);
        let kept: HashSet<String> = out.iter().map(|c| c.address.to_lowercase()).collect();
        let from_mail = self.mail_contacts(account_id, query, limit + kept.len() as i64)?;
        for c in from_mail {
            if out.len() as i64 >= limit {
                break;
            }
            if !kept.contains(&c.address.to_lowercase()) {
                out.push(c);
            }
        }
        Ok(out)
    }

    fn mail_contacts(&self, account_id: i64, query: &str, limit: i64) -> Result<Vec<Contact>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT MAX(name), address FROM (
               SELECT from_name AS name, from_addr AS address FROM messages WHERE account_id = ?1
               UNION ALL
               SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.address')
               FROM messages m JOIN folders f ON f.id = m.folder_id, json_each(m.to_json) j
               WHERE m.account_id = ?1 AND f.role = 'sent'
             )
             WHERE address != '' AND address NOT LIKE '%no-reply%' AND address NOT LIKE '%noreply%'
               AND (instr(lower(address), lower(?2)) > 0 OR instr(lower(name), lower(?2)) > 0)
             GROUP BY lower(address) ORDER BY COUNT(*) DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![account_id, query, limit], |r| {
            Ok(Contact {
                name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                address: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- sender pictures and settings ----

    pub fn avatar(&self, key: &str) -> Result<Option<(Option<String>, i64)>> {
        Ok(self
            .db()
            .query_row(
                "SELECT data, fetched_at FROM avatars WHERE key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn save_avatar(&self, key: &str, data: Option<&str>) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO avatars (key, data, fetched_at) VALUES (?1, ?2, unixepoch())",
            params![key, data],
        )?;
        Ok(())
    }

    /// Remembers that this email had pictures kept from loading.
    pub fn note_images_held(&self, message_id: i64) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO images_held
             SELECT COALESCE(rfc_id, '#' || id) FROM messages WHERE id = ?1",
            [message_id],
        )?;
        Ok(())
    }

    /// How many emails have had pictures kept from loading. Kept apart from the
    /// mail itself, so moving or clearing mail doesn't lower it.
    pub fn images_held_count(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT COUNT(*) FROM images_held", [], |r| r.get(0))?)
    }

    /// Forgets all saved mail, keeping the inboxes, so it can be downloaded again.
    pub fn clear_saved_mail(&self) -> Result<()> {
        self.db().execute_batch(
            "DELETE FROM messages; DELETE FROM ai_summaries; UPDATE folders SET uid_validity = NULL, last_uid = 0, modseq = NULL,
               uid_next = NULL, server_total = NULL, last_error = NULL;",
        )?;
        Ok(())
    }

    pub fn clear_avatars(&self) -> Result<()> {
        self.db().execute("DELETE FROM avatars", [])?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .reading_db()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn all_settings(&self) -> Result<BTreeMap<String, String>> {
        let db = self.reading_db();
        let mut st = db.prepare("SELECT key, value FROM settings")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.db()
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Senders whose email may load its pictures, as chosen by the person.
    pub fn image_senders(&self) -> Result<Vec<String>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT substr(key, 8) FROM settings WHERE key LIKE 'images:%' ORDER BY key",
        )?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_image_sender(&self, address: &str, allowed: bool) -> Result<()> {
        let key = format!("images:{}", address.trim().to_ascii_lowercase());
        if allowed {
            self.set_setting(&key, "on")
        } else {
            self.db()
                .execute("DELETE FROM settings WHERE key = ?1", [key])?;
            Ok(())
        }
    }

    // ---- drafts ----

    #[allow(clippy::too_many_arguments)]
    pub fn insert_draft(&self, d: &Draft) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO drafts (account_id, mode, parent_message_id, to_json, cc_json, bcc_json,
               subject, html, text, quoted_html, quoted_text, attachments_json, from_address,
               pgp_encrypt, pgp_sign, from_encrypted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                d.account_id,
                d.mode,
                d.parent_message_id,
                json(&d.to),
                json(&d.cc),
                json(&d.bcc),
                d.subject,
                d.html,
                d.text,
                d.quoted_html,
                d.quoted_text,
                json(&d.attachments),
                d.from_address,
                d.encrypt,
                d.sign,
                d.from_encrypted,
            ],
        )?;
        Ok(db.last_insert_rowid())
    }

    /// Saves what the person has written so far.
    pub fn update_draft(&self, d: &Draft) -> Result<()> {
        self.db().execute(
            "UPDATE drafts SET account_id = ?2, to_json = ?3, cc_json = ?4, bcc_json = ?5,
               subject = ?6, html = ?7, text = ?8, attachments_json = ?9, from_address = ?10,
               pgp_encrypt = ?11, pgp_sign = ?12, plain_ok = ?13, updated_at = unixepoch()
             WHERE id = ?1",
            params![
                d.id,
                d.account_id,
                json(&d.to),
                json(&d.cc),
                json(&d.bcc),
                d.subject,
                d.html,
                d.text,
                json(&d.attachments),
                d.from_address,
                d.encrypt,
                d.sign,
                d.plain_ok,
            ],
        )?;
        Ok(())
    }

    /// The person confirmed this draft may go unencrypted.
    pub fn allow_plain_draft(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE drafts SET plain_ok = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn draft(&self, id: i64) -> Result<Draft> {
        self.db()
            .query_row(
                &format!("SELECT {DRAFT_COLUMNS} FROM drafts WHERE id = ?1"),
                [id],
                row_to_draft,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("draft".into()))
    }

    pub fn drafts(&self) -> Result<Vec<Draft>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {DRAFT_COLUMNS} FROM drafts WHERE sent_at IS NULL ORDER BY updated_at DESC, id DESC"
        ))?;
        let rows = st.query_map([], row_to_draft)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Marks a draft as gone, so nothing sends it again, before it's cleared away.
    pub fn mark_draft_sent(&self, id: i64) -> Result<()> {
        self.db().execute(
            "UPDATE drafts SET sent_at = unixepoch(), send_at = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn draft_was_sent(&self, id: i64) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT sent_at IS NOT NULL FROM drafts WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Drafts that went but couldn't be cleared away at the time.
    pub fn sent_drafts_left(&self) -> Result<Vec<i64>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id FROM drafts WHERE sent_at IS NOT NULL")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_draft(&self, id: i64) -> Result<()> {
        let db = self.db();
        // Its copy on the mail server has to go too, whenever that can be reached.
        db.execute(
            "INSERT OR IGNORE INTO draft_leftovers (account_id, rfc_id)
             SELECT account_id, server_rfc_id FROM drafts WHERE id = ?1 AND server_rfc_id IS NOT NULL",
            [id],
        )?;
        db.execute("DELETE FROM drafts WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn update_attachments(&self, d: &Draft) -> Result<()> {
        self.db().execute(
            "UPDATE drafts SET attachments_json = ?2 WHERE id = ?1",
            params![d.id, json(&d.attachments)],
        )?;
        Ok(())
    }

    pub fn message_with_rfc(&self, account_id: i64, rfc_id: &str) -> Result<Option<i64>> {
        Ok(self
            .db()
            .query_row(
                "SELECT id FROM messages WHERE account_id = ?1 AND rfc_id = ?2 LIMIT 1",
                params![account_id, rfc_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn draft_ids_for_account(&self, account_id: i64) -> Result<Vec<i64>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id FROM drafts WHERE account_id = ?1")?;
        let rows = st.query_map([account_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const MESSAGE_COLUMNS: &str = concat!(
    "m.id, m.subject, m.from_name, m.from_addr, m.to_json, m.cc_json, m.date,
     m.seen, m.flagged, m.folder_id, m.rfc_id,
     b.message_id IS NOT NULL, b.text, b.html, b.attachments_json,
     m.account_id, m.thread_id, m.reply_to_json, m.bcc_json,
    COALESCE(v.risk, 0), COALESCE(v.reasons_json, '[]'), ",
    crate::newsletters::can_unsubscribe!(),
    ", m.due_at, b.pgp_json"
);

const DRAFT_COLUMNS: &str =
    "id, account_id, mode, parent_message_id, to_json, cc_json, bcc_json, subject,
     html, text, quoted_html, quoted_text, attachments_json, updated_at, from_address,
     send_at, send_problem, remind_days, made_by_ai, pgp_encrypt, pgp_sign, from_encrypted,
     plain_ok";

fn row_to_detail(r: &Row<'_>) -> rusqlite::Result<MessageDetail> {
    let has_body: bool = r.get(11)?;
    let body = if has_body {
        Some(Body {
            text: r.get(12)?,
            html: r.get(13)?,
            attachments: parse_json(r.get::<_, String>(14)?),
            pgp: pgp_info(r.get(23)?),
        })
    } else {
        None
    };
    Ok(MessageDetail {
        id: r.get(0)?,
        account_id: r.get(15)?,
        thread_id: r.get(16)?,
        rfc_id: r.get(10)?,
        subject: r.get(1)?,
        from: Addr {
            name: r.get(2)?,
            address: r.get(3)?,
        },
        reply_to: parse_json(r.get::<_, String>(17)?),
        to: parse_json(r.get::<_, String>(4)?),
        cc: parse_json(r.get::<_, String>(5)?),
        bcc: parse_json(r.get::<_, String>(18)?),
        date: r.get(6)?,
        seen: r.get(7)?,
        flagged: r.get(8)?,
        folder_id: r.get(9)?,
        body,
        risk: r.get(19)?,
        warnings: parse_json::<crate::spam::Reason>(r.get::<_, String>(20)?)
            .into_iter()
            .map(|x| x.text)
            .collect(),
        can_unsubscribe: r.get(21)?,
        due_at: r.get(22)?,
    })
}

fn row_to_draft(r: &Row<'_>) -> rusqlite::Result<Draft> {
    Ok(Draft {
        id: r.get(0)?,
        account_id: r.get(1)?,
        mode: r.get(2)?,
        parent_message_id: r.get(3)?,
        to: parse_json(r.get::<_, String>(4)?),
        cc: parse_json(r.get::<_, String>(5)?),
        bcc: parse_json(r.get::<_, String>(6)?),
        subject: r.get(7)?,
        html: r.get(8)?,
        text: r.get(9)?,
        quoted_html: r.get(10)?,
        quoted_text: r.get(11)?,
        attachments: parse_json(r.get::<_, String>(12)?),
        updated_at: r.get(13)?,
        from_address: r.get(14)?,
        send_at: r.get(15)?,
        send_problem: r.get(16)?,
        remind_days: r.get(17)?,
        made_by_ai: r.get(18)?,
        encrypt: r.get(19)?,
        sign: r.get(20)?,
        from_encrypted: r.get(21)?,
        plain_ok: r.get(22)?,
    })
}

/// INBOX first, then the usual folders, then the user's own in name order.
const FOLDER_ORDER: &str = "CASE role WHEN 'inbox' THEN 0 WHEN 'sent' THEN 1 WHEN 'drafts' THEN 2
    WHEN 'archive' THEN 3 WHEN 'junk' THEN 4 WHEN 'trash' THEN 5 ELSE 6 END, name COLLATE NOCASE";

fn row_to_body(r: &Row<'_>) -> rusqlite::Result<Body> {
    Ok(Body {
        text: r.get(0)?,
        html: r.get(1)?,
        attachments: parse_json(r.get::<_, String>(2)?),
        pgp: pgp_info(r.get(3)?),
    })
}

fn pgp_info(saved: Option<String>) -> Option<crate::model::PgpInfo> {
    serde_json::from_str(&saved?).ok()
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn parse_json<T: serde::de::DeserializeOwned>(s: String) -> Vec<T> {
    serde_json::from_str(&s).unwrap_or_default()
}

/// Picks the thread for a new message from its References chain.
///
/// Mail arrives in any order, so a message can also link two threads that
/// looked separate. When that happens they are merged into one.
fn assign_thread(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    folder_id: i64,
    uid: u32,
    h: &ParsedHeaders,
    date: i64,
) -> Result<String> {
    let mut chain: Vec<&str> = h.references.iter().map(String::as_str).collect();
    chain.extend(h.in_reply_to.iter().map(String::as_str));
    if let Some(own) = &h.rfc_id {
        chain.push(own);
    }
    if chain.is_empty() {
        // No IDs at all. Only a matching reply subject can place it.
        return Ok(match thread_by_subject(tx, account_id, h, date)? {
            Some(thread) => thread,
            None => format!("noid:{folder_id}:{uid}"),
        });
    }

    let marks = vec!["?"; chain.len()].join(",");
    // Two separate lookups, each with its own index. As one query with OR, SQLite walks
    // every message in the inbox, and bringing down a big mailbox never finishes. Plain
    // UNION took 17 ms each on a real mailbox, holding up the whole app for seconds per
    // batch, so repeats are removed below instead.
    let sql = format!(
        "SELECT thread_id FROM messages WHERE account_id = ? AND rfc_id IN ({marks})
         UNION ALL
         SELECT thread_id FROM messages WHERE account_id = ? AND thread_id IN ({marks})"
    );
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![&account_id];
    for c in chain.iter() {
        values.push(c);
    }
    values.push(&account_id);
    for c in chain.iter() {
        values.push(c);
    }
    let mut found: Vec<String> = {
        let mut st = tx.prepare_cached(&sql)?;
        let rows = st.query_map(params_from_iter(values), |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    found.sort_unstable();
    found.dedup();

    let root = chain[0].to_string();
    // The same choice on every device, as a team knows a conversation by this name.
    let Some(first) = found.first().cloned() else {
        // Some mail apps drop the IDs from replies.
        return Ok(thread_by_subject(tx, account_id, h, date)?.unwrap_or(root));
    };
    for other in found.iter().filter(|t| **t != first) {
        join_thread_into(tx, account_id, &first, other)?;
    }
    Ok(first)
}

/// Moves everything of conversation `other` into `first`: its emails, marks, follow-ups
/// and tags.
fn join_thread_into(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    first: &str,
    other: &str,
) -> Result<()> {
    tx.execute(
        "UPDATE messages SET thread_id = ?1 WHERE account_id = ?2 AND thread_id = ?3",
        params![first, account_id, other],
    )?;
    // Whatever was chosen for either part still holds for the whole.
    tx.execute(
        "INSERT OR IGNORE INTO thread_marks (account_id, thread_id) VALUES (?1, ?2)",
        params![account_id, first],
    )?;
    tx.execute(
        "UPDATE thread_marks SET
           pinned = MAX(pinned, COALESCE((SELECT pinned FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3), 0)),
           muted = MAX(muted, COALESCE((SELECT muted FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3), 0)),
           snooze_until = COALESCE(snooze_until, (SELECT snooze_until FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3)),
           snoozed_at = COALESCE(snoozed_at, (SELECT snoozed_at FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3)),
           follow_at = COALESCE(follow_at, (SELECT follow_at FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3)),
           follow_set_at = COALESCE(follow_set_at, (SELECT follow_set_at FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3)),
           returned_at = COALESCE(returned_at, (SELECT returned_at FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3)),
           follow_due = MAX(follow_due, COALESCE((SELECT follow_due FROM thread_marks WHERE account_id = ?1 AND thread_id = ?3), 0))
         WHERE account_id = ?1 AND thread_id = ?2",
        params![account_id, first, other],
    )?;
    tx.execute(
        "DELETE FROM thread_marks WHERE account_id = ?1 AND thread_id = ?2",
        params![account_id, other],
    )?;
    // Follow-ups go with the conversation, or they'd never see the reply that should
    // stop them. If both parts had some on the way, the whole keeps one set.
    tx.execute(
        "UPDATE sequence_runs SET state = 'stopped', next_at = NULL
         WHERE account_id = ?1 AND thread_id = ?3 AND state = 'running'
           AND EXISTS (SELECT 1 FROM sequence_runs
                       WHERE account_id = ?1 AND thread_id = ?2 AND state = 'running')",
        params![account_id, first, other],
    )?;
    tx.execute(
        "UPDATE sequence_runs SET thread_id = ?2 WHERE account_id = ?1 AND thread_id = ?3",
        params![account_id, first, other],
    )?;
    // Tags on either part are on the whole.
    tx.execute(
        "UPDATE OR IGNORE thread_tags SET thread_id = ?2 WHERE account_id = ?1 AND thread_id = ?3",
        params![account_id, first, other],
    )?;
    tx.execute(
        "DELETE FROM thread_tags WHERE account_id = ?1 AND thread_id = ?2",
        params![account_id, other],
    )?;
    tx.execute(
        "DELETE FROM thread_marks WHERE account_id = ?1 AND thread_id = ?2 AND pinned = 0 AND muted = 0
           AND snooze_until IS NULL AND returned_at IS NULL AND follow_at IS NULL AND follow_due = 0 AND priority IS NULL",
        params![account_id, first],
    )?;
    Ok(())
}

/// Puts emails with the search words in their subject first, as they're what's most likely
/// wanted. If that's all of them or none of them, the order by date says everything.
/// Whether this email has every word asked for in its subject, its people or its own new text.
/// With no body yet, its preview stands in for the text.
fn own_words_match(db: &Connection, id: i64, f: &SearchFilters) -> Result<bool> {
    let (subject, people): (String, String) = db.query_row(
        "SELECT COALESCE(subject, ''),
                COALESCE(from_name, '') || ' ' || from_addr || ' ' || to_json || ' ' || cc_json
         FROM messages WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let has = |text: &str| {
        crate::search::matches_exactly(&f.words, &subject, &people, text)
            && (f.similar.is_empty()
                || f.similar.iter().any(|w| {
                    crate::search::matches_exactly(std::slice::from_ref(w), &subject, &people, text)
                }))
    };
    // The subject and people are enough most of the time, and the body is the slow part.
    if has("") {
        return Ok(true);
    }
    let (body, snippet): (Option<String>, String) = db.query_row(
        "SELECT b.text, COALESCE(m.snippet, '')
         FROM messages m LEFT JOIN bodies b ON b.message_id = m.id WHERE m.id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let text = match body {
        Some(body) => crate::priority::without_quotes(&body),
        None => snippet,
    };
    Ok(has(&text))
}

/// The facts an email's markup states, read as it's saved.
/// The sender's own Unsubscribe link in the email, when its headers gave none. Only for mail
/// whose sender passed the provider's check, so the link's site really is the sender's.
fn note_way_out(tx: &rusqlite::Transaction, message_id: i64, html: Option<&str>) -> Result<()> {
    let Some(html) = html.filter(|h| h.to_ascii_lowercase().contains("unsubscribe")) else {
        return Ok(());
    };
    let from: Option<String> = tx
        .query_row(
            "SELECT from_addr FROM messages WHERE id = ?1 AND unsubscribe IS NULL AND auth = 'pass'",
            [message_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(link) = from.and_then(|from| crate::spam::unsubscribe_in_body(html, &from)) {
        tx.execute(
            "UPDATE messages SET unsubscribe = ?2, unsubscribe_one_click = 0 WHERE id = ?1",
            params![message_id, link],
        )?;
    }
    Ok(())
}

fn save_facts(tx: &rusqlite::Transaction, message_id: i64, html: Option<&str>) -> Result<()> {
    tx.execute("DELETE FROM facts WHERE message_id = ?1", [message_id])?;
    let Some(html) = html.filter(|h| h.contains("ld+json")) else {
        return Ok(());
    };
    for f in crate::facts::facts_in(html) {
        tx.execute(
            "INSERT INTO facts (message_id, kind, starts_at, place, value_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![message_id, f.kind, f.starts_at, f.place, f.value.to_string()],
        )?;
    }
    Ok(())
}

fn mark_best_matches(list: &mut [ThreadSummary], f: &SearchFilters) {
    let words: Vec<String> = f
        .words
        .iter()
        .chain(&f.similar)
        .map(|w| w.to_lowercase())
        .collect();
    if words.is_empty() {
        return;
    }
    let is_best = |t: &ThreadSummary| {
        let subject = t.subject.to_lowercase();
        words.iter().any(|w| subject.contains(w.as_str()))
    };
    let best = list.iter().filter(|t| is_best(t)).count();
    if best == 0 || best == list.len() {
        return;
    }
    for t in list.iter_mut() {
        t.best_match = is_best(t);
    }
    list.sort_by_key(|t| !t.best_match);
}

/// Whether two emails with the same reply subject are between the same people: each wrote
/// to the other, or one person wrote both to someone in common. The second is a person
/// answering again while the replies in between aren't on this device, such as when
/// they were sent from another app that didn't keep them in Sent.
fn same_people(
    sender: &str,
    people: &HashSet<String>,
    from: &str,
    theirs: &HashSet<String>,
) -> bool {
    (people.contains(from) && theirs.contains(sender))
        || (from == sender && !people.is_disjoint(theirs))
}

/// Finds the conversation for a reply that lost its IDs. This is deliberately
/// strict, because joining unrelated mail is worse than a split conversation: it
/// must say "Re:", match the subject, be recent, and each sender must be someone
/// the other message was addressed to.
fn thread_by_subject(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    h: &ParsedHeaders,
    date: i64,
) -> Result<Option<String>> {
    if !parse::is_reply_subject(&h.subject) {
        return Ok(None);
    }
    let base = parse::base_subject(&h.subject);
    let Some(sender) = h.from.as_ref().map(|f| f.address.to_ascii_lowercase()) else {
        return Ok(None);
    };
    if base.is_empty() || sender.is_empty() {
        return Ok(None);
    }
    let lower = |list: &[Addr]| -> HashSet<String> {
        list.iter()
            .map(|a| a.address.to_ascii_lowercase())
            .collect()
    };
    let mut people = lower(&h.to);
    people.extend(lower(&h.cc));

    let mut st = tx.prepare(
        "SELECT thread_id, from_addr, to_json, cc_json FROM messages
         WHERE account_id = ?1 AND base_subject = ?2 AND date BETWEEN ?3 AND ?4
         ORDER BY date DESC LIMIT 50",
    )?;
    let rows = st.query_map(
        params![
            account_id,
            base,
            date - THREAD_WINDOW_SECONDS,
            date + CLOCK_SKEW_SECONDS
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )?;
    for row in rows {
        let (thread, from, to, cc) = row?;
        let mut theirs = lower(&parse_json::<Addr>(to));
        theirs.extend(lower(&parse_json::<Addr>(cc)));
        if same_people(&sender, &people, &from.to_ascii_lowercase(), &theirs) {
            return Ok(Some(thread));
        }
    }
    Ok(None)
}

/// A team's shared state of one conversation, as the server last said.
pub struct TeamThreadRow {
    pub thread_key: String,
    pub assignee: Option<i64>,
    pub status: String,
    pub status_at: i64,
    pub sent_count: i64,
    pub last_sent_by: Option<i64>,
    pub last_sent_at: i64,
    pub sending_by: Option<i64>,
    pub sending_until: i64,
    pub seq: i64,
}

pub struct TeamInfo {
    pub team_id: i64,
    pub name: String,
    pub me: i64,
    pub role: String,
}

#[derive(serde::Serialize)]
pub struct TeamPerson {
    pub user_id: i64,
    pub email: String,
    pub role: String,
}

pub struct TeamComment {
    pub id: String,
    pub author: i64,
    pub text: String,
    pub created_at: i64,
}

#[derive(serde::Serialize)]
pub struct TeamTemplate {
    pub team_id: i64,
    pub id: String,
    pub name: String,
    pub body: String,
    pub version: i64,
}

impl Store {
    pub fn team_info(&self, team_id: i64) -> Result<Option<TeamInfo>> {
        Ok(self
            .db()
            .query_row(
                "SELECT team_id, name, me, role FROM team_info WHERE team_id = ?1",
                [team_id],
                team_info_row,
            )
            .optional()?)
    }

    /// All the teams known here, oldest first.
    pub fn team_infos(&self) -> Result<Vec<TeamInfo>> {
        let db = self.db();
        let mut st =
            db.prepare("SELECT team_id, name, me, role FROM team_info ORDER BY team_id")?;
        let rows = st.query_map([], team_info_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_team_info(
        &self,
        team_id: i64,
        name: &str,
        me: i64,
        role: &str,
        members_version: i64,
        members: &[(i64, String, String)],
    ) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO team_info (team_id, name, me, role, members_version)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (team_id) DO UPDATE SET name = ?2, me = ?3, role = ?4,
               members_version = ?5",
            params![team_id, name, me, role, members_version],
        )?;
        tx.execute("DELETE FROM team_people WHERE team_id = ?1", [team_id])?;
        for (id, email, person_role) in members {
            tx.execute(
                "INSERT INTO team_people (team_id, user_id, email, role) VALUES (?1, ?2, ?3, ?4)",
                params![team_id, id, email, person_role],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Nothing of this team stays here. Other teams aren't touched.
    pub fn wipe_team(&self, team_id: i64) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        for table in [
            "team_info",
            "team_people",
            "team_threads",
            "team_comments",
            "team_templates",
            "draft_team_base",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE team_id = ?1"),
                [team_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn team_members(&self, team_id: i64) -> Result<Vec<TeamPerson>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT user_id, email, role FROM team_people WHERE team_id = ?1 ORDER BY rowid",
        )?;
        let rows = st.query_map([team_id], |r| {
            Ok(TeamPerson {
                user_id: r.get(0)?,
                email: r.get(1)?,
                role: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn team_member_email(&self, team_id: i64, user_id: i64) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "SELECT email FROM team_people WHERE team_id = ?1 AND user_id = ?2",
                [team_id, user_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn team_member_emails(
        &self,
        team_id: i64,
    ) -> Result<std::collections::HashMap<i64, String>> {
        Ok(self
            .team_members(team_id)?
            .into_iter()
            .map(|m| (m.user_id, m.email))
            .collect())
    }

    fn team_number(&self, team_id: i64, column: &str) -> Result<i64> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {column} FROM team_info WHERE team_id = ?1"),
                [team_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn team_seq(&self, team_id: i64) -> Result<i64> {
        self.team_number(team_id, "seq")
    }

    pub fn team_members_version(&self, team_id: i64) -> Result<i64> {
        self.team_number(team_id, "members_version")
    }

    pub fn team_vault_seen(&self, team_id: i64) -> Result<i64> {
        self.team_number(team_id, "vault_seen")
    }

    /// Only ever forwards, as answers can arrive out of order.
    pub fn advance_team_seq(&self, team_id: i64, seq: i64) -> Result<()> {
        self.db().execute(
            "UPDATE team_info SET seq = MAX(seq, ?2) WHERE team_id = ?1",
            [team_id, seq],
        )?;
        Ok(())
    }

    pub fn set_team_vault_seen(&self, team_id: i64, version: i64) -> Result<()> {
        self.db().execute(
            "UPDATE team_info SET vault_seen = MAX(vault_seen, ?2) WHERE team_id = ?1",
            [team_id, version],
        )?;
        Ok(())
    }

    /// Conversations and notes are scrambled with their team's key, so these names can't
    /// clash between teams.
    pub fn team_thread(&self, key: &str) -> Result<Option<TeamThreadRow>> {
        Ok(self
            .db()
            .query_row(
                "SELECT thread_key, assignee, status, status_at, sent_count, last_sent_by,
                        last_sent_at, sending_by, seq, sending_until
                 FROM team_threads WHERE thread_key = ?1",
                [key],
                |r| {
                    Ok(TeamThreadRow {
                        thread_key: r.get(0)?,
                        assignee: r.get(1)?,
                        status: r.get(2)?,
                        status_at: r.get(3)?,
                        sent_count: r.get(4)?,
                        last_sent_by: r.get(5)?,
                        last_sent_at: r.get(6)?,
                        sending_by: r.get(7)?,
                        seq: r.get(8)?,
                        sending_until: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn save_team_thread(&self, team_id: i64, t: &TeamThreadRow) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO team_threads
               (thread_key, assignee, status, status_at, sent_count, last_sent_by, last_sent_at,
                sending_by, seq, sending_until, team_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                t.thread_key,
                t.assignee,
                t.status,
                t.status_at,
                t.sent_count,
                t.last_sent_by,
                t.last_sent_at,
                t.sending_by,
                t.seq,
                t.sending_until,
                team_id
            ],
        )?;
        Ok(())
    }

    pub fn team_comment_seq(&self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .db()
            .query_row("SELECT seq FROM team_comments WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn team_comment_team(&self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .db()
            .query_row(
                "SELECT team_id FROM team_comments WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_team_comment(
        &self,
        team_id: i64,
        id: &str,
        thread_key: &str,
        author: i64,
        text: Option<&str>,
        created_at: i64,
        seq: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO team_comments (id, thread_key, author, text, created_at, seq, team_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, thread_key, author, text, created_at, seq, team_id],
        )?;
        Ok(())
    }

    /// In the same order on everyone's screen.
    pub fn team_comments_on(&self, thread_key: &str) -> Result<Vec<TeamComment>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, author, text, created_at FROM team_comments
             WHERE thread_key = ?1 AND text IS NOT NULL ORDER BY created_at, id",
        )?;
        let rows = st.query_map([thread_key], |r| {
            Ok(TeamComment {
                id: r.get(0)?,
                author: r.get(1)?,
                text: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn team_comment_counts(&self) -> Result<std::collections::HashMap<String, i64>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT thread_key, COUNT(*) FROM team_comments WHERE text IS NOT NULL GROUP BY thread_key",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn team_template_seq(&self, team_id: i64, id: &str) -> Result<Option<i64>> {
        Ok(self
            .db()
            .query_row(
                "SELECT seq FROM team_templates WHERE team_id = ?1 AND id = ?2",
                params![team_id, id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn save_team_template(
        &self,
        team_id: i64,
        id: &str,
        name: Option<&str>,
        body: Option<&str>,
        version: i64,
        seq: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO team_templates (team_id, id, name, body, version, seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![team_id, id, name, body, version, seq],
        )?;
        Ok(())
    }

    /// One team's, or everyone's when writing an email.
    pub fn team_templates(&self, team_id: Option<i64>) -> Result<Vec<TeamTemplate>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT team_id, id, name, body, version FROM team_templates
             WHERE name IS NOT NULL AND (?1 IS NULL OR team_id = ?1) ORDER BY name COLLATE NOCASE",
        )?;
        let rows = st.query_map([team_id], |r| {
            Ok(TeamTemplate {
                team_id: r.get(0)?,
                id: r.get(1)?,
                name: r.get(2)?,
                body: r.get(3)?,
                version: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The newest email that came in, leaving out the team's own replies.
    pub fn thread_last_incoming(&self, account_id: i64, thread_id: &str) -> Result<i64> {
        Ok(self.db().query_row(
            "SELECT COALESCE(MAX(m.date), 0) FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.thread_id = ?2 AND f.role != 'sent'",
            params![account_id, thread_id],
            |r| r.get(0),
        )?)
    }

    /// Whether this email was sent encrypted, as far as its saved body says.
    pub fn is_encrypted(&self, message_id: i64) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {ENCRYPTED_SQL} FROM bodies b WHERE b.message_id = ?1"),
                [message_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Whether any email in a conversation (or the one email, for `msg:<id>`) was sent
    /// encrypted.
    pub fn thread_encrypted(&self, account_id: i64, thread_id: &str) -> Result<bool> {
        if let Some(id) = thread_id.strip_prefix("msg:").and_then(|i| i.parse().ok()) {
            return self.is_encrypted(id);
        }
        Ok(self.db().query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM messages m JOIN bodies b ON b.message_id = m.id
                 WHERE m.account_id = ?1 AND m.thread_id = ?2 AND {ENCRYPTED_SQL})"
            ),
            params![account_id, thread_id],
            |r| r.get(0),
        )?)
    }

    pub fn message_thread(&self, message_id: i64) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "SELECT thread_id FROM messages WHERE id = ?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn save_draft_team_base(
        &self,
        team_id: i64,
        draft_id: i64,
        thread_key: &str,
        sent_count: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT OR REPLACE INTO draft_team_base (draft_id, thread_key, sent_count, team_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![draft_id, thread_key, sent_count, team_id],
        )?;
        Ok(())
    }

    pub fn clear_draft_team_base(&self, draft_id: i64) -> Result<()> {
        self.db().execute(
            "DELETE FROM draft_team_base WHERE draft_id = ?1",
            [draft_id],
        )?;
        Ok(())
    }

    /// Which conversation a reply belongs to, and how many team replies had gone when
    /// it was started. Kept apart from the message, which a teammate may have moved.
    pub fn draft_team_base(&self, draft_id: i64) -> Result<Option<(String, i64)>> {
        Ok(self
            .db()
            .query_row(
                "SELECT thread_key, sent_count FROM draft_team_base WHERE draft_id = ?1",
                [draft_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
}

/// One row of a list page before it's filled in: its key, and what it's sorted by.
struct PageKey {
    key: SqlValue,
    pinned: i64,
    when: i64,
}

impl PageKey {
    /// The name the filled-in row goes by.
    fn name(&self) -> String {
        match &self.key {
            SqlValue::Text(t) => t.clone(),
            SqlValue::Integer(n) => format!("msg:{n}"),
            _ => String::new(),
        }
    }
}

fn team_info_row(r: &rusqlite::Row) -> rusqlite::Result<TeamInfo> {
    Ok(TeamInfo {
        team_id: r.get(0)?,
        name: r.get(1)?,
        me: r.get(2)?,
        role: r.get(3)?,
    })
}

/// Finds a conversation's copies, or a single email's copies in other folders. The single
/// email's half is a list of ids so each part uses its index: as one OR it read every
/// message in the database for each email, which took hours on a big mailbox.
fn thread_copies_sql(single: bool) -> String {
    let find = if single {
        "m.id IN (SELECT ?3 UNION
           SELECT o.id FROM messages s JOIN messages o
             ON o.account_id = s.account_id AND o.rfc_id = s.rfc_id
            WHERE s.id = ?3 AND o.from_addr = s.from_addr AND o.subject = s.subject)"
    } else {
        "m.thread_id = ?2"
    };
    // The `+`s keep SQLite from walking every message of the inbox in date order
    // instead of going straight to the few it wants.
    let account = if single {
        "+m.account_id = ?1"
    } else {
        "m.account_id = ?1"
    };
    format!(
        "SELECT m.id, m.folder_id, f.path, f.role, f.uid_validity, m.uid, m.seen, m.flagged, m.is_draft
         FROM messages m JOIN folders f ON f.id = m.folder_id
         WHERE {account} AND (?2 IS NOT NULL) AND (?3 IS NULL OR ?3 IS NOT NULL) AND {find}
         ORDER BY +m.date ASC, m.id ASC"
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn finding_an_emails_copies_uses_the_indexes() {
        let s = Store::open_in_memory().unwrap();
        for single in [true, false] {
            let plan: Vec<String> = {
                let db = s.db();
                let mut st = db
                    .prepare(&format!("EXPLAIN QUERY PLAN {}", thread_copies_sql(single)))
                    .unwrap();
                st.query_map(params![1, "msg:5", 5], |r| r.get::<_, String>(3))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            };
            // Reading every message, or every one in the inbox, for each email took hours on
            // a big mailbox. Each lookup has to go straight to the emails it wants.
            let wide = |l: &String| {
                l.starts_with("SCAN m")
                    || l.starts_with("SCAN o")
                    || l.starts_with("SCAN s")
                    || l.ends_with("(account_id=?)")
            };
            assert!(!plan.iter().any(wide), "{single}: {plan:?}");
        }
    }

    use super::*;

    #[test]
    fn a_draft_marked_as_sent_is_never_sent_or_shown_again() {
        let s = Store::open_in_memory().unwrap();
        let a = s
            .insert_account("me@x.test", "Me", "x", 993, Security::Tls, "me")
            .unwrap();
        let id = s
            .insert_draft(&Draft {
                id: 0,
                account_id: a.id,
                mode: "new".into(),
                parent_message_id: None,
                from_address: String::new(),
                to: vec![],
                cc: vec![],
                bcc: vec![],
                subject: "Hi".into(),
                html: String::new(),
                text: String::new(),
                quoted_html: String::new(),
                quoted_text: String::new(),
                attachments: vec![],
                updated_at: 0,
                send_at: None,
                send_problem: None,
                remind_days: None,
                made_by_ai: false,
                encrypt: false,
                sign: false,
                from_encrypted: false,
                plain_ok: false,
            })
            .unwrap();
        s.set_send_at(id, Some(10), None).unwrap();
        assert_eq!(s.drafts_due(20).unwrap(), vec![id]);
        s.mark_draft_sent(id).unwrap();
        assert!(s.draft_was_sent(id).unwrap());
        assert!(s.drafts_due(20).unwrap().is_empty());
        assert!(s.drafts().unwrap().is_empty());
        assert_eq!(s.sent_drafts_left().unwrap(), vec![id]);
    }

    #[test]
    fn a_number_used_by_another_build_still_gets_every_table() {
        // A test copy used number 33 for a column of its own, so 33 here looked done.
        let conn = Connection::open_in_memory().unwrap();
        let facts = MIGRATIONS
            .iter()
            .position(|m| m.contains("CREATE TABLE facts"))
            .unwrap();
        for sql in &MIGRATIONS[..facts] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "ALTER TABLE messages ADD COLUMN focus_done INTEGER NOT NULL DEFAULT 0;",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", (facts + 1) as i64)
            .unwrap();
        let store = Store::init(conn).unwrap();
        let db = store.db();
        assert!(has_object(&db, "facts").unwrap());
        assert!(has_object(&db, "facts_message").unwrap());
        // Dropped and renamed tables stay as they are.
        assert!(has_object(&db, "team_people").unwrap());
        assert!(!has_object(&db, "team_people_by_team").unwrap());
    }

    #[test]
    fn a_change_already_there_from_another_build_is_skipped() {
        let conn = Connection::open_in_memory().unwrap();
        let last = MIGRATIONS.len() - 1;
        for sql in &MIGRATIONS[..last] {
            conn.execute_batch(sql).unwrap();
        }
        // The last change was already made by another build, under a different number.
        conn.execute_batch(MIGRATIONS[last]).unwrap();
        conn.pragma_update(None, "user_version", last as i64)
            .unwrap();
        let store = Store::init(conn).unwrap();
        let version: i64 = store
            .db()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn what_a_change_makes_is_read_from_its_sql() {
        assert_eq!(
            made_by("CREATE UNIQUE INDEX a_b ON a (b); ALTER TABLE x ADD COLUMN y TEXT;"),
            vec![
                Made::Object("a_b".into()),
                Made::Column("x".into(), "y".into())
            ]
        );
        assert_eq!(
            made_by("CREATE TABLE IF NOT EXISTS repairs (name TEXT)"),
            vec![Made::Object("repairs".into())]
        );
        assert!(made_by("UPDATE messages SET x = 1").is_empty());
    }

    #[test]
    fn a_database_numbered_in_the_other_order_gets_only_what_it_missed() {
        let conn = Connection::open_in_memory().unwrap();
        let split = MIGRATIONS
            .iter()
            .position(|m| m.contains("CREATE TABLE tags"))
            .unwrap();
        for sql in &MIGRATIONS[..split] {
            conn.execute_batch(sql).unwrap();
        }
        let mut had = 0;
        for sql in MIGRATIONS {
            if [
                "CREATE TABLE meaning_todo",
                "CREATE TABLE agent_activity",
                "ADD COLUMN message_key",
                "CREATE INDEX messages_newsletters",
            ]
            .iter()
            .any(|m| sql.contains(m))
            {
                conn.execute_batch(sql).unwrap();
                had += 1;
            }
        }
        assert_eq!(had, 4);
        conn.pragma_update(None, "user_version", (split + had) as i64)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (email, display_name, imap_host, imap_port, security, username)
             VALUES ('me@x.test', 'Me', 'x', 993, 'tls', 'me');",
        )
        .unwrap();

        let store = Store::init(conn).unwrap();

        let db = store.db();
        for name in [
            "tags",
            "pgp_keys",
            "people",
            "agent_activity",
            "messages_newsletters",
        ] {
            assert!(has_object(&db, name).unwrap(), "{name}");
        }
        let version: i64 = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
        let kept: i64 = db
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1, "nothing already there is lost");
    }

    #[test]
    fn a_conversation_left_with_two_running_follow_ups_keeps_one_and_can_start_no_more() {
        let conn = Connection::open_in_memory().unwrap();
        let before = MIGRATIONS
            .iter()
            .position(|m| m.contains("sequence_runs_one_running"))
            .unwrap();
        for sql in &MIGRATIONS[..before] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", before as i64)
            .unwrap();
        conn.execute_batch(
            r#"INSERT INTO accounts (id, email, display_name, imap_host, imap_port, security, username)
                   VALUES (1, 'me@x.test', 'Me', 'x', 993, 'tls', 'me');
               INSERT INTO sequence_runs (sequence_id, account_id, thread_id, message_id, started_at,
                   seen_upto, sent, next_at, state)
                   VALUES (NULL, 1, 't', 1, 0, 0, 0, 10, 'running'),
                          (NULL, 1, 't', 1, 0, 0, 0, 10, 'running'),
                          (NULL, 1, 'u', 1, 0, 0, 0, 10, 'running');"#,
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let states: Vec<(i64, String)> = {
            let db = s.db();
            let mut st = db
                .prepare("SELECT id, state FROM sequence_runs ORDER BY id")
                .unwrap();
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(
            states,
            vec![
                (1, "running".to_string()),
                (2, "stopped".to_string()),
                (3, "running".to_string())
            ]
        );
        let again = s.db().execute(
            "INSERT INTO sequence_runs (sequence_id, account_id, thread_id, message_id, started_at,
                 seen_upto, sent, next_at, state)
             VALUES (NULL, 1, 't', 1, 0, 0, 0, 10, 'running')",
            [],
        );
        assert!(again.is_err());
    }

    #[test]
    fn a_team_saved_before_several_were_possible_comes_through_whole() {
        let conn = Connection::open_in_memory().unwrap();
        // Everything before the change to several teams, wherever that sits in the list.
        let teams = MIGRATIONS
            .iter()
            .position(|m| m.contains("team_people_by_team"))
            .unwrap();
        let older = &MIGRATIONS[..teams];
        for sql in older {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", older.len() as i64)
            .unwrap();
        conn.execute_batch(
            r#"INSERT INTO team_info (id, team_id, name, me, role, members_version, vault_seen, seq)
                   VALUES (1, 42, 'Grayic', 5, 'admin', 3, 4, 9);
               INSERT INTO team_people (user_id, email, role) VALUES (5, 'a@x.test', 'admin'), (6, 'b@x.test', 'member');
               INSERT INTO team_threads (thread_key, status, status_at, sent_count, last_sent_at, seq)
                   VALUES ('k', 'open', 0, 0, 0, 1);
               INSERT INTO team_comments (id, thread_key, author, text, created_at, seq)
                   VALUES ('c', 'k', 5, 'Note', 1, 2);
               INSERT INTO team_templates (id, name, body, version, seq) VALUES ('t', 'Hello', 'Hi', 1, 3);
               INSERT INTO draft_team_base (draft_id, thread_key, sent_count) VALUES (1, 'k', 0);
               INSERT INTO accounts (email, display_name, imap_host, imap_port, security, username, sync_id, prefs_json)
                   VALUES ('s@x.test', 'S', 'x', 993, 'tls', 's', 'one', '{"team_shared":true}'),
                          ('p@x.test', 'P', 'x', 993, 'tls', 'p', 'two', '{}');"#,
        )
        .unwrap();

        let s = Store::init(conn).unwrap();
        let info = s.team_info(42).unwrap().unwrap();
        assert_eq!(
            (info.name.as_str(), info.me, info.role.as_str()),
            ("Grayic", 5, "admin")
        );
        assert_eq!(s.team_seq(42).unwrap(), 9);
        assert_eq!(s.team_vault_seen(42).unwrap(), 4);
        assert_eq!(s.team_members_version(42).unwrap(), 3);
        assert_eq!(s.team_members(42).unwrap().len(), 2);
        assert_eq!(
            s.team_member_email(42, 6).unwrap().as_deref(),
            Some("b@x.test")
        );
        assert_eq!(s.team_templates(Some(42)).unwrap()[0].name, "Hello");
        assert_eq!(s.team_comment_team("c").unwrap(), Some(42));
        assert!(s.team_thread("k").unwrap().is_some());
        let accounts = s.list_accounts().unwrap();
        let shared = accounts.iter().find(|a| a.email == "s@x.test").unwrap();
        assert_eq!(shared.prefs.team_id, 42);
        let personal = accounts.iter().find(|a| a.email == "p@x.test").unwrap();
        assert_eq!(personal.prefs.team_id, 0);

        // A second team sits beside it, and wiping one leaves the other.
        s.save_team_info(
            43,
            "Second",
            5,
            "owner",
            1,
            &[(5, "a@x.test".into(), "owner".into())],
        )
        .unwrap();
        assert_eq!(s.team_infos().unwrap().len(), 2);
        assert_eq!(s.team_member_email(43, 6).unwrap(), None);
        s.wipe_team(43).unwrap();
        assert_eq!(s.team_infos().unwrap().len(), 1);
        assert_eq!(s.team_members(42).unwrap().len(), 2);
        s.wipe_team(42).unwrap();
        assert!(s.team_thread("k").unwrap().is_none());
        assert!(s.draft_team_base(1).unwrap().is_none());
    }

    #[test]
    fn inboxes_keep_the_order_they_were_dragged_into() {
        let s = Store::open_in_memory().unwrap();
        let add = |email: &str| {
            s.insert_account(email, "", "x.test", 993, Security::Tls, email)
                .unwrap()
                .id
        };
        let ids = || {
            s.list_accounts()
                .unwrap()
                .iter()
                .map(|a| a.id)
                .collect::<Vec<_>>()
        };
        let (a, b, c) = (add("a@x.test"), add("b@x.test"), add("c@x.test"));
        assert_eq!(ids(), [a, b, c]);
        s.set_setting("inbox_order", &format!("{c},{a},{b}"))
            .unwrap();
        assert_eq!(ids(), [c, a, b]);
        // A new one goes last, and one that's gone leaves no gap.
        let d = add("d@x.test");
        s.delete_account(a).unwrap();
        assert_eq!(ids(), [c, b, d]);
    }

    fn setup() -> (Store, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        let a = s
            .insert_account("a@x.test", "A", "x.test", 993, Security::Tls, "a")
            .unwrap();
        let f = s
            .replace_folders(a.id, &[("INBOX".into(), "Inbox".into(), "inbox".into())])
            .unwrap();
        (s, a.id, f[0].0)
    }

    /// As if filters had looked at everything new, so it shows in the lists.
    fn filters_done(s: &Store) {
        s.db()
            .execute("UPDATE messages SET rule_done = 1", [])
            .unwrap();
    }

    fn msg(uid: u32, id: &str, refs: &[&str]) -> IncomingMessage {
        IncomingMessage {
            uid,
            is_draft: false,
            seen: false,
            flagged: false,
            size: 10,
            internal_date: Some(uid as i64),
            signals: None,
            headers: ParsedHeaders {
                rfc_id: Some(id.into()),
                references: refs.iter().map(|s| s.to_string()).collect(),
                subject: format!("s{uid}"),
                ..Default::default()
            },
        }
    }

    #[test]
    fn new_mail_waits_out_of_sight_until_filters_have_looked() {
        let (s, a, f) = setup();
        // Long after the server got it: how long it's held counts from reaching this Mac.
        let late = msg(1, "n", &[]);
        let mut stuck = msg(2, "o", &[]);
        stuck.headers.references = vec!["n".into()];
        let mut alone = msg(3, "x", &[]);
        alone.internal_date = Some(30);
        s.insert_messages(a, f, &[late]).unwrap();
        let first = s.rule_mail_new(a, 10).unwrap();
        s.mark_rule_done(first[0].id, &first[0].key).unwrap();
        s.insert_messages(a, f, &[stuck, alone]).unwrap();
        let listed = |grouped| -> Vec<(String, i64)> {
            s.list_threads_on_tab(f, 50, 0, grouped, None)
                .unwrap()
                .into_iter()
                .map(|t| (t.subject, t.unread_count))
                .collect()
        };
        // The reply waiting in a conversation doesn't show in its row or its count.
        assert_eq!(listed(true), [("s1".to_string(), 1)]);
        assert_eq!(listed(false), [("s1".to_string(), 1)]);
        assert_eq!(
            s.tab_counts(f, true).unwrap()["good_to_know"] + s.tab_counts(f, true).unwrap()["rest"],
            1
        );
        // Filters that never finish don't hide it for long.
        s.db()
            .execute("UPDATE messages SET held_until = 1 WHERE uid = 3", [])
            .unwrap();
        assert_eq!(listed(false).len(), 2);
        for m in s.rule_mail_new(a, 10).unwrap() {
            s.mark_rule_done(m.id, &m.key).unwrap();
        }
        assert_eq!(listed(false).len(), 3);
        assert_eq!(listed(true).len(), 2);
    }

    #[test]
    fn a_tab_a_filter_chose_stays_when_sorting_comes_after() {
        let (s, a, f) = setup();
        s.insert_messages(a, f, &[msg(1, "n", &[])]).unwrap();
        let id = s.rule_mail_new(a, 10).unwrap()[0].id;
        s.choose_tab(id, "newsletters").unwrap();
        s.sort_into_tab(id, "general").unwrap();
        s.save_ai_sort(id, Some("general"), None).unwrap();
        let tab: String = s
            .db()
            .query_row("SELECT category FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tab, "newsletters");
    }

    #[test]
    fn receipts_gather_in_purchases() {
        let (s, a, f) = setup();
        let mut receipt = msg(1, "r", &[]);
        receipt.headers.subject = "Your receipt from Apple".into();
        let mut offer = msg(2, "o", &[]);
        offer.headers.subject = "Get 20% cashback with City Bank Cards".into();
        s.insert_messages(a, f, &[receipt, offer]).unwrap();
        // Worked out as it arrives: nothing left for the background round.
        assert_eq!(s.sort_purchases().unwrap(), 0);
        s.db()
            .execute("UPDATE messages SET purchase = NULL", [])
            .unwrap();
        assert_eq!(s.sort_purchases().unwrap(), 2);
        filters_done(&s);
        let listed: Vec<String> = s
            .list_marked("purchases")
            .unwrap()
            .into_iter()
            .map(|t| t.subject)
            .collect();
        assert_eq!(listed, ["Your receipt from Apple"]);
        // Still in the inbox too.
        assert_eq!(
            s.list_threads_on_tab(f, 50, 0, true, None).unwrap().len(),
            2
        );
    }

    #[test]
    #[ignore = "about 2 minutes to fill: cargo test -- --ignored finding_copies"]
    fn finding_copies_stays_quick_in_a_big_mailbox() {
        let (s, a, f) = setup();
        let mail: Vec<IncomingMessage> = (1..=40_000)
            .map(|uid| msg(uid, &format!("m{uid}"), &[]))
            .collect();
        s.insert_messages(a, f, &mail).unwrap();
        let ids: Vec<(i64, String)> = {
            let db = s.db();
            let mut st = db
                .prepare("SELECT id, thread_id FROM messages ORDER BY id LIMIT 2000")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        // Marking a whole inbox read looks up every email's copies, one after another.
        let started = Instant::now();
        for (id, thread) in &ids {
            assert_eq!(s.thread_copies(a, &format!("msg:{id}")).unwrap().len(), 1);
            assert_eq!(s.thread_copies(a, thread).unwrap().len(), 1);
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn whether_a_body_is_saved_is_kept_on_the_email() {
        let (s, a, f) = setup();
        s.insert_messages(a, f, &[msg(1, "a", &[]), msg(2, "b", &[])])
            .unwrap();
        let id = |uid: u32| -> i64 {
            s.db()
                .query_row(
                    "SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2",
                    params![f, uid],
                    |r| r.get(0),
                )
                .unwrap()
        };
        // The id first: the database is held by one question at a time.
        let mark = |uid: u32| -> i64 {
            let id = id(uid);
            s.db()
                .query_row("SELECT has_body FROM messages WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let wanted = || -> Vec<u32> {
            s.bodies_wanted(f, 0, u32::MAX, 10)
                .unwrap()
                .into_iter()
                .map(|(uid, _)| uid)
                .collect()
        };
        let body = |text: &str| Body {
            text: Some(text.into()),
            html: None,
            attachments: Vec::new(),
            pgp: None,
        };
        assert_eq!(wanted(), [2, 1]);
        // An email with a file to open is marked so; one with only a picture in its text isn't.
        let files = |uid: u32| -> i64 {
            let id = id(uid);
            s.db()
                .query_row("SELECT has_files FROM messages WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let with = |inline: bool| Body {
            text: Some("see attached".into()),
            html: None,
            attachments: vec![AttachmentInfo {
                name: "a.pdf".into(),
                mime: "application/pdf".into(),
                size: 10,
                part: Some(1),
                inline,
            }],
            pgp: None,
        };
        s.save_body(id(2), &with(false)).unwrap();
        assert_eq!(files(2), 1);
        s.save_body(id(2), &with(true)).unwrap();
        assert_eq!(files(2), 0);
        // The id first: the database is held by one question at a time.
        let two = id(2);
        s.db()
            .execute("DELETE FROM bodies WHERE message_id = ?1", [two])
            .unwrap();
        assert_eq!(wanted(), [2, 1]);
        s.save_body(id(1), &body("hi")).unwrap();
        assert_eq!((mark(1), mark(2)), (1, 0));
        assert_eq!(wanted(), [2]);
        // Saved again it's still saved; taken away, it's wanted again.
        s.save_body(id(1), &body("hi again")).unwrap();
        assert_eq!(mark(1), 1);
        let one = id(1);
        s.db()
            .execute("DELETE FROM bodies WHERE message_id = ?1", [one])
            .unwrap();
        assert_eq!(mark(1), 0);
        assert_eq!(wanted(), [2, 1]);
    }

    /// What the refresh and the sidebar ask every few seconds reads a small index, never every
    /// email nor its body: on a big, encrypted mailbox that's what kept the Mac busy.
    #[test]
    fn what_is_asked_all_the_time_reads_only_an_index() {
        let (s, _, _) = setup();
        let plan = |sql: &str| -> String {
            let db = s.db();
            let mut st = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let zeros = vec![0i64; st.parameter_count()];
            let rows = st
                .query_map(params_from_iter(zeros), |r| r.get::<_, String>(3))
                .unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join(" | ")
        };
        for (what, sql) in [
            ("bodies still to come", BODIES_WANTED.to_string()),
            ("health", HEALTH_FOLDERS.to_string()),
            ("twin bodies", TWIN_BODIES.to_string()),
            ("folder counts", folder_counts_sql(true)),
            ("folder counts by email", folder_counts_sql(false)),
        ] {
            let plan = plan(&sql);
            assert!(
                !plan.contains("SCAN bodies") && !plan.contains("SEARCH bodies"),
                "{what} reads bodies: {plan}"
            );
            assert!(
                !plan.contains("SCAN m ") && !plan.ends_with("SCAN m"),
                "{what} reads every email: {plan}"
            );
        }
        assert!(plan(HEALTH_FOLDERS).contains("COVERING INDEX"));
        assert!(plan(&folder_counts_sql(true)).contains("COVERING INDEX"));

        // What every refresh runs for the spam check, learning, bulk mail and rules: only the
        // few still to do are read, bodies included, never every email.
        for (what, sql) in [
            ("spam check", mail_to_check_sql()),
            ("learning from junk", learn_sql(LEARN_FROM_JUNK)),
            ("learning from good mail", learn_sql(LEARN_FROM_GOOD)),
            ("bulk mail", MASS_MAIL_TO_SORT.to_string()),
            (
                "rules",
                crate::rules::rule_mail_sql(crate::rules::RULE_MAIL_NEW),
            ),
        ] {
            let plan = plan(&sql);
            assert!(
                !plan.contains("SCAN m ")
                    && !plan.ends_with("SCAN m")
                    && !plan.contains("SCAN m |"),
                "{what} reads every email: {plan}"
            );
            assert!(!plan.contains("SCAN b"), "{what} reads every body: {plan}");
        }

        // The lists themselves, in every tab, and how many need the person: read from one index
        // that holds what they sort and group by, never from the emails' own rows.
        for tab in [None, Some("needs_you"), Some("good_to_know"), Some("rest")] {
            for unread in [false, true] {
                let plan = plan(&page_keys_sql("inbox", true, tab, unread));
                assert!(
                    plan.contains("COVERING INDEX messages_list"),
                    "the {tab:?} list reads emails: {plan}"
                );
            }
        }
        let needs = plan(&crate::priority::needs_you_sql(true));
        assert!(
            needs.contains("COVERING INDEX messages_list"),
            "needs you: {needs}"
        );
        // Each row of a list says if it has files without reading any email's body.
        let rows = plan(&summary_sql(
            "m.thread_id",
            "m.account_id = ?1 AND m.thread_id IN (?2)",
            "GROUP BY m.thread_id",
        ));
        assert!(!rows.contains("SEARCH b "), "list rows read bodies: {rows}");
    }

    #[test]
    fn a_read_mark_changed_here_stays_until_the_server_has_it_or_cant_be_told() {
        let (s, a, f) = setup();
        s.insert_messages(a, f, &[msg(1, "a", &[]), msg(2, "b", &[])])
            .unwrap();
        let row = |uid: u32, what: &str| -> i64 {
            s.db()
                .query_row(
                    &format!("SELECT {what} FROM messages WHERE folder_id = ?1 AND uid = ?2"),
                    params![f, uid],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let copy = |uid: u32| ThreadCopy {
            is_draft: false,
            id: row(uid, "id"),
            folder_id: f,
            folder_path: "INBOX".into(),
            folder_role: "inbox".into(),
            uid_validity: None,
            uid,
            seen: false,
            flagged: false,
        };
        let (one, two) = (copy(1), copy(2));
        s.set_seen_held(&[&one, &two], true).unwrap();
        // A refresh that read the server before it heard.
        let stale = [(1, false, true), (2, false, true)];
        s.apply_flags(f, &stale, false).unwrap();
        assert_eq!((row(1, "seen"), row(2, "seen")), (1, 1));
        assert_eq!(
            (row(1, "flagged"), row(2, "flagged")),
            (1, 1),
            "stars still come"
        );
        // The server has the first now, and still a refresh that read it a moment before can
        // land. The second couldn't be told, so the server's mark is the one that counts.
        s.seen_heard(&[(f, 1)], true);
        s.seen_heard(&[(f, 2)], false);
        s.apply_flags(f, &stale, false).unwrap();
        assert_eq!((row(1, "seen"), row(2, "seen")), (1, 0));
    }

    #[test]
    fn a_search_puts_subject_matches_first_only_when_it_tells_them_apart() {
        let (s, a, f) = setup();
        let mut older = msg(1, "a", &[]);
        older.headers.subject = "Your invoice".into();
        let mut newer = msg(2, "b", &[]);
        newer.headers.subject = "Hello".into();
        s.insert_messages(a, f, &[older, newer]).unwrap();
        let ids: Vec<i64> = {
            let db = s.db();
            let mut st = db.prepare("SELECT id FROM messages ORDER BY id").unwrap();
            let rows = st.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        let filters = SearchFilters {
            words: vec!["invoice".into()],
            ..Default::default()
        };
        let (list, _) = s.search_threads(&filters, &ids, 50, true).unwrap();
        let shown: Vec<(&str, bool)> = list
            .iter()
            .map(|t| (t.subject.as_str(), t.best_match))
            .collect();
        assert_eq!(
            shown,
            vec![("Your invoice", true), ("Hello", false)],
            "not newest first"
        );

        let everything = SearchFilters {
            words: vec!["o".into()],
            ..Default::default()
        };
        let (list, _) = s.search_threads(&everything, &ids, 50, true).unwrap();
        assert!(list.iter().all(|t| !t.best_match), "all of them is no help");
        assert_eq!(list[0].subject, "Hello", "so newest first stays");
    }

    #[test]
    fn the_unread_filter_leaves_out_what_has_been_read() {
        let (s, a, f) = setup();
        let mut read = msg(1, "a", &[]);
        read.seen = true;
        s.insert_messages(a, f, &[read, msg(2, "b", &[])]).unwrap();
        filters_done(&s);
        let all = s
            .list_threads_filtered(f, 50, 0, true, None, false)
            .unwrap();
        assert_eq!(all.len(), 2);
        let unread = s.list_threads_filtered(f, 50, 0, true, None, true).unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].unread_count, 1);
    }

    #[test]
    fn replies_join_their_thread() {
        let (s, a, f) = setup();
        s.insert_messages(
            a,
            f,
            &[msg(1, "a", &[]), msg(2, "b", &["a"]), msg(3, "z", &[])],
        )
        .unwrap();
        filters_done(&s);
        let t = s.list_threads(f, 50, 0).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].message_count, 1);
        assert_eq!(t[1].message_count, 2);
    }

    #[test]
    fn reply_arriving_before_its_parent_still_threads() {
        let (s, a, f) = setup();
        s.insert_messages(a, f, &[msg(1, "b", &["a"]), msg(2, "a", &[])])
            .unwrap();
        filters_done(&s);
        assert_eq!(s.list_threads(f, 50, 0).unwrap().len(), 1);
    }

    #[test]
    fn a_later_message_merges_two_separate_threads() {
        let (s, a, f) = setup();
        // "c" only names "b", and "b" only names "a", so they start apart.
        s.insert_messages(a, f, &[msg(1, "a", &[]), msg(2, "c", &["b"])])
            .unwrap();
        filters_done(&s);
        assert_eq!(s.list_threads(f, 50, 0).unwrap().len(), 2);
        s.insert_messages(a, f, &[msg(3, "b", &["a"])]).unwrap();
        filters_done(&s);
        let t = s.list_threads(f, 50, 0).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].message_count, 3);
    }

    #[test]
    fn messages_without_ids_do_not_clump_together() {
        let (s, a, f) = setup();
        let mut one = msg(1, "x", &[]);
        one.headers.rfc_id = None;
        let mut two = msg(2, "y", &[]);
        two.headers.rfc_id = None;
        s.insert_messages(a, f, &[one, two]).unwrap();
        filters_done(&s);
        assert_eq!(s.list_threads(f, 50, 0).unwrap().len(), 2);
    }

    #[test]
    fn deleting_an_account_removes_all_its_mail() {
        let (s, a, f) = setup();
        s.insert_messages(a, f, &[msg(1, "a", &[])]).unwrap();
        s.delete_account(a).unwrap();
        let n: i64 = s
            .db()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn deleted_mail_gives_its_room_back_to_the_mac() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        let store = Store::open(&path).unwrap();
        let big = "x".repeat(200_000);
        for i in 0..400 {
            store.set_setting(&format!("filler{i}"), &big).unwrap();
        }
        store
            .db()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        let full = std::fs::metadata(&path).unwrap().len();
        store
            .db()
            .execute("DELETE FROM settings WHERE key LIKE 'filler%'", [])
            .unwrap();
        store.give_room_back().join().unwrap();
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(after < full / 4, "{full} bytes before, {after} after");
        // And it still works afterwards.
        store.set_setting("still", "here").unwrap();
        assert_eq!(store.setting("still").unwrap().as_deref(), Some("here"));
    }

    #[test]
    fn opening_a_conversation_does_not_wait_for_the_database_lock() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store::open(&dir.path().join("mail.db")).unwrap());
        let _busy = store.db();
        let reading = store.clone();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            done.send(reading.get_thread(1, "t", None).map(|list| list.len()))
                .unwrap();
        });
        let found = finished
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("waited for the lock");
        assert_eq!(found.unwrap(), 0);
    }

    #[test]
    fn upgrading_a_database_keeps_existing_inboxes_and_mail() {
        // A database from before sign-in existed, like one already in use.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1i64).unwrap();
            conn.execute(
                "INSERT INTO accounts (email, display_name, imap_host, imap_port, security, username)
                 VALUES ('a@x.test', 'A', 'x.test', 993, 'tls', 'a'), ('b@x.test', 'B', 'x.test', 993, 'tls', 'b')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO folders (account_id, path, name, role) VALUES (1, 'INBOX', 'Inbox', 'inbox')",
                [],
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let accounts = s.list_accounts().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].sync_id.len(), 32);
        assert_ne!(accounts[0].sync_id, accounts[1].sync_id);
        assert_eq!(s.list_folders(1).unwrap().len(), 1);
        assert!(s.identity().unwrap().is_none());
    }

    #[test]
    fn migrations_run_once_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        drop(Store::open(&path).unwrap());
        let s = Store::open(&path).unwrap();
        assert!(s.list_accounts().unwrap().is_empty());
    }

    /// The quick look at recent mail gives just what going through all of it would: a pinned
    /// old conversation and an old one answered today still come first.
    #[test]
    fn the_quick_look_lists_what_the_whole_look_does() {
        let s = Store::open_in_memory().unwrap();
        let a = s
            .insert_account("a@x.test", "A", "x.test", 993, Security::Tls, "a")
            .unwrap();
        let f = s
            .replace_folders(
                a.id,
                &[
                    ("INBOX".into(), "Inbox".into(), "inbox".into()),
                    ("Sent".into(), "Sent".into(), "sent".into()),
                ],
            )
            .unwrap();
        let (inbox, sent) = (f[0].0, f[1].0);
        let now = chrono::Utc::now().timestamp();
        let day = 86_400;
        // One email a day for 200 days.
        let mail: Vec<IncomingMessage> = (1..=200u32)
            .map(|uid| {
                let mut m = msg(uid, &format!("i{uid}"), &[]);
                m.internal_date = Some(now - (200 - uid as i64) * day);
                m
            })
            .collect();
        s.insert_messages(a.id, inbox, &mail).unwrap();
        // A reply sent today to the email from 150 days ago.
        let mut reply = msg(1, "r1", &["i50"]);
        reply.internal_date = Some(now);
        s.insert_messages(a.id, sent, &[reply]).unwrap();
        filters_done(&s);
        let old_thread: String = s
            .db()
            .query_row(
                "SELECT thread_id FROM messages WHERE rfc_id = 'i10'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        s.db()
            .execute(
                "INSERT INTO thread_marks (account_id, thread_id, pinned) VALUES (?1, ?2, 1)",
                params![a.id, old_thread],
            )
            .unwrap();
        s.db()
            .execute(
                "UPDATE messages SET priority = 'needs_you' WHERE id % 3 = 0",
                [],
            )
            .unwrap();
        for grouped in [true, false] {
            for tab in [None, Some("needs_you"), Some("good_to_know")] {
                for limit in [5, 12, 40] {
                    let whole: Vec<String> = {
                        let db = s.reading_db();
                        let mut st = db
                            .prepare(&page_keys_sql("inbox", grouped, tab, false))
                            .unwrap();
                        st.query_map(params![inbox, limit, 0], |r| {
                            Ok(format!("{:?}", r.get::<_, SqlValue>(0)?))
                        })
                        .unwrap()
                        .collect::<rusqlite::Result<_>>()
                        .unwrap()
                    };
                    let quick: Vec<String> = s
                        .page_keys(inbox, limit, 0, grouped, tab, false)
                        .unwrap()
                        .into_iter()
                        .map(|k| format!("{:?}", k.key))
                        .collect();
                    assert_eq!(quick, whole, "grouped {grouped}, tab {tab:?}, {limit} rows");
                }
            }
        }
        // The pinned one leads, and the one answered today comes next.
        let top = s.list_threads(inbox, 3, 0).unwrap();
        assert_eq!(top[0].thread_id, old_thread);
    }

    /// Run by hand: `cargo test -p mailcore --release big_inbox_lists_quickly -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn big_inbox_lists_quickly() {
        let dir = std::env::temp_dir().join(format!("mailzo-bench-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.db");
        let s = Store::open(&path).unwrap();
        let mut inboxes = vec![];
        for n in 0..3 {
            let a = s
                .insert_account(
                    &format!("a{n}@x.test"),
                    "A",
                    "x.test",
                    993,
                    Security::Tls,
                    "a",
                )
                .unwrap();
            let f = s
                .replace_folders(
                    a.id,
                    &[
                        ("INBOX".into(), "Inbox".into(), "inbox".into()),
                        ("Sent".into(), "Sent".into(), "sent".into()),
                    ],
                )
                .unwrap();
            for (chunk, folder) in [(0u32, f[0].0), (1, f[1].0)] {
                let count = if chunk == 0 { 40_000 } else { 6_000 };
                let mail: Vec<IncomingMessage> = (1..=count)
                    .map(|uid| {
                        let id = format!("{n}-{chunk}-{uid}");
                        // Threads of about four, replies pointing at the first.
                        let root = format!("{n}-0-{}", (uid / 4) * 4 + 1);
                        let refs: Vec<&str> = if chunk == 0 && uid % 4 == 1 {
                            vec![]
                        } else {
                            vec![root.as_str()]
                        };
                        let mut m = msg(uid, &id, &refs);
                        m.internal_date =
                            Some(chrono::Utc::now().timestamp() - (count - uid) as i64 * 600);
                        m
                    })
                    .collect();
                s.insert_messages(a.id, folder, &mail).unwrap();
            }
            inboxes.push(f[0].0);
        }
        filters_done(&s);
        s.db()
            .execute_batch("UPDATE messages SET priority = 'needs_you' WHERE id % 7 = 0; ANALYZE;")
            .unwrap();
        {
            let db = s.reading_db();
            let mut st = db
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN {}",
                    page_keys_sql_from("inbox", true, None, false, true)
                ))
                .unwrap();
            let rows: Vec<String> = st
                .query_map(params![inboxes[0], 200, 0, 0], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            eprintln!("PLAN:\n{}", rows.join("\n"));
        }
        for tab in [None, Some("needs_you")] {
            let started = Instant::now();
            for f in &inboxes {
                s.page_keys(*f, 200, 0, true, tab, false).unwrap();
            }
            eprintln!("keys {tab:?} in {:?}", started.elapsed());
            let started = Instant::now();
            for f in &inboxes {
                let db = s.reading_db();
                let mut st = db
                    .prepare(&page_keys_sql("inbox", true, tab, false))
                    .unwrap();
                let n = st
                    .query_map(params![f, 200, 0], |_| Ok(()))
                    .unwrap()
                    .count();
                assert!(n > 0);
            }
            eprintln!("whole keys {tab:?} in {:?}", started.elapsed());
        }
        for tab in [None, Some("needs_you"), Some("good_to_know"), Some("rest")] {
            let started = Instant::now();
            let rows = s.list_across(&inboxes, 200, 0, true, tab, false).unwrap();
            eprintln!(
                "tab {tab:?}: {} rows in {:?}",
                rows.len(),
                started.elapsed()
            );
        }
        let started = Instant::now();
        for f in &inboxes {
            s.tab_counts(*f, true).unwrap();
        }
        eprintln!("tab counts in {:?}", started.elapsed());
        drop(s);
        let started = Instant::now();
        drop(Store::open(&path).unwrap());
        eprintln!("opened again in {:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
