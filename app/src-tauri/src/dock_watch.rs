//! Notices Mailzo being taken out of the Mac's Dock, so Show in Dock doesn't go on saying it's there.
//!
//! An app's icon stays in the Dock for as long as it runs and asks for one, so all that removing it
//! can do is stop it being kept there. That's taken as not wanting it in the Dock at all.
//! Other docks keep their own lists, which this can't see.

use mailcore::Engine;
use serde_json::json;
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const LOOK_EVERY: Duration = Duration::from_secs(5);

fn take_out_of_dock(app: &AppHandle) {
    let engine = app.state::<Engine>();
    let off = json!({ "key": "show_in_dock", "value": "off" });
    // Refused when the menu bar is off too, as there'd be no way back to the app.
    if tauri::async_runtime::block_on(engine.call("set_setting", off)).is_ok() {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        let _ = app.emit("settings-changed", ());
    }
}

pub fn watch(app: AppHandle) {
    let home = std::env::var("HOME").unwrap_or_default();
    let dock = crate::selftest_env("INBOX_SELFTEST_DOCK")
        .unwrap_or(format!("{home}/Library/Preferences/com.apple.dock"));
    let identifier = app.config().identifier.clone();
    std::thread::spawn(move || {
        let (mut seen_at, mut kept) = (None, None);
        loop {
            // Only asks the Dock when its file has changed, which is rarely.
            let changed_at = std::fs::metadata(format!("{dock}.plist"))
                .and_then(|file| file.modified())
                .ok();
            if changed_at != seen_at {
                seen_at = changed_at;
                let listing = Command::new("defaults")
                    .args(["read", &dock, "persistent-apps"])
                    .output()
                    .ok()
                    .filter(|listing| listing.status.success());
                if let Some(listing) = listing {
                    let now = String::from_utf8_lossy(&listing.stdout).contains(&identifier);
                    if kept == Some(true) && !now {
                        take_out_of_dock(&app);
                    }
                    kept = Some(now);
                }
            }
            std::thread::sleep(LOOK_EVERY);
        }
    });
}
