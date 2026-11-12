//! Connecting other AI apps edits their own settings files, so these prove it adds one entry
//! and leaves everything else alone. Every test uses an empty pretend home folder.

use mailcore::connections::{connect, disconnect, list, setup_text, Link, Places};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const COMMAND: &str = "/Applications/My Mail.app/Contents/MacOS/inbox-app";
const CLAUDE: &str = "Library/Application Support/Claude/claude_desktop_config.json";
const CODE: &str = ".claude.json";
const CODEX: &str = ".codex/config.toml";
const CURSOR: &str = ".cursor/mcp.json";
const VSCODE: &str = "Library/Application Support/Code/User/mcp.json";

fn pretend() -> (tempfile::TempDir, Places) {
    let dir = tempfile::tempdir().unwrap();
    let places = Places {
        home: dir.path().join("home"),
        applications: dir.path().join("Applications"),
    };
    fs::create_dir_all(&places.home).unwrap();
    fs::create_dir_all(&places.applications).unwrap();
    (dir, places)
}

fn put(places: &Places, file: &str, text: &str) {
    let path = places.home.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn text_of(places: &Places, file: &str) -> String {
    fs::read_to_string(places.home.join(file)).unwrap()
}

fn json_of(places: &Places, file: &str) -> Value {
    serde_json::from_str(&text_of(places, file)).unwrap()
}

fn link_of(places: &Places, id: &str) -> Link {
    list(places, COMMAND)
        .into_iter()
        .find(|a| a.id == id)
        .unwrap()
        .link
}

#[test]
fn only_apps_on_this_mac_are_listed() {
    let (_dir, places) = pretend();
    assert!(list(&places, COMMAND).is_empty());

    fs::create_dir_all(places.applications.join("Cursor.app")).unwrap();
    fs::create_dir_all(places.home.join(".claude")).unwrap();
    put(&places, CLAUDE, "{}");
    let ids: Vec<&str> = list(&places, COMMAND).iter().map(|a| a.id).collect();
    assert_eq!(ids, ["claude", "claude-code", "cursor"]);
    assert!(list(&places, COMMAND)
        .iter()
        .all(|a| a.link == Link::NotConnected));
}

#[test]
fn connecting_keeps_everything_else_and_a_copy() {
    let (_dir, places) = pretend();
    let before = json!({
        "preferences": { "theme": "dark", "sizes": [1, 2] },
        "mcpServers": { "other": { "command": "x", "args": ["y"], "env": { "KEY": "kept" } } },
    });
    put(&places, CLAUDE, &before.to_string());

    connect(&places, "claude", COMMAND).unwrap();

    let after = json_of(&places, CLAUDE);
    assert_eq!(after["preferences"], before["preferences"]);
    assert_eq!(after["mcpServers"]["other"], before["mcpServers"]["other"]);
    assert_eq!(
        after["mcpServers"]["mailzo"],
        json!({ "command": COMMAND, "args": ["--mcp"] })
    );
    assert_eq!(link_of(&places, "claude"), Link::Connected);
    let copy = text_of(&places, &format!("{CLAUDE}.before-dakly"));
    assert_eq!(serde_json::from_str::<Value>(&copy).unwrap(), before);

    disconnect(&places, "claude", COMMAND).unwrap();
    assert_eq!(json_of(&places, CLAUDE), before);
    assert_eq!(link_of(&places, "claude"), Link::NotConnected);
}

#[test]
fn each_app_gets_the_shape_it_reads() {
    let (_dir, places) = pretend();
    for folder in [
        ".cursor",
        ".claude",
        "Library/Application Support/Code/User",
    ] {
        fs::create_dir_all(places.home.join(folder)).unwrap();
    }
    connect(&places, "cursor", COMMAND).unwrap();
    connect(&places, "claude-code", COMMAND).unwrap();
    connect(&places, "vscode", COMMAND).unwrap();
    assert_eq!(
        json_of(&places, CURSOR)["mcpServers"]["mailzo"],
        json!({ "command": COMMAND, "args": ["--mcp"] })
    );
    assert_eq!(
        json_of(&places, CODE)["mcpServers"]["mailzo"],
        json!({ "type": "stdio", "command": COMMAND, "args": ["--mcp"] })
    );
    assert_eq!(
        json_of(&places, VSCODE)["servers"]["mailzo"],
        json!({ "type": "stdio", "command": COMMAND, "args": ["--mcp"] })
    );
}

#[test]
fn a_file_it_cant_read_is_never_touched() {
    let (_dir, places) = pretend();
    let notes = "{\n  // my notes\n  \"servers\": {}\n}\n";
    for (file, text) in [
        (VSCODE, notes),
        (CURSOR, "[1, 2]"),
        (CLAUDE, "{\"mcpServers\": 3}"),
    ] {
        put(&places, file, text);
    }
    for (id, file, text) in [
        ("vscode", VSCODE, notes),
        ("cursor", CURSOR, "[1, 2]"),
        ("claude", CLAUDE, "{\"mcpServers\": 3}"),
    ] {
        assert_eq!(link_of(&places, id), Link::Unreadable);
        assert!(connect(&places, id, COMMAND).is_err());
        assert!(disconnect(&places, id, COMMAND).is_err());
        assert_eq!(text_of(&places, file), text);
        assert!(!places.home.join(format!("{file}.before-dakly")).exists());
    }
}

#[test]
fn someone_elses_entry_of_the_same_name_stays() {
    let (_dir, places) = pretend();
    let theirs = json!({ "mcpServers": { "mailzo": { "command": "theirs", "args": ["serve"] } } });
    put(&places, CURSOR, &theirs.to_string());
    assert_eq!(link_of(&places, "cursor"), Link::Taken);
    assert!(connect(&places, "cursor", COMMAND).is_err());
    disconnect(&places, "cursor", COMMAND).unwrap();
    assert_eq!(json_of(&places, CURSOR), theirs);
}

#[test]
fn a_connection_to_an_old_copy_can_be_fixed() {
    let (_dir, places) = pretend();
    fs::create_dir_all(places.home.join(".cursor")).unwrap();
    connect(&places, "cursor", "/old/place/inbox-app").unwrap();
    assert_eq!(link_of(&places, "cursor"), Link::Outdated);
    connect(&places, "cursor", COMMAND).unwrap();
    assert_eq!(link_of(&places, "cursor"), Link::Connected);
}

#[test]
fn codex_keeps_its_comments_and_order() {
    let (_dir, places) = pretend();
    let before = "# my comment\nmodel = \"x\"\n\n[mcp_servers.other]\ncommand = \"y\"\n\n[plugins.\"a@b\"]\nenabled = true\n";
    put(&places, CODEX, before);

    connect(&places, "codex", COMMAND).unwrap();
    let after = text_of(&places, CODEX);
    assert!(after.starts_with(before));
    assert!(after.contains(&format!(
        "[mcp_servers.mailzo]\ncommand = \"{COMMAND}\"\nargs = [\"--mcp\"]\n"
    )));
    assert_eq!(link_of(&places, "codex"), Link::Connected);

    // Connecting twice doesn't add it twice, which would break Codex.
    connect(&places, "codex", COMMAND).unwrap();
    assert_eq!(text_of(&places, CODEX), after);
    connect(&places, "codex", "/new/place").unwrap();
    assert_eq!(
        text_of(&places, CODEX)
            .matches("[mcp_servers.mailzo]")
            .count(),
        1
    );

    disconnect(&places, "codex", "/new/place").unwrap();
    assert_eq!(text_of(&places, CODEX), before);
}

#[test]
fn codex_section_in_the_middle_comes_out_cleanly() {
    let (_dir, places) = pretend();
    put(&places, CODEX, "a = 1\n\n[mcp_servers.mailzo]\ncommand = \"z\"\nargs = [\"--mcp\"]\n\n[mcp_servers.mailzo.env]\nK = \"v\"\n\n[tail]\nb = 2\n");
    assert_eq!(link_of(&places, "codex"), Link::Outdated);
    disconnect(&places, "codex", COMMAND).unwrap();
    assert_eq!(text_of(&places, CODEX), "a = 1\n\n[tail]\nb = 2\n");
}

#[test]
fn codex_leaves_a_clashing_name_alone() {
    let (_dir, places) = pretend();
    for text in [
        "[mcp_servers.mailzo]\ncommand = \"theirs\"\nargs = [\"serve\"]\n",
        "mcp_servers.mailzo.command = \"theirs\"\n",
        "[mcp_servers]\nmailzo = { command = \"theirs\" }\n",
    ] {
        put(&places, CODEX, text);
        assert_eq!(link_of(&places, "codex"), Link::Taken);
        assert!(connect(&places, "codex", COMMAND).is_err());
        disconnect(&places, "codex", COMMAND).unwrap();
        assert_eq!(text_of(&places, CODEX), text);
    }
}

#[cfg(unix)]
#[test]
fn private_files_stay_private_and_links_stay_links() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (dir, places) = pretend();
    let real = dir.path().join("dotfiles-claude.json");
    fs::write(&real, "{\"secret\":1}").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&real, places.home.join(CODE)).unwrap();

    connect(&places, "claude-code", COMMAND).unwrap();

    assert!(fs::symlink_metadata(places.home.join(CODE))
        .unwrap()
        .file_type()
        .is_symlink());
    let saved: Value = serde_json::from_str(&fs::read_to_string(&real).unwrap()).unwrap();
    assert_eq!(saved["secret"], 1);
    assert!(saved["mcpServers"]["mailzo"].is_object());
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&real), 0o600);
    assert_eq!(
        mode(&dir.path().join("dotfiles-claude.json.before-dakly")),
        0o600
    );
    assert!(!dir
        .path()
        .join("dotfiles-claude.json.dakly-saving")
        .exists());
}

