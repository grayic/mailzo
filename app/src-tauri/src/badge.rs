//! The red unread badge on the menu bar icon, like other Mac apps have.
//!
//! The icon itself stays a template image, so it keeps matching the menu bar's light or dark
//! look; the badge is a small red picture laid over its corner. A picture, not a live box and
//! label: the menu bar copies the icon to every screen, and a box and label kept asking to be
//! copied again, which held the Mac at half a core for as long as the app ran.

use std::cell::RefCell;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAttributedStringNSStringDrawing, NSBezierPath, NSColor, NSFont, NSImage, NSImageView,
    NSStatusBarButton, NSStatusItem, NSTextField, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Room between the number and the badge's left and right edges.
const PADDING: f64 = 1.0;
/// The badge's height, and its smallest width, so one or two digits make a circle.
const SIZE: f64 = 15.0;
const TEXT_SIZE: f64 = 8.0;
/// How far the badge reaches over the icon's corner, so the icon stays almost whole.
const OVERLAP: f64 = 3.0;
/// The menu bar's own "as wide as it needs" length.
const VARIABLE_LENGTH: f64 = -1.0;

thread_local! {
    static BADGE: RefCell<Option<(Retained<NSImageView>, Retained<NSTextField>)>> = const { RefCell::new(None) };
    /// The icon the badge sits on. Hiding and showing the icon makes a new one.
    static PARENT: RefCell<Option<Retained<NSStatusBarButton>>> = const { RefCell::new(None) };
    /// The icon as the app gave it, and the same icon with room on its right for the badge.
    static PLAIN: RefCell<Option<Retained<NSImage>>> = const { RefCell::new(None) };
    static ROOMY: RefCell<Option<Retained<NSImage>>> = const { RefCell::new(None) };
}

/// The picture's view, and a label that's never shown, kept for how its number looks.
fn make(mtm: MainThreadMarker) -> (Retained<NSImageView>, Retained<NSTextField>) {
    let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(TEXT_SIZE)));
    label.setTextColor(Some(&NSColor::whiteColor()));
    (NSImageView::new(mtm), label)
}

/// Shows `count` on the icon, or hides the badge when it's zero or `hidden` is set.
pub fn show(item: &NSStatusItem, count: i64, hidden: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(button) = item.button(mtm) else {
        return;
    };
    // The icon as given, never the one with room for the badge.
    let shown = button.image();
    let is_roomy = |image: &NSImage| {
        ROOMY.with(|r| {
            r.borrow()
                .as_ref()
                .is_some_and(|r| std::ptr::eq::<NSImage>(&**r, image))
        })
    };
    let plain = PLAIN.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(image) = shown.as_ref().filter(|i| !is_roomy(i)) {
            *p = Some(image.clone());
        }
        p.clone()
    });
    BADGE.with(|cell| {
        let mut cell = cell.borrow_mut();
        if count <= 0 || hidden {
            if let Some((badge, _)) = cell.as_ref() {
                badge.setHidden(true);
            }
            if shown.as_ref().is_some_and(|i| is_roomy(i)) {
                button.setImage(plain.as_deref());
            }
            item.setLength(VARIABLE_LENGTH);
            return;
        }
        let (badge, label) = cell.get_or_insert_with(|| make(mtm));
        let on_button = PARENT.with(|parent| {
            let mut parent = parent.borrow_mut();
            let same = parent
                .as_ref()
                .is_some_and(|p| std::ptr::eq::<NSStatusBarButton>(&**p, &*button));
            *parent = Some(button.clone());
            same
        });
        if !on_button {
            badge.removeFromSuperview();
            // Underneath the view that takes the icon's clicks, so a click on the badge still
            // opens the panel.
            button.addSubview_positioned_relativeTo(badge, NSWindowOrderingMode::Below, None);
        }
        let text = if count > 99 {
            "99+".to_string()
        } else {
            count.to_string()
        };
        label.setStringValue(&NSString::from_str(&text));
        let number = label.attributedStringValue();
        // A circle for one or two digits, a pill only when the number needs more room.
        let text = number.size();
        let height = (text.height.ceil() + 2.0 * PADDING).max(SIZE);
        let width = (text.width.ceil() + 2.0 * PADDING).max(SIZE);
        // Drawn when shown, at the sharpness each screen needs, with the number in the middle.
        let draw = RcBlock::new(move |rect: NSRect| -> Bool {
            NSColor::systemRedColor().setFill();
            let round = rect.size.height / 2.0;
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, round, round).fill();
            let size = number.size();
            number.drawAtPoint(NSPoint::new(
                ((rect.size.width - size.width) / 2.0).round(),
                ((rect.size.height - size.height) / 2.0).round(),
            ));
            Bool::YES
        });
        let picture =
            NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(width, height), false, &draw);
        badge.setImage(Some(&picture));

        // The icon stays where it always is, and the slot grows on its right only, by as much
        // of the badge as reaches past it: the icon carries that much clear room on its right.
        // A slot of a set length gets the menu bar's own space either side of it, so that
        // length is just the icon and its room.
        let icon = plain
            .as_ref()
            .map_or(NSSize::new(18.0, 18.0), |image| image.size());
        let reach = width - OVERLAP;
        if let Some(plain) = plain.clone() {
            let draw = RcBlock::new(move |_: NSRect| -> Bool {
                plain.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), icon));
                Bool::YES
            });
            let roomy = NSImage::imageWithSize_flipped_drawingHandler(
                NSSize::new(icon.width + reach, icon.height),
                false,
                &draw,
            );
            roomy.setTemplate(true);
            button.setImage(Some(&roomy));
            ROOMY.with(|r| *r.borrow_mut() = Some(roomy));
        }
        item.setLength(icon.width + reach);
        let x = icon.width - OVERLAP;
        let bounds = button.bounds();
        // Inside the icon's own bounds: anything outside them is redrawn every time.
        let y = if button.isFlipped() {
            bounds.size.height - height
        } else {
            0.0
        };
        badge.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
        badge.setHidden(false);
    });
}
