mod access;
mod bar;
mod bulk;
mod card;
mod clutter;
mod disk;
mod docker;
mod files;
#[cfg(target_os = "macos")]
mod fsevents;
#[cfg(target_os = "macos")]
mod grant;
mod history;
mod mosaic;
mod pixel_text;
mod uninstall;
mod update;
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
mod ui;

use std::borrow::Cow;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

actions!(
    beautifuldisk,
    [Quit, CheckForUpdates, Dismiss, ZoomIn, ZoomOut, ZoomReset]
);

assets::icon_assets!(
    ExtraIcons,
    [RefreshCw, LayoutGrid, ArrowDown, Container, Check]
);

/// The app's own two-layer icons from `assets/icons`, served as
/// `icons/<name>.base.svg` and `icons/<name>.accent.svg`.
macro_rules! duo_icons {
    ($($name:literal),* $(,)?) => {
        &[$(
            (concat!("icons/", $name, ".base.svg"), include_bytes!(concat!("../assets/icons/", $name, ".base.svg"))),
            (concat!("icons/", $name, ".accent.svg"), include_bytes!(concat!("../assets/icons/", $name, ".accent.svg"))),
        )*]
    };
}

const DUO_ICONS: &[(&str, &[u8])] = duo_icons!(
    "apps",
    "developer",
    "photos",
    "movies",
    "documents",
    "music",
    "system",
    "caches",
    "trash",
    "desktop",
    "downloads",
    "full-disk",
);

/// Component icons plus the handful this app uses.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = DUO_ICONS.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(
            DUO_ICONS
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .map(|(p, _)| SharedString::from(*p)),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// The app's typeface. Numbers in tiles stay in Menlo.
pub const FONT: &str = "Figtree";

fn main() {
    // `--demo` plays a scripted scan and clean with sample data and deletes nothing.
    // `--autoplay` is the demo playing itself: it hovers a few items, then clears.
    let autoplay = std::env::args().any(|arg| arg == "--autoplay");
    let demo = autoplay || std::env::args().any(|arg| arg == "--demo");
    // `--no-sound` never opens an audio output stream.
    let sound = !std::env::args().any(|arg| arg == "--no-sound");
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
        // Figtree from Google Fonts, bundled so it looks the same everywhere.
        let fonts = [
            include_bytes!("../assets/fonts/Figtree-Regular.ttf").as_slice(),
            include_bytes!("../assets/fonts/Figtree-Medium.ttf"),
            include_bytes!("../assets/fonts/Figtree-SemiBold.ttf"),
            include_bytes!("../assets/fonts/Figtree-Bold.ttf"),
        ];
        let _ = cx
            .text_system()
            .add_fonts(fonts.into_iter().map(Cow::Borrowed).collect());
        Theme::global_mut(cx).font_family = FONT.into();
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", Quit, None),
            KeyBinding::new("escape", Dismiss, Some("Card")),
            // "secondary" is Cmd on macOS and Ctrl on Linux.
            KeyBinding::new("secondary-=", ZoomIn, None),
            KeyBinding::new("secondary-+", ZoomIn, None),
            KeyBinding::new("secondary--", ZoomOut, None),
            KeyBinding::new("secondary-0", ZoomReset, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &ZoomIn, cx| ui::change(ui::Change::In, cx));
        cx.on_action(|_: &ZoomOut, cx| ui::change(ui::Change::Out, cx));
        cx.on_action(|_: &ZoomReset, cx| ui::change(ui::Change::Reset, cx));
        #[cfg(target_os = "macos")]
        {
            cx.on_action(|_: &CheckForUpdates, _| update::check());
            cx.set_menus([Menu::new("Beautiful Disk").items([
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Quit Beautiful Disk", Quit),
            ])]);
            if !demo {
                update::start();
            }
        }

        let screen = cx
            .primary_display()
            .map_or(size(px(1440.0), px(900.0)), |d| d.bounds().size);
        ui::set(ui::initial(screen), cx);
        let window = ui::window_size(screen);
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
            cx.new(|cx| card::Card::new(demo, autoplay, sound, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
