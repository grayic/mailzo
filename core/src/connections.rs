//! One-click connection to the person's other AI apps. Each of those apps keeps a settings
//! file listing what it may talk to, so connecting means adding one entry that starts
//! `inbox-app --mcp`. Nothing here runs until the person clicks: everything else in the
//! file stays as it was, a copy is kept first, and a file we can't read is left alone.

use crate::error::{CoreError, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const ENTRY: &str = "mailzo";
/// What connections were called under the app's earlier names, Dakly and Inbox. They still work,
/// and connecting again or disconnecting clears them.
const OLD_ENTRIES: [&str; 2] = ["dakly", "inbox"];
const ARG: &str = "--mcp";
const CODEX_HEADER: &str = "[mcp_servers.mailzo]";

#[derive(Clone, Copy, PartialEq)]
enum Format {
    /// `{ "mcpServers": { "mailzo": { "command": .., "args": [..] } } }`
    Plain,
    /// The same, with `"type": "stdio"` on the entry.
    Typed,
    /// VS Code calls the list `servers`.
    VsCode,
    /// Codex keeps its settings in TOML.
    Codex,
}

struct AiApp {
    id: &'static str,
    name: &'static str,
    /// Inside the home folder.
    file: &'static str,
    format: Format,
    /// What shows the app is on this Mac, besides its settings file.
    bundle: &'static str,
    folder: &'static str,
    /// What the person has to do for the connection to start working.
    after: &'static str,
}

const APPS: &[AiApp] = &[
    AiApp {
        id: "claude",
        name: "Claude",
        file: "Library/Application Support/Claude/claude_desktop_config.json",
        format: Format::Plain,
        bundle: "Claude.app",
        folder: "Library/Application Support/Claude",
        after: "Quit Claude and open it again to finish.",
    },
    AiApp {
        id: "claude-code",
        name: "Claude Code",
        file: ".claude.json",
        format: Format::Typed,
        bundle: "",
        folder: ".claude",
        after: "Start a new Claude Code chat to use it.",
    },
    AiApp {
        id: "codex",
        name: "Codex",
        file: ".codex/config.toml",
        format: Format::Codex,
        bundle: "Codex.app",
        folder: ".codex",
        after: "Start a new Codex chat to use it.",
    },
    AiApp {
        id: "cursor",
        name: "Cursor",
        file: ".cursor/mcp.json",
        format: Format::Plain,
        bundle: "Cursor.app",
        folder: ".cursor",
        after: "Restart Cursor to finish.",
    },
    AiApp {
        id: "vscode",
        name: "VS Code",
        file: "Library/Application Support/Code/User/mcp.json",
        format: Format::VsCode,
        bundle: "Visual Studio Code.app",
        folder: "Library/Application Support/Code/User",
        after: "Restart VS Code, then choose Trust when it asks.",
    },
    AiApp {
        id: "windsurf",
        name: "Windsurf",
        file: ".codeium/windsurf/mcp_config.json",
        format: Format::Plain,
        bundle: "Windsurf.app",
        folder: ".codeium/windsurf",
        after: "Restart Windsurf to finish.",
    },
];

/// Where to look for the apps. A test points this at an empty folder, so it can never touch
/// the real ones.
#[derive(Clone, Debug)]
pub struct Places {
    pub home: PathBuf,
    pub applications: PathBuf,
}

impl Places {
    pub fn of_this_mac() -> Self {
        Places {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            applications: PathBuf::from("/Applications"),
        }
    }

    fn has(&self, app: &AiApp) -> bool {
        if self.home.as_os_str().is_empty() {
            return false;
        }
        self.home.join(app.file).exists()
            || self.home.join(app.folder).is_dir()
            || (!app.bundle.is_empty()
                && (self.applications.join(app.bundle).exists()
                    || self.home.join("Applications").join(app.bundle).exists()))
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    Connected,
    NotConnected,
    /// Ours, but it points at where this app used to be.
    Outdated,
    /// Something else already uses the name, and it isn't ours to change.
    Taken,
    /// The file has something we can't safely read, such as comments.
    Unreadable,
}

#[derive(Debug, Serialize)]
pub struct AppLink {
    pub id: &'static str,
    pub name: &'static str,
    pub link: Link,
    pub after: &'static str,
}

/// The AI apps on this Mac, and whether each is connected.
pub fn list(places: &Places, command: &str) -> Vec<AppLink> {
    APPS.iter()
        .filter(|app| places.has(app))
        .map(|app| AppLink {
            id: app.id,
            name: app.name,
            link: link_of(app, &places.home.join(app.file), command),
            after: app.after,
        })
        .collect()
}

pub fn connect(places: &Places, id: &str, command: &str) -> Result<()> {
    let app = find(id)?;
    let path = places.home.join(app.file);
    match link_of(app, &path, command) {
        Link::Connected => return Ok(()),
        Link::Taken => {
            return Err(CoreError::Invalid(format!(
                "{} already has a different connection called \"{ENTRY}\", so we left it alone.",
                app.name
            )))
        }
        Link::Unreadable => return Err(cant_read(app)),
        Link::NotConnected | Link::Outdated => {}
    }
    let text = if app.format == Format::Codex {
        codex_with_entry(
            &read_text(&path).unwrap_or_default().unwrap_or_default(),
            command,
        )
    } else {
        let mut file = read_json(&path).map_err(|_| cant_read(app))?;
        let list = list_in(&mut file, app.format).ok_or_else(|| cant_read(app))?;
        list.insert(ENTRY.into(), entry(app.format, command));
        drop_old_entry(list);
        pretty(&file)?
    };
    write_safely(&path, &text)
}

/// Takes out our entry and nothing else. Someone else's entry of the same name stays.
pub fn disconnect(places: &Places, id: &str, command: &str) -> Result<()> {
    let app = find(id)?;
    let path = places.home.join(app.file);
    match link_of(app, &path, command) {
        Link::Connected | Link::Outdated => {}
        Link::NotConnected | Link::Taken => return Ok(()),
        Link::Unreadable => return Err(cant_read(app)),
    }
    let text = if app.format == Format::Codex {
        codex_without_entry(&read_text(&path)?.unwrap_or_default())
    } else {
        let mut file = read_json(&path).map_err(|_| cant_read(app))?;
        let list = list_in(&mut file, app.format).ok_or_else(|| cant_read(app))?;
        list.remove(ENTRY);
        drop_old_entry(list);
        pretty(&file)?
    };
    write_safely(&path, &text)
}

/// For any other app: what to paste into its settings.
pub fn setup_text(command: &str) -> String {
    let mut list = Map::new();
    list.insert(ENTRY.into(), entry(Format::Plain, command));
    pretty(&json!({ "mcpServers": list })).unwrap_or_default()
}

fn find(id: &str) -> Result<&'static AiApp> {
    APPS.iter()
        .find(|app| app.id == id)
        .ok_or_else(|| CoreError::Invalid("Unknown AI app.".into()))
}

fn cant_read(app: &AiApp) -> CoreError {
    CoreError::Invalid(format!(
        "We couldn't safely read {}'s settings, so we left them alone. Use Copy setup instead.",
        app.name
    ))
}

fn entry(format: Format, command: &str) -> Value {
    if format == Format::Plain {
        json!({ "command": command, "args": [ARG] })
    } else {
        json!({ "type": "stdio", "command": command, "args": [ARG] })
    }
}

fn link_of(app: &AiApp, path: &Path, command: &str) -> Link {
    if app.format == Format::Codex {
        return match read_text(path) {
            Ok(text) => codex_link(&text.unwrap_or_default(), command),
            Err(_) => Link::Unreadable,
        };
    }
    let Ok(mut file) = read_json(path) else {
        return Link::Unreadable;
    };
    let Some(list) = list_in(&mut file, app.format) else {
        return Link::Unreadable;
    };
    let Some(found) = list.get(ENTRY) else {
        return if is_old_entry(list) {
            Link::Outdated
        } else {
            Link::NotConnected
        };
    };
    if found.get("args") != Some(&json!([ARG])) {
        Link::Taken
    } else if found.get("command").and_then(Value::as_str) == Some(command) {
        Link::Connected
    } else {
        Link::Outdated
    }
}

fn is_ours(list: &Map<String, Value>, name: &str) -> bool {
    list.get(name)
        .is_some_and(|e| e.get("args") == Some(&json!([ARG])))
}

fn is_old_entry(list: &Map<String, Value>) -> bool {
    OLD_ENTRIES.iter().any(|name| is_ours(list, name))
}

fn drop_old_entry(list: &mut Map<String, Value>) {
    for name in OLD_ENTRIES {
        if is_ours(list, name) {
            list.remove(name);
        }
    }
}

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::Invalid(e.to_string())),
    }
}

