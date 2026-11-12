use mailcore::model::{Security, SmtpSecurity};
use mailcore::signin::{MailSignIn, Servers};
use mailcore::{CoreError, Engine, HelperStore, KeychainStore, SecretStore, Store};
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[cfg(desktop)]
use tauri::webview::NewWindowResponse;
use tauri::{Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

mod apple_ai;
#[cfg(all(test, apple_ai))]
mod apple_writing_bench;
#[cfg(target_os = "macos")]
mod badge;
#[cfg(target_os = "macos")]
mod banners;
#[cfg(target_os = "macos")]
mod dock_watch;
#[cfg(target_os = "macos")]
mod idle_meanings;
#[cfg(target_os = "macos")]
mod lock;
mod login_item;
mod meaning;
mod mouse_buttons;
#[cfg(target_os = "macos")]
mod panel;
mod pasteboard;
#[cfg(target_os = "macos")]
mod right_click;
#[cfg(desktop)]
mod updates;
#[cfg(target_os = "macos")]
mod window_drag;

const KEYCHAIN_SERVICE: &str = "com.grayic.inbox";
const ACCOUNT_SERVER: &str = match option_env!("INBOX_ACCOUNT_SERVER") {
    Some(url) => url,
    None => "https://account.getmailzo.com",
};

#[cfg(target_os = "macos")]
const MENU_BAR_ICON: &str = "main";

/// How long the Mac takes to finish moving an app into or out of the Dock.
#[cfg(target_os = "macos")]
const DOCK_CHANGE_SETTLES: std::time::Duration = std::time::Duration::from_millis(150);
/// How long the Mac takes to make this the active app.
#[cfg(target_os = "macos")]
const DOCK_ACTIVATING: std::time::Duration = std::time::Duration::from_millis(500);
/// A new Dock icon loses a count sent before it exists, and nothing says when it does, so the
/// count goes this many more times, one settling time apart.
#[cfg(target_os = "macos")]
const DOCK_COUNT_RESENDS: usize = 8;

/// The last unread count asked for, for a Dock icon that appears later.
#[cfg(target_os = "macos")]
static UNREAD_COUNT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// How long a new email's sender and subject stay beside the menu bar icon.
#[cfg(target_os = "macos")]
const MENU_BAR_NEWS: std::time::Duration = std::time::Duration::from_secs(5);

/// Counts up each time a new email shows beside the menu bar icon; 0 while the count shows.
#[cfg(target_os = "macos")]
static MENU_BAR_SHOWING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Set by the local build scripts, and left out of Apple-signed builds.
const SECRETS_HELPER: Option<&str> = option_env!("INBOX_SECRETS_HELPER");

/// Set when building, and never kept in the code. Without them, browser sign-in stays off.
const MICROSOFT_CLIENT_ID: Option<&str> = option_env!("INBOX_MICROSOFT_CLIENT_ID");
const GOOGLE_CLIENT_ID: Option<&str> = option_env!("INBOX_GOOGLE_CLIENT_ID");
const GOOGLE_CLIENT_SECRET: Option<&str> = option_env!("INBOX_GOOGLE_CLIENT_SECRET");

/// The one door from the screens into the mail engine.
///
/// The database sits behind one lock, so each call waits for it on a thread of its own. Waiting on
/// the runtime's few threads would leave mail connections unanswered until the servers hang up.
#[tauri::command]
async fn call(app: tauri::AppHandle, cmd: String, args: Option<Value>) -> Result<Value, CoreError> {
    if let Some(problem) = app.try_state::<CantStart>() {
        return Err(CoreError::Invalid(problem.0.clone()));
    }
    let asked = std::time::Instant::now();
    tauri::async_runtime::spawn_blocking(move || {
        let engine = app.state::<Engine>();
        let started = std::time::Instant::now();
        let reply = tauri::async_runtime::block_on(engine.call(&cmd, args.unwrap_or(Value::Null)));
        // Slow calls are named, so a slow start or a stuck screen can be traced afterwards.
        let took = started.elapsed();
        if took >= SLOW_CALL || std::env::var_os("MAILZO_TIMING").is_some() {
            eprintln!(
                "[{:>7.0?} since launch] {cmd}: waited {:.0?}, ran {took:.0?}",
                launched().elapsed(),
                started - asked
            );
        }
        reply
    })
    .await
    .map_err(|e| CoreError::Invalid(e.to_string()))?
}

/// A call taking longer than this is noted in the log.
const SLOW_CALL: std::time::Duration = std::time::Duration::from_millis(400);

fn launched() -> std::time::Instant {
    static AT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *AT.get_or_init(std::time::Instant::now)
}

static CLICKED_BANNER: Mutex<Option<(i64, String)>> = Mutex::new(None);

/// The inbox and conversation a clicked banner was about. The window asks rather than being told,
/// as the click can be what started the app, before the window is listening.
#[tauri::command]
fn clicked_banner() -> Option<(i64, String)> {
    CLICKED_BANNER.lock().unwrap().take()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn open_from_banner(app: &tauri::AppHandle, thread: Option<(i64, String)>) {
    *CLICKED_BANNER.lock().unwrap() = thread;
    show_main_window(app.clone());
    let _ = app.emit("banner-clicked", ());
}

/// Debug builds only: a self-test has no way to click a real banner.
#[tauri::command]
fn selftest_click_banner(app: tauri::AppHandle, account_id: i64, thread_id: String) {
    if selftest_env("INBOX_SELFTEST_SHARE").is_some() {
        open_from_banner(&app, Some((account_id, thread_id)));
    }
}

/// Matches the window's own buttons to the theme chosen in Settings.
#[tauri::command]
fn set_window_theme(window: tauri::WebviewWindow, theme: String) {
    let _ = window.set_theme(match theme.as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    });
}

/// The unread count on the Dock icon and beside the menu bar icon. Zero hides it.
#[tauri::command]
fn set_dock_badge(window: tauri::WebviewWindow, count: i64) {
    #[cfg(desktop)]
    let _ = window.set_badge_count((count > 0).then_some(count));
    #[cfg(target_os = "macos")]
    UNREAD_COUNT.store(count, std::sync::atomic::Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    if MENU_BAR_SHOWING.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        show_count_in_menu_bar(window.app_handle());
    }
    #[cfg(mobile)]
    let _ = (window, count);
}

/// The unread count as a red badge on the menu bar icon. Zero shows nothing.
#[cfg(target_os = "macos")]
fn show_count_in_menu_bar(app: &tauri::AppHandle) {
    let count = UNREAD_COUNT.load(std::sync::atomic::Ordering::Relaxed);
    if let Some(icon) = app.tray_by_id(MENU_BAR_ICON) {
        let _ = icon.set_title(None::<&str>);
        let _ = icon.with_inner_tray_icon(move |inner| {
            if let Some(item) = inner.ns_status_item() {
                badge::show(&item, count, false);
            }
        });
    }
}

/// Shows who a new email is from and what it's about beside the menu bar icon for a few
/// seconds, then the unread count again. A newer email takes over from an older one.
#[cfg(target_os = "macos")]
fn show_news_in_menu_bar(app: &tauri::AppHandle, alert: &mailcore::alerts::Alert) {
    use std::sync::atomic::Ordering;
    const LONGEST: usize = 48;
    let line = alert.second_line();
    let joined = if line.trim().is_empty() {
        alert.title.clone()
    } else {
        format!("{} · {}", alert.title, line)
    };
    let mut text: String = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > LONGEST {
        text = text
            .chars()
            .take(LONGEST - 1)
            .collect::<String>()
            .trim_end()
            .to_string()
            + "…";
    }
    let Some(icon) = app.tray_by_id(MENU_BAR_ICON) else {
        return;
    };
    let turn = MENU_BAR_SHOWING.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = icon.with_inner_tray_icon(|inner| {
        if let Some(item) = inner.ns_status_item() {
            badge::show(&item, 0, true);
        }
    });
    let _ = icon.set_title(Some(text));
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(MENU_BAR_NEWS);
        if MENU_BAR_SHOWING
            .compare_exchange(turn, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            show_count_in_menu_bar(&app);
        }
    });
}

