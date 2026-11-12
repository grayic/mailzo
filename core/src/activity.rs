//! What connected AI apps have done in the mail, so the person can check. It stays on this device,
//! holds who and which subject but never what an email says, and goes when they sign out.

use crate::engine::Engine;
use crate::error::Result;
use crate::identity::unix_now;
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

const KEEP_SECONDS: i64 = 90 * 86_400;
const MOST_SHOWN: i64 = 2_000;
const WORDS_KEPT: usize = 200;
const RECIPIENTS_KEPT: usize = 2_000;

/// The kinds a person can filter by. An app asking how its own send went isn't doing anything.
fn kind_of(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "list_inboxes" | "search_mail" | "recent_mail" | "read_conversation" | "list_folders"
        | "list_drafts" | "read_attachment" | "spam_or_trash" => "reading",
        "mark_read"
        | "archive_conversation"
        | "move_conversation"
        | "delete_conversation"
        | "delete_forever" => "organizing",
        "save_draft" | "send_email" => "sending",
        _ => return None,
    })
}

/// What apps call themselves, in the words the MCP page uses.
fn app_name(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let known = [
        ("claude-code", "Claude Code"),
        ("claude code", "Claude Code"),
        ("claude", "Claude"),
        ("codex", "Codex"),
        ("cursor", "Cursor"),
        ("windsurf", "Windsurf"),
        ("visual studio code", "VS Code"),
        ("vscode", "VS Code"),
    ];
    match known.iter().find(|(word, _)| lower.contains(word)) {
        Some((_, name)) => name.to_string(),
        None if raw.trim().is_empty() => "An AI app".to_string(),
        None => clip(raw),
    }
}

fn clip(text: &str) -> String {
    text.trim().chars().take(WORDS_KEPT).collect()
}

/// Everyone, up to a limit, saying so when some don't fit.
fn people_in_words(people: &[String]) -> String {
    let mut words = String::new();
    for (i, who) in people.iter().enumerate() {
        if words.chars().count() + who.chars().count() > RECIPIENTS_KEPT {
            return format!("{words} and {} more", people.len() - i)
                .trim_start()
                .to_string();
        }
        if i > 0 {
            words.push_str(", ");
        }
        words.push_str(who);
    }
    words
}

fn was_refused(problem: &str) -> bool {
    problem.contains("allowed") || problem.contains("turned off")
}

/// Which conversation a tool is about, as it is before the tool changes or removes it.
pub(crate) struct Brief {
    account_id: i64,
    thread_id: String,
    folder_id: i64,
    subject: String,
    who: String,
}