/// A missing or empty file counts as an empty list. Anything else has to parse.
fn read_json(path: &Path) -> Result<Value> {
    let text = read_text(path)?.unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| CoreError::Invalid(e.to_string()))
}

fn list_in(file: &mut Value, format: Format) -> Option<&mut Map<String, Value>> {
    let key = if format == Format::VsCode {
        "servers"
    } else {
        "mcpServers"
    };
    file.as_object_mut()?
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
}

fn pretty(value: &Value) -> Result<String> {
    let mut text =
        serde_json::to_string_pretty(value).map_err(|e| CoreError::Invalid(e.to_string()))?;
    text.push('\n');
    Ok(text)
}

/// Swaps the file in one step, so a crash can't leave half of it. Follows a link to the real
/// file and keeps its permissions, since some of these files hold private settings.
fn write_safely(path: &Path, text: &str) -> Result<()> {
    let failed =
        |e: std::io::Error| CoreError::Invalid(format!("Couldn't save the connection: {e}"));
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = real.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let name = real
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let beside = |ending: &str| real.with_file_name(format!("{name}.{ending}"));
    let old = std::fs::metadata(&real).ok();
    if old.is_some() {
        std::fs::copy(&real, beside("before-dakly")).map_err(failed)?;
    }
    let temp = beside("dakly-saving");
    std::fs::write(&temp, text).map_err(failed)?;
    if let Some(old) = old {
        std::fs::set_permissions(&temp, old.permissions()).map_err(failed)?;
    }
    std::fs::rename(&temp, &real).map_err(failed)
}

