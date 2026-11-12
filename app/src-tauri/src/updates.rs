//! New versions of the app, fetched and put in place from inside it, so nobody downloads it again.
//!
//! The app asks our update address for the newest version on its channel, downloads it in the
//! background and keeps it ready. It is only put in place when the person presses Restart.
//! Every update is signed with our private update key and checked against the public key in
//! `tauri.conf.json`, so a file from anyone else is refused.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Where the update lists live. A build can point somewhere else, such as a test server.
const FEED: &str = match option_env!("DAKLY_UPDATE_FEED") {
    Some(feed) => feed,
    None => "https://updates.getmailzo.com",
};

/// The downloaded update waiting for a restart, and the channel it came from.
#[derive(Default)]
pub struct Ready(Mutex<Option<Waiting>>);

struct Waiting {
    update: Update,
    bytes: Vec<u8>,
    channel: &'static str,
}

#[derive(Serialize)]
pub struct UpdateInfo {
    version: String,
    notes: Option<String>,
}

fn info(update: &Update) -> UpdateInfo {
    UpdateInfo {
        version: update.version.clone(),
        notes: update.body.clone().filter(|n| !n.trim().is_empty()),
    }
}

/// This copy's version, for Settings.
#[tauri::command]
pub fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Looks for a newer version and downloads it. Returns it once it is ready to put in place.
#[tauri::command]
pub async fn update_check(
    app: tauri::AppHandle,
    channel: String,
) -> Result<Option<UpdateInfo>, String> {
    // A copy being worked on is never replaced by a published one: a debug build, or one built
    // on this Mac to use here (only those reach the Keychain through the helper).
    let working_copy = (cfg!(debug_assertions) && option_env!("DAKLY_UPDATE_FEED").is_none())
        || option_env!("INBOX_SECRETS_HELPER").is_some();
    if working_copy {
        // Said plainly, so it isn't mistaken for having the newest version.
        return Err(WORKING_COPY.to_string());
    }
    let channel = if channel == "beta" { "beta" } else { "stable" };
    {
        let state = app.state::<Ready>();
        let mut ready = state.0.lock().unwrap();
        match ready.as_ref() {
            Some(waiting) if waiting.channel == channel => return Ok(Some(info(&waiting.update))),
            // Someone switched channel, so the one downloaded for the other may not be wanted.
            Some(_) => *ready = None,
            None => {}
        }
    }
    let url = format!("{FEED}/{channel}.json")
        .parse()
        .map_err(|_| "The update address isn't valid.".to_string())?;
    // Normally only a newer version is offered. A different one is too, so a version that went
    // out by mistake can be taken back from people who already have it, but only when the list
    // says it's a rollback (checked below, as the comparator can't see the list's own fields).
    let newer = Arc::new(AtomicBool::new(false));
    let seen = newer.clone();
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .map(|b| {
            b.version_comparator(move |current, release| {
                seen.store(release.version > current, Ordering::Relaxed);
                release.version != current
            })
        })
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|_| {
        "Couldn't check for updates. Check your internet connection and try again.".to_string()
    })?
    else {
        return Ok(None);
    };
    let rollback = update.raw_json.get("rollback").and_then(|r| r.as_bool()) == Some(true);
    if !newer.load(Ordering::Relaxed) && !rollback {
        return Ok(None);
    }
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|_| "The update didn't download. Mailzo tries again later.".to_string())?;
    let found = info(&update);
    *app.state::<Ready>().0.lock().unwrap() = Some(Waiting {
        update,
        bytes,
        channel,
    });
    Ok(Some(found))
}

/// Said when the update ready came from the channel no longer chosen. The UI matches it.
const WORKING_COPY: &str = "This copy of Mailzo was built on this Mac, so it doesn't update itself. It's updated by building it again.";
const OTHER_CHANNEL: &str = "That update was for the other channel, so it wasn't put in place. Check for updates to get this one's.";