impl Store {
    /// By conversation, or by one email when `account_id` is None.
    fn activity_brief(
        &self,
        account_id: Option<i64>,
        thread_id: &str,
        email: Option<i64>,
    ) -> Option<Brief> {
        self.reading_db()
            .query_row(
                "SELECT m.account_id, COALESCE(m.thread_id, 'msg:' || m.id), m.folder_id, m.subject,
                        CASE WHEN m.from_name = '' THEN m.from_addr ELSE m.from_name END
                 FROM messages m
                 WHERE (?1 IS NULL OR m.account_id = ?1) AND (m.thread_id = ?2 OR m.id = ?3)
                 ORDER BY m.date DESC LIMIT 1",
                params![account_id, thread_id, email],
                |r| {
                    Ok(Brief {
                        account_id: r.get(0)?,
                        thread_id: r.get(1)?,
                        folder_id: r.get(2)?,
                        subject: r.get(3)?,
                        who: r.get(4)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    fn known_account(&self, account_id: i64) -> Option<i64> {
        self.reading_db()
            .query_row("SELECT id FROM accounts WHERE id = ?1", [account_id], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    #[allow(clippy::too_many_arguments)]
    fn save_activity(
        &self,
        app: &str,
        tool: &str,
        kind: &str,
        outcome: &str,
        problem: Option<&str>,
        account_id: Option<i64>,
        brief: Option<&Brief>,
        detail: &Value,
    ) -> Result<()> {
        let now = unix_now();
        let db = self.db();
        db.execute(
            "INSERT INTO agent_activity
                 (at, app, tool, kind, outcome, problem, account_id, thread_id, folder_id,
                  subject, who, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                now,
                app,
                tool,
                kind,
                outcome,
                problem,
                account_id,
                brief.map(|b| b.thread_id.as_str()),
                brief.map(|b| b.folder_id),
                brief.map(|b| b.subject.as_str()),
                brief.map(|b| b.who.as_str()),
                detail.to_string(),
            ],
        )?;
        db.execute(
            "DELETE FROM agent_activity WHERE at < ?1",
            [now - KEEP_SECONDS],
        )?;
        Ok(())
    }

    fn list_activity(&self) -> Result<Vec<Value>> {
        let db = self.reading_db();
        // What really changed always stays. Reading and being refused each have their own limit,
        // so a looping app can't bury a change, or either of those under the other.
        let mut st = db.prepare_cached(
            "SELECT id, at, app, tool, kind, outcome, problem, account_id, thread_id, folder_id,
                    subject, who, detail
             FROM agent_activity
             WHERE (kind != 'reading' AND outcome = 'done')
                OR id IN (
                    SELECT id FROM agent_activity WHERE kind = 'reading' AND outcome = 'done'
                    ORDER BY at DESC, id DESC LIMIT ?1
                )
                OR id IN (
                    SELECT id FROM agent_activity WHERE outcome != 'done'
                    ORDER BY at DESC, id DESC LIMIT ?1
                )
             ORDER BY at DESC, id DESC",
        )?;
        let rows = st.query_map([MOST_SHOWN], |r| {
            let detail: String = r.get(12)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "at": r.get::<_, i64>(1)?,
                "app": r.get::<_, String>(2)?,
                "tool": r.get::<_, String>(3)?,
                "kind": r.get::<_, String>(4)?,
                "outcome": r.get::<_, String>(5)?,
                "problem": r.get::<_, Option<String>>(6)?,
                "account_id": r.get::<_, Option<i64>>(7)?,
                "thread_id": r.get::<_, Option<String>>(8)?,
                "folder_id": r.get::<_, Option<i64>>(9)?,
                "subject": r.get::<_, Option<String>>(10)?,
                "who": r.get::<_, Option<String>>(11)?,
                "detail": serde_json::from_str::<Value>(&detail).unwrap_or(Value::Null),
            }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn clear_activity(&self) -> Result<()> {
        self.db().execute("DELETE FROM agent_activity", [])?;
        Ok(())
    }
}

impl Engine {
    /// Looked up first, because some tools leave nothing to look up afterwards.
    /// Only by what this tool acts on, as an app can send along whatever else it likes.
    pub(crate) fn brief_for_activity(&self, tool: &str, args: &Value) -> Option<Brief> {
        let email = match tool {
            "read_attachment" => "message_id",
            "save_draft" | "send_email" => "reply_to_message_id",
            "read_conversation"
            | "mark_read"
            | "archive_conversation"
            | "move_conversation"
            | "delete_conversation"
            | "delete_forever" => {
                let thread_id = args["thread_id"].as_str()?;
                let one_email = thread_id.strip_prefix("msg:").and_then(|i| i.parse().ok());
                return self.store.activity_brief(
                    Some(args["account_id"].as_i64()?),
                    thread_id,
                    one_email,
                );
            }
            _ => return None,
        };
        self.store
            .activity_brief(None, "", Some(args[email].as_i64()?))
    }

    /// A problem writing this down never costs the app its answer.
    pub(crate) fn note_agent_activity(
        &self,
        app: &str,
        tool: &str,
        args: &Value,
        brief: Option<Brief>,
        problem: Option<String>,
    ) {
        let Some(kind) = kind_of(tool) else {
            return;
        };
        let outcome = match problem.as_deref() {
            None => "done",
            Some(p) if was_refused(p) => "refused",
            Some(_) => "failed",
        };
        let text = |key: &str| clip(args[key].as_str().unwrap_or(""));
        let detail = match tool {
            "search_mail" => json!({ "query": text("query") }),
            "move_conversation" => json!({ "folder": text("folder") }),
            "spam_or_trash" => json!({ "folder": text("folder") }),
            "mark_read" => json!({ "read": args["read"].as_bool().unwrap_or(true) }),
            "save_draft" | "send_email" => {
                // Read just as the tool reads them, so this is who it really goes to.
                let people = |key: &str| -> Vec<String> {
                    crate::mcp::addresses(&args[key])
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| a.address)
                        .collect()
                };
                // With nobody named, a reply goes to whoever the email asks replies to go to.
                let mut to = people("to");
                if to.is_empty() {
                    if let Some(id) = args["reply_to_message_id"].as_i64() {
                        let account = args["account_id"].as_i64().unwrap_or(0);
                        to = self.reply_goes_to(account, id).unwrap_or_default();
                    }
                }
                json!({
                    "to": people_in_words(&to),
                    "cc": people_in_words(&people("cc")),
                    "subject": text("subject"),
                    "needs_ok": tool == "send_email" && !self.mcp_switch("mcp_send_unasked"),
                })
            }
            _ => Value::Null,
        };
        // A tool can be asked about an inbox that doesn't exist.
        let account_id = brief.as_ref().map(|b| b.account_id).or_else(|| {
            args["account_id"]
                .as_i64()
                .and_then(|id| self.store.known_account(id))
        });
        let problem = problem.as_deref().map(clip);
        let _ = self.store.save_activity(
            &app_name(app),
            tool,
            kind,
            outcome,
            problem.as_deref(),
            account_id,
            brief.as_ref(),
            &detail,
        );
    }

    pub(crate) fn agent_activity(&self) -> Result<Value> {
        Ok(json!(self.store.list_activity()?))
    }

    pub(crate) fn clear_agent_activity(&self) -> Result<Value> {
        self.store.clear_activity()?;
        Ok(json!({}))
    }
}