/// Starts the app at login, or stops it. Also runs at each launch, in case the app has moved.
#[tauri::command]
fn set_open_at_login(app: tauri::AppHandle, on: bool) {
    // A development copy mustn't replace or remove the installed app's login item.
    #[cfg(target_os = "macos")]
    if !cfg!(debug_assertions) {
        let (Ok(home), Ok(exe)) = (app.path().home_dir(), std::env::current_exe()) else {
            return;
        };
        let folder = home.join("Library/LaunchAgents");
        if let Err(e) = login_item::set(&folder, &app.config().identifier, &exe, on) {
            eprintln!("login item failed: {e}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, on);
}

#[tauri::command]
fn set_menu_bar_icon(app: tauri::AppHandle, on: bool) {
    #[cfg(target_os = "macos")]
    if let Some(icon) = app.tray_by_id(MENU_BAR_ICON) {
        let _ = icon.set_visible(on);
        // A shown-again icon is a new one, without its badge.
        if on && MENU_BAR_SHOWING.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            show_count_in_menu_bar(&app);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, on);
}

#[tauri::command]
fn set_dock_icon(app: tauri::AppHandle, on: bool) {
    #[cfg(target_os = "macos")]
    {
        // Coming or going from the Dock sends an app behind the others for a moment. Staying on
        // top until it's the active app again means that doesn't show.
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.set_always_on_top(true);
        }
        let _ = app.set_activation_policy(if on {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        });
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(DOCK_CHANGE_SETTLES);
            show_main_window(handle.clone());
            std::thread::sleep(DOCK_ACTIVATING);
            if let Some(main) = handle.get_webview_window("main") {
                let _ = main.set_always_on_top(false);
            }
        });
        if on {
            std::thread::spawn(move || {
                for _ in 0..DOCK_COUNT_RESENDS {
                    if let Some(main) = app.get_webview_window("main") {
                        let count = UNREAD_COUNT.load(std::sync::atomic::Ordering::Relaxed);
                        let _ = main.set_badge_count((count > 0).then_some(count));
                    }
                    std::thread::sleep(DOCK_CHANGE_SETTLES);
                }
            });
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, on);
}

/// Above every other app's windows, even while the Mac is still making this the active app.
#[cfg(target_os = "macos")]
fn put_in_front(window: &tauri::WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let ns_window = ns_window as usize;
    let _ = window.run_on_main_thread(move || {
        let ns_window = ns_window as *mut objc2::runtime::AnyObject;
        let _: () = unsafe { objc2::msg_send![ns_window, orderFrontRegardless] };
    });
}

/// The app opens its own windows. Left on, the Mac's window restoring holds up the launch after a
/// crash to ask about reopening them, which nobody sees when it's starting at login.
#[cfg(target_os = "macos")]
fn never_ask_to_reopen_windows() {
    let key = objc2_foundation::NSString::from_str("ApplePersistenceIgnoreState");
    unsafe {
        let defaults: *mut objc2::runtime::AnyObject =
            objc2::msg_send![objc2::class!(NSUserDefaults), standardUserDefaults];
        let _: () = objc2::msg_send![defaults, setBool: true, forKey: &*key];
    }
}

/// A link on the app's own screens, such as a setup step's "Open Google App passwords".
/// Asked for straight from the screen, as a link that opens a new window isn't always
/// passed on by the Mac's web view, and then nothing happens.
#[tauri::command]
fn open_link(app: tauri::AppHandle, url: String) {
    if let Ok(url) = Url::parse(&url) {
        open_link_outside(&app, &url);
    }
}

#[tauri::command]
fn hide_this_window(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

/// The library lights the icon on any press, but only puts it out again after a left click.
#[cfg(target_os = "macos")]
fn stop_lighting_menu_bar_icon(icon: &tauri::tray::TrayIcon) {
    use objc2::{msg_send, rc::Retained, runtime::AnyObject};
    let _ = icon.with_inner_tray_icon(|inner| {
        let Some(item) = inner.ns_status_item() else {
            return;
        };
        unsafe {
            let button: Option<Retained<AnyObject>> = msg_send![&*item, button];
            if let Some(button) = button {
                let _: () = msg_send![&*button, highlight: false];
            }
        }
    });
}

/// With the screen off the Mac slows background apps right down: a look at an inbox went from 3
/// seconds to nearly 3 minutes, so new mail went unannounced until someone came back. The Mac can
/// still go to sleep with this on.
#[cfg(target_os = "macos")]
fn keep_up_with_mail() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &NSString::from_str("Watching for new mail"),
    );
    // For as long as the app runs.
    std::mem::forget(activity);
}

