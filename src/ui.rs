//! One zoom for the whole interface, so it reads the same on a laptop and
//! on a big monitor set to more space. Styling in rems follows the theme's
//! font size; fixed sizes go through [`z`], and dot grids through [`snap`].
//!
//! It starts from the screen's height in points (a 32" 4K at "looks like
//! 2560x1440" gets 1.2, 1080 points or less stays at 1.0), and Cmd or
//! Ctrl with +, - and 0 change it. A change by hand is saved.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::component::Theme;
use gpui_kit::{App, Pixels, Size, px, size};

use crate::disk;

/// The window's size at zoom 1.
const WINDOW: (f32, f32) = (720.0, 680.0);
const STEP: f32 = 0.1;
const MIN: f32 = 0.8;
const MAX: f32 = 2.0;

static ZOOM: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0
static SCALE: AtomicU32 = AtomicU32::new(0x4000_0000); // 2.0

pub fn zoom() -> f32 {
    f32::from_bits(ZOOM.load(Ordering::Relaxed))
}

/// A design size at the current zoom.
pub fn z(v: f32) -> Pixels {
    px(s(v))
}

/// [`z`] as a plain number, for layout math.
pub fn s(v: f32) -> f32 {
    v * zoom()
}

/// [`s`] rounded to whole device pixels, so dot grids stay even at any zoom.
pub fn snap(v: f32) -> f32 {
    let scale = f32::from_bits(SCALE.load(Ordering::Relaxed));
    ((s(v) * scale).round() / scale).max(1.0 / scale)
}

/// The window's device pixels per point, as of its last frame.
pub fn set_scale(scale: f32) {
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
}

/// The zoom to start with: the saved one, or one picked for the screen.
pub fn initial(screen: Size<Pixels>) -> f32 {
    let zoom = saved().unwrap_or_else(|| (f32::from(screen.height) / 1200.0).clamp(1.0, 1.6));
    fit(zoom, screen)
}

/// Keeps a zoom above 1 from making the window bigger than `screen` has
/// room for. Never pushes it below 1: on a small screen the window gets
/// shorter instead, and the lists scroll.
fn fit(zoom: f32, screen: Size<Pixels>) -> f32 {
    let room =
        (f32::from(screen.width) * 0.9 / WINDOW.0).min(f32::from(screen.height) * 0.85 / WINDOW.1);
    if zoom > 1.0 {
        zoom.min(room.max(1.0))
    } else {
        zoom
    }
}

/// Sets the zoom for everything drawn from now on.
pub fn set(zoom: f32, cx: &mut App) {
    ZOOM.store(zoom.to_bits(), Ordering::Relaxed);
    Theme::global_mut(cx).font_size = px(16.0 * zoom);
}

/// The window at the current zoom, never more than 90% by 85% of the screen.
pub fn window_size(screen: Size<Pixels>) -> Size<Pixels> {
    size(
        z(WINDOW.0).min(screen.width * 0.9),
        z(WINDOW.1).min(screen.height * 0.85),
    )
}

pub enum Change {
    In,
    Out,
    Reset,
}

/// Zooms the open window by hand and remembers it, or forgets it on reset.
pub fn change(change: Change, cx: &mut App) {
    let screen = cx
        .primary_display()
        .map_or(size(px(1440.0), px(900.0)), |d| d.bounds().size);
    let zoom = match change {
        // Whole steps, so repeated presses don't drift.
        Change::In => ((zoom() + STEP).min(MAX) * 10.0).round() / 10.0,
        Change::Out => ((zoom() - STEP).max(MIN) * 10.0).round() / 10.0,
        Change::Reset => {
            let _ = fs::remove_file(path());
            initial(screen)
        }
    };
    let zoom = fit(zoom, screen);
    if !matches!(change, Change::Reset) {
        let path = path();
        let _ = fs::create_dir_all(path.parent().unwrap())
            .and_then(|_| fs::write(&path, format!("{zoom}\n")));
    }
    set(zoom, cx);
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, _| {
            window.resize(window_size(screen));
            window.refresh();
        });
    }
}

fn saved() -> Option<f32> {
    let zoom: f32 = fs::read_to_string(path()).ok()?.trim().parse().ok()?;
    zoom.is_finite().then(|| zoom.clamp(MIN, MAX))
}

fn path() -> PathBuf {
    let home = disk::home();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/beautifuldisk/zoom")
    } else {
        home.join(".config/beautifuldisk/zoom")
    }
}
