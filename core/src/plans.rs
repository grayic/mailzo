//! Using an AI plan the person already pays for. The companies' rules don't let an app sign
//! people in or touch their login, so this runs the company's own program on this Mac, signed
//! in by the person through the company's own page. It never reads that login. Every tool the
//! program has is switched off, because an email can try to give the AI instructions.

use crate::ai::ai_error;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub(crate) const VIA_KEY: &str = "key";
const STATUS_WAIT: Duration = Duration::from_secs(15);
const ANSWER_WAIT: Duration = Duration::from_secs(180);
/// Each answer starts a whole program, so a few at a time is plenty.
pub(crate) const AT_ONCE: usize = 2;
pub(crate) const CLAUDE_MODELS: [&str; 3] = ["haiku", "sonnet", "opus"];

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Plan {
    Claude,
    ChatGpt,
}

impl Plan {
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude_code" => Some(Plan::Claude),
            "codex" => Some(Plan::ChatGpt),
            _ => None,
        }
    }

    fn program(self) -> &'static str {
        match self {
            Plan::Claude => "claude",
            Plan::ChatGpt => "codex",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Plan::Claude => "Claude",
            Plan::ChatGpt => "ChatGPT",
        }
    }
}

/// One subscription, as the settings page shows it.
#[derive(Debug, Serialize)]
pub struct PlanRow {
    pub id: &'static str,
    pub name: &'static str,
    pub program: &'static str,
    pub install_page: &'static str,
    pub found: bool,
    pub signed_in: bool,
}

