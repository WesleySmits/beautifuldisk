mod bar;
mod bulk;
mod card;
mod disk;
mod fsevents;
mod tree;

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
    let demo = std::env::args().any(|arg| arg == "--demo");
    application().with_assets(AppAssets).run(move |cx| {
        init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(600.0), px(300.0)), cx)),
            titlebar: None,
            is_resizable: false,
            ..Default::default()
        };
        open_window(options, cx, |_, cx| cx.new(|_| card::Card::new(demo)))
            .expect("failed to open window");
        cx.activate(true);
    });
}
