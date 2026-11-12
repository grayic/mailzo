//! Takes out what the web view adds to every right-click menu but makes no sense in an app.

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};
use objc2_foundation::NSString;

/// Reload would restart the screens, losing anything still waiting out its Undo time. There's no
/// history to go back through, and an email's frame has nowhere to open.
const TAKEN_OUT: [&str; 4] = [
    "WKMenuItemIdentifierReload",
    "WKMenuItemIdentifierOpenFrameInNewWindow",
    "WKMenuItemIdentifierGoBack",
    "WKMenuItemIdentifierGoForward",
];

extern "C-unwind" fn will_open_menu(
    this: &AnyObject,
    _: Sel,
    menu: *mut AnyObject,
    event: *mut AnyObject,
) {
    unsafe {
        let _: () = msg_send![super(this, class!(WKWebView)), willOpenMenu: menu, withEvent: event];
        let count: isize = msg_send![menu, numberOfItems];
        for i in (0..count).rev() {
            let item: *mut AnyObject = msg_send![menu, itemAtIndex: i];
            let id: *mut NSString = msg_send![item, identifier];
            let unwanted = id
                .as_ref()
                .is_some_and(|id| TAKEN_OUT.contains(&id.to_string().as_str()));
            if unwanted {
                let _: () = msg_send![menu, removeItemAtIndex: i];
            }
        }
        // Lines only belong between items, so none are left at either end or side by side.
        let count: isize = msg_send![menu, numberOfItems];
        let mut after_a_line = true;
        for i in (0..count).rev() {
            let item: *mut AnyObject = msg_send![menu, itemAtIndex: i];
            let line: bool = msg_send![item, isSeparatorItem];
            if line && after_a_line {
                let _: () = msg_send![menu, removeItemAtIndex: i];
            }
            after_a_line = line;
        }
        let count: isize = msg_send![menu, numberOfItems];
        if count > 0 {
            let first: *mut AnyObject = msg_send![menu, itemAtIndex: 0isize];
            let line: bool = msg_send![first, isSeparatorItem];
            if line {
                let _: () = msg_send![menu, removeItemAtIndex: 0isize];
            }
        }
    }
}

/// For every web view, as they all share one class. The class is asked of a real one, as its name
/// changes with each version of the library.
pub fn tidy_menus(window: &tauri::WebviewWindow) {
    let _ = window.with_webview(|view| unsafe {
        let web_view = view.inner().cast::<AnyObject>();
        let its_class: *const AnyClass = msg_send![web_view, class];
        let hook: extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *mut AnyObject) =
            will_open_menu;
        ffi::class_addMethod(
            its_class.cast_mut(),
            sel!(willOpenMenu:withEvent:),
            std::mem::transmute::<*const (), Imp>(hook as *const ()),
            c"v@:@@".as_ptr(),
        );
    });
}