/// Settings… in the app's menu, where every Mac app has it.
#[cfg(target_os = "macos")]
fn add_settings_to_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    let menu = Menu::default(app.handle())?;
    if let Some(app_menu) = menu.items()?.first().and_then(|m| m.as_submenu().cloned()) {
        let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
        // Below About, and the line under it.
        app_menu.insert_items(&[&settings, &PredefinedMenuItem::separator(app)?], 2)?;
    }
    // Paste and Match Style under Paste, on ⌘⇧V as in Gmail and Chrome.
    let edit = menu
        .items()?
        .into_iter()
        .filter_map(|m| m.as_submenu().cloned())
        .find(|m| m.text().is_ok_and(|t| t == "Edit"));
    if let Some(edit) = edit {
        let plain = MenuItem::with_id(
            app,
            "paste-plain",
            "Paste and Match Style",
            true,
            Some("CmdOrCtrl+Shift+V"),
        )?;
        let items = edit.items()?;
        let after_paste = items
            .iter()
            .position(|i| {
                i.as_predefined_menuitem()
                    .is_some_and(|p| p.text().is_ok_and(|t| t == "Paste"))
            })
            .map_or(items.len(), |at| at + 1);
        edit.insert(&plain, after_paste)?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id() == "settings" {
            show_main_window(app.clone());
            let _ = app.emit("open-settings", ());
        } else if event.id() == "paste-plain" {
            let _ = app.run_on_main_thread(paste_as_plain_text);
        }
    });
    Ok(())
}