/// Where the programs usually are. An app opened from the Dock doesn't get the terminal's search
/// path, so it has to look for itself.
fn find_program(home: &Path, name: &str, only_in: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = only_in {
        return Some(dir.join(name)).filter(|p| p.is_file());
    }
    let mut places: Vec<PathBuf> = vec![
        home.join(".local/bin"),
        home.join(format!(".{name}/local")),
        home.join(".npm-global/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];
    if name == "codex" {
        places.push(PathBuf::from("/Applications/Codex.app/Contents/Resources"));
    }
    places
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn with_quiet_env(command: &mut Command, home: &Path, program: &Path) {
    let folder = program.parent().unwrap_or(Path::new("/"));
    command
        .env_clear()
        .env("HOME", home)
        .env("USER", std::env::var_os("USER").unwrap_or_default())
        .env("LOGNAME", std::env::var_os("USER").unwrap_or_default())
        .env("LANG", "en_US.UTF-8")
        .env(
            "PATH",
            format!(
                "{}:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin",
                folder.display()
            ),
        )
        // The person's own notes for their coding work have no place in an email answer.
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
}

/// Only when measuring by hand, with INBOX_AI_LOG naming a file: how long each answer took
/// and what it used, one line each. The app never sets it, so it never writes this.
pub(crate) fn log_for_measuring(
    system: &str,
    question: &str,
    took: std::time::Duration,
    reply: &Value,
) {
    let Some(path) = std::env::var_os("INBOX_AI_LOG") else {
        return;
    };
    let line = serde_json::json!({
        "what": system.chars().take(60).collect::<String>(),
        "sent_chars": system.len() + question.len(),
        "wall_ms": took.as_millis() as u64,
        "api_ms": reply["duration_api_ms"],
        "usage": reply["usage"],
        "cost_usd": reply["total_cost_usd"],
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

fn claude_args(system: &str, model: &str) -> Vec<String> {
    [
        "-p",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--output-format",
        "json",
        "--model",
        model,
        "--system-prompt",
        system,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn codex_args(answer_file: &Path, folder: &Path, writing: bool) -> Vec<String> {
    let effort = if writing { "medium" } else { "low" };
    let mut args: Vec<String> = [
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "web_search=\"disabled\"",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push("-c".into());
    args.push(format!("model_reasoning_effort=\"{effort}\""));
    for feature in [
        "shell_tool",
        "apps",
        "browser_use",
        "computer_use",
        "plugins",
    ] {
        args.push("--disable".into());
        args.push(feature.into());
    }
    args.push("-o".into());
    args.push(answer_file.to_string_lossy().to_string());
    args.push("-C".into());
    args.push(folder.to_string_lossy().to_string());
    args.push("-".into());
    args
}

fn limit_or_failure(plan: Plan, said: &str) -> CoreError {
    let lower = said.to_ascii_lowercase();
    // "Not logged in", "Failed to authenticate: OAuth session expired" and the like.
    let signed_out = [
        "logged in",
        "/login",
        "sign in",
        "log in",
        "authenticat",
        "oauth",
        "expired",
        "unauthorized",
        "credentials",
        "api key",
    ];
    if signed_out.iter().any(|w| lower.contains(w)) {
        return ai_error(
            "ai_key",
            &format!("Sign in to {} again in Settings › AI.", plan.name()),
        );
    }
    if lower.contains("limit") || lower.contains("usage") || lower.contains("quota") {
        return ai_error(
            "ai_limit",
            &format!("You've reached your {} plan's limit for now.", plan.name()),
        );
    }
    ai_error("ai_failed", "The AI couldn't work that one out.")
}

impl Engine {
    /// The plan this device is set to use, if it isn't using a key.
    pub(crate) fn ai_plan(&self) -> Result<Option<Plan>> {
        Ok(Plan::from_id(&self.setting("ai_via")?))
    }

    fn plan_program(&self, plan: Plan) -> Option<PathBuf> {
        find_program(
            &self.ai_app_places.home,
            plan.program(),
            self.plan_programs.as_deref(),
        )
    }

    async fn plan_signed_in(&self, plan: Plan, program: &Path) -> bool {
        let mut command = Command::new(program);
        with_quiet_env(&mut command, &self.ai_app_places.home, program);
        match plan {
            Plan::Claude => command.args(["auth", "status"]),
            Plan::ChatGpt => command.args(["login", "status"]),
        };
        let Ok(Ok(out)) = tokio::time::timeout(STATUS_WAIT, command.output()).await else {
            return false;
        };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        match plan {
            Plan::Claude => serde_json::from_str::<Value>(text.trim())
                .is_ok_and(|v| v["loggedIn"] == true && v["authMethod"] != "apiKey"),
            // Signed in with a key would bill them separately, which isn't what they chose.
            Plan::ChatGpt => out.status.success() && text.contains("ChatGPT"),
        }
    }

    pub(crate) async fn ai_plans(&self) -> Result<Vec<PlanRow>> {
        let mut rows = Vec::new();
        for (id, plan, install_page) in [
            (
                "claude_code",
                Plan::Claude,
                "https://code.claude.com/docs/en/quickstart",
            ),
            (
                "codex",
                Plan::ChatGpt,
                "https://developers.openai.com/codex/cli",
            ),
        ] {
            let program = self.plan_program(plan);
            let signed_in = match &program {
                Some(p) => self.plan_signed_in(plan, p).await,
                None => false,
            };
            rows.push(PlanRow {
                id,
                name: plan.name(),
                program: plan.program(),
                install_page,
                found: program.is_some(),
                signed_in,
            });
        }
        Ok(rows)
    }

    /// Opens the company's own sign-in page, by way of its own program.
    pub(crate) fn ai_plan_sign_in(&self, id: &str) -> Result<Value> {
        let plan = Plan::from_id(id).ok_or_else(|| CoreError::Invalid("Bad request.".into()))?;
        let program = self
            .plan_program(plan)
            .ok_or_else(|| ai_error("ai_key", "Install it first."))?;
        let mut command = Command::new(&program);
        with_quiet_env(&mut command, &self.ai_app_places.home, &program);
        match plan {
            Plan::Claude => command.args(["auth", "login"]),
            Plan::ChatGpt => command.args(["login"]),
        };
        // It finishes by itself when the person is done, and the page keeps checking.
        command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(false);
        command
            .spawn()
            .map_err(|_| ai_error("ai_failed", "Couldn't open the sign-in page."))?;
        Ok(serde_json::json!({ "ok": true }))
    }

    /// Switches to a plan once a small question has come back through it.
    pub(crate) async fn ai_plan_use(&self, id: &str) -> Result<Value> {
        let plan = Plan::from_id(id).ok_or_else(|| CoreError::Invalid("Bad request.".into()))?;
        let answer = self
            .ask_plan(
                plan,
                false,
                "Reply with only this JSON object and nothing else: {\"ok\":true}",
                "Are you there?",
            )
            .await?;
        if !answer.contains("true") {
            return Err(ai_error("ai_failed", "That didn't give a usable answer."));
        }
        self.store.set_setting("ai_via", id)?;
        self.save_plan_picks(plan)?;
        self.settings_changed().await?;
        self.ai_status()
    }

    /// One question through the plan's program. The question goes in by the program's input,
    /// not its command line, where other programs on the Mac could see an email's words.
    pub(crate) async fn ask_plan(
        &self,
        plan: Plan,
        writing: bool,
        system: &str,
        question: &str,
    ) -> Result<String> {
        let program = self.plan_program(plan).ok_or_else(|| {
            ai_error(
                "ai_key",
                &format!("{} isn't installed on this Mac any more.", plan.program()),
            )
        })?;
        let _turn = self
            .plan_turns
            .acquire()
            .await
            .map_err(|_| ai_error("ai_failed", "The AI couldn't work that one out."))?;
        // An empty folder to start in, so it has nothing of the person's nearby.
        let folder = self.data_dir.join("ai-empty");
        std::fs::create_dir_all(&folder)
            .map_err(|_| ai_error("ai_failed", "The AI couldn't work that one out."))?;
        let answer_file = self
            .data_dir
            .join(format!("ai-answer-{}", crate::crypto::random_hex(8)));
        let mut command = Command::new(&program);
        with_quiet_env(&mut command, &self.ai_app_places.home, &program);
        command.current_dir(&folder).stdin(Stdio::piped());
        let mut input = question.to_string();
        match plan {
            Plan::Claude => {
                command.args(claude_args(system, &self.plan_model(writing)?));
            }
            Plan::ChatGpt => {
                command.args(codex_args(&answer_file, &folder, writing));
                input = format!("{system}\n\n{question}");
            }
        }
        let started = std::time::Instant::now();
        let mut child = command
            .spawn()
            .map_err(|_| ai_error("ai_failed", "Couldn't start the AI program."))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(|_| ai_error("ai_failed", "Couldn't start the AI program."))?;
        }
        let done = tokio::time::timeout(ANSWER_WAIT, child.wait_with_output()).await;
        let saved = std::fs::read_to_string(&answer_file);
        let _ = std::fs::remove_file(&answer_file);
        let out = done
            .map_err(|_| ai_error("ai_failed", "The AI took too long to answer."))?
            .map_err(|_| ai_error("ai_failed", "The AI couldn't work that one out."))?;
        let printed = String::from_utf8_lossy(&out.stdout);
        let errors = String::from_utf8_lossy(&out.stderr);
        self.note_ai_usage(crate::allowance::Usage::default(), "", false);
        match plan {
            Plan::Claude => {
                // Why it failed goes in the log, never the answer, which may quote an email.
                let value: Value = serde_json::from_str(printed.trim()).map_err(|_| {
                    eprintln!(
                        "claude: answer not readable (exit {:?}, {} chars out): {}",
                        out.status.code(),
                        printed.len(),
                        errors.chars().take(300).collect::<String>()
                    );
                    limit_or_failure(plan, &errors)
                })?;
                log_for_measuring(system, question, started.elapsed(), &value);
                let result = value["result"].as_str().unwrap_or("");
                if value["is_error"] == true || !out.status.success() {
                    eprintln!(
                        "claude: failed (exit {:?}, {}): {}",
                        out.status.code(),
                        value["subtype"],
                        // Only claude's own error, never what may be the reply's words.
                        if value["is_error"] == true && result.starts_with("API Error") {
                            result.chars().take(200).collect::<String>()
                        } else {
                            format!("{} chars", result.len())
                        }
                    );
                    return Err(limit_or_failure(plan, result));
                }
                Ok(crate::ai::without_code_fence(result).to_string())
            }
            Plan::ChatGpt => match saved {
                Ok(text) if out.status.success() => {
                    Ok(crate::ai::without_code_fence(&text).to_string())
                }
                _ => Err(limit_or_failure(plan, &format!("{printed}{errors}"))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(e: CoreError) -> String {
        match e {
            CoreError::Ai { code, .. } => code,
            _ => String::new(),
        }
    }

    #[test]
    fn an_expired_sign_in_asks_to_sign_in_again() {
        let said = "Failed to authenticate: OAuth session expired and could not be refreshed";
        assert_eq!(code(limit_or_failure(Plan::Claude, said)), "ai_key");
        assert_eq!(
            code(limit_or_failure(
                Plan::Claude,
                "Not logged in · Please run /login"
            )),
            "ai_key"
        );
        assert_eq!(
            code(limit_or_failure(Plan::Claude, "Claude usage limit reached")),
            "ai_limit"
        );
        assert_eq!(
            code(limit_or_failure(Plan::Claude, "something else")),
            "ai_failed"
        );
    }
}