#[test]
fn an_unknown_app_is_refused_and_setup_text_is_ready_to_paste() {
    let (_dir, places) = pretend();
    assert!(connect(&places, "../../etc", COMMAND).is_err());
    let setup: Value = serde_json::from_str(&setup_text(COMMAND)).unwrap();
    assert_eq!(setup["mcpServers"]["mailzo"]["args"], json!(["--mcp"]));
}

#[test]
fn a_connection_from_the_old_name_keeps_working_until_it_is_redone() {
    let (_dir, places) = pretend();
    fs::create_dir_all(places.home.join(".cursor")).unwrap();
    let mine = json!({ "command": COMMAND, "args": ["--mcp"] });
    put(
        &places,
        CURSOR,
        &json!({ "mcpServers": { "inbox": mine, "other": { "command": "x" } } }).to_string(),
    );
    assert_eq!(link_of(&places, "cursor"), Link::Outdated);
    connect(&places, "cursor", COMMAND).unwrap();
    assert_eq!(link_of(&places, "cursor"), Link::Connected);
    let after = json_of(&places, CURSOR);
    assert!(after["mcpServers"]["inbox"].is_null());
    assert!(after["mcpServers"]["mailzo"].is_object());
    assert!(after["mcpServers"]["other"].is_object());

    // Someone else's connection that happens to use the old name stays.
    let theirs = json!({ "mcpServers": { "inbox": { "command": "theirs", "args": ["serve"] } } });
    put(&places, CURSOR, &theirs.to_string());
    assert_eq!(link_of(&places, "cursor"), Link::NotConnected);
    connect(&places, "cursor", COMMAND).unwrap();
    assert!(json_of(&places, CURSOR)["mcpServers"]["inbox"].is_object());

    // Codex, the same way.
    put(
        &places,
        CODEX,
        &format!("a = 1\n\n[mcp_servers.inbox]\ncommand = \"{COMMAND}\"\nargs = [\"--mcp\"]\n"),
    );
    assert_eq!(link_of(&places, "codex"), Link::Outdated);
    connect(&places, "codex", COMMAND).unwrap();
    let text = text_of(&places, CODEX);
    assert!(!text.contains("mcp_servers.inbox"), "{text}");
    assert!(text.contains("[mcp_servers.mailzo]"), "{text}");
}