/// Pastes only the clipboard's text into whatever is being typed in. The web view hands the
/// page a paste with nothing but plain text, so the composer keeps its own style.
#[cfg(target_os = "macos")]
fn paste_as_plain_text() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel};
    unsafe {
        let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let nobody: *mut AnyObject = std::ptr::null_mut();
        let _: bool =
            msg_send![ns_app, sendAction: sel!(pasteAsPlainText:), to: nobody, from: nobody];
    }
}

/// The icon beside the clock. A click opens the unread panel, and a right-click opens the app's
/// window or puts it away.
#[cfg(target_os = "macos")]
fn add_menu_bar_icon(app: &tauri::App, visible: bool) -> tauri::Result<()> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let icon = TrayIconBuilder::with_id(MENU_BAR_ICON)
        .icon(tauri::include_image!("icons/menubar.png"))
        .icon_as_template(true)
        .tooltip("Mailzo")
        .on_tray_icon_event(|icon, event| {
            let TrayIconEvent::Click {
                button,
                button_state,
                rect,
                ..
            } = event
            else {
                return;
            };
            let app = icon.app_handle();
            match (button, button_state) {
                (MouseButton::Left | MouseButton::Right, MouseButtonState::Down) => {
                    panel::note_what_was_in_front(app)
                }
                (MouseButton::Left, MouseButtonState::Up) => panel::toggle(app, rect),
                (MouseButton::Right, MouseButtonState::Up) => {
                    stop_lighting_menu_bar_icon(icon);
                    panel::toggle_main(app);
                }
                _ => {}
            }
        })
        .build(app)?;
    icon.set_visible(visible)
}

/// Shows a notification, or notes it for the self-test, which has no way to see one.
fn show_alert(app: &tauri::AppHandle, alert: &mailcore::alerts::Alert) {
    if let Some(log) = selftest_env("INBOX_SELFTEST_LINKS") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(f, "alert: {}: {}", alert.title, alert.second_line());
        }
        return;
    }
    #[cfg(target_os = "macos")]
    if app
        .state::<Engine>()
        .setting("alert_menu_bar")
        .map_or(true, |v| v == "on")
    {
        show_news_in_menu_bar(app, alert);
    }
    #[cfg(target_os = "macos")]
    {
        let tone = alert
            .sound
            .then(|| app.state::<Engine>().alert_tone().unwrap_or_default());
        banners::show(
            &alert.title,
            &alert.subtitle,
            &alert.body,
            alert.thread.as_ref(),
            tone.as_deref(),
        );
    }
    #[cfg(not(target_os = "macos"))]
    if let Err(e) = app
        .notification()
        .builder()
        .title(&alert.title)
        .body(
            [alert.subtitle.as_str(), alert.body.as_str()]
                .iter()
                .filter(|l| !l.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .show()
    {
        eprintln!("notification failed: {e}");
    }
}

/// Plays by itself, so there's a sound even when banners are turned off for the app.
/// Where a sound is. Only the ones on offer, by name, so nothing else can reach this path.
#[cfg(target_os = "macos")]
fn tone_file(tone: &str) -> Option<std::path::PathBuf> {
    use mailcore::settings::{ALERT_TONES, MAC_MAIL_TONE};
    if !ALERT_TONES.contains(&tone) {
        return None;
    }
    if tone == MAC_MAIL_TONE {
        return Some("/System/Applications/Mail.app/Contents/Resources/New Mail.aiff".into());
    }
    let exe = std::env::current_exe().ok()?;
    Some(
        exe.parent()?
            .parent()?
            .join(format!("Resources/{tone}.wav")),
    )
}

/// Lets someone hear a sound while choosing it.
#[tauri::command]
fn preview_alert_tone(tone: String) {
    #[cfg(target_os = "macos")]
    if let Some(file) = tone_file(&tone) {
        std::thread::spawn(move || {
            let _ = std::process::Command::new("/usr/bin/afplay")
                .arg(file)
                .status();
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = tone;
}

/// Debug builds only: scripts/mac-selftest.sh drives the real screens with a
/// script, then this writes the results to a file and closes the app.
#[tauri::command]
fn selftest_report(app: tauri::AppHandle, report: String) {
    if !cfg!(debug_assertions) {
        return;
    }
    if let Ok(path) = std::env::var("INBOX_SELFTEST_OUT") {
        let _ = std::fs::write(path, report);
        app.exit(0);
    }
}

/// Debug builds only: lets two copies of the app pass notes during a self-test.
#[tauri::command]
fn selftest_share(name: String, value: Option<String>) -> Option<String> {
    let dir = selftest_env("INBOX_SELFTEST_SHARE")?;
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let path = std::path::Path::new(&dir).join(name);
    match value {
        Some(v) => std::fs::write(path, v).ok().map(|_| String::new()),
        None => std::fs::read_to_string(path).ok(),
    }
}

/// The Mac's print window, for whatever the screens have laid out for printing.
#[tauri::command]
fn print_page(window: tauri::WebviewWindow) {
    #[cfg(mobile)]
    let _ = &window;
    if let Some(log) = selftest_env("INBOX_SELFTEST_LINKS") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(f, "print");
        }
        return;
    }
    #[cfg(desktop)]
    let _ = window.print();
}

#[cfg(mobile)]
fn device_name() -> String {
    "iPhone".to_string()
}

/// The computer's own name, so people can recognise it when approving it.
#[cfg(desktop)]
fn device_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".to_string())
}

/// The self-test's throwaway folder and Keychain name, in debug builds only.
fn selftest_env(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok()
    } else {
        None
    }
}

