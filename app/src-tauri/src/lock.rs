//! Locking the app: proving it's the Mac's owner with Touch ID, or the Mac's password
//! where there's no Touch ID. The check is the system's own; the app never sees a
//! fingerprint or the password, only whether it worked.

/// Asks the Mac to check it's its owner. True when they pass, false when they cancel
/// or fail. An error only when this Mac can't check at all.
#[tauri::command]
pub async fn prove_its_me(reason: String) -> Result<bool, String> {
    // The system's objects stay on one thread, which waits there for the answer.
    tauri::async_runtime::spawn_blocking(move || ask_the_mac(&reason))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(target_os = "macos")]
fn ask_the_mac(reason: &str) -> Result<bool, String> {
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    let context = unsafe { LAContext::new() };
    let policy = LAPolicy::DeviceOwnerAuthentication;
    if let Err(problem) = unsafe { context.canEvaluatePolicy_error(policy) } {
        return Err(problem.localizedDescription().to_string());
    }
    let (tell, answer) = std::sync::mpsc::channel::<bool>();
    let reply = block2::RcBlock::new(move |passed: objc2::runtime::Bool, _: *mut NSError| {
        let _ = tell.send(passed.as_bool());
    });
    let reason = NSString::from_str(reason);
    unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };
    // The context lives until here, so the check isn't cancelled while it's showing.
    let passed = answer.recv().unwrap_or(false);
    drop(context);
    Ok(passed)
}

#[cfg(not(target_os = "macos"))]
fn ask_the_mac(_reason: &str) -> Result<bool, String> {
    Err("This device can't lock the app yet.".into())
}

// ---- the lock itself, kept in the engine ----

use mailcore::Engine;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

/// With "after time away" chosen, this long without using Mailzo locks it again.
const AWAY: Duration = Duration::from_secs(15 * 60);
/// How often the lock looks at the time, the Mac's sleep and its screen lock.
const LOOK_EVERY: Duration = Duration::from_secs(5);
/// The wall clock running this far ahead of the app's own clock means the Mac slept.
const SLEPT: Duration = Duration::from_secs(30);

/// When the last of Mailzo's windows lost focus. None while one of them has it.
static AWAY_SINCE: Mutex<Option<SystemTime>> = Mutex::new(None);

fn lock_mode(engine: &Engine) -> String {
    engine.setting("app_lock").unwrap_or_else(|_| "off".into())
}

/// Locks the engine, and tells every window so the lock screen shows.
pub fn lock_now(app: &AppHandle) {
    let engine = app.state::<Engine>();
    if !engine.is_locked() {
        engine.set_locked(true);
        let _ = app.emit("lock-changed", ());
    }
}

/// The only way to unlock: the Mac checks it's its owner, and only then is the engine
/// unlocked. The screens can't unlock it by any other command.
#[tauri::command]
pub async fn unlock_app(app: AppHandle, reason: String) -> Result<bool, String> {
    let passed = prove_its_me(reason).await?;
    if passed {
        app.state::<Engine>().set_locked(false);
        *AWAY_SINCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = app.emit("lock-changed", ());
    }
    Ok(passed)
}

/// One of Mailzo's windows gained or lost focus.
pub fn note_focus(focused: bool) {
    let mut since = AWAY_SINCE.lock().unwrap_or_else(|e| e.into_inner());
    if focused {
        *since = None;
    } else if since.is_none() {
        *since = Some(SystemTime::now());
    }
}

/// Locked from the start when locking is on, then locked again after time away, when the
/// Mac sleeps and when its screen locks, with "after time away" chosen.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn start(app: &AppHandle) {
    let engine = app.state::<Engine>();
    if lock_mode(&engine) != "off" {
        engine.set_locked(true);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let (mut wall, mut steady) = (SystemTime::now(), Instant::now());
        loop {
            std::thread::sleep(LOOK_EVERY);
            let engine = app.state::<Engine>();
            // The app's own clock stops while the Mac sleeps; the wall clock doesn't.
            let wall_gone = wall.elapsed().unwrap_or_default();
            let slept = wall_gone > steady.elapsed() + SLEPT;
            (wall, steady) = (SystemTime::now(), Instant::now());
            if engine.is_locked() || lock_mode(&engine) != "away" {
                continue;
            }
            let away_too_long = AWAY_SINCE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|t| t.elapsed().unwrap_or_default() >= AWAY);
            if slept || away_too_long || screen_is_locked() {
                lock_now(&app);
            }
        }
    });
}

/// Whether the Mac's screen is locked now, as its login window reports it.
#[cfg(target_os = "macos")]
fn screen_is_locked() -> bool {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFRelease(cf: *const c_void);
        static kCFBooleanTrue: *const c_void;
    }
    let key = objc2_foundation::NSString::from_str("CGSSessionScreenIsLocked");
    unsafe {
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() {
            return false;
        }
        let key_ptr = objc2::rc::Retained::as_ptr(&key) as *const c_void;
        let value = CFDictionaryGetValue(session, key_ptr);
        let locked = !value.is_null() && value == kCFBooleanTrue;
        CFRelease(session);
        locked
    }
}

#[cfg(not(target_os = "macos"))]
fn screen_is_locked() -> bool {
    false
}
