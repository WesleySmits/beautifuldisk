mod bar;
mod bulk;
mod card;
mod clutter;
mod disk;
#[cfg(target_os = "macos")]
mod fsevents;
/// Linux has no change journal that outlives the app, so every scan is full.
#[cfg(not(target_os = "macos"))]
mod fsevents {
    pub struct Change {
        pub path: std::path::PathBuf,
        pub recursive: bool,
    }

    pub fn current_id() -> u64 {
        0
    }

    pub fn changes_since(_since: u64, _roots: &[std::path::PathBuf]) -> Option<Vec<Change>> {
        None
    }
}
mod icons;
mod sound;
mod tree;
mod treemap;

use std::borrow::Cow;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

actions!(beautifuldisk, [Quit]);

assets::icon_assets!(
    ExtraIcons,
    [
        AppWindow,
        SquareTerminal,
        Image,
        Film,
        FileText,
        Music,
        Settings,
        Package,
        Trash,
        RefreshCw
    ]
);

/// Component icons plus the handful this app uses.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

fn main() {
    // `--demo` plays a scripted scan and clean with sample data and deletes nothing.
    // `--autoplay` is the demo playing itself: it hovers a few items, then clears.
    let autoplay = std::env::args().any(|arg| arg == "--autoplay");
    let demo = autoplay || std::env::args().any(|arg| arg == "--demo");
    // Where to put an autoplay window ("x,y"), so a recording can keep it
    // clear of the pointer.
    let at = std::env::var("BEAUTIFULDISK_AT")
        .ok()
        .filter(|_| autoplay)
        .and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
        });
    application().with_assets(AppAssets).run(move |cx| {
        init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        // Room for the cleanup list, but never more than 70% of the screen.
        let screen = cx
            .primary_display()
            .map_or(size(px(1440.0), px(900.0)), |d| d.bounds().size);
        let window = size(
            px(720.0).min(screen.width * 0.7),
            px(680.0).min(screen.height * 0.7),
        );
        let options = WindowOptions {
            window_bounds: Some(match at {
                Some((x, y)) => WindowBounds::Windowed(Bounds::new(point(px(x), px(y)), window)),
                None => WindowBounds::centered(window, cx),
            }),
            // macOS: no title bar. Linux: the title shows in the taskbar and
            // app switcher.
            titlebar: (!cfg!(target_os = "macos")).then(|| TitlebarOptions {
                title: Some("Beautiful Disk".into()),
                ..Default::default()
            }),
            // Matches beautifuldisk.desktop, so Linux docks show its icon.
            app_id: Some("beautifuldisk".into()),
            is_resizable: false,
            ..Default::default()
        };
        open_window(options, cx, |_, cx| {
            cx.new(|_| card::Card::new(demo, autoplay))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