/// A link clicked in an email arrives here. It opens in the real browser or mail
/// app, never inside this app, and only if it is a web or mail link.
/// Whether a window may go to this address itself: only the app's own pages, the email
/// frame's blank and inline documents, and files the page made (such as a PDF preview).
/// Everything else is a link, which goes to the browser or nowhere.
pub(crate) fn our_page(url: &Url, dev: Option<&Url>) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        // A blob made by one of our pages carries that page's address inside it.
        "blob" => Url::parse(url.path())
            .ok()
            .filter(|inner| inner.scheme() != "blob")
            .is_some_and(|inner| our_page(&inner, dev)),
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || dev.is_some_and(|d| {
                    d.scheme() == url.scheme()
                        && d.host_str() == url.host_str()
                        && d.port_or_known_default() == url.port_or_known_default()
                })
        }
        _ => false,
    }
}

/// The development server's address, in a development build only.
pub(crate) fn dev_page(app: &tauri::AppHandle) -> Option<Url> {
    if cfg!(debug_assertions) {
        app.config().build.dev_url.clone()
    } else {
        None
    }
}

/// Keeps a window on the app's own pages. A link it tries to follow opens in the browser
/// (web and mail links only); anything else is stopped.
pub(crate) fn guard(app: &tauri::AppHandle) -> impl Fn(&Url) -> bool + Send + 'static {
    let handle = app.clone();
    let dev = dev_page(app);
    move |url: &Url| {
        let ours = our_page(url, dev.as_ref());
        if !ours {
            open_link_outside(&handle, url);
        }
        ours
    }
}

fn open_link_outside(app: &tauri::AppHandle, url: &Url) {
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return;
    }
    if let Some(log) = selftest_env("INBOX_SELFTEST_LINKS") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(f, "{url}");
        }
        return;
    }
    let _ = app.opener().open_url(url.as_str(), None::<&str>);
}

/// Opens a saved attachment, or shows it in Finder. The engine has already checked
/// that this app saved it and that it is safe to open.
fn use_saved_file(app: &tauri::AppHandle, path: &str, open: bool) {
    if let Some(log) = selftest_env("INBOX_SELFTEST_LINKS") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let action = if open { "open" } else { "reveal" };
            let name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let _ = writeln!(f, "{action}: {name}");
        }
        return;
    }
    if open {
        let _ = app.opener().open_path(path, None::<&str>);
    } else {
        let _ = app.opener().reveal_item_in_dir(path);
    }
}

/// Opens Google's sign-in page in the real browser. The self-test has no person
/// to click, so it follows the page itself.
fn open_sign_in_page(app: &tauri::AppHandle, url: &str) {
    if selftest_env("INBOX_SELFTEST_GOOGLE").is_some() {
        let url = url.to_string();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("curl")
                .args(["-sL", "-o", "/dev/null", "--max-time", "10", &url])
                .status();
        });
        return;
    }
    let _ = app.opener().open_url(url, None::<&str>);
}

/// Something needs the person's answer now, whatever else they're looking at.
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        #[cfg(target_os = "macos")]
        put_in_front(&window);
    }
}

