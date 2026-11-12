//! The small window under the menu bar icon, for dealing with unread mail.

use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::webview::NewWindowResponse;
use tauri::{LogicalPosition, Manager, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "panel";

/// Whether the panel was in use when the icon was pressed.
static PANEL_WAS_IN_USE: AtomicBool = AtomicBool::new(false);
/// Whether the main window was in use when the panel opened, to go back to when it closes.
static MAIN_WAS_IN_FRONT: AtomicBool = AtomicBool::new(false);
const WIDTH: f64 = 400.0;
const HEIGHT: f64 = 560.0;

/// Between the panel and the screen's edge.
const GAP: f64 = 6.0;

/// The Mac's `NSWindowStyleMaskNonactivatingPanel`.
const NON_ACTIVATING: usize = 1 << 7;
/// The Mac's `NSWindowCollectionBehaviorFullScreenAuxiliary`: it can open over a full-screen app.
const OVER_FULL_SCREEN: usize = 1 << 8;

/// Centred under the icon, but never past the edge of the icon's screen.
fn left_edge(icon_left: f64, icon_width: f64, screen_left: f64, screen_width: f64) -> f64 {
    let centred = icon_left + icon_width / 2.0 - WIDTH / 2.0;
    centred
        .min(screen_left + screen_width - WIDTH - GAP)
        .max(screen_left + GAP)
}

/// Built when the app starts, so it opens at once when the icon is clicked.
pub fn add(app: &tauri::App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let panel = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?panel".into()))
        .title("Mailzo")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .visible(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        // Lets files dropped on a message reach the screen as attachments.
        .disable_drag_drop_handler()
        .on_navigation(crate::guard(app.handle()))
        .on_new_window(move |url, _features| {
            crate::open_link_outside(&handle, &url);
            NewWindowResponse::Deny
        })
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        // A message waiting out its undo time counts down here, even with the panel closed.
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .build()?;
    become_a_mac_panel(&panel);
    hide_window_buttons(&panel);
    Ok(())
}

/// Accepts the keyboard, which a window with no title of its own needn't, and leaves Esc to the
/// screen, as a Mac panel would otherwise close itself even in the middle of a reply.
fn panel_class() -> &'static AnyClass {
    extern "C-unwind" fn yes(_: &AnyObject, _: Sel) -> Bool {
        Bool::YES
    }
    extern "C-unwind" fn ignore(_: &AnyObject, _: Sel, _: *mut AnyObject) {}
    let mut class = ClassBuilder::new(c"DaklyPanel", class!(NSPanel)).expect("only added once");
    // The window library stores a little of its own after the window, and still reaches for it.
    class.add_ivar::<usize>(c"room_for_the_window_library");
    unsafe {
        class.add_method(
            sel!(canBecomeKeyWindow),
            yes as extern "C-unwind" fn(_, _) -> _,
        );
        class.add_method(
            sel!(cancelOperation:),
            ignore as extern "C-unwind" fn(_, _, _),
        );
    }
    class.register()
}

/// An ordinary window can only take the keyboard by making its whole app the active one, which
/// brings the main window forward. A Mac panel takes it and leaves every app where it was.
fn become_a_mac_panel(window: &WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let ns_window = ns_window.cast::<AnyObject>();
    unsafe {
        // Safe to swap, as the two are the same size and a Mac panel adds nothing of its own.
        AnyObject::set_class(&*ns_window, panel_class());
        let style: usize = msg_send![ns_window, styleMask];
        let _: () = msg_send![ns_window, setStyleMask: style | NON_ACTIVATING];
        let behavior: usize = msg_send![ns_window, collectionBehavior];
        let _: () = msg_send![ns_window, setCollectionBehavior: behavior | OVER_FULL_SCREEN];
        let _: () = msg_send![ns_window, setHidesOnDeactivate: false];
    }
}

/// A titled window keeps the Mac's own corners and shadow. Only its buttons have to go.
fn hide_window_buttons(window: &WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let ns_window = ns_window.cast::<AnyObject>();
    // Close, minimise and zoom.
    for kind in 0usize..=2 {
        unsafe {
            let button: *mut AnyObject = msg_send![ns_window, standardWindowButton: kind];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: true];
            }
        }
    }
}

