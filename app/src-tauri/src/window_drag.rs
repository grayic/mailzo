//! Starts a window drag from the press itself.
//!
//! A drag region asks for the drag from the page, which takes a moment. By then the pointer has
//! usually moved, or the app has just come to the front, so the "current event" that Tauri hands
//! to macOS is no longer the press, and macOS quietly does nothing.

use std::cell::RefCell;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};

/// NSEventMaskLeftMouseDown.
const LEFT_PRESSES: u64 = 1 << 1;
const LEFT_BUTTON: usize = 1;

thread_local! {
    static LAST_PRESS: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

extern "C-unwind" fn drag_from_the_press(this: &AnyObject, _: Sel, current: *mut AnyObject) {
    unsafe {
        // A click can be over before the request gets here, and a drag started then would have
        // nothing to end it.
        let held: usize = msg_send![class!(NSEvent), pressedMouseButtons];
        if held & LEFT_BUTTON == 0 {
            return;
        }
        let press = LAST_PRESS.with(|last| last.borrow().clone());
        let event = press
            .as_deref()
            .map_or(current.cast_const(), |p| p as *const AnyObject);
        let _: () = msg_send![super(this, class!(NSWindow)), performWindowDragWithEvent: event];
    }
}

/// For the main window's class. The class is asked of the real window, as the library makes it.
pub fn start_drags_from_the_press(window: &tauri::WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    unsafe {
        let its_class: *const AnyClass = msg_send![ns_window.cast::<AnyObject>(), class];
        let hook: extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) = drag_from_the_press;
        ffi::class_addMethod(
            its_class.cast_mut(),
            sel!(performWindowDragWithEvent:),
            std::mem::transmute::<*const (), Imp>(hook as *const ()),
            c"v@:@".as_ptr(),
        );
        let remember = RcBlock::new(|event: *mut AnyObject| -> *mut AnyObject {
            LAST_PRESS.with(|last| *last.borrow_mut() = Retained::retain(event));
            event
        });
        let _: *mut AnyObject = msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: LEFT_PRESSES,
            handler: &*remember
        ];
    }
}
