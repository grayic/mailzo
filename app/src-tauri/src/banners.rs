//! Mac notifications, through the same system permission is asked from. Once an app has used this
//! one, the Mac turns away anything sent through the older one, without saying so.

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread};
use objc2_foundation::{NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DaklyBanners"]
    struct Banners;

    unsafe impl NSObjectProtocol for Banners {}

    unsafe impl UNUserNotificationCenterDelegate for Banners {
        /// Left to itself, the Mac shows nothing while the app is in front.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        /// Clicking one brings the window back, even if it was closed, on the email it's about.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn clicked(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &block2::DynBlock<dyn Fn()>,
        ) {
            if let Some(app) = APP.get() {
                let name = response.notification().request().identifier();
                crate::open_from_banner(app, thread_in(&name.to_string()));
            }
            done.call(());
        }
    }
);

/// A banner's name also says what it's about, as the Mac has dropped `targetContentIdentifier`,
/// the field meant for this, by the time a banner is delivered.
fn banner_name(sent: u128, thread: Option<&(i64, String)>) -> String {
    match thread {
        Some((account, thread)) => format!("{sent} {account} {thread}"),
        None => sent.to_string(),
    }
}

fn thread_in(name: &str) -> Option<(i64, String)> {
    let (_sent, about) = name.split_once(' ')?;
    let (account, thread) = about.split_once(' ')?;
    Some((account.parse().ok()?, thread.to_string()))
}

#[test]
fn a_banner_name_keeps_the_conversation_it_is_about() {
    let thread = (7, "a b@c".to_string());
    assert_eq!(thread_in(&banner_name(1, Some(&thread))), Some(thread));
    assert_eq!(thread_in(&banner_name(1, None)), None);
}

/// Reaching for these from a loose program, as in development, would crash it.
fn in_an_app() -> bool {
    std::env::current_exe().is_ok_and(|exe| crate::login_item::app_around(&exe).is_some())
}

/// The Mac only shows banners, and a count on the Dock icon, for apps allowed to. It asks once.
pub fn ask(app: &tauri::AppHandle) {
    if !in_an_app() {
        return;
    }
    let _ = APP.set(app.clone());
    let center = UNUserNotificationCenter::currentNotificationCenter();
    // The Mac only holds on to this loosely, and it's needed for as long as the app runs.
    let banners: Retained<Banners> = unsafe { msg_send![Banners::alloc(), init] };
    center.setDelegate(Some(ProtocolObject::from_ref(&*banners)));
    std::mem::forget(banners);
    let answered = block2::RcBlock::new(|_allowed: objc2::runtime::Bool, problem: *mut NSError| {
        if let Some(problem) = unsafe { problem.as_ref() } {
            eprintln!("notification permission failed: {problem}");
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Badge
            | UNAuthorizationOptions::Sound,
        &answered,
    );
}

/// Several emails arriving together make one sound.
fn first_in_a_while() -> bool {
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |t| t.as_secs() as i64);
    now - LAST.swap(now, Ordering::Relaxed) >= 2
}

/// The name to ask the Mac for. It plays its own default, without saying so, for a name it can't
/// find, and an app's own folders aren't dependable, so the sound is kept where it always looks.
fn sound_name(tone: &str) -> Option<String> {
    let from = crate::tone_file(tone)?;
    let name = format!("Dakly {tone}.{}", from.extension()?.to_str()?);
    let folder = std::path::Path::new(&std::env::var("HOME").ok()?).join("Library/Sounds");
    let to = folder.join(&name);
    let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).ok();
    if size(&to) != size(&from) {
        std::fs::create_dir_all(&folder).ok()?;
        std::fs::copy(&from, &to).ok()?;
    }
    Some(name)
}

/// The sender, the subject on one line (the Mac cuts it short with "…"), and what it says.
pub fn show(
    title: &str,
    subtitle: &str,
    body: &str,
    thread: Option<&(i64, String)>,
    tone: Option<&str>,
) {
    if !in_an_app() {
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    if !subtitle.is_empty() {
        content.setSubtitle(&NSString::from_str(subtitle));
    }
    content.setBody(&NSString::from_str(body));
    // The Mac plays it as the banner appears. Played from here, it came well before the banner.
    if let Some(tone) = tone.filter(|_| first_in_a_while()) {
        let sound = match sound_name(tone) {
            Some(name) => UNNotificationSound::soundNamed(&NSString::from_str(&name)),
            None => UNNotificationSound::defaultSound(),
        };
        content.setSound(Some(&sound));
    }
    // Sharing a name would replace the one before, rather than show another.
    let sent = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |t| t.as_nanos());
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&banner_name(sent, thread)),
        &content,
        None,
    );
    let sent = block2::RcBlock::new(|problem: *mut NSError| {
        if let Some(problem) = unsafe { problem.as_ref() } {
            eprintln!("notification failed: {problem}");
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&sent));
}
