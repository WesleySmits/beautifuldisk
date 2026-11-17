//! First launch on macOS: the folders macOS guards, as dithered tiles. Each
//! one asks for permission only when the user clicks it, instead of the
//! scan firing every prompt at once.

use std::time::Duration;

use super::*;
use crate::access::{self, Folder};

#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Waiting,
    /// macOS is showing its prompt.
    Asking,
    Allowed,
    Denied,
}

/// Rows: one per folder, then Full Disk Access.
const ROWS: usize = 4;
const FULL: usize = 3;

/// Share of each tile, from the left, kept free of dots for its text.
const TEXT_SPACE: f32 = 0.5;

pub(super) struct Onboarding {
    folders: [Ask; 3],
    /// What was last saved as allowed, to save again only on a change.
    saved: Vec<Folder>,
    full_disk: bool,
    hover: Option<usize>,
    glow: [f32; ROWS],
    /// Follows System Settings with the drag helper while it is open.
    #[cfg(target_os = "macos")]
    helper: bool,
}

impl Onboarding {
    /// `allowed`: folders allowed before the app last quit.
    fn new(allowed: Vec<Folder>) -> Self {
        let folders = Folder::ALL.map(|f| {
            if allowed.contains(&f) {
                Ask::Allowed
            } else {
                Ask::Waiting
            }
        });
        Self {
            folders,
            saved: allowed,
            full_disk: false,
            hover: None,
            glow: [0.0; ROWS],
            #[cfg(target_os = "macos")]
            helper: false,
        }
    }

    fn allowed(&self) -> Vec<Folder> {
        Folder::ALL
            .into_iter()
            .zip(self.folders)
            .filter(|&(_, ask)| ask == Ask::Allowed)
            .map(|(folder, _)| folder)
            .collect()
    }

    fn on(&self, row: usize) -> bool {
        if row == FULL {
            self.full_disk
        } else {
            self.folders[row] == Ask::Allowed
        }
    }

    fn asking(&self, row: usize) -> bool {
        row < FULL && self.folders[row] == Ask::Asking
    }

    /// Eases each tile's glow. True while anything still moves.
    pub(super) fn tick(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for row in 0..ROWS {
            let to = match (self.asking(row), self.hover == Some(row), self.on(row)) {
                (true, _, _) => 1.0,
                (_, true, true) => 1.0,
                (_, false, true) => 0.55,
                (_, true, false) => 0.8,
                (_, false, false) => 0.0,
            };
            self.glow[row] = approach(self.glow[row], to, dt, 0.06);
            if (self.glow[row] - to).abs() > 0.005 {
                moving = true;
            } else {
                self.glow[row] = to;
            }
            // The dots shimmer while macOS asks.
            moving |= self.asking(row);
        }
        moving
    }
}

fn row_tone(row: usize) -> Tone {
    match row {
        0 => Tone::Hue(225.0),
        1 => tone(Category::Documents),
        2 => Tone::Hue(185.0),
        _ => Tone::Green,
    }
}

const ICONS: [&str; ROWS] = ["desktop", "documents", "downloads", "full-disk"];
const TITLES: [&str; ROWS] = ["Desktop", "Documents", "Downloads", "Full Disk Access"];
const DETAILS: [&str; ROWS] = [
    "Screenshots and files on your desktop.",
    "Projects and files in Documents.",
    "Old installers and other downloads.",
    "All of the above, plus app data.",
];

/// A label on a solid chip, readable over the tile's dots.
fn chip() -> Div {
    h_flex()
        .gap_1p5()
        .px_2()
        .h(px(24.0))
        .rounded(px(6.0))
        .bg(rgb(0x232327))
        .border_1()
        .border_color(rgb(LINE))
        .text_size(px(12.0))
}

