use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use dither_gpui::dither_core::Tone;
use dither_gpui::dither_core::ease::approach;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::bar::{self, Bar, Particle, Rng, Segment};
use crate::disk::{self, Category, Cleanable, Progress, Report, Volume};

const BG: u32 = 0x161618;
const FG: u32 = 0xededef;
const MUTED: u32 = 0x8b8b90;
const LINE: u32 = 0x26262a;
const OK: u32 = 0x4ccb76;

/// Below this a category is not worth offering to clear.
const WORTH_CLEARING: u64 = 500_000_000;

fn tone(category: Category) -> Tone {
    match category {
        Category::Apps => Tone::Blue,
        Category::Developer => Tone::Green,
        Category::Photos => Tone::Pink,
        Category::Movies => Tone::Purple,
        Category::Documents => Tone::Hue(205.0),
        Category::Music => Tone::Hue(95.0),
        Category::System => Tone::Grey,
        Category::Caches => Tone::Orange,
        Category::Trash => Tone::Red,
    }
}

fn icon(category: Category) -> IconName {
    match category {
        Category::Apps => IconName::AppWindow,
        Category::Developer => IconName::SquareTerminal,
        Category::Photos => IconName::Image,
        Category::Movies => IconName::Film,
        Category::Documents => IconName::FileText,
        Category::Music => IconName::Music,
        Category::System => IconName::Settings,
        Category::Caches => IconName::Package,
        Category::Trash => IconName::Trash,
    }
}

fn gb(bytes: f64) -> String {
    format!("{:.1} GB", bytes / 1e9)
}

/// Disk capacity reads like a product spec: "512 GB", "1 TB".
fn capacity(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{} TB", (gb / 100.0).round() / 10.0)
    } else {
        format!("{} GB", gb.round())
    }
}

fn tilde(path: &str) -> String {
    let home = disk::home();
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(rest) if home != "/" => format!("~{rest}"),
        _ => path.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Scanning,
    Idle,
    Clearing(Cleanable),
}

pub struct Card {
    demo: bool,
    volume: Volume,
    progress: Arc<Progress>,
    phase: Phase,
    report: Report,
    /// Bytes per category: where the worker says they are, and what is on screen.
    target: [f64; 9],
    shown: [f64; 9],
    glow: [f32; 9],
    edge_strength: f32,
    hover: Option<Category>,
    cleared: Vec<Cleanable>,
    /// Size of the category being cleared when clearing started.
    clearing_from: u64,
    freed: u64,
    particles: Vec<Particle>,
    rng: Rng,
    time: f32,
    last_frame: Instant,
    /// Whether the previous frame asked for another. If not, the time since
    /// it is idle time, not animation time.
    animating: bool,
    bar_bounds: Rc<Cell<Bounds<Pixels>>>,
    legend_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The legend's hover highlight glides between rows: x, y, width relative
    /// to the legend, and opacity.
    pill: [f32; 3],
    pill_velocity: [f32; 3],
    pill_alpha: f32,
}

/// Legend geometry, shared by the layout and the gliding highlight.
const ROW_H: f32 = 22.0;
const ROW_GAP: f32 = 2.0;
const COL_GAP: f32 = 12.0;

/// Stiffness of the highlight's glide. Settles in about 0.2s.
const PILL_OMEGA: f32 = 30.0;

/// One step of a critically damped spring, solved exactly so any frame time
/// is stable. `x` is the offset from the target. Keeping velocity across
/// steps means a new target mid-glide bends the path instead of jerking it.
fn spring(x: f32, v: f32, dt: f32) -> (f32, f32) {
    let w = PILL_OMEGA;
    let c = v + w * x;
    let e = (-w * dt).exp();
    ((x + c * dt) * e, (v - w * c * dt) * e)
}

impl Card {
    pub fn new(demo: bool) -> Self {
        let mut card = Self {
            demo,
            volume: Volume { total: 0, used: 0 },
            progress: Arc::default(),
            phase: Phase::Idle,
            report: Report::default(),
            target: [0.0; 9],
            shown: [0.0; 9],
            glow: [1.0; 9],
            edge_strength: 0.0,
            hover: None,
            cleared: Vec::new(),
            clearing_from: 0,
            freed: 0,
            particles: Vec::new(),
            rng: Rng::new(),
            time: 0.0,
            last_frame: Instant::now(),
            animating: false,
            bar_bounds: Rc::default(),
            legend_bounds: Rc::default(),
            pill: [0.0; 3],
            pill_velocity: [0.0; 3],
            pill_alpha: 0.0,
        };
        card.scan();
        card
    }