/// The section a TOML line opens, if it opens one.
fn toml_section(line: &str) -> Option<&str> {
    let line = line.split('#').next().unwrap_or("").trim();
    (line.starts_with('[') && line.ends_with(']')).then_some(line)
}

/// Where a section of ours starts and stops, counting lines.
fn codex_block(text: &str, entry: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let header = format!("[mcp_servers.{entry}]");
    let nested = format!("[mcp_servers.{entry}.");
    let start = lines
        .iter()
        .position(|l| toml_section(l) == Some(header.as_str()))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| toml_section(l).is_some_and(|s| !s.starts_with(&nested)))
        .map_or(lines.len(), |i| start + 1 + i);
    Some((start, end))
}

fn codex_block_is_ours(text: &str, start: usize, end: usize) -> bool {
    let squeezed = |line: &str| line.split_whitespace().collect::<String>();
    text.lines()
        .skip(start)
        .take(end - start)
        .any(|l| squeezed(l) == format!("args=[\"{ARG}\"]"))
}

fn toml_string(text: &str) -> String {
    Value::from(text).to_string()
}

fn codex_link(text: &str, command: &str) -> Link {
    let Some((start, end)) = codex_block(text, ENTRY) else {
        // The same name written another way would clash with a section we add.
        let mut section = "";
        for line in text.lines() {
            if let Some(s) = toml_section(line) {
                section = s;
            }
            let line = line.trim_start();
            if line.starts_with("mcp_servers.mailzo")
                || (section == "[mcp_servers]" && line.starts_with("mailzo"))
            {
                return Link::Taken;
            }
        }
        let old = OLD_ENTRIES.iter().any(|name| {
            codex_block(text, name)
                .is_some_and(|(start, end)| codex_block_is_ours(text, start, end))
        });
        return if old {
            Link::Outdated
        } else {
            Link::NotConnected
        };
    };
    let block: Vec<&str> = text.lines().skip(start).take(end - start).collect();
    if !codex_block_is_ours(text, start, end) {
        Link::Taken
    } else if block
        .iter()
        .any(|l| l.trim() == format!("command = {}", toml_string(command)))
    {
        Link::Connected
    } else {
        Link::Outdated
    }
}

fn codex_with_entry(text: &str, command: &str) -> String {
    let mut out = codex_without_entry(text);
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "{CODEX_HEADER}\ncommand = {}\nargs = [\"{ARG}\"]\n",
        toml_string(command)
    ));
    out
}

fn codex_without_entry(text: &str) -> String {
    let mut text = codex_without(text, ENTRY);
    for name in OLD_ENTRIES {
        if let Some((start, end)) = codex_block(&text, name) {
            if codex_block_is_ours(&text, start, end) {
                text = codex_without(&text, name);
            }
        }
    }
    text
}

fn codex_without(text: &str, entry: &str) -> String {
    let Some((start, end)) = codex_block(text, entry) else {
        return text.to_string();
    };
    let lines: Vec<&str> = text.lines().collect();
    // At the end of the file, also the blank line we put in front of it.
    let before = if end == lines.len() && start > 0 && lines[start - 1].trim().is_empty() {
        start - 1
    } else {
        start
    };
    let mut kept: Vec<&str> = lines[..before].to_vec();
    kept.extend(&lines[end..]);
    if kept.is_empty() {
        String::new()
    } else {
        kept.join("\n") + "\n"
    }
}
