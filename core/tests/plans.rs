//! Using an AI plan through the company's own program. These use stand-in programs, which write
//! down how they were started, so nothing reaches a real account.

use mailcore::connections::Places;
use mailcore::{engine::Engine, secrets::MemoryStore, store::Store};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

struct Rig {
    dir: tempfile::TempDir,
    engine: Engine,
}

const CLAUDE: &str = r#"#!/bin/sh
here="$(dirname "$0")"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  if [ -f "$here/signed-in" ]; then echo '{"loggedIn":true,"authMethod":"claude.ai"}'; else echo '{"loggedIn":false,"authMethod":"none"}'; fi
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "login" ]; then touch "$here/signed-in"; exit 0; fi
printf '%s\n' "$@" > "$here/args"
pwd > "$here/where"
env > "$here/env"
cat > "$here/input"
if [ -f "$here/out-of-limit" ]; then echo '{"is_error":true,"result":"Claude usage limit reached"}'; exit 1; fi
if [ ! -f "$here/signed-in" ]; then echo '{"is_error":true,"result":"Not logged in · Please run /login"}'; exit 1; fi
if [ -f "$here/answer" ]; then
  python3 -c "import json,sys; print(json.dumps({'is_error':False,'result':open(sys.argv[1]).read()}))" "$here/answer"
else
  echo '{"is_error":false,"result":"{\"ok\":true}"}'
fi
"#;

const CODEX: &str = r#"#!/bin/sh
here="$(dirname "$0")"
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  if [ -f "$here/codex-in" ]; then echo 'Logged in using ChatGPT'; exit 0; else echo 'Not logged in'; exit 1; fi
fi
if [ "$1" = "login" ]; then touch "$here/codex-in"; exit 0; fi
printf '%s\n' "$@" > "$here/codex-args"
cat > "$here/codex-input"
out=""; prev=""
for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
printf '```json\n{"ok":true}\n```' > "$out"
"#;

fn write_program(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn rig(programs: &[(&str, &str)]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in programs {
        write_program(dir.path(), name, text);
    }
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_folders(dir.path().join("data"), dir.path().join("downloads"))
    .with_ai_app_places(Places {
        home: dir.path().join("home"),
        applications: dir.path().join("Applications"),
    })
    .with_plan_programs(dir.path().to_path_buf());
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    Rig { dir, engine }
}

impl Rig {
    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap()
    }

    async fn plans(&self) -> Value {
        self.engine.call("ai_plans", json!({})).await.unwrap()
    }

    async fn sign_in_claude(&self) {
        self.engine
            .call("ai_plan_sign_in", json!({ "id": "claude_code" }))
            .await
            .unwrap();
        for _ in 0..50 {
            if self.dir.path().join("signed-in").exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("it never signed in");
    }
}

#[tokio::test]
async fn it_shows_what_is_installed_and_signed_in() {
    let r = rig(&[("claude", CLAUDE)]);
    let plans = r.plans().await;
    assert_eq!(plans[0]["id"], "claude_code");
    assert_eq!(plans[0]["found"], true);
    assert_eq!(plans[0]["signed_in"], false);
    assert_eq!(plans[1]["found"], false);

    r.sign_in_claude().await;
    assert_eq!(r.plans().await[0]["signed_in"], true);
    assert!(r
        .engine
        .call("ai_plan_sign_in", json!({ "id": "codex" }))
        .await
        .is_err());
}

#[tokio::test]
async fn an_email_can_never_reach_a_tool_or_the_command_line() {
    let r = rig(&[("claude", CLAUDE)]);
    r.sign_in_claude().await;
    let status = r
        .engine
        .call("ai_plan_use", json!({ "id": "claude_code" }))
        .await
        .unwrap();
    assert_eq!(status["via"], "claude_code");
    assert_eq!(status["has_key"], true);

    let args: Vec<String> = r.file("args").lines().map(String::from).collect();
    let after = |flag: &str| {
        let at = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("no {flag}"));
        args[at + 1].clone()
    };
    assert_eq!(after("--tools"), "", "every tool is off");
    assert_eq!(after("--setting-sources"), "");
    assert_eq!(after("--mcp-config"), r#"{"mcpServers":{}}"#);
    assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    assert!(args.iter().any(|a| a == "--disable-slash-commands"));
    assert!(args.iter().any(|a| a == "--no-session-persistence"));
    assert!(!args
        .iter()
        .any(|a| a.contains("dangerously") || a.contains("bypass")));
    assert_eq!(after("--model"), "haiku");
    assert!(
        r.file("where").trim().ends_with("data/ai-empty"),
        "starts somewhere empty"
    );
    assert!(!r.file("input").is_empty());

    // Keys and the like in the app's surroundings don't get passed on.
    let env = r.file("env");
    assert!(env.contains("CLAUDE_CODE_DISABLE_CLAUDE_MDS=1"));
    assert!(!env.contains("ANTHROPIC_API_KEY"));
}

#[tokio::test]
async fn codex_runs_with_no_shell_and_read_only() {
    let r = rig(&[("codex", CODEX)]);
    r.engine
        .call("ai_plan_sign_in", json!({ "id": "codex" }))
        .await
        .unwrap();
    for _ in 0..50 {
        if r.dir.path().join("codex-in").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let status = r
        .engine
        .call("ai_plan_use", json!({ "id": "codex" }))
        .await
        .unwrap();
    assert_eq!(status["via"], "codex");
    let args = r.file("codex-args");
    for needed in [
        "read-only",
        "shell_tool",
        "--ignore-user-config",
        "--ignore-rules",
        "--ephemeral",
        "approval_policy=\"never\"",
    ] {
        assert!(args.contains(needed), "{needed} is missing");
    }
    assert!(!args.contains("dangerously") && !args.contains("--approve-for-me"));
    assert!(
        !r.dir.path().join("data").read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("ai-answer")),
        "its answer file is cleaned up"
    );
}

#[tokio::test]
async fn a_plan_works_only_when_it_answers() {
    let r = rig(&[("claude", CLAUDE)]);
    let refused = r
        .engine
        .call("ai_plan_use", json!({ "id": "claude_code" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("Sign in"), "{refused}");
    let status = r.engine.call("ai_status", json!({})).await.unwrap();
    assert_eq!(status["via"], "key");
    assert_eq!(status["has_key"], false);

    r.sign_in_claude().await;
    std::fs::write(r.dir.path().join("out-of-limit"), "").unwrap();
    let limit = r
        .engine
        .call("ai_plan_use", json!({ "id": "claude_code" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(limit.contains("limit"), "{limit}");
    assert!(r
        .engine
        .call("ai_plan_use", json!({ "id": "nonsense" }))
        .await
        .is_err());
}

#[tokio::test]
async fn leaving_a_plan_goes_back_to_no_ai() {
    let r = rig(&[("claude", CLAUDE)]);
    r.sign_in_claude().await;
    r.engine
        .call("ai_plan_use", json!({ "id": "claude_code" }))
        .await
        .unwrap();
    let status = r.engine.call("ai_remove_key", json!({})).await.unwrap();
    assert_eq!(status["via"], "key");
    assert_eq!(status["has_key"], false);
}
