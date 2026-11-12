//! The side buttons many mice have go back and forward, as they do in a browser.
//!
//! macOS gives those buttons to the window rather than the page, so they're watched for here
//! and passed on to the page, which moves just as its own back and forward buttons do. They
//! work over an open email too, whose frame the page can't hear. With no driver they come as
//! buttons 3 and 4; drivers such as Logi Options+ send them as a swipe instead, back as a
//! swipe to the right and forward as one to the left.

use block2::RcBlock;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use tauri::Emitter;

/// NSEventMaskOtherMouseUp: a button other than the left and right being let go.
const OTHER_RELEASES: u64 = 1 << 26;
/// NSEventMaskSwipe, and the NSEventTypeSwipe it watches for.
const SWIPES: u64 = 1 << 31;
const SWIPE: usize = 31;
/// The side buttons, numbered the way browsers number them.
const BACK_BUTTON: isize = 3;
const FORWARD_BUTTON: isize = 4;

pub fn go_back_and_forward(window: &tauri::WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let main = ns_window as usize;
    let window = window.clone();
    let watch = RcBlock::new(move |event: *mut AnyObject| -> *mut AnyObject {
        let (from, kind): (*mut AnyObject, usize) =
            unsafe { (msg_send![event, window], msg_send![event, type]) };
        let way = if kind == SWIPE {
            let across: f64 = unsafe { msg_send![event, deltaX] };
            if across < 0.0 {
                "back"
            } else if across > 0.0 {
                "forward"
            } else {
                return event;
            }
        } else {
            let button: isize = unsafe { msg_send![event, buttonNumber] };
            match button {
                BACK_BUTTON => "back",
                FORWARD_BUTTON => "forward",
                _ => return event,
            }
        };
        if from as usize != main {
            return event;
        }
        let _ = window.emit("travel", way);
        // Handled here, so the page doesn't also try to go through a history it doesn't have.
        std::ptr::null_mut()
    });
    unsafe {
        let _: *mut AnyObject = msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: OTHER_RELEASES | SWIPES,
            handler: &*watch
        ];
    }
}
