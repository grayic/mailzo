//! Copying text through macOS itself, so it works from any window, the menu bar panel too,
//! however the click came.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;

/// Puts text on the clipboard. False if macOS wouldn't take it.
#[tauri::command]
pub fn copy_text(text: String) -> bool {
    unsafe {
        let board: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        let _: isize = msg_send![board, clearContents];
        let kind = NSString::from_str("public.utf8-plain-text");
        let text = NSString::from_str(&text);
        msg_send![board, setString: &*text, forType: &*kind]
    }
}