    fn scan(&mut self) {
        self.volume = if self.demo {
            disk::demo::VOLUME
        } else {
            disk::volume()
        };
        self.progress = Arc::default();
        self.phase = Phase::Scanning;
        self.report = Report::default();
        self.target = [0.0; 9];
        self.shown = [0.0; 9];
        self.hover = None;
        self.cleared.clear();
        self.freed = 0;
        if self.demo {
            disk::demo::scan(self.progress.clone());
        } else {
            disk::scan(self.progress.clone());
        }
    }

    fn clear(&mut self, target: Cleanable) {
        self.phase = Phase::Clearing(target);
        self.clearing_from = self.progress.bytes(target.category());
        self.hover = None;
        let progress = self.progress.clone();
        if self.demo {
            disk::demo::clean(target, progress);
        } else {
            disk::clean(target, progress);
        }
    }

    fn suggestion(&self) -> Option<Cleanable> {
        if let Phase::Clearing(target) = self.phase {
            return Some(target);
        }
        Cleanable::ALL.into_iter().find(|c| {
            !self.cleared.contains(c) && self.progress.bytes(c.category()) >= WORTH_CLEARING
        })
    }

    fn used_shown(&self) -> f64 {
        self.shown.iter().sum()
    }

    /// Samples the worker, eases every animated value toward its target and
    /// says whether another frame is needed.
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        // The first frame after idle (a hover starting a glide) would see
        // seconds of elapsed time; use one display frame so it starts smooth.
        let dt = if self.animating {
            (now - self.last_frame).as_secs_f32().min(1.0 / 20.0)
        } else {
            1.0 / 120.0
        };
        self.last_frame = now;
        self.time += dt;

        let working = self.phase != Phase::Idle;
        if working {
            for c in Category::ALL {
                if c != Category::System {
                    self.target[c as usize] = self.progress.bytes(c) as f64;
                }
            }
            self.freed = self.progress.freed();
        }
        if self.progress.is_done() {
            match self.phase {
                Phase::Scanning => {
                    self.report = self.progress.report();
                    let known: f64 = self.target.iter().sum();
                    self.target[Category::System as usize] =
                        (self.volume.used as f64 - known).max(0.0);
                    self.phase = Phase::Idle;
                }
                Phase::Clearing(target) => {
                    self.cleared.push(target);
                    self.phase = Phase::Idle;
                }
                Phase::Idle => {}
            }
        }

        let mut moving = false;
        for i in 0..9 {
            let (from, to) = (self.shown[i], self.target[i]);
            self.shown[i] = approach(from as f32, to as f32, dt, 0.09) as f64;
            if (self.shown[i] - to).abs() < 5e6 {
                self.shown[i] = to;
            } else {
                moving = true;
            }

            let category = Category::ALL[i];
            let glow_to = match (self.phase, self.hover) {
                (Phase::Idle, Some(h)) if h == category => 1.0,
                (Phase::Idle, Some(_)) => 0.15,
                (Phase::Idle, None) => 0.45,
                _ => 1.0,
            };
            self.glow[i] = approach(self.glow[i], glow_to, dt, 0.07);
            if (self.glow[i] - glow_to).abs() > 0.005 {
                moving = true;
            } else {
                self.glow[i] = glow_to;
            }
        }

        // Gliding highlight: slide to the hovered row; when nothing is
        // hovered, fade out in place. Appearing from nothing jumps there
        // first so it never slides in from a stale spot.
        let legend_w = f32::from(self.legend_bounds.get().size.width);
        let hovered = self
            .hover
            .filter(|_| self.phase == Phase::Idle && legend_w > 0.0);
        if let Some(c) = hovered {
            let i = c as usize;
            let w = (legend_w - 2.0 * COL_GAP) / 3.0;
            let to = [
                (i % 3) as f32 * (w + COL_GAP),
                (i / 3) as f32 * (ROW_H + ROW_GAP),
                w,
            ];
            for k in 0..3 {
                if self.pill_alpha < 0.02 {
                    self.pill[k] = to[k];
                    self.pill_velocity[k] = 0.0;
                    continue;
                }
                let (x, v) = spring(self.pill[k] - to[k], self.pill_velocity[k], dt);
                if x.abs() > 0.05 || v.abs() > 1.0 {
                    self.pill[k] = to[k] + x;
                    self.pill_velocity[k] = v;
                    moving = true;
                } else {
                    self.pill[k] = to[k];
                    self.pill_velocity[k] = 0.0;
                }
            }
        }
        let alpha_to = if hovered.is_some() { 1.0 } else { 0.0 };
        self.pill_alpha = approach(self.pill_alpha, alpha_to, dt, 0.06);
        if (self.pill_alpha - alpha_to).abs() > 0.005 {
            moving = true;
        } else {
            self.pill_alpha = alpha_to;
        }

        let edge_to = if working { 1.0 } else { 0.0 };
        self.edge_strength = approach(self.edge_strength, edge_to, dt, 0.12);
        if (self.edge_strength - edge_to).abs() > 0.005 {
            moving = true;
        }