/// Pressing in the menu bar hands the front to whichever app owns the menu bar, and without a
/// Dock icon that's never this one. That also closes the panel, as it has lost the focus. So this
/// looks before any of that happens, and letting go of the icon goes by what it saw.
pub fn note_what_was_in_front(app: &tauri::AppHandle) {
    let in_use = |label: &str| {
        app.get_webview_window(label)
            .is_some_and(|window| window.is_focused().unwrap_or(false))
    };
    let panel_in_use = in_use(LABEL);
    PANEL_WAS_IN_USE.store(panel_in_use, Ordering::Relaxed);
    if !panel_in_use {
        // A window stays the app's chosen one even while the app is behind the others.
        let app_in_use: bool = unsafe {
            let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            msg_send![ns_app, isActive]
        };
        MAIN_WAS_IN_FRONT.store(app_in_use && in_use("main"), Ordering::Relaxed);
    }
    // Staying on top through the handover means the main window never drops behind and comes back.
    if let Some(main) = main_if_it_was_in_front(app) {
        let _ = main.set_always_on_top(true);
    }
}

/// The main window is left alone unless the press could have moved it from the front.
fn main_if_it_was_in_front(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main").filter(|main| {
        MAIN_WAS_IN_FRONT.load(Ordering::Relaxed) && main.is_visible().unwrap_or(false)
    })
}

/// Puts the main window away if it was what's in front, and otherwise brings it there.
pub fn toggle_main(app: &tauri::AppHandle) {
    let main = main_if_it_was_in_front(app);
    if let Some(main) = &main {
        let _ = main.set_always_on_top(false);
    }
    match main {
        Some(main) if !PANEL_WAS_IN_USE.load(Ordering::Relaxed) => {
            let _ = main.hide();
        }
        _ => crate::show_main_window(app.clone()),
    }
}

/// Shows the panel under the icon, or puts it away if it's already what's in front.
pub fn toggle(app: &tauri::AppHandle, icon: Rect) {
    let Some(panel) = app.get_webview_window(LABEL) else {
        return;
    };
    let main = main_if_it_was_in_front(app);
    if let Some(main) = &main {
        let _ = main.set_always_on_top(false);
        crate::put_in_front(main);
    }
    if PANEL_WAS_IN_USE.load(Ordering::Relaxed) {
        let _ = panel.hide();
        if let Some(main) = main {
            let _ = main.set_focus();
        }
        return;
    }
    if let Some((x, y)) = place_under(&panel, icon) {
        let _ = panel.set_position(LogicalPosition::new(x, y));
    }
    let _ = panel.show();
    crate::put_in_front(&panel);
}

/// Each screen has its own scale, so the icon's place only makes sense on the right one.
/// The icon was just clicked, so the pointer says which that is. The Mac gives the pointer's
/// place using the main screen's scale, and the icon's using its own screen's.
fn place_under(panel: &WebviewWindow, icon: Rect) -> Option<(f64, f64)> {
    let pointer = panel.cursor_position().ok()?;
    let main_scale = panel.primary_monitor().ok()??.scale_factor();
    let screen = panel
        .monitor_from_point(pointer.x / main_scale, pointer.y / main_scale)
        .ok()??;
    let scale = screen.scale_factor();
    let at = icon.position.to_logical::<f64>(scale);
    let size = icon.size.to_logical::<f64>(scale);
    let left = f64::from(screen.position().x) / scale;
    let width = f64::from(screen.size().width) / scale;
    Some((left_edge(at.x, size.width, left, width), at.y + size.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_sits_centred_under_the_icon() {
        assert_eq!(left_edge(1000.0, 40.0, 0.0, 2560.0), 820.0);
    }

    #[test]
    fn the_panel_stays_on_the_screen_near_either_edge() {
        assert_eq!(left_edge(2500.0, 40.0, 0.0, 2560.0), 2154.0);
        assert_eq!(left_edge(10.0, 20.0, 0.0, 2560.0), 6.0);
    }

    #[test]
    fn the_panel_stays_on_a_screen_left_of_the_main_one() {
        assert_eq!(left_edge(-30.0, 30.0, -1920.0, 1920.0), -406.0);
    }
}
