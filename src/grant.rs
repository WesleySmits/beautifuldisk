//! Native pieces of the Full Disk Access helper: dragging this app into
//! System Settings' list, and keeping the helper next to the Settings
//! window. macOS lists an app for Full Disk Access only once it is added,
//! and dragging it in is the quickest way.

use std::path::PathBuf;

use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSRunningApplication, NSScreen, NSView, NSWorkspace,
};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BeautifulDiskDragSource"]
    struct DragSource;

    unsafe impl NSObjectProtocol for DragSource {}

    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy | NSDragOperation::Generic
        }
    }
);

/// The app bundle, or the bare binary when run outside one.
fn app_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(PathBuf::from);
    Some(bundle.unwrap_or(exe))
}

/// Starts dragging this app, from inside the mouse-down handler that is
/// running now.
pub fn drag_app() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return;
    };
    let Some(view) = event.window(mtm).and_then(|w| w.contentView()) else {
        return;
    };
    let Some(path) = app_path().and_then(|p| p.to_str().map(NSString::from_str)) else {
        return;
    };
    let url = NSURL::fileURLWithPath(&path);
    let item = NSDraggingItem::initWithPasteboardWriter(
        NSDraggingItem::alloc(),
        ProtocolObject::from_ref(&*url),
    );
    let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
    let at = view.convertPoint_fromView(event.locationInWindow(), None);
    let frame = NSRect::new(
        NSPoint::new(at.x - 32.0, at.y - 32.0),
        NSSize::new(64.0, 64.0),
    );
    // SAFETY: an NSImage is what the dragging frame's contents take.
    unsafe { item.setDraggingFrame_contents(frame, Some(&icon)) };
    let source: Retained<DragSource> = unsafe { msg_send![DragSource::alloc(mtm), init] };
    view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&[item]),
        &event,
        ProtocolObject::from_ref(&*source),
    );
    // The session doesn't keep its source alive, and it outlives this call.
    std::mem::forget(source);
}

/// The front System Settings window: x, y, width, height, with y measured
/// down from the top of the main screen. Window positions need no
/// permission to read.
pub fn settings_frame() -> Option<[f64; 4]> {
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    // SAFETY: CFArray and NSArray are toll-free bridged, and the window list
    // holds dictionaries.
    let list: Retained<NSArray<NSDictionary<NSString, AnyObject>>> = unsafe {
        Retained::from_raw(
            objc2_core_foundation::CFRetained::into_raw(list)
                .as_ptr()
                .cast(),
        )
    }?;
    let number = |dict: &NSDictionary<NSString, AnyObject>, key: &str| {
        let value = dict.objectForKey(&NSString::from_str(key))?;
        Some(value.downcast::<NSNumber>().ok()?.doubleValue())
    };
    for window in list.iter() {
        if number(&window, "kCGWindowLayer") != Some(0.0) {
            continue;
        }
        let Some(pid) = number(&window, "kCGWindowOwnerPID") else {
            continue;
        };
        let settings = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
            .and_then(|app| app.bundleIdentifier())
            .is_some_and(|id| id.to_string() == "com.apple.systempreferences");
        if !settings {
            continue;
        }
        let bounds = window
            .objectForKey(&NSString::from_str("kCGWindowBounds"))?
            .downcast::<NSDictionary>()
            .ok()?;
        // SAFETY: the bounds dictionary maps strings to numbers.
        let bounds: Retained<NSDictionary<NSString, AnyObject>> =
            unsafe { Retained::cast_unchecked(bounds) };
        return Some([
            number(&bounds, "X")?,
            number(&bounds, "Y")?,
            number(&bounds, "Width")?,
            number(&bounds, "Height")?,
        ]);
    }
    None
}

/// Puts `window` just under the System Settings window at `settings` (from
/// [`settings_frame`]), centered on it. With no room below, it overlaps the
/// window's bottom edge instead.
pub fn attach(window: &Window, settings: [f64; 4]) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points at the window's live NSView.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let (Some(panel), Some(main)) = (view.window(), NSScreen::screens(mtm).firstObject()) else {
        return;
    };
    let [x, y, w, h] = settings;
    let size = panel.frame().size;
    // AppKit measures up from the bottom of the main screen; flip to match.
    let screen_h = main.frame().size.height;
    let bottom = screen_h - main.visibleFrame().origin.y;
    let mut top = y + h + 10.0;
    if top + size.height > bottom {
        top = y + h - size.height - 16.0;
    }
    let left = x + (w - size.width) / 2.0;
    panel.setFrameTopLeftPoint(NSPoint::new(left, screen_h - top));
}
