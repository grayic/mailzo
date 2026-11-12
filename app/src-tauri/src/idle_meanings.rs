//! Lets search by meaning catch up on older mail while nobody's using the Mac.
//!
//! Each refresh only does a little, to stay out of the way. That's slow after a big first download,
//! so this does more when it can't be noticed: nobody at the Mac, on power, at low priority, and in
//! short batches so it stops soon after they come back.

use mailcore::Engine;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const IDLE_AFTER_SECONDS: f64 = 60.0;
const LOOK_EVERY: Duration = Duration::from_secs(15);
const BETWEEN_BATCHES: Duration = Duration::from_secs(1);

/// kCGEventSourceStateCombinedSessionState and kCGAnyInputEventType.
const THIS_SESSION: i32 = 0;
const ANY_INPUT: u32 = u32::MAX;
/// kIOPSTimeRemainingUnlimited, which is what plugged in looks like.
const ON_POWER: f64 = -2.0;
/// QOS_CLASS_UTILITY.
const LOW_PRIORITY: u32 = 0x11;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSGetTimeRemainingEstimate() -> f64;
}

extern "C" {
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
}

fn mac_is_free() -> bool {
    unsafe {
        CGEventSourceSecondsSinceLastEventType(THIS_SESSION, ANY_INPUT) >= IDLE_AFTER_SECONDS
            && IOPSGetTimeRemainingEstimate() == ON_POWER
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        unsafe {
            pthread_set_qos_class_self_np(LOW_PRIORITY, 0);
        }
        loop {
            let did_some =
                mac_is_free() && app.state::<Engine>().work_out_meanings().unwrap_or(0) > 0;
            std::thread::sleep(if did_some {
                BETWEEN_BATCHES
            } else {
                LOOK_EVERY
            });
        }
    });
}