        let bounds = self.bar_bounds.get();
        let width = f32::from(bounds.size.width);
        let emit = match self.phase {
            Phase::Clearing(target) if width > 0.0 => {
                let (_, end) = self.span(target.category());
                Some((
                    end * width,
                    f32::from(bounds.size.height),
                    tone(target.category()),
                ))
            }
            _ => None,
        };
        bar::step_particles(&mut self.particles, &mut self.rng, dt, emit, 260.0);

        working || moving || !self.particles.is_empty()
    }

    /// Start and end of a category's segment as fractions of the bar.
    fn span(&self, category: Category) -> (f32, f32) {
        let total = self.volume.total.max(1) as f64;
        let i = category as usize;
        let start: f64 = self.shown[..i].iter().sum();
        (
            (start / total) as f32,
            ((start + self.shown[i]) / total) as f32,
        )
    }

    fn bar(&self) -> Bar {
        let segments = Category::ALL
            .iter()
            .map(|&c| {
                let (start, end) = self.span(c);
                Segment {
                    tone: tone(c),
                    start,
                    end,
                    glow: self.glow[c as usize],
                }
            })
            .collect();
        let edge = match self.phase {
            Phase::Clearing(target) => Some(target.category() as usize),
            _ => (0..9).rev().find(|&i| self.shown[i] > 0.0),
        };
        Bar {
            segments,
            edge,
            edge_strength: self.edge_strength,
            time: self.time,
        }
    }

    fn category_at(&self, x: Pixels) -> Option<Category> {
        let bounds = self.bar_bounds.get();
        let f = f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0);
        Category::ALL.into_iter().find(|&c| {
            let (start, end) = self.span(c);
            f >= start && f < end
        })
    }

    fn detail(&self, category: Category) -> Option<String> {
        let r = &self.report;
        let i = category as usize;
        let largest_file = || {
            r.largest_file[i].as_ref().map(|(name, bytes)| {
                format!("Your largest file is {name} at {}.", gb(*bytes as f64))
            })
        };
        match category {
            Category::Apps => r.largest_item[i].as_ref().map(|(name, bytes)| {
                let name = name.strip_suffix(".app").unwrap_or(name);
                format!("{name} is the largest at {}.", gb(*bytes as f64))
            }),
            Category::Developer => {
                let mut parts = Vec::new();
                if r.simulators >= 100_000_000 {
                    parts.push(format!("Simulators take {}.", gb(r.simulators as f64)));
                }
                if r.build_output >= 100_000_000 {
                    parts.push(format!("Build output takes {}.", gb(r.build_output as f64)));
                }
                if parts.is_empty() {
                    r.largest_item[i].as_ref().map(|(name, bytes)| {
                        format!("Most of it is {name} at {}.", gb(*bytes as f64))
                    })
                } else {
                    Some(parts.join(" "))
                }
            }
            Category::Photos | Category::Movies | Category::Documents | Category::Music => {
                largest_file()
            }
            Category::System => Some("macOS, app data and files outside your folders.".into()),
            Category::Caches => Some("Apps rebuild these when they need them again.".into()),
            Category::Trash => Some("Things you've already deleted.".into()),
        }
    }

    fn status_line(&self) -> AnyElement {
        let muted = rgb(MUTED);
        let path = |label: &'static str| {
            h_flex()
                .gap_2()
                .min_w_0()
                .child(div().flex_none().text_color(rgb(FG)).child(label))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family("Menlo")
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child(tilde(&self.progress.current())),
                )
                .into_any_element()
        };
        match self.phase {
            Phase::Scanning => return path("Calculating"),
            Phase::Clearing(Cleanable::Caches) => return path("Clearing caches"),
            Phase::Clearing(Cleanable::Trash) => return path("Emptying the Trash"),
            Phase::Idle => {}
        }
        let available = gb(self.volume.total as f64 - self.used_shown());
        if let Some(c) = self.hover {
            return h_flex()
                .gap_2()
                .min_w_0()
                .child(
                    Icon::new(icon(c))
                        .size_3p5()
                        .text_color(dither_gpui::hsla(tone(c).base())),
                )
                .child(div().flex_none().text_color(rgb(FG)).child(c.name()))
                .child(
                    div()
                        .flex_none()
                        .text_color(muted)
                        .child(gb(self.shown[c as usize])),
                )
                .children(
                    self.detail(c)
                        .map(|d| div().min_w_0().truncate().text_color(muted).child(d)),
                )
                .into_any_element();
        }
        if self.freed > 0 {
            return h_flex()
                .gap_1p5()
                .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                .child(
                    div()
                        .text_color(rgb(FG))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("Freed {}.", gb(self.freed as f64))),
                )
                .child(
                    div()
                        .text_color(muted)
                        .child(format!("{available} available")),
                )
                .into_any_element();
        }
        div()
            .text_color(muted)
            .child(format!("{available} available"))
            .into_any_element()
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = h_flex().h(px(30.0)).gap_2().items_center();
        if self.phase == Phase::Scanning {
            return row
                .child(Spinner::new().small().color(rgb(MUTED).into()))
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Looking for things you can clear"),
                )
                .into_any_element();
        }
        let Some(target) = self.suggestion() else {
            return row
                .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Nothing else is worth clearing right now."),
                )
                .into_any_element();
        };
        let category = target.category();
        // While clearing, keep the sentence about what was there when it started.
        let bytes = gb(match self.phase {
            Phase::Clearing(_) => self.clearing_from,
            _ => self.progress.bytes(category),
        } as f64);
        let (text, action) = match target {
            Cleanable::Caches => (
                format!("Caches are using {bytes}. Apps rebuild them when they need them."),
                "Clear caches",
            ),
            Cleanable::Trash => (
                format!("The Trash is holding {bytes} of things you've already deleted."),
                "Empty Trash",
            ),
        };
        row.child(
            Icon::new(icon(category))
                .size_3p5()
                .text_color(dither_gpui::hsla(tone(category).base())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(FG))
                .child(text),
        )
        .child(
            Button::new("clean")
                .small()
                .label(action)
                .disabled(self.phase != Phase::Idle)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.clear(target);
                    cx.notify();
                })),
        )
        .into_any_element()
    }
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.animating = self.tick();
        if self.animating {
            window.request_animation_frame();
        }
        let idle = self.phase == Phase::Idle;

        let header = h_flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_size(px(26.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(gb(self.used_shown())),
                            )
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(MUTED))
                                    .child(format!("of {} used", capacity(self.volume.total))),
                            ),
                    )
                    .child(div().h(px(18.0)).child(self.status_line())),
            )
            .child(
                Button::new("rescan")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label("Rescan")
                    .disabled(!idle)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scan();
                        cx.notify();
                    })),
            );

        let model = self.bar();
        let particles = self.particles.clone();
        let bar_bounds = self.bar_bounds.clone();
        const DUST: f32 = 80.0;
        let bar = div()
            .id("bar")
            .relative()
            .h(px(54.0))
            .w_full()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.phase != Phase::Idle || !this.bar_bounds.get().contains(&e.position) {
                    return;
                }
                let hover = this.category_at(e.position.x);
                if hover != this.hover {
                    this.hover = hover;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.hover = None;
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| bar_bounds.set(bounds),
                    move |bounds, _, window, _| bar::paint(&model, bounds, window),
                )
                .absolute()
                .size_full(),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let origin = bounds.origin + point(px(0.0), px(DUST));
                        bar::paint_particles(&particles, origin, window);
                    },
                )
                .absolute()
                .left_0()
                .right_0()
                .top(px(-DUST))
                .h(px(54.0 + DUST)),
            );

        let legend_bounds = self.legend_bounds.clone();
        let [pill_x, pill_y, pill_w] = self.pill;
        let pill = div()
            .absolute()
            .left(px(pill_x))
            .top(px(pill_y))
            .w(px(pill_w))
            .h(px(ROW_H))
            .rounded(px(6.0))
            .bg(rgb(0x232327))
            .opacity(self.pill_alpha);
        let rows = Category::ALL.chunks(3).map(|row| {
            h_flex().gap(px(COL_GAP)).children(row.iter().map(|&c| {
                h_flex()
                    .id(c.name())
                    .flex_1()
                    .h(px(ROW_H))
                    .px_1p5()
                    .gap_2()
                    .rounded(px(6.0))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if this.phase != Phase::Idle {
                            return;
                        }
                        if *hovered {
                            this.hover = Some(c);
                        } else if this.hover == Some(c) {
                            this.hover = None;
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(icon(c))
                            .size_3p5()
                            .text_color(dither_gpui::hsla(tone(c).base())),
                    )
                    .child(div().flex_1().text_color(rgb(FG)).child(c.name()))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(MUTED))
                            .child(gb(self.shown[c as usize])),
                    )
            }))
        });
        let legend = v_flex()
            .relative()
            .gap(px(ROW_GAP))
            .child(pill)
            .child(
                canvas(
                    move |bounds, _, _| legend_bounds.set(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(rows);

        v_flex()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .text_size(px(13.0))
            .px_5()
            .pt_5()
            .pb_3()
            .gap_4()
            .child(header)
            .child(bar)
            .child(legend)
            .child(
                v_flex()
                    .gap_3()
                    .child(div().h(px(1.0)).bg(rgb(LINE)))
                    .child(self.footer(cx)),
            )
    }
}