impl Card {
    /// Shows the folder list instead of scanning, if this is the first launch.
    pub(super) fn onboard(&mut self, cx: &mut Context<Self>) -> bool {
        if self.demo {
            return false;
        }
        let Some(allowed) = access::load(&disk::home()) else {
            return false;
        };
        self.onboarding = Some(Onboarding::new(allowed));
        // Picks up changes made in System Settings: a folder turned on after
        // it was denied, or Full Disk Access. Checking these never prompts.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let home = disk::home();
                let checked = this.update(cx, |this, cx| {
                    let Some(o) = &mut this.onboarding else {
                        return false;
                    };
                    let mut changed = false;
                    if !o.full_disk && access::full_disk(&home) {
                        // Every folder is readable now; the user can still
                        // switch any of them off.
                        o.full_disk = true;
                        for ask in &mut o.folders {
                            *ask = Ask::Allowed;
                        }
                        changed = true;
                    }
                    for (i, folder) in Folder::ALL.into_iter().enumerate() {
                        if o.folders[i] == Ask::Denied && access::request(&home, folder) {
                            o.folders[i] = Ask::Allowed;
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                    let allowed = o.allowed();
                    if allowed != o.saved {
                        access::save_progress(&home, &allowed);
                        o.saved = allowed;
                    }
                    true
                });
                if !matches!(checked, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        true
    }

    fn ask(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(o) = &mut self.onboarding else {
            return;
        };
        if row == FULL {
            if o.full_disk {
                return;
            }
            cx.open_url(access::FULL_DISK_SETTINGS);
            self.show_helper(cx);
            return;
        }
        match o.folders[row] {
            Ask::Waiting => {
                o.folders[row] = Ask::Asking;
                let folder = Folder::ALL[row];
                cx.spawn(async move |this, cx| {
                    // Blocks until the user answers the prompt.
                    let ok = cx
                        .background_executor()
                        .spawn(async move { access::request(&disk::home(), folder) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(o) = &mut this.onboarding {
                            o.folders[row] = if ok { Ask::Allowed } else { Ask::Denied };
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Ask::Denied => cx.open_url(access::FOLDER_SETTINGS),
            // Skip it. macOS keeps the permission, so turning it back on
            // doesn't ask again.
            Ask::Allowed => o.folders[row] = Ask::Waiting,
            Ask::Asking => {}
        }
    }

    /// macOS lists an app for Full Disk Access only once it's added, so a
    /// panel rides along under System Settings with the app's icon to drag
    /// into the list. It closes when access arrives or Settings closes.
    #[cfg(target_os = "macos")]
    fn show_helper(&mut self, cx: &mut Context<Self>) {
        use crate::grant;
        let Some(o) = &mut self.onboarding else {
            return;
        };
        if o.helper {
            return;
        }
        o.helper = true;
        cx.spawn(async move |this, cx| {
            let mut panel: Option<WindowHandle<Helper>> = None;
            // Settings takes a moment to open; give up if it never does.
            let mut waited = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let done = this
                    .update(cx, |this, _| {
                        this.onboarding.as_ref().is_none_or(|o| o.full_disk)
                    })
                    .unwrap_or(true);
                let frame = grant::settings_frame();
                waited += 1;
                if done || (frame.is_none() && (panel.is_some() || waited > 40)) {
                    break;
                }
                let Some(frame) = frame else {
                    continue;
                };
                if panel.is_none() {
                    let options = WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.0), px(0.0)),
                            size(px(400.0), px(84.0)),
                        ))),
                        titlebar: None,
                        focus: false,
                        kind: WindowKind::PopUp,
                        is_movable: false,
                        is_resizable: false,
                        is_minimizable: false,
                        ..Default::default()
                    };
                    panel = cx
                        .open_window(options, |_, cx| cx.new(|_| Helper::new()))
                        .ok();
                }
                if let Some(panel) = panel {
                    let _ = panel.update(cx, |_, window, _| grant::attach(window, frame));
                }
            }
            if let Some(panel) = panel {
                let _ = panel.update(cx, |_, window, _| window.remove_window());
            }
            let _ = this.update(cx, |this, _| {
                if let Some(o) = &mut this.onboarding {
                    o.helper = false;
                }
            });
        })
        .detach();
    }

    #[cfg(not(target_os = "macos"))]
    fn show_helper(&mut self, _cx: &mut Context<Self>) {}

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.onboarding.take() else {
            return;
        };
        access::save(&disk::home(), &o.allowed());
        self.scan(cx);
    }

    pub(super) fn render_onboarding(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(o) = &self.onboarding else {
            return div().into_any_element();
        };
        let muted = rgb(MUTED);
        let time = self.time;
        let rows = (0..ROWS).map(|row| {
            let tile = treemap::Tile {
                tone: row_tone(row),
                on: o.on(row),
                glow: o.glow[row],
                reveal: 1.0,
                busy: o.asking(row),
                clear: TEXT_SPACE,
            };
            let state = match (row, o.full_disk, o.folders.get(row)) {
                (FULL, true, _) | (_, _, Some(Ask::Allowed)) => chip()
                    .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                    .child("Allowed"),
                (FULL, false, _) => chip().child("Open Settings"),
                (_, _, Some(Ask::Asking)) => chip()
                    .child(Spinner::new().small().color(muted.into()))
                    .child("Waiting for macOS"),
                (_, _, Some(Ask::Denied)) => chip().child("Turn on in Settings"),
                _ => chip().child("Allow"),
            };
            div()
                .id(("access", row))
                .relative()
                .h(px(64.0))
                .w_full()
                .cursor_pointer()
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if let Some(o) = &mut this.onboarding {
                        if *hovered {
                            o.hover = Some(row);
                        } else if o.hover == Some(row) {
                            o.hover = None;
                        }
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ask(row, cx);
                    cx.notify();
                }))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            treemap::paint(&tile, bounds, bounds.origin, time, window)
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    h_flex()
                        .absolute()
                        .size_full()
                        .px_4()
                        .gap_3()
                        .child(duo(ICONS[row], row_tone(row), 20.0))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(div().font_weight(FontWeight::MEDIUM).child(TITLES[row]))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.0))
                                        .text_color(muted)
                                        .child(DETAILS[row]),
                                ),
                        )
                        .child(state.flex_none()),
                )
        });

        let asking = (0..FULL).any(|row| o.asking(row));
        v_flex()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .text_size(px(13.0))
            .px_5()
            .pt_5()
            .pb_3()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(26.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Choose what to scan"),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child("macOS asks once for each folder you pick."),
                    ),
            )
            .child(v_flex().gap(px(treemap::GAP)).children(rows))
            .child(div().flex_1())
            .child(div().flex_none().h(px(1.0)).bg(rgb(LINE)))
            .child(
                h_flex()
                    .h(px(30.0))
                    .justify_between()
                    .child(
                        div()
                            .text_color(muted)
                            .child("Folders you skip aren't scanned."),
                    )
                    .child(
                        Button::new("start")
                            .primary()
                            .small()
                            .label("Scan")
                            .disabled(asking)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.finish_onboarding(cx);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// The panel under System Settings: the app's icon to drag into the Full
/// Disk Access list.
#[cfg(target_os = "macos")]
struct Helper {
    icon: Arc<Image>,
    started: Instant,
}

#[cfg(target_os = "macos")]
impl Helper {
    fn new() -> Self {
        let png = include_bytes!("../../assets/icon/icon-1024.png");
        Self {
            icon: Arc::new(Image::from_bytes(ImageFormat::Png, png.to_vec())),
            started: Instant::now(),
        }
    }
}

#[cfg(target_os = "macos")]
impl Render for Helper {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let time = self.started.elapsed().as_secs_f32();
        // A slow breath on the dots behind the icon, so the eye finds it.
        // Settles after a few seconds so it doesn't repaint forever.
        let glow = if time < 6.0 {
            window.request_animation_frame();
            0.6 + 0.4 * (time * 2.4).sin().abs()
        } else {
            0.8
        };
        let tile = treemap::Tile {
            tone: Tone::Green,
            on: true,
            glow,
            reveal: 1.0,
            busy: false,
            clear: 0.62,
        };
        div()
            .size_full()
            .font_family(crate::FONT)
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .text_size(px(13.0))
            .p(px(6.0))
            .child(
                div()
                    .relative()
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                treemap::paint(&tile, bounds, bounds.origin, time, window)
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        h_flex()
                            .absolute()
                            .size_full()
                            .pl_4()
                            .pr_3()
                            .gap_3()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("Drag Beautiful Disk into the list"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(MUTED))
                                            .child("Then switch it on."),
                                    ),
                            )
                            .child(
                                div()
                                    .id("drag-app")
                                    .flex_none()
                                    .cursor_grab()
                                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                                        crate::grant::drag_app()
                                    })
                                    .child(img(self.icon.clone()).size(px(52.0))),
                            ),
                    ),
            )
    }
}