/// Started by an AI app rather than a person: passes messages to the running mail app.
#[cfg(desktop)]
fn mcp_bridge() {
    let home = std::env::var("HOME").unwrap_or_default();
    let socket = std::path::Path::new(&home)
        .join("Library/Application Support")
        .join(KEYCHAIN_SERVICE)
        .join("mcp.sock");
    if let Err(e) = tauri::async_runtime::block_on(mailcore::mcp::mcp_bridge(&socket)) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// Why the mail couldn't be opened, in words for the person. Every screen shows it.
struct CantStart(String);

/// Opens the mail database encrypted, with its key in the Keychain beside the other secrets.
/// Says whether the saved mail was unreadable and a new database was started.
fn open_mail(path: &std::path::Path, secrets: &dyn SecretStore) -> Result<(Store, bool), String> {
    use mailcore::store::Opening;
    let opened = Store::open_protected(path, secrets).map_err(|e| e.to_string())?;
    match &opened.opening {
        Opening::LeftUnencrypted(why) => {
            eprintln!("mail database not encrypted yet, trying again next launch: {why}")
        }
        Opening::StartedFresh(moved) => eprintln!(
            "mail database key missing, unreadable mail kept at {}",
            moved.display()
        ),
        Opening::Migrated => eprintln!("mail database encrypted"),
        Opening::Encrypted => {}
    }
    let fresh = matches!(opened.opening, Opening::StartedFresh(_));
    Ok((opened.store, fresh))
}

/// A computer updates itself from inside the app. A phone gets new versions from its store.
fn with_updates(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Ready::default());
    builder
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(desktop)]
    if std::env::args().any(|a| a == "--mcp") {
        mcp_bridge();
        return;
    }
    // The self-test asks what a test device kept through the program that kept it, which the
    // Keychain already trusts, so it never asks for the login password. Never in a release.
    #[cfg(debug_assertions)]
    if let Some(service) = std::env::args()
        .skip_while(|a| a != "--selftest-keychain-names")
        .nth(1)
    {
        for name in KeychainStore::new(&service).names().unwrap_or_default() {
            println!("{name}");
        }
        return;
    }
    #[cfg(target_os = "macos")]
    never_ask_to_reopen_windows();
    with_updates(tauri::Builder::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .register_asynchronous_uri_scheme_protocol("inboximage", |context, request, responder| {
            let app = context.app_handle().clone();
            let address = mailcore::images::image_address(request.uri().path());
            tauri::async_runtime::spawn(async move {
                let engine = app.state::<Engine>();
                let reply = tauri::http::Response::builder()
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Cache-Control", "max-age=86400");
                let reply = match engine.email_image(&address).await {
                    Ok((kind, bytes)) => reply.header("Content-Type", kind).body(bytes),
                    Err(_) => reply.status(404).body(Vec::new()),
                };
                if let Ok(reply) = reply {
                    responder.respond(reply);
                }
            });
        })
        .setup(|app| {
            let dir = match selftest_env("INBOX_SELFTEST_DATA") {
                Some(d) => d.into(),
                None => app.path().app_data_dir()?,
            };
            let secrets: Arc<dyn SecretStore> =
                match (selftest_env("INBOX_SELFTEST_KEYCHAIN"), SECRETS_HELPER) {
                    (Some(service), _) => Arc::new(KeychainStore::new(&service)),
                    (None, Some(helper)) => Arc::new(HelperStore::new(helper, KEYCHAIN_SERVICE)),
                    (None, None) => Arc::new(KeychainStore::new(KEYCHAIN_SERVICE)),
                };
            launched();
            let opening = std::time::Instant::now();
            let opened = open_mail(&dir.join("mail.db"), secrets.as_ref());
            eprintln!("mail database opened in {:.0?}", opening.elapsed());
            let (store, started_fresh) = match opened {
                Ok(opened) => opened,
                Err(problem) => {
                    // The screens say why, instead of the app closing with no word at all.
                    app.manage(CantStart(problem));
                    (Store::open_in_memory()?, false)
                }
            };
            let server =
                selftest_env("INBOX_SELFTEST_SERVER").unwrap_or(ACCOUNT_SERVER.to_string());
            let name = selftest_env("INBOX_SELFTEST_DEVICE").unwrap_or_else(device_name);
            let opener_handle = app.handle().clone();
            let file_handle = app.handle().clone();
            let alert_handle = app.handle().clone();
            let new_mail_handle = app.handle().clone();
            let downloads = match selftest_env("INBOX_SELFTEST_DATA") {
                Some(d) => std::path::Path::new(&d).join("downloads"),
                // A phone has no Downloads folder. Files go in the app's own documents.
                None => app
                    .path()
                    .download_dir()
                    .or_else(|_| app.path().document_dir())?,
            };
            let socket = dir.join("mcp.sock");
            let mut engine = Engine::new(store, secrets)
                .with_embedder(std::sync::Arc::new(|text: &str| meaning::embed(text)))
                .with_on_device(std::sync::Arc::new(apple_ai::AppleAi))
                .with_cloud(&server, &name)?
                .with_folders(dir.clone(), downloads)
                .with_file_opener(Arc::new(move |path: &str, open: bool| {
                    use_saved_file(&file_handle, path, open);
                }))
                .with_alert_sender(Arc::new(move |alert: &mailcore::alerts::Alert| {
                    show_alert(&alert_handle, alert);
                }))
                // The menu bar and the main window show new mail as its banner appears.
                .with_new_mail_listener(Arc::new(move |_account: i64| {
                    let _ = new_mail_handle.emit("new-mail", ());
                }))
                .with_url_opener(Arc::new(move |url: &str| {
                    open_sign_in_page(&opener_handle, url);
                }));
            if let Some(url) = selftest_env("INBOX_SELFTEST_GOOGLE") {
                engine = engine.with_google_auth_url(&url);
            }
            // The self-test's stand-in picture server runs on this Mac, which real emails
            // are never allowed to reach. Debug builds only, like every self-test setting.
            if selftest_env("INBOX_SELFTEST_DATA").is_some() {
                engine = engine.with_local_images();
            }
            if let Some(base) = selftest_env("INBOX_SELFTEST_OAUTH") {
                for provider in ["microsoft", "google"] {
                    engine = engine.with_mail_sign_in(MailSignIn {
                        provider: provider.into(),
                        auth_url: format!("{base}/authorize"),
                        token_url: format!("{base}/token"),
                        client_id: "fake-mail-client".into(),
                        client_secret: None,
                        scope: "mail".into(),
                        extra_consent: None,
                        extra: vec![],
                        servers: Some(Servers {
                            imap: ("127.0.0.1".into(), 1143, Security::Plain),
                            smtp: ("127.0.0.1".into(), 1025, SmtpSecurity::Plain),
                        }),
                    });
                }
            } else {
                if let Some(id) = MICROSOFT_CLIENT_ID {
                    engine = engine.with_mail_sign_in(MailSignIn::microsoft(id));
                }
                if let (Some(id), Some(secret)) = (GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET) {
                    engine = engine.with_mail_sign_in(MailSignIn::google(id, secret));
                }
            }
            if let Some(url) = selftest_env("INBOX_SELFTEST_DIRECTORY") {
                engine = engine.with_provider_lookup(&url, Arc::new(|_: &str| vec![]));
            }
            if let Some(url) = selftest_env("INBOX_SELFTEST_AI") {
                engine = engine.with_ai_url(&url);
            }
            if let Some(base) = selftest_env("INBOX_SELFTEST_AVATARS") {
                engine =
                    engine.with_avatar_sources(&format!("{base}/person"), &format!("{base}/logo"));
            }
            app.manage(engine);
            // Locked behind Touch ID from the start when locking is on, kept in the engine.
            #[cfg(target_os = "macos")]
            lock::start(app.handle());
            if started_fresh {
                show_alert(
                    app.handle(),
                    &mailcore::alerts::Alert {
                        title: "Your saved mail couldn't be opened".into(),
                        body: "Its key was missing from the Keychain, so Mailzo started fresh. \
                               Add your email accounts again and your mail will download again."
                            .into(),
                        sound: false,
                        thread: None,
                        ..Default::default()
                    },
                );
            }
            #[cfg(desktop)]
            if let Some(channel) = selftest_env("INBOX_SELFTEST_UPDATE") {
                // A secret saved before an update must still read after it, with no password asked.
                let kept = selftest_env("INBOX_SELFTEST_KEYCHAIN").map(|service| {
                    let store = KeychainStore::new(&service);
                    match store.get("update-test") {
                        Ok(Some(v)) => format!("keychain read {v}"),
                        _ => match store.set("update-test", "kept") {
                            Ok(()) => "keychain wrote".to_string(),
                            Err(e) => format!("keychain failed {e}"),
                        },
                    }
                });
                updates::selftest(app.handle().clone(), channel, dir.join("update.log"), kept);
            }
            #[cfg(desktop)]
            {
                let serving = app.handle().clone();
                let can_start = app.try_state::<CantStart>().is_none();
                tauri::async_runtime::spawn(async move {
                    let engine = serving.state::<Engine>();
                    if can_start {
                        let _ = engine.mcp_serve(&socket).await;
                    }
                });
                #[cfg(debug_assertions)]
                if dir.join("dev-calls-on").exists() {
                    let serving = app.handle().clone();
                    let path = dir.join("dev-calls.sock");
                    tauri::async_runtime::spawn(async move {
                        let engine = serving.state::<Engine>();
                        let _ = engine.dev_serve(&path).await;
                    });
                }
            }
            #[cfg(mobile)]
            let _ = socket;

            #[cfg(target_os = "macos")]
            {
                add_settings_to_menu(app)?;
                keep_up_with_mail();
            }

            // The scripted self-tests need a normal app, with a Dock icon and nothing else.
            #[cfg(target_os = "macos")]
            if selftest_env("INBOX_SELFTEST_JS").is_none() {
                let engine = app.state::<Engine>();
                let on = |key| engine.setting(key).is_ok_and(|value| value == "on");
                add_menu_bar_icon(app, on("show_in_menu_bar"))?;
                panel::add(app)?;
                banners::ask(app.handle());
                if !on("show_in_dock") {
                    app.handle()
                        .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
                }
                set_open_at_login(app.handle().clone(), on("open_at_login"));
                dock_watch::watch(app.handle().clone());
                idle_meanings::start(app.handle().clone());
            }

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default());
            #[cfg(desktop)]
            let window = {
                let handle = app.handle().clone();
                window
                    .title("Mailzo")
                    .inner_size(1320.0, 860.0)
                    .min_inner_size(1020.0, 600.0)
                    // Otherwise a window that's behind others ignores the first press, so it can't
                    // be dragged by its top bar until it has been clicked once.
                    .accept_first_mouse(true)
                    // Lets files dropped on a message reach the screen as attachments.
                    .disable_drag_drop_handler()
                    .on_navigation(guard(app.handle()))
                    .on_new_window(move |url, _features| {
                        open_link_outside(&handle, &url);
                        NewWindowResponse::Deny
                    })
            };
            // A phone's web view has no separate windows, so a link in an email shows up as
            // this page trying to go somewhere. It goes to the browser instead.
            #[cfg(mobile)]
            let window = {
                let handle = app.handle().clone();
                window.on_navigation(move |url| {
                    let ours = matches!(url.scheme(), "tauri" | "about" | "data")
                        || url.host_str() == Some("tauri.localhost");
                    if !ours {
                        open_link_outside(&handle, url);
                    }
                    ours
                })
            };
            // The screens run up to the top edge, with the window buttons on them.
            #[cfg(target_os = "macos")]
            let window = window
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true)
                .traffic_light_position(tauri::LogicalPosition::new(16.0, 18.0))
                .visible(!std::env::args().any(|a| a == login_item::BACKGROUND))
                // Mail is checked from the page, which has to keep running while it's hidden.
                .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);
            let main_window = window.build()?;
            #[cfg(target_os = "macos")]
            right_click::tidy_menus(&main_window);
            #[cfg(target_os = "macos")]
            window_drag::start_drags_from_the_press(&main_window);
            mouse_buttons::go_back_and_forward(&main_window);
            #[cfg(not(target_os = "macos"))]
            let _ = main_window;
            Ok(())
        })
        // Closing the window only hides it, so mail keeps arriving until the person quits.
        .on_window_event(|window, event| {
            // Time away from Mailzo counts towards locking it again.
            if let tauri::WindowEvent::Focused(focused) = event {
                lock::note_focus(*focused);
            }
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
        })
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                if let Some(path) = selftest_env("INBOX_SELFTEST_JS") {
                    if let Ok(js) = std::fs::read_to_string(path) {
                        let _ = webview.eval(js);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            lock::prove_its_me,
            lock::unlock_app,
            preview_alert_tone,
            show_main_window,
            clicked_banner,
            call,
            set_window_theme,
            set_dock_badge,
            set_open_at_login,
            set_menu_bar_icon,
            set_dock_icon,
            hide_this_window,
            open_link,
            selftest_report,
            selftest_share,
            selftest_click_banner,
            print_page,
            pasteboard::copy_text,
            #[cfg(desktop)]
            updates::app_version,
            #[cfg(desktop)]
            updates::update_check,
            #[cfg(desktop)]
            updates::update_install,
            #[cfg(desktop)]
            updates::update_forget
        ])
        .build(tauri::generate_context!())
        .expect("the app failed to start")
        .run(|app, event| {
            // The Dock icon was clicked, or the app was opened again from Finder.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main_window(app.clone());
            }
            // The very last moment of quitting, after everything has closed. The normal way out
            // then runs the system's clean-up while mail may still be read on other threads,
            // which took things away from under the encrypted database and crashed the app on
            // its way out ("Mailzo quit unexpectedly"). Leaving at once skips that; the database
            // keeps everything it finished saving.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Exit = event {
                app.cleanup_before_exit();
                // SAFETY: ends this process straight away; nothing after it runs.
                unsafe { libc::_exit(0) };
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_stay_on_the_apps_own_pages() {
        let dev: Url = "http://127.0.0.1:5177".parse().unwrap();
        let ok = |u: &str, dev: Option<&Url>| our_page(&u.parse().unwrap(), dev);
        assert!(ok("tauri://localhost/index.html?panel", None));
        assert!(ok("https://tauri.localhost/", None));
        assert!(ok("about:srcdoc", None));
        assert!(ok("about:blank", None));
        assert!(ok("blob:tauri://localhost/1234", None));
        assert!(ok("http://127.0.0.1:5177/", Some(&dev)));
        for bad in [
            "https://example.com/",
            "http://127.0.0.1:5177/",
            "http://127.0.0.1:9999/",
            "file:///etc/passwd",
            "data:text/html,hi",
            "blob:https://example.com/1234",
            "javascript:alert(1)",
            "tauri://evil/",
            "https://tauri.localhost.evil.example/",
            "x-apple.systempreferences:com.apple.preference",
        ] {
            assert!(!ok(bad, None), "{bad}");
        }
        assert!(!ok("http://127.0.0.1:9999/", Some(&dev)));
    }
}
