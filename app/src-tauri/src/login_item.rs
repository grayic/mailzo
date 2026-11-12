//! Starting the app when the person logs in to their Mac.

use std::path::Path;

/// Passed at login, so the app starts without its window.
pub const BACKGROUND: &str = "--background";

/// The app this program is inside, if it's inside one.
pub fn app_around(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
}

/// Adds or removes the file the Mac reads at login.
// ponytail: the Mac's newer login item service knows when it's been turned off in System
// Settings, but needs macOS 13 and can't ask for a hidden start. This file can't tell, so
// the switch in Settings can say on when the Mac has turned it off. Worth moving once 13 is
// the oldest Mac supported.
pub fn set(folder: &Path, label: &str, exe: &Path, on: bool) -> std::io::Result<()> {
    let file = folder.join(format!("{label}.plist"));
    if !on {
        return match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    // A loose program, as in development, has nothing for the Mac to open.
    let Some(app) = app_around(exe) else {
        return Ok(());
    };
    let app = app
        .display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;");
    std::fs::create_dir_all(folder)?;
    // `open` uses the copy that's already running, if the Mac reopened it first, rather
    // than starting a second one on the same mail.
    std::fs::write(
        file,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>AssociatedBundleIdentifiers</key><array><string>{label}</string></array>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/open</string>
    <string>-g</string>
    <string>-a</string>
    <string>{app}</string>
    <string>--args</string>
    <string>{BACKGROUND}</string>
  </array>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const LABEL: &str = "com.example.mail";

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("login-item-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Reads the file the way the Mac does, so a badly formed one fails here too.
    fn arguments(file: &Path) -> Vec<String> {
        let out = std::process::Command::new("/usr/bin/plutil")
            .args(["-extract", "ProgramArguments", "json", "-o", "-"])
            .arg(file)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    #[test]
    fn turning_it_on_opens_the_app_in_the_background_at_login() {
        let dir = folder("on");
        let exe = Path::new("/Apps & Tools/Mail.app/Contents/MacOS/mail");
        set(&dir, LABEL, exe, true).unwrap();
        assert_eq!(
            arguments(&dir.join("com.example.mail.plist")),
            [
                "/usr/bin/open",
                "-g",
                "-a",
                "/Apps & Tools/Mail.app",
                "--args",
                "--background",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn turning_it_off_removes_it_even_if_it_was_never_on() {
        let dir = folder("off");
        let exe = Path::new("/Applications/Mail.app/Contents/MacOS/mail");
        set(&dir, LABEL, exe, false).unwrap();
        set(&dir, LABEL, exe, true).unwrap();
        assert!(dir.join("com.example.mail.plist").exists());
        set(&dir, LABEL, exe, false).unwrap();
        assert!(!dir.join("com.example.mail.plist").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_program_outside_an_app_is_never_added() {
        let dir = folder("loose");
        set(&dir, LABEL, Path::new("/tmp/target/debug/mail"), true).unwrap();
        assert!(!dir.join("com.example.mail.plist").exists());
    }
}