/// Puts the downloaded version in place and opens it.
///
/// The unpacking runs away from the main thread, so the window doesn't freeze meanwhile, and the
/// restart itself goes back to the main thread. If unpacking fails, the update stays ready, so
/// Restart can be pressed again.
#[tauri::command]
pub async fn update_install(app: tauri::AppHandle, channel: String) -> Result<(), String> {
    let channel = if channel == "beta" { "beta" } else { "stable" };
    let Some(waiting) = app.state::<Ready>().0.lock().unwrap().take() else {
        return Err("There's no update ready yet.".into());
    };
    // Downloaded before the person switched channel: it goes, rather than move them to the wrong one.
    if waiting.channel != channel {
        return Err(OTHER_CHANNEL.into());
    }
    let (result, waiting) = tauri::async_runtime::spawn_blocking(move || {
        let result = waiting.update.install(&waiting.bytes);
        (result, waiting)
    })
    .await
    .map_err(|_| {
        "The update couldn't be put in place. Your current version still works.".to_string()
    })?;
    if result.is_err() {
        let state = app.state::<Ready>();
        let mut ready = state.0.lock().unwrap();
        // A newer download that arrived meanwhile wins.
        if ready.is_none() {
            *ready = Some(waiting);
        }
        return Err(
            "The update couldn't be put in place. Your current version still works.".into(),
        );
    }
    let restarting = app.clone();
    if app
        .run_on_main_thread(move || restart_cleanly(&restarting))
        .is_err()
    {
        restart_cleanly(&app);
    }
    Ok(())
}

/// Opens the new version, then leaves at once.
///
/// The usual restart leaves through the normal way out, which runs the system's clean-up while
/// mail is still being read and saved on other threads. That clean-up takes things away from
/// under the encrypted database, and the old copy crashed on its way out ("Mailzo quit
/// unexpectedly") after every update. Leaving at once skips it: the database keeps everything
/// it finished saving, and anything half-done is simply not there, as after any stop.
fn restart_cleanly(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // …/Mailzo.app/Contents/MacOS/inbox-app → …/Mailzo.app
        let bundle = std::env::current_exe().ok().and_then(|exe| {
            let app_dir = exe.parent()?.parent()?.parent()?.to_path_buf();
            (app_dir.extension()? == "app").then_some(app_dir)
        });
        if let Some(bundle) = bundle {
            let opened = std::process::Command::new("/usr/bin/open")
                .arg("-n")
                .arg(&bundle)
                .stdin(std::process::Stdio::null())
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false);
            if opened {
                app.cleanup_before_exit();
                // SAFETY: ends this process straight away; nothing after it runs.
                unsafe { libc::_exit(0) };
            }
        }
    }
    // Not from an app bundle (or opening it failed): the framework's own way, from any thread.
    // Not app.restart(), which off the main thread goes through the quit handler below and
    // would leave without opening anything.
    app.cleanup_before_exit();
    tauri::process::restart(&app.env());
}

/// Throws away an update downloaded for a channel the person has just switched away from.
#[tauri::command]
pub fn update_forget(app: tauri::AppHandle) {
    *app.state::<Ready>().0.lock().unwrap() = None;
}

/// Self-test only: writes this copy's version, then updates without anyone pressing Restart.
pub fn selftest(
    app: tauri::AppHandle,
    channel: String,
    log: std::path::PathBuf,
    kept: Option<String>,
) {
    use std::io::Write;
    let note = move |line: String| {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            let _ = writeln!(f, "{line}");
        }
    };
    note(format!("running {}", app.package_info().version));
    if let Some(kept) = kept {
        note(kept);
    }
    tauri::async_runtime::spawn(async move {
        match update_check(app.clone(), channel.clone()).await {
            Ok(Some(found)) => {
                note(format!("downloaded {}", found.version));
                if let Err(e) = update_install(app, channel).await {
                    note(format!("install failed: {e}"));
                }
            }
            Ok(None) => note("no update".into()),
            Err(e) => note(format!("check failed: {e}")),
        }
    });
}
